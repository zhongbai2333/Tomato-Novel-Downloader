//! 段评渲染与媒体预取。
//!
//! 包含段落评论的缓存加载、评论媒体（头像/图片）预取、评论页 XHTML 渲染。

#[cfg(feature = "official-api")]
use std::collections::HashSet;
#[cfg(feature = "official-api")]
use std::fs;
#[cfg(feature = "official-api")]
use std::path::Path;

#[cfg(feature = "official-api")]
use crossbeam_channel as channel;
#[cfg(feature = "official-api")]
use tracing::debug;
use tracing::info;

#[cfg(feature = "official-api")]
use super::book_manager::BookManager;
#[cfg(feature = "official-api")]
use super::epub_generator::EpubGenerator;
#[cfg(feature = "official-api")]
use super::html_utils::escape_html;
#[cfg(feature = "official-api")]
use super::image_utils::{ensure_cached_image, sha1_hex};
#[cfg(feature = "official-api")]
use super::segment_shared::SegmentCommentsChapterCache;
#[cfg(feature = "official-api")]
use super::segment_utils;

// ── 缓存加载 ────────────────────────────────────────────────────

#[cfg(feature = "official-api")]
pub(crate) fn load_segment_comments_cache(
    manager: &BookManager,
    chapter_id: &str,
) -> Option<SegmentCommentsChapterCache> {
    let path = manager
        .book_folder()
        .join("segment_comments")
        .join(format!("{}.json", chapter_id));
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice::<SegmentCommentsChapterCache>(&bytes).ok()
}

// ── 评论媒体预取 ────────────────────────────────────────────────

