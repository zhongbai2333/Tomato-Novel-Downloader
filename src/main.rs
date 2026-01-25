//! Tomato Novel Downloader（番茄小说下载器）Rust 实现。
//!
//! 本 crate 负责：配置加载、交互界面（TUI/CLI）、下载调度、内容解析与导出（txt/epub/有声书等）。
//!
//! 代码结构（读代码入口）：
//! - `base_system`：配置/日志/重试/路径等基础设施
//! - `download`：下载流程编排（拉目录、拉内容、冷却/重试等）
//! - `book_parser`：解析与导出（epub/txt/媒体/有声书）
//! - `ui`：TUI 与无 UI（old cli）两套交互
//! - `prewarm_state`：启动预热状态（与 UI 协作显示）

use anyhow::{Result, anyhow};
use clap::Parser;
use std::thread;

mod base_system;
mod book_parser;
mod download;
mod network_parser;
mod prewarm_state;
mod third_party;
mod ui;

use base_system::config::{load_or_create, load_or_create_with_base};
use base_system::context::Config;
use base_system::logging::{LogOptions, LogSystem};
use tracing::{info, warn};

#[cfg(all(feature = "official-api", feature = "no-official-api"))]
compile_error!(
    "features 'official-api' and 'no-official-api' are mutually exclusive; use exactly one"
);

#[cfg(feature = "official-api")]
use tomato_novel_official_api::prewarm_iid;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Parser)]
#[command(name = "tomato-novel-downloader")]
#[command(about = "Tomato Novel Downloader (Rust TUI)")]
struct Cli {
    /// 启用调试日志输出
    #[arg(long, default_value_t = false)]
    debug: bool,

    /// 启用服务器模式（Web UI）
    #[arg(long, default_value_t = false)]
    server: bool,

    /// Web UI 密码（启用锁模式，防止陌生人使用）
    #[arg(long)]
    password: Option<String>,

    /// 显示版本信息后退出
    #[arg(long, default_value_t = false)]
    version: bool,

    /// 检查并执行程序自更新（从 GitHub Releases 下载并替换当前可执行文件）
    #[arg(long, default_value_t = false)]
    self_update: bool,

    /// 自更新时自动确认（等价于提示输入 Y）
    #[arg(long, default_value_t = false)]
    self_update_yes: bool,

    /// 数据目录路径（用于存放 config.yml 和 logs 等文件，方便 Docker 挂载）
    #[arg(long)]
    data_dir: Option<String>,

    /// 直接下载指定 book_id 的小说（非交互模式）
    #[arg(long)]
    download: Option<String>,

    /// 更新指定 book_id 的小说（非交互模式）
    #[arg(long)]
    update: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.version {
        println!("Tomato Novel Downloader v{}", VERSION);
        return Ok(());
    }

    let data_dir = cli.data_dir.as_ref().map(std::path::Path::new);
    let _log = init_logging(cli.debug, data_dir)?;

    if cli.self_update {
        let _ = base_system::self_update::check_for_updates(VERSION, cli.self_update_yes);
        return Ok(());
    }

    // 启动时强制热更新（仅当 SHA256 不同且 tag 相同）。
    // 例外：cargo run/开发态运行时跳过。
    let _ = base_system::self_update::check_hotfix_and_apply(VERSION);

    prewarm_state::mark_prewarm_start();
    thread::spawn(|| {
        #[cfg(feature = "official-api")]
        {
            match prewarm_iid() {
                Ok(_) => info!(target: "startup", "IID 预热完成"),
                Err(err) => warn!(target: "startup", "IID 预热失败: {err}"),
            }
        }

        #[cfg(not(feature = "official-api"))]
        {
            info!(target: "startup", "no-official-api 构建：跳过 IID 预热");
        }
        prewarm_state::mark_prewarm_done();
    });

    let mut config = load_config_from_data_dir(data_dir)?;

    // Handle command-line download/update modes
    if cli.download.is_some() || cli.update.is_some() {
        info!(target: "startup", "当前版本: v{}", VERSION);

        if let Some(book_id) = cli.download {
            return ui::noui::download::download_book(&book_id, &config);
        }

        if let Some(book_id) = cli.update {
            println!("更新指定书籍 book_id={}", book_id);
            return ui::noui::download::download_book(&book_id, &config);
        }
    }

    if cli.server {
        let password = cli
            .password
            .or_else(|| std::env::var("TOMATO_WEB_PASSWORD").ok());
        return ui::web::run(&mut config, password);
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
                config = load_config_from_data_dir(data_dir)?;
                config.old_cli = true;
            }
            ui::tui::TuiExit::SelfUpdate { auto_yes } => {
                let _ = base_system::self_update::check_for_updates(VERSION, auto_yes);
                return Ok(());
            }
        }
    }
}

fn load_config_from_data_dir(data_dir: Option<&std::path::Path>) -> Result<Config> {
    if let Some(dir) = data_dir {
        load_or_create_with_base::<Config>(None, Some(dir)).map_err(|e| anyhow!(e.to_string()))
    } else {
        load_or_create::<Config>(None).map_err(|e| anyhow!(e.to_string()))
    }
}

fn init_logging(debug: bool, base_dir: Option<&std::path::Path>) -> Result<LogSystem> {
    let opts = LogOptions {
        debug,
        use_color: true,
        archive_on_exit: true,
        console: false,
        broadcast_to_ui: true,
    };
    if let Some(base_dir) = base_dir {
        LogSystem::init_with_base(opts, Some(base_dir)).map_err(|e| anyhow!(e))
    } else {
        LogSystem::init(opts).map_err(|e| anyhow!(e))
    }
}
