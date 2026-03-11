//! 无 UI（旧 CLI）交互入口。
//!
//! 使用标准输入输出进行交互，并在进入前尽量恢复终端模式。

use std::io::{self, BufRead, Write};

use anyhow::Result;

use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};

use crate::base_system::context::Config;

mod app_update;
mod config;
mod download;
mod history;
mod update;

fn show_config_menu(config: &mut Config) -> Result<()> {
    config::show_config_menu(config)
}

pub(crate) fn download_book(book_id: &str, config: &Config) -> Result<()> {
    download::download_book(book_id, config)
}

pub(crate) fn update_existing_book_non_interactive(
    book_id: &str,
    config: &Config,
    retry_failed: bool,
) -> Result<()> {
    download::update_existing_book_non_interactive(book_id, config, retry_failed)
}

pub fn run(config: &mut Config) -> Result<()> {
    // In case the previous run exited while in TUI raw mode (e.g., Ctrl+C),
    // best-effort restore the console so stdin line input works in PowerShell.
    let _ = disable_raw_mode();
    let mut out = io::stdout();
    let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);

    println!(
        "欢迎使用番茄小说下载器! v{}\n\
项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader \n\
Fork From: https://github.com/Dlmily/Tomato-Novel-Downloader-Lite \n\
作者: zhongbai233 (https://github.com/zhongbai2333) \n\
项目早期代码: Dlmily (https://github.com/Dlmily) \n\
\n\
项目说明: 此项目基于Dlmily的项目Fork而来, 我对其进行重构 + 优化, 添加更对功能, 包括: EPUB下载支持、更好的断点传输、更好的错误管理等特性 \n\
本项目[完全]基于第三方API, [未]使用官方API, 如有需要可以查看Dlmily的项目 \n\
本项目仅供网络爬虫技术、网页数据处理及相关研究的学习用途。请勿将其用于任何违反法律法规或侵犯他人权益的活动。",
        env!("CARGO_PKG_VERSION")
    );

    #[cfg(feature = "official-api")]
    println!(
        "\n【免费声明】本程序完全免费，若发现收费渠道，请勿上当受骗！\n\
      官方仓库: https://github.com/zhongbai2333/Tomato-Novel-Downloader"
    );

    // 每次启动检查程序更新（不影响后续流程，失败直接忽略）。
    app_update::startup_check();

    loop {
        let prompt = format!(
            "旧 CLI 已禁用新建下载；请输入命令（s配置 / h下载历史 / u更新小说 / c检查更新 / U程序自更新 / q退出，默认保存到 {}）：",
            config.default_save_dir().display()
        );
        let input = read_line(&prompt)?;
        let text = input.trim();
        if text.is_empty() {
            continue;
        }
        if text.eq_ignore_ascii_case("q") {
            println!("已退出。");
            break;
        }
        if text.eq_ignore_ascii_case("s") {
            show_config_menu(config)?;
            continue;
        }
        if text.eq_ignore_ascii_case("h") {
            history::show_history_menu()?;
            continue;
        }
        if text.eq_ignore_ascii_case("u") {
            if let Some(book_id) = update::update_menu(config)? {
                println!("已选择更新 book_id={}\n", book_id);
                // 直接进入该书下载流程
                match download_book(&book_id, config) {
                    Ok(()) => println!("下载完成\n"),
                    Err(err) => println!("下载失败: {}\n", err),
                }
            }
            continue;
        }

        if text.eq_ignore_ascii_case("c") {
            app_update::check_update_menu()?;
            continue;
        }

        if text == "U" {
            let _ = crate::base_system::self_update::check_for_updates(
                env!("CARGO_PKG_VERSION"),
                false,
            );
            continue;
        }

        println!(
            "旧 CLI 模式已禁用下载新小说。\n如需新增下载，请使用 TUI 或 Web UI；旧 CLI 仅保留“u”更新本地已有小说。\n"
        );
    }

    Ok(())
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush().ok();
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line)
}
