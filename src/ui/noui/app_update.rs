//! 无 UI（旧 CLI）下的程序更新检查与提示。

use anyhow::Result;

use crate::base_system::app_update;

pub(super) fn startup_check() {
    let report = match app_update::check_update_report_blocking_with_timeout(
        env!("CARGO_PKG_VERSION"),
        std::time::Duration::from_secs(3),
    ) {
        Ok(r) => r,
        Err(_) => return,
    };

    if !app_update::should_notify_startup(&report) {
        return;
    }

    println!(
        "\n提示：检测到新版本 {}（当前 {}）。输入 c 查看更新日志；输入 U 执行自更新（若可用）。\n",
        report.latest.tag_name, report.current_tag
    );

    if let Some(body) = report.latest.body.as_deref() {
        let preview = preview_notes(body, 8, 800);
        if !preview.trim().is_empty() {
            println!("更新日志（节选）：\n{}\n", preview);
        }
    }
}

pub(super) fn check_update_menu() -> Result<()> {
    let report = app_update::check_update_report_blocking(env!("CARGO_PKG_VERSION"))?;

    println!("\n===== 程序更新检查 =====");
    println!("当前版本: {}", report.current_tag);
    println!("最新版本: {}", report.latest.tag_name);

    if report.is_new_version {
        println!("状态: 有新版本");
    } else {
        println!("状态: 已是最新版本");
    }

    if let Some(url) = report.latest.html_url.as_deref()
        && !url.trim().is_empty()
    {
        println!("Release: {}", url);
    }

    if let Some(body) = report.latest.body.as_deref() {
        let text = body.trim();
        if !text.is_empty() {
            println!("\n----- 更新日志 -----\n{}\n--------------------", text);
        }
    }

    if report.is_new_version {
        let dismissed = app_update::dismissed_release_tag();
        if dismissed.as_deref() == Some(&report.latest.tag_name) {
            println!("提示：你已设置忽略该版本提醒（仍可手动检查）。");
        }

        let ans = super::read_line("是否对该版本设置不再提醒？[y/N]: ")?;
        let ans = ans.trim().to_ascii_lowercase();
        if ans == "y" || ans == "yes" {
            app_update::dismiss_release_tag(&report.latest.tag_name)?;
            println!("已设置：不再提醒 {}\n", report.latest.tag_name);
        }
    }

    Ok(())
}

fn preview_notes(body: &str, max_lines: usize, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, line) in body.lines().enumerate() {
        if i >= max_lines {
            out.push('…');
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        if out.len() >= max_chars {
            // 在字符边界安全截断，避免中文等多字节字符被截断导致 panic
            let mut end = max_chars;
            while !out.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            out.truncate(end);
            out.push('…');
            break;
        }
    }
    out
}
