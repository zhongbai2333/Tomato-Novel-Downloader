//! noUI 下载历史查看。

use anyhow::Result;

use crate::base_system::download_history::read_download_history;

pub(super) fn show_history_menu() -> Result<()> {
    let mut keyword: Option<String> = None;

    loop {
        let items = read_download_history(50, keyword.as_deref());
        println!("\n===== 下载历史（最近 50 条） =====");
        if let Some(k) = keyword.as_deref() {
            println!("过滤关键字: {}", k);
        }

        if items.is_empty() {
            println!("暂无记录");
        } else {
            for (i, it) in items.iter().enumerate() {
                println!(
                    "{:>2}. [{}] 《{}》({}) | 作者: {} | {} | 状态: {}",
                    i + 1,
                    it.timestamp,
                    it.book_name,
                    it.book_id,
                    if it.author.trim().is_empty() {
                        "未知"
                    } else {
                        it.author.trim()
                    },
                    it.progress,
                    it.status
                );
            }
        }

        println!("\n操作：Enter=刷新, f=设置过滤关键字, c=清空过滤, q=返回");
        let cmd = super::read_line("选择: ")?;
        let cmd = cmd.trim();
        if cmd.is_empty() {
            continue;
        }
        if cmd.eq_ignore_ascii_case("q") {
            break;
        }
        if cmd.eq_ignore_ascii_case("f") {
            let q = super::read_line("输入书名/作者/ID关键字: ")?;
            let q = q.trim();
            if q.is_empty() {
                println!("关键字为空，保持当前过滤。\n");
            } else {
                keyword = Some(q.to_string());
            }
            continue;
        }
        if cmd.eq_ignore_ascii_case("c") {
            keyword = None;
            continue;
        }
    }

    Ok(())
}
