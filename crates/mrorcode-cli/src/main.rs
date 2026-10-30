//! mrorcode 命令行入口。

#![allow(clippy::large_enum_variant)]

use std::path::PathBuf;

use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand};
use mrorcode_agent::AssembleOptions;
use mrorcode_cli::runlog::RunLog;
use mrorcode_cli::{headless, repl};
use mrorcode_config::Config;

// clap 内置的 `Usage:`/`Commands:`/`Options:` 是英文常量，没有公开的改文案接口；
// 而 `{subcommands}`/`{options}` 只渲染条目、不带标题，所以标题由模板自己给。
/// 顶层帮助模板。
const HELP_TEMPLATE: &str =
    "{about-with-newline}\n用法：{usage}\n\n命令：\n{subcommands}\n\n选项：\n{options}";
/// 子命令帮助模板（没有子命令段，位置参数称「参数」）。
const RUN_HELP_TEMPLATE: &str =
    "{about-with-newline}\n用法：{usage}\n\n参数：\n{positionals}\n\n选项：\n{options}";

#[derive(Parser, Debug)]
#[command(
    name = "mrorcode",
    version,
    about = "快速终端 AI 编码助手",
    help_template = HELP_TEMPLATE,
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true
)]
struct Cli {
    /// 子命令；缺省进入裸 ANSI REPL。
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

    // 内置 `-h/-V` 被禁用后手工补回，只为让说明文字是中文；
    // 两者在解析阶段由 clap 直接打印并退出，字段本身不会读到。
    #[arg(
        short = 'h',
        long = "help",
        action = ArgAction::Help,
        global = true,
        help = "显示帮助"
    )]
    #[allow(dead_code)]
    help: Option<bool>,

    // `-V` 只留在顶层：子命令没有 version，`ArgAction::Version` 传播过去会触发 clap 调试断言。
    #[arg(
        short = 'V',
        long = "version",
        action = ArgAction::Version,
        help = "显示版本"
    )]
    #[allow(dead_code)]
    version: Option<bool>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 一次性任务（无交互）。
    #[command(help_template = RUN_HELP_TEMPLATE)]
    Run(RunArgs),

    /// 显示帮助信息。
    Help,
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
    // 运行日志默认开启（`~/.mrorcode/logs/run-<时间戳>.log`）：转录与诊断行同文件，
    // 保留最近 20 份；`MRORCODE_LOG=0` 可关闭。
    let run_log = RunLog::start();
    mrorcode_cli::runlog::init_tracing(run_log.as_ref());

    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async_main(cli, run_log))
}

async fn async_main(cli: Cli, run_log: Option<RunLog>) -> anyhow::Result<()> {
    let working_dir = match cli.common.cd {
        Some(dir) => dir.canonicalize().unwrap_or(dir),
        None => std::env::current_dir()?,
    };

    let mut loaded = Config::load();
    for w in loaded.warnings.drain(..) {
        eprintln!("配置警告：{w}");
    }
    if let Some(model) = cli.common.model {
        loaded.config.default_model = Some(model);
    }
    if let Some(provider) = cli.common.provider {
        loaded.config.default_provider = Some(provider);
    }

    match cli.command {
        Some(Commands::Help) => {
            Cli::command().print_help()?;
            println!();
            std::process::exit(0);
        }
        Some(Commands::Run(run)) => {
            let resolved = match loaded.config.resolve() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{e}");
                    eprintln!();
                    eprintln!(
                        "示例配置（~/.mrorcode/config.toml）：\n\
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
                eprintln!("MCP 警告：{w}");
            }
            let code =
                headless::run_headless(&mut assembly, &run.prompt, run.json, run_log.as_ref())
                    .await;
            assembly.handle.shutdown();
            std::process::exit(code);
        }
        None => {
            let options = AssembleOptions {
                skip_permissions: cli.common.skip_permissions,
                compaction: compaction_policy(&loaded.config),
            };
            let mut assembly = mrorcode_agent::assemble(
                &loaded.config.resolve().map_err(|e| {
                    eprintln!("{e}");
                    anyhow::anyhow!("未配置 provider")
                })?,
                working_dir,
                &options,
                None,
                &loaded.config.mcp.servers,
            )
            .await?;
            for w in &assembly.warnings {
                eprintln!("MCP 警告：{w}");
            }
            repl::run_repl(&mut assembly, run_log).await?;
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
