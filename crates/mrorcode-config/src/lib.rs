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
    /// 上下文压缩策略（M3）。
    pub compaction: CompactionConfig,
    /// MCP stdio servers（M3）。
    pub mcp: McpConfig,
}

/// 上下文压缩配置。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    /// 保留最近多少个 user turn 原文。
    pub keep_recent: usize,
    /// 估算 token 超过该值时压缩；0 = 关闭。
    pub threshold_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            keep_recent: 6,
            threshold_tokens: 100_000,
        }
    }
}

/// MCP 配置集合。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub servers: Vec<McpServer>,
}

/// 单个 stdio MCP server。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// 单个 provider 配置。M1 仅 `openai_compat`；M5-b 起支持 `anthropic`。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// 协议类型，缺省 "openai_compat"。
    #[serde(rename = "type", default)]
    pub provider_type: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub context_window: Option<u32>,
    /// 同 provider 下可切换的额外模型（M2 模型选择器数据源）。
    #[serde(default)]
    pub models: Vec<String>,
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
    /// 同 provider 下可切换的全部模型（含当前 model，去重保序）。
    pub models: Vec<String>,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no provider configured: create {path} with a [providers.*] section or set MRORCODE_BASE_URL")]
    NoProvider { path: String },
    #[error("provider '{name}' is missing required field: {field}")]
    MissingField { name: String, field: &'static str },
    #[error("unknown provider type '{0}' (supported: openai_compat, anthropic)")]
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
    /// `MRORCODE_PROVIDER`（条目名）/ `MRORCODE_ANTHROPIC_API_KEY`。
    pub fn apply_environment(&mut self, vars: impl Iterator<Item = (String, String)>) {
        let mut base_url = None;
        let mut api_key = None;
        let mut anthropic_key = None;
        let mut model = None;
        let mut provider_name = None;
        for (k, v) in vars {
            match k.as_str() {
                "MRORCODE_BASE_URL" => base_url = Some(v),
                "MRORCODE_API_KEY" => api_key = Some(v),
                "MRORCODE_ANTHROPIC_API_KEY" => anthropic_key = Some(v),
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
        if base_url.is_none() && api_key.is_none() && anthropic_key.is_none() {
            return;
        }
        let name = self
            .default_provider
            .clone()
            .or_else(|| self.providers.keys().next().cloned())
            .unwrap_or_else(|| "default".to_string());
        let entry = self.providers.entry(name.clone()).or_default();
        // anthropic key 优先写入；若无 type 且有 anthropic key 则标记为 anthropic
        if anthropic_key.is_some() && entry.provider_type.is_none() {
            entry.provider_type = Some("anthropic".to_string());
        }
        if entry.provider_type.is_none() {
            entry.provider_type = Some("openai_compat".to_string());
        }
        if let Some(v) = base_url {
            entry.base_url = Some(v);
        }
        if let Some(v) = api_key {
            entry.api_key = Some(v);
        }
        if let Some(v) = anthropic_key {
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
        let name = self
            .default_provider
            .clone()
            .or_else(|| self.providers.keys().next().cloned())
            .ok_or_else(|| ConfigError::NoProvider {
                path: Self::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.mrorcode/config.toml".to_string()),
            })?;
        let cfg = self
            .providers
            .get(&name)
            .ok_or_else(|| ConfigError::NoProvider {
                path: Self::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.mrorcode/config.toml".to_string()),
            })?;
        // 默认解析：顶层 default_model 可覆盖条目模型。
        let model_override = self.default_model.clone();
        resolve_entry(&name, cfg, model_override)
    }

    /// 按条目名解析指定 provider（运行中切换用，M5-e）。
    ///
    /// 与 `resolve()` 的区别：不看 `default_provider`，且顶层 `default_model`
    /// 不跨条目生效（只用该条目自己的 model）。
    pub fn resolve_named(&self, name: &str) -> Result<ResolvedProvider, ConfigError> {
        let cfg = self
            .providers
            .get(name)
            .ok_or_else(|| ConfigError::NoProvider {
                path: Self::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.mrorcode/config.toml".to_string()),
            })?;
        resolve_entry(name, cfg, None)
    }
}

/// 单个 provider 条目的校验与解析；`model_override` 为顶层 default_model（仅默认解析时传入）。
fn resolve_entry(
    name: &str,
    cfg: &ProviderConfig,
    model_override: Option<String>,
) -> Result<ResolvedProvider, ConfigError> {
    let provider_type = cfg
        .provider_type
        .clone()
        .unwrap_or_else(|| "openai_compat".to_string());
    match provider_type.as_str() {
        "openai_compat" | "anthropic" => {}
        other => return Err(ConfigError::UnknownProviderType(other.to_string())),
    }
    let base_url = cfg
        .base_url
        .clone()
        .ok_or_else(|| ConfigError::MissingField {
            name: name.to_string(),
            field: "base_url",
        })?;
    let model = model_override
        .or_else(|| cfg.model.clone())
        .ok_or_else(|| ConfigError::MissingField {
            name: name.to_string(),
            field: "model",
        })?;
    let mut models: Vec<String> = Vec::new();
    for m in std::iter::once(model.clone()).chain(cfg.models.iter().cloned()) {
        if !models.contains(&m) {
            models.push(m);
        }
    }
    Ok(ResolvedProvider {
        name: name.to_string(),
        provider_type,
        base_url,
        api_key: cfg.api_key.clone().unwrap_or_default(),
        model,
        context_window: cfg.context_window.unwrap_or(128_000),
        models,
    })
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
                provider_type: Some("billing".into()),
                base_url: Some("u".into()),
                api_key: None,
                model: Some("m".into()),
                context_window: None,
                models: Vec::new(),
            },
        );
        cfg.default_provider = Some("x".into());
        assert!(matches!(
            cfg.resolve(),
            Err(ConfigError::UnknownProviderType(t)) if t == "billing"
        ));
    }

    #[test]
    fn anthropic_type_resolves() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "claude".into(),
            ProviderConfig {
                provider_type: Some("anthropic".into()),
                base_url: Some("https://api.anthropic.com".into()),
                api_key: Some("sk-ant-xxx".into()),
                model: Some("claude-sonnet-4-5".into()),
                context_window: Some(200_000),
                models: vec!["claude-opus-4".into()],
            },
        );
        cfg.default_provider = Some("claude".into());
        let resolved = cfg.resolve().unwrap();
        assert_eq!(resolved.provider_type, "anthropic");
        assert_eq!(resolved.base_url, "https://api.anthropic.com");
        assert_eq!(resolved.model, "claude-sonnet-4-5");
        assert_eq!(resolved.models, vec!["claude-sonnet-4-5", "claude-opus-4"]);
    }

    #[test]
    fn compaction_defaults_and_parse() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.compaction.keep_recent, 6);
        assert_eq!(cfg.compaction.threshold_tokens, 100_000);

        let cfg: Config = toml::from_str(
            r#"
[compaction]
keep_recent = 3
threshold_tokens = 0
"#,
        )
        .unwrap();
        assert_eq!(cfg.compaction.keep_recent, 3);
        assert_eq!(cfg.compaction.threshold_tokens, 0);
    }

    #[test]
    fn resolve_named_picks_non_default_entry() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "a".into(),
            ProviderConfig {
                base_url: Some("https://a.example/v1".into()),
                api_key: None,
                model: Some("ma".into()),
                ..Default::default()
            },
        );
        cfg.providers.insert(
            "b".into(),
            ProviderConfig {
                provider_type: Some("anthropic".into()),
                base_url: Some("https://api.anthropic.com".into()),
                api_key: Some("k".into()),
                model: Some("mb".into()),
                ..Default::default()
            },
        );
        cfg.default_provider = Some("a".into());
        // 默认解析为 a；按名解析得到 b
        assert_eq!(cfg.resolve().unwrap().name, "a");
        let named = cfg.resolve_named("b").unwrap();
        assert_eq!(named.name, "b");
        assert_eq!(named.provider_type, "anthropic");
        assert_eq!(named.model, "mb");
        // 未知条目报错
        assert!(matches!(
            cfg.resolve_named("nope"),
            Err(ConfigError::NoProvider { .. })
        ));
    }

    #[test]
    fn resolve_named_ignores_top_level_default_model() {
        let mut cfg = Config {
            default_model: Some("top-model".into()),
            ..Default::default()
        };
        cfg.providers.insert(
            "p".into(),
            ProviderConfig {
                base_url: Some("https://x/v1".into()),
                model: Some("entry-model".into()),
                ..Default::default()
            },
        );
        // 默认解析受 default_model 影响；按名解析只用条目模型
        assert_eq!(cfg.resolve().unwrap().model, "top-model");
        assert_eq!(cfg.resolve_named("p").unwrap().model, "entry-model");
    }

    #[test]
    fn mcp_servers_parse() {
        let cfg: Config = toml::from_str(
            r#"
[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
env = { FOO = "bar" }
"#,
        )
        .unwrap();
        assert_eq!(cfg.mcp.servers.len(), 1);
        let s = &cfg.mcp.servers[0];
        assert_eq!(s.name, "filesystem");
        assert_eq!(s.command, "npx");
        assert_eq!(s.args.len(), 3);
        assert_eq!(s.env.get("FOO").map(String::as_str), Some("bar"));
    }
}
