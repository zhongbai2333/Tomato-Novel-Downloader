//! TUI 封面/基础信息展示。

use super::*;
use image::{DynamicImage, GenericImageView, imageops::FilterType};

pub(super) fn handle_event_cover(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                app.view = app.previous_view;
                app.status = "返回".to_string();
            }
            _ => {}
        },
        Event::Mouse(me) => {
            if let MouseEventKind::Down(MouseButton::Left) = me.kind {
                app.view = app.previous_view;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn show_cover(
    app: &mut App,
    book_id: &str,
    title: &str,
    folder: Option<PathBuf>,
) -> Result<()> {
    app.previous_view = app.view;
    app.cover_lines.clear();

    let cover_title = format!("《{}》 ({})", title, book_id);
    app.cover_title = cover_title;

    let candidates = cover_candidates(app, book_id, title, folder);
    let Some(path) = candidates.into_iter().find(|p| p.exists()) else {
        app.view = View::Cover;
        app.status = "未找到封面文件".to_string();
        return Ok(());
    };

    let img = image::open(&path).with_context(|| format!("读取封面失败: {}", path.display()))?;
    let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 24));
    let ascii = image_to_ascii(img, term_w, term_h);
    app.cover_lines = if ascii.is_empty() {
        vec!["封面太小，无法显示".to_string()]
    } else {
        ascii
    };
    app.view = View::Cover;
    app.status = format!("封面: {} (按 q 返回)", path.display());
    Ok(())
}

pub(super) fn draw_cover(frame: &mut ratatui::Frame, app: &mut App) {
    let (main, log_area) = super::split_with_log(frame.size());
    let title = if app.cover_title.is_empty() {
        "封面预览".to_string()
    } else {
        app.cover_title.clone()
    };

    let lines: Vec<Line> = if app.cover_lines.is_empty() {
        vec![Line::from("未找到封面，按 q 返回")]
    } else {
        app.cover_lines.iter().cloned().map(Line::from).collect()
    };

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, main);
    super::render_log_box(frame, log_area, app);
}

fn cover_candidates(
    app: &App,
    book_id: &str,
    title: &str,
    folder: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let safe_title = safe_fs_name(title, "_", 120);

    if let Some(base) = folder.as_ref() {
        push_cover_candidates(&mut paths, base, title, &safe_title);
    }

    if let Some(status) = app.config.get_status_folder_path() {
        push_cover_candidates(&mut paths, &status, title, &safe_title);
    }

    let default_folder = app
        .config
        .default_save_dir()
        .join(format!("{}_{}", book_id, safe_title));
    push_cover_candidates(&mut paths, &default_folder, title, &safe_title);

    paths
}

fn push_cover_candidates(list: &mut Vec<PathBuf>, base: &Path, title: &str, safe_title: &str) {
    let names = [
        format!("{safe_title}.jpg"),
        format!("{safe_title}.png"),
        format!("{}.jpg", title),
        format!("{}.png", title),
        "cover.jpg".to_string(),
        "cover.png".to_string(),
    ];

    for name in names {
        let path = base.join(&name);
        if !list.contains(&path) {
            list.push(path);
        }
    }
}

fn image_to_ascii(img: DynamicImage, term_w: u16, term_h: u16) -> Vec<String> {
    const PALETTE: &[u8] = b" .:-=+*#%@";
    let max_width = term_w.saturating_sub(6).max(16) as u32;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Vec::new();
    }

    let target_width = std::cmp::min(max_width, w.max(1));
    let mut target_height = h
        .saturating_mul(target_width)
        .saturating_mul(2)
        .saturating_div(w.max(1));
    let max_height = term_h.saturating_sub(4).max(8) as u32;
    if target_height > max_height {
        target_height = max_height;
    }

    let gray = img
        .resize(
            target_width.max(1),
            target_height.max(1),
            FilterType::Triangle,
        )
        .to_luma8();
    let mut lines = Vec::with_capacity(gray.height() as usize);
    for y in 0..gray.height() {
        let mut line = String::with_capacity(gray.width() as usize);
        for x in 0..gray.width() {
            let v = gray.get_pixel(x, y)[0] as f32 / 255.0;
            let idx = (v * (PALETTE.len() as f32 - 1.0)).round() as usize;
            line.push(*PALETTE.get(idx).unwrap_or(&b' ') as char);
        }
        lines.push(line);
    }
    lines
}
