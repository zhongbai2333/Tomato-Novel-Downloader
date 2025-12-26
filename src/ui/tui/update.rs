//! TUI 更新检查与提示页面。

use super::*;
use std::thread;

pub(super) fn handle_event_update(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('b') => exit_update_view(app)?,
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
        let pos_in = |rect: Rect, col: u16, row: u16| {
            col >= rect.x
                && col < rect.x + rect.width
                && row >= rect.y
                && row < rect.y + rect.height
        };
        if pos_in(area, me.column, me.row)
            && matches!(me.kind, MouseEventKind::Down(MouseButton::Left))
        {
            return exit_update_view(app);
        }
    }

    if let Some(layout) = app.last_update_layout {
        let list_area = layout[1];
        let pos_in = |area: Rect, col: u16, row: u16| {
            col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
        };
        match me.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if pos_in(list_area, me.column, me.row) {
                    let idx = me.row.saturating_sub(list_area.y + 1) as usize;
                    let list = if app.show_no_update {
                        &app.update_no_updates
                    } else {
                        &app.update_entries
                    };
                    if idx < list.len() {
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
                    let idx = me.row.saturating_sub(list_area.y + 1) as usize;
                    let list = if app.show_no_update {
                        &app.update_no_updates
                    } else {
                        &app.update_entries
                    };
                    if idx < list.len() {
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
    let mut updates = Vec::new();
    let mut no_updates = Vec::new();

    let save_dir = config.default_save_dir();
    if !save_dir.exists() {
        return Ok((updates, no_updates));
    }
    let dir_reader =
        fs::read_dir(&save_dir).with_context(|| format!("read dir {}", save_dir.display()))?;
    let client = DirectoryClient::new().context("init DirectoryClient")?;
    for entry in dir_reader.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let (book_id, book_name) = match name.split_once('_') {
            Some((id, n)) if id.chars().all(|c| c.is_ascii_digit()) => {
                (id.to_string(), n.to_string())
            }
            _ => continue,
        };

        let downloaded_count = read_downloaded_count(&path, &book_id).unwrap_or(0);
        let chapter_list = match client.fetch_directory(&book_id) {
            Ok(d) => d.chapters,
            Err(_) => Vec::new(),
        };
        if chapter_list.is_empty() {
            continue;
        }
        let total = chapter_list.len();
        let new_count = total.saturating_sub(downloaded_count);
        let label = format!("《{}》({}) — 新章节: {}", book_name, book_id, new_count);
        let entry = UpdateEntry {
            book_id: book_id.clone(),
            book_name: book_name.clone(),
            folder: path.clone(),
            label,
            _new_count: new_count,
            _has_update: new_count > 0,
        };
        if new_count > 0 {
            updates.push(entry);
        } else {
            no_updates.push(entry);
        }
    }
    Ok((updates, no_updates))
}

pub(super) fn expected_book_folder(config: &Config, plan: &DownloadPlan) -> PathBuf {
    crate::base_system::book_paths::book_folder_path(
        config,
        &plan.book_id,
        plan.meta.book_name.as_deref(),
    )
}

pub(super) fn read_downloaded_count(folder: &Path, book_id: &str) -> Option<usize> {
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

    // 仅统计“成功下载”的章节：downloaded[chapter_id] = [title, content]
    // 其中 content=null 表示下载失败（或未完成），不应该计入“已下载”。
    let mut ok = 0usize;
    for (_cid, pair) in downloaded {
        match pair {
            Value::Array(arr) => {
                if arr.get(1).and_then(|v| v.as_str()).is_some() {
                    ok += 1;
                }
            }
            Value::Object(obj) => {
                if obj.get("content").and_then(|v| v.as_str()).is_some() {
                    ok += 1;
                }
            }
            _ => {}
        }
    }
    Some(ok)
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
        Span::raw("  |  上下选择，Enter 下载，n 切换无更新，b 或右下角返回"),
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
    let list_widget = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(if app.show_no_update {
                    "无更新书籍"
                } else {
                    "有更新书籍"
                }),
        )
        .highlight_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(list_widget, layout[1], &mut app.update_state);

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
