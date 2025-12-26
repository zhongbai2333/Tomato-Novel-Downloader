//! TUI 预览页（内容/片段展示）。

use std::thread;

use anyhow::{Result, anyhow};
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::prelude::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};
use tracing::{info, warn};

use crate::base_system::context::safe_fs_name;
use crate::download::downloader::{self, BookMeta, ChapterRange, ProgressSnapshot, SavePhase};

use super::download::{request_cancel_download, start_download_task};
use super::update::{expected_book_folder, read_downloaded_count};
use super::{
    App, Focus, PendingDownload, PreviewFocus, PreviewModalLayout, View, WorkerMsg,
    format_word_count, render_log_box, start_spinner, truncate, upsert_result_detail_from_plan,
};

pub(super) fn handle_event_preview(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Paste(s) => {
            if app.preview_focus == PreviewFocus::Range {
                app.preview_range.push_str(&s);
            }
        }
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                cancel_preview(app);
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                request_cancel_download(app);
            }
            KeyCode::Tab => {
                app.preview_focus = match app.preview_focus {
                    PreviewFocus::Range => PreviewFocus::Buttons,
                    PreviewFocus::Buttons => PreviewFocus::Range,
                };
            }
            KeyCode::Backspace => {
                if app.preview_focus == PreviewFocus::Range {
                    app.preview_range.pop();
                }
            }
            KeyCode::Enter => {
                if app.preview_focus == PreviewFocus::Range {
                    confirm_preview(app)?;
                } else {
                    match app.preview_buttons.selected().unwrap_or(0) {
                        0 => confirm_preview(app)?,
                        _ => cancel_preview(app),
                    }
                }
            }
            KeyCode::Up => {
                if app.preview_focus == PreviewFocus::Buttons {
                    let sel = app.preview_buttons.selected().unwrap_or(0);
                    let next = if sel == 0 { 1 } else { sel - 1 };
                    app.preview_buttons.select(Some(next.min(1)));
                } else {
                    preview_scroll_up(app, 1);
                }
            }
            KeyCode::Down => {
                if app.preview_focus == PreviewFocus::Buttons {
                    let sel = app.preview_buttons.selected().unwrap_or(0);
                    let next = if sel >= 1 { 0 } else { sel + 1 };
                    app.preview_buttons.select(Some(next.min(1)));
                } else {
                    preview_scroll_down(app, 1);
                }
            }
            KeyCode::PageUp => preview_scroll_up(app, 5),
            KeyCode::PageDown => preview_scroll_down(app, 5),
            KeyCode::Home => preview_scroll_to_top(app),
            KeyCode::End => preview_scroll_to_bottom(app),
            KeyCode::Left => {
                if app.preview_focus == PreviewFocus::Buttons {
                    let sel = app.preview_buttons.selected().unwrap_or(0);
                    let next = if sel == 0 { 1 } else { sel - 1 };
                    app.preview_buttons.select(Some(next.min(1)));
                }
            }
            KeyCode::Right => {
                if app.preview_focus == PreviewFocus::Buttons {
                    let sel = app.preview_buttons.selected().unwrap_or(0);
                    let next = if sel >= 1 { 0 } else { sel + 1 };
                    app.preview_buttons.select(Some(next.min(1)));
                }
            }
            KeyCode::Char(c)
                if app.preview_focus == PreviewFocus::Range
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                app.preview_range.push(c);
            }
            _ => {}
        },
        Event::Resize(_, _) => {}
        Event::Mouse(me) => handle_mouse_preview(app, me)?,
        _ => {}
    }
    Ok(())
}

