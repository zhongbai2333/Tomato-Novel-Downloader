use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use std::path::{Path as FsPath, PathBuf};
use tracing::{debug, info, warn};

use crate::base_system::book_id::resolve_book_id;
use crate::base_system::book_paths::book_folder_path;
use crate::base_system::context::safe_fs_name;
use crate::base_system::file_cleaner::is_empty_dir;
use crate::book_parser::image_utils::ensure_cached_image;
use crate::download::downloader as dl;
use crate::network_parser::network::{FanqieWebConfig, FanqieWebNetwork};
use crate::ui::web::state::AppState;

fn preview_cover_cache_dir() -> PathBuf {
    std::env::temp_dir()
        .join("tomato-novel-downloader")
        .join("webui_preview_cover")
}

fn parse_cover_key(path: &FsPath) -> Option<String> {
    let stem = path.file_stem()?.to_str()?.trim().to_string();
    if stem.len() != 40 {
        return None;
    }
    if !stem.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(stem)
}

fn resolve_local_preview_cover_key(
    cfg: &crate::base_system::context::Config,
    meta: &dl::BookMeta,
) -> Option<String> {
    let mut cfg = cfg.clone();
    cfg.force_convert_images_to_jpeg = true;
    cfg.jpeg_retry_convert = true;
    cfg.convert_heic_to_jpeg = true;
    cfg.keep_heic_original = false;

    let cache_dir = preview_cover_cache_dir();
    let candidates = [meta.detail_cover_url.as_deref(), meta.cover_url.as_deref()];

    for url in candidates.into_iter().flatten() {
        let u = url.trim();
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            continue;
        }

        match ensure_cached_image(&cfg, u, &cache_dir) {
            Ok(Some((path, mime, ext))) => {
                let is_jpeg = mime.eq_ignore_ascii_case("image/jpeg")
                    || ext.eq_ignore_ascii_case(".jpg")
                    || ext.eq_ignore_ascii_case(".jpeg");
                if !is_jpeg {
                    debug!(url = u, mime, ext, "封面非 JPEG 格式，跳过");
                    continue;
                }
                if let Some(key) = parse_cover_key(&path) {
                    debug!(url = u, key = %key, "成功缓存封面 JPEG");
                    return Some(key);
                }
            }
            Ok(None) => {
                debug!(url = u, "封面下载/转码返回 None");
            }
            Err(e) => {
                warn!(url = u, error = %e, "封面下载失败");
            }
        }
    }

    warn!(
        cover_url = ?meta.cover_url,
        detail_cover_url = ?meta.detail_cover_url,
        "所有封面候选 URL 均获取失败"
    );
    None
}

fn load_preview_cover_jpeg(key: &str) -> Option<Vec<u8>> {
    if key.len() != 40 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let dir = preview_cover_cache_dir();
    let p_jpeg = dir.join(format!("{key}.jpeg"));
    let p_jpg = dir.join(format!("{key}.jpg"));

    if let Ok(bytes) = std::fs::read(&p_jpeg) {
        return Some(bytes);
    }
    if let Ok(bytes) = std::fs::read(&p_jpg) {
        return Some(bytes);
    }

    None
}

fn resolve_local_preview_cover_key_with_web_fallback(
    cfg: &crate::base_system::context::Config,
    book_id: &str,
    meta: &dl::BookMeta,
) -> Option<String> {
    // 第一轮：使用 prepare_download_plan 返回的元数据中的封面 URL
    if let Some(k) = resolve_local_preview_cover_key(cfg, meta) {
        return Some(k);
    }

    // 第二轮：通过 web 页面抓取封面 URL（不同来源，URL 可能不同）
    debug!(book_id, "官方 API 封面获取失败，尝试 web 页面抓取封面 URL");
    let web = match FanqieWebNetwork::new(FanqieWebConfig::default()) {
        Ok(w) => w,
        Err(e) => {
            warn!(error = %e, "初始化 FanqieWebNetwork 失败");
            return None;
        }
    };
    let (_, _, _, _, cover_url, detail_cover_url, html_img_cover_url, _, _) =
        web.get_book_info(book_id);
    debug!(
        book_id,
        ?cover_url,
        ?detail_cover_url,
        ?html_img_cover_url,
        "web 页面抓取封面 URL 结果"
    );

    if cover_url.is_none() && detail_cover_url.is_none() && html_img_cover_url.is_none() {
        warn!(book_id, "web 页面也未找到封面 URL");
        return None;
    }

    // 优先使用 HTML <img class="book-cover-img"> 的 src（浏览器兼容格式，非 HEIC）
    let web_meta = dl::BookMeta {
        cover_url: detail_cover_url.or(cover_url),
        detail_cover_url: html_img_cover_url,
        ..Default::default()
    };
    resolve_local_preview_cover_key(cfg, &web_meta)
}

