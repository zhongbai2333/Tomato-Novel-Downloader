use super::*;

pub(super) fn handle_event_config(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if let Some((cat_idx, entry_idx)) = app.cfg_editing {
                let editing_bool = app
                    .cfg_categories
                    .get(cat_idx)
                    .and_then(|c| c.entries.get(entry_idx))
                    .is_some_and(|e| super::cfg_field_is_bool(e.field));
                match key.code {
                    KeyCode::Esc => {
                        app.cfg_editing = None;
                        app.cfg_edit_buffer.clear();
                        app.status = "取消修改".to_string();
                    }
                    KeyCode::Enter => {
                        if editing_bool {
                            let sel = app.cfg_bool_state.selected().unwrap_or(0);
                            app.cfg_edit_buffer =
                                if sel == 0 { "true" } else { "false" }.to_string();
                        }
                        if let Err(err) = super::apply_cfg_edit(app, cat_idx, entry_idx) {
                            app.status = format!("保存失败: {err}");
                        } else {
                            app.cfg_editing = None;
                            app.cfg_edit_buffer.clear();
                        }
                    }
                    KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
                        if editing_bool =>
                    {
                        let cur = app.cfg_bool_state.selected().unwrap_or(0);
                        let next = if cur == 0 { 1 } else { 0 };
                        app.cfg_bool_state.select(Some(next));
                    }
                    KeyCode::Backspace if !editing_bool => {
                        app.cfg_edit_buffer.pop();
                    }
                    KeyCode::Char(c) if !editing_bool => {
                        app.cfg_edit_buffer.push(c);
                    }
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('c') => {
                        app.view = View::Home;
                        app.status = "返回主菜单".to_string();
                        app.cfg_focus = ConfigFocus::Entry;
                    }
                    KeyCode::Tab => {
                        app.cfg_focus = match app.cfg_focus {
                            ConfigFocus::Category => ConfigFocus::Entry,
                            ConfigFocus::Entry => ConfigFocus::Category,
                        };
                    }
                    KeyCode::Left => {
                        super::select_prev_category(app);
                        app.cfg_focus = ConfigFocus::Category;
                    }
                    KeyCode::Right => {
                        super::select_next_category(app);
                        app.cfg_focus = ConfigFocus::Category;
                    }
                    KeyCode::Up => match app.cfg_focus {
                        ConfigFocus::Category => super::select_prev_category(app),
                        ConfigFocus::Entry => super::select_prev_entry(app),
                    },
                    KeyCode::Down => match app.cfg_focus {
                        ConfigFocus::Category => super::select_next_category(app),
                        ConfigFocus::Entry => super::select_next_entry(app),
                    },
                    KeyCode::Enter if app.cfg_button_state.selected().is_some() => {
                        app.view = View::Home;
                        app.status = "返回主菜单".to_string();
                    }
                    KeyCode::Enter => match app.cfg_focus {
                        ConfigFocus::Category => {
                            app.cfg_focus = ConfigFocus::Entry;
                            super::ensure_entry_selection(app);
                        }
                        ConfigFocus::Entry => super::start_cfg_edit(app),
                    },
                    KeyCode::Char('b') => {
                        app.view = View::Home;
                        app.status = "返回主菜单".to_string();
                    }
                    _ => {}
                }
            }
        }
        Event::Mouse(me) => handle_mouse_config(app, me)?,
        Event::Resize(_, _) => {}
        _ => {}
    }

    Ok(())
}