pub(super) fn handle_mouse_preview(app: &mut App, me: event::MouseEvent) -> Result<()> {
    let pos_in = |area: Rect, col: u16, row: u16| {
        col >= area.x
            && col < area.x.saturating_add(area.width)
            && row >= area.y
            && row < area.y.saturating_add(area.height)
    };

    if let Some(stop_area) = app.stop_button_area
        && matches!(me.kind, MouseEventKind::Down(MouseButton::Left))
        && pos_in(stop_area, me.column, me.row)
    {
        request_cancel_download(app);
        return Ok(());
    }

    if matches!(
        me.kind,
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
    ) {
        let up = matches!(me.kind, MouseEventKind::ScrollUp);
        if let Some(layout) = app.last_preview_modal.clone()
            && pos_in(layout.info, me.column, me.row)
        {
            if up {
                preview_scroll_up(app, 1);
            } else {
                preview_scroll_down(app, 1);
            }
            return Ok(());
        }
        if let Some(area) = app.last_preview_desc_area
            && pos_in(area, me.column, me.row)
        {
            if up {
                preview_scroll_up(app, 1);
            } else {
                preview_scroll_down(app, 1);
            }
            return Ok(());
        }
    }

    if let Some(layout) = app.last_preview_modal.clone() {
        if matches!(me.kind, MouseEventKind::Down(MouseButton::Left)) {
            if pos_in(layout.range, me.column, me.row) {
                app.preview_focus = PreviewFocus::Range;
                return Ok(());
            }
            if pos_in(layout.buttons, me.column, me.row) {
                let idx = me.row.saturating_sub(layout.buttons.y + 1) as usize;
                let picked = idx.min(1);
                app.preview_buttons.select(Some(picked));
                if picked == 0 {
                    confirm_preview(app)?;
                } else {
                    cancel_preview(app);
                }
                return Ok(());
            }
        }
        if matches!(me.kind, MouseEventKind::Moved) {
            if pos_in(layout.buttons, me.column, me.row) {
                let idx = me.row.saturating_sub(layout.buttons.y + 1) as usize;
                app.preview_buttons.select(Some(idx.min(1)));
                app.preview_focus = PreviewFocus::Buttons;
                return Ok(());
            }
            if pos_in(layout.range, me.column, me.row) {
                app.preview_focus = PreviewFocus::Range;
                return Ok(());
            }
        }
    }

    Ok(())
}

pub(super) fn parse_range_input(input: &str, total: usize) -> Result<Option<ChapterRange>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() > 2 {
        return Err(anyhow!("格式应为 start-end，例如 1-10"));
    }

    let start_part = parts.first().copied().unwrap_or("").trim();
    let end_part = parts.get(1).copied().unwrap_or("").trim();

    let start = if start_part.is_empty() {
        1
    } else {
        start_part
            .parse::<usize>()
            .map_err(|_| anyhow!("起始章节需为数字"))?
    };
    let end = if end_part.is_empty() {
        total
    } else {
        end_part
            .parse::<usize>()
            .map_err(|_| anyhow!("结束章节需为数字"))?
    };

    if start == 0 || end == 0 {
        return Err(anyhow!("章节编号需大于 0"));
    }
    if start > end {
        return Err(anyhow!("起始章节不能大于结束章节"));
    }
    if start > total {
        return Err(anyhow!("起始章节超过目录长度"));
    }

    Ok(Some(ChapterRange {
        start,
        end: end.min(total),
    }))
}

pub(super) fn start_preview_task(app: &mut App, book_id: String, hint: BookMeta) -> Result<()> {
    app.pending_download = None;
    app.messages.clear();
    app.cover_lines.clear();
    app.cover_title.clear();
    app.download_progress = None;
    app.download_cancel_flag = None;
    app.stop_button_area = None;
    app.preview_desc_scroll = 0;
    app.preview_desc_scroll_max = 0;
    app.preview_modal_scroll = 0;
    app.preview_modal_scroll_max = 0;
    app.last_preview_desc_area = None;
    info!(target: "ui", book_id = %book_id, "开始加载目录/预览");
    start_spinner(app, format!("加载目录: {book_id}"));
    let tx = app.worker_tx.clone();
    let cfg = app.config.clone();
    thread::spawn(move || {
        let result = downloader::prepare_download_plan(&cfg, &book_id, hint).map(|plan| {
            let folder = expected_book_folder(&cfg, &plan);
            let downloaded = read_downloaded_count(&folder, &plan.book_id).unwrap_or(0);
            PendingDownload {
                plan,
                downloaded_count: downloaded,
            }
        });
        let _ = tx.send(WorkerMsg::PreviewReady(Box::new(result)));
    });
    Ok(())
}

