//! 下载计划准备与元数据搜索。
//!
//! 负责从官方 API 或 Web 端拉取目录、章节列表，合并元数据，生成 `DownloadPlan`。

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use tracing::{info, warn};

use crate::base_system::book_paths;
use crate::base_system::context::Config;
use crate::base_system::json_extract;
use crate::network_parser::network::{FanqieWebConfig, FanqieWebNetwork};

use super::models::{
    BookMeta, ChapterRange, ChapterRef, DownloadPlan, drop_tag_equals_category, merge_meta,
    merge_meta_prefer_hint_name, merge_tag_lists,
};
use super::third_party::resolve_api_urls;

#[cfg(feature = "official-api")]
use tomato_novel_official_api::{DirectoryClient, SearchClient};

// ── 下载计划准备（官方 API 版本）──────────────────────────────────

/// 预先拉取目录与元数据，便于 UI 展示预览/范围选择。
#[cfg(feature = "official-api")]
pub fn prepare_download_plan(
    config: &Config,
    book_id: &str,
    meta_hint: BookMeta,
) -> Result<DownloadPlan> {
    info!(target: "download", book_id, "准备下载计划");
    let directory = DirectoryClient::new().context("init DirectoryClient")?;
    let (dir_url, _content_urls) = resolve_api_urls(config)?;
    let api_url = dir_url.as_deref();

    // 并行回退：预先尝试 Web 目录/简介（失败不影响主流程）
    let web_plan = prepare_download_plan_web(config, book_id, meta_hint.clone()).ok();

    // 首次获取目录和元数据。
    let mut dir = match directory.fetch_directory_with_cover(book_id, api_url, None) {
        Ok(d) => d,
        Err(e) => {
            warn!(
                target: "download",
                book_id,
                error = %e,
                "官方 API 获取目录失败，尝试使用 web 回退"
            );
            if let Some(plan) = web_plan {
                return Ok(plan);
            }
            return Err(anyhow!(e).context(format!("fetch directory for book_id={book_id}")));
        }
    };

    if dir.chapters.is_empty() {
        warn!(
            target: "download",
            book_id,
            "官方 API 目录为空，尝试使用 web 回退"
        );
        if let Some(plan) = web_plan {
            return Ok(plan);
        }
        return Err(anyhow!("目录为空"));
    }

    let meta_from_dir: BookMeta = dir.meta.clone().into();
    // For book_name: Prefer the hint (what user saw in search) to maintain consistency
    // For other metadata: Prefer authoritative directory metadata
    let merged = merge_meta_prefer_hint_name(meta_from_dir, meta_hint);
    let mut completed_meta =
        if merged.book_name.is_some() && merged.author.is_some() && merged.description.is_some() {
            merged
        } else {
            merge_meta(merged, search_metadata(book_id).unwrap_or_default())
        };

    if let Some(web_plan) = web_plan.as_ref() {
        completed_meta = merge_meta(completed_meta, web_plan.meta.clone());
        completed_meta.tags = merge_tag_lists(&completed_meta.tags, &web_plan.meta.tags);
        completed_meta.tags =
            drop_tag_equals_category(&completed_meta.tags, &completed_meta.category);
        // 连载状态优先使用 web 页面解析结果（更稳定）。
        // 若 web 未提供，再保留官方侧推断。
        completed_meta.finished = web_plan.meta.finished.or(completed_meta.finished);
    }

    // 应用用户配置的书名字段偏好（移到前面，在下载封面之前）
    if let Some(preferred_name) = config.pick_preferred_book_name(&completed_meta) {
        completed_meta.book_name = Some(preferred_name);
    }

    // 直接使用已有的 cover_url 下载封面，避免冗余的第二次 API 调用（该调用
    // 容易因速率限制/冷却而间歇性失败，导致封面概率丢失）。
    if completed_meta.cover_url.is_some() || completed_meta.detail_cover_url.is_some() {
        let cover_dir =
            book_paths::book_folder_path(config, book_id, completed_meta.book_name.as_deref());
        download_cover_direct(config, book_id, &completed_meta, &cover_dir);
    }

    if let Some(web_plan) = web_plan.as_ref() {
        dir.chapters = merge_chapters_with_web(dir.chapters, &web_plan.chapters);
    }

    Ok(DownloadPlan {
        book_id: dir.book_id.clone(),
        meta: completed_meta,
        chapters: dir.chapters,
        _raw: dir.raw,
    })
}

// ── 下载计划准备（非 official-api 版本）──────────────────────────

