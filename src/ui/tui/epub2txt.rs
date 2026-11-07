//! EPUB 转 TXT 页面。
//!
//! 输入本地 `.epub` 文件路径（或包含 epub 的目录），在后台线程中转换，
//! 结果写回 `app.epub2txt_log` 并在页面上滚动展示。

use super::*;

use std::path::Path;
use std::thread;

pub(super) fn handle_event_epub2txt(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Paste(s) => {
            if !app.epub2txt_running {
                app.epub2txt_input.push_str(&s);
            }
        }
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                app.view = View::Home;
                app.focus = Focus::Input;
                app.status = "已返回首页".to_string();
            }
            KeyCode::Enter => start_convert(app)?,
            KeyCode::Backspace => {
                if !app.epub2txt_running {
                    app.epub2txt_input.pop();
                }
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                #[cfg(feature = "clipboard")]
                {
                    if let Ok(Some(text)) = super::clipboard::get_text() {
                        app.epub2txt_input.push_str(&text);
                    } else {
                        app.status = "读取剪贴板失败（可手动粘贴，或使用 Shift+Insert）".to_string();
                    }
                }
                #[cfg(not(feature = "clipboard"))]
                {
                    app.status = "当前构建未启用剪贴板支持".to_string();
                }
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if !app.epub2txt_running {
                    app.epub2txt_input.push(c);
                }
            }
            _ => {}
        },
        _ => {}
    }

    Ok(())
}

fn start_convert(app: &mut App) -> Result<()> {
    if app.epub2txt_running {
        app.status = "转换进行中，请稍候…".to_string();
        return Ok(());
    }

    // Windows 资源管理器“复制为路径”会带引号，这里顺手剥掉。
    let cleaned = app
        .epub2txt_input
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string();

    if cleaned.is_empty() {
        app.status = "请输入 EPUB 文件或目录路径".to_string();
        return Ok(());
    }

    app.epub2txt_input = cleaned.clone();
    app.epub2txt_running = true;
    app.epub2txt_log.push(format!("开始转换: {cleaned}"));
    if app.epub2txt_log.len() > 200 {
        let overflow = app.epub2txt_log.len() - 200;
        app.epub2txt_log.drain(0..overflow);
    }
    app.status = "转换中…".to_string();
    super::start_spinner(app, "转换中…");

    let tx = app.worker_tx.clone();
    thread::spawn(move || {
        let result = crate::book_parser::epub_reader::convert_path(Path::new(&cleaned), None);
        let _ = tx.send(WorkerMsg::Epub2TxtDone(result));
    });

    Ok(())
}

pub(super) fn draw_epub2txt(frame: &mut ratatui::Frame, app: &mut App) {
    let (main, log_area) = super::split_with_log(frame.size());
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(3),
        ])
        .split(main);

    let header_line = Line::from(vec![
        Span::styled(
            "EPUB 转 TXT",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  Enter 开始转换，Esc 返回；支持单个 .epub 或目录（递归）"),
    ]);
    let header = Paragraph::new(header_line)
        .block(Block::default().borders(Borders::ALL).title("转换工具"));
    frame.render_widget(header, layout[0]);

    let input_style = if app.epub2txt_running {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Yellow)
    };
    let input = Paragraph::new(format!("> {}", app.epub2txt_input))
        .style(input_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("EPUB 路径（Ctrl+V 粘贴）"),
        );
    frame.render_widget(input, layout[1]);

    let visible = layout[2].height.saturating_sub(2).max(1) as usize;
    let lines: Vec<Line> = app
        .epub2txt_log
        .iter()
        .rev()
        .take(visible)
        .rev()
        .map(|line| Line::from(line.as_str()))
        .collect();
    let log = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .title("转换日志（最新在底部）"),
    );
    frame.render_widget(log, layout[2]);

    let status = Paragraph::new(Line::from(app.status.clone()))
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("状态"));
    frame.render_widget(status, layout[3]);

    super::render_log_box(frame, log_area, app);
}