pub(super) fn handle_mouse_config(app: &mut App, me: event::MouseEvent) -> Result<()> {
    let Some(layout) = app.last_config_layout else {
        return Ok(());
    };
    let header = layout[0];
    let cat_area = layout[1];
    let entry_area = layout[2];
    let button_area = app.last_config_button;
    let pos_in = |area: Rect, col: u16, row: u16| {
        col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
    };

    match me.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((cat_idx, entry_idx)) = app.cfg_editing {
                let editing_bool = app
                    .cfg_categories
                    .get(cat_idx)
                    .and_then(|c| c.entries.get(entry_idx))
                    .is_some_and(|e| super::cfg_field_is_bool(e.field));

                if editing_bool {
                    if let Some(bool_area) = app.last_config_bool_area {
                        if pos_in(bool_area, me.column, me.row) {
                            let idx = me.row.saturating_sub(bool_area.y + 1) as usize;
                            if idx < 2 {
                                app.cfg_bool_state.select(Some(idx));
                                app.cfg_edit_buffer =
                                    if idx == 0 { "true" } else { "false" }.to_string();
                                if let Err(err) = super::apply_cfg_edit(app, cat_idx, entry_idx) {
                                    app.status = format!("保存失败: {err}");
                                } else {
                                    app.cfg_editing = None;
                                    app.cfg_edit_buffer.clear();
                                }
                            }
                            return Ok(());
                        }
                    }
                }

                // 编辑中时，点击其他区域不触发选择/跳转
                return Ok(());
            }

            if pos_in(cat_area, me.column, me.row) {
                if !app.cfg_categories.is_empty() {
                    let idx = me.row.saturating_sub(cat_area.y + 1) as usize;
                    if idx < app.cfg_categories.len() {
                        app.cfg_cat_state.select(Some(idx));
                        super::ensure_entry_selection(app);
                        app.cfg_focus = ConfigFocus::Category;
                    }
                }
                return Ok(());
            }

            if let Some(btn_area) = button_area {
                if pos_in(btn_area, me.column, me.row) {
                    app.cfg_button_state.select(Some(0));
                    app.view = View::Home;
                    app.status = "返回主菜单".to_string();
                    return Ok(());
                }
            }

            if pos_in(entry_area, me.column, me.row) {
                if let Some(entries) = super::current_cfg_entries(app) {
                    let idx = me.row.saturating_sub(entry_area.y + 1) as usize;
                    if idx < entries.len() {
                        app.cfg_entry_state.select(Some(idx));
                        app.cfg_focus = ConfigFocus::Entry;
                        super::start_cfg_edit(app);
                    }
                }
                return Ok(());
            }

            if pos_in(header, me.column, me.row) {
                return Ok(());
            }
        }
        MouseEventKind::Moved => {
            if let Some((cat_idx, entry_idx)) = app.cfg_editing {
                let editing_bool = app
                    .cfg_categories
                    .get(cat_idx)
                    .and_then(|c| c.entries.get(entry_idx))
                    .is_some_and(|e| super::cfg_field_is_bool(e.field));

                if editing_bool {
                    if let Some(bool_area) = app.last_config_bool_area {
                        if pos_in(bool_area, me.column, me.row) {
                            let idx = me.row.saturating_sub(bool_area.y + 1) as usize;
                            if idx < 2 {
                                app.cfg_bool_state.select(Some(idx));
                            }
                            return Ok(());
                        }
                    }
                }

                return Ok(());
            }

            if pos_in(cat_area, me.column, me.row) {
                if !app.cfg_categories.is_empty() {
                    let idx = me.row.saturating_sub(cat_area.y + 1) as usize;
                    if idx < app.cfg_categories.len() {
                        app.cfg_cat_state.select(Some(idx));
                        super::ensure_entry_selection(app);
                        app.cfg_focus = ConfigFocus::Category;
                    }
                }
                return Ok(());
            }

            if let Some(btn_area) = button_area {
                if pos_in(btn_area, me.column, me.row) {
                    app.cfg_button_state.select(Some(0));
                    return Ok(());
                } else {
                    app.cfg_button_state.select(None);
                }
            }

            if pos_in(entry_area, me.column, me.row) {
                if let Some(entries) = super::current_cfg_entries(app) {
                    let idx = me.row.saturating_sub(entry_area.y + 1) as usize;
                    if idx < entries.len() {
                        app.cfg_entry_state.select(Some(idx));
                        app.cfg_focus = ConfigFocus::Entry;
                    }
                }
                return Ok(());
            }
        }
        _ => {}
    }

    Ok(())
}

