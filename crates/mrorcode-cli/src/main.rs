//! mrorcode 命令行入口。

#![allow(clippy::large_enum_variant)]

mod headless;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use mrorcode_agent::AssembleOptions;
use mrorcode_config::Config;

#[derive(Parser, Debug)]
#[command(name = "mrorcode", version, about = "A fast terminal AI coding agent")]
struct Cli {
    /// 子命令；缺省进入 TUI（M1 暂未实现）。
    #[command(subcommand)]
    command: Option<Commands>,

    #[command(flatten)]
    common: CommonArgs,
}

#[derive(Args, Debug)]
struct CommonArgs {
    /// 切换工作目录。
    #[arg(short = 'C', long = "cd", global = true)]
    cd: Option<PathBuf>,

    /// 覆盖使用的模型。
    #[arg(long = "model", global = true)]
    model: Option<String>,

    /// 覆盖使用的 provider 条目名。
    #[arg(long = "provider", global = true)]
    provider: Option<String>,

    /// 跳过所有工具审批（仅限可信的自动化场景）。
    #[arg(long = "dangerously-skip-permissions", global = true)]
    skip_permissions: bool,

    /// 恢复一个已持久化的会话 id（TUI 与 run 均可用）。
    #[arg(long = "resume", global = true)]
    resume: Option<u64>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 一次性任务（无交互）。
    Run(RunArgs),
}

#[derive(Args, Debug)]
struct RunArgs {
    /// 提示文本。
    prompt: String,

    /// 输出版本化 JSONL 事件流。
    #[arg(long)]
    json: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> anyhow::Result<()> {
    let working_dir = match cli.common.cd {
        Some(dir) => dir.canonicalize().unwrap_or(dir),
        None => std::env::current_dir()?,
    };

    let mut loaded = Config::load();
    for w in loaded.warnings.drain(..) {
        eprintln!("config warning: {w}");
    }
    if let Some(model) = cli.common.model {
        loaded.config.default_model = Some(model);
    }
    if let Some(provider) = cli.common.provider {
        loaded.config.default_provider = Some(provider);
    }

    match cli.command {
        Some(Commands::Run(run)) => {
            let resolved = match loaded.config.resolve() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{e}");
                    eprintln!();
                    eprintln!(
                        "example config (~/.mrorcode/config.toml):\n\
                         \n\
                         default_model = \"your-model\"\n\
                         \n\
                         [providers.default]\n\
                         type = \"openai_compat\"\n\
                         base_url = \"https://api.example.com/v1\"\n\
                         api_key = \"sk-...\"\n\
                         model = \"your-model\""
                    );
                    std::process::exit(2);
                }
            };

            let options = AssembleOptions {
                skip_permissions: cli.common.skip_permissions,
                compaction: compaction_policy(&loaded.config),
            };
            let mut assembly = mrorcode_agent::assemble(
                &resolved,
                working_dir,
                &options,
                None,
                &loaded.config.mcp.servers,
            )
            .await?;
            for w in &assembly.warnings {
                eprintln!("mcp warning: {w}");
            }
            let code = headless::run_headless(&mut assembly, &run.prompt, run.json).await;
            assembly.handle.shutdown();
            std::process::exit(code);
        }
        None => {
            let options = AssembleOptions {
                skip_permissions: cli.common.skip_permissions,
                compaction: compaction_policy(&loaded.config),
            };
            let assembly = mrorcode_agent::assemble(
                &loaded.config.resolve().map_err(|e| {
                    eprintln!("{e}");
                    anyhow::anyhow!("provider not configured")
                })?,
                working_dir,
                &options,
                None,
                &loaded.config.mcp.servers,
            )
            .await?;
            for w in &assembly.warnings {
                eprintln!("mcp warning: {w}");
            }
            mrorcode_tui::run(assembly, cli.common.resume, loaded.config, options).await?;
            std::process::exit(0);
        }
    }
}

/// 从 config 生成内核压缩策略。
fn compaction_policy(config: &mrorcode_config::Config) -> mrorcode_kernel::agent::CompactionPolicy {
    mrorcode_kernel::agent::CompactionPolicy {
        threshold_tokens: config.compaction.threshold_tokens,
        keep_recent: config.compaction.keep_recent,
    }
}
