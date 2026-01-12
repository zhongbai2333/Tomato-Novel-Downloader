//! TUI 首页。

use super::*;

use std::io::Write;
use std::path::Path;

use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::base_system::config::{ConfigSpec, write_with_comments};

pub(super) fn handle_event_home(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Paste(s) => {
            if app.focus == Focus::Input {
                app.input.push_str(&s);
            }
        }
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') => {
                if app.focus == Focus::Input {
                    app.input.push('q');
                } else {
                    app.should_quit = true;
                }
            }
            KeyCode::Char('c') => {
                if app.focus == Focus::Input {
                    app.input.push('c');
                } else {
                    super::switch_view(app, MenuAction::Config)?;
                }
            }
            KeyCode::Char('u') => {
                if app.focus == Focus::Input {
                    app.input.push('u');
                } else {
                    super::switch_view(app, MenuAction::Update)?;
                }
            }
            KeyCode::Char('a') => {
                if app.focus == Focus::Input {
                    app.input.push('a');
                } else {
                    super::switch_view(app, MenuAction::About)?;
                }
            }
            KeyCode::Esc => {
                app.focus = Focus::Input;
                app.results.clear();
                app.list_state.select(None);
                if app.pending_download.is_some() {
                    app.pending_download = None;
                    app.status = "已取消待下载的预览".to_string();
                }
            }
            KeyCode::Tab => cycle_focus(app),
            KeyCode::Backspace => {
                if app.focus == Focus::Input {
                    app.input.pop();
                }
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                #[cfg(feature = "clipboard")]
                {
                    match super::clipboard::get_text() {
                        Ok(Some(text)) => app.input.push_str(&text),
                        Ok(None) => {
                            #[cfg(target_os = "android")]
                            {
                                app.status = "Android 剪贴板未就绪：需要 Termux + termux-api（termux-clipboard-get）".to_string();
                            }
                            #[cfg(not(target_os = "android"))]
                            {
                                app.status = "当前构建未包含剪贴板后端（启用 clipboard-arboard）"
                                    .to_string();
                            }
                        }
                        Err(e) => {
                            app.status = format!("读取剪贴板失败：{e}");
                        }
                    }
                }

                #[cfg(not(feature = "clipboard"))]
                {
                    app.status = "当前构建未启用剪贴板支持".to_string();
                }
            }
            KeyCode::Char('p') => {
                if app.focus == Focus::Input {
                    app.input.push('p');
                } else if app.focus == Focus::Results
                    && let Some(idx) = app.list_state.selected()
                    && let Some(item) = app.results.get(idx).cloned()
                {
                    super::cover::show_cover(app, &item.book_id, &item.title, None)?;
                }
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if app.focus == Focus::Input {
                    app.input.push(c);
                }
            }
            KeyCode::Up => match app.focus {
                Focus::Results => app.select_prev(),
                Focus::Menu => select_prev_menu(app),
                Focus::Input => {}
            },
            KeyCode::Down => match app.focus {
                Focus::Results => app.select_next(),
                Focus::Menu => select_next_menu(app),
                Focus::Input => {}
            },
            KeyCode::Enter => match app.focus {
                Focus::Input => process_input(app)?,
                Focus::Results => {
                    if app.list_state.selected().is_some() {
                        download_selected(app)?;
                    }
                }
                Focus::Menu => super::trigger_menu_action(app)?,
            },
            _ => {}
        },
        Event::Mouse(me) => handle_mouse_home(app, me)?,
        Event::Resize(_, _) => {}
        _ => {}
    }

    Ok(())
}

