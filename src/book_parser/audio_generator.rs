//! 有声书生成（TTS）。

use std::fs;
use std::io::Write;
#[cfg(feature = "tts")]
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
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

// Edge Read Aloud 对单次 SSML 文本长度存在服务端限制。这里按字符数做保守切块，
// 给 SSML 包装和 XML 转义留下余量，避免长章节整章合成失败。
const TTS_CHUNK_MAX_CHARS: usize = 1800;

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

fn re_tags() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<[^>]+>").expect("hardcoded TTS tag regex should compile"))
}

fn re_multi_nl() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\n{2,}").expect("hardcoded TTS newline regex should compile"))
}

fn re_tabs() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"[\t\f\v]+").expect("hardcoded TTS whitespace regex should compile")
    })
}

fn re_spaces() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r" {2,}").expect("hardcoded TTS space regex should compile"))
}

fn sanitize_for_tts(title: &str, content: &str) -> String {
    // Ported from Python: f"{title}。\n{content}" + whitespace/html cleanup.
    let mut combined = format!("{}。\n{}", title, content);
    combined = combined.replace('\u{3000}', " ");
    combined = combined.replace("&nbsp;", " ");

    // Remove HTML tags.
    // NOTE: we keep it simple and consistent with the Python regex.
    combined = re_tags().replace_all(&combined, " ").to_string();

    combined = combined.replace("\r", "\n");
    combined = re_multi_nl().replace_all(&combined, "\n").to_string();
    combined = re_tabs().replace_all(&combined, " ").to_string();
    combined = re_spaces().replace_all(&combined, " ").to_string();

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

fn is_tts_sentence_boundary(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '。' | '！' | '？' | '；' | '：' | '，' | '、' | '…' | '!' | '?' | ';' | ':' | ','
    )
}

fn flush_tts_chunk(current: &mut String, current_chars: &mut usize, chunks: &mut Vec<String>) {
    let chunk = current.trim();
    if !chunk.is_empty() {
        chunks.push(chunk.to_string());
    }
    current.clear();
    *current_chars = 0;
}

fn split_oversized_tts_unit(unit: &str, chunks: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_chars = 0usize;

    for ch in unit.chars() {
        current.push(ch);
        current_chars += 1;

        if current_chars >= TTS_CHUNK_MAX_CHARS {
            flush_tts_chunk(&mut current, &mut current_chars, chunks);
        }
    }

    flush_tts_chunk(&mut current, &mut current_chars, chunks);
}

fn append_tts_unit(
    unit: &str,
    current: &mut String,
    current_chars: &mut usize,
    chunks: &mut Vec<String>,
) {
    let unit_chars = unit.chars().count();
    if unit_chars == 0 {
        return;
    }

    if unit_chars > TTS_CHUNK_MAX_CHARS {
        flush_tts_chunk(current, current_chars, chunks);
        split_oversized_tts_unit(unit, chunks);
        return;
    }

    if *current_chars > 0 && *current_chars + unit_chars > TTS_CHUNK_MAX_CHARS {
        flush_tts_chunk(current, current_chars, chunks);
    }

    current.push_str(unit);
    *current_chars += unit_chars;
}

fn split_tts_text(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    if text.chars().count() <= TTS_CHUNK_MAX_CHARS {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    let mut start = 0usize;

    for (idx, ch) in text.char_indices() {
        if is_tts_sentence_boundary(ch) {
            let end = idx + ch.len_utf8();
            append_tts_unit(
                &text[start..end],
                &mut current,
                &mut current_chars,
                &mut chunks,
            );
            start = end;
        }
    }

    if start < text.len() {
        append_tts_unit(
            &text[start..],
            &mut current,
            &mut current_chars,
            &mut chunks,
        );
    }
    flush_tts_chunk(&mut current, &mut current_chars, &mut chunks);

    chunks
}

fn is_wav_audio_format(audio_format: &str) -> bool {
    let f = audio_format.trim().to_ascii_lowercase();
    f.starts_with("riff-") || f.contains("wav") || f.contains("pcm")
}

struct WavParts<'a> {
    fmt: &'a [u8],
    data: &'a [u8],
}