#[cfg(feature = "official-api")]
pub(crate) fn prefetch_comment_media(
    cfg: &crate::base_system::context::Config,
    per_para: &[(i32, tomato_novel_official_api::ReviewResponse)],
    images_dir: &Path,
) {
    if !(cfg.download_comment_images || cfg.download_comment_avatars) {
        return;
    }

    let mut urls: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (_para_idx, resp) in per_para {
        for item in &resp.reviews {
            if cfg.download_comment_avatars
                && let Some(url) = item.user.avatar.as_deref()
            {
                let u = url.trim();
                if !u.is_empty() && seen.insert(u.to_string()) {
                    urls.push(u.to_string());
                }
            }
            if cfg.download_comment_images {
                for img in &item.images {
                    let u = img.url.trim();
                    if !u.is_empty() && seen.insert(u.to_string()) {
                        urls.push(u.to_string());
                    }
                }
            }
        }
    }

    if urls.is_empty() {
        return;
    }

    // Respect per-chapter cap (0 means no cap).
    if cfg.media_limit_per_chapter > 0 {
        urls.truncate(cfg.media_limit_per_chapter);
    }

    let workers = cfg.media_download_workers.clamp(1, 64);
    let worker_count = workers.min(urls.len().max(1));
    if worker_count <= 1 {
        for u in urls {
            let _ = ensure_cached_image(cfg, &u, images_dir);
        }
        return;
    }

    let (tx, rx) = channel::unbounded::<String>();
    for u in urls {
        let _ = tx.send(u);
    }
    drop(tx);

    let mut handles = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let rx = rx.clone();
        let images_dir = images_dir.to_path_buf();
        let cfg = cfg.clone();
        handles.push(std::thread::spawn(move || {
            for u in rx.iter() {
                let _ = ensure_cached_image(&cfg, &u, &images_dir);
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
}

// ── 段评页面渲染 ────────────────────────────────────────────────

#[cfg(feature = "official-api")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_segment_comment_page(
    chapter_title: &str,
    chapter_file: &str,
    chapter_html: &str,
    per_para: &[(i32, tomato_novel_official_api::ReviewResponse)],
    cfg: &crate::base_system::context::Config,
    resources_added: &mut HashSet<String>,
    images_dir: &Path,
    epub: &mut EpubGenerator,
) -> anyhow::Result<String> {
    let mut avatar_used = 0usize;
    let mut image_used = 0usize;

    let mut html = String::new();
    html.push_str(&format!("<h2>{} - 段评</h2>", escape_html(chapter_title)));

    for (para_idx, resp) in per_para {
        let idx_usize = (*para_idx).max(0) as usize;
        let snippet = resp
            .meta
            .para_content
            .as_deref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| segment_utils::extract_para_snippet(chapter_html, idx_usize));
        let snippet = segment_utils::convert_bracket_emojis(&snippet);

        let disp_idx = idx_usize + 1;
        let cjk_idx = segment_utils::to_cjk_numeral(disp_idx as i32);
        let title_html = if !snippet.trim().is_empty() {
            format!(
                "<span class=\"para-title\"><span class=\"para-index\">{}、</span> <span class=\"para-src\">&quot;{}&quot;</span></span>",
                escape_html(&cjk_idx),
                escape_html(snippet.trim())
            )
        } else {
            format!(
                "<span class=\"para-title\">第 {} 段</span>",
                escape_html(&disp_idx.to_string())
            )
        };
        html.push_str(&format!(
            "<h3 id=\"para-{}\">{}</h3>",
            idx_usize, title_html
        ));
        html.push_str(&format!(
            "<div class=\"back-to-chapter\"><a href=\"{}#p-{}\">↩ 回到正文</a></div>",
            escape_html(chapter_file),
            idx_usize
        ));

        html.push_str("<ol>");
        for item in &resp.reviews {
            let user = item
                .user
                .name
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("匿名");
            let text = segment_utils::convert_bracket_emojis(&item.text);
            if text.trim().is_empty() {
                continue;
            }
            let text = escape_html(text.trim());

            let mut avatar_html = String::new();
            if cfg.download_comment_avatars
                && let Some(url) = item.user.avatar.as_deref()
            {
                if let Ok(Some((path, mime, ext))) = ensure_cached_image(cfg, url, images_dir) {
                    let hash = sha1_hex(url);
                    let resource_path = format!("images/{}{}", hash, ext);
                    if !resources_added.contains(&resource_path)
                        && let Ok(bytes) = fs::read(&path)
                        && epub.add_resource_bytes(&resource_path, bytes, mime).is_ok()
                    {
                        resources_added.insert(resource_path.clone());
                    }
                    if resources_added.contains(&resource_path) {
                        avatar_html = format!(
                            "<img class=\"avatar\" alt=\"\" src=\"{}\"/>",
                            escape_html(&resource_path)
                        );
                        avatar_used += 1;
                    } else {
                        debug!(target: "segment", url = %url, "avatar not added to epub resources (read/add_resource failed)");
                    }
                } else {
                    debug!(target: "segment", url = %url, "avatar ensure_cached_image failed/empty");
                }
            }

            let mut images_html = String::new();
            if cfg.download_comment_images {
                let mut imgs = Vec::new();
                for img in &item.images {
                    let url = img.url.trim();
                    if url.is_empty() {
                        continue;
                    }
                    if let Ok(Some((path, mime, ext))) = ensure_cached_image(cfg, url, images_dir) {
                        let hash = sha1_hex(url);
                        let resource_path = format!("images/{}{}", hash, ext);
                        if !resources_added.contains(&resource_path)
                            && let Ok(bytes) = fs::read(&path)
                            && epub.add_resource_bytes(&resource_path, bytes, mime).is_ok()
                        {
                            resources_added.insert(resource_path.clone());
                        }
                        if resources_added.contains(&resource_path) {
                            imgs.push(format!(
                                "<img alt=\"\" src=\"{}\"/>",
                                escape_html(&resource_path)
                            ));
                            image_used += 1;
                        } else {
                            debug!(target: "segment", url = %url, "comment image not added to epub resources (read/add_resource failed)");
                        }
                    } else {
                        debug!(target: "segment", url = %url, "comment image ensure_cached_image failed/empty");
                    }
                }
                if !imgs.is_empty() {
                    images_html = format!("<div class=\"seg-images\">{}</div>", imgs.join(""));
                }
            }

            let mut meta_line = String::new();
            meta_line.push_str("<small class=\"seg-meta\">");
            meta_line.push_str(&avatar_html);
            meta_line.push_str(&format!("作者：{}", escape_html(user)));

            if let Some(ts) = item.created_ts {
                let mut t = ts;
                if t > 1_000_000_000_000 {
                    t /= 1000;
                }
                if t > 0 {
                    meta_line.push_str(&format!(" | 时间：{}", escape_html(&t.to_string())));
                }
            }
            meta_line.push_str(&format!(" | 赞：{}", item.digg_count));
            meta_line.push_str("</small>");

            html.push_str("<li class=\"seg-item\">");
            html.push_str(&format!("<p>{}</p>", text));
            if !images_html.is_empty() {
                html.push_str(&images_html);
            }
            html.push_str(&format!("<p>{}</p>", meta_line));
            html.push_str("</li>");
        }
        html.push_str("</ol>");
    }

    let top_n_cfg = cfg.segment_comments_top_n.max(1);
    html.push_str(&format!(
        "<p><small>仅展示每段前 {} 条评论（若有），实际总数以接口为准。</small></p>",
        top_n_cfg
    ));

    info!(
        target: "segment",
        chapter = %chapter_title,
        para_groups = per_para.len(),
        avatar_used,
        image_used,
        download_comment_avatars = cfg.download_comment_avatars,
        download_comment_images = cfg.download_comment_images,
        "segment comment page rendered"
    );

    Ok(html)
}
