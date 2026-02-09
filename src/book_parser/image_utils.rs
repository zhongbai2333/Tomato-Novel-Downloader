//! 图片下载、缓存、格式转换。
//!
//! 负责从网络获取图片、本地缓存检查、JPEG 转码等。

use std::path::{Path, PathBuf};

use image::GenericImageView;
use sha1::{Digest, Sha1};

use crate::base_system::context::Config;

use super::segment_shared::write_atomic;

// ── 哈希 ────────────────────────────────────────────────────────

pub(crate) fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

// ── MIME / 扩展名 ───────────────────────────────────────────────

fn mime_from_ext(ext: &str) -> &'static str {
    match ext {
        ".jpg" | ".jpeg" => "image/jpeg",
        ".png" => "image/png",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        ".avif" => "image/avif",
        ".heic" | ".heif" => "image/heic",
        _ => "application/octet-stream",
    }
}

fn sniff_mime_ext(bytes: &[u8]) -> (&'static str, &'static str) {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return ("image/jpeg", ".jpeg");
    }
    if bytes.len() >= 8
        && bytes[0] == 0x89
        && bytes[1] == 0x50
        && bytes[2] == 0x4E
        && bytes[3] == 0x47
        && bytes[4] == 0x0D
        && bytes[5] == 0x0A
        && bytes[6] == 0x1A
        && bytes[7] == 0x0A
    {
        return ("image/png", ".png");
    }
    if bytes.len() >= 6 && (&bytes[0..3] == b"GIF") {
        return ("image/gif", ".gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return ("image/webp", ".webp");
    }
    // HEIC/HEIF detection: ISO BMFF 'ftyp' brand.
    if bytes.len() >= 16 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"heic"
            || brand == b"heif"
            || brand == b"heix"
            || brand == b"mif1"
            || brand == b"msf1"
        {
            return ("image/heic", ".heic");
        }
    }
    ("application/octet-stream", "")
}

// ── JPEG 转码 ───────────────────────────────────────────────────

fn try_convert_to_jpeg(bytes: &[u8], quality: u8, max_dim: u32) -> Option<Vec<u8>> {
    let mut img = image::load_from_memory(bytes).ok()?;

    if max_dim > 0 {
        let (w, h) = img.dimensions();
        let longest = w.max(h);
        if longest > max_dim {
            let scale = max_dim as f32 / longest as f32;
            let nw = ((w as f32) * scale).round().max(1.0) as u32;
            let nh = ((h as f32) * scale).round().max(1.0) as u32;
            img = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
        }
    }

    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    {
        let q = quality.clamp(1, 100);
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q);
        encoder
            .encode(
                &rgb,
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .ok()?;
    }
    Some(out)
}

// ── 缓存查找 ────────────────────────────────────────────────────

fn find_cached_image(
    images_dir: &Path,
    hash: &str,
) -> Option<(PathBuf, &'static str, &'static str)> {
    let exts = [
        ".jpeg", ".jpg", ".png", ".gif", ".webp", ".avif", ".heic", ".heif",
    ];
    for ext in exts {
        let p = images_dir.join(format!("{hash}{ext}"));
        if p.exists() {
            return Some((p, mime_from_ext(ext), ext));
        }
    }
    None
}

// ── 网络获取 + 归一化 ──────────────────────────────────────────

fn fetch_and_normalize_image(
    cfg: &Config,
    url: &str,
) -> anyhow::Result<Option<(Vec<u8>, &'static str, &'static str)>> {
    if !cfg.blocked_media_domains.is_empty() {
        let lowered = url.to_ascii_lowercase();
        if cfg
            .blocked_media_domains
            .iter()
            .any(|d| !d.trim().is_empty() && lowered.contains(&d.to_ascii_lowercase()))
        {
            return Ok(None);
        }
    }

    let bytes = match crate::third_party::media_fetch::fetch_bytes(
        url,
        std::time::Duration::from_millis(10_000),
    ) {
        Some(b) => b,
        None => return Ok(None),
    };

    let (mime, ext) = sniff_mime_ext(&bytes);

    // 转码逻辑：
    // - force_convert_images_to_jpeg=true：无条件尽量转
    // - jpeg_retry_convert=true：当识别失败/或非 jpeg 时尝试转（可提升兼容性）
    let should_try_jpeg = cfg.force_convert_images_to_jpeg
        || cfg.jpeg_retry_convert && (mime == "application/octet-stream" || mime != "image/jpeg");
    if should_try_jpeg
        // HEIC/HEIF needs explicit opt-in for conversion attempt (image crate likely can't decode it).
        && (mime != "image/heic" || cfg.convert_heic_to_jpeg)
        && let Some(jpeg) = try_convert_to_jpeg(&bytes, cfg.jpeg_quality, cfg.media_max_dimension_px)
    {
        return Ok(Some((jpeg, "image/jpeg", ".jpeg")));
    }

    // If it's HEIC/HEIF and conversion failed, keep original only when configured.
    if mime == "image/heic" {
        if cfg.keep_heic_original {
            return Ok(Some((bytes, "image/heic", ext)));
        }
        return Ok(None);
    }

    if mime == "application/octet-stream" {
        return Ok(None);
    }

    Ok(Some((bytes, mime, ext)))
}

// ── 确保本地缓存 ───────────────────────────────────────────────

pub(crate) fn ensure_cached_image(
    cfg: &Config,
    url: &str,
    images_dir: &Path,
) -> anyhow::Result<Option<(PathBuf, &'static str, &'static str)>> {
    if !cfg.blocked_media_domains.is_empty() {
        let lowered = url.to_ascii_lowercase();
        if cfg
            .blocked_media_domains
            .iter()
            .any(|d| !d.trim().is_empty() && lowered.contains(&d.to_ascii_lowercase()))
        {
            return Ok(None);
        }
    }

    let hash = sha1_hex(url);
    if let Some(hit) = find_cached_image(images_dir, &hash) {
        return Ok(Some(hit));
    }

    let fetched = fetch_and_normalize_image(cfg, url)?;
    let Some((bytes, mime, ext)) = fetched else {
        return Ok(None);
    };

    std::fs::create_dir_all(images_dir)?;
    let out_path = images_dir.join(format!("{hash}{ext}"));
    if !out_path.exists() {
        let _ = write_atomic(&out_path, &bytes);
    }
    Ok(Some((out_path, mime, ext)))
}
