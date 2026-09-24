//! 会话磁盘持久化：`~/.mrorcode/sessions/<id>.jsonl` + `<id>.snapshot.json`。
//!
//! - transcript（jsonl）：追加写、不压缩，用于 recall；解析失败的行跳过并 warning。
//! - snapshot（json）：原子覆盖写（tmp→rename），用于快速 resume。
//!
//! 加载优先读 snapshot；缺失则从 jsonl 重建（`SessionRecord.conversation`）。

use std::io::Write;
use std::path::{Path, PathBuf};

use mrorcode_kernel::message::Conversation;
use mrorcode_kernel::session::{derive_title, SessionRecord, SessionSummary, TranscriptEntry};

/// transcript/snapshot 所在目录名。
pub const SESSIONS_DIR: &str = "sessions";

/// 会话文件存储。
#[derive(Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    /// 标准位置：`<home>/.mrorcode/sessions`。
    pub fn standard() -> Option<Self> {
        dirs::home_dir().map(|h| Self {
            root: h.join(".mrorcode").join(SESSIONS_DIR),
        })
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn transcript_path(&self, id: u64) -> PathBuf {
        self.root.join(format!("{id}.jsonl"))
    }

    fn snapshot_path(&self, id: u64) -> PathBuf {
        self.root.join(format!("{id}.snapshot.json"))
    }

    /// 追加一条 transcript 行（每轮终态调用，不参与流式热路径）。
    pub fn append_transcript(&self, id: u64, entry: &TranscriptEntry) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.transcript_path(id))?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// 追加整轮全部消息。
    pub fn append_conversation_tail(
        &self,
        id: u64,
        messages: &[mrorcode_kernel::message::Message],
    ) -> std::io::Result<()> {
        for message in messages {
            for entry in TranscriptEntry::from_message(message) {
                self.append_transcript(id, &entry)?;
            }
        }
        Ok(())
    }

    /// 原子写入快照（tmp → rename）。
    pub fn save_snapshot(&self, record: &SessionRecord) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let json = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
        let final_path = self.snapshot_path(record.id);
        let tmp = final_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &final_path)?;
        Ok(())
    }

    /// 加载会话：优先 snapshot，缺失则从 jsonl 重建（用 `id`/`title` 兜底）。
    pub fn load(&self, id: u64) -> Option<SessionRecord> {
        if let Ok(text) = std::fs::read_to_string(self.snapshot_path(id)) {
            if let Ok(mut record) = serde_json::from_str::<SessionRecord>(&text) {
                record.conversation.repair_pairing();
                return Some(record);
            }
        }
        self.rebuild_from_transcript(id)
    }

    fn rebuild_from_transcript(&self, id: u64) -> Option<SessionRecord> {
        let text = std::fs::read_to_string(self.transcript_path(id)).ok()?;
        let mut conversation = Conversation::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<TranscriptEntry>(line) {
                Ok(entry) => {
                    if let Some(msg) = entry.to_message() {
                        conversation.push(msg);
                    }
                }
                Err(e) => {
                    tracing::warn!("skipping corrupt transcript line: {e}");
                }
            }
        }
        if conversation.messages.is_empty() {
            return None;
        }
        conversation.repair_pairing();
        Some(SessionRecord {
            id,
            title: derive_title(&conversation),
            updated_at: file_mtime_secs(&self.transcript_path(id)),
            conversation,
        })
    }

    /// 列出全部会话摘要（按更新时间倒序）。
    pub fn list(&self) -> Vec<SessionSummary> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return out,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let id = if let Some(rest) = name.strip_suffix(".snapshot.json") {
                rest.parse::<u64>().ok()
            } else if let Some(rest) = name.strip_suffix(".jsonl") {
                rest.parse::<u64>().ok()
            } else {
                None
            };
            let Some(id) = id else { continue };
            if let Some(record) = self.load(id) {
                out.push(SessionSummary::from_record(&record));
            }
        }
        // 同一 id 的 snapshot 与 jsonl 会各命中一次，去重按 id
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        out.dedup_by_key(|s| s.id);
        out
    }

    /// 删除会话（transcript + snapshot）。
    pub fn delete(&self, id: u64) -> std::io::Result<()> {
        let t = self.transcript_path(id);
        if t.exists() {
            std::fs::remove_file(t)?;
        }
        let s = self.snapshot_path(id);
        if s.exists() {
            std::fs::remove_file(s)?;
        }
        Ok(())
    }
}

fn file_mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 当前 Unix 秒。
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mrorcode_kernel::message::Message;
    use mrorcode_kernel::session::TranscriptEntry;

    fn temp_store(tag: &str) -> SessionStore {
        let root =
            std::env::temp_dir().join(format!("mrorcode-store-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        SessionStore::with_root(root)
    }

    fn seed(store: &SessionStore, id: u64) {
        let mut convo = Conversation::new();
        convo.push(Message::user("hello"));
        convo.push(Message::assistant_text("world"));
        store.append_conversation_tail(id, &convo.messages).unwrap();
        store
            .save_snapshot(&SessionRecord {
                id,
                title: "hello".into(),
                updated_at: 100,
                conversation: convo,
            })
            .unwrap();
    }

    #[test]
    fn snapshot_roundtrip() {
        let store = temp_store("snap");
        seed(&store, 1);
        let loaded = store.load(1).unwrap();
        assert_eq!(loaded.title, "hello");
        assert_eq!(loaded.conversation.messages.len(), 2);
        assert_eq!(loaded.conversation.messages[0].text, "hello");
    }

    #[test]
    fn rebuild_from_transcript_when_snapshot_missing() {
        let store = temp_store("rebuild");
        store
            .append_transcript(7, &TranscriptEntry::User { text: "q".into() })
            .unwrap();
        store
            .append_transcript(
                7,
                &TranscriptEntry::Assistant {
                    text: "a".into(),
                    reasoning: None,
                },
            )
            .unwrap();
        let loaded = store.load(7).unwrap();
        assert_eq!(loaded.conversation.messages.len(), 2);
        assert_eq!(loaded.title, "q");
    }

    #[test]
    fn corrupt_transcript_lines_are_skipped() {
        let store = temp_store("corrupt");
        store
            .append_transcript(9, &TranscriptEntry::User { text: "ok".into() })
            .unwrap();
        // 手工追加坏行
        let path = store.transcript_path(9);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{{ not json").unwrap();
        let loaded = store.load(9).unwrap();
        assert_eq!(loaded.conversation.messages.len(), 1);
    }

    #[test]
    fn list_and_delete() {
        let store = temp_store("list");
        seed(&store, 1);
        seed(&store, 2);
        let mut ids: Vec<u64> = store.list().into_iter().map(|s| s.id).collect();
        ids.sort();
        assert_eq!(ids, vec![1, 2]);
        store.delete(1).unwrap();
        assert_eq!(store.list().len(), 1);
        assert!(store.load(1).is_none());
    }
}
