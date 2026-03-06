//! 导出收尾（finalize）与后处理。
//!
//! 包括写入最终文件、自动打开产物等"完成后"逻辑。
//! 具体子模块：`finalize_epub`、`html_utils`、`image_utils`、`segment_comments`、`segment_shared`。

use std::fs::File;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{collections::HashMap, collections::HashSet};

use serde_json::Value;

use tracing::{error, info, warn};

use crossterm::event::EnableMouseCapture;
use crossterm::terminal::enable_raw_mode;

use super::audio_generator::generate_audiobook;
use super::book_manager::BookManager;
use super::finalize_epub::finalize_epub;
use crate::base_system::context::safe_fs_name;

/// 生成最终输出；返回是否需要延迟清理缓存。
pub fn run_finalize(
    manager: &mut BookManager,
    chapters: &[Value],
    _result: i32,
    directory_raw: Option<&Value>,
    reporter: Option<&mut crate::download::downloader::ProgressReporter>,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> bool {
    info!(target: "book_manager", "finalize start: chapters={}", chapters.len());

    let mut reporter = reporter;

    // "下载完后选择"：在生成文件前询问用户使用哪个书名（仅 CLI 模式兜底）
    if manager.config.is_ask_after_download()
        && !manager.book_name_selected_after_download
        && let Some(chosen) = prompt_book_name_selection(manager)
    {
        info!(target: "book_manager", "用户选择书名: {}", chosen);
        manager.book_name = chosen;
        manager.book_name_selected_after_download = true;
    }

    let fmt = manager.config.novel_format.to_lowercase();
    let output_path = match prepare_output_path(manager, &fmt) {
        Ok(p) => p,
        Err(e) => {
            error!(target: "book_manager", error = ?e, "prepare output path failed");
            return false;
        }
    };

    let result: anyhow::Result<()> = if fmt == "txt" {
        finalize_txt(manager, chapters, &output_path, directory_raw)
    } else {
        let reporter_ref = {
            #[allow(clippy::needless_option_as_deref)]
            reporter.as_deref_mut()
        };
        finalize_epub(manager, chapters, &output_path, directory_raw, reporter_ref)
    };

    if let Err(e) = result {
        error!(target: "book_manager", error = ?e, "finalize failed");
        return false;
    }

    info!(target: "book_manager", "written: {}", output_path.display());

    if manager.config.auto_open_downloaded_files {
        if let Err(e) = open_in_default_app(&output_path) {
            warn!(target: "book_manager", error = ?e, "auto open downloaded file failed");
        }

        // Best-effort: re-assert TUI terminal modes after spawning external opener.
        if reporter.as_ref().is_some_and(|r| r.has_ui_callback()) {
            let _ = enable_raw_mode();
            let mut out = std::io::stdout();
            let _ = crossterm::execute!(&mut out, EnableMouseCapture);
        }
    }

    let audiobook_bar = reporter.as_ref().and_then(|r| r.cli_save_bar());
    let quiet = reporter.as_ref().is_some_and(|r| r.has_ui_callback());
    let reporter_ref = {
        #[allow(clippy::needless_option_as_deref)]
        reporter.as_deref_mut()
    };
    if !generate_audiobook(
        manager,
        chapters,
        audiobook_bar.as_ref(),
        quiet,
        reporter_ref,
        cancel,
    ) {
        warn!(target: "book_manager", "audiobook generation failed");
    }

    true
}

fn open_in_default_app(path: &Path) -> std::io::Result<()> {
    if cfg!(target_os = "windows") {
        Command::new("explorer").arg(path).spawn()?;
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()?;
        return Ok(());
    }
    Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

fn prepare_output_path(manager: &BookManager, fmt: &str) -> std::io::Result<PathBuf> {
    let raw_name = if manager.book_name.is_empty() {
        "book"
    } else {
        manager.book_name.as_str()
    };
    let safe_book = safe_fs_name(raw_name, "_", 120);
    let dir = manager.default_save_dir();
    std::fs::create_dir_all(&dir)?;

    // bulk_files: TXT 每章一个文件，输出到"小说名"文件夹
    if fmt == "txt" && manager.config.bulk_files {
        return Ok(dir.join(safe_book));
    }

    let suffix = if fmt == "epub" { "epub" } else { "txt" };
    let output_path = dir.join(format!("{}.{}", safe_book, suffix));

    // 检查文件是否已存在且不允许覆盖
    if !manager.config.allow_overwrite_files && output_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("文件已存在且配置禁止覆盖: {}", output_path.display()),
        ));
    }

    Ok(output_path)
}

