//! TUI 下载页。
//!
//! 处理用户输入、启动下载任务、展示进度与状态。

use std::sync::{Arc, atomic::AtomicBool};
use std::thread;

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::download::downloader::{self, ChapterRange, ProgressSnapshot, SavePhase};

use super::{App, Focus, PendingDownload, View, WorkerMsg, start_spinner};

pub(super) fn request_cancel_download(app: &mut App) {
    if let Some(flag) = app.download_cancel_flag.as_ref() {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        app.status = "已请求停止下载…".to_string();
        app.push_message("已发送停止信号，稍后结束当前任务");
    } else {
        app.status = "当前没有正在进行的下载".to_string();
    }
    app.stop_button_area = None;
}

pub(super) fn start_download_task(
    app: &mut App,
    pending: PendingDownload,
    range: Option<ChapterRange>,
) -> Result<()> {
    // keep pending info for preview overlay while download runs
    app.pending_download = Some(pending.clone());
    app.preview_modal_open = false;
    app.preview_desc_scroll = 0;
    app.preview_desc_scroll_max = 0;
    app.preview_modal_scroll = 0;
    app.preview_modal_scroll_max = 0;
    app.last_preview_desc_area = None;
    app.download_progress = Some(ProgressSnapshot {
        group_done: 0,
        group_total: pending.plan.chapters.len().div_ceil(25),
        saved_chapters: pending.downloaded_count,
        chapter_total: pending.plan.chapters.len(),
        save_phase: SavePhase::TextSave,
        comment_fetch: 0,
        comment_total: if app.config.enable_segment_comments {
            pending.plan.chapters.len()
        } else {
            0
        },
        comment_saved: 0,
    });
    app.messages.clear();
    app.results.clear();
    app.list_state.select(None);
    app.cover_lines.clear();
    app.cover_title.clear();

    let book_id = pending.plan.book_id.clone();
    let title = pending
        .plan
        .meta
        .book_name
        .clone()
        .unwrap_or_else(|| book_id.clone());

    app.status = format!("开始下载: 《{}》 ({})", title, book_id);
    info!(target: "ui", book_id = %book_id, "启动下载任务");
    debug!(
        target: "ui",
        book_id = %book_id,
        save_path = %app.config.save_path,
        format = %app.config.novel_format,
        workers = app.config.max_workers,
        "下载参数"
    );

    start_spinner(app, format!("下载中: {book_id}"));
    let tx = app.worker_tx.clone();
    let progress_tx = app.worker_tx.clone();
    let cfg = app.config.clone();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    app.download_cancel_flag = Some(cancel_flag.clone());
    app.stop_button_area = None;
    thread::spawn(move || {
        let progress_cb = move |snap: ProgressSnapshot| {
            let _ = progress_tx.send(WorkerMsg::DownloadProgress(snap));
        };
        let result = downloader::download_with_plan(
            &cfg,
            pending.plan,
            range,
            Some(Box::new(progress_cb)),
            Some(cancel_flag),
        );
        let msg = WorkerMsg::DownloadDone { book_id, result };
        let _ = tx.send(msg);
    });
    Ok(())
}

pub(super) fn apply_download_progress(app: &mut App, snap: ProgressSnapshot) {
    app.download_progress = Some(snap);
}

pub(super) fn apply_download_done(app: &mut App, book_id: String, result: Result<()>) {
    match result {
        Ok(()) => {
            app.status = format!("下载完成: {book_id}");
            app.push_message("下载完成");
            info!(target: "ui", book_id = %book_id, "下载完成");
            app.pending_download = None;
            app.preview_range.clear();
            app.preview_buttons.select(Some(0));
            app.preview_modal_open = false;
            app.download_progress = None;
            app.view = View::Home;
            app.focus = Focus::Input;
            app.download_cancel_flag = None;
            app.stop_button_area = None;
        }
        Err(err) => {
            app.status = format!("下载失败: {err}");
            app.push_message(format!("下载失败: {err}"));
            warn!(target: "ui", book_id = %book_id, "下载失败: {err}");
            app.pending_download = None;
            app.preview_range.clear();
            app.preview_buttons.select(Some(0));
            app.preview_modal_open = false;
            app.download_progress = None;
            app.view = View::Home;
            app.focus = Focus::Input;
            app.download_cancel_flag = None;
            app.stop_button_area = None;
        }
    }
}