fn extract_wav_parts(bytes: &[u8]) -> std::result::Result<WavParts<'_>, String> {
    if bytes.len() < 12 {
        return Err("文件头过短".to_string());
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("不是 RIFF/WAVE 数据".to_string());
    }

    let mut fmt = None;
    let mut data = None;
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let start = pos + 8;
        let end = start
            .checked_add(size)
            .ok_or_else(|| "chunk 大小溢出".to_string())?;
        if end > bytes.len() {
            return Err("chunk 数据不完整".to_string());
        }

        match id {
            b"fmt " if fmt.is_none() => fmt = Some(&bytes[start..end]),
            b"data" if data.is_none() => data = Some(&bytes[start..end]),
            _ => {}
        }

        pos = end;
        if size % 2 == 1 && pos < bytes.len() {
            pos += 1;
        }
    }

    let fmt = fmt.ok_or_else(|| "缺少 fmt chunk".to_string())?;
    let data = data.ok_or_else(|| "缺少 data chunk".to_string())?;
    Ok(WavParts { fmt, data })
}

fn push_u32_le(out: &mut Vec<u8>, value: usize, what: &str) -> std::result::Result<(), String> {
    let value = u32::try_from(value).map_err(|_| format!("{what} 超出 WAV 4GiB 限制"))?;
    out.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn build_wav(fmt: &[u8], data: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let fmt_pad = fmt.len() % 2;
    let data_pad = data.len() % 2;
    let riff_size = 4usize
        .checked_add(8 + fmt.len() + fmt_pad)
        .and_then(|v| v.checked_add(8 + data.len() + data_pad))
        .ok_or_else(|| "WAV 大小溢出".to_string())?;

    let mut out = Vec::with_capacity(8 + riff_size);
    out.extend_from_slice(b"RIFF");
    push_u32_le(&mut out, riff_size, "RIFF 大小")?;
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    push_u32_le(&mut out, fmt.len(), "fmt chunk 大小")?;
    out.extend_from_slice(fmt);
    if fmt_pad == 1 {
        out.push(0);
    }

    out.extend_from_slice(b"data");
    push_u32_le(&mut out, data.len(), "data chunk 大小")?;
    out.extend_from_slice(data);
    if data_pad == 1 {
        out.push(0);
    }

    Ok(out)
}

fn concatenate_wav_chunks(chunks: Vec<Vec<u8>>) -> std::result::Result<Vec<u8>, String> {
    let mut fmt: Option<Vec<u8>> = None;
    let mut data = Vec::new();

    for (idx, chunk) in chunks.iter().enumerate() {
        let parts =
            extract_wav_parts(chunk).map_err(|e| format!("第 {} 段 WAV 无效：{}", idx + 1, e))?;
        match fmt.as_deref() {
            Some(existing) if existing != parts.fmt => {
                return Err(format!("第 {} 段 WAV 参数与前文不一致", idx + 1));
            }
            Some(_) => {}
            None => fmt = Some(parts.fmt.to_vec()),
        }
        data.extend_from_slice(parts.data);
    }

    let fmt = fmt.ok_or_else(|| "没有可拼接的 WAV 数据".to_string())?;
    build_wav(&fmt, &data)
}

fn concatenate_audio_chunks(
    audio_format: &str,
    chunks: Vec<Vec<u8>>,
) -> std::result::Result<Vec<u8>, String> {
    if chunks.is_empty() {
        return Ok(Vec::new());
    }
    if chunks.len() == 1 {
        return Ok(chunks.into_iter().next().unwrap_or_default());
    }

    if is_wav_audio_format(audio_format) {
        return concatenate_wav_chunks(chunks);
    }

    let total_len = chunks.iter().try_fold(0usize, |acc, chunk| {
        acc.checked_add(chunk.len())
            .ok_or_else(|| "音频大小溢出".to_string())
    })?;
    let mut out = Vec::with_capacity(total_len);
    for chunk in chunks {
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

fn tts_cancelled(cancel: Option<&Arc<std::sync::atomic::AtomicBool>>) -> bool {
    cancel
        .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false)
}

fn synthesize_tts_chunks<F>(
    chunks: Vec<String>,
    audio_format: &str,
    mut synthesize: F,
    cancel: Option<&Arc<std::sync::atomic::AtomicBool>>,
) -> std::result::Result<Vec<u8>, String>
where
    F: FnMut(&str) -> std::result::Result<Vec<u8>, String>,
{
    if chunks.is_empty() {
        return Ok(Vec::new());
    }

    if chunks.len() == 1 {
        return synthesize(&chunks[0]);
    }

    let total = chunks.len();
    let mut audio_chunks = Vec::with_capacity(total);
    for (idx, chunk) in chunks.into_iter().enumerate() {
        if tts_cancelled(cancel) {
            return Err("已取消".to_string());
        }

        let bytes = synthesize(&chunk)
            .map_err(|e| format!("第 {}/{} 段合成失败：{}", idx + 1, total, e))?;
        if bytes.is_empty() {
            return Err(format!("第 {}/{} 段未返回音频", idx + 1, total));
        }
        audio_chunks.push(bytes);
    }

    concatenate_audio_chunks(audio_format, audio_chunks).map_err(|e| format!("音频拼接失败：{}", e))
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
                let chunks = split_tts_text(&job.text);
                if chunks.len() > 1 {
                    pb.println(format!(
                        "[TTS] 章节 {}《{}》文本较长，拆分为 {} 段合成",
                        job.idx,
                        job.title,
                        chunks.len()
                    ));
                }
                let audio_format = config.audio_format.clone();
                let r = synthesize_tts_chunks(
                    chunks,
                    &audio_format,
                    |chunk| match &mut backend {
                        #[cfg(feature = "tts")]
                        Backend::Ms(tts) => tts
                            .synthesize(chunk, &make_ms_config(&config))
                            .map(|a| a.audio_bytes)
                            .map_err(|e| e.to_string()),
                        #[cfg(feature = "tts-native")]
                        Backend::Edge(tts) => tts
                            .synthesize(chunk, &make_edge_config(&config))
                            .map(|a| a.audio_bytes)
                            .map_err(|e| e.to_string()),
                    },
                    cancel.as_ref(),
                );

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

#[cfg(test)]
mod tests {
    use super::{TTS_CHUNK_MAX_CHARS, concatenate_audio_chunks, extract_wav_parts, split_tts_text};

    fn wav_bytes(data: &[u8]) -> Vec<u8> {
        let fmt: [u8; 16] = [
            1, 0, // PCM
            1, 0, // mono
            0xC0, 0x5D, 0, 0, // 24000 Hz
            0x80, 0xBB, 0, 0, // 48000 bytes/sec
            2, 0, // block align
            16, 0, // bits/sample
        ];
        super::build_wav(&fmt, data).unwrap()
    }

    #[test]
    fn split_tts_text_prefers_sentence_boundaries() {
        let sentence = "这是一个用于测试的句子。";
        let text = sentence.repeat(TTS_CHUNK_MAX_CHARS / sentence.chars().count() + 3);

        let chunks = split_tts_text(&text);

        assert!(chunks.len() > 1);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= TTS_CHUNK_MAX_CHARS)
        );
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn split_tts_text_hard_splits_oversized_unit() {
        let text = "长".repeat(TTS_CHUNK_MAX_CHARS + 37);

        let chunks = split_tts_text(&text);

        assert_eq!(chunks.len(), 2);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= TTS_CHUNK_MAX_CHARS)
        );
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn concatenate_audio_chunks_concatenates_mp3_like_streams() {
        let bytes = concatenate_audio_chunks(
            "audio-24khz-48kbitrate-mono-mp3",
            vec![b"aaa".to_vec(), b"bbb".to_vec(), b"ccc".to_vec()],
        )
        .unwrap();

        assert_eq!(bytes, b"aaabbbccc");
    }

    #[test]
    fn concatenate_audio_chunks_rewrites_single_wav_header() {
        let bytes = concatenate_audio_chunks(
            "riff-24khz-16bit-mono-pcm",
            vec![wav_bytes(&[1, 2, 3, 4]), wav_bytes(&[5, 6])],
        )
        .unwrap();
        let parts = extract_wav_parts(&bytes).unwrap();

        assert_eq!(parts.data, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(bytes.windows(4).filter(|w| *w == b"RIFF").count(), 1);
    }
}
