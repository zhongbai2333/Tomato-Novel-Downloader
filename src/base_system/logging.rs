//! 日志系统初始化与控制台恢复。

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::{io, panic, thread, time::Duration};

use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
use ctrlc;
use regex::{Captures, Regex};
use time::OffsetDateTime;
use time::macros::format_description;
use tracing::{error, info};
use tracing_appender::non_blocking::{self, WorkerGuard};
use tracing_appender::rolling;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use zip::CompressionMethod;
use zip::write::FileOptions;

const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024; // 10MB
const ARCHIVE_WAIT_MS: u64 = 1000; // allow file handles to settle on Windows

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("logging already initialized")]
    AlreadyInitialized,
    #[error("subscriber init failed: {0}")]
    SubscriberInit(#[from] tracing_subscriber::util::TryInitError),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("time formatting failed: {0}")]
    Time(#[from] time::error::Format),
}

#[derive(Clone, Copy, Debug)]
pub struct LogOptions {
    pub debug: bool,
    pub use_color: bool,
    pub archive_on_exit: bool,
    pub console: bool,
    pub broadcast_to_ui: bool,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            debug: false,
            use_color: true,
            archive_on_exit: true,
            console: true,
            broadcast_to_ui: true,
        }
    }
}

static LOG_CHANNEL: OnceLock<(
    crossbeam_channel::Sender<String>,
    crossbeam_channel::Receiver<String>,
)> = OnceLock::new();
static LOGS_DIR: OnceLock<PathBuf> = OnceLock::new();
static ENDPOINT_REDACTION_RE: OnceLock<Regex> = OnceLock::new();

const REDACTED_ENDPOINT: &str = "[REDACTED_ENDPOINT]";

/// Remove URL endpoints from text before it is persisted, shown in the TUI, or exported.
///
/// This covers absolute HTTP/WebSocket URLs, bare domain names emitted by HTTP client debug
/// logs, and API/service route paths. Other diagnostic context is intentionally preserved.
pub(crate) fn redact_log_endpoints(text: &str) -> String {
    let re = ENDPOINT_REDACTION_RE.get_or_init(|| {
        Regex::new(
            r#"(?ix)
                \b(?:https?|wss?)://[^\s<>"'`]+
                |
                //(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}(?::[0-9]{1,5})?(?:/[^\s<>"'`]*)?
                |
                \b(?:url|uri|endpoint|host|domain)\s*=\s*[^\s,;}\]]+
                |
                \(\s*"?https?"?\s*,\s*"?(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}(?::[0-9]{1,5})?(?:/[^\s<>"'`]*)?
                |
                \b(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.){2,}[a-z]{2,63}(?::[0-9]{1,5})?(?:/[^\s<>"'`]*)?
                |
                \b(?:[0-9]{1,3}\.){3}[0-9]{1,3}:[0-9]{1,5}\b
                |
                \[[0-9a-f:]+\]:[0-9]{1,5}\b
                |
                /(?:api|service)/[^\s<>"'`]*
            "#,
        )
        .expect("valid endpoint redaction regex")
    });
    re.replace_all(text, |caps: &Captures<'_>| {
        let matched = caps.get(0).expect("redaction match");
        let is_domain_inside_path = matched.start() > 0
            && text.as_bytes()[matched.start() - 1] == b'/'
            && !matched.as_str().starts_with('/');
        if is_domain_inside_path {
            matched.as_str().to_string()
        } else {
            REDACTED_ENDPOINT.to_string()
        }
    })
    .into_owned()
}

struct RedactingWriter<W: io::Write> {
    inner: W,
    buffer: Vec<u8>,
}

impl<W: io::Write> RedactingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
        }
    }

    fn write_buffer(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&self.buffer);
        let redacted = redact_log_endpoints(&text);
        self.inner.write_all(redacted.as_bytes())?;
        self.buffer.clear();
        Ok(())
    }
}

impl<W: io::Write> io::Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.write_buffer()?;
        self.inner.flush()
    }
}

impl<W: io::Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        let _ = self.write_buffer();
        let _ = self.inner.flush();
    }
}

struct RedactingMakeWriter<M> {
    inner: M,
}

impl<M> RedactingMakeWriter<M> {
    fn new(inner: M) -> Self {
        Self { inner }
    }
}

impl<'a, M> MakeWriter<'a> for RedactingMakeWriter<M>
where
    M: MakeWriter<'a>,
{
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter::new(self.inner.make_writer())
    }
}

pub fn current_logs_dir() -> Option<PathBuf> {
    LOGS_DIR.get().cloned()
}

#[derive(Clone)]
struct ChannelWriter {
    tx: crossbeam_channel::Sender<String>,
}