/// no-official-api：使用 FanqieWebNetwork 拉目录 + 拉书本信息。
#[cfg(not(feature = "official-api"))]
pub fn prepare_download_plan(
    config: &Config,
    book_id: &str,
    meta_hint: BookMeta,
) -> Result<DownloadPlan> {
    info!(target: "download", book_id, "准备下载计划（no-official）");
    prepare_download_plan_web(config, book_id, meta_hint)
}

// ── Web 端回退 ──────────────────────────────────────────────────

fn parse_chapter_ref_from_value(v: &Value) -> Option<ChapterRef> {
    let maps = json_extract::collect_maps(v);
    let id = maps.iter().find_map(|m| {
        json_extract::pick_string(
            m,
            &[
                "item_id",
                "itemId",
                "chapter_id",
                "chapterId",
                "catalog_id",
                "catalogId",
                "id",
            ],
        )
    })?;
    let title = maps
        .iter()
        .find_map(|m| {
            json_extract::pick_string(
                m,
                &[
                    "title",
                    "chapter_title",
                    "chapterTitle",
                    "name",
                    "chapter_name",
                ],
            )
        })
        .unwrap_or_else(|| id.clone());
    Some(ChapterRef { id, title })
}

/// 使用 FanqieWebNetwork 拉目录 + 拉书本信息（可作为官方 API 的回退路径）。
fn prepare_download_plan_web(
    config: &Config,
    book_id: &str,
    meta_hint: BookMeta,
) -> Result<DownloadPlan> {
    info!(target: "download", book_id, "准备下载计划（web fallback）");

    let web_cfg = FanqieWebConfig {
        request_timeout: Duration::from_secs(config.request_timeout.max(1)),
        max_retries: config.max_retries.max(1) as usize,
        ..Default::default()
    };
    let web = FanqieWebNetwork::new(web_cfg).context("init FanqieWebNetwork")?;

    let chapter_values = web
        .fetch_chapter_list(book_id)
        .ok_or_else(|| anyhow!("获取章节列表失败"))?;
    if chapter_values.is_empty() {
        return Err(anyhow!("目录为空"));
    }

    let mut chapters: Vec<ChapterRef> = chapter_values
        .iter()
        .filter_map(parse_chapter_ref_from_value)
        .collect();
    // 保底：如果解析失败导致为空，至少让用户得到一个明确错误
    if chapters.is_empty() {
        return Err(anyhow!("解析章节列表失败（未能提取 item_id/title）"));
    }

    let (
        book_name,
        author,
        description,
        tags_opt,
        cover_url,
        detail_cover_url,
        _html_img_cover_url,
        chapter_count,
        finished,
    ) = web.get_book_info(book_id);
    let web_meta = BookMeta {
        book_name,
        author,
        description,
        tags: tags_opt.unwrap_or_default(),
        cover_url,
        detail_cover_url,
        chapter_count,
        finished,
        ..BookMeta::default()
    };

    // 对齐官方逻辑：优先保持用户"看到的书名"（hint），其余字段尽量用 web 拉到的
    let mut completed_meta = merge_meta_prefer_hint_name(web_meta, meta_hint);
    if let Some(preferred_name) = config.pick_preferred_book_name(&completed_meta) {
        completed_meta.book_name = Some(preferred_name);
    }

    // no-official：目前不做封面下载（cover_url 往往缺失），避免额外请求与逻辑分叉

    let raw = serde_json::json!({
        "book_id": book_id,
        "chapters": chapter_values,
        "source": "fanqie_web",
    });

    // 章节顺序：web 接口一般已经是正确顺序；保险起见保持原顺序即可
    Ok(DownloadPlan {
        book_id: book_id.to_string(),
        meta: completed_meta,
        chapters: std::mem::take(&mut chapters),
        _raw: raw,
    })
}

// ── 章节合并 ──────────────────────────────────────────────────

pub(crate) fn merge_chapters_with_web(
    official: Vec<ChapterRef>,
    web: &[ChapterRef],
) -> Vec<ChapterRef> {
    if official.is_empty() {
        return web.to_vec();
    }
    if web.is_empty() {
        return official;
    }

    let mut web_title_map = HashMap::new();
    for ch in web {
        if !ch.id.trim().is_empty() {
            web_title_map.insert(ch.id.clone(), ch.title.clone());
        }
    }

    let mut seen = HashSet::new();
    let mut merged = Vec::with_capacity(official.len() + web.len().saturating_sub(official.len()));

    for mut ch in official {
        if !seen.insert(ch.id.clone()) {
            warn!(target: "download", id = %ch.id, title = %ch.title, "跳过官方源中的重复章节");
            continue;
        }
        if ch.title.trim().is_empty()
            && let Some(title) = web_title_map.get(&ch.id)
        {
            ch.title = title.clone();
        }
        merged.push(ch);
    }

    for ch in web {
        if !seen.contains(&ch.id) {
            merged.push(ch.clone());
        }
    }

    merged
}

