//! 下载历史持久化（JSONL）。
//!
//! 记录每次下载/更新任务的关键信息，便于跨会话追溯。

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::logging;

const HISTORY_FILE_NAME: &str = "download_history.jsonl";

pub fn history_file_path() -> PathBuf {
    let logs_dir = logging::current_logs_dir().unwrap_or_else(|| PathBuf::from("logs"));
    logs_dir.join(HISTORY_FILE_NAME)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadHistoryRecord {
    pub timestamp: String,
    pub book_id: String,
    pub book_name: String,
    pub author: String,
    pub selected_chapters: usize,
    pub success_chapters: usize,
    pub failed_chapters: usize,
    pub progress: String,
    pub status: String,
}

impl DownloadHistoryRecord {
    pub fn new(
        book_id: String,
        book_name: String,
        author: String,
        selected_chapters: usize,
        success_chapters: usize,
        failed_chapters: usize,
        status: String,
    ) -> Self {
        let timestamp = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
        let progress = format!(
            "成功 {}/{} 章，失败 {} 章",
            success_chapters, selected_chapters, failed_chapters
        );

        Self {
            timestamp,
            book_id,
            book_name,
            author,
            selected_chapters,
            success_chapters,
            failed_chapters,
            progress,
            status,
        }
    }
}

pub fn append_download_history(record: &DownloadHistoryRecord) {
    let logs_dir = logging::current_logs_dir().unwrap_or_else(|| PathBuf::from("logs"));
    if fs::create_dir_all(&logs_dir).is_err() {
        return;
    }

    let path = logs_dir.join(HISTORY_FILE_NAME);
    let mut file = match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(_) => return,
    };

    let line = match serde_json::to_string(record) {
        Ok(v) => v,
        Err(_) => return,
    };

    let _ = writeln!(file, "{line}");
    let _ = file.flush();
}

pub fn read_download_history(limit: usize, keyword: Option<&str>) -> Vec<DownloadHistoryRecord> {
    let path = history_file_path();
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let keyword = keyword
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());

    let mut out: Vec<DownloadHistoryRecord> = Vec::new();
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<DownloadHistoryRecord>(trimmed) else {
            continue;
        };

        if let Some(k) = keyword.as_deref() {
            let hay = format!(
                "{} {} {} {} {}",
                rec.book_id, rec.book_name, rec.author, rec.progress, rec.status
            )
            .to_ascii_lowercase();
            if !hay.contains(k) {
                continue;
            }
        }
        out.push(rec);
    }

    out.reverse();
    if limit > 0 && out.len() > limit {
        out.truncate(limit);
    }
    out
}
