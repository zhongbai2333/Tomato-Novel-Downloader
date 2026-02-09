//! 进度上报与 CLI 进度条管理。

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use super::models::ChapterRef;
use super::models::{ProgressSnapshot, SavePhase};
use crate::base_system::context::Config;

struct CliBars {
    _mp: MultiProgress,
    download_bar: ProgressBar,
    save_bar: ProgressBar,
}

pub(crate) struct ProgressReporter {
    pub(crate) snapshot: ProgressSnapshot,
    pub(crate) cb: Option<Box<dyn FnMut(ProgressSnapshot) + Send>>, // optional UI callback
    cli: Option<CliBars>,
}

impl ProgressReporter {
    pub(crate) fn emit(&mut self) {
        if let Some(cb) = self.cb.as_mut() {
            cb(self.snapshot);
        }
    }

    pub(crate) fn inc_group(&mut self) {
        self.snapshot.group_done += 1;
        self.emit();
    }

    pub(crate) fn inc_saved(&mut self) {
        self.snapshot.saved_chapters += 1;
        self.emit();
    }

    pub(crate) fn set_save_phase(&mut self, phase: SavePhase) {
        self.snapshot.save_phase = phase;
        self.emit();
    }

    pub(crate) fn reset_save_progress(&mut self, total: usize) {
        self.snapshot.saved_chapters = 0;
        self.snapshot.chapter_total = total;
        self.emit();
    }

    pub(crate) fn inc_save_progress(&mut self) {
        if self.snapshot.chapter_total == 0 {
            return;
        }
        self.snapshot.saved_chapters =
            (self.snapshot.saved_chapters + 1).min(self.snapshot.chapter_total);
        self.emit();
    }

    pub(crate) fn inc_comment_fetch(&mut self) {
        if self.snapshot.comment_total == 0 {
            return;
        }
        self.snapshot.comment_fetch =
            (self.snapshot.comment_fetch + 1).min(self.snapshot.comment_total);
        self.emit();
    }

    pub(crate) fn inc_comment_saved(&mut self) {
        if self.snapshot.comment_total == 0 {
            return;
        }
        self.snapshot.comment_saved =
            (self.snapshot.comment_saved + 1).min(self.snapshot.comment_total);
        self.emit();
    }

    pub(crate) fn reset_for_retry(&mut self, total: usize, pending_len: usize) {
        self.snapshot.group_done = 0;
        self.snapshot.group_total = pending_len.div_ceil(25);
        self.snapshot.saved_chapters = total.saturating_sub(pending_len);
        self.snapshot.chapter_total = total;
        self.snapshot.save_phase = SavePhase::TextSave;
        self.emit();

        if let Some(cli) = self.cli.as_ref() {
            cli.download_bar
                .set_length(self.snapshot.group_total as u64);
            cli.download_bar.set_position(0);
            cli.save_bar.set_length(pending_len as u64);
            cli.save_bar.set_position(0);
        }
    }

    pub(crate) fn cli_download_bar(&self) -> Option<ProgressBar> {
        self.cli.as_ref().map(|c| c.download_bar.clone())
    }

    pub(crate) fn cli_save_bar(&self) -> Option<ProgressBar> {
        self.cli.as_ref().map(|c| c.save_bar.clone())
    }

    pub(crate) fn finish_cli_bars(&mut self) {
        let Some(cli) = self.cli.take() else {
            return;
        };
        cli.download_bar.finish_and_clear();
        cli.save_bar.finish_and_clear();
        drop(cli);
    }

    pub(crate) fn has_ui_callback(&self) -> bool {
        self.cb.is_some()
    }
}

// ── 构造函数 ──────────────────────────────────────────────────

pub(crate) fn segment_enabled(cfg: &Config) -> bool {
    #[cfg(feature = "official-api")]
    {
        cfg.enable_segment_comments && cfg.novel_format.eq_ignore_ascii_case("epub")
    }
    #[cfg(not(feature = "official-api"))]
    {
        let _ = cfg;
        false
    }
}

pub(crate) fn make_reporter(
    config: &Config,
    chosen: &[ChapterRef],
    pending: &[ChapterRef],
    progress: Option<Box<dyn FnMut(ProgressSnapshot) + Send>>,
) -> ProgressReporter {
    let total = chosen.len();
    let group_total = pending.len().div_ceil(25);

    let use_cli_bars = progress.is_none()
        && config.use_official_api
        && config.max_workers.max(1) <= 1
        && !pending.is_empty();

    let cli = if use_cli_bars {
        let mp = MultiProgress::with_draw_target(ProgressDrawTarget::stderr());
        let style = ProgressStyle::with_template(
            "{prefix} [{elapsed_precise}] {wide_bar} {pos}/{len} ({eta})",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("##-");

        let download_bar = mp.add(ProgressBar::new(group_total as u64));
        download_bar.set_style(style.clone());
        download_bar.set_prefix("章节下载");

        let save_bar = mp.add(ProgressBar::new(pending.len() as u64));
        save_bar.set_style(style);
        save_bar.set_prefix("正文保存");

        Some(CliBars {
            _mp: mp,
            download_bar,
            save_bar,
        })
    } else {
        None
    };

    let mut reporter = ProgressReporter {
        snapshot: ProgressSnapshot {
            group_done: 0,
            group_total,
            saved_chapters: total.saturating_sub(pending.len()),
            chapter_total: total,
            save_phase: SavePhase::TextSave,
            comment_fetch: 0,
            comment_total: if segment_enabled(config) { total } else { 0 },
            comment_saved: 0,
        },
        cb: progress,
        cli,
    };
    reporter.emit();
    reporter
}