// ── 搜索补元数据 ──────────────────────────────────────────────────

#[cfg(feature = "official-api")]
fn search_metadata(book_id: &str) -> Option<BookMeta> {
    let client = SearchClient::new().ok()?;
    let resp = client.search_books(book_id).ok()?;
    let book = resp.books.into_iter().find(|b| b.book_id == book_id)?;
    let maps = json_extract::collect_maps(&book.raw);

    let description = maps.iter().find_map(|m| {
        json_extract::pick_string(
            m,
            &[
                "description",
                "desc",
                "abstract",
                "intro",
                "summary",
                "book_abstract",
                "recommendation_reason",
            ],
        )
    });
    let tags = maps
        .iter()
        .find_map(|m| json_extract::pick_tags_opt(m))
        .unwrap_or_default();
    let cover_url = maps.iter().find_map(|m| json_extract::pick_cover(m));
    let detail_cover_url = maps.iter().find_map(|m| json_extract::pick_detail_cover(m));
    let finished = maps.iter().find_map(|m| json_extract::pick_finished(m));
    let chapter_count = maps
        .iter()
        .find_map(|m| json_extract::pick_chapter_count(m));
    let word_count = maps.iter().find_map(|m| json_extract::pick_word_count(m));
    let score = maps.iter().find_map(|m| json_extract::pick_score(m));
    let read_count = maps.iter().find_map(|m| json_extract::pick_read_count(m));
    let read_count_text = maps
        .iter()
        .find_map(|m| json_extract::pick_read_count_text(m));
    let book_short_name = maps
        .iter()
        .find_map(|m| json_extract::pick_book_short_name(m));
    let original_book_name = maps
        .iter()
        .find_map(|m| json_extract::pick_original_book_name(m));
    let first_chapter_title = maps
        .iter()
        .find_map(|m| json_extract::pick_first_chapter_title(m));
    let last_chapter_title = maps
        .iter()
        .find_map(|m| json_extract::pick_last_chapter_title(m));
    let category = maps.iter().find_map(|m| json_extract::pick_category(m));
    let cover_primary_color = maps
        .iter()
        .find_map(|m| json_extract::pick_cover_primary_color(m));

    Some(BookMeta {
        book_name: book.title,
        author: book.author,
        description,
        tags,
        cover_url,
        detail_cover_url,
        finished,
        chapter_count,
        word_count,
        score,
        read_count,
        read_count_text,
        book_short_name,
        original_book_name,
        first_chapter_title,
        last_chapter_title,
        category,
        cover_primary_color,
    })
}

#[cfg(not(feature = "official-api"))]
fn search_metadata(_book_id: &str) -> Option<BookMeta> {
    None
}

// ── 范围过滤 ──────────────────────────────────────────────────

pub(crate) fn apply_range(chapters: &[ChapterRef], range: Option<ChapterRange>) -> Vec<ChapterRef> {
    let total = chapters.len();
    match range {
        None => chapters.to_vec(),
        Some(r) => {
            if r.start == 0 || r.start > r.end {
                return Vec::new();
            }
            let start_idx = r.start.saturating_sub(1);
            let end_idx = r.end.min(total).saturating_sub(1);
            if start_idx >= chapters.len() {
                return Vec::new();
            }
            chapters
                .iter()
                .skip(start_idx)
                .take(end_idx.saturating_sub(start_idx) + 1)
                .cloned()
                .collect()
        }
    }
}

// ── 封面直接下载 ────────────────────────────────────────────