impl std::io::Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf).to_string();
        let _ = self.tx.send(text);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn take_broadcast_rx() -> Option<crossbeam_channel::Receiver<String>> {
    LOG_CHANNEL.get().map(|(_, rx)| rx.clone())
}

#[derive(Clone)]
struct ChannelWriterMake {
    tx: crossbeam_channel::Sender<String>,
}

impl<'a> MakeWriter<'a> for ChannelWriterMake {
    type Writer = ChannelWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ChannelWriter {
            tx: self.tx.clone(),
        }
    }
}

pub struct LogSystem {
    runtime: Arc<LogRuntime>,
}

impl LogSystem {
    pub fn init(options: LogOptions) -> Result<Self, LogError> {
        Self::init_with_base(options, None)
    }

    /// Initialize the logging system, optionally using a base directory.
    ///
    /// # Arguments
    /// * `options` - Logging configuration options
    /// * `base_dir` - If provided, creates logs in base_dir/logs, otherwise uses ./logs
    pub fn init_with_base(options: LogOptions, base_dir: Option<&Path>) -> Result<Self, LogError> {
        let logs_dir = if let Some(base) = base_dir {
            base.join("logs")
        } else {
            PathBuf::from("logs")
        };
        fs::create_dir_all(&logs_dir)?;
        let _ = LOGS_DIR.set(logs_dir.clone());
        let latest_log = logs_dir.join("latest.log");

        archive_if_large(&latest_log, &logs_dir)?;

        let file_appender = rolling::never(&logs_dir, "latest.log");
        let (file_writer, guard) = non_blocking::NonBlockingBuilder::default()
            .lossy(false)
            .finish(file_appender);

        let console_level = if options.debug {
            LevelFilter::DEBUG
        } else {
            LevelFilter::INFO
        };

        let console_writer: BoxMakeWriter = if options.console {
            BoxMakeWriter::new(io::stdout)
        } else {
            BoxMakeWriter::new(io::sink)
        };

        let console_layer = fmt::layer()
            .with_target(false)
            .with_level(true)
            .with_thread_names(true)
            .with_ansi(options.use_color)
            .with_writer(RedactingMakeWriter::new(console_writer))
            .with_filter(console_level);

        let broadcast_layer = if options.broadcast_to_ui {
            let (tx, _rx) = LOG_CHANNEL
                .get_or_init(crossbeam_channel::unbounded)
                .clone();
            let writer = RedactingMakeWriter::new(BoxMakeWriter::new(ChannelWriterMake { tx }));
            Some(
                fmt::layer()
                    .with_target(false)
                    .with_level(true)
                    .with_thread_names(false)
                    .with_ansi(false)
                    .with_writer(writer)
                    .with_filter(console_level),
            )
        } else {
            None
        };

        // Persist DEBUG details even when the interactive console stays at INFO. Exported logs
        // should be useful for diagnosis without asking users to reproduce with a special flag.
        let file_level = LevelFilter::DEBUG;

        let file_layer = fmt::layer()
            .with_target(true)
            .with_level(true)
            .with_thread_names(true)
            .with_thread_ids(true)
            .with_file(true)
            .with_line_number(true)
            .with_ansi(false)
            .with_writer(RedactingMakeWriter::new(file_writer))
            .with_filter(file_level);

        tracing_subscriber::registry()
            .with(console_layer)
            .with(file_layer)
            .with(broadcast_layer)
            .try_init()
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("global subscriber") || msg.contains("already") {
                    LogError::AlreadyInitialized
                } else {
                    LogError::SubscriberInit(e)
                }
            })?;

        info!(
            target: "logging",
            version = env!("CARGO_PKG_VERSION"),
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            console_debug = options.debug,
            file_level = "DEBUG",
            endpoint_redaction = true,
            "日志系统已初始化"
        );

        let runtime = Arc::new(LogRuntime {
            logs_dir,
            latest_log,
            guard: Mutex::new(Some(guard)),
            exit_hooks: Mutex::new(Vec::new()),
            exit_called: AtomicBool::new(false),
            archive_on_exit: options.archive_on_exit,
        });

        runtime.install_signal_handler();
        runtime.install_panic_hook();

        Ok(Self { runtime })
    }
}

impl Drop for LogSystem {
    fn drop(&mut self) {
        self.runtime.safe_exit();
    }
}

struct LogRuntime {
    logs_dir: PathBuf,
    latest_log: PathBuf,
    guard: Mutex<Option<WorkerGuard>>,
    exit_hooks: Mutex<Vec<Box<dyn FnOnce() + Send + 'static>>>,
    exit_called: AtomicBool,
    archive_on_exit: bool,
}

