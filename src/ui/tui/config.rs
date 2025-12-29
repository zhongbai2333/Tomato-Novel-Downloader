//! TUI 配置页 UI 组件。

use super::*;

use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

pub(super) fn handle_event_config(app: &mut App, event: Event) -> Result<()> {
    if app.segment_comments_confirm_open {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => {
                    app.segment_comments_confirm_open = false;
                    app.segment_comments_confirm_ctx = None;
                    app.last_segment_comments_confirm_options = None;
                    app.cfg_editing = None;
                    app.cfg_edit_buffer.clear();
                    app.status = "已取消开启段评".to_string();
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                    let cur = app.segment_comments_confirm_state.selected().unwrap_or(1);
                    let next = if cur == 0 { 1 } else { 0 };
                    app.segment_comments_confirm_state.select(Some(next));
                }
                KeyCode::Enter => {
                    let sel = app.segment_comments_confirm_state.selected().unwrap_or(1);
                    if sel == 0 {
                        if let Some((cat_idx, entry_idx)) = app.segment_comments_confirm_ctx.take()
                            && let Err(err) = super::apply_cfg_edit(app, cat_idx, entry_idx)
                        {
                            app.status = format!("保存失败: {err}");
                        }
                        app.cfg_editing = None;
                        app.cfg_edit_buffer.clear();
                        app.status = "已开启段评（注意：可能触发 IP 风控，且下载更慢）".to_string();
                    } else {
                        app.status = "已取消开启段评".to_string();
                        app.segment_comments_confirm_ctx = None;
                        app.cfg_editing = None;
                        app.cfg_edit_buffer.clear();
                    }
                    app.segment_comments_confirm_open = false;
                    app.last_segment_comments_confirm_options = None;
                }
                _ => {}
            },
            Event::Mouse(me) if matches!(me.kind, MouseEventKind::Down(MouseButton::Left)) => {
                if let Some(opt) = app.last_segment_comments_confirm_options {
                    let pos_in = |area: Rect, col: u16, row: u16| {
                        col >= area.x
                            && col < area.x + area.width
                            && row >= area.y
                            && row < area.y + area.height
                    };
                    if pos_in(opt, me.column, me.row) {
                        // account for list border: first item row at opt.y + 1
                        let idx = me.row.saturating_sub(opt.y + 1) as usize;
                        if idx < 2 {
                            app.segment_comments_confirm_state.select(Some(idx));
                            // Click acts like Enter.
                            if idx == 0 {
                                if let Some((cat_idx, entry_idx)) =
                                    app.segment_comments_confirm_ctx.take()
                                    && let Err(err) = super::apply_cfg_edit(app, cat_idx, entry_idx)
                                {
                                    app.status = format!("保存失败: {err}");
                                }
                                app.cfg_editing = None;
                                app.cfg_edit_buffer.clear();
                                app.status =
                                    "已开启段评（注意：可能触发 IP 风控，且下载更慢）".to_string();
                            } else {
                                app.status = "已取消开启段评".to_string();
                                app.segment_comments_confirm_ctx = None;
                                app.cfg_editing = None;
                                app.cfg_edit_buffer.clear();
                            }
                            app.segment_comments_confirm_open = false;
                            app.last_segment_comments_confirm_options = None;
                        }
                    }
                }
            }
            Event::Mouse(me) if matches!(me.kind, MouseEventKind::Moved) => {
                if let Some(opt) = app.last_segment_comments_confirm_options {
                    let pos_in = |area: Rect, col: u16, row: u16| {
                        col >= area.x
                            && col < area.x + area.width
                            && row >= area.y
                            && row < area.y + area.height
                    };
                    if pos_in(opt, me.column, me.row) {
                        let idx = me.row.saturating_sub(opt.y + 1) as usize;
                        if idx < 2 {
                            app.segment_comments_confirm_state.select(Some(idx));
                        }
                    }
                }
            }
            _ => {}
        }
        return Ok(());
    }

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

                            // Two-step confirm when enabling segment comments.
                            let is_enable_segment = app
                                .cfg_categories
                                .get(cat_idx)
                                .and_then(|c| c.entries.get(entry_idx))
                                .is_some_and(|e| {
                                    matches!(
                                        e.field,
                                        super::config_model::ConfigField::EnableSegmentComments
                                    )
                                });
                            let want_enable = sel == 0;
                            if is_enable_segment
                                && want_enable
                                && !app.config.enable_segment_comments
                            {
                                app.segment_comments_confirm_open = true;
                                app.segment_comments_confirm_ctx = Some((cat_idx, entry_idx));
                                app.segment_comments_confirm_state.select(Some(1));
                                app.status = "确认开启段评？".to_string();
                                return Ok(());
                            }
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
    let pos_in = |area: Rect, col: u16, row: u16| super::pos_in(area, col, row);

    match me.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = matches!(me.kind, MouseEventKind::ScrollUp);

            // When editing a bool, wheel toggles the True/False selector.
            if let Some((cat_idx, entry_idx)) = app.cfg_editing {
                let editing_bool = app
                    .cfg_categories
                    .get(cat_idx)
                    .and_then(|c| c.entries.get(entry_idx))
                    .is_some_and(|e| super::cfg_field_is_bool(e.field));
                if editing_bool
                    && let Some(bool_area) = app.last_config_bool_area
                    && pos_in(bool_area, me.column, me.row)
                {
                    let cur = app.cfg_bool_state.selected().unwrap_or(0);
                    let next = if cur == 0 { 1 } else { 0 };
                    app.cfg_bool_state.select(Some(next));
                    app.cfg_edit_buffer = if next == 0 { "true" } else { "false" }.to_string();
                    return Ok(());
                }
            }

            // Scroll categories.
            if pos_in(cat_area, me.column, me.row) && !app.cfg_categories.is_empty() {
                let cur = app.cfg_cat_state.selected().unwrap_or(0);
                let next = if up {
                    cur.saturating_sub(1)
                } else {
                    (cur + 1).min(app.cfg_categories.len().saturating_sub(1))
                };
                app.cfg_cat_state.select(Some(next));
                super::ensure_entry_selection(app);
                app.cfg_focus = ConfigFocus::Category;
                return Ok(());
            }

            // Scroll entries.
            if pos_in(entry_area, me.column, me.row) {
                if let Some(entries) = super::current_cfg_entries(app)
                    && !entries.is_empty()
                {
                    let cur = app.cfg_entry_state.selected().unwrap_or(0);
                    let next = if up {
                        cur.saturating_sub(1)
                    } else {
                        (cur + 1).min(entries.len().saturating_sub(1))
                    };
                    app.cfg_entry_state.select(Some(next));
                    app.cfg_focus = ConfigFocus::Entry;
                    return Ok(());
                }
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((cat_idx, entry_idx)) = app.cfg_editing {
                let editing_bool = app
                    .cfg_categories
                    .get(cat_idx)
                    .and_then(|c| c.entries.get(entry_idx))
                    .is_some_and(|e| super::cfg_field_is_bool(e.field));

                if editing_bool
                    && let Some(bool_area) = app.last_config_bool_area
                    && pos_in(bool_area, me.column, me.row)
                {
                    let idx = me.row.saturating_sub(bool_area.y + 1) as usize;
                    if idx < 2 {
                        app.cfg_bool_state.select(Some(idx));
                        app.cfg_edit_buffer = if idx == 0 { "true" } else { "false" }.to_string();

                        // Two-step confirm when enabling segment comments (mouse path).
                        let is_enable_segment = app
                            .cfg_categories
                            .get(cat_idx)
                            .and_then(|c| c.entries.get(entry_idx))
                            .is_some_and(|e| {
                                matches!(
                                    e.field,
                                    super::config_model::ConfigField::EnableSegmentComments
                                )
                            });
                        let want_enable = idx == 0;
                        if is_enable_segment && want_enable && !app.config.enable_segment_comments {
                            app.segment_comments_confirm_open = true;
                            app.segment_comments_confirm_ctx = Some((cat_idx, entry_idx));
                            app.segment_comments_confirm_state.select(Some(1));
                            app.status = "确认开启段评？".to_string();
                            return Ok(());
                        }

                        if let Err(err) = super::apply_cfg_edit(app, cat_idx, entry_idx) {
                            app.status = format!("保存失败: {err}");
                        } else {
                            app.cfg_editing = None;
                            app.cfg_edit_buffer.clear();
                        }
                    }
                    return Ok(());
                }

                // 编辑中时，点击其他区域不触发选择/跳转
                return Ok(());
            }

            if pos_in(cat_area, me.column, me.row) {
                if !app.cfg_categories.is_empty() {
                    if let Some(idx) = super::list_index_from_mouse_row(
                        cat_area,
                        me.row,
                        &app.cfg_cat_state,
                        app.cfg_categories.len(),
                    ) {
                        app.cfg_cat_state.select(Some(idx));
                        super::ensure_entry_selection(app);
                        app.cfg_focus = ConfigFocus::Category;
                        app.cfg_cat_hover = Some(idx);
                    }
                }
                return Ok(());
            }

            if let Some(btn_area) = button_area
                && pos_in(btn_area, me.column, me.row)
            {
                app.cfg_button_state.select(Some(0));
                app.view = View::Home;
                app.status = "返回主菜单".to_string();
                return Ok(());
            }

            if pos_in(entry_area, me.column, me.row) {
                if let Some(entries) = super::current_cfg_entries(app) {
                    if let Some(idx) = super::list_index_from_mouse_row(
                        entry_area,
                        me.row,
                        &app.cfg_entry_state,
                        entries.len(),
                    ) {
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
            // Hover category: only change color, do NOT change selected category / entries.
            if pos_in(cat_area, me.column, me.row) {
                if !app.cfg_categories.is_empty() {
                    if let Some(idx) = super::list_index_from_mouse_row(
                        cat_area,
                        me.row,
                        &app.cfg_cat_state,
                        app.cfg_categories.len(),
                    ) {
                        app.cfg_cat_hover = Some(idx);
                    } else {
                        app.cfg_cat_hover = None;
                    }
                }
                return Ok(());
            } else {
                app.cfg_cat_hover = None;
            }

            if let Some((cat_idx, entry_idx)) = app.cfg_editing {
                let editing_bool = app
                    .cfg_categories
                    .get(cat_idx)
                    .and_then(|c| c.entries.get(entry_idx))
                    .is_some_and(|e| super::cfg_field_is_bool(e.field));

                if editing_bool
                    && let Some(bool_area) = app.last_config_bool_area
                    && pos_in(bool_area, me.column, me.row)
                {
                    let idx = me.row.saturating_sub(bool_area.y + 1) as usize;
                    if idx < 2 {
                        app.cfg_bool_state.select(Some(idx));
                    }
                    return Ok(());
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
                    if let Some(idx) = super::list_index_from_mouse_row(
                        entry_area,
                        me.row,
                        &app.cfg_entry_state,
                        entries.len(),
                    ) {
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
        arr[1] = body.first().copied().unwrap_or_default();
        arr[2] = body.get(1).copied().unwrap_or_default();
        app.last_config_layout = Some(arr);

        let footer = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(18), Constraint::Min(10)])
            .split(layout[2]);
        app.last_config_button = footer.first().copied();
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

    let active_cat = app.cfg_cat_state.selected();
    let hover_cat = app.cfg_cat_hover;
    let cat_items: Vec<ListItem> = app
        .cfg_categories
        .iter()
        .enumerate()
        .map(|(idx, c)| {
            let is_active = active_cat == Some(idx);
            let is_hover = hover_cat == Some(idx) && !is_active;
            let mut style = Style::default();
            if is_active {
                style = style.fg(Color::LightCyan);
                if app.cfg_focus == ConfigFocus::Category {
                    style = style.add_modifier(Modifier::BOLD);
                }
            } else if is_hover {
                // Hover: change color but don't bold, and don't change entries.
                style = style.fg(Color::LightCyan);
            }
            ListItem::new(Line::from(Span::styled(c.title, style)))
        })
        .collect();
    let cat_block = Block::default()
        .borders(Borders::ALL)
        .title("分类 (左右/Tab 切换)");
    frame.render_widget(cat_block.clone(), body[0]);
    let cat_inner = cat_block.inner(body[0]);
    let need_cat_scrollbar = app.cfg_categories.len() > 0
        && cat_inner.height > 0
        && app.cfg_categories.len() > cat_inner.height as usize;
    let (cat_area, cat_sb) = if need_cat_scrollbar && cat_inner.width > 0 {
        let w = cat_inner.width.saturating_sub(1).max(1);
        (
            Rect {
                x: cat_inner.x,
                y: cat_inner.y,
                width: w,
                height: cat_inner.height,
            },
            Some(Rect {
                x: cat_inner.x.saturating_add(w),
                y: cat_inner.y,
                width: 1,
                height: cat_inner.height,
            }),
        )
    } else {
        (cat_inner, None)
    };

    let cat_list = List::new(cat_items)
        // Keep highlight style minimal; active/hover styles are applied per-item.
        .highlight_style(Style::default())
        .highlight_symbol(">> ");
    frame.render_stateful_widget(cat_list, cat_area, &mut app.cfg_cat_state);
    if let Some(sb_area) = cat_sb {
        let pos = app
            .cfg_cat_state
            .selected()
            .unwrap_or(0)
            .min(app.cfg_categories.len().saturating_sub(1));
        let mut sb_state = ScrollbarState::new(app.cfg_categories.len()).position(pos);
        let sb = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
        frame.render_stateful_widget(sb, sb_area, &mut sb_state);
    }

    let entries = super::current_cfg_entries(app);
    let entry_items: Vec<ListItem> = if let Some(entries) = entries {
        entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let val = super::current_cfg_value(app, entry.field);
                let mut spans = vec![Span::raw(format!("{}: {}", entry.title, val))];
                if let Some((cat_i, entry_i)) = app.cfg_editing
                    && Some(cat_i) == app.cfg_cat_state.selected()
                    && entry_i == idx
                {
                    spans.push(Span::raw("  [编辑中] "));
                    if !super::cfg_field_is_bool(entry.field) {
                        spans.push(Span::styled(
                            app.cfg_edit_buffer.clone(),
                            Style::default().fg(Color::Yellow),
                        ));
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

    let entry_block = Block::default()
        .borders(Borders::ALL)
        .title("配置项 (上下选择, 回车编辑/保存)");
    frame.render_widget(entry_block.clone(), body[1]);
    let entry_inner = entry_block.inner(body[1]);
    let entry_len = super::current_cfg_entries(app).map(|e| e.len()).unwrap_or(0);
    let need_entry_scrollbar =
        entry_len > 0 && entry_inner.height > 0 && entry_len > entry_inner.height as usize;
    let (entry_area, entry_sb) = if need_entry_scrollbar && entry_inner.width > 0 {
        let w = entry_inner.width.saturating_sub(1).max(1);
        (
            Rect {
                x: entry_inner.x,
                y: entry_inner.y,
                width: w,
                height: entry_inner.height,
            },
            Some(Rect {
                x: entry_inner.x.saturating_add(w),
                y: entry_inner.y,
                width: 1,
                height: entry_inner.height,
            }),
        )
    } else {
        (entry_inner, None)
    };

    let entry_list = List::new(entry_items)
        .highlight_style(entry_highlight)
        .highlight_symbol(">> ");
    frame.render_stateful_widget(entry_list, entry_area, &mut app.cfg_entry_state);
    if let Some(sb_area) = entry_sb {
        let pos = app
            .cfg_entry_state
            .selected()
            .unwrap_or(0)
            .min(entry_len.saturating_sub(1));
        let mut sb_state = ScrollbarState::new(entry_len).position(pos);
        let sb = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
        frame.render_stateful_widget(sb, sb_area, &mut sb_state);
    }

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

    if !app.segment_comments_confirm_open {
        app.last_segment_comments_confirm_options = None;
    }

    if app.segment_comments_confirm_open {
        render_segment_comments_confirm_modal(frame, app);
    }
}

fn render_segment_comments_confirm_modal(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.size();
    let w = (area.width as f32 * 0.72) as u16;
    // Keep the modal compact.
    let h: u16 = 12;
    let modal = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w.max(40).min(area.width.saturating_sub(2)),
        height: h.min(area.height.saturating_sub(2)).max(8),
    };

    frame.render_widget(Clear, modal);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("确认开启段评？")
        .border_style(Style::default().fg(Color::Yellow));
    frame.render_widget(block, modal);

    let inner = Rect {
        x: modal.x + 1,
        y: modal.y + 1,
        width: modal.width.saturating_sub(2),
        height: modal.height.saturating_sub(2),
    };

    let parts = Layout::default()
        .direction(Direction::Vertical)
        // Options list with borders needs at least 4 rows to show 2 items.
        .constraints([Constraint::Length(7), Constraint::Length(4)])
        .split(inner);

    let msg = vec![
        Line::from("段评会额外发送大量请求，容易触发 IP 风控。"),
        Line::from("同时下载会明显变慢（尤其是开启头像/图片下载时）。"),
        Line::from("建议：segment_comments_workers=1，关闭头像/图片下载。"),
        Line::from(""),
        Line::from("Enter 确认 / Esc 取消 / ←→ 切换"),
    ];
    let p = Paragraph::new(msg).wrap(Wrap { trim: true });
    frame.render_widget(p, parts[0]);

    let items = vec![ListItem::new("仍然开启"), ListItem::new("取消")];
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("选择"))
        .highlight_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    app.last_segment_comments_confirm_options = Some(parts[1]);
    frame.render_stateful_widget(list, parts[1], &mut app.segment_comments_confirm_state);
}
