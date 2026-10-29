//! read-before-write 追踪：write/edit 前必须已由 read 读过该文件。
//!
//! 共享于 read/write/edit 工具实例之间，记录「已读且未在读取后被外部修改」的路径。
//! 记录键为规范化路径；读取时附带 mtime+len 指纹，命中时校验指纹以防陈旧覆盖。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 一次成功读取的指纹。
#[derive(Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    mtime: Option<std::time::SystemTime>,
}

#[derive(Default)]
pub struct ReadTracker {
    seen: Mutex<HashMap<PathBuf, Fingerprint>>,
}

impl ReadTracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 记录一次成功读取。
    pub fn mark(&self, path: &Path) {
        if let Ok(meta) = std::fs::metadata(path) {
            let fp = Fingerprint {
                len: meta.len(),
                mtime: meta.modified().ok(),
            };
            self.seen.lock().unwrap().insert(path.to_path_buf(), fp);
        }
    }

    /// 是否已读且自读取后未被外部修改。
    pub fn is_fresh(&self, path: &Path) -> bool {
        let guard = self.seen.lock().unwrap();
        let Some(expected) = guard.get(path) else {
            return false;
        };
        match std::fs::metadata(path) {
            Ok(meta) => {
                let fp = Fingerprint {
                    len: meta.len(),
                    mtime: meta.modified().ok(),
                };
                fp == *expected
            }
            Err(_) => false,
        }
    }

    /// 写入后刷新指纹（保持后续读取可信）。
    pub fn mark_written(&self, path: &Path) {
        self.mark(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_read_and_detects_external_change() {
        let dir = std::env::temp_dir().join(format!("mrorcode-tracker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "one").unwrap();

        let t = ReadTracker::new();
        assert!(!t.is_fresh(&f));
        t.mark(&f);
        assert!(t.is_fresh(&f));

        // 外部改写（长度变化）使指纹失效
        std::fs::write(&f, "one two three").unwrap();
        assert!(!t.is_fresh(&f));

        std::fs::remove_dir_all(&dir).ok();
    }
}
