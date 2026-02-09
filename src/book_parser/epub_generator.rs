//! EPUB 生成器。

use std::fs;
use std::io::{Cursor, Read as _, Write as _};
use std::path::Path;

use anyhow::Result;
use epub_builder::{EpubBuilder, EpubContent, EpubVersion, ReferenceType, ZipLibrary};

use crate::base_system::context::{Config, safe_fs_name};

/// 用于从 book_id 确定性生成 UUID v5 的命名空间。
/// 这保证同一本书（同 book_id）的 dc:identifier 永远不变。
const EPUB_UUID_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

pub struct EpubGenerator {
    book: EpubBuilder<ZipLibrary>,
    chapters: Vec<(String, String)>,
    style: String,
    #[allow(dead_code)]
    file_counter: usize,
    title: String,
    /// 原始 book_id，用于生成确定性 UUID 并在后处理中替换为 dc:identifier 的值。
    book_id: String,
}

impl EpubGenerator {
    pub fn new(
        identifier: &str,
        title: &str,
        author: &str,
        tags: &str,
        description: &str,
        cfg: &Config,
    ) -> Result<Self> {
        let zip = ZipLibrary::new().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let mut book = EpubBuilder::new(zip).map_err(|e| anyhow::anyhow!(e.to_string()))?;

        // 使用 EPUB 3.0 格式输出，与 Python 版本一致。
        book.epub_version(EpubVersion::V30);

        // 使用 UUID v5 从 book_id 确定性生成 dc:identifier，保证同一本书
        // 无论更新多少次，identifier 都不会改变，阅读器可正确识别/恢复进度。
        let stable_uuid = uuid::Uuid::new_v5(&EPUB_UUID_NAMESPACE, identifier.as_bytes());
        book.set_uuid(stable_uuid);
        book.metadata("title", title).ok();
        book.metadata("lang", "zh").ok();

        // 设置 toc_name 为书名，使 toc.ncx docTitle 和 nav.xhtml title 显示书名
        // 而非默认的 "Table Of Contents"。
        book.metadata("toc_name", title).ok();

        let author = author.trim();
        if !author.is_empty() {
            // epub-builder 的 "author" key 会写入 dc:creator 标签。
            book.metadata("author", author).ok();
        }

        let tags = tags.trim();
        if !tags.is_empty() {
            // Tags/keywords
            book.metadata("subject", tags).ok();
        }

        let description = description.trim();
        if !description.is_empty() {
            book.metadata("description", description).ok();
        }

        // epub-builder 不支持 "publisher" metadata key，
        // 改用 generator 字段标记来源。
        book.metadata("generator", "Tomato-Novel-Downloader").ok();

        let indent_em = cfg.first_line_indent_em.max(0.0);
        let indent_rule = if indent_em > 0.0 {
            format!("text-indent:{}em;", indent_em)
        } else {
            "text-indent:0;".to_string()
        };
        let css = format!(
            "body {{ font-family: serif; color:#000 !important; line-height:1.5; }}
             p {{ color:#000 !important; {} margin:0 0 .8em 0; line-height:1.5; }}
             p.no-indent {{ text-indent:0; }}
             p.img-desc {{ color:#999 !important; font-size:0.75em; text-indent:0; text-align:center; margin:-.4em 0 .9em 0; }}
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
            book_id: identifier.to_string(),
        })
    }

    #[allow(dead_code)]
    pub fn add_chapter(&mut self, title: &str, content: &str) {
        let file_name = format!("chapter_{:05}.xhtml", self.file_counter);
        self.file_counter += 1;
        self.add_chapter_named(file_name, title, content);
    }

    pub fn add_chapter_named(&mut self, file_name: String, title: &str, content: &str) {
        let cleaned = if content.trim().is_empty() {
            "<p class='no-indent'>本章内容未下载完成或为空（可能是用户中断或网络错误）。</p>"
                .to_string()
        } else {
            content.to_string()
        };
        self.chapters
            .push((file_name, wrap_chapter_html(title, &cleaned)));
    }

    #[allow(dead_code)]
    pub fn add_aux_page(&mut self, title: &str, content: &str, include_in_spine: bool) -> String {
        let file_name = format!("aux_{:05}.xhtml", self.file_counter);
        self.file_counter += 1;
        self.add_aux_page_named(file_name, title, content, include_in_spine)
    }

    pub fn add_aux_page_named(
        &mut self,
        file_name: String,
        title: &str,
        content: &str,
        include_in_spine: bool,
    ) -> String {
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
        file_name
    }

    pub fn add_resource_bytes(&mut self, path: &str, bytes: Vec<u8>, mime: &str) -> Result<()> {
        self.book
            .add_resource(path, Cursor::new(bytes), mime)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }

    pub fn generate(&mut self, output_path: &Path, cfg: &Config) -> Result<()> {
        if let Some(base) = cfg.get_status_folder_path() {
            let safe_title = safe_fs_name(&self.title, "_", 120);
            let cover_path_safe_jpg = base.join(format!("{safe_title}.jpg"));
            let cover_path_safe_png = base.join(format!("{safe_title}.png"));
            let cover_path_legacy_jpg = base.join(format!("{}.jpg", self.title));
            let cover_path_legacy_png = base.join(format!("{}.png", self.title));
            if let Some(bytes) = read_first_existing(&[
                &cover_path_safe_jpg,
                &cover_path_safe_png,
                &cover_path_legacy_jpg,
                &cover_path_legacy_png,
            ]) {
                let cursor = Cursor::new(bytes);
                self.book
                    .add_cover_image("images/cover.jpg", cursor, "image/jpeg")
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            }
        }

        // 使用 stylesheet() 而非 add_resource()，防止 epub-builder 自动创建空的 stylesheet.css。
        self.book
            .stylesheet(Cursor::new(self.style.clone()))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        for (file_name, html) in &self.chapters {
            self.book
                .add_content(
                    EpubContent::new(file_name.clone(), Cursor::new(html.clone()))
                        .title(title_from_file_or_html(file_name, html))
                        .reftype(ReferenceType::Text),
                )
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }

        let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        // 先生成到内存缓冲区，然后后处理替换 dc:identifier 为原始 book_id。
        // epub-builder 强制输出 urn:uuid:xxx 格式，无法通过 API 设置纯文本 identifier。
        let mut buffer = Vec::new();
        self.book
            .generate(&mut buffer)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let buffer = self.fixup_epub_identifier(buffer)?;
        fs::write(output_path, buffer)?;
        Ok(())
    }

    /// 后处理 EPUB zip：
    /// 1. 将 content.opf 和 toc.ncx 中的 `urn:uuid:xxx` 替换为原始 book_id
    /// 2. 在 toc.ncx 的 `<head>` 中补充 `<meta name="dtb:uid" content="{book_id}" />`
    fn fixup_epub_identifier(&self, epub_bytes: Vec<u8>) -> Result<Vec<u8>> {
        let stable_uuid = uuid::Uuid::new_v5(&EPUB_UUID_NAMESPACE, self.book_id.as_bytes());
        let urn_str = format!("urn:uuid:{}", stable_uuid.hyphenated());

        let reader = Cursor::new(epub_bytes);
        let mut archive = zip::ZipArchive::new(reader)
            .map_err(|e| anyhow::anyhow!("failed to read generated epub: {e}"))?;

        // 读取所有条目到内存
        let mut entries = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| anyhow::anyhow!("zip entry read error: {e}"))?;
            let name = entry.name().to_string();
            let compression = entry.compression();
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            entries.push((name, compression, data));
        }
        drop(archive);

        // 重写 zip，在 content.opf 和 toc.ncx 中将 urn:uuid:xxx 替换为原始 book_id，
        // 并在 toc.ncx 中补充 dtb:uid meta。
        // 同时移除 com.apple.ibooks.display-options.xml，该文件声明 specified-fonts=true
        // 导致 Calibre/Kindle 转换时误认为 EPUB 内嵌了字体，回退到英文默认字体。
        let dtb_uid_meta = format!("<meta name=\"dtb:uid\" content=\"{}\" />", self.book_id);
        let mut out = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut out);
            for (name, compression, data) in entries {
                // 跳过 iBooks display-options 文件，避免 Kindle 字体识别问题
                if name.contains("com.apple.ibooks.display-options") {
                    continue;
                }
                let needs_fixup = name.ends_with("content.opf") || name.ends_with("toc.ncx");
                let options = zip::write::FileOptions::default().compression_method(compression);
                writer
                    .start_file(&name, options)
                    .map_err(|e| anyhow::anyhow!("zip write error: {e}"))?;

                if needs_fixup {
                    match String::from_utf8(data) {
                        Ok(text) => {
                            let mut fixed = text.replace(&urn_str, &self.book_id);
                            // 在 toc.ncx 中补充 dtb:uid（epub-builder 模板未包含此项）
                            if name.ends_with("toc.ncx") && !fixed.contains("dtb:uid") {
                                fixed = fixed.replace(
                                    "<meta name=\"dtb:depth\"",
                                    &format!("{}\n    <meta name=\"dtb:depth\"", dtb_uid_meta),
                                );
                            }
                            writer.write_all(fixed.as_bytes())?;
                        }
                        Err(e) => {
                            writer.write_all(&e.into_bytes())?;
                        }
                    }
                } else {
                    writer.write_all(&data)?;
                }
            }
            writer
                .finish()
                .map_err(|e| anyhow::anyhow!("zip finish error: {e}"))?;
        }

        Ok(out.into_inner())
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
    let escaped_title = html_escape(title);
    format!(
        "<?xml version='1.0' encoding='utf-8'?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" epub:prefix=\"z3998: http://www.daisy.org/z3998/2012/vocab/structure/#\" lang=\"zh\" xml:lang=\"zh\">\n  <head>\n    <title>{}</title>\n    <link href=\"stylesheet.css\" rel=\"stylesheet\" type=\"text/css\"/>\n  </head>\n  <body><h1>{}</h1>\n{}\n  </body>\n</html>",
        escaped_title, escaped_title, body
    )
}

fn title_from_file_or_html(file_name: &str, html: &str) -> String {
    if let Some(start) = html.find("<title>")
        && let Some(end) = html[start + 7..].find("</title>")
    {
        let raw = &html[start + 7..start + 7 + end];
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    file_name.to_string()
}
