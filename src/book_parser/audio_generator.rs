//! 有声书生成（TTS）。

use std::fs;
use std::io::Write;
#[cfg(feature = "tts")]
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

#[cfg(feature = "tts-native")]
use super::edge_tts::{EdgeTtsClient, SpeechConfig as EdgeSpeechConfig};
use crossbeam_channel as channel;
use indicatif::{ProgressBar, ProgressStyle};
#[cfg(feature = "tts")]
use msedge_tts::tts::SpeechConfig as MsSpeechConfig;
#[cfg(feature = "tts")]
use msedge_tts::tts::client::{MSEdgeTTSClient, connect};
use regex::Regex;
use serde_json::Value;
use tracing::{error, info, warn};

use super::book_manager::BookManager;
use crate::base_system::context::safe_fs_name;
use crate::download::downloader::{ProgressReporter, SavePhase};

struct ChapterJob {
    idx: usize,
    title: String,
    text: String,
    out_path: PathBuf,
    tmp_path: PathBuf,
}

#[derive(Debug, Clone)]
struct AudiobookSpeechConfig {
    voice_name: String,
    audio_format: String,
    pitch: i32,
    rate: i32,
    volume: i32,
}

fn parse_percent_i32(input: &str) -> i32 {
    let s = input.trim();
    if s.is_empty() {
        return 0;
    }
    let s = s.strip_suffix('%').unwrap_or(s).trim();
    if s.is_empty() {
        return 0;
    }
    if let Ok(v) = s.parse::<i32>() {
        return v;
    }
    if let Ok(v) = s.parse::<f64>() {
        return v.round() as i32;
    }
    0
}

fn parse_pitch_hz_i32(input: &str) -> i32 {
    let s = input.trim();
    if s.is_empty() {
        return 0;
    }
    let lower = s.to_ascii_lowercase();
    if lower == "default" || lower == "auto" || lower == "none" {
        return 0;
    }

    // Edge TTS uses prosody pitch in Hz (integer).
    // Accept forms like: +2Hz, -10hz, 0Hz, 12
    let s2 = lower.strip_suffix("hz").unwrap_or(&lower).trim();
    if let Ok(v) = s2.parse::<i32>() {
        return v;
    }
    if let Ok(v) = s2.parse::<f64>() {
        return v.round() as i32;
    }
    0
}

fn audio_format_from_simple(fmt: &str) -> (&'static str, &'static str) {
    let f = fmt.trim().to_ascii_lowercase();
    match f.as_str() {
        // mp3: streaming format
        "mp3" => ("mp3", "audio-24khz-48kbitrate-mono-mp3"),
        // wav: riff pcm
        "wav" => ("wav", "riff-24khz-16bit-mono-pcm"),
        _ => ("mp3", "audio-24khz-48kbitrate-mono-mp3"),
    }
}

fn sanitize_for_tts(title: &str, content: &str) -> String {
    // Ported from Python: f"{title}。\n{content}" + whitespace/html cleanup.
    let mut combined = format!("{}。\n{}", title, content);
    combined = combined.replace('\u{3000}', " ");
    combined = combined.replace("&nbsp;", " ");

    // Remove HTML tags.
    // NOTE: we keep it simple and consistent with the Python regex.
    let re_tags = Regex::new(r"<[^>]+>").unwrap();
    combined = re_tags.replace_all(&combined, " ").to_string();

    combined = combined.replace("\r", "\n");
    let re_multi_nl = Regex::new(r"\n{2,}").unwrap();
    combined = re_multi_nl.replace_all(&combined, "\n").to_string();
    let re_tabs = Regex::new(r"[\t\f\v]+").unwrap();
    combined = re_tabs.replace_all(&combined, " ").to_string();
    let re_spaces = Regex::new(r" {2,}").unwrap();
    combined = re_spaces.replace_all(&combined, " ").to_string();

    combined.trim().to_string()
}

fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn write_atomic(path: &Path, tmp_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    ensure_parent(path)?;
    ensure_parent(tmp_path)?;

    let _ = fs::remove_file(tmp_path);
    let _ = fs::remove_file(path);

    {
        let mut f = fs::File::create(tmp_path)?;
        f.write_all(bytes)?;
        f.flush()?;
    }
    fs::rename(tmp_path, path)?;
    Ok(())
}