pub(super) fn handle_mouse_home(app: &mut App, me: event::MouseEvent) -> Result<()> {
    if let Some(layout) = app.last_home_layout {
        let header = layout[0];
        let input_area = layout[1];
        let menu_area = layout[2];
        let results_area = layout[3];
        let status_area = layout[4];
        let pos_in = |area: Rect, col: u16, row: u16| super::pos_in(area, col, row);
        match me.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if pos_in(menu_area, me.column, me.row) {
                    let up = matches!(me.kind, MouseEventKind::ScrollUp);
                    if app.menu_state.selected().is_none() {
                        app.menu_state.select(Some(0));
                    } else if up {
                        let sel = app.menu_state.selected().unwrap_or(0);
                        let prev = sel.saturating_sub(1);
                        app.menu_state.select(Some(prev));
                    } else {
                        let sel = app.menu_state.selected().unwrap_or(0);
                        let next = (sel + 1).min(MENU_ITEMS.len().saturating_sub(1));
                        app.menu_state.select(Some(next));
                    }
                    app.focus = Focus::Menu;
                    return Ok(());
                }
                if pos_in(results_area, me.column, me.row) && !app.results.is_empty() {
                    let up = matches!(me.kind, MouseEventKind::ScrollUp);
                    if app.list_state.selected().is_none() {
                        app.list_state.select(Some(0));
                    } else if up {
                        app.select_prev();
                    } else {
                        app.select_next();
                    }
                    app.focus = Focus::Results;
                    return Ok(());
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if pos_in(input_area, me.column, me.row) {
                    app.focus = Focus::Input;
                    return Ok(());
                }
                if pos_in(menu_area, me.column, me.row) {
                    app.focus = Focus::Menu;
                    if let Some(idx) = super::list_index_from_mouse_row(
                        menu_area,
                        me.row,
                        &app.menu_state,
                        MENU_ITEMS.len(),
                    ) {
                        app.menu_state.select(Some(idx));
                        super::trigger_menu_action(app)?;
                    }
                    return Ok(());
                }
                if pos_in(results_area, me.column, me.row) {
                    if !app.results.is_empty()
                        && let Some(idx) = super::list_index_from_mouse_row(
                            results_area,
                            me.row,
                            &app.list_state,
                            app.results.len(),
                        )
                    {
                        app.list_state.select(Some(idx));
                        app.focus = Focus::Results;
                        let hint = book_meta_from_item(&app.results[idx]);
                        super::start_preview_task(app, app.results[idx].book_id.clone(), hint)?;
                    }
                    return Ok(());
                }
                if pos_in(status_area, me.column, me.row) || pos_in(header, me.column, me.row) {
                    return Ok(());
                }
            }
            MouseEventKind::Moved => {
                if pos_in(menu_area, me.column, me.row) {
                    if let Some(idx) = super::list_index_from_mouse_row(
                        menu_area,
                        me.row,
                        &app.menu_state,
                        MENU_ITEMS.len(),
                    ) {
                        app.menu_state.select(Some(idx));
                        app.focus = Focus::Menu;
                    }
                    return Ok(());
                }
                if pos_in(results_area, me.column, me.row) {
                    if !app.results.is_empty()
                        && let Some(idx) = super::list_index_from_mouse_row(
                            results_area,
                            me.row,
                            &app.list_state,
                            app.results.len(),
                        )
                    {
                        app.list_state.select(Some(idx));
                        app.focus = Focus::Results;
                    }
                    return Ok(());
                }
                if pos_in(input_area, me.column, me.row) {
                    app.focus = Focus::Input;
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn cycle_focus(app: &mut App) {
    app.focus = match app.focus {
        Focus::Input => Focus::Menu,
        Focus::Menu => {
            if app.results.is_empty() {
                Focus::Input
            } else {
                Focus::Results
            }
        }
        Focus::Results => Focus::Input,
    };
}

fn select_next_menu(app: &mut App) {
    let len = MENU_ITEMS.len();
    if len == 0 {
        return;
    }
    let next = app
        .menu_state
        .selected()
        .map(|i| (i + 1) % len)
        .unwrap_or(0);
    app.menu_state.select(Some(next));
}

fn select_prev_menu(app: &mut App) {
    let len = MENU_ITEMS.len();
    if len == 0 {
        return;
    }
    let prev = app
        .menu_state
        .selected()
        .map(|i| if i == 0 { len - 1 } else { i - 1 })
        .unwrap_or(len - 1);
    app.menu_state.select(Some(prev));
}

pub(super) fn process_input(app: &mut App) -> Result<()> {
    let text = app.input.trim();

    if text.eq_ignore_ascii_case("ooo") {
        app.config.old_cli = true;
        let path = Path::new(<Config as ConfigSpec>::FILE_NAME);
        if let Err(err) = write_with_comments(&app.config, path) {
            app.status = format!("切换失败: {err}");
            return Ok(());
        }

        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x07");
        let _ = out.flush();

        app.status = "已切换到旧版CLI(读屏友好)，请手动重启程序。".to_string();
        app.input.clear();
        app.should_quit = true;
        return Ok(());
    }

    if let Some(pending) = app.pending_download.clone() {
        match parse_range_input(text, pending.plan.chapters.len()) {
            Ok(range) => {
                super::start_download_task(app, pending, range)?;
                app.input.clear();
            }
            Err(err) => {
                app.status = format!("范围无效: {}", err);
            }
        }
        return Ok(());
    }

    if text.is_empty() {
        app.status = String::from("请输入书名、链接或 book_id，按 Enter 开始。");
        return Ok(());
    }

    if let Some(book_id) = parse_book_id(text) {
        app.focus = Focus::Input;
        app.status = format!("准备下载书籍 {book_id} …");
        super::start_preview_task(app, book_id, BookMeta::default())?;
        app.input.clear();
        app.results.clear();
        app.list_state.select(None);
    } else {
        super::start_search_task(app, text.to_string())?;
    }

    Ok(())
}

fn download_selected(app: &mut App) -> Result<()> {
    let Some(idx) = app.list_state.selected() else {
        return Ok(());
    };
    if idx >= app.results.len() {
        return Ok(());
    }
    let book = app.results[idx].clone();
    app.focus = Focus::Input;
    let hint = book_meta_from_item(&book);
    super::start_preview_task(app, book.book_id.clone(), hint)
}

fn book_meta_from_item(item: &SearchItem) -> BookMeta {
    let mut meta = BookMeta::default();
    if !item.title.is_empty() {
        meta.book_name = Some(item.title.clone());
    }
    if !item.author.is_empty() {
        meta.author = Some(item.author.clone());
    }
    if let Some(detail) = item.detail.as_ref() {
        if let Some(desc) = detail.description.clone() {
            meta.description = Some(desc);
        }
        if !detail.tags.is_empty() {
            meta.tags = detail.tags.clone();
        }
        meta.chapter_count = detail.chapter_count;
        meta.finished = detail.finished;
        meta.cover_url = detail.cover_url.clone();
        meta.detail_cover_url = detail.detail_cover_url.clone();
        meta.word_count = detail.word_count;
        meta.score = detail.score;
        meta.read_count = detail.read_count.clone();
        meta.read_count_text = detail.read_count_text.clone();
        meta.book_short_name = detail.book_short_name.clone();
        meta.original_book_name = detail.original_book_name.clone();
        meta.first_chapter_title = detail.first_chapter_title.clone();
        meta.last_chapter_title = detail.last_chapter_title.clone();
        meta.category = detail.category.clone();
        meta.cover_primary_color = detail.cover_primary_color.clone();
    }
    meta
}

fn current_selection_detail_lines(app: &App) -> Option<Vec<Line<'static>>> {
    let idx = app.list_state.selected()?;
    let item = app.results.get(idx)?;
    let mut lines = Vec::new();
    lines.push(Line::from(format!(
        "选中: 《{}》 | 作者: {} | ID: {}",
        item.title, item.author, item.book_id
    )));

    if let Some(detail) = item.detail.as_ref() {
        let mut status_parts: Vec<String> = Vec::new();
        if let Some(words) = detail.word_count {
            status_parts.push(format!("字数: {}", super::format_word_count(words)));
        }
        if !status_parts.is_empty() {
            lines.push(Line::from(status_parts.join(" | ")));
        }

        let mut meta_parts: Vec<String> = Vec::new();
        if let Some(score) = detail.score {
            meta_parts.push(format!("评分: {:.1}", score));
        }
        if let Some(reads) = detail
            .read_count_text
            .as_ref()
            .or(detail.read_count.as_ref())
        {
            meta_parts.push(format!("阅读: {}", reads));
        }
        if let Some(cat) = detail.category.as_ref() {
            meta_parts.push(format!("分类: {}", cat));
        }
        if !meta_parts.is_empty() {
            lines.push(Line::from(meta_parts.join(" | ")));
        }

        if detail.book_short_name.is_some() || detail.original_book_name.is_some() {
            let mut alias = Vec::new();
            if let Some(short) = detail.book_short_name.as_ref() {
                alias.push(format!("别名: {}", short));
            }
            if let Some(orig) = detail.original_book_name.as_ref() {
                alias.push(format!("原名: {}", orig));
            }
            lines.push(Line::from(alias.join(" | ")));
        }

        if detail.first_chapter_title.is_some() || detail.last_chapter_title.is_some() {
            let mut bounds = Vec::new();
            if let Some(first) = detail.first_chapter_title.as_ref() {
                bounds.push(format!("首章: {}", truncate(first, 48)));
            }
            if let Some(last) = detail.last_chapter_title.as_ref() {
                bounds.push(format!("末章: {}", truncate(last, 48)));
            }
            if !bounds.is_empty() {
                lines.push(Line::from(bounds.join(" | ")));
            }
        }

        {
            let mut row = Vec::new();
            if let Some(cnt) = detail.chapter_count {
                row.push(format!("章节: {}", cnt));
            }
            if let Some(done) = detail.finished {
                row.push(format!("状态: {}", if done { "完结" } else { "连载" }));
            }
            if !row.is_empty() {
                lines.push(Line::from(row.join(" | ")));
            }
        }

        if !detail.tags.is_empty() {
            lines.push(Line::from(format!("标签: {}", detail.tags.join(" | "))));
        }
        if let Some(desc) = detail.description.as_ref() {
            lines.push(Line::from(format!("简介: {}", truncate(desc, 220))));
        } else {
            lines.push(Line::from("简介: 暂无"));
        }
    } else {
        lines.push(Line::from("简介: 未加载"));
    }

    Some(lines)
}

pub(super) fn draw_home(frame: &mut ratatui::Frame, app: &mut App) {
    let (main, log_area) = super::split_with_log(frame.size());
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Min(6),
        ])
        .split(main);
    if layout.len() == 5 {
        let mut arr = [Rect::default(); 5];
        arr.copy_from_slice(&layout);
        app.last_home_layout = Some(arr);
    }

    let header_line = Line::from(vec![
        Span::styled(
            "番茄小说下载器 TUI",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  输出目录: "),
        Span::styled(
            app.config.default_save_dir().display().to_string(),
            Style::default().fg(Color::Green),
        ),
        Span::raw("  |  c: 配置, q: 退出"),
    ]);

    let header = Paragraph::new(header_line).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Tomato Novel Downloader"),
    );
    frame.render_widget(header, layout[0]);

    let input_style = if app.focus == Focus::Input {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let input = Paragraph::new(format!("> {}", app.input))
        .style(input_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("输入书名/ID/链接 (Enter 确认, Tab 切换)"),
        );
    frame.render_widget(input, layout[1]);

    let menu_items: Vec<ListItem> = MENU_ITEMS
        .iter()
        .map(|(label, _)| ListItem::new(*label))
        .collect();
    let menu_style = if app.focus == Focus::Menu {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let menu_block = Block::default()
        .borders(Borders::ALL)
        .title("操作 (Enter 或鼠标点击)");
    frame.render_widget(menu_block.clone(), layout[2]);
    let menu_inner = menu_block.inner(layout[2]);
    let menu_len = MENU_ITEMS.len();
    let need_scrollbar =
        menu_len > 0 && menu_inner.height > 0 && menu_len > menu_inner.height as usize;
    let (menu_area, menu_sb_area) = if need_scrollbar && menu_inner.width > 0 {
        let list_w = menu_inner.width.saturating_sub(1).max(1);
        (
            Rect {
                x: menu_inner.x,
                y: menu_inner.y,
                width: list_w,
                height: menu_inner.height,
            },
            Some(Rect {
                x: menu_inner.x.saturating_add(list_w),
                y: menu_inner.y,
                width: 1,
                height: menu_inner.height,
            }),
        )
    } else {
        (menu_inner, None)
    };
    let menu_list = List::new(menu_items)
        .highlight_style(menu_style.add_modifier(Modifier::BOLD))
        .highlight_symbol(">> ");
    frame.render_stateful_widget(menu_list, menu_area, &mut app.menu_state);
    if let Some(sb_area) = menu_sb_area {
        let pos = app
            .menu_state
            .selected()
            .unwrap_or(0)
            .min(menu_len.saturating_sub(1));
        let mut sb_state = ScrollbarState::new(menu_len).position(pos);
        let sb = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
        frame.render_stateful_widget(sb, sb_area, &mut sb_state);
    }

    let items: Vec<ListItem> = if app.results.is_empty() {
        vec![ListItem::new("无搜索结果")]
    } else {
        app.results
            .iter()
            .map(|b| {
                let label = format!("{} | {} | {}", b.title, b.book_id, b.author);
                ListItem::new(label)
            })
            .collect()
    };

    let results_block = Block::default()
        .borders(Borders::ALL)
        .title("搜索结果 (上下选择, Enter 下载)");
    frame.render_widget(results_block.clone(), layout[3]);
    let results_inner = results_block.inner(layout[3]);

    let results_len = app.results.len();
    let need_scrollbar =
        results_len > 0 && results_inner.height > 0 && results_len > results_inner.height as usize;
    let (list_area, sb_area) = if need_scrollbar && results_inner.width > 0 {
        let list_w = results_inner.width.saturating_sub(1).max(1);
        (
            Rect {
                x: results_inner.x,
                y: results_inner.y,
                width: list_w,
                height: results_inner.height,
            },
            Some(Rect {
                x: results_inner.x.saturating_add(list_w),
                y: results_inner.y,
                width: 1,
                height: results_inner.height,
            }),
        )
    } else {
        (results_inner, None)
    };

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(list, list_area, &mut app.list_state);

    if let Some(sb_area) = sb_area {
        let pos = app
            .list_state
            .selected()
            .unwrap_or(0)
            .min(results_len.saturating_sub(1));
        let mut sb_state = ScrollbarState::new(results_len).position(pos);
        let sb = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
        frame.render_stateful_widget(sb, sb_area, &mut sb_state);
    }

    let mut msg_lines: Vec<Line> = Vec::new();
    if let Some(detail) = current_selection_detail_lines(app) {
        msg_lines.extend(detail);
        msg_lines.push(Line::from(""));
    }
    msg_lines.push(Line::from(app.status.clone()));
    if !app.messages.is_empty() {
        msg_lines.push(Line::from(""));
        msg_lines.extend(
            app.messages
                .iter()
                .rev()
                .take(6)
                .rev()
                .map(|m| Line::from(m.as_str())),
        );
    }

    let messages = Paragraph::new(msg_lines)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("状态 / 消息"));

    frame.render_widget(messages, layout[4]);
    super::render_log_box(frame, log_area, app);
}
