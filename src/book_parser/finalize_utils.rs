use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tracing::{error, info};

use super::book_manager::BookManager;
use super::epub_generator::EpubGenerator;
use crate::base_system::context::safe_fs_name;

/// 生成最终输出；返回是否需要延迟清理缓存。
pub fn run_finalize(manager: &mut BookManager, chapters: &[Value], _result: i32) -> bool {
    info!(target: "book_manager", "finalize start: chapters={}", chapters.len());

    let fmt = manager.config.novel_format.to_lowercase();
    let output_path = match prepare_output_path(manager, &fmt) {
        Ok(p) => p,
        Err(e) => {
            error!(target: "book_manager", error = ?e, "prepare output path failed");
            return false;
        }
    };

    let result: anyhow::Result<()> = if fmt == "txt" {
        finalize_txt(manager, chapters, &output_path)
    } else {
        finalize_epub(manager, chapters, &output_path)
    };

    if let Err(e) = result {
        error!(target: "book_manager", error = ?e, "finalize failed");
        return false;
    }

    info!(target: "book_manager", "written: {}", output_path.display());
    false
}

/// 执行延迟清理（当前直接调用）。
pub fn perform_deferred_cleanup(manager: &mut BookManager) {
    if let Err(e) = manager.cleanup_status_folder() {
        error!(target: "book_manager", error = ?e, "deferred cleanup failed");
    }
}

fn prepare_output_path(manager: &BookManager, fmt: &str) -> std::io::Result<PathBuf> {
    let suffix = if fmt == "epub" { "epub" } else { "txt" };
    let raw_name = if manager.book_name.is_empty() {
        "book"
    } else {
        manager.book_name.as_str()
    };
    let safe = safe_fs_name(raw_name, "_", 120);
    let dir = manager.config.default_save_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{}.{}", safe, suffix)))
}

fn finalize_txt(manager: &BookManager, chapters: &[Value], path: &Path) -> anyhow::Result<()> {
    let mut f = File::create(path)?;
    writeln!(f, "book_id={}", manager.book_id)?;
    writeln!(f, "")?;
    for ch in chapters {
        let title = ch.get("title").and_then(|v| v.as_str()).unwrap_or("章节");
        let content = ch.get("content").and_then(|v| v.as_str()).unwrap_or("");
        writeln!(f, "{}\n", title)?;
        writeln!(f, "{}\n", content.trim())?;
        writeln!(f, "\n----------------------------------------\n")?;
    }
    Ok(())
}

fn finalize_epub(manager: &BookManager, chapters: &[Value], path: &Path) -> anyhow::Result<()> {
    let mut epub_gen = EpubGenerator::new(&manager.book_id, &manager.book_name, &manager.config)?;

    // 简单的介绍页
    let intro_html = format!(
        "<p>书名：{}</p><p>作者：{}</p><p>标签：{}</p><p>简介：{}</p>",
        escape_html(&manager.book_name),
        escape_html(&manager.author),
        escape_html(&manager.tags),
        escape_html(&manager.description)
    );
    epub_gen.add_aux_page("简介", &intro_html, true);

    for ch in chapters {
        let title = ch.get("title").and_then(|v| v.as_str()).unwrap_or("章节");
        let content_html = ch.get("content").and_then(|v| v.as_str()).unwrap_or("");

        epub_gen.add_chapter(title, content_html);
    }

    epub_gen.generate(path, &manager.config)?;
    Ok(())
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
