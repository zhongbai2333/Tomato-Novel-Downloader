use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::Result;
use epub_builder::{EpubBuilder, EpubContent, ReferenceType, ZipLibrary};

use crate::base_system::context::{Config, safe_fs_name};

pub struct EpubGenerator {
    book: EpubBuilder<ZipLibrary>,
    chapters: Vec<(String, String)>,
    style: String,
    file_counter: usize,
    title: String,
}

impl EpubGenerator {
    pub fn new(identifier: &str, title: &str, cfg: &Config) -> Result<Self> {
        let zip = ZipLibrary::new().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let mut book = EpubBuilder::new(zip).map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let safe_id = safe_fs_name(identifier, "_", 120);
        book.metadata("identifier", safe_id).ok();
        book.metadata("title", title).ok();
        book.metadata("language", "en").ok();
        book.metadata("author", "unknown").ok();

        let indent_em = cfg.first_line_indent_em.max(0.0);
        let indent_rule = if indent_em > 0.0 {
            format!("text-indent:{}em;", indent_em)
        } else {
            "text-indent:0;".to_string()
        };
        let css = format!(
            "body {{ color:#000 !important; line-height:1.5; }}
             p {{ color:#000 !important; {} margin:0 0 .8em 0; line-height:1.5; }}
             p.no-indent {{ text-indent:0; }}
             a.seg-link {{ color: inherit; text-decoration: none; }}
             a.seg-link:hover {{ text-decoration: underline; }}
             .seg-count {{ color:#999; font-size:0.75em; margin-left:.3em; text-decoration: none; }}
             h3 {{ background:#f5f5f5; padding:.4em .6em; color:#333; font-weight:500; }}
             .para-title .para-index {{ font-weight:600; margin-right:.25em; color:#333; }}
             .para-title .para-src {{ color:#333; }}
             .para-title small {{ color:#666; }}
             .back-to-chapter {{ margin:.2em 0 .8em 0; }}
             .back-to-chapter a {{ font-size:.85em; color:#666; text-decoration:none; }}
             .back-to-chapter a:hover {{ color:#333; text-decoration:underline; }}
             .seg-images {{ margin:.25em 0 .5em 0; display:block; }}
             .seg-images img {{ max-width:100%; height:auto; max-height:220px; margin-right:.4em; margin-bottom:.2em; border-radius:2px; object-fit:contain; }}
             .avatar {{ width:36px; height:36px; border-radius:50%; object-fit:cover; vertical-align:middle; margin-right:.5em; }}
             .seg-meta {{ color:#666; display:block; text-align:right; }}
             li.seg-item {{ border-bottom:1px solid #ddd; padding:.5em 0 .6em 0; }}
             li.seg-item:last-child {{ border-bottom:none; }}",
            indent_rule
        );

        Ok(Self {
            book,
            chapters: Vec::new(),
            style: css,
            file_counter: 0,
            title: title.to_string(),
        })
    }

    pub fn add_chapter(&mut self, title: &str, content: &str) {
        let file_name = format!("chapter_{:05}.xhtml", self.file_counter);
        self.file_counter += 1;
        let cleaned = if content.trim().is_empty() {
            format!(
                "<h3>{}</h3><p class='no-indent'>本章内容未下载完成或为空（可能是用户中断或网络错误）。</p>",
                html_escape(title)
            )
        } else {
            content.to_string()
        };
        self.chapters
            .push((file_name.clone(), wrap_chapter_html(title, &cleaned)));
    }

    pub fn add_aux_page(&mut self, title: &str, content: &str, include_in_spine: bool) {
        let file_name = format!("aux_{:05}.xhtml", self.file_counter);
        self.file_counter += 1;
        let cleaned = if content.trim().is_empty() {
            format!(
                "<h3>{}</h3><p class='no-indent'>（空页面）</p>",
                html_escape(title)
            )
        } else {
            content.to_string()
        };
        if include_in_spine {
            self.chapters
                .push((file_name.clone(), wrap_chapter_html(title, &cleaned)));
        }
    }

    pub fn generate(&mut self, output_path: &Path, cfg: &Config) -> Result<()> {
        if let Some(base) = cfg.get_status_folder_path() {
            let safe_title = safe_fs_name(&self.title, "_", 120);
            let cover_path_safe = base.join(format!("{safe_title}.jpg"));
            let cover_path_legacy = base.join(format!("{}.jpg", self.title));
            if let Some(bytes) = read_first_existing(&[&cover_path_safe, &cover_path_legacy]) {
                let cursor = Cursor::new(bytes);
                self.book
                    .add_content(
                        EpubContent::new("images/cover.jpg", cursor)
                            .title("cover")
                            .reftype(ReferenceType::Cover),
                    )
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            }
        }

        self.book
            .add_content(
                EpubContent::new("styles/main.css", Cursor::new(self.style.clone())).title("style"),
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        for (file_name, html) in &self.chapters {
            self.book
                .add_content(
                    EpubContent::new(file_name.clone(), Cursor::new(html.clone()))
                        .title(file_name)
                        .reftype(ReferenceType::Text),
                )
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }

        let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let file = fs::File::create(output_path)?;
        self.book
            .generate(file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

fn read_first_existing(paths: &[&Path]) -> Option<Vec<u8>> {
    for p in paths {
        if let Ok(bytes) = fs::read(p) {
            return Some(bytes);
        }
    }
    None
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn wrap_chapter_html(title: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{}</title></head><body><h3>{}</h3><div>{}</div></body></html>",
        html_escape(title),
        html_escape(title),
        body
    )
}