pub(crate) async fn api_preview(
    State(state): State<AppState>,
    Path(book_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let book_id = tokio::task::spawn_blocking(move || resolve_book_id(&book_id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::BAD_REQUEST)?;
    if book_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // 并发限制：与 search 共用同一个信号量，最多 2 个上游 API 请求并发。
    #[cfg(feature = "official-api")]
    let _permit = state
        .api_semaphore
        .acquire()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let cfg_for_plan = cfg.clone();
    let book_id_for_plan = book_id.clone();

    let plan = tokio::task::spawn_blocking(move || {
        dl::prepare_download_plan(&cfg_for_plan, &book_id_for_plan, dl::BookMeta::default())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let meta = &plan.meta;
    let chapter_count = plan.chapters.len();

    let cfg_for_cover = cfg.clone();
    let meta_for_cover = meta.clone();
    let book_id_for_cover = book_id.clone();
    let cover_key = tokio::task::spawn_blocking(move || {
        resolve_local_preview_cover_key_with_web_fallback(
            &cfg_for_cover,
            &book_id_for_cover,
            &meta_for_cover,
        )
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let local_cover_url = cover_key
        .map(|k| format!("/api/preview-cover/{k}"))
        .or_else(|| Some(format!("/api/preview-cover-by-book/{book_id}")));

    Ok(Json(json!({
        "book_id": book_id,
        "book_name": meta.book_name,
        "original_book_name": meta.original_book_name,
        "author": meta.author,
        "description": meta.description,
        "tags": meta.tags,
        "cover_url": local_cover_url,
        "detail_cover_url": local_cover_url,
        "finished": meta.finished,
        "chapter_count": chapter_count,
        "word_count": meta.word_count,
        "score": meta.score,
        "read_count": meta.read_count,
        "read_count_text": meta.read_count_text,
        "category": meta.category,
        "first_chapter_title": meta.first_chapter_title,
        "last_chapter_title": meta.last_chapter_title,
    })))
}

pub(crate) async fn api_preview_cover(Path(key): Path<String>) -> Result<Response, StatusCode> {
    let bytes = tokio::task::spawn_blocking(move || load_preview_cover_jpeg(&key))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let mut resp = Response::new(Body::from(bytes));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    resp.headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    resp.headers_mut()
        .insert(header::EXPIRES, HeaderValue::from_static("0"));
    Ok(resp)
}

pub(crate) async fn api_preview_cover_by_book(
    State(state): State<AppState>,
    Path(book_id): Path<String>,
) -> Result<Response, StatusCode> {
    let book_id = tokio::task::spawn_blocking(move || resolve_book_id(&book_id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::BAD_REQUEST)?;
    if book_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    let cfg_for_plan = cfg.clone();
    let book_id_for_plan = book_id.clone();
    let plan = tokio::task::spawn_blocking(move || {
        dl::prepare_download_plan(&cfg_for_plan, &book_id_for_plan, dl::BookMeta::default())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let meta_for_cover = plan.meta.clone();
    let book_id_for_cover = book_id.clone();
    let key = tokio::task::spawn_blocking(move || {
        resolve_local_preview_cover_key_with_web_fallback(&cfg, &book_id_for_cover, &meta_for_cover)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let bytes = if let Some(key) = key {
        tokio::task::spawn_blocking(move || load_preview_cover_jpeg(&key))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or_else(|| {
                warn!(book_id = %book_id, "封面缓存文件读取失败");
                StatusCode::NOT_FOUND
            })?
    } else {
        warn!(book_id = %book_id, "无法获取封面（官方 API 和 web 页面均未找到可用封面 URL）");
        return Err(StatusCode::NOT_FOUND);
    };

    let mut resp = Response::new(Body::from(bytes));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    resp.headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    resp.headers_mut()
        .insert(header::EXPIRES, HeaderValue::from_static("0"));
    Ok(resp)
}

/// 清理预览产生的封面文件（仅当文件夹只含封面、且无 status.json 时删除）。
/// 移植自 TUI 的 `cleanup_preview_cover_artifacts`。
pub(crate) async fn api_preview_cleanup(
    State(state): State<AppState>,
    Path(book_id): Path<String>,
) -> StatusCode {
    let book_id = tokio::task::spawn_blocking(move || resolve_book_id(&book_id))
        .await
        .unwrap_or(None)
        .unwrap_or_default();
    if book_id.is_empty() {
        return StatusCode::BAD_REQUEST;
    }

    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    if !cfg.auto_clear_dump {
        debug!(book_id = %book_id, "auto_clear_dump 未开启，跳过预览清理");
        return StatusCode::NO_CONTENT;
    }

    // 需要获取 book_name 以定位文件夹，调用 prepare_download_plan 一次
    let cfg_for_plan = cfg.clone();
    let bid = book_id.clone();
    let plan_result = tokio::task::spawn_blocking(move || {
        dl::prepare_download_plan(&cfg_for_plan, &bid, dl::BookMeta::default())
    })
    .await;

    let plan = match plan_result {
        Ok(Ok(p)) => p,
        _ => {
            // plan 获取失败，无法定位文件夹，静默返回
            debug!(book_id = %book_id, "cleanup: 无法获取下载计划，跳过");
            return StatusCode::NO_CONTENT;
        }
    };

    let book_name = match plan.meta.book_name.as_deref() {
        Some(n) => n.to_string(),
        None => {
            debug!(book_id = %book_id, "cleanup: plan 中无 book_name，跳过");
            return StatusCode::NO_CONTENT;
        }
    };

    // 在 blocking 线程执行文件系统清理
    let cfg_for_cleanup = cfg.clone();
    let bid_for_cleanup = book_id.clone();
    tokio::task::spawn_blocking(move || {
        cleanup_preview_cover_dir(&cfg_for_cleanup, &bid_for_cleanup, &book_name);
    })
    .await
    .ok();

    StatusCode::NO_CONTENT
}

/// 实际的文件清理逻辑（同步），与 TUI 版本逻辑一致。
fn cleanup_preview_cover_dir(
    cfg: &crate::base_system::context::Config,
    book_id: &str,
    book_name: &str,
) {
    let dir = book_folder_path(cfg, book_id, Some(book_name));
    if !dir.exists() {
        return;
    }
    // 存在 status.json 说明已有真正的下载，不应删除
    if dir.join("status.json").exists() {
        return;
    }

    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return;
    };

    let safe_name = safe_fs_name(book_name, "_", 120);
    let mut entries: Vec<PathBuf> = Vec::new();
    for ent in read_dir.flatten() {
        entries.push(ent.path());
    }
    if entries.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        info!(path = %dir.display(), "cleanup: 删除空的预览文件夹");
        return;
    }

    let is_cover_like = |p: &FsPath| -> bool {
        if p.is_dir() {
            return false;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        let Some(ext) = p.extension().and_then(|s| s.to_str()) else {
            return false;
        };
        let ext_lower = ext.to_ascii_lowercase();
        let is_img = matches!(
            ext_lower.as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "gif" | "heic" | "heif"
        );
        if !is_img {
            return false;
        }
        stem == safe_name || stem.eq_ignore_ascii_case("cover")
    };

    // 如果存在非封面文件，中止清理
    if entries.iter().any(|p| !is_cover_like(p)) {
        debug!(path = %dir.display(), "cleanup: 文件夹包含非封面文件，跳过");
        return;
    }

    for p in &entries {
        let _ = std::fs::remove_file(p);
    }

    if is_empty_dir(&dir).unwrap_or(false) {
        let _ = std::fs::remove_dir_all(&dir);
        info!(path = %dir.display(), "cleanup: 已清理预览产生的封面文件夹");
    }
}
