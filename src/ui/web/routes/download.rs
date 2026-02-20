use std::io::{Read, Seek, Write};
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use tokio_util::io::ReaderStream;
use zip::ZipWriter;
use zip::write::FileOptions;

use crate::ui::web::state::AppState;

fn make_content_disposition(filename: &str) -> Option<header::HeaderValue> {
    // RFC 5987 filename* for UTF-8 names, plus ASCII fallback for legacy clients.
    fn is_unreserved(b: u8) -> bool {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')
    }

    let mut encoded = String::with_capacity(filename.len() * 3);
    for &b in filename.as_bytes() {
        if is_unreserved(b) {
            encoded.push(char::from(b));
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{b:02X}"));
        }
    }

    let ascii_fallback = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();

    let value = format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        ascii_fallback, encoded
    );
    header::HeaderValue::from_str(&value).ok()
}

pub(crate) async fn download_file(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
) -> Result<Response, StatusCode> {
    let base = state.library_root.as_ref();
    let (_target, target_canon) = resolve_target(base, &path)?;

    let meta = std::fs::metadata(&target_canon).map_err(|_| StatusCode::NOT_FOUND)?;
    if !meta.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }

    let ext = target_canon
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !is_allowed_ext(&ext) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mime = match ext.as_str() {
        "epub" => "application/epub+zip",
        "txt" => "text/plain; charset=utf-8",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    };

    let file = tokio::fs::File::open(&target_canon)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, header::HeaderValue::from_static(mime));

    if let Some(name) = target_canon.file_name().and_then(|s| s.to_str())
        && let Some(hv) = make_content_disposition(name)
    {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, hv);
    }

    Ok(resp)
}

fn is_allowed_ext(ext: &str) -> bool {
    matches!(ext, "epub" | "txt" | "mp3" | "wav")
}

fn resolve_target(
    base: &std::path::Path,
    path: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), StatusCode> {
    if path.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }

    let base_canon = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
    let target = base.join(path);
    let target_canon = std::fs::canonicalize(&target).map_err(|_| StatusCode::NOT_FOUND)?;
    if !target_canon.starts_with(&base_canon) {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok((target, target_canon))
}

pub(crate) async fn download_zip(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
) -> Result<Response, StatusCode> {
    let base = state.library_root.as_ref();
    let (target, target_canon) = resolve_target(base, &path)?;

    let meta = std::fs::metadata(&target_canon).map_err(|_| StatusCode::NOT_FOUND)?;
    if !meta.is_dir() {
        return Err(StatusCode::NOT_FOUND);
    }

    // build zip to a temp file (zip writer requires Seek)
    let zip_path = tokio::task::spawn_blocking(move || build_zip_to_temp(&target_canon))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let filename = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("download")
        .to_string();

    let file = tokio::fs::File::open(&zip_path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let stream = TempFileStream {
        _temp: zip_path,
        inner: ReaderStream::new(file),
    };
    let body = Body::from_stream(stream);

    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/zip"),
    );

    let zip_name = format!("{filename}.zip");
    if let Some(hv) = make_content_disposition(&zip_name) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, hv);
    }

    Ok(resp)
}

struct TempFileStream {
    _temp: tempfile::TempPath,
    inner: ReaderStream<tokio::fs::File>,
}

impl futures_core::Stream for TempFileStream {
    type Item = Result<axum::body::Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

fn build_zip_to_temp(dir: &std::path::Path) -> std::io::Result<tempfile::TempPath> {
    let mut tmp = tempfile::Builder::new().suffix(".zip").tempfile()?;

    {
        let out = tmp.as_file_mut();
        out.seek(std::io::SeekFrom::Start(0))?;
        out.set_len(0)?;

        let mut zip = ZipWriter::new(out);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let base = dir;
        let mut buf = vec![0u8; 8192];
        let _ = add_dir_to_zip(&mut zip, base, base, options, &mut buf);
        let out = zip.finish()?;
        out.flush()?;
    }

    Ok(tmp.into_temp_path())
}

fn add_dir_to_zip<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    base: &std::path::Path,
    dir: &std::path::Path,
    options: FileOptions,
    buf: &mut Vec<u8>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        if meta.is_dir() {
            let _ = add_dir_to_zip(zip, base, &path, options, buf);
            continue;
        }

        if !meta.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_allowed_ext(&ext) {
            continue;
        }

        let rel = path.strip_prefix(base).unwrap_or(&path);
        let name = rel.to_string_lossy().replace('\\', "/");
        zip.start_file(name, options)?;
        let mut f = std::fs::File::open(&path)?;
        loop {
            let n = f.read(buf)?;
            if n == 0 {
                break;
            }
            zip.write_all(&buf[..n])?;
        }
    }
    Ok(())
}
