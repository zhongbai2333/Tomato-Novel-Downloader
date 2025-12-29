//! TUI 更新检查与提示页面。

use super::*;
use std::thread;

use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::base_system::novel_updates;

pub(super) fn handle_event_update(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('b') => exit_update_view(app)?,
            KeyCode::Char('i') => {
                // 切换当前选中书籍的忽略更新状态
                if let Some(entry) = current_update_entry(app) {
                    // 加载BookManager来切换忽略状态
                    let mut manager = match crate::book_parser::book_manager::BookManager::new(
                        app.config.clone(),
                        &entry.book_id,
                        &entry.book_name,
                    ) {
                        Ok(m) => m,
                        Err(e) => {
                            app.status = format!("加载书籍状态失败: {}", e);
                            return Ok(());
                        }
                    };
                    
                    // 加载现有状态
                    manager.load_existing_status(&entry.book_id, &entry.book_name);
                    
                    // 切换忽略状态并保存
                    let new_state = manager.toggle_ignore_updates();
                    
                    if new_state {
                        app.status = format!("已将《{}》添加到忽略列表", entry.book_name);
                    } else {
                        app.status = format!("已将《{}》从忽略列表移除", entry.book_name);
                    }
                    
                    // 重新扫描更新
                    show_update_menu(app)?;
                }
            }
            KeyCode::Char('p') => {
                if let Some(entry) = current_update_entry(app) {
                    super::cover::show_cover(
                        app,
                        &entry.book_id,
                        &entry.book_name,
                        Some(entry.folder.clone()),
                    )?;
                }
            }
            KeyCode::Char('n') => {
                app.show_no_update = !app.show_no_update;
                if (app.show_no_update && !app.update_no_updates.is_empty())
                    || (!app.show_no_update && !app.update_entries.is_empty())
                {
                    app.update_state.select(Some(0));
                } else {
                    app.update_state.select(None);
                }
            }
            KeyCode::Up => {
                select_prev_update(app);
            }
            KeyCode::Down => {
                select_next_update(app);
            }
            KeyCode::Enter => {
                if let Some(entry) = current_update_entry(app) {
                    app.status = format!("更新: {}", entry.label);
                    let hint = BookMeta {
                        book_name: Some(entry.book_name.clone()),
                        ..BookMeta::default()
                    };
                    super::start_preview_task(app, entry.book_id.clone(), hint)?;
                }
            }
            _ => {}
        },
        Event::Mouse(me) => handle_mouse_update(app, me)?,
        Event::Resize(_, _) => {}
        _ => {}
    }
    Ok(())
}

fn exit_update_view(app: &mut App) -> Result<()> {
    app.view = View::Home;
    app.status = "返回主菜单".to_string();
    Ok(())
}