/// 直接使用已知的封面 URL 下载封面到目标目录，带超时和重试。
/// 避免通过重新调用 `fetch_directory_with_cover` 下载（该方式是多余的完整 API
/// 调用，容易因速率限制/冷却导致间歇性失败）。
///
/// 使用 Downloader 侧的 `safe_fs_name` 命名，确保与 EPUB 生成器查找逻辑一致。
#[cfg(feature = "official-api")]
fn download_cover_direct(
    config: &Config,
    book_id: &str,
    meta: &BookMeta,
    cover_dir: &std::path::Path,
) {
    use crate::base_system::context::safe_fs_name;

    let book_name = meta.book_name.as_deref().unwrap_or("cover");
    let safe_name = safe_fs_name(book_name, "_", 120);

    // 检查是否已有封面文件（任何受支持的扩展名）
    let extensions = ["jpg", "jpeg", "png", "webp"];
    for ext in &extensions {
        let existing = cover_dir.join(format!("{safe_name}.{ext}"));
        if existing.exists() {
            info!(target: "download", book_id, path = %existing.display(), "封面文件已存在，跳过下载");
            return;
        }
    }

    let _ = std::fs::create_dir_all(cover_dir);
    let timeout = Duration::from_millis(10_000);
    let max_retries = 3u32;

    // 优先尝试 detail_cover_url，其次 cover_url
    let urls: Vec<&str> = [meta.detail_cover_url.as_deref(), meta.cover_url.as_deref()]
        .into_iter()
        .flatten()
        .filter(|u| {
            let t = u.trim();
            !t.is_empty() && (t.starts_with("http://") || t.starts_with("https://"))
        })
        .collect();

    for url in &urls {
        for attempt in 0..max_retries {
            if attempt > 0 {
                let base_ms = 300u64 * (1u64 << attempt.min(3));
                std::thread::sleep(Duration::from_millis(base_ms));
            }

            let bytes = match crate::third_party::media_fetch::fetch_bytes(url, timeout) {
                Some(b) if !b.is_empty() => b,
                _ => {
                    warn!(
                        target: "download",
                        book_id,
                        url,
                        attempt = attempt + 1,
                        max_retries,
                        "封面下载失败，重试中"
                    );
                    continue;
                }
            };

            // 通过 magic bytes 嗅探实际格式（不依赖 URL 扩展名，后者不可靠）
            let is_heic = bytes.len() >= 12
                && &bytes[4..8] == b"ftyp"
                && matches!(&bytes[8..12], b"heic" | b"heix" | b"mif1" | b"msf1");

            if is_heic {
                info!(target: "download", book_id, "封面为 HEIC 格式，尝试从 web 页面获取 JPEG 封面替换");
                // HEIC 无法直接在 EPUB 中使用，尝试 web 页面获取 JPEG 替代
                if let Some(jpeg_bytes) = fetch_web_cover_jpeg(config, book_id) {
                    let path = cover_dir.join(format!("{safe_name}.jpg"));
                    if std::fs::write(&path, &jpeg_bytes).is_ok() {
                        info!(target: "download", book_id, path = %path.display(), "HEIC 封面已替换为 web 页面 JPEG");
                        return;
                    }
                }
                // web 页面也失败，保存原始 HEIC（作为兜底）
                let path = cover_dir.join(format!("{safe_name}.heic"));
                let _ = std::fs::write(&path, &bytes);
                warn!(target: "download", book_id, "所有 JPEG 替换尝试失败，保留 HEIC 封面");
                return;
            }

            // 非 HEIC：根据嗅探结果确定扩展名
            let ext = if bytes.len() >= 8 && bytes[0] == 0x89 && &bytes[1..4] == b"PNG" {
                "png"
            } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
                "webp"
            } else {
                // JPEG 或无法识别，统一用 jpg
                "jpg"
            };

            let path = cover_dir.join(format!("{safe_name}.{ext}"));
            if std::fs::write(&path, &bytes).is_ok() {
                info!(target: "download", book_id, path = %path.display(), "封面下载成功");
                return;
            }
        }
    }

    warn!(target: "download", book_id, "所有封面 URL 均下载失败");
}

/// 从 web 页面 `<img class="book-cover-img">` 获取浏览器兼容格式（通常 JPEG）的封面字节。
#[cfg(feature = "official-api")]
fn fetch_web_cover_jpeg(config: &Config, book_id: &str) -> Option<Vec<u8>> {
    let web_cfg = FanqieWebConfig {
        request_timeout: Duration::from_secs(config.request_timeout.max(1)),
        max_retries: 2,
        ..Default::default()
    };
    let web = FanqieWebNetwork::new(web_cfg).ok()?;
    let (_, _, _, _, _, _, html_img_cover_url, _, _) = web.get_book_info(book_id);
    let img_url = html_img_cover_url?;
    if img_url.trim().is_empty() {
        return None;
    }

    let bytes =
        crate::third_party::media_fetch::fetch_bytes(&img_url, Duration::from_millis(10_000))?;

    // 确认下载到的确实不是 HEIC
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"heic" || brand == b"heix" || brand == b"mif1" || brand == b"msf1" {
            warn!(target: "download", book_id, "web 页面封面仍为 HEIC，放弃替换");
            return None;
        }
    }

    Some(bytes)
}
