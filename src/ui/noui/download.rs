use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use tomato_novel_official_api::{ChapterRef, SearchClient};

use crate::base_system::context::Config;
use crate::download::downloader as dl;

pub(super) fn search_and_pick(keyword: &str) -> Result<Option<String>> {
    let client = SearchClient::new().context("init SearchClient")?;
    let resp = client
        .search_books(keyword)
        .with_context(|| format!("搜索失败: {}", keyword))?;

    if resp.books.is_empty() {
        println!("未搜索到结果\n");
        return Ok(None);
    }

    println!("\n===== 搜索结果 =====");
    for (idx, b) in resp.books.iter().enumerate() {
        println!(
            "{}. 书名: {} | ID: {} | 作者: {}",
            idx + 1,
            b.title.as_deref().unwrap_or(""),
            b.book_id,
            b.author.as_deref().unwrap_or("")
        );
    }
    println!("0. 取消\n");

    let choice = super::read_line("请输入编号：")?;
    let choice = choice.trim();
    if choice == "0" || choice.eq_ignore_ascii_case("q") {
        return Ok(None);
    }
    if let Ok(idx) = choice.parse::<usize>() {
        if idx >= 1 && idx <= resp.books.len() {
            return Ok(Some(resp.books[idx - 1].book_id.clone()));
        }
    }

    println!("输入无效，已取消\n");
    Ok(None)
}

