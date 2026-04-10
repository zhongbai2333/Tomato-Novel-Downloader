//! PDF 输出后端 —— 基于 genpdf 0.2 生成 A5 尺寸 PDF。

use std::path::{Path, PathBuf};

use genpdf::elements::{Break, PageBreak, Paragraph};
use genpdf::fonts::FontData;
use genpdf::style::Style;
use genpdf::{Alignment, Document, Element, Scale, Size};
use serde_json::Value;
use tracing::{debug, info, warn};

use super::book_manager::BookManager;
use super::finalize_utils::volume_title_map_for_chapters;
use super::parser::ContentParser;
use crate::base_system::context::safe_fs_name;

/// 查找第一个存在的路径。
fn find_first_existing(paths: &[PathBuf]) -> Option<PathBuf> {
    for p in paths {
        if p.exists() {
            return Some(p.clone());
        }
    }
    None
}

/// 计算字符的"视觉半宽"：CJK / 全角字符 = 2，ASCII / 半角 = 1。
fn char_visual_width(c: char) -> usize {
    // CJK Unified Ideographs, CJK Ext-A/B, CJK Compatibility, Kangxi Radicals
    // Fullwidth forms, CJK Symbols, Kana, Hangul, etc.
    match c {
        '\u{1100}'..='\u{115F}'   // Hangul Jamo
        | '\u{2E80}'..='\u{303E}' // CJK Radicals, Kangxi, CJK Symbols
        | '\u{3040}'..='\u{33BF}' // Hiragana, Katakana, CJK Compatibility
        | '\u{3400}'..='\u{4DBF}' // CJK Ext-A
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
        | '\u{A000}'..='\u{A4CF}' // Yi
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{FE30}'..='\u{FE6F}' // CJK Compatibility Forms
        | '\u{FF01}'..='\u{FF60}' // Fullwidth Forms
        | '\u{FFE0}'..='\u{FFE6}' // Fullwidth Signs
        | '\u{20000}'..='\u{2FA1F}' // CJK Ext-B..F, Compatibility Supplement
        => 2,
        _ => 1,
    }
}

/// 标点禁则：这些字符不应出现在行首。
fn is_no_break_before(c: char) -> bool {
    matches!(
        c,
        '，' | '。'
            | '、'
            | '；'
            | '：'
            | '！'
            | '？'
            | '）'
            | '》'
            | '」'
            | '』'
            | '】'
            | '〉'
            | '〕'
            | '…'
            | '—'
            | '～'
            | '·'
            | '.'
            | ','
            | '!'
            | '?'
            | ')'
            | ';'
            | ':'
            | '>'
    )
}

