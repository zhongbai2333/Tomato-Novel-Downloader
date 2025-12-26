use anyhow::{Result, anyhow};
use clap::Parser;
use std::thread;

mod base_system;
mod book_parser;
mod download;
mod prewarm_state;
mod ui;

use base_system::config::load_or_create;
use base_system::context::Config;
use base_system::logging::{LogOptions, LogSystem};
use tomato_novel_official_api::prewarm_iid;
use tracing::{info, warn};

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

    let _log = init_logging(cli.debug)?;

    prewarm_state::mark_prewarm_start();
    thread::spawn(|| {
        match prewarm_iid() {
            Ok(_) => info!(target: "startup", "IID 预热完成"),
            Err(err) => warn!(target: "startup", "IID 预热失败: {err}"),
        }
        prewarm_state::mark_prewarm_done();
    });

    let mut config = load_or_create::<Config>(None).map_err(|e| anyhow!(e.to_string()))?;

    if cli.server {
        println!("服务器模式暂未实现，当前仅支持终端 UI 模式。");
        return Ok(());
    }

    loop {
        if config.old_cli {
            info!(target: "startup", "当前版本: v{}", VERSION);
            return ui::noui::run(&mut config);
        }

        match ui::tui::run(config)? {
            ui::tui::TuiExit::Quit => return Ok(()),
            ui::tui::TuiExit::SwitchToOldCli => {
                // 模拟“重启”：重新从磁盘加载配置，然后进入 noui
                config = load_or_create::<Config>(None).map_err(|e| anyhow!(e.to_string()))?;
                config.old_cli = true;
            }
        }
    }
}

fn init_logging(debug: bool) -> Result<LogSystem> {
    let opts = LogOptions {
        debug,
        use_color: true,
        archive_on_exit: true,
        console: false,
        broadcast_to_ui: true,
    };
    LogSystem::init(opts).map_err(|e| anyhow!(e))
}