pub(super) fn download_book(book_id: &str, config: &Config) -> Result<()> {
    let start_time = Instant::now();

    let plan = dl::prepare_download_plan(config, book_id, dl::BookMeta::default())
        .with_context(|| format!("准备下载计划失败: book_id={}", book_id))?;

    let book_name = plan
        .meta
        .book_name
        .clone()
        .unwrap_or_else(|| plan.book_id.clone());

    // 打印书籍信息（对齐 old_main.py 的信息展示）
    println!("\n书名: {}", book_name);
    if let Some(author) = plan.meta.author.as_deref() {
        println!("作者: {}", author);
    }
    if let Some(finished) = plan.meta.finished {
        println!("是否完结: {}", if finished { "完结" } else { "连载" });
    }
    if let Some(count) = plan.meta.chapter_count {
        println!("章节数: {}", count);
    }
    if !plan.meta.tags.is_empty() {
        println!("标签: {}", plan.meta.tags.join("|"));
    }
    if let Some(desc) = plan.meta.description.as_deref() {
        let mut short = desc.to_string();
        if short.chars().count() > 50 {
            short = short.chars().take(50).collect::<String>() + "...";
        }
        println!("简介: {}", short);
    }

    // 初始化 BookManager 并尝试加载历史状态
    let mut manager = dl::init_manager_from_plan(config, &plan)?;
    let resumed =
        manager.load_existing_status(&manager.book_id.clone(), &manager.book_name.clone());
    if resumed {
        println!("\n已检测到历史下载记录，可继续下载或选择重新下载。\n");
    }

    // 若封面已经下载到状态目录，尝试 ASCII 预览
    if let Some(cover) = find_cover_image(manager.book_folder()) {
        let _ = preview_cover_ascii(&cover);
    }

    let total = plan.chapters.len();
    let (downloaded_ok, failed_count) = count_download_state(&manager, &plan.chapters);
    println!(
        "共发现 {} 章，下载失败 {} 章，已下载 {} 章",
        total, failed_count, downloaded_ok
    );

    let mut range: Option<dl::ChapterRange> = None;
    let mode = if downloaded_ok > 0 || failed_count > 0 {
        select_download_mode(failed_count > 0)?
    } else {
        DownloadMode::RangeOrAll
    };

    match mode {
        DownloadMode::Cancel => {
            let _ = manager.cleanup_status_folder();
            return Ok(());
        }
        DownloadMode::Full => {
            manager.downloaded.clear();
            println!("将重新下载全部章节");
        }
        DownloadMode::RangeIgnoreHistory | DownloadMode::RangeOrAll => {
            range = prompt_range(total)?;
            if matches!(mode, DownloadMode::RangeIgnoreHistory) {
                manager.downloaded.clear();
            }
        }
        DownloadMode::Resume | DownloadMode::FailedOnly => {}
    }

    let chosen_chapters = apply_range(&plan.chapters, range);
    if chosen_chapters.is_empty() {
        println!("范围无效或章节为空\n");
        let _ = manager.cleanup_status_folder();
        return Ok(());
    }

    let mut pending = match mode {
        DownloadMode::FailedOnly => dl::pending_failed(&manager, &chosen_chapters),
        _ => dl::pending_resume(&manager, &chosen_chapters),
    };

    if matches!(mode, DownloadMode::Resume) {
        println!(
            "继续下载剩余章节: {} 章 (已完成 {})",
            pending.len(),
            chosen_chapters.len().saturating_sub(pending.len())
        );
    }

    if pending.is_empty() {
        println!("没有需要下载的章节，操作结束。\n");
        dl::finalize_from_manager(&mut manager, &chosen_chapters)?;
        return Ok(());
    }

    println!("\n开始下载...");
    loop {
        let mut reporter = dl::make_reporter(config, &chosen_chapters, &pending, None);
        let book_name = manager.book_name.clone();
        let result = dl::download_chapters_into_manager(
            config,
            &plan.book_id,
            &book_name,
            &mut manager,
            &pending,
            &mut reporter,
            None,
        )?;
        println!(
            "\n下载完成（阶段）成功: {} 章 | 失败: {} 章 | 取消: {} 章",
            result.success, result.failed, result.canceled
        );

        pending = dl::pending_failed(&manager, &chosen_chapters);
        if pending.is_empty() {
            break;
        }

        let ans = super::read_line("是否重新下载错误章节？[Y/n]: ")?;
        let ans = ans.trim().to_ascii_lowercase();
        if ans == "n" {
            println!("失败章节已保留在缓存/状态文件中。\n");
            break;
        }
        println!("\n重新下载失败章节: {} 章...", pending.len());
    }

    dl::finalize_from_manager(&mut manager, &chosen_chapters)?;
    println!(
        "\n下载完成！用时 {:.1} 秒",
        start_time.elapsed().as_secs_f32()
    );
    println!("已保存到 {}", manager.default_save_dir().display());
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadMode {
    Resume,
    Full,
    FailedOnly,
    RangeIgnoreHistory,
    RangeOrAll,
    Cancel,
}

fn select_download_mode(has_failed: bool) -> Result<DownloadMode> {
    println!("\n===== 下载模式选择 =====");
    println!("1. 继续下载未完成章节");
    println!("2. 全部重新下载");
    if has_failed {
        println!("3. 仅重新下载失败章节");
    }
    println!("4. 指定章节范围重新下载 (忽略历史记录)");
    println!("q. 取消");
    let sel = super::read_line("请选择(默认1): ")?;
    let sel = sel.trim().to_ascii_lowercase();
    let mode = match sel.as_str() {
        "" | "1" => DownloadMode::Resume,
        "2" => DownloadMode::Full,
        "3" if has_failed => DownloadMode::FailedOnly,
        "4" => DownloadMode::RangeIgnoreHistory,
        "q" => DownloadMode::Cancel,
        _ => DownloadMode::Resume,
    };
    Ok(mode)
}

fn prompt_range(total: usize) -> Result<Option<dl::ChapterRange>> {
    let text = super::read_line("输入章节范围 形如 10~200 (留空表示全部): ")?;
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let Some((a, b)) = text.split_once('~') else {
        println!("范围格式错误，应为 a~b，将使用全部章节");
        return Ok(None);
    };
    let Ok(mut start) = a.trim().parse::<usize>() else {
        println!("范围解析失败，将使用全部章节");
        return Ok(None);
    };
    let Ok(mut end) = b.trim().parse::<usize>() else {
        println!("范围解析失败，将使用全部章节");
        return Ok(None);
    };
    if start == 0 {
        start = 1;
    }
    if end == 0 {
        end = 1;
    }
    start = start.min(total).max(1);
    end = end.min(total).max(1);
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    println!("已选择章节范围: {}~{}", start, end);
    Ok(Some(dl::ChapterRange { start, end }))
}

fn apply_range(chapters: &[ChapterRef], range: Option<dl::ChapterRange>) -> Vec<ChapterRef> {
    let total = chapters.len();
    match range {
        None => chapters.to_vec(),
        Some(r) => {
            if r.start == 0 || r.start > r.end {
                return Vec::new();
            }
            let start_idx = r.start.saturating_sub(1);
            let end_idx = r.end.min(total).saturating_sub(1);
            if start_idx >= chapters.len() {
                return Vec::new();
            }
            chapters
                .iter()
                .skip(start_idx)
                .take(end_idx.saturating_sub(start_idx) + 1)
                .cloned()
                .collect()
        }
    }
}

fn count_download_state(
    manager: &crate::book_parser::book_manager::BookManager,
    chapters: &[ChapterRef],
) -> (usize, usize) {
    let mut ok = 0usize;
    let mut failed = 0usize;
    for ch in chapters {
        match manager.downloaded.get(&ch.id) {
            Some((_, Some(_))) => ok += 1,
            Some((_, None)) => failed += 1,
            None => {}
        }
    }
    (ok, failed)
}

fn find_cover_image(folder: &Path) -> Option<PathBuf> {
    let rd = fs::read_dir(folder).ok()?;
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
            return Some(p);
        }
    }
    None
}

fn preview_cover_ascii(image_path: &Path) -> Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let cols = cols.max(40) as u32;
    let rows = rows.max(10) as u32;
    println!(
        "\n{}封面预览{}",
        "=".repeat((cols as usize).saturating_sub(16) / 2),
        "=".repeat((cols as usize).saturating_sub(16) / 2)
    );

    let img = image::open(image_path)
        .with_context(|| format!("打开封面失败: {}", image_path.display()))?;
    let gray = img.to_luma8();

    // 字符宽高比矫正：字符通常更“高”，所以宽度多取一些、并降低高度
    let target_w = cols;
    let target_h = (rows.saturating_sub(6)).max(8);
    let resized = image::imageops::resize(
        &gray,
        target_w,
        target_h,
        image::imageops::FilterType::Triangle,
    );

    const PALETTE: &[u8] = b" .:-=+*#%@";
    for y in 0..resized.height() {
        let mut line = String::with_capacity(resized.width() as usize);
        for x in 0..resized.width() {
            let v = resized.get_pixel(x, y)[0] as usize;
            let idx = v * (PALETTE.len() - 1) / 255;
            line.push(PALETTE[idx] as char);
        }
        println!("{}", line);
    }
    println!();
    Ok(())
}
