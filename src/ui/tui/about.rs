//! TUI About 页面。
//!
//! 展示项目信息，并提供打开链接等按钮。

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
                    if cfg!(feature = "docker") {
                        app.view = View::Home;
                        app.status = "返回主菜单".to_string();
                    } else {
                        check_app_update(app)?;
                    }
                }
                Some(2) => {
                    if !cfg!(feature = "docker") {
                        request_self_update(app)?;
                    }
                }
                Some(3) => {
                    if !cfg!(feature = "docker") {
                        dismiss_app_update(app)?;
                    }
                }
                Some(4) => {
                    if !cfg!(feature = "docker") {
                        app.view = View::Home;
                        app.status = "返回主菜单".to_string();
                    }
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
            Constraint::Length(7),
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
        Span::raw("  |  q/Esc 返回"),
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

    let mut text = String::new();
    text.push_str("项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader\n");
    text.push_str("Fork From: https://github.com/Dlmily/Tomato-Novel-Downloader-Lite\n");
    text.push_str("作者: zhongbai2333\n");
    text.push_str("本项目仅供学习交流使用，请勿用于商业及违法行为。\n");
    text.push_str(&format!("\n当前版本: v{}\n", env!("CARGO_PKG_VERSION")));

    text.push_str("\n===== 程序更新 =====\n");
    if cfg!(feature = "docker") {
        text.push_str("Docker 构建已禁用程序自更新，请通过重新拉取镜像进行升级。\n");
    } else if let Some(rep) = &app.app_update_report {
        text.push_str(&format!("当前: {}\n", rep.current_tag));
        text.push_str(&format!("最新: {}\n", rep.latest.tag_name));
        if rep.is_new_version {
            text.push_str("状态: 有新版本\n");
        } else {
            text.push_str("状态: 已是最新版本\n");
        }
        if rep.is_dismissed {
            text.push_str("提示: 已设置忽略该版本提醒（仍可手动检查）\n");
        }
        if let Some(url) = rep.latest.html_url.as_deref()
            && !url.trim().is_empty()
        {
            text.push_str(&format!("Release: {}\n", url));
        }
        if let Some(body) = rep.latest.body.as_deref() {
            let body = body.trim();
            if !body.is_empty() {
                text.push_str("\n更新日志（节选）:\n");
                text.push_str(&preview_notes(body, 16, 1800));
                text.push('\n');
            }
        }
    } else {
        text.push_str("未检查更新（点击“检查程序更新”）\n");
    }

    let body = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("项目说明"));
    frame.render_widget(body, layout[2]);

    super::render_log_box(frame, log_area, app);
}

fn check_app_update(app: &mut App) -> Result<()> {
    app.status = "正在检查程序更新…".to_string();
    super::start_app_update_check(app);
    Ok(())
}

fn dismiss_app_update(app: &mut App) -> Result<()> {
    let Some(rep) = app.app_update_report.clone() else {
        app.status = "尚未获取更新信息，先点“检查程序更新”".to_string();
        return Ok(());
    };
    if !rep.is_new_version {
        app.status = "当前已是最新版本，无需设置提醒".to_string();
        return Ok(());
    }
    let tag = rep.latest.tag_name.clone();
    crate::base_system::app_update::dismiss_release_tag(&tag)?;
    let mut new_rep = rep;
    new_rep.is_dismissed = true;
    app.app_update_report = Some(new_rep);
    app.status = format!("已设置不再提醒 {}", tag);
    Ok(())
}

fn request_self_update(app: &mut App) -> Result<()> {
    app.status = "即将执行自更新（退出 TUI 后开始）…".to_string();
    app.self_update_requested = true;
    // TUI 内已有明确的“执行自更新”按钮，点击即视为确认。
    // 因此不再在 self_update 内二次询问。
    app.self_update_auto_yes = true;
    app.should_quit = true;
    Ok(())
}

fn preview_notes(body: &str, max_lines: usize, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, line) in body.lines().enumerate() {
        if i >= max_lines {
            out.push('…');
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        if out.len() >= max_chars {
            // 在字符边界安全截断，避免中文等多字节字符被截断导致 panic
            let mut end = max_chars;
            while !out.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            out.truncate(end);
            out.push('…');
            break;
        }
    }
    out
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
                    if cfg!(feature = "docker") {
                        app.view = View::Home;
                        app.status = "返回主菜单".to_string();
                    } else {
                        check_app_update(app)?;
                    }
                }
                2 => {
                    if !cfg!(feature = "docker") {
                        app.about_btn_state.select(Some(2));
                        request_self_update(app)?;
                    }
                }
                3 => {
                    if !cfg!(feature = "docker") {
                        app.about_btn_state.select(Some(3));
                        dismiss_app_update(app)?;
                    }
                }
                4 => {
                    if !cfg!(feature = "docker") {
                        app.about_btn_state.select(Some(4));
                        app.view = View::Home;
                        app.status = "返回主菜单".to_string();
                    }
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
        // Avoid spawning `cmd.exe` (it can mutate console modes and break mouse events).
        // `explorer.exe` uses the default URL handler without touching our console settings.
        Command::new("explorer")
            .arg(url)
            .spawn()
            .or_else(|_| Command::new("cmd").args(["/C", "start", url]).spawn())
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };

    match spawn_result {
        Ok(_) => app.status = format!("已尝试在浏览器打开: {url}"),
        Err(e) => app.status = format!("打开浏览器失败: {e}"),
    }

    // Best-effort: some OS openers may still toggle console modes.
    // Re-assert our expected modes so returning to the TUI keeps mouse usable.
    let _ = enable_raw_mode();
    let mut out = std::io::stdout();
    let _ = crossterm_execute!(&mut out, EnableMouseCapture);
    Ok(())
}
