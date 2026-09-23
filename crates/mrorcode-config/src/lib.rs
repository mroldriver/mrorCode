//! mrorcode 配置模型与加载（leaf crate）。
//!
//! 优先级（高 → 低）：环境变量 > `$MRORCODE_CONFIG` 指定文件 > `~/.mrorcode/config.toml`。
//! 文件缺失不是错误；文件损坏产生诊断警告而不是 panic。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 配置文件根。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// providers 表中默认使用的条目名；缺省取表中第一个。
    pub default_provider: Option<String>,
    /// 默认模型（覆盖 provider 内 model）。
    pub default_model: Option<String>,
    pub providers: HashMap<String, ProviderConfig>,
}

/// 单个 provider 配置。M1 仅 `openai_compat` 一种类型。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// 协议类型，缺省 "openai_compat"。
    #[serde(rename = "type")]
    pub provider_type: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub context_window: Option<u32>,
}

/// 加载结果（带诊断）。
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub config: Config,
    /// 实际读取的配置文件路径（若存在）。
    pub path: Option<PathBuf>,
    /// 非致命诊断（如损坏的 TOML 段）。
    pub warnings: Vec<String>,
}

/// 解析后的可用 provider（装配层直接消费）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub context_window: u32,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no provider configured: create {path} with a [providers.*] section or set MRORCODE_BASE_URL")]
    NoProvider { path: String },
    #[error("provider '{name}' is missing required field: {field}")]
    MissingField { name: String, field: &'static str },
    #[error("unknown provider type '{0}' (supported: openai_compat)")]
    UnknownProviderType(String),
}

