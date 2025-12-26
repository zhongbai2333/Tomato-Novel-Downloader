//! 无 UI 的更新检查与提示。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;
use tomato_novel_official_api::DirectoryClient;

use crate::base_system::context::Config;

#[derive(Debug, Clone)]
pub(super) struct UpdateEntry {
    pub(super) book_id: String,
    pub(super) label: String,
    pub(super) new_count: usize,
}

pub(super) fn update_menu(config: &Config) -> Result<Option<String>> {
    let save_dir = config.default_save_dir();
    if !save_dir.exists() {
        println!(
            "没有可供更新的小说（保存目录不存在）：{}\n",
            save_dir.display()
        );
        return Ok(None);
    }

    let (updates, no_updates) = scan_updates(config, &save_dir)?;
    if updates.is_empty() && no_updates.is_empty() {
        println!("没有可供更新的小说\n");
        return Ok(None);
    }

    loop {
        println!("\n===== 可供更新的小说列表 =====");
        for (idx, u) in updates.iter().enumerate() {
            println!("{}. {}", idx + 1, u.label);
        }
        let opt_no_update = if no_updates.is_empty() {
            None
        } else {
            let n = updates.len() + 1;
            println!("{}. 无更新 ({})", n, no_updates.len());
            Some(n)
        };
        println!("q. 退出\n");

        let sel = super::read_line("请输入编号：")?;
        let sel = sel.trim().to_ascii_lowercase();
        if sel == "q" {
            println!("已取消更新\n");
            return Ok(None);
        }
        let Ok(n) = sel.parse::<usize>() else {
            println!("错误：请输入数字编号或 q 退出。\n");
            continue;
        };

        if n >= 1 && n <= updates.len() {
            return Ok(Some(updates[n - 1].book_id.clone()));
        }

        if let Some(no_idx) = opt_no_update
            && n == no_idx
            && let Some(book_id) = select_from_list(&no_updates, "无更新的书籍")?
        {
            return Ok(Some(book_id));
        }
        if let Some(no_idx) = opt_no_update
            && n == no_idx
        {
            continue;
        }

        let max = opt_no_update.unwrap_or(updates.len());
        println!("错误：请输入 1 到 {} 之间的数字，或 q 退出。\n", max);
    }
}

fn select_from_list(list: &[UpdateEntry], title: &str) -> Result<Option<String>> {
    loop {
        println!("\n===== {} =====", title);
        for (idx, u) in list.iter().enumerate() {
            println!("{}. {}", idx + 1, u.label);
        }
        println!("q. 取消并返回上级菜单\n");

        let sel = super::read_line("请输入编号：")?;
        let sel = sel.trim().to_ascii_lowercase();
        if sel == "q" {
            return Ok(None);
        }
        let Ok(n) = sel.parse::<usize>() else {
            println!("错误：请输入数字编号或 q 返回。\n");
            continue;
        };
        if n >= 1 && n <= list.len() {
            return Ok(Some(list[n - 1].book_id.clone()));
        }
        println!("错误：请输入 1 到 {} 之间的数字，或 q 返回。\n", list.len());
    }
}

fn scan_updates(_config: &Config, save_dir: &Path) -> Result<(Vec<UpdateEntry>, Vec<UpdateEntry>)> {
    let mut updates = Vec::new();
    let mut no_updates = Vec::new();

    let client = DirectoryClient::new().context("init DirectoryClient")?;
    let dir_reader =
        fs::read_dir(save_dir).with_context(|| format!("read dir {}", save_dir.display()))?;
    for entry in dir_reader.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let Some((book_id, book_name)) = name.split_once('_') else {
            continue;
        };
        if !book_id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let downloaded_count = read_downloaded_count(&path, book_id).unwrap_or(0);
        let chapter_list = match client.fetch_directory(book_id) {
            Ok(d) => d.chapters,
            Err(_) => Vec::new(),
        };
        if chapter_list.is_empty() {
            continue;
        }
        let total = chapter_list.len();
        let new_count = total.saturating_sub(downloaded_count);
        let label = format!("《{}》({}) — 新章节：{}", book_name, book_id, new_count);
        let entry = UpdateEntry {
            book_id: book_id.to_string(),
            label,
            new_count,
        };
        if entry.new_count > 0 {
            updates.push(entry);
        } else {
            no_updates.push(entry);
        }
    }

    Ok((updates, no_updates))
}

fn read_downloaded_count(folder: &Path, book_id: &str) -> Option<usize> {
    let status_new = folder.join("status.json");
    let status_old = folder.join(format!("chapter_status_{}.json", book_id));
    let path = if status_new.exists() {
        status_new
    } else if status_old.exists() {
        status_old
    } else {
        return None;
    };
    let data = fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&data).ok()?;
    let downloaded = value.get("downloaded")?.as_object()?;
    Some(downloaded.len())
}
