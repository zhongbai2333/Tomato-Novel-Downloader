use std::io::{self, Read, Seek, Write};
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::io::ReaderStream;
use zip::ZipWriter;
use zip::write::FileOptions;

use crate::ui::web::state::AppState;

static ZIP_SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(3)));
const MAX_ZIP_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 8_192;
const MAX_ZIP_DEPTH: usize = 32;
const MAX_ZIP_BUILD_TIME: Duration = Duration::from_secs(60);

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

    let slot = ZIP_SLOTS
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    // Keep the slot during compression and until the archive body is dropped.
    let (zip_path, slot) = tokio::task::spawn_blocking(move || {
        build_zip_to_temp(&target_canon).map(|path| (path, slot))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|err| match err.kind() {
        io::ErrorKind::FileTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        io::ErrorKind::TimedOut => StatusCode::REQUEST_TIMEOUT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    })?;

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
        _slot: slot,
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
    _slot: OwnedSemaphorePermit,
    inner: ReaderStream<tokio::fs::File>,
}

impl futures_core::Stream for TempFileStream {
    type Item = Result<axum::body::Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

fn build_zip_to_temp(dir: &std::path::Path) -> io::Result<tempfile::TempPath> {
    let tmp = tempfile::Builder::new().suffix(".zip").tempfile()?;
    let mut archive = ZipArchiveWriter {
        zip: ZipWriter::new(tmp.reopen()?),
        base: dir,
        deadline: Instant::now() + MAX_ZIP_BUILD_TIME,
        bytes_left: MAX_ZIP_BYTES,
        entries_left: MAX_ZIP_ENTRIES,
    };
    archive.add_directory(dir, 0)?;
    let ZipArchiveWriter { mut zip, .. } = archive;
    zip.finish()?.flush()?;
    Ok(tmp.into_temp_path())
}

struct ZipArchiveWriter<'a, W: Write + Seek> {
    zip: ZipWriter<W>,
    base: &'a std::path::Path,
    deadline: Instant,
    bytes_left: u64,
    entries_left: usize,
}

impl<W: Write + Seek> ZipArchiveWriter<'_, W> {
    fn check_deadline(&self) -> io::Result<()> {
        if Instant::now() > self.deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    }

    fn add_directory(&mut self, dir: &std::path::Path, depth: usize) -> io::Result<()> {
        if depth > MAX_ZIP_DEPTH {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        for entry in std::fs::read_dir(dir)? {
            self.check_deadline()?;
            self.entries_left = self
                .entries_left
                .checked_sub(1)
                .ok_or(io::ErrorKind::FileTooLarge)?;
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                self.add_directory(&path, depth + 1)?;
            } else if kind.is_file() {
                self.add_file(&path)?;
            }
        }
        Ok(())
    }

    fn add_file(&mut self, path: &std::path::Path) -> io::Result<()> {
        let ext = path
            .extension()
            .and_then(|part| part.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_allowed_ext(&ext) {
            return Ok(());
        }
        let canonical = std::fs::canonicalize(path)?;
        if !canonical.starts_with(self.base) || canonical != path {
            return Ok(());
        }
        let rel = path.strip_prefix(self.base).map_err(io::Error::other)?;
        let name = rel
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect::<Vec<_>>();
        if name.len() != rel.components().count()
            || name.iter().any(|component| component.contains(['\\', ':']))
        {
            return Ok(());
        }

        let mut file = std::fs::File::open(path)?;
        if !file.metadata()?.is_file() {
            return Ok(());
        }
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        self.zip.start_file(name.join("/"), options)?;
        let mut buf = [0u8; 8192];
        loop {
            self.check_deadline()?;
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            self.bytes_left = self
                .bytes_left
                .checked_sub(n as u64)
                .ok_or(io::ErrorKind::FileTooLarge)?;
            self.zip.write_all(&buf[..n])?;
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::build_zip_to_temp;

    #[test]
    fn zip_includes_regular_books_and_omits_symbolic_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::write(root.join("book.txt"), "book").unwrap();
        std::os::unix::fs::symlink(root.join("book.txt"), root.join("alias.txt")).unwrap();

        let zip = build_zip_to_temp(&root).unwrap();
        let file = std::fs::File::open(zip).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();
        assert_eq!(archive.file_names().collect::<Vec<_>>(), vec!["book.txt"]);
    }
}