/// genpdf 0.2 的文本换行在 CJK 下会出问题（内部只按 ASCII 空格分词），
/// 因此按视觉宽度预分行：CJK 字符计 2 个半宽单位，ASCII 计 1 个。
/// `max_half_widths` 为一行允许的最大半宽单位数。
/// 同时遵守标点禁则：不在行首放置收尾标点。
fn pre_wrap_line(line: &str, max_half_widths: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width: usize = 0;

    for c in line.chars() {
        let w = char_visual_width(c);
        // 超宽时换行，但如果当前字符是禁止行首的标点则留在本行
        if current_width + w > max_half_widths && !current.is_empty() && !is_no_break_before(c) {
            segments.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(c);
        current_width += w;
    }
    if !current.is_empty() {
        segments.push(current);
    }
    if segments.is_empty() {
        segments.push(String::new());
    }
    segments
}

/// 生成 PDF 并写入 `path`。
pub(super) fn finalize_pdf(
    manager: &BookManager,
    chapters: &[Value],
    path: &Path,
    directory_raw: Option<&Value>,
) -> anyhow::Result<()> {
    // ── 字体加载 ──────────────────────────────────────────────
    let font_path = manager
        .config
        .resolve_pdf_font_path()
        .ok_or_else(|| anyhow::anyhow!("未找到可用的中文字体，请在配置中设置 pdf_font_path"))?;

    debug!(target: "finalize_pdf", font = %font_path.display(), "loading font");
    let font_data = FontData::new(std::fs::read(&font_path)?, None)
        .map_err(|e| anyhow::anyhow!("加载字体失败: {e}"))?;

    let font_family = genpdf::fonts::FontFamily {
        regular: font_data.clone(),
        bold: font_data.clone(),
        italic: font_data.clone(),
        bold_italic: font_data,
    };

    // ── 文档初始化 ────────────────────────────────────────────
    let mut doc = Document::new(font_family);
    doc.set_title(&manager.book_name);

    // A5: 148 × 210 mm
    let page_size = Size::new(148, 210);
    doc.set_paper_size(page_size);

    // 页边距
    let mut decorator = genpdf::SimplePageDecorator::new();
    decorator.set_margins(genpdf::Margins::trbl(15, 15, 15, 15));
    doc.set_page_decorator(decorator);
    doc.set_minimal_conformance();
    doc.set_line_spacing(1.5);

    // ── 封面图 ─────────────────────────────────────────────────
    let safe_title = safe_fs_name(&manager.book_name, "_", 120);
    let extensions = ["jpg", "jpeg", "png", "webp"];
    let cover_candidates: Vec<PathBuf> = if let Some(base) = manager.config.get_status_folder_path()
    {
        let mut v = Vec::new();
        for ext in &extensions {
            v.push(base.join(format!("{safe_title}.{ext}")));
        }
        for ext in &extensions {
            v.push(base.join(format!("{}.{ext}", manager.book_name)));
        }
        v
    } else {
        vec![]
    };

    if let Some(cover_path) = find_first_existing(&cover_candidates) {
        debug!(target: "finalize_pdf", cover = %cover_path.display(), "adding cover image");
        match genpdf::elements::Image::from_path(&cover_path) {
            Ok(img) => {
                // 根据图片实际像素尺寸计算缩放，使封面适应 A5 内容区域 (118×180mm)
                let content_w_mm = 118.0_f64;
                let content_h_mm = 180.0_f64;
                let dpi = 300.0_f64;
                let mmpi = 25.4_f64;

                let scale = if let Ok(reader) = image::ImageReader::open(&cover_path) {
                    if let Ok(dim) = reader.into_dimensions() {
                        let img_w_mm = mmpi * dim.0 as f64 / dpi;
                        let img_h_mm = mmpi * dim.1 as f64 / dpi;
                        let s = (content_w_mm / img_w_mm).min(content_h_mm / img_h_mm);
                        debug!(target: "finalize_pdf", px_w = dim.0, px_h = dim.1, scale = s, "cover scale");
                        s
                    } else {
                        2.0
                    }
                } else {
                    2.0
                };

                let img = img
                    .with_alignment(Alignment::Center)
                    .with_scale(Scale::new(scale, scale));
                doc.push(img);
                // 封面后直接分页（genpdf 不会自动分页）
                doc.push(PageBreak::new());
            }
            Err(e) => {
                warn!(target: "finalize_pdf", error = ?e, "cover image load failed, skipping");
            }
        }
    }

    // ── 书籍信息页 ────────────────────────────────────────────
    // A5 内容区 118mm，10pt 中文字约 3.53mm → 33 字 ≈ 66 半宽单位
    const MAX_HW_BODY: usize = 66;
    const MAX_HW_TITLE: usize = 40;
    let title_style = Style::new().with_font_size(18);
    let meta_style = Style::new().with_font_size(11);
    for seg in pre_wrap_line(&manager.book_name, MAX_HW_TITLE) {
        doc.push(
            Paragraph::new(seg)
                .aligned(Alignment::Center)
                .styled(title_style),
        );
    }
    doc.push(Break::new(1));

    if !manager.author.trim().is_empty() {
        doc.push(Paragraph::new(format!("作者：{}", manager.author)).styled(meta_style));
    }

    let status_text = match manager.finished {
        Some(true) => "完结",
        Some(false) => "连载",
        None => "未知",
    };
    doc.push(Paragraph::new(format!("状态：{status_text}")).styled(meta_style));

    if let Some(category) = manager.category.as_deref()
        && !category.trim().is_empty()
    {
        doc.push(Paragraph::new(format!("分类：{}", category.trim())).styled(meta_style));
    }

    if !manager.tags.trim().is_empty() {
        doc.push(Paragraph::new(format!("标签：{}", manager.tags)).styled(meta_style));
    }

    if !manager.description.trim().is_empty() {
        doc.push(Break::new(1));
        doc.push(Paragraph::new("简介：").styled(meta_style));
        // 先按换行符拆分，再对每小段做视觉宽度预分行
        for sub_line in manager.description.trim().lines() {
            let sub_line = sub_line.trim();
            if sub_line.is_empty() {
                doc.push(Break::new(0.3));
            } else {
                for seg in pre_wrap_line(sub_line, MAX_HW_BODY) {
                    doc.push(
                        Paragraph::new(seg)
                            .aligned(Alignment::Left)
                            .styled(meta_style),
                    );
                }
            }
        }
    }

    doc.push(PageBreak::new());

    // ── 分卷 / 章节 ──────────────────────────────────────────
    let volume_map = volume_title_map_for_chapters(chapters, directory_raw);
    let volume_style = Style::new().with_font_size(16);
    let chapter_title_style = Style::new().with_font_size(14);
    let body_style = Style::new().with_font_size(10);

    let mut last_volume: Option<String> = None;

    for ch in chapters {
        let chapter_id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let title = ch.get("title").and_then(|v| v.as_str()).unwrap_or("章节");
        let content = ch.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let content = ContentParser::clean_plain(content, title);

        // 分卷标题
        if let Some(vol) = volume_map.get(chapter_id) {
            let vol = vol.trim();
            if !vol.is_empty() && last_volume.as_deref() != Some(vol) {
                doc.push(Break::new(1));
                doc.push(
                    Paragraph::new(vol)
                        .aligned(Alignment::Center)
                        .styled(volume_style),
                );
                doc.push(Break::new(1));
                last_volume = Some(vol.to_string());
            }
        }

        // 章节标题
        for seg in pre_wrap_line(title, MAX_HW_TITLE) {
            doc.push(Paragraph::new(seg).styled(chapter_title_style));
        }
        doc.push(Break::new(0.5));

        // 正文：每段首行加两个全角空格缩进
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                doc.push(Break::new(0.3));
            } else {
                let indented = format!("\u{3000}\u{3000}{line}");
                let segs = pre_wrap_line(&indented, MAX_HW_BODY);
                for seg in segs {
                    doc.push(
                        Paragraph::new(seg)
                            .aligned(Alignment::Left)
                            .styled(body_style),
                    );
                }
            }
        }

        doc.push(PageBreak::new());
    }

    // ── 渲染输出 ──────────────────────────────────────────────
    doc.render_to_file(path)
        .map_err(|e| anyhow::anyhow!("PDF 渲染失败: {e}"))?;

    info!(target: "finalize_pdf", path = %path.display(), "PDF generated");
    Ok(())
}