pub(super) fn confirm_preview(app: &mut App) -> Result<()> {
    let pending = match app.pending_download.clone() {
        Some(p) => p,
        None => return Ok(()),
    };

    let total = pending.plan.chapters.len();
    let input = app.preview_range.trim();
    let range = if input.is_empty() {
        None
    } else {
        match parse_range_input(input, total) {
            Ok(r) => r,
            Err(err) => {
                app.status = format!("范围无效: {err}");
                return Ok(());
            }
        }
    };

    app.preview_range.clear();
    app.preview_buttons.select(Some(0));
    app.view = View::Preview;
    app.focus = Focus::Input;
    app.input.clear();

    start_download_task(app, pending, range)
}

pub(super) fn cancel_preview(app: &mut App) {
    // If preview downloaded cover into a fresh folder, clean it up on cancel.
    cleanup_preview_cover_artifacts(app);

    app.pending_download = None;
    app.preview_range.clear();
    app.preview_buttons.select(Some(0));
    app.preview_modal_open = false;
    app.download_progress = None;
    app.preview_desc_scroll = 0;
    app.preview_desc_scroll_max = 0;
    app.preview_modal_scroll = 0;
    app.preview_modal_scroll_max = 0;
    app.last_preview_desc_area = None;
    app.view = View::Home;
    app.focus = Focus::Input;
    app.status = "已取消预览".to_string();
    app.download_cancel_flag = None;
    app.stop_button_area = None;
}