impl Config {
    /// 默认配置目录：`~/.mrorcode`。
    pub fn config_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".mrorcode"))
    }

    pub fn config_path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("config.toml"))
    }

    /// 标准加载流程（文件 + 进程环境变量）。
    pub fn load() -> Loaded {
        let path = std::env::var("MRORCODE_CONFIG")
            .ok()
            .map(PathBuf::from)
            .or_else(Self::config_path);
        let mut loaded = Self::load_from(path.as_deref());
        loaded.config.apply_environment(std::env::vars());
        loaded
    }

    /// 仅从文件加载（不读环境变量），测试友好。
    pub fn load_from(path: Option<&Path>) -> Loaded {
        let mut warnings = Vec::new();
        let (config, resolved_path) = match path {
            Some(p) if p.exists() => {
                let text = match std::fs::read_to_string(p) {
                    Ok(t) => t,
                    Err(e) => {
                        warnings.push(format!("failed to read {}: {e}", p.display()));
                        return Loaded {
                            config: Config::default(),
                            path: Some(p.to_path_buf()),
                            warnings,
                        };
                    }
                };
                match toml::from_str::<Config>(&text) {
                    Ok(cfg) => (cfg, Some(p.to_path_buf())),
                    Err(e) => {
                        warnings.push(format!(
                            "failed to parse {}: {e} (continuing with empty config)",
                            p.display()
                        ));
                        (Config::default(), Some(p.to_path_buf()))
                    }
                }
            }
            _ => (Config::default(), None),
        };
        Loaded {
            config,
            path: resolved_path,
            warnings,
        }
    }

    /// 应用环境变量覆盖（注入式接口，便于测试）：
    /// `MRORCODE_BASE_URL` / `MRORCODE_API_KEY` / `MRORCODE_MODEL` /
    /// `MRORCODE_PROVIDER`（条目名）。
    pub fn apply_environment(&mut self, vars: impl Iterator<Item = (String, String)>) {
        let mut base_url = None;
        let mut api_key = None;
        let mut model = None;
        let mut provider_name = None;
        for (k, v) in vars {
            match k.as_str() {
                "MRORCODE_BASE_URL" => base_url = Some(v),
                "MRORCODE_API_KEY" => api_key = Some(v),
                "MRORCODE_MODEL" => model = Some(v),
                "MRORCODE_PROVIDER" => provider_name = Some(v),
                _ => {}
            }
        }
        if model.is_some() {
            self.default_model = model.clone();
        }
        if provider_name.is_some() {
            self.default_provider = provider_name;
        }
        if base_url.is_none() && api_key.is_none() {
            return;
        }
        let name = self
            .default_provider
            .clone()
            .or_else(|| self.providers.keys().next().cloned())
            .unwrap_or_else(|| "default".to_string());
        let entry = self.providers.entry(name.clone()).or_default();
        if entry.provider_type.is_none() {
            entry.provider_type = Some("openai_compat".to_string());
        }
        if let Some(v) = base_url {
            entry.base_url = Some(v);
        }
        if let Some(v) = api_key {
            entry.api_key = Some(v);
        }
        self.default_provider = Some(name);
    }

    /// 选中的 provider 条目。
    pub fn active_provider(&self) -> Option<(&str, &ProviderConfig)> {
        let name = self
            .default_provider
            .as_deref()
            .or_else(|| self.providers.keys().next().map(String::as_str))?;
        self.providers
            .get_key_value(name)
            .map(|(k, v)| (k.as_str(), v))
    }

    /// 校验并解析为可装配的 provider。本地端点允许空 api_key。
    pub fn resolve(&self) -> Result<ResolvedProvider, ConfigError> {
        let (name, cfg) = self
            .active_provider()
            .ok_or_else(|| ConfigError::NoProvider {
                path: Self::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.mrorcode/config.toml".to_string()),
            })?;
        let provider_type = cfg
            .provider_type
            .clone()
            .unwrap_or_else(|| "openai_compat".to_string());
        if provider_type != "openai_compat" {
            return Err(ConfigError::UnknownProviderType(provider_type));
        }
        let base_url = cfg
            .base_url
            .clone()
            .ok_or_else(|| ConfigError::MissingField {
                name: name.to_string(),
                field: "base_url",
            })?;
        let model = self
            .default_model
            .clone()
            .or_else(|| cfg.model.clone())
            .ok_or_else(|| ConfigError::MissingField {
                name: name.to_string(),
                field: "model",
            })?;
        Ok(ResolvedProvider {
            name: name.to_string(),
            provider_type,
            base_url,
            api_key: cfg.api_key.clone().unwrap_or_default(),
            model,
            context_window: cfg.context_window.unwrap_or(128_000),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_defaults_and_parse() {
        let cfg: Config = toml::from_str(
            r#"
default_model = "deepseek-chat"

[providers.glm]
type = "openai_compat"
base_url = "https://example.com/v1"
api_key = "secret"
model = "glm-4"
context_window = 200000
"#,
        )
        .unwrap();
        assert_eq!(cfg.default_model.as_deref(), Some("deepseek-chat"));
        let glm = cfg.providers.get("glm").unwrap();
        assert_eq!(glm.base_url.as_deref(), Some("https://example.com/v1"));
        assert_eq!(glm.context_window, Some(200_000));
    }

    #[test]
    fn broken_toml_produces_warning_not_panic() {
        let dir = std::env::temp_dir().join(format!("mrorcode-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is = = not toml [").unwrap();

        let loaded = Config::load_from(Some(&path));
        assert!(loaded.path.is_some());
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.config.providers.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn env_overrides_create_and_fill_provider() {
        let mut cfg = Config::default();
        cfg.apply_environment(
            [
                (
                    "MRORCODE_BASE_URL".to_string(),
                    "http://localhost:11434/v1".to_string(),
                ),
                ("MRORCODE_API_KEY".to_string(), "k".to_string()),
                ("MRORCODE_MODEL".to_string(), "local-model".to_string()),
            ]
            .into_iter(),
        );
        let resolved = cfg.resolve().unwrap();
        assert_eq!(resolved.base_url, "http://localhost:11434/v1");
        assert_eq!(resolved.model, "local-model");
        assert_eq!(resolved.name, "default");
        assert_eq!(resolved.provider_type, "openai_compat");
    }

    #[test]
    fn missing_config_errors_with_hint() {
        let cfg = Config::default();
        assert!(matches!(cfg.resolve(), Err(ConfigError::NoProvider { .. })));
    }

    #[test]
    fn unknown_provider_type_rejected() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "x".into(),
            ProviderConfig {
                provider_type: Some("anthropic".into()),
                base_url: Some("u".into()),
                api_key: None,
                model: Some("m".into()),
                context_window: None,
            },
        );
        cfg.default_provider = Some("x".into());
        assert!(matches!(
            cfg.resolve(),
            Err(ConfigError::UnknownProviderType(t)) if t == "anthropic"
        ));
    }
}