pub(super) fn handle_mouse_update(app: &mut App, me: event::MouseEvent) -> Result<()> {
    if let Some(area) = app.last_update_exit_button {
        let pos_in = |rect: Rect, col: u16, row: u16| super::pos_in(rect, col, row);
        if pos_in(area, me.column, me.row)
            && matches!(me.kind, MouseEventKind::Down(MouseButton::Left))
        {
            return exit_update_view(app);
        }
    }

    if let Some(layout) = app.last_update_layout {
        let list_area = layout[1];
        let pos_in = |area: Rect, col: u16, row: u16| super::pos_in(area, col, row);
        match me.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if pos_in(list_area, me.column, me.row) {
                    let list = if app.show_no_update {
                        &app.update_no_updates
                    } else {
                        &app.update_entries
                    };
                    if list.is_empty() {
                        return Ok(());
                    }
                    if app.update_state.selected().is_none() {
                        app.update_state.select(Some(0));
                        return Ok(());
                    }
                    if matches!(me.kind, MouseEventKind::ScrollUp) {
                        select_prev_update(app);
                    } else {
                        select_next_update(app);
                    }
                    return Ok(());
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if pos_in(list_area, me.column, me.row) {
                    let list = if app.show_no_update {
                        &app.update_no_updates
                    } else {
                        &app.update_entries
                    };
                    if let Some(idx) =
                        super::list_index_from_mouse_row(
                            list_area,
                            me.row,
                            &app.update_state,
                            list.len(),
                        )
                    {
                        app.update_state.select(Some(idx));
                        if let Some(entry) = current_update_entry(app) {
                            let hint = BookMeta {
                                book_name: Some(entry.book_name.clone()),
                                ..BookMeta::default()
                            };
                            super::start_preview_task(app, entry.book_id.clone(), hint)?;
                        }
                    }
                }
            }
            MouseEventKind::Moved => {
                if pos_in(list_area, me.column, me.row) {
                    let list = if app.show_no_update {
                        &app.update_no_updates
                    } else {
                        &app.update_entries
                    };
                    if let Some(idx) =
                        super::list_index_from_mouse_row(
                            list_area,
                            me.row,
                            &app.update_state,
                            list.len(),
                        )
                    {
                        app.update_state.select(Some(idx));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn select_next_update(app: &mut App) {
    let list = if app.show_no_update {
        &app.update_no_updates
    } else {
        &app.update_entries
    };
    if list.is_empty() {
        app.update_state.select(None);
        return;
    }
    let next = app
        .update_state
        .selected()
        .map(|i| (i + 1) % list.len())
        .unwrap_or(0);
    app.update_state.select(Some(next));
}

fn select_prev_update(app: &mut App) {
    let list = if app.show_no_update {
        &app.update_no_updates
    } else {
        &app.update_entries
    };
    if list.is_empty() {
        app.update_state.select(None);
        return;
    }
    let prev = app
        .update_state
        .selected()
        .map(|i| if i == 0 { list.len() - 1 } else { i - 1 })
        .unwrap_or(0);
    app.update_state.select(Some(prev));
}

fn current_update_entry(app: &App) -> Option<UpdateEntry> {
    let list = if app.show_no_update {
        &app.update_no_updates
    } else {
        &app.update_entries
    };
    let idx = app.update_state.selected()?;
    list.get(idx).cloned()
}

pub(super) fn show_update_menu(app: &mut App) -> Result<()> {
    app.status = "扫描本地小说…".to_string();
    app.update_entries.clear();
    app.update_no_updates.clear();
    app.update_state.select(None);
    app.show_no_update = false;
    app.view = View::Update;
    super::start_spinner(app, "扫描本地小说…");

    let cfg = app.config.clone();
    let tx = app.worker_tx.clone();
    info!(target: "ui", "启动更新扫描");
    thread::spawn(move || {
        let result = scan_updates(&cfg);
        let _ = tx.send(WorkerMsg::UpdateScanned(result));
    });
    Ok(())
}

fn scan_updates(config: &Config) -> Result<(Vec<UpdateEntry>, Vec<UpdateEntry>)> {
    let save_dir = config.default_save_dir();
    let scan = novel_updates::scan_novel_updates(&save_dir)?;

    let to_entry = |it: novel_updates::NovelUpdateRow| {
        let ignore_marker = if it.is_ignored { "[已忽略] " } else { "" };
        let label = if it.new_count > 0 && it.local_failed > 0 {
            format!(
                "{}《{}》({}) — 新章节: {} | 失败章节: {}",
                ignore_marker, it.book_name, it.book_id, it.new_count, it.local_failed
            )
        } else if it.new_count > 0 {
            format!("{}《{}》({}) — 新章节: {}", ignore_marker, it.book_name, it.book_id, it.new_count)
        } else if it.local_failed > 0 {
            format!("{}《{}》({}) — 失败章节: {}", ignore_marker, it.book_name, it.book_id, it.local_failed)
        } else {
            format!("{}《{}》({}) — 新章节: 0", ignore_marker, it.book_name, it.book_id)
        };

        UpdateEntry {
            book_id: it.book_id.clone(),
            book_name: it.book_name.clone(),
            folder: it.folder.clone(),
            label,
            _new_count: it.new_count,
            _has_update: it.has_update,
        }
    };

    Ok((
        scan.updates.into_iter().map(to_entry).collect(),
        scan.no_updates.into_iter().map(to_entry).collect(),
    ))
}

pub(super) fn draw_update(frame: &mut ratatui::Frame, app: &mut App) {
    let (main, log_area) = super::split_with_log(frame.size());
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(4),
        ])
        .split(main);
    if layout.len() == 3 {
        let mut arr = [Rect::default(); 3];
        arr.copy_from_slice(&layout);
        app.last_update_layout = Some(arr);
    }

    let header_line = Line::from(vec![
        Span::styled(
            "更新",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  上下选择，Enter 下载，i 忽略/取消忽略，n 切换无更新，b 或右下角返回"),
    ]);
    let header =
        Paragraph::new(header_line).block(Block::default().borders(Borders::ALL).title("更新检测"));
    frame.render_widget(header, layout[0]);

    let list = if app.show_no_update {
        &app.update_no_updates
    } else {
        &app.update_entries
    };
    let items: Vec<ListItem> = if list.is_empty() {
        vec![ListItem::new("没有可展示的项目")]
    } else {
        list.iter()
            .map(|u| ListItem::new(u.label.clone()))
            .collect()
    };
    let list_title = if app.show_no_update {
        "无更新书籍"
    } else {
        "有更新书籍"
    };
    let list_block = Block::default().borders(Borders::ALL).title(list_title);
    frame.render_widget(list_block.clone(), layout[1]);
    let inner = list_block.inner(layout[1]);

    let need_scrollbar = list.len() > 0 && inner.height > 0 && list.len() > inner.height as usize;
    let (list_area, sb_area) = if need_scrollbar && inner.width > 0 {
        let list_w = inner.width.saturating_sub(1).max(1);
        (
            Rect {
                x: inner.x,
                y: inner.y,
                width: list_w,
                height: inner.height,
            },
            Some(Rect {
                x: inner.x.saturating_add(list_w),
                y: inner.y,
                width: 1,
                height: inner.height,
            }),
        )
    } else {
        (inner, None)
    };

    let list_widget = List::new(items)
        .highlight_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(list_widget, list_area, &mut app.update_state);

    if let Some(sb_area) = sb_area {
        let pos = app
            .update_state
            .selected()
            .unwrap_or(0)
            .min(list.len().saturating_sub(1));
        let mut sb_state = ScrollbarState::new(list.len()).position(pos);
        let sb = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
        frame.render_stateful_widget(sb, sb_area, &mut sb_state);
    }

    let mut msg_lines = vec![Line::from(app.status.clone())];
    if !app.update_entries.is_empty() {
        msg_lines.push(Line::from(format!(
            "有更新: {} 本",
            app.update_entries.len()
        )));
    }
    if !app.update_no_updates.is_empty() {
        msg_lines.push(Line::from(format!(
            "无更新: {} 本 (按 n 查看)",
            app.update_no_updates.len()
        )));
    }

    let footer_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(16)])
        .split(layout[2]);

    let footer = Paragraph::new(msg_lines)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("提示"));
    frame.render_widget(footer, footer_layout[0]);

    let exit_btn = Paragraph::new(Line::from("返回主菜单"))
        .alignment(ratatui::layout::Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("返回")
                .style(Style::default().fg(Color::Yellow)),
        );
    frame.render_widget(exit_btn, footer_layout[1]);
    app.last_update_exit_button = Some(footer_layout[1]);

    super::render_log_box(frame, log_area, app);
}