pub(super) fn cleanup_preview_cover_artifacts(app: &mut App) {
    if !app.config.auto_clear_dump {
        return;
    }
    let Some(pending) = app.pending_download.as_ref() else {
        return;
    };
    let Some(book_name) = pending.plan.meta.book_name.as_deref() else {
        return;
    };

    // NOTE: preview currently downloads cover into the same book folder name as status folder.
    // Only delete when the folder contains *only* cover-like files and no status.json.
    let dir = crate::base_system::book_paths::book_folder_path(
        &app.config,
        &pending.plan.book_id,
        Some(book_name),
    );
    if !dir.exists() {
        return;
    }
    if dir.join("status.json").exists() {
        return;
    }

    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return;
    };

    let safe_name = safe_fs_name(book_name, "_", 120);
    let mut entries: Vec<std::path::PathBuf> = Vec::new();
    for ent in read_dir.flatten() {
        entries.push(ent.path());
    }
    if entries.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let is_cover_like = |p: &std::path::Path| {
        if p.is_dir() {
            return false;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        let Some(ext) = p.extension().and_then(|s| s.to_str()) else {
            return false;
        };
        let ext = ext.to_ascii_lowercase();
        let is_img = matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif");
        if !is_img {
            return false;
        }
        stem == safe_name || stem.eq_ignore_ascii_case("cover")
    };

    // Abort if there are non-cover files.
    if entries.iter().any(|p| !is_cover_like(p)) {
        return;
    }

    for p in &entries {
        let _ = std::fs::remove_file(p);
    }

    // Remove dir if empty.
    if crate::base_system::file_cleaner::is_empty_dir(&dir).unwrap_or(false) {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

pub(super) fn wrapped_line_count(text: &str, width: u16) -> usize {
    let w = width.max(1) as usize;
    let mut total = 0usize;
    for line in text.lines() {
        let wrapped = textwrap::wrap(line, w);
        total = total.saturating_add(wrapped.len().max(1));
    }
    total.max(1)
}

pub(super) fn preview_scroll_up(app: &mut App, lines: u16) {
    if app.preview_modal_open {
        app.preview_modal_scroll = app.preview_modal_scroll.saturating_sub(lines);
    } else {
        app.preview_desc_scroll = app.preview_desc_scroll.saturating_sub(lines);
    }
}

pub(super) fn preview_scroll_down(app: &mut App, lines: u16) {
    if app.preview_modal_open {
        let max = app.preview_modal_scroll_max;
        app.preview_modal_scroll = (app.preview_modal_scroll.saturating_add(lines)).min(max);
    } else {
        let max = app.preview_desc_scroll_max;
        app.preview_desc_scroll = (app.preview_desc_scroll.saturating_add(lines)).min(max);
    }
}

pub(super) fn preview_scroll_to_top(app: &mut App) {
    if app.preview_modal_open {
        app.preview_modal_scroll = 0;
    } else {
        app.preview_desc_scroll = 0;
    }
}

pub(super) fn preview_scroll_to_bottom(app: &mut App) {
    if app.preview_modal_open {
        app.preview_modal_scroll = app.preview_modal_scroll_max;
    } else {
        app.preview_desc_scroll = app.preview_desc_scroll_max;
    }
}

pub(super) fn draw_preview(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.size();
    let progress_height: u16 = 7;
    let log_height = area.height.saturating_sub(progress_height);

    let log_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: log_height.max(1),
    };
    // When downloading, show a small description pane above logs.
    // Keep it minimal: only render if we have enough vertical space.
    let desc_h: u16 = 5;
    let min_log_h: u16 = 3;
    let can_show_desc = !app.preview_modal_open
        && app.pending_download.is_some()
        && log_area.height > desc_h.saturating_add(min_log_h);

    if can_show_desc {
        let desc_area = Rect {
            x: log_area.x,
            y: log_area.y,
            width: log_area.width,
            height: desc_h.min(log_area.height),
        };
        app.last_preview_desc_area = Some(desc_area);
        let log_rest = Rect {
            x: log_area.x,
            y: log_area.y.saturating_add(desc_area.height),
            width: log_area.width,
            height: log_area.height.saturating_sub(desc_area.height).max(1),
        };

        let desc_text = app
            .pending_download
            .as_ref()
            .and_then(|p| p.plan.meta.description.as_deref())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("暂无简介");

        let desc_block = Block::default()
            .borders(Borders::ALL)
            .title("简介 (↑↓/滚轮)");
        frame.render_widget(desc_block.clone(), desc_area);
        let inner = desc_block.inner(desc_area);

        let full_lines = wrapped_line_count(desc_text, inner.width);
        let visible_h = inner.height as usize;
        let scrollable = full_lines > visible_h && inner.width > 1;
        let (text_area, scroll_area, total_lines) = if scrollable {
            let text_w = inner.width.saturating_sub(1).max(1);
            let total = wrapped_line_count(desc_text, text_w);
            (
                Rect {
                    x: inner.x,
                    y: inner.y,
                    width: text_w,
                    height: inner.height,
                },
                Some(Rect {
                    x: inner.x.saturating_add(text_w),
                    y: inner.y,
                    width: 1,
                    height: inner.height,
                }),
                total,
            )
        } else {
            (inner, None, full_lines)
        };

        let max_scroll = total_lines
            .saturating_sub(text_area.height as usize)
            .min(u16::MAX as usize) as u16;
        app.preview_desc_scroll_max = max_scroll;
        app.preview_desc_scroll = app.preview_desc_scroll.min(app.preview_desc_scroll_max);

        let para = Paragraph::new(desc_text.to_string())
            .wrap(Wrap { trim: true })
            .scroll((app.preview_desc_scroll, 0));
        frame.render_widget(para, text_area);
        if let Some(sb_area) = scroll_area {
            let mut state =
                ScrollbarState::new(total_lines).position(app.preview_desc_scroll as usize);
            let sb = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
            frame.render_stateful_widget(sb, sb_area, &mut state);
        }

        render_log_box(frame, log_rest, app);
    } else {
        app.last_preview_desc_area = None;
        app.preview_desc_scroll = 0;
        app.preview_desc_scroll_max = 0;
        render_log_box(frame, log_area, app);
    }
    app.last_preview_layout = Some([log_area, Rect::default()]);

    let progress_area = Rect {
        x: area.x,
        y: area.y.saturating_add(log_area.height),
        width: area.width,
        height: progress_height.min(area.height.saturating_sub(log_area.height)),
    };
    if let Some(layout) = app.last_preview_layout.as_mut() {
        layout[1] = progress_area;
    }
    let empty = ProgressSnapshot::default();
    let snap = app.download_progress.as_ref().unwrap_or(&empty);

    let show_comments = app.config.enable_segment_comments && snap.comment_total > 0;
    let mut items: Vec<(&str, usize, usize, Color)> = Vec::new();
    items.push((
        "组下载",
        snap.group_done,
        snap.group_total.max(1),
        Color::LightCyan,
    ));
    items.push((
        match snap.save_phase {
            SavePhase::Audiobook => "有声书",
            SavePhase::TextSave => "正文保存",
        },
        snap.saved_chapters,
        snap.chapter_total.max(1),
        Color::Green,
    ));
    if show_comments {
        items.push((
            "段评抓取",
            snap.comment_fetch,
            snap.comment_total.max(1),
            Color::Yellow,
        ));
        items.push((
            "段评保存",
            snap.comment_saved,
            snap.comment_total.max(1),
            Color::Magenta,
        ));
    }

    let inner = Block::default().borders(Borders::ALL).title("进度");
    frame.render_widget(inner.clone(), progress_area);

    let inner_area = Rect {
        x: progress_area.x.saturating_add(1),
        y: progress_area.y.saturating_add(1),
        width: progress_area.width.saturating_sub(2).max(1),
        height: progress_area.height.saturating_sub(2).max(1),
    };

    app.stop_button_area = None;
    if !items.is_empty() && inner_area.height > 0 {
        let mut constraints = Vec::new();
        for _ in &items {
            constraints.push(Constraint::Length(1));
        }
        // extra line for stop button
        constraints.push(Constraint::Length(1));

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner_area);

        for (idx, (label, done, total, color)) in items.into_iter().enumerate() {
            if let Some(area) = rows.get(idx) {
                let ratio = if total == 0 {
                    0.0
                } else {
                    (done as f64 / total as f64).clamp(0.0, 1.0)
                };
                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(color))
                    .ratio(ratio)
                    .label(format!("{label} {done}/{total}"));
                frame.render_widget(gauge, *area);
            }
        }

        if let Some(btn_area) = rows.last() {
            let txt = if app.download_cancel_flag.is_some() {
                "[ 停止下载 ] (S/点击)"
            } else {
                ""
            };
            let para = Paragraph::new(txt)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
            frame.render_widget(para, *btn_area);
            if app.download_cancel_flag.is_some() {
                app.stop_button_area = Some(*btn_area);
            }
        }
    }

    app.last_preview_modal = None;
    if app.preview_modal_open {
        let modal_w = area.width.min(80).max(40.min(area.width));
        let modal_h = 18.min(area.height);
        let modal_x = area
            .x
            .saturating_add(area.width.saturating_sub(modal_w) / 2);
        let modal_y = area
            .y
            .saturating_add(log_area.height.saturating_sub(modal_h) / 2);
        let modal = Rect {
            x: modal_x,
            y: modal_y,
            width: modal_w,
            height: modal_h,
        };

        let inner = Rect {
            x: modal.x.saturating_add(1),
            y: modal.y.saturating_add(1),
            width: modal.width.saturating_sub(2).max(1),
            height: modal.height.saturating_sub(2).max(1),
        };

        let pending = app.pending_download.as_ref();
        let fallback_meta = BookMeta::default();
        let (title, original_title, author, total, downloaded, meta) = pending
            .map(|p| {
                (
                    p.plan
                        .meta
                        .book_name
                        .clone()
                        .unwrap_or_else(|| "预览".to_string()),
                    p.plan.meta.original_book_name.clone(),
                    p.plan.meta.author.clone(),
                    p.plan.chapters.len(),
                    p.downloaded_count,
                    &p.plan.meta,
                )
            })
            .unwrap_or(("预览".to_string(), None, None, 0, 0, &fallback_meta));

        let mut title_line = format!("《{}》", title);
        if let Some(orig) = original_title.as_ref()
            && !orig.is_empty()
        {
            title_line.push_str(&format!(" ({})", orig));
        }

        let mut meta_lines: Vec<Line> = Vec::new();
        let mut info_plain_lines: Vec<String> = Vec::new();
        let mut row1: Vec<String> = Vec::new();
        row1.push(format!("章节: {} (已下载 {})", total, downloaded));
        if let Some(done) = meta.finished {
            let label = if done { "完结" } else { "连载" };
            row1.push(format!("状态: {}", label));
        }
        if let Some(author) = author.as_ref()
            && !author.is_empty()
        {
            row1.push(format!("作者: {}", author));
        }
        let row1_s = row1.join(" | ");
        meta_lines.push(Line::from(row1_s.clone()));
        info_plain_lines.push(row1_s);

        if let Some(desc) = meta.description.as_ref() {
            if !desc.is_empty() {
                let desc = desc.trim();
                meta_lines.push(Line::from(format!("简介: {}", desc)));
                info_plain_lines.push(format!("简介: {}", desc));
            } else {
                meta_lines.push(Line::from("简介: 暂无"));
                info_plain_lines.push("简介: 暂无".to_string());
            }
        } else {
            meta_lines.push(Line::from("简介: 暂无"));
            info_plain_lines.push("简介: 暂无".to_string());
        }

        let mut row2: Vec<String> = Vec::new();
        if let Some(score) = meta.score {
            row2.push(format!("评分: {:.1}", score));
        }
        if let Some(words) = meta.word_count {
            row2.push(format!("字数: {}", format_word_count(words)));
        }
        if let Some(reads) = meta.read_count_text.as_ref().or(meta.read_count.as_ref()) {
            row2.push(format!("阅读: {}", reads));
        }
        if !row2.is_empty() {
            let row2_s = row2.join(" | ");
            meta_lines.push(Line::from(row2_s.clone()));
            info_plain_lines.push(row2_s);
        }

        let mut row3: Vec<String> = Vec::new();
        if let Some(cat) = meta.category.as_ref()
            && !cat.is_empty()
        {
            row3.push(format!("类别: {}", cat));
        }
        if !meta.tags.is_empty() {
            row3.push(format!("标签: {}", meta.tags.join(" | ")));
        }
        if !row3.is_empty() {
            let row3_s = row3.join(" | ");
            meta_lines.push(Line::from(row3_s.clone()));
            info_plain_lines.push(row3_s);
        }

        let mut row4: Vec<String> = Vec::new();
        if let Some(first) = meta.first_chapter_title.as_ref()
            && !first.is_empty()
        {
            row4.push(format!("首章: {}", truncate(first, 50)));
        }
        if let Some(last) = meta.last_chapter_title.as_ref()
            && !last.is_empty()
        {
            row4.push(format!("末章: {}", truncate(last, 50)));
        }
        if !row4.is_empty() {
            let row4_s = row4.join(" | ");
            meta_lines.push(Line::from(row4_s.clone()));
            info_plain_lines.push(row4_s);
        }

        let mut info_lines = Vec::new();
        info_lines.push(Line::from(Span::styled(
            title_line.clone(),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )));
        info_lines.extend(meta_lines);

        // Plain text copy for scroll range calculation
        let mut info_plain = String::new();
        info_plain.push_str(&title_line);
        for s in &info_plain_lines {
            info_plain.push('\n');
            info_plain.push_str(s);
        }

        let range_focus = app.preview_focus == PreviewFocus::Range;
        let range_style = if range_focus {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let range_line = Paragraph::new(format!("> {}", app.preview_range))
            .style(range_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("下载范围 (空=全部)"),
            );

        let buttons = ["确定", "取消"];
        let button_items: Vec<ListItem> = buttons.iter().map(|b| ListItem::new(*b)).collect();
        let button_style = if app.preview_focus == PreviewFocus::Buttons {
            Style::default().fg(Color::LightCyan)
        } else {
            Style::default()
        };
        let button_list = List::new(button_items)
            .block(Block::default().borders(Borders::ALL).title("操作"))
            .highlight_style(button_style.add_modifier(Modifier::BOLD))
            .highlight_symbol(">> ");

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(6),
                Constraint::Length(3),
                Constraint::Length(4),
            ])
            .split(inner);

        // Scroll range for modal info area (based on chunks[0])
        let full_lines = wrapped_line_count(&info_plain, chunks[0].width);
        let visible_h = chunks[0].height as usize;
        let scrollable = full_lines > visible_h && chunks[0].width > 1;
        let (info_text_area, info_scroll_area, total_lines) = if scrollable {
            let text_w = chunks[0].width.saturating_sub(1).max(1);
            let total = wrapped_line_count(&info_plain, text_w);
            (
                Rect {
                    x: chunks[0].x,
                    y: chunks[0].y,
                    width: text_w,
                    height: chunks[0].height,
                },
                Some(Rect {
                    x: chunks[0].x.saturating_add(text_w),
                    y: chunks[0].y,
                    width: 1,
                    height: chunks[0].height,
                }),
                total,
            )
        } else {
            (chunks[0], None, full_lines)
        };
        let max_scroll = total_lines
            .saturating_sub(info_text_area.height as usize)
            .min(u16::MAX as usize) as u16;
        app.preview_modal_scroll_max = max_scroll;
        app.preview_modal_scroll = app.preview_modal_scroll.min(app.preview_modal_scroll_max);

        frame.render_widget(Clear, modal);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title("预览与下载 (↑↓/滚轮)")
                .title_alignment(Alignment::Center),
            modal,
        );

        let info_para = Paragraph::new(info_lines)
            .wrap(Wrap { trim: true })
            .scroll((app.preview_modal_scroll, 0));
        frame.render_widget(info_para, info_text_area);
        if let Some(sb_area) = info_scroll_area {
            let mut state =
                ScrollbarState::new(total_lines).position(app.preview_modal_scroll as usize);
            let sb = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
            frame.render_stateful_widget(sb, sb_area, &mut state);
        }
        frame.render_widget(range_line, chunks[1]);
        frame.render_stateful_widget(button_list, chunks[2], &mut app.preview_buttons);
        app.last_preview_modal = Some(PreviewModalLayout {
            _modal: modal,
            info: chunks[0],
            range: chunks[1],
            buttons: chunks[2],
        });
    }
}