pub(super) fn draw_config(frame: &mut ratatui::Frame, app: &mut App) {
    let (main, log_area) = super::split_with_log(frame.size());
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(8),
        ])
        .split(main);

    if layout.len() == 3 {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(24), Constraint::Min(40)])
            .split(layout[1]);
        let mut arr = [Rect::default(); 3];
        arr[0] = layout[0];
        arr[1] = body.get(0).copied().unwrap_or_default();
        arr[2] = body.get(1).copied().unwrap_or_default();
        app.last_config_layout = Some(arr);

        let footer = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(18), Constraint::Min(10)])
            .split(layout[2]);
        app.last_config_button = footer.get(0).copied();
    }

    let header_line = Line::from(vec![
        Span::styled(
            "配置编辑",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  左右/Tab 切分类, 上下选项, 回车编辑, q 返回"),
    ]);

    let header = Paragraph::new(header_line).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Tomato Novel Downloader"),
    );
    frame.render_widget(header, layout[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(40)])
        .split(layout[1]);

    let cat_items: Vec<ListItem> = app
        .cfg_categories
        .iter()
        .map(|c| ListItem::new(c.title))
        .collect();
    let cat_highlight = if app.cfg_focus == ConfigFocus::Category {
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::LightCyan)
    };
    let cat_list = List::new(cat_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("分类 (左右/Tab 切换)"),
        )
        .highlight_style(cat_highlight)
        .highlight_symbol(">> ");
    frame.render_stateful_widget(cat_list, body[0], &mut app.cfg_cat_state);

    let entries = super::current_cfg_entries(app);
    let entry_items: Vec<ListItem> = if let Some(entries) = entries {
        entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let val = super::current_cfg_value(app, entry.field);
                let mut spans = vec![Span::raw(format!("{}: {}", entry.title, val))];
                if let Some((cat_i, entry_i)) = app.cfg_editing {
                    if Some(cat_i) == app.cfg_cat_state.selected() && entry_i == idx {
                        spans.push(Span::raw("  [编辑中] "));
                        if !super::cfg_field_is_bool(entry.field) {
                            spans.push(Span::styled(
                                app.cfg_edit_buffer.clone(),
                                Style::default().fg(Color::Yellow),
                            ));
                        }
                    }
                }
                ListItem::new(Line::from(spans))
            })
            .collect()
    } else {
        vec![ListItem::new("无可编辑配置")]
    };

    let entry_highlight = if app.cfg_focus == ConfigFocus::Entry {
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::LightCyan)
    };

    let entry_list = List::new(entry_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("配置项 (上下选择, 回车编辑/保存)"),
        )
        .highlight_style(entry_highlight)
        .highlight_symbol(">> ");

    frame.render_stateful_widget(entry_list, body[1], &mut app.cfg_entry_state);

    let footer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Min(10)])
        .split(layout[2]);

    let btn_items: Vec<ListItem> = vec![ListItem::new("返回")];
    let btn_list = List::new(btn_items)
        .block(Block::default().borders(Borders::ALL).title("操作"))
        .highlight_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(btn_list, footer[0], &mut app.cfg_button_state);

    let mut msg_lines: Vec<Line> = vec![Line::from(app.status.clone())];

    let editing_bool = app
        .cfg_editing
        .and_then(|(cat_idx, entry_idx)| {
            app.cfg_categories
                .get(cat_idx)
                .and_then(|c| c.entries.get(entry_idx))
        })
        .is_some_and(|e| super::cfg_field_is_bool(e.field));

    if app.cfg_editing.is_some() && editing_bool {
        let status_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(3)])
            .split(footer[1]);

        let items = vec![ListItem::new("True"), ListItem::new("False")];
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("选择值(方向键/鼠标，Enter保存)"),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");
        app.last_config_bool_area = Some(status_layout[0]);
        frame.render_stateful_widget(list, status_layout[0], &mut app.cfg_bool_state);

        msg_lines.push(Line::from(
            "编辑中: 方向键选择 True/False，Enter 保存，Esc 取消。",
        ));
        let messages = Paragraph::new(msg_lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title("状态"));
        frame.render_widget(messages, status_layout[1]);
    } else {
        app.last_config_bool_area = None;

        if app.cfg_editing.is_some() {
            msg_lines.push(Line::from("编辑中: 回车保存，Esc 取消。"));
        } else {
            msg_lines.push(Line::from(
                "左右/Tab 切换分类，↑↓ 选择，Enter 编辑，鼠标点击或按钮返回。",
            ));
        }

        let messages = Paragraph::new(msg_lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title("状态"));

        frame.render_widget(messages, footer[1]);
    }

    super::render_log_box(frame, log_area, app);
}
