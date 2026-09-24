//! TUI 本地偏好存储（KV）：`~/.mrorcode/state.json`。
//!
//! 单进程内同步读写（文件 < 4KB）；写入走 临时文件 + rename 原子替换。
//! 所有字段 serde default，旧版本/损坏文件回退默认值不崩溃。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 提示历史上限。
pub const HISTORY_CAP: usize = 50;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeybindEntry {
    /// 键位序列，如 `"ctrl+p"` / `"f2"` / `"leader n"`。
    pub keys: String,
    /// 动作标识（keymap 模块定义其枚举与字符串的映射）。
    pub action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KVData {
    /// 主题名（文件名去 .json）。
    pub theme: Option<String>,
    /// dark / light。
    pub theme_mode: String,
    pub sidebar: Option<bool>,
    pub model: Option<String>,
    pub keybinds: Vec<KeybindEntry>,
    pub prompt_history: Vec<String>,
}

impl Default for KVData {
    fn default() -> Self {
        Self {
            theme: None,
            theme_mode: "dark".to_string(),
            sidebar: Some(true),
            model: None,
            keybinds: Vec::new(),
            prompt_history: Vec::new(),
        }
    }
}

/// 偏好存储：持有路径与数据，`save()` 原子落盘。
pub struct Store {
    path: PathBuf,
    pub data: KVData,
}

impl Store {
    pub fn path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".mrorcode").join("state.json"))
    }

    /// 从标准路径加载；文件不存在是正常情况。
    pub fn load() -> (Self, Vec<String>) {
        match Self::path() {
            Some(p) => Self::load_from(&p),
            None => (
                Self {
                    path: PathBuf::from("state.json"),
                    data: KVData::default(),
                },
                vec!["home directory unavailable; preferences not persisted".to_string()],
            ),
        }
    }

    pub fn load_from(path: &Path) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        if !path.exists() {
            return (
                Self {
                    path: path.to_path_buf(),
                    data: KVData::default(),
                },
                warnings,
            );
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                return (
                    Self {
                        path: path.to_path_buf(),
                        data: KVData::default(),
                    },
                    vec![format!("cannot read {}: {e}", path.display())],
                );
            }
        };
        match serde_json::from_str::<KVData>(&text) {
            Ok(data) => (
                Self {
                    path: path.to_path_buf(),
                    data,
                },
                warnings,
            ),
            Err(e) => {
                warnings.push(format!(
                    "corrupt state {}: {e} (using defaults; old file left in place)",
                    path.display()
                ));
                (
                    Self {
                        path: path.to_path_buf(),
                        data: KVData::default(),
                    },
                    warnings,
                )
            }
        }
    }

    /// 原子写入：同目录临时文件 → rename。
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&self.data).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// 追加一条提示历史（去重后置顶，上限 HISTORY_CAP）。
    pub fn push_history(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.data.prompt_history.retain(|h| h != text);
        self.data.prompt_history.push(text.to_string());
        if self.data.prompt_history.len() > HISTORY_CAP {
            let drop_n = self.data.prompt_history.len() - HISTORY_CAP;
            self.data.prompt_history.drain(0..drop_n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mrorcode-kv-{}-{name}.json", std::process::id()))
    }

    #[test]
    fn missing_file_is_defaults() {
        let p = temp_path("missing");
        let _ = std::fs::remove_file(&p);
        let (store, warnings) = Store::load_from(&p);
        assert!(warnings.is_empty());
        assert_eq!(store.data.theme_mode, "dark");
        assert!(store.data.sidebar.unwrap());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let p = temp_path("roundtrip");
        let _ = std::fs::remove_file(&p);
        let (mut store, _) = Store::load_from(&p);
        store.data.theme = Some("nord".into());
        store.data.model = Some("m-1".into());
        store.push_history("first");
        store.save().unwrap();

        let (loaded, warnings) = Store::load_from(&p);
        assert!(warnings.is_empty());
        assert_eq!(loaded.data.theme.as_deref(), Some("nord"));
        assert_eq!(loaded.data.model.as_deref(), Some("m-1"));
        assert_eq!(loaded.data.prompt_history, vec!["first".to_string()]);

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn history_dedup_cap_and_order() {
        let p = temp_path("hist");
        let (mut store, _) = Store::load_from(&p);
        store.push_history("a");
        store.push_history("b");
        store.push_history("a"); // 去重置顶
        assert_eq!(store.data.prompt_history, vec!["b", "a"]);

        for i in 0..(HISTORY_CAP + 10) {
            store.push_history(&format!("x{i}"));
        }
        assert_eq!(store.data.prompt_history.len(), HISTORY_CAP);
        // 最新的在末尾，最旧的被淘汰
        assert_eq!(
            store.data.prompt_history.last().unwrap(),
            &format!("x{}", HISTORY_CAP + 9)
        );

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn corrupt_file_falls_back_without_panic() {
        let p = temp_path("corrupt");
        std::fs::write(&p, "{ not json").unwrap();
        let (store, warnings) = Store::load_from(&p);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("corrupt state"));
        assert_eq!(store.data.theme_mode, "dark");
        // 旧文件保留未动
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ not json");
        std::fs::remove_file(&p).ok();
    }
}