/// 将已下载章节内容转换为音频文件（使用 Edge TTS / Read Aloud）。
///
/// - 输出目录：`{默认保存目录}/{书名}_audio/`
/// - 文件命名：`0001-章节标题.mp3|wav`
/// - 失败策略：单章失败只记录错误，整体仍继续；最终返回值仅表示是否“未被取消/未发生致命初始化错误”。
pub fn generate_audiobook(
    manager: &BookManager,
    chapters: &[Value],
    bar: Option<&ProgressBar>,
    quiet: bool,
    mut progress: Option<&mut ProgressReporter>,
    cancel: Option<&Arc<std::sync::atomic::AtomicBool>>,
) -> bool {
    let cfg = &manager.config;
    if !cfg.enable_audiobook {
        return true;
    }

    if cancel
        .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false)
    {
        return false;
    }

    let book_name = if manager.book_name.trim().is_empty() {
        manager.book_id.as_str()
    } else {
        manager.book_name.as_str()
    };
    let safe_book = safe_fs_name(book_name, "_", 120);
    let output_dir = manager.default_save_dir();
    let audio_dir = output_dir.join(format!("{}_audio", safe_book));
    if let Err(e) = fs::create_dir_all(&audio_dir) {
        error!(target: "book_manager", error = ?e, "create audio output dir failed");
        return false;
    }

    let voice = {
        let v = cfg.audiobook_voice.trim();
        if v.is_empty() {
            "zh-CN-XiaoxiaoNeural".to_string()
        } else {
            v.to_string()
        }
    };
    let rate = parse_percent_i32(&cfg.audiobook_rate);
    let volume = parse_percent_i32(&cfg.audiobook_volume);
    let pitch = {
        let raw = cfg.audiobook_pitch.trim();
        if raw.to_ascii_lowercase().ends_with("st") {
            warn!(target: "book_manager", "[TTS] pitch 不支持 st 单位（当前实现仅支持 Hz），已忽略：{}", raw);
            0
        } else {
            parse_pitch_hz_i32(raw)
        }
    };

    let (ext, audio_format) = audio_format_from_simple(&cfg.audiobook_format);
    if cfg.audiobook_format.trim().is_empty() {
        // keep default
    } else {
        let f = cfg.audiobook_format.trim().to_ascii_lowercase();
        if f != "mp3" && f != "wav" {
            warn!(target: "book_manager", "[TTS] 音频格式 {} 不受支持，已回退为 mp3", f);
        }
    }

    let config = Arc::new(AudiobookSpeechConfig {
        voice_name: voice,
        audio_format: audio_format.to_string(),
        pitch,
        rate,
        volume,
    });

    #[cfg(feature = "tts")]
    fn make_ms_config(cfg: &Arc<AudiobookSpeechConfig>) -> MsSpeechConfig {
        let cfg = cfg.as_ref();
        MsSpeechConfig {
            voice_name: cfg.voice_name.clone(),
            audio_format: cfg.audio_format.clone(),
            pitch: cfg.pitch,
            rate: cfg.rate,
            volume: cfg.volume,
        }
    }

    #[cfg(feature = "tts-native")]
    fn make_edge_config(cfg: &Arc<AudiobookSpeechConfig>) -> EdgeSpeechConfig {
        let cfg = cfg.as_ref();
        EdgeSpeechConfig {
            voice_name: cfg.voice_name.clone(),
            audio_format: cfg.audio_format.clone(),
            pitch: cfg.pitch,
            rate: cfg.rate,
            volume: cfg.volume,
        }
    }

    let mut jobs = Vec::new();
    for (index, chapter) in (chapters.iter()).enumerate() {
        let cid = chapter.get("id").and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        });
        let Some(cid) = cid else { continue };
        let stored = manager
            .downloaded
            .get(&cid)
            .or_else(|| manager.downloaded.get(&cid.to_string()));
        let Some((stored_title, stored_content)) = stored else {
            continue;
        };
        let content = match stored_content.as_deref() {
            Some(s) if !s.trim().is_empty() => s,
            _ => continue,
        };
        let title = if !stored_title.trim().is_empty() {
            stored_title.clone()
        } else {
            chapter
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("章节")
                .to_string()
        };

        let text = sanitize_for_tts(&title, content);
        if text.trim().is_empty() {
            continue;
        }

        let idx = index + 1;
        let file_name = format!("{:04}-{}.{}", idx, safe_fs_name(&title, "_", 120), ext);
        let out_path = audio_dir.join(file_name);
        let tmp_path = out_path.with_extension(format!("{}.partial", ext));
        jobs.push(ChapterJob {
            idx,
            title,
            text,
            out_path,
            tmp_path,
        });
    }

    if jobs.is_empty() {
        info!(target: "book_manager", "无可用章节内容，跳过有声小说生成");
        return true;
    }

    if let Some(p) = progress.as_deref_mut() {
        p.set_save_phase(SavePhase::Audiobook);
        p.reset_save_progress(jobs.len());
    }

    let mut concurrency = cfg.audiobook_concurrency.max(1);
    concurrency = concurrency.min(jobs.len());

    info!(
        target: "book_manager",
        "开始生成有声小说：chapters={} -> {}，并发={}",
        jobs.len(),
        audio_dir.display(),
        concurrency
    );

    // Fail-fast probe: if we cannot connect at all, skip spawning workers.
    {
        let mut ok = false;
        #[cfg(feature = "tts")]
        {
            if connect().is_ok() {
                ok = true;
            }
        }
        #[cfg(all(not(feature = "tts"), feature = "tts-native"))]
        {
            if EdgeTtsClient::connect().is_ok() {
                ok = true;
            }
        }
        #[cfg(all(feature = "tts", feature = "tts-native"))]
        {
            if !ok && EdgeTtsClient::connect().is_ok() {
                ok = true;
            }
        }
        if !ok {
            error!(target: "book_manager", "[TTS] 无法连接到语音服务（msedge-tts / native 均失败）");
            return false;
        }
    }

    let (pb, owns_bar) = if let Some(existing) = bar {
        existing.set_prefix("有声书");
        existing.set_length(jobs.len() as u64);
        existing.set_position(0);
        existing.set_message("");
        (existing.clone(), false)
    } else if quiet {
        // TUI/自定义 UI 渲染场景下，避免 indicatif 进度条打乱终端布局。
        let pb = ProgressBar::hidden();
        pb.set_length(jobs.len() as u64);
        pb.set_position(0);
        (pb, true)
    } else {
        let pb = ProgressBar::new(jobs.len() as u64);
        let style = ProgressStyle::with_template("{prefix} {bar:40.cyan/blue} {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("=>-");
        pb.set_style(style);
        pb.set_prefix("有声书");
        (pb, true)
    };

    let (tx, rx) = channel::unbounded::<ChapterJob>();
    let (done_tx, done_rx) = channel::unbounded::<()>();
    let errors = Arc::new(AtomicUsize::new(0));

    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let rx = rx.clone();
        let config = config.clone();
        let pb = pb.clone();
        let errors = errors.clone();
        let done_tx = done_tx.clone();
        let cancel = cancel.map(Arc::clone);
        workers.push(thread::spawn(move || {
            enum Backend {
                #[cfg(feature = "tts")]
                Ms(MSEdgeTTSClient<TcpStream>),
                #[cfg(feature = "tts-native")]
                Edge(EdgeTtsClient),
            }

            let mut backend = None;

            #[cfg(feature = "tts")]
            {
                if let Ok(c) = connect() {
                    backend = Some(Backend::Ms(c));
                }
            }
            #[cfg(all(feature = "tts-native", not(feature = "tts")))]
            {
                if let Ok(c) = EdgeTtsClient::connect() {
                    backend = Some(Backend::Edge(c));
                }
            }
            #[cfg(all(feature = "tts", feature = "tts-native"))]
            {
                if backend.is_none() {
                    if let Ok(c) = EdgeTtsClient::connect() {
                        backend = Some(Backend::Edge(c));
                    }
                }
            }

            let mut backend = match backend {
                Some(b) => b,
                None => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    pb.println("[TTS] connect failed");
                    // Drain jobs so progress won't hang.
                    while rx
                        .recv_timeout(std::time::Duration::from_millis(200))
                        .is_ok()
                    {
                        pb.inc(1);
                        let _ = done_tx.send(());
                    }
                    return;
                }
            };

            loop {
                if cancel
                    .as_ref()
                    .map(|c| c.load(Ordering::Relaxed))
                    .unwrap_or(false)
                {
                    // Drain remaining jobs so main thread won't hang waiting for done signals.
                    while rx
                        .recv_timeout(std::time::Duration::from_millis(200))
                        .is_ok()
                    {
                        pb.inc(1);
                        let _ = done_tx.send(());
                    }
                    return;
                }

                let job = match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                    Ok(j) => j,
                    Err(channel::RecvTimeoutError::Timeout) => continue,
                    Err(channel::RecvTimeoutError::Disconnected) => break,
                };
                let r: std::result::Result<Vec<u8>, String> = match &mut backend {
                    #[cfg(feature = "tts")]
                    Backend::Ms(tts) => tts
                        .synthesize(&job.text, &make_ms_config(&config))
                        .map(|a| a.audio_bytes)
                        .map_err(|e| e.to_string()),
                    #[cfg(feature = "tts-native")]
                    Backend::Edge(tts) => tts
                        .synthesize(&job.text, &make_edge_config(&config))
                        .map(|a| a.audio_bytes)
                        .map_err(|e| e.to_string()),
                };

                match r {
                    Ok(bytes) => {
                        if let Err(e) = write_atomic(&job.out_path, &job.tmp_path, &bytes) {
                            errors.fetch_add(1, Ordering::Relaxed);
                            pb.println(format!(
                                "[TTS] 章节 {}《{}》写入失败：{}",
                                job.idx, job.title, e
                            ));
                        }
                    }
                    Err(e) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                        pb.println(format!(
                            "[TTS] 章节 {}《{}》生成失败：{}",
                            job.idx, job.title, e
                        ));
                    }
                }
                pb.inc(1);
                let _ = done_tx.send(());
            }
        }));
    }
    drop(rx);
    drop(done_tx);

    let total_jobs = jobs.len();

    for job in jobs {
        if tx.send(job).is_err() {
            break;
        }
    }
    drop(tx);

    for _ in 0..total_jobs {
        if done_rx.recv().is_err() {
            break;
        }
        if let Some(p) = progress.as_deref_mut() {
            p.inc_save_progress();
        }
    }

    for w in workers {
        let _ = w.join();
    }

    if owns_bar {
        pb.finish_and_clear();
    }
    let err_cnt = errors.load(Ordering::Relaxed);
    if err_cnt > 0 {
        warn!(
            target: "book_manager",
            "有声小说生成完成（部分失败 {} 章）：{}",
            err_cnt,
            audio_dir.display()
        );
    } else {
        info!(target: "book_manager", "有声小说生成完成：{}", audio_dir.display());
    }

    true
}