fn finalize_txt(
    manager: &BookManager,
    chapters: &[Value],
    path: &Path,
    directory_raw: Option<&Value>,
) -> anyhow::Result<()> {
    let volume_title_by_chapter_id = volume_title_map_for_chapters(chapters, directory_raw);

    if manager.config.bulk_files {
        std::fs::create_dir_all(path)?;

        // 书籍信息
        let mut meta = File::create(path.join("0000_书籍信息.txt"))?;
        writeln!(meta, "书名：{}", manager.book_name)?;
        if !manager.author.trim().is_empty() {
            writeln!(meta, "作者：{}", manager.author)?;
        }
        writeln!(meta, "book_id={}", manager.book_id)?;

        let status_text = match manager.finished {
            Some(true) => "完结",
            Some(false) => "连载",
            None => "未知",
        };
        writeln!(meta, "状态：{}", status_text)?;

        if let Some(score) = manager.score {
            writeln!(meta, "评分：{:.1}", score)?;
        }
        if let Some(word_count) = manager.word_count {
            writeln!(meta, "字数：{}", word_count)?;
        }
        if let Some(chapter_count) = manager.chapter_count {
            writeln!(meta, "章节：{}", chapter_count)?;
        }
        if let Some(category) = manager.category.as_deref()
            && !category.trim().is_empty()
        {
            writeln!(meta, "分类：{}", category.trim())?;
        }
        if !manager.tags.trim().is_empty() {
            writeln!(meta, "标签：{}", manager.tags)?;
        }
        if let Some(read_count_text) = manager.read_count_text.as_deref()
            && !read_count_text.trim().is_empty()
        {
            writeln!(meta, "在读：{}", read_count_text.trim())?;
        }

        if !manager.description.trim().is_empty() {
            writeln!(meta)?;
            writeln!(meta, "简介：")?;
            writeln!(meta, "{}", manager.description.trim())?;
        }

        // 章节拆分
        let width = chapters.len().to_string().len().max(4);
        for (idx, ch) in chapters.iter().enumerate() {
            let chapter_id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let title = ch.get("title").and_then(|v| v.as_str()).unwrap_or("章节");
            let content = ch.get("content").and_then(|v| v.as_str()).unwrap_or("");

            let safe_title = safe_fs_name(title, "_", 120);
            let filename = format!(
                "{num:0width$}_{title}.txt",
                num = idx + 1,
                width = width,
                title = safe_title
            );
            let mut f = File::create(path.join(filename))?;
            if let Some(vol) = volume_title_by_chapter_id.get(chapter_id)
                && !vol.trim().is_empty()
            {
                writeln!(f, "分卷：{}", vol.trim())?;
                writeln!(f)?;
            }
            writeln!(f, "{}", title)?;
            writeln!(f)?;
            // Do not `trim()` here: it will remove leading full-width indent (U+3000) from the first paragraph.
            writeln!(f, "{}", content.trim_end())?;
        }

        return Ok(());
    }

    let mut f = File::create(path)?;

    writeln!(f, "书名：{}", manager.book_name)?;
    if !manager.author.trim().is_empty() {
        writeln!(f, "作者：{}", manager.author)?;
    }
    writeln!(f, "book_id={}", manager.book_id)?;

    let status_text = match manager.finished {
        Some(true) => "完结",
        Some(false) => "连载",
        None => "未知",
    };
    writeln!(f, "状态：{}", status_text)?;

    if let Some(score) = manager.score {
        writeln!(f, "评分：{:.1}", score)?;
    }
    if let Some(word_count) = manager.word_count {
        writeln!(f, "字数：{}", word_count)?;
    }
    if let Some(chapter_count) = manager.chapter_count {
        writeln!(f, "章节：{}", chapter_count)?;
    }
    if let Some(category) = manager.category.as_deref()
        && !category.trim().is_empty()
    {
        writeln!(f, "分类：{}", category.trim())?;
    }
    if !manager.tags.trim().is_empty() {
        writeln!(f, "标签：{}", manager.tags)?;
    }
    if let Some(read_count_text) = manager.read_count_text.as_deref()
        && !read_count_text.trim().is_empty()
    {
        writeln!(f, "在读：{}", read_count_text.trim())?;
    }

    if !manager.description.trim().is_empty() {
        writeln!(f)?;
        writeln!(f, "简介：")?;
        writeln!(f, "{}", manager.description.trim())?;
    }

    writeln!(f)?;
    writeln!(f, "{}", "=".repeat(40))?;
    writeln!(f)?;

    let mut last_volume: Option<String> = None;

    for ch in chapters {
        let chapter_id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let title = ch.get("title").and_then(|v| v.as_str()).unwrap_or("章节");
        let content = ch.get("content").and_then(|v| v.as_str()).unwrap_or("");

        if let Some(vol) = volume_title_by_chapter_id.get(chapter_id)
            && !vol.trim().is_empty()
            && last_volume.as_deref() != Some(vol.trim())
        {
            writeln!(f, "【{}】\n", vol.trim())?;
            last_volume = Some(vol.trim().to_string());
        }
        writeln!(f, "{}\n", title)?;
        // Do not `trim()` here: it will remove leading full-width indent (U+3000) from the first paragraph.
        writeln!(f, "{}\n", content.trim_end())?;
        writeln!(f, "\n----------------------------------------\n")?;
    }
    Ok(())
}

