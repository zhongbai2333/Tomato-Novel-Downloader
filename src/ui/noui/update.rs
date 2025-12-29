//! 无 UI 的更新检查与提示。

use std::path::Path;

use anyhow::Result;
use crate::base_system::novel_updates;

use crate::base_system::context::Config;

#[derive(Debug, Clone)]
pub(super) struct UpdateEntry {
    pub(super) book_id: String,
    pub(super) label: String,
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

fn scan_updates(config: &Config, save_dir: &Path) -> Result<(Vec<UpdateEntry>, Vec<UpdateEntry>)> {
    let scan = novel_updates::scan_novel_updates(save_dir, Some(config))?;

    let to_entry = |it: novel_updates::NovelUpdateRow| {
        let ignore_marker = if it.is_ignored { "[已忽略] " } else { "" };
        UpdateEntry {
            book_id: it.book_id.clone(),
            label: format!("{}《{}》({}) — 新章节：{}", ignore_marker, it.book_name, it.book_id, it.new_count),
        }
    };

    Ok((
        scan.updates.into_iter().map(to_entry).collect(),
        scan.no_updates.into_iter().map(to_entry).collect(),
    ))
}