pub(super) fn apply_preview_ready(app: &mut App, pending: PendingDownload) {
    let title = pending
        .plan
        .meta
        .book_name
        .clone()
        .unwrap_or_else(|| pending.plan.book_id.clone());
    let total = pending.plan.chapters.len();
    let downloaded = pending.downloaded_count;

    // Reuse directory-derived metadata to enrich the selected search result.
    // This keeps home preview info complete without extra API calls.
    upsert_result_detail_from_plan(app, &pending.plan.book_id, &pending.plan.meta);

    app.pending_download = Some(pending);
    app.view = View::Preview;
    app.preview_focus = PreviewFocus::Range;
    app.preview_buttons.select(Some(0));
    app.preview_range.clear();
    app.preview_modal_open = true;
    app.input.clear();
    app.download_progress = Some(ProgressSnapshot {
        group_done: 0,
        group_total: total.div_ceil(25),
        saved_chapters: downloaded,
        chapter_total: total,
        save_phase: SavePhase::TextSave,
        comment_fetch: 0,
        comment_total: if app.config.enable_segment_comments {
            total
        } else {
            0
        },
        comment_saved: 0,
    });
    app.status = format!("预览: 《{}》 共 {} 章，已下载 {}", title, total, downloaded);
}

pub(super) fn apply_preview_error(app: &mut App, err: anyhow::Error) {
    app.status = format!("加载目录失败: {err}");
    app.push_message(format!("加载目录失败: {err}"));
    warn!(target: "ui", "加载目录失败: {err}");
}
