use anyhow::{Result, anyhow};
use clap::Parser;

mod base_system;
mod book_parser;
mod download;
mod ui;

use base_system::config::load_or_create;
use base_system::context::Config;
use base_system::logging::{LogError, LogOptions, LogSystem};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Parser)]
#[command(name = "tomato-novel-downloader")]
#[command(about = "Tomato Novel Downloader (Rust TUI)")]
struct Cli {
    /// 启用调试日志输出
    #[arg(long, default_value_t = false)]
    debug: bool,

    /// 启用服务器模式（暂未实现）
    #[arg(long, default_value_t = false)]
    server: bool,

    /// 显示版本信息后退出
    #[arg(long, default_value_t = false)]
    version: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.version {
        println!("Tomato Novel Downloader v{}", VERSION);
        return Ok(());
    }

    init_logging(cli.debug)?;

    let mut config = load_or_create::<Config>(None).map_err(|e| anyhow!(e.to_string()))?;

    if cli.server {
        println!("服务器模式暂未实现，当前仅支持终端 UI 模式。");
        return Ok(());
    }

    if config.old_cli {
        ui::noui::run(&mut config)
    } else {
        ui::tui::run(config)
    }
}

fn init_logging(debug: bool) -> Result<()> {
    let opts = LogOptions {
        debug,
        use_color: true,
        archive_on_exit: true,
    };
    match LogSystem::init(opts) {
        Ok(_) => Ok(()),
        Err(LogError::AlreadyInitialized) => Ok(()),
        Err(err) => Err(anyhow!(err)),
    }
}