impl LogRuntime {
    fn install_signal_handler(self: &Arc<Self>) {
        let runtime = Arc::clone(self);
        let _ = ctrlc::set_handler(move || {
            // Best-effort console restore: if the app is in TUI raw mode / alt screen,
            // leaving it as-is will make subsequent PowerShell input appear "stuck".
            let _ = disable_raw_mode();
            let mut out = io::stdout();
            let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);

            runtime.safe_exit();
            std::process::exit(0);
        });
    }

    fn install_panic_hook(self: &Arc<Self>) {
        let runtime = Arc::clone(self);
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if let Some(location) = info.location() {
                error!("panic at {}:{}: {}", location.file(), location.line(), info);
            } else {
                error!("panic: {info}");
            }
            runtime.safe_exit();
            previous(info);
        }));
    }

    fn safe_exit(&self) {
        if self.exit_called.swap(true, Ordering::SeqCst) {
            return;
        }

        if let Ok(mut hooks) = self.exit_hooks.lock() {
            while let Some(func) = hooks.pop() {
                func();
            }
        }

        if let Ok(mut guard) = self.guard.lock() {
            guard.take();
        }

        thread::sleep(Duration::from_millis(ARCHIVE_WAIT_MS));

        if self.archive_on_exit
            && let Err(err) = archive_log_file(&self.latest_log, &self.logs_dir)
        {
            eprintln!("failed to archive log: {err}");
        }
    }
}

fn archive_if_large(latest_log: &Path, logs_dir: &Path) -> Result<(), LogError> {
    if let Ok(meta) = fs::metadata(latest_log)
        && meta.len() >= MAX_LOG_BYTES
    {
        archive_log_file(latest_log, logs_dir)?;
    }
    Ok(())
}

fn archive_log_file(latest_log: &Path, logs_dir: &Path) -> Result<Option<PathBuf>, LogError> {
    if !latest_log.exists() {
        return Ok(None);
    }
    let meta = fs::metadata(latest_log)?;
    if meta.len() == 0 {
        let _ = fs::remove_file(latest_log);
        return Ok(None);
    }

    let timestamp = OffsetDateTime::now_utc().format(format_description!(
        "[year][month][day]_[hour][minute][second]"
    ))?;
    let archive_path = logs_dir.join(format!("log_{timestamp}.zip"));
    let temp_log = logs_dir.join(format!("temp_{timestamp}.log"));
    fs::copy(latest_log, &temp_log)?;

    let file = File::create(&archive_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(format!("{timestamp}.log"), options)?;

    let mut temp_file = File::open(&temp_log)?;
    io::copy(&mut temp_file, &mut zip)?;
    zip.finish()?;

    let _ = fs::remove_file(&temp_log);
    let _ = fs::remove_file(latest_log);

    info!("log archived to {}", archive_path.display());
    Ok(Some(archive_path))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use super::{REDACTED_ENDPOINT, RedactingWriter, redact_log_endpoints};

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn redacts_absolute_urls_domains_and_api_paths() {
        let input = concat!(
            "request failed for https://api5-normal-sinfonlinec.fqnovel.com/reading/bookapi/search/tab/v?q=test; ",
            "pool=(https, api5-normal-sinfonlinec.fqnovel.com); ",
            "path=/api/search status=502; register=/service/2/device_register/; ",
            "proxy=127.0.0.1:10808"
        );

        let output = redact_log_endpoints(input);

        assert!(!output.contains("fqnovel.com"));
        assert!(!output.contains("/api/search"));
        assert!(!output.contains("device_register"));
        assert!(!output.contains("127.0.0.1:10808"));
        assert!(output.matches(REDACTED_ENDPOINT).count() >= 5);
        assert!(output.contains("request failed"));
        assert!(output.contains("status=502"));
    }

    #[test]
    fn preserves_non_endpoint_diagnostics() {
        let input = concat!(
            "surface=web stage=search error_kind=network status=403 timeout_ms=15000 ",
            "dns=failed archive=logs/latest.log source=/tmp/index.crates.io-hash/src/main.rs"
        );

        assert_eq!(redact_log_endpoints(input), input);
    }

    #[test]
    fn writer_redacts_an_endpoint_split_across_writes() {
        let output = SharedBuffer::default();
        let inspect = output.clone();
        {
            let mut writer = RedactingWriter::new(output);
            writer
                .write_all(b"network error for https://api5-normal-")
                .unwrap();
            writer
                .write_all(b"sinfonlinec.fqnovel.com/search status=403")
                .unwrap();
            writer.flush().unwrap();
        }

        let text = String::from_utf8(inspect.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text, "network error for [REDACTED_ENDPOINT] status=403");
    }
}
