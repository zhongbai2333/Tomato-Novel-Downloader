use super::*;

pub(super) fn handle_event_about(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                app.view = View::Home;
                app.status = "返回主菜单".to_string();
            }
            KeyCode::Enter => match app.about_btn_state.selected() {
                Some(0) => {
                    open_github_repo(app)?;
                }
                Some(1) => {
                    app.view = View::Home;
                    app.status = "返回主菜单".to_string();
                }
                _ => {}
            },
            KeyCode::Up => {
                let idx = app.about_btn_state.selected().unwrap_or(0);
                let prev = if idx == 0 {
                    ABOUT_BUTTONS.len() - 1
                } else {
                    idx - 1
                };
                app.about_btn_state.select(Some(prev));
            }
            KeyCode::Down => {
                let idx = app.about_btn_state.selected().unwrap_or(0);
                let next = (idx + 1) % ABOUT_BUTTONS.len();
                app.about_btn_state.select(Some(next));
            }
            _ => {}
        },
        Event::Mouse(me) => handle_mouse_about(app, me)?,
        _ => {}
    }
    Ok(())
}

pub(super) fn draw_about(frame: &mut ratatui::Frame, app: &mut App) {
    let (main, log_area) = super::split_with_log(frame.size());
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(5),
        ])
        .split(main);

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "关于 / About",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  按 q/Enter 返回"),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Tomato Novel Downloader"),
    );
    frame.render_widget(header, layout[0]);

    let button_area = layout[1];
    let btn_items: Vec<ListItem> = ABOUT_BUTTONS.iter().map(|b| ListItem::new(*b)).collect();
    let btn_list = List::new(btn_items)
        .block(Block::default().borders(Borders::ALL).title("操作"))
        .highlight_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(btn_list, button_area, &mut app.about_btn_state);
    app.last_about_buttons = Some(button_area);

    let text = "项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader\nFork From: https://github.com/Dlmily/Tomato-Novel-Downloader-Lite\n作者: zhongbai2333\n本项目仅供学习交流使用，请勿用于商业及违法行为。";
    let body = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("项目说明"));
    frame.render_widget(body, layout[2]);

    super::render_log_box(frame, log_area, app);
}

fn handle_mouse_about(app: &mut App, me: event::MouseEvent) -> Result<()> {
    let Some(area) = app.last_about_buttons else {
        return Ok(());
    };
    let pos_in = |rect: Rect, col: u16, row: u16| {
        col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
    };
    if !pos_in(area, me.column, me.row) {
        return Ok(());
    }

    match me.kind {
        MouseEventKind::Moved => {
            let idx = me.row.saturating_sub(area.y + 1) as usize;
            if idx < ABOUT_BUTTONS.len() {
                app.about_btn_state.select(Some(idx));
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let idx = me.row.saturating_sub(area.y + 1) as usize;
            match idx {
                0 => {
                    app.about_btn_state.select(Some(0));
                    open_github_repo(app)?;
                }
                1 => {
                    app.about_btn_state.select(Some(1));
                    app.view = View::Home;
                    app.status = "返回主菜单".to_string();
                }
                _ => {}
            }
        }
        _ => {}
    }

    Ok(())
}

fn open_github_repo(app: &mut App) -> Result<()> {
    let url = "https://github.com/zhongbai2333/Tomato-Novel-Downloader";
    let spawn_result = if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", "start", url]).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };

    match spawn_result {
        Ok(_) => app.status = format!("已尝试在浏览器打开: {url}"),
        Err(e) => app.status = format!("打开浏览器失败: {e}"),
    }
    Ok(())
}
