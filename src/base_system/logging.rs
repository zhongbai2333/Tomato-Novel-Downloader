use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::{io, panic, thread, time::Duration};

use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
use ctrlc;
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
        let logs_dir = PathBuf::from("logs");
        fs::create_dir_all(&logs_dir)?;
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
            .with_writer(console_writer)
            .with_filter(console_level);

        let broadcast_layer = if options.broadcast_to_ui {
            let (tx, _rx) = LOG_CHANNEL
                .get_or_init(crossbeam_channel::unbounded)
                .clone();
            let writer = BoxMakeWriter::new(ChannelWriterMake { tx });
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

        let file_layer = fmt::layer()
            .with_target(false)
            .with_level(true)
            .with_thread_names(true)
            .with_ansi(false)
            .with_writer(file_writer)
            .with_filter(LevelFilter::DEBUG);

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

    pub fn add_exit_hook<F>(&self, func: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if let Ok(mut hooks) = self.runtime.exit_hooks.lock() {
            hooks.push(Box::new(func));
        }
    }

    pub fn safe_exit(&self) {
        self.runtime.safe_exit();
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

        if self.archive_on_exit {
            if let Err(err) = archive_log_file(&self.latest_log, &self.logs_dir) {
                eprintln!("failed to archive log: {err}");
            }
        }
    }
}

fn archive_if_large(latest_log: &Path, logs_dir: &Path) -> Result<(), LogError> {
    if let Ok(meta) = fs::metadata(latest_log) {
        if meta.len() >= MAX_LOG_BYTES {
            archive_log_file(latest_log, logs_dir)?;
        }
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
