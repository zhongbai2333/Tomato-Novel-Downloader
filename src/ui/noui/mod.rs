//! 无 UI（旧 CLI）交互入口。
//!
//! 使用标准输入输出进行交互，并在进入前尽量恢复终端模式。

use std::fs;
use std::io::{self, BufRead, Write};

use anyhow::{Context, Result};

use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};

use crate::base_system::context::Config;

mod config;
mod download;
mod update;

fn show_config_menu(config: &mut Config) -> Result<()> {
    config::show_config_menu(config)
}

fn search_and_pick(keyword: &str) -> Result<Option<String>> {
    download::search_and_pick(keyword)
}

fn download_book(book_id: &str, config: &Config) -> Result<()> {
    download::download_book(book_id, config)
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

    loop {
        let prompt = format!(
            "请输入 小说ID/书本链接（分享链接）/书本名字（输入s配置 / u更新 / U程序更新 / q退出，默认保存到 {}）：",
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

        if text == "U" {
            let _ = crate::base_system::self_update::check_for_updates(
                env!("CARGO_PKG_VERSION"),
                false,
            );
            continue;
        }

        // 解析 book_id / 链接 / 搜索
        let mut book_id = parse_book_id(text);
        if book_id.is_none() && text.chars().all(|c| c.is_ascii_digit()) {
            book_id = Some(text.to_string());
        }
        if book_id.is_none() {
            book_id = search_and_pick(text)?;
            if book_id.is_none() {
                continue;
            }
        }

        let book_id = book_id.unwrap();
        let save_dir_input = read_line(&format!(
            "保存路径（默认：{}）：",
            config.default_save_dir().display()
        ))?;
        if !save_dir_input.trim().is_empty() {
            let p = save_dir_input.trim().trim_end_matches(['/', '\\']);
            fs::create_dir_all(p).with_context(|| format!("创建目录失败: {}", p))?;
            config.save_path = p.to_string();
        }

        println!("开始下载 book_id={}", book_id);
        match download_book(&book_id, config) {
            Ok(()) => println!("下载完成\n"),
            Err(err) => println!("下载失败: {}\n", err),
        }
    }

    Ok(())
}

fn parse_book_id(input: &str) -> Option<String> {
    crate::base_system::book_id::parse_book_id(input)
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush().ok();
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line)
}