fn volume_title_map_for_chapters(
    chapters: &[Value],
    directory_raw: Option<&Value>,
) -> HashMap<String, String> {
    let Some(raw) = directory_raw else {
        return HashMap::new();
    };

    let known_chapter_ids: HashSet<String> = chapters
        .iter()
        .filter_map(|ch| ch.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();

    if known_chapter_ids.is_empty() {
        return HashMap::new();
    }

    let volumes = extract_volume_to_chapter_ids(raw, &known_chapter_ids);
    let mut volume_order: Vec<String> = Vec::new();
    let mut volume_title_by_chapter_id: HashMap<String, String> = HashMap::new();

    for (title, ids) in &volumes {
        let t = title.trim();
        if t.is_empty() || ids.is_empty() {
            continue;
        }
        if !volume_order.contains(&t.to_string()) {
            volume_order.push(t.to_string());
        }
        for id in ids {
            volume_title_by_chapter_id
                .entry(id.clone())
                .or_insert_with(|| t.to_string());
        }
    }

    let skip_default_single =
        volume_order.len() == 1 && is_default_volume_name(volume_order[0].as_str());
    if skip_default_single {
        return HashMap::new();
    }

    volume_title_by_chapter_id
}

fn is_default_volume_name(name: &str) -> bool {
    let s = name.trim();
    if s.is_empty() {
        return true;
    }
    let normalized: String = s
        .chars()
        .map(|c| match c {
            '：' | ':' | '—' | '–' | '_' | '·' | '｜' | '|' => ' ',
            _ => c,
        })
        .collect();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = normalized.to_lowercase();

    let exact = [
        "默认",
        "默认卷",
        "默认分卷",
        "第一卷",
        "第1卷",
        "卷一",
        "卷1",
        "第一卷 默认",
        "第1卷 默认",
    ];
    for e in exact {
        if lower == e {
            return true;
        }
    }

    let lower_ascii = lower.replace(['.', '_'], " ");
    let lower_ascii = lower_ascii.trim();
    if lower_ascii == "volume 1" || lower_ascii == "vol 1" || lower_ascii == "vol1" {
        return true;
    }
    false
}

fn extract_volume_to_chapter_ids(
    directory_raw: &Value,
    known_chapter_ids: &HashSet<String>,
) -> Vec<(String, Vec<String>)> {
    fn pick_string_or_number(v: Option<&Value>) -> Option<String> {
        match v {
            Some(Value::String(s)) => {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    }

    fn pick_volume_title(obj: &serde_json::Map<String, Value>) -> Option<String> {
        let candidates = [
            "volume_title",
            "volume_name",
            "section_title",
            "section_name",
            "group_title",
            "group_name",
        ];
        for k in candidates {
            if let Some(Value::String(s)) = obj.get(k) {
                let t = s.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        None
    }

    fn pick_title(obj: &serde_json::Map<String, Value>) -> Option<String> {
        let candidates = [
            "volume_title",
            "volume_name",
            "catalog_name",
            "catalog_title",
            "section_title",
            "section_name",
            "group_title",
            "group_name",
            "title",
            "name",
        ];
        for k in candidates {
            if let Some(Value::String(s)) = obj.get(k) {
                let t = s.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        None
    }

    fn looks_like_chapter_obj(obj: &serde_json::Map<String, Value>) -> Option<String> {
        let id = pick_string_or_number(
            obj.get("chapter_id")
                .or_else(|| obj.get("item_id"))
                .or_else(|| obj.get("catalog_id"))
                .or_else(|| obj.get("id")),
        )?;
        Some(id)
    }

    fn push_chapter(
        title: &str,
        chapter_id: &str,
        known_chapter_ids: &HashSet<String>,
        idx_by_title: &mut HashMap<String, usize>,
        out: &mut Vec<(String, Vec<String>)>,
    ) {
        let t = title.trim();
        if t.is_empty() {
            return;
        }
        if !known_chapter_ids.contains(chapter_id) {
            return;
        }
        let i = if let Some(i) = idx_by_title.get(t) {
            *i
        } else {
            let i = out.len();
            out.push((t.to_string(), Vec::new()));
            idx_by_title.insert(t.to_string(), i);
            i
        };

        let ids = &mut out[i].1;
        if ids.last().is_some_and(|last| last == chapter_id) {
            return;
        }
        if !ids.contains(&chapter_id.to_string()) {
            ids.push(chapter_id.to_string());
        }
    }

    fn visit(
        v: &Value,
        current_volume: Option<&str>,
        known_chapter_ids: &HashSet<String>,
        idx_by_title: &mut HashMap<String, usize>,
        out: &mut Vec<(String, Vec<String>)>,
    ) {
        match v {
            Value::Array(arr) => {
                for it in arr {
                    visit(it, current_volume, known_chapter_ids, idx_by_title, out);
                }
            }
            Value::Object(obj) => {
                let is_chapter = looks_like_chapter_obj(obj).is_some();

                if let Some(vol) = pick_volume_title(obj)
                    && let Some(id) = looks_like_chapter_obj(obj)
                {
                    push_chapter(&vol, &id, known_chapter_ids, idx_by_title, out);
                }

                if let Some(vol) = current_volume
                    && let Some(id) = looks_like_chapter_obj(obj)
                {
                    push_chapter(vol, &id, known_chapter_ids, idx_by_title, out);
                }

                let title_here = if is_chapter {
                    pick_volume_title(obj)
                } else {
                    pick_title(obj)
                };
                let next_volume = title_here.as_deref().or(current_volume);

                let child_keys = [
                    "catalog_data",
                    "item_data_list",
                    "items",
                    "item_list",
                    "children",
                    "child_list",
                    "sub_items",
                    "sub_item_list",
                    "chapter_list",
                    "chapters",
                    "chapter_ids",
                ];

                for k in child_keys {
                    if let Some(arr) = obj.get(k).and_then(Value::as_array) {
                        for child in arr {
                            if let (Some(vol), Some(id)) =
                                (next_volume, pick_string_or_number(Some(child)))
                            {
                                push_chapter(vol, &id, known_chapter_ids, idx_by_title, out);
                            } else {
                                visit(child, next_volume, known_chapter_ids, idx_by_title, out);
                            }
                        }
                    }
                }

                for (k, vv) in obj {
                    if child_keys.contains(&k.as_str()) {
                        continue;
                    }
                    visit(vv, next_volume, known_chapter_ids, idx_by_title, out);
                }
            }
            _ => {}
        }
    }

    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut idx_by_title: HashMap<String, usize> = HashMap::new();

    if let Some(items) = directory_raw
        .get("item_data_list")
        .and_then(Value::as_array)
    {
        let mut current: Option<String> = None;
        for it in items {
            let Some(obj) = it.as_object() else {
                continue;
            };
            if let Some(vol) = pick_volume_title(obj) {
                current = Some(vol);
            }
            let Some(id) = looks_like_chapter_obj(obj) else {
                continue;
            };
            if let Some(vol) = current.as_deref() {
                push_chapter(vol, &id, known_chapter_ids, &mut idx_by_title, &mut out);
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    visit(
        directory_raw,
        None,
        known_chapter_ids,
        &mut idx_by_title,
        &mut out,
    );
    out
}

/// 下载完后让用户选择使用哪个书名（仅 CLI 模式）。
///
/// 从 `BookManager` 收集所有可用的书名变体，展示编号让用户选择。
/// 返回 `Some(chosen_name)` 表示用户选了新名字，`None` 表示保持默认。
fn prompt_book_name_selection(manager: &BookManager) -> Option<String> {
    // 收集可用的书名选项（去重）
    let mut options: Vec<(&str, String)> = Vec::new();

    let default_name = &manager.book_name;
    options.push(("默认书名", default_name.clone()));

    if let Some(orig) = &manager.original_book_name
        && !orig.is_empty()
        && orig != default_name
    {
        options.push(("原始书名", orig.clone()));
    }

    if let Some(short) = &manager.book_short_name
        && !short.is_empty()
        && short != default_name
    {
        // 也和 original_book_name 去重
        let dup = manager
            .original_book_name
            .as_ref()
            .is_some_and(|o| o == short);
        if !dup {
            options.push(("短书名", short.clone()));
        }
    }

    // 如果只有一个选项就不需要询问
    if options.len() <= 1 {
        return None;
    }

    println!("\n=== 选择书名 ===");
    for (idx, (label, name)) in options.iter().enumerate() {
        let marker = if idx == 0 { " (当前)" } else { "" };
        println!("  {}. {}: {}{}", idx + 1, label, name, marker);
    }

    print!("请选择 [1]: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return None;
    }

    let choice = line.trim();
    let idx = if choice.is_empty() {
        1
    } else {
        match choice.parse::<usize>() {
            Ok(i) if i >= 1 && i <= options.len() => i,
            _ => return None,
        }
    };

    let chosen = options[idx - 1].1.clone();
    if chosen == *default_name {
        return None;
    }
    Some(chosen)
}
