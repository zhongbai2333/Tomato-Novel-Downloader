use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::AtomicBool,
    mpsc::{self, Receiver, Sender},
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::execute as crossterm_execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::prelude::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use serde_json::Value;
use tomato_novel_official_api::{DirectoryClient, SearchClient};
use tracing::{debug, info, warn};

mod about;
mod config;
mod cover;
mod home;
mod update;

use update::{expected_book_folder, read_downloaded_count, show_update_menu};

use crate::base_system::config::{ConfigSpec, write_with_comments};
use crate::base_system::context::{Config, safe_fs_name};
use crate::base_system::json_extract;
use crate::base_system::logging::take_broadcast_rx;
use crate::download::downloader::{self, BookMeta, ChapterRange, DownloadPlan, ProgressSnapshot};
use crate::prewarm_state;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Input,
    Menu,
    Results,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Home,
    Config,
    Update,
    About,
    Cover,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuAction {
    Confirm,
    Config,
    Update,
    About,
    Quit,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ConfigField {
    SavePath,
    NovelFormat,
    BulkFiles,
    AutoClearDump,
    OldCli,
    FirstLineIndentEm,
    EnableSegmentComments,
    UseOfficialApi,
    ApiEndpoints,
    MaxWorkers,
    RequestTimeout,
    MaxRetries,
    MinConnectTimeout,
    ForceExitTimeout,
    GracefulExit,
    MinWait,
    MaxWait,
    EnableAudiobook,
    AudiobookVoice,
    AudiobookRate,
    AudiobookVolume,
    AudiobookPitch,
    AudiobookFormat,
    AudiobookConcurrency,
    SegmentCommentsTopN,
    SegmentCommentsWorkers,
    DownloadCommentImages,
    DownloadCommentAvatars,
    MediaDownloadWorkers,
    BlockedMediaDomains,
    ForceConvertImagesToJpeg,
    JpegRetryConvert,
    JpegQuality,
    ConvertHeicToJpeg,
    KeepHeicOriginal,
    MediaLimitPerChapter,
    MediaMaxDimensionPx,
    MediaTotalLimitMb,
}

#[derive(Debug, Clone)]
pub(super) struct ConfigEntry {
    title: &'static str,
    field: ConfigField,
}

#[derive(Debug, Clone)]
struct ConfigCategory {
    title: &'static str,
    entries: Vec<ConfigEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigFocus {
    Category,
    Entry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewFocus {
    Range,
    Buttons,
}

fn build_config_categories() -> Vec<ConfigCategory> {
    vec![
        ConfigCategory {
            title: "基础与格式",
            entries: vec![
                ConfigEntry {
                    title: "保存路径",
                    field: ConfigField::SavePath,
                },
                ConfigEntry {
                    title: "小说格式(txt/epub)",
                    field: ConfigField::NovelFormat,
                },
                ConfigEntry {
                    title: "首行缩进(em)",
                    field: ConfigField::FirstLineIndentEm,
                },
                ConfigEntry {
                    title: "散装文件保存",
                    field: ConfigField::BulkFiles,
                },
                ConfigEntry {
                    title: "自动清理缓存",
                    field: ConfigField::AutoClearDump,
                },
                ConfigEntry {
                    title: "旧版 CLI UI",
                    field: ConfigField::OldCli,
                },
            ],
        },
        ConfigCategory {
            title: "网络与调度",
            entries: vec![
                ConfigEntry {
                    title: "最大线程数",
                    field: ConfigField::MaxWorkers,
                },
                ConfigEntry {
                    title: "请求超时(s)",
                    field: ConfigField::RequestTimeout,
                },
                ConfigEntry {
                    title: "最大重试次数",
                    field: ConfigField::MaxRetries,
                },
                ConfigEntry {
                    title: "最小连接超时(s)",
                    field: ConfigField::MinConnectTimeout,
                },
                ConfigEntry {
                    title: "强制退出等待(s)",
                    field: ConfigField::ForceExitTimeout,
                },
                ConfigEntry {
                    title: "优雅退出",
                    field: ConfigField::GracefulExit,
                },
                ConfigEntry {
                    title: "最小等待时间(ms)",
                    field: ConfigField::MinWait,
                },
                ConfigEntry {
                    title: "最大等待时间(ms)",
                    field: ConfigField::MaxWait,
                },
            ],
        },
        ConfigCategory {
            title: "API",
            entries: vec![
                ConfigEntry {
                    title: "使用官方API",
                    field: ConfigField::UseOfficialApi,
                },
                ConfigEntry {
                    title: "API 列表(逗号分隔)",
                    field: ConfigField::ApiEndpoints,
                },
            ],
        },
        ConfigCategory {
            title: "段评",
            entries: vec![
                ConfigEntry {
                    title: "启用段评",
                    field: ConfigField::EnableSegmentComments,
                },
                ConfigEntry {
                    title: "每段评论数上限",
                    field: ConfigField::SegmentCommentsTopN,
                },
                ConfigEntry {
                    title: "段评并发线程数",
                    field: ConfigField::SegmentCommentsWorkers,
                },
            ],
        },
        ConfigCategory {
            title: "媒体下载",
            entries: vec![
                ConfigEntry {
                    title: "下载评论图片",
                    field: ConfigField::DownloadCommentImages,
                },
                ConfigEntry {
                    title: "下载评论头像",
                    field: ConfigField::DownloadCommentAvatars,
                },
                ConfigEntry {
                    title: "媒体下载线程数",
                    field: ConfigField::MediaDownloadWorkers,
                },
                ConfigEntry {
                    title: "阻止的图片域名",
                    field: ConfigField::BlockedMediaDomains,
                },
                ConfigEntry {
                    title: "强制转成 JPEG",
                    field: ConfigField::ForceConvertImagesToJpeg,
                },
                ConfigEntry {
                    title: "失败重试再转 JPEG",
                    field: ConfigField::JpegRetryConvert,
                },
                ConfigEntry {
                    title: "JPEG 质量(0-100)",
                    field: ConfigField::JpegQuality,
                },
                ConfigEntry {
                    title: "HEIC 转 JPEG",
                    field: ConfigField::ConvertHeicToJpeg,
                },
                ConfigEntry {
                    title: "保留 HEIC 原图",
                    field: ConfigField::KeepHeicOriginal,
                },
                ConfigEntry {
                    title: "单章节媒体上限",
                    field: ConfigField::MediaLimitPerChapter,
                },
                ConfigEntry {
                    title: "媒体最大尺寸(px)",
                    field: ConfigField::MediaMaxDimensionPx,
                },
                ConfigEntry {
                    title: "媒体总体积上限(MB)",
                    field: ConfigField::MediaTotalLimitMb,
                },
            ],
        },
        ConfigCategory {
            title: "有声书",
            entries: vec![
                ConfigEntry {
                    title: "启用有声书",
                    field: ConfigField::EnableAudiobook,
                },
                ConfigEntry {
                    title: "发音人",
                    field: ConfigField::AudiobookVoice,
                },
                ConfigEntry {
                    title: "语速调整",
                    field: ConfigField::AudiobookRate,
                },
                ConfigEntry {
                    title: "音量调整",
                    field: ConfigField::AudiobookVolume,
                },
                ConfigEntry {
                    title: "音调调整",
                    field: ConfigField::AudiobookPitch,
                },
                ConfigEntry {
                    title: "输出格式(mp3/wav)",
                    field: ConfigField::AudiobookFormat,
                },
                ConfigEntry {
                    title: "并发生成章节数",
                    field: ConfigField::AudiobookConcurrency,
                },
            ],
        },
    ]
}

#[derive(Debug, Clone)]
struct UpdateEntry {
    book_id: String,
    book_name: String,
    folder: PathBuf,
    label: String,
    _new_count: usize,
    _has_update: bool,
}

#[derive(Debug)]
enum WorkerMsg {
    SearchDone(Result<Vec<SearchItem>>),
    DownloadDone { book_id: String, result: Result<()> },
    DownloadProgress(ProgressSnapshot),
    PreviewReady(Result<PendingDownload>),
    UpdateScanned(Result<(Vec<UpdateEntry>, Vec<UpdateEntry>)>),
}

#[derive(Clone, Debug)]
struct SearchItem {
    title: String,
    author: String,
    book_id: String,
    detail: Option<BookDetail>,
}

#[derive(Clone, Debug, Default)]
struct BookDetail {
    description: Option<String>,
    tags: Vec<String>,
    chapter_count: Option<usize>,
    finished: Option<bool>,
    cover_url: Option<String>,
    detail_cover_url: Option<String>,
    word_count: Option<usize>,
    score: Option<f32>,
    read_count: Option<String>,
    read_count_text: Option<String>,
    book_short_name: Option<String>,
    original_book_name: Option<String>,
    first_chapter_title: Option<String>,
    last_chapter_title: Option<String>,
    category: Option<String>,
    cover_primary_color: Option<String>,
}

impl BookDetail {
    fn has_data(&self) -> bool {
        self.description.is_some()
            || !self.tags.is_empty()
            || self.chapter_count.is_some()
            || self.finished.is_some()
            || self.cover_url.is_some()
            || self.detail_cover_url.is_some()
            || self.word_count.is_some()
            || self.score.is_some()
            || self.read_count.is_some()
            || self.read_count_text.is_some()
            || self.book_short_name.is_some()
            || self.original_book_name.is_some()
            || self.first_chapter_title.is_some()
            || self.last_chapter_title.is_some()
            || self.category.is_some()
            || self.cover_primary_color.is_some()
    }
}

#[derive(Clone, Debug)]
pub(super) struct PendingDownload {
    plan: DownloadPlan,
    downloaded_count: usize,
}

pub(super) struct App {
    input: String,
    focus: Focus,
    status: String,
    messages: Vec<String>,
    logs: Vec<String>,
    results: Vec<SearchItem>,
    list_state: ListState,
    config: Config,
    should_quit: bool,
    view: View,
    previous_view: View,
    menu_state: ListState,

    // config state
    cfg_categories: Vec<ConfigCategory>,
    cfg_cat_state: ListState,
    cfg_entry_state: ListState,
    cfg_button_state: ListState,
    cfg_focus: ConfigFocus,
    cfg_editing: Option<(usize, usize)>,
    cfg_edit_buffer: String,
    last_config_layout: Option<[Rect; 3]>,
    last_config_button: Option<Rect>,

    // update state
    update_entries: Vec<UpdateEntry>,
    update_no_updates: Vec<UpdateEntry>,
    update_state: ListState,
    show_no_update: bool,
    last_update_layout: Option<[Rect; 3]>,
    last_update_exit_button: Option<Rect>,

    // about state
    about_btn_state: ListState,
    last_about_buttons: Option<Rect>,

    // cover state
    cover_lines: Vec<String>,
    cover_title: String,
    _previous_view_cover: View,

    // home layout
    last_home_layout: Option<[Rect; 5]>,

    // worker
    worker_tx: Sender<WorkerMsg>,
    worker_rx: Receiver<WorkerMsg>,

    // spinner
    spinner_active: bool,
    spinner_text: String,
    spinner_idx: usize,
    spinner_last: Instant,

    // download
    pending_download: Option<PendingDownload>,

    // log
    log_rx: Option<crossbeam_channel::Receiver<String>>,

    // iid
    iid_prewarm_active: bool,
    prewarm_spinner_idx: usize,
    prewarm_spinner_last: Instant,

    // preview modal
    preview_focus: PreviewFocus,
    preview_buttons: ListState,
    preview_range: String,
    preview_modal_open: bool,

    // preview layout cache (for mouse)
    last_preview_layout: Option<[Rect; 2]>,
    last_preview_modal: Option<PreviewModalLayout>,

    // preview scroll (description/info)
    preview_desc_scroll: u16,
    preview_desc_scroll_max: u16,
    preview_modal_scroll: u16,
    preview_modal_scroll_max: u16,
    last_preview_desc_area: Option<Rect>,

    // download progress
    download_progress: Option<ProgressSnapshot>,

    // download cancel
    download_cancel_flag: Option<Arc<AtomicBool>>,
    stop_button_area: Option<Rect>,
}
#[derive(Clone, Debug, Default)]
struct PreviewModalLayout {
    _modal: Rect,
    info: Rect,
    range: Rect,
    buttons: Rect,
}

impl App {
    fn new(config: Config, worker_tx: Sender<WorkerMsg>, worker_rx: Receiver<WorkerMsg>) -> Self {
        let mut menu_state = ListState::default();
        menu_state.select(Some(0));

        let mut update_state = ListState::default();
        update_state.select(None);

        let mut about_btn_state = ListState::default();
        about_btn_state.select(Some(0));

        let cfg_categories = build_config_categories();
        let mut cfg_cat_state = ListState::default();
        cfg_cat_state.select(Some(0));
        let mut cfg_entry_state = ListState::default();
        cfg_entry_state.select(Some(0));
        let mut cfg_button_state = ListState::default();
        cfg_button_state.select(None);
        let mut preview_buttons = ListState::default();
        preview_buttons.select(Some(0));

        Self {
            input: String::new(),
            focus: Focus::Input,
            status: "输入书名/ID/链接，Enter 确认，Tab 切换焦点，q 退出".to_string(),
            messages: Vec::new(),
            logs: Vec::new(),
            results: Vec::new(),
            list_state: ListState::default(),
            config,
            should_quit: false,
            view: View::Home,
            previous_view: View::Home,
            menu_state,
            cfg_categories,
            cfg_cat_state,
            cfg_entry_state,
            cfg_button_state,
            cfg_focus: ConfigFocus::Entry,
            cfg_editing: None,
            cfg_edit_buffer: String::new(),
            last_config_layout: None,
            last_config_button: None,
            update_entries: Vec::new(),
            update_no_updates: Vec::new(),
            update_state,
            show_no_update: false,
            last_update_layout: None,
            last_update_exit_button: None,
            about_btn_state,
            last_about_buttons: None,
            cover_lines: Vec::new(),
            cover_title: String::new(),
            _previous_view_cover: View::Home,
            last_home_layout: None,
            worker_tx,
            worker_rx,
            spinner_active: false,
            spinner_text: String::new(),
            spinner_idx: 0,
            spinner_last: Instant::now(),
            pending_download: None,
            log_rx: take_broadcast_rx(),
            iid_prewarm_active: prewarm_state::is_prewarm_in_progress(),
            prewarm_spinner_idx: 0,
            prewarm_spinner_last: Instant::now(),
            preview_focus: PreviewFocus::Range,
            preview_buttons,
            preview_range: String::new(),
            preview_modal_open: false,
            last_preview_layout: None,
            last_preview_modal: None,
            preview_desc_scroll: 0,
            preview_desc_scroll_max: 0,
            preview_modal_scroll: 0,
            preview_modal_scroll_max: 0,
            last_preview_desc_area: None,
            download_progress: None,
            download_cancel_flag: None,
            stop_button_area: None,
        }
    }

    fn push_message(&mut self, msg: impl Into<String>) {
        self.messages.push(msg.into());
        if self.messages.len() > 8 {
            let overflow = self.messages.len() - 8;
            self.messages.drain(0..overflow);
        }
    }

    fn push_log(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        let trimmed = msg.trim_end_matches(['\r', '\n']);
        self.logs.push(trimmed.to_string());
        if self.logs.len() > 200 {
            let overflow = self.logs.len() - 200;
            self.logs.drain(0..overflow);
        }
    }

    fn select_next(&mut self) {
        if self.results.is_empty() {
            self.list_state.select(None);
            return;
        }
        let next = match self.list_state.selected() {
            Some(idx) if idx + 1 < self.results.len() => idx + 1,
            _ => 0,
        };
        self.list_state.select(Some(next));
    }

    fn select_prev(&mut self) {
        if self.results.is_empty() {
            self.list_state.select(None);
            return;
        }
        let prev = match self.list_state.selected() {
            Some(0) | None => self.results.len().saturating_sub(1),
            Some(idx) => idx - 1,
        };
        self.list_state.select(Some(prev));
    }
}

pub fn run(config: Config) -> Result<()> {
    let (worker_tx, worker_rx) = mpsc::channel();
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
    crossterm_execute!(stdout, EnableMouseCapture).context("enable mouse capture")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("init terminal")?;

    let result = run_loop(&mut terminal, config, worker_tx, worker_rx);

    disable_raw_mode().ok();
    crossterm_execute!(terminal.backend_mut(), DisableMouseCapture).ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    config: Config,
    worker_tx: Sender<WorkerMsg>,
    worker_rx: Receiver<WorkerMsg>,
) -> Result<()> {
    let mut app = App::new(config, worker_tx, worker_rx);

    loop {
        tick_spinner(&mut app);
        tick_prewarm_spinner(&mut app);
        poll_worker(&mut app)?;
        drain_log_channel(&mut app);
        sync_prewarm_state(&mut app);

        terminal.draw(|f| {
            draw_ui(f, &mut app);
            render_prewarm_overlay(f, &app);
        })?;

        if !handle_event(&mut app)? {
            break;
        }
    }

    Ok(())
}

fn draw_ui(frame: &mut ratatui::Frame, app: &mut App) {
    match app.view {
        View::Home => home::draw_home(frame, app),
        View::Config => config::draw_config(frame, app),
        View::Update => update::draw_update(frame, app),
        View::About => about::draw_about(frame, app),
        View::Cover => cover::draw_cover(frame, app),
        View::Preview => draw_preview(frame, app),
    }
}

fn handle_event(app: &mut App) -> Result<bool> {
    if !event::poll(Duration::from_millis(200)).context("poll event")? {
        return Ok(true);
    }

    let evt = event::read().context("read event")?;
    match app.view {
        View::Home => home::handle_event_home(app, evt)?,
        View::Config => config::handle_event_config(app, evt)?,
        View::Update => update::handle_event_update(app, evt)?,
        View::About => about::handle_event_about(app, evt)?,
        View::Cover => cover::handle_event_cover(app, evt)?,
        View::Preview => handle_event_preview(app, evt)?,
    }

    Ok(!app.should_quit)
}

fn handle_event_preview(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Paste(s) => {
            if app.preview_focus == PreviewFocus::Range {
                app.preview_range.push_str(&s);
            }
        }
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                cancel_preview(app);
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                request_cancel_download(app);
            }
            KeyCode::Tab => {
                app.preview_focus = match app.preview_focus {
                    PreviewFocus::Range => PreviewFocus::Buttons,
                    PreviewFocus::Buttons => PreviewFocus::Range,
                };
            }
            KeyCode::Backspace => {
                if app.preview_focus == PreviewFocus::Range {
                    app.preview_range.pop();
                }
            }
            KeyCode::Enter => {
                if app.preview_focus == PreviewFocus::Range {
                    confirm_preview(app)?;
                } else {
                    match app.preview_buttons.selected().unwrap_or(0) {
                        0 => confirm_preview(app)?,
                        _ => cancel_preview(app),
                    }
                }
            }
            KeyCode::Up => {
                if app.preview_focus == PreviewFocus::Buttons {
                    let sel = app.preview_buttons.selected().unwrap_or(0);
                    let next = if sel == 0 { 1 } else { sel - 1 };
                    app.preview_buttons.select(Some(next.min(1)));
                } else {
                    preview_scroll_up(app, 1);
                }
            }
            KeyCode::Down => {
                if app.preview_focus == PreviewFocus::Buttons {
                    let sel = app.preview_buttons.selected().unwrap_or(0);
                    let next = if sel >= 1 { 0 } else { sel + 1 };
                    app.preview_buttons.select(Some(next.min(1)));
                } else {
                    preview_scroll_down(app, 1);
                }
            }
            KeyCode::PageUp => preview_scroll_up(app, 5),
            KeyCode::PageDown => preview_scroll_down(app, 5),
            KeyCode::Home => preview_scroll_to_top(app),
            KeyCode::End => preview_scroll_to_bottom(app),
            KeyCode::Left => {
                if app.preview_focus == PreviewFocus::Buttons {
                    let sel = app.preview_buttons.selected().unwrap_or(0);
                    let next = if sel == 0 { 1 } else { sel - 1 };
                    app.preview_buttons.select(Some(next.min(1)));
                }
            }
            KeyCode::Right => {
                if app.preview_focus == PreviewFocus::Buttons {
                    let sel = app.preview_buttons.selected().unwrap_or(0);
                    let next = if sel >= 1 { 0 } else { sel + 1 };
                    app.preview_buttons.select(Some(next.min(1)));
                }
            }
            KeyCode::Char(c)
                if app.preview_focus == PreviewFocus::Range
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                app.preview_range.push(c);
            }
            _ => {}
        },
        Event::Resize(_, _) => {}
        Event::Mouse(me) => handle_mouse_preview(app, me)?,
        _ => {}
    }
    Ok(())
}

fn handle_mouse_preview(app: &mut App, me: event::MouseEvent) -> Result<()> {
    let pos_in = |area: Rect, col: u16, row: u16| {
        col >= area.x
            && col < area.x.saturating_add(area.width)
            && row >= area.y
            && row < area.y.saturating_add(area.height)
    };

    if let Some(stop_area) = app.stop_button_area {
        if matches!(me.kind, MouseEventKind::Down(MouseButton::Left))
            && pos_in(stop_area, me.column, me.row)
        {
            request_cancel_download(app);
            return Ok(());
        }
    }

    if matches!(
        me.kind,
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
    ) {
        let up = matches!(me.kind, MouseEventKind::ScrollUp);
        if let Some(layout) = app.last_preview_modal.clone() {
            if pos_in(layout.info, me.column, me.row) {
                if up {
                    preview_scroll_up(app, 1);
                } else {
                    preview_scroll_down(app, 1);
                }
                return Ok(());
            }
        }
        if let Some(area) = app.last_preview_desc_area {
            if pos_in(area, me.column, me.row) {
                if up {
                    preview_scroll_up(app, 1);
                } else {
                    preview_scroll_down(app, 1);
                }
                return Ok(());
            }
        }
    }

    if let Some(layout) = app.last_preview_modal.clone() {
        if matches!(me.kind, MouseEventKind::Down(MouseButton::Left)) {
            if pos_in(layout.range, me.column, me.row) {
                app.preview_focus = PreviewFocus::Range;
                return Ok(());
            }
            if pos_in(layout.buttons, me.column, me.row) {
                let idx = me.row.saturating_sub(layout.buttons.y + 1) as usize;
                let picked = idx.min(1);
                app.preview_buttons.select(Some(picked));
                if picked == 0 {
                    confirm_preview(app)?;
                } else {
                    cancel_preview(app);
                }
                return Ok(());
            }
        }
        if matches!(me.kind, MouseEventKind::Moved) {
            if pos_in(layout.buttons, me.column, me.row) {
                let idx = me.row.saturating_sub(layout.buttons.y + 1) as usize;
                app.preview_buttons.select(Some(idx.min(1)));
                app.preview_focus = PreviewFocus::Buttons;
                return Ok(());
            }
            if pos_in(layout.range, me.column, me.row) {
                app.preview_focus = PreviewFocus::Range;
                return Ok(());
            }
        }
    }

    Ok(())
}

fn request_cancel_download(app: &mut App) {
    if let Some(flag) = app.download_cancel_flag.as_ref() {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        app.status = "已请求停止下载…".to_string();
        app.push_message("已发送停止信号，稍后结束当前任务");
    } else {
        app.status = "当前没有正在进行的下载".to_string();
    }
    app.stop_button_area = None;
}

fn search_books(query: &str) -> Result<Vec<SearchItem>> {
    let client = SearchClient::new().context("init SearchClient")?;
    let resp = client.search_books(query).context("search_books")?;
    let mut results = Vec::new();
    for book in resp.books {
        let title = book.title.unwrap_or_default();
        let author = book.author.unwrap_or_default();
        let detail = detail_from_search(&book.raw);
        let detail = if detail.has_data() {
            Some(detail)
        } else {
            None
        };

        results.push(SearchItem {
            title,
            author,
            book_id: book.book_id,
            detail,
        });
    }
    Ok(results)
}

fn detail_from_search(raw: &Value) -> BookDetail {
    let maps = json_extract::collect_maps(raw);

    let description = maps.iter().find_map(|m| {
        json_extract::pick_string(
            m,
            &[
                "abstract",
                "desc",
                "description",
                "brief",
                "intro",
                "summary",
                "recommendation_reason",
                "book_abstract",
            ],
        )
    });
    let tags = maps
        .iter()
        .find_map(|m| json_extract::pick_tags_opt(m))
        .unwrap_or_default();
    let cover_url = maps.iter().find_map(|m| json_extract::pick_cover(m));
    let detail_cover_url = maps.iter().find_map(|m| json_extract::pick_detail_cover(m));
    let word_count = maps.iter().find_map(|m| json_extract::pick_word_count(m));
    let score = maps.iter().find_map(|m| json_extract::pick_score(m));
    let read_count = maps.iter().find_map(|m| json_extract::pick_read_count(m));
    let read_count_text = maps
        .iter()
        .find_map(|m| json_extract::pick_read_count_text(m));
    let book_short_name = maps
        .iter()
        .find_map(|m| json_extract::pick_book_short_name(m));
    let original_book_name = maps
        .iter()
        .find_map(|m| json_extract::pick_original_book_name(m));
    let first_chapter_title = maps
        .iter()
        .find_map(|m| json_extract::pick_first_chapter_title(m));
    let last_chapter_title = maps
        .iter()
        .find_map(|m| json_extract::pick_last_chapter_title(m));
    let category = maps.iter().find_map(|m| json_extract::pick_category(m));
    let cover_primary_color = maps
        .iter()
        .find_map(|m| json_extract::pick_cover_primary_color(m));

    BookDetail {
        description,
        tags,
        chapter_count: None,
        finished: None,
        cover_url,
        detail_cover_url,
        word_count,
        score,
        read_count,
        read_count_text,
        book_short_name,
        original_book_name,
        first_chapter_title,
        last_chapter_title,
        category,
        cover_primary_color,
    }
}

fn detail_from_meta(meta: &BookMeta) -> BookDetail {
    BookDetail {
        description: meta.description.clone(),
        tags: meta.tags.clone(),
        chapter_count: meta.chapter_count,
        finished: meta.finished,
        cover_url: meta.cover_url.clone(),
        detail_cover_url: meta.detail_cover_url.clone(),
        word_count: meta.word_count,
        score: meta.score,
        read_count: meta.read_count.clone(),
        read_count_text: meta.read_count_text.clone(),
        book_short_name: meta.book_short_name.clone(),
        original_book_name: meta.original_book_name.clone(),
        first_chapter_title: meta.first_chapter_title.clone(),
        last_chapter_title: meta.last_chapter_title.clone(),
        category: meta.category.clone(),
        cover_primary_color: meta.cover_primary_color.clone(),
    }
}

fn merge_detail(primary: BookDetail, fallback: BookDetail) -> BookDetail {
    BookDetail {
        description: primary.description.or(fallback.description),
        tags: if primary.tags.is_empty() {
            fallback.tags
        } else {
            primary.tags
        },
        chapter_count: primary.chapter_count.or(fallback.chapter_count),
        finished: primary.finished.or(fallback.finished),
        cover_url: primary.cover_url.or(fallback.cover_url),
        detail_cover_url: primary.detail_cover_url.or(fallback.detail_cover_url),
        word_count: primary.word_count.or(fallback.word_count),
        score: primary.score.or(fallback.score),
        read_count: primary.read_count.or(fallback.read_count),
        read_count_text: primary.read_count_text.or(fallback.read_count_text),
        book_short_name: primary.book_short_name.or(fallback.book_short_name),
        original_book_name: primary.original_book_name.or(fallback.original_book_name),
        first_chapter_title: primary.first_chapter_title.or(fallback.first_chapter_title),
        last_chapter_title: primary.last_chapter_title.or(fallback.last_chapter_title),
        category: primary.category.or(fallback.category),
        cover_primary_color: primary.cover_primary_color.or(fallback.cover_primary_color),
    }
}

fn upsert_result_detail_from_plan(app: &mut App, book_id: &str, meta: &BookMeta) {
    let Some(idx) = app.results.iter().position(|b| b.book_id == book_id) else {
        return;
    };
    let incoming = detail_from_meta(meta);
    if let Some(old) = app.results[idx].detail.take() {
        app.results[idx].detail = Some(merge_detail(incoming, old));
    } else {
        app.results[idx].detail = Some(incoming);
    }
}

// JSON 字段提取 helper 已抽取到 base_system::json_extract

pub(super) fn parse_book_id(input: &str) -> Option<String> {
    crate::base_system::book_id::parse_book_id(input)
}

pub(super) fn parse_range_input(input: &str, total: usize) -> Result<Option<ChapterRange>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() > 2 {
        return Err(anyhow!("格式应为 start-end，例如 1-10"));
    }

    let start_part = parts.get(0).copied().unwrap_or("").trim();
    let end_part = parts.get(1).copied().unwrap_or("").trim();

    let start = if start_part.is_empty() {
        1
    } else {
        start_part
            .parse::<usize>()
            .map_err(|_| anyhow!("起始章节需为数字"))?
    };
    let end = if end_part.is_empty() {
        total
    } else {
        end_part
            .parse::<usize>()
            .map_err(|_| anyhow!("结束章节需为数字"))?
    };

    if start == 0 || end == 0 {
        return Err(anyhow!("章节编号需大于 0"));
    }
    if start > end {
        return Err(anyhow!("起始章节不能大于结束章节"));
    }
    if start > total {
        return Err(anyhow!("起始章节超过目录长度"));
    }

    Ok(Some(ChapterRange {
        start,
        end: end.min(total),
    }))
}

const MENU_ITEMS: &[(&str, MenuAction)] = &[
    ("确定", MenuAction::Confirm),
    ("配置", MenuAction::Config),
    ("更新", MenuAction::Update),
    ("关于", MenuAction::About),
    ("退出", MenuAction::Quit),
];

const SPINNER_FRAMES: &[char] = &['|', '/', '-', '\\'];

const LOG_HEIGHT: u16 = 7;

const ABOUT_BUTTONS: &[&str] = &["打开Github仓库", "返回"];

fn current_category(app: &App) -> Option<(usize, &ConfigCategory)> {
    let idx = app.cfg_cat_state.selected()?;
    app.cfg_categories.get(idx).map(|c| (idx, c))
}

pub(super) fn current_cfg_entries(app: &App) -> Option<&[ConfigEntry]> {
    current_category(app).map(|(_, c)| c.entries.as_slice())
}

pub(super) fn ensure_entry_selection(app: &mut App) {
    let mut entry_state = ListState::default();
    if let Some(entries) = current_cfg_entries(app) {
        if !entries.is_empty() {
            let idx = app
                .cfg_entry_state
                .selected()
                .unwrap_or(0)
                .min(entries.len().saturating_sub(1));
            entry_state.select(Some(idx));
        }
    }
    if entry_state.selected().is_some() {
        app.cfg_entry_state = entry_state;
    } else {
        app.cfg_entry_state.select(None);
    }
}

pub(super) fn select_next_category(app: &mut App) {
    if app.cfg_categories.is_empty() {
        app.cfg_cat_state.select(None);
        return;
    }
    let next = app
        .cfg_cat_state
        .selected()
        .map(|i| (i + 1) % app.cfg_categories.len())
        .unwrap_or(0);
    app.cfg_cat_state.select(Some(next));
    ensure_entry_selection(app);
    if let Some((_, cat)) = current_category(app) {
        app.status = format!("当前分类: {}", cat.title);
    }
}

pub(super) fn select_prev_category(app: &mut App) {
    if app.cfg_categories.is_empty() {
        app.cfg_cat_state.select(None);
        return;
    }
    let prev = app
        .cfg_cat_state
        .selected()
        .map(|i| {
            if i == 0 {
                app.cfg_categories.len() - 1
            } else {
                i - 1
            }
        })
        .unwrap_or(0);
    app.cfg_cat_state.select(Some(prev));
    ensure_entry_selection(app);
    if let Some((_, cat)) = current_category(app) {
        app.status = format!("当前分类: {}", cat.title);
    }
}

pub(super) fn select_next_entry(app: &mut App) {
    let Some(entries) = current_cfg_entries(app) else {
        app.cfg_entry_state.select(None);
        return;
    };
    if entries.is_empty() {
        app.cfg_entry_state.select(None);
        return;
    }
    let next = app
        .cfg_entry_state
        .selected()
        .map(|i| (i + 1) % entries.len())
        .unwrap_or(0);
    app.cfg_entry_state.select(Some(next));
}

pub(super) fn select_prev_entry(app: &mut App) {
    let Some(entries) = current_cfg_entries(app) else {
        app.cfg_entry_state.select(None);
        return;
    };
    if entries.is_empty() {
        app.cfg_entry_state.select(None);
        return;
    }
    let prev = app
        .cfg_entry_state
        .selected()
        .map(|i| if i == 0 { entries.len() - 1 } else { i - 1 })
        .unwrap_or(0);
    app.cfg_entry_state.select(Some(prev));
}

pub(super) fn start_spinner(app: &mut App, text: impl Into<String>) {
    app.spinner_active = true;
    app.spinner_text = text.into();
    app.spinner_idx = 0;
    app.spinner_last = Instant::now();
    app.status = format!("{} {}", app.spinner_text, SPINNER_FRAMES[app.spinner_idx]);
}

pub(super) fn stop_spinner(app: &mut App) {
    app.spinner_active = false;
    app.spinner_text.clear();
}

fn tick_spinner(app: &mut App) {
    if !app.spinner_active {
        return;
    }
    if app.spinner_last.elapsed() < Duration::from_millis(140) {
        return;
    }
    app.spinner_idx = (app.spinner_idx + 1) % SPINNER_FRAMES.len();
    app.spinner_last = Instant::now();
    app.status = format!("{} {}", app.spinner_text, SPINNER_FRAMES[app.spinner_idx]);
}

fn split_with_log(area: Rect) -> (Rect, Rect) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(LOG_HEIGHT.max(4)),
            Constraint::Length(LOG_HEIGHT),
        ])
        .split(area);
    let main = layout.get(0).copied().unwrap_or(area);
    let log = layout.get(1).copied().unwrap_or(Rect {
        x: area.x,
        y: area
            .y
            .saturating_add(area.height.saturating_sub(LOG_HEIGHT)),
        width: area.width,
        height: LOG_HEIGHT,
    });
    (main, log)
}

fn render_log_box(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let mut lines = Vec::new();
    if app.logs.is_empty() {
        lines.push(Line::from("日志: 暂无"));
    } else {
        // Fit to visible height (area minus top/bottom borders) so the view auto-sticks to latest entries.
        let visible = area
            .height
            .saturating_sub(2) // account for block borders
            .max(1) as usize;
        lines.extend(
            app.logs
                .iter()
                .rev()
                .take(visible)
                .rev()
                .map(|m| style_log_line(m)),
        );
    }

    let log = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("日志"));
    frame.render_widget(log, area);
}

fn render_prewarm_overlay(frame: &mut ratatui::Frame, app: &App) {
    if !app.iid_prewarm_active {
        return;
    }

    let area = frame.size();
    let width = 28;
    let height = 5;
    let x = area.x.saturating_add(area.width.saturating_sub(width + 1));
    let y = area.y;
    let overlay = Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    };

    let inner = Rect {
        x: overlay.x.saturating_add(1),
        y: overlay.y.saturating_add(1),
        width: overlay.width.saturating_sub(2).max(1),
        height: overlay.height.saturating_sub(2).max(1),
    };

    let spinner = SPINNER_FRAMES[(app.prewarm_spinner_idx) % SPINNER_FRAMES.len()];
    let text = format!(" IID 预热中… {}", spinner);
    let lines = vec![
        Line::from(Span::styled(
            " 初始化",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(text, Style::default().fg(Color::Yellow))),
    ];

    frame.render_widget(Clear, overlay);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("正在预热 IID")
        .title_alignment(Alignment::Right);
    frame.render_widget(block, overlay);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true }),
        inner,
    );
}

fn wrapped_line_count(text: &str, width: u16) -> usize {
    let w = width.max(1) as usize;
    let mut total = 0usize;
    for line in text.lines() {
        let wrapped = textwrap::wrap(line, w);
        total = total.saturating_add(wrapped.len().max(1));
    }
    total.max(1)
}

fn preview_scroll_up(app: &mut App, lines: u16) {
    if app.preview_modal_open {
        app.preview_modal_scroll = app.preview_modal_scroll.saturating_sub(lines);
    } else {
        app.preview_desc_scroll = app.preview_desc_scroll.saturating_sub(lines);
    }
}

fn preview_scroll_down(app: &mut App, lines: u16) {
    if app.preview_modal_open {
        let max = app.preview_modal_scroll_max;
        app.preview_modal_scroll = (app.preview_modal_scroll.saturating_add(lines)).min(max);
    } else {
        let max = app.preview_desc_scroll_max;
        app.preview_desc_scroll = (app.preview_desc_scroll.saturating_add(lines)).min(max);
    }
}

fn preview_scroll_to_top(app: &mut App) {
    if app.preview_modal_open {
        app.preview_modal_scroll = 0;
    } else {
        app.preview_desc_scroll = 0;
    }
}

fn preview_scroll_to_bottom(app: &mut App) {
    if app.preview_modal_open {
        app.preview_modal_scroll = app.preview_modal_scroll_max;
    } else {
        app.preview_desc_scroll = app.preview_desc_scroll_max;
    }
}

fn draw_preview(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.size();
    let progress_height: u16 = 7;
    let log_height = area.height.saturating_sub(progress_height);

    let log_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: log_height.max(1),
    };
    // When downloading, show a small description pane above logs.
    // Keep it minimal: only render if we have enough vertical space.
    let desc_h: u16 = 5;
    let min_log_h: u16 = 3;
    let can_show_desc = !app.preview_modal_open
        && app.pending_download.is_some()
        && log_area.height > desc_h.saturating_add(min_log_h);

    if can_show_desc {
        let desc_area = Rect {
            x: log_area.x,
            y: log_area.y,
            width: log_area.width,
            height: desc_h.min(log_area.height),
        };
        app.last_preview_desc_area = Some(desc_area);
        let log_rest = Rect {
            x: log_area.x,
            y: log_area.y.saturating_add(desc_area.height),
            width: log_area.width,
            height: log_area.height.saturating_sub(desc_area.height).max(1),
        };

        let desc_text = app
            .pending_download
            .as_ref()
            .and_then(|p| p.plan.meta.description.as_deref())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("暂无简介");

        let desc_block = Block::default()
            .borders(Borders::ALL)
            .title("简介 (↑↓/滚轮)");
        frame.render_widget(desc_block.clone(), desc_area);
        let inner = desc_block.inner(desc_area);

        let full_lines = wrapped_line_count(desc_text, inner.width);
        let visible_h = inner.height as usize;
        let scrollable = full_lines > visible_h && inner.width > 1;
        let (text_area, scroll_area, total_lines) = if scrollable {
            let text_w = inner.width.saturating_sub(1).max(1);
            let total = wrapped_line_count(desc_text, text_w);
            (
                Rect {
                    x: inner.x,
                    y: inner.y,
                    width: text_w,
                    height: inner.height,
                },
                Some(Rect {
                    x: inner.x.saturating_add(text_w),
                    y: inner.y,
                    width: 1,
                    height: inner.height,
                }),
                total,
            )
        } else {
            (inner, None, full_lines)
        };

        let max_scroll = total_lines
            .saturating_sub(text_area.height as usize)
            .min(u16::MAX as usize) as u16;
        app.preview_desc_scroll_max = max_scroll;
        app.preview_desc_scroll = app.preview_desc_scroll.min(app.preview_desc_scroll_max);

        let para = Paragraph::new(desc_text.to_string())
            .wrap(Wrap { trim: true })
            .scroll((app.preview_desc_scroll, 0));
        frame.render_widget(para, text_area);
        if let Some(sb_area) = scroll_area {
            let mut state =
                ScrollbarState::new(total_lines).position(app.preview_desc_scroll as usize);
            let sb = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
            frame.render_stateful_widget(sb, sb_area, &mut state);
        }

        render_log_box(frame, log_rest, app);
    } else {
        app.last_preview_desc_area = None;
        app.preview_desc_scroll = 0;
        app.preview_desc_scroll_max = 0;
        render_log_box(frame, log_area, app);
    }
    app.last_preview_layout = Some([log_area, Rect::default()]);

    let progress_area = Rect {
        x: area.x,
        y: area.y.saturating_add(log_area.height),
        width: area.width,
        height: progress_height.min(area.height.saturating_sub(log_area.height)),
    };
    if let Some(layout) = app.last_preview_layout.as_mut() {
        layout[1] = progress_area;
    }
    let empty = ProgressSnapshot::default();
    let snap = app.download_progress.as_ref().unwrap_or(&empty);

    let show_comments = app.config.enable_segment_comments && snap.comment_total > 0;
    let mut items: Vec<(&str, usize, usize, Color)> = Vec::new();
    items.push((
        "组下载",
        snap.group_done,
        snap.group_total.max(1),
        Color::LightCyan,
    ));
    items.push((
        "正文保存",
        snap.saved_chapters,
        snap.chapter_total.max(1),
        Color::Green,
    ));
    if show_comments {
        items.push((
            "段评抓取",
            snap.comment_fetch,
            snap.comment_total.max(1),
            Color::Yellow,
        ));
        items.push((
            "段评保存",
            snap.comment_saved,
            snap.comment_total.max(1),
            Color::Magenta,
        ));
    }

    let inner = Block::default().borders(Borders::ALL).title("进度");
    frame.render_widget(inner.clone(), progress_area);

    let inner_area = Rect {
        x: progress_area.x.saturating_add(1),
        y: progress_area.y.saturating_add(1),
        width: progress_area.width.saturating_sub(2).max(1),
        height: progress_area.height.saturating_sub(2).max(1),
    };

    app.stop_button_area = None;
    if !items.is_empty() && inner_area.height > 0 {
        let mut constraints = Vec::new();
        for _ in &items {
            constraints.push(Constraint::Length(1));
        }
        // extra line for stop button
        constraints.push(Constraint::Length(1));

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner_area);

        for (idx, (label, done, total, color)) in items.into_iter().enumerate() {
            if let Some(area) = rows.get(idx) {
                let ratio = if total == 0 {
                    0.0
                } else {
                    (done as f64 / total as f64).clamp(0.0, 1.0)
                };
                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(color))
                    .ratio(ratio)
                    .label(format!("{label} {done}/{total}"));
                frame.render_widget(gauge, *area);
            }
        }

        if let Some(btn_area) = rows.last() {
            let txt = if app.download_cancel_flag.is_some() {
                "[ 停止下载 ] (S/点击)"
            } else {
                ""
            };
            let para = Paragraph::new(txt)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
            frame.render_widget(para, *btn_area);
            if app.download_cancel_flag.is_some() {
                app.stop_button_area = Some(*btn_area);
            }
        }
    }

    app.last_preview_modal = None;
    if app.preview_modal_open {
        let modal_w = area.width.min(80).max(40.min(area.width));
        let modal_h = 18.min(area.height);
        let modal_x = area
            .x
            .saturating_add(area.width.saturating_sub(modal_w) / 2);
        let modal_y = area
            .y
            .saturating_add(log_area.height.saturating_sub(modal_h) / 2);
        let modal = Rect {
            x: modal_x,
            y: modal_y,
            width: modal_w,
            height: modal_h,
        };

        let inner = Rect {
            x: modal.x.saturating_add(1),
            y: modal.y.saturating_add(1),
            width: modal.width.saturating_sub(2).max(1),
            height: modal.height.saturating_sub(2).max(1),
        };

        let pending = app.pending_download.as_ref();
        let fallback_meta = BookMeta::default();
        let (title, original_title, author, total, downloaded, meta) = pending
            .map(|p| {
                (
                    p.plan
                        .meta
                        .book_name
                        .clone()
                        .unwrap_or_else(|| "预览".to_string()),
                    p.plan.meta.original_book_name.clone(),
                    p.plan.meta.author.clone(),
                    p.plan.chapters.len(),
                    p.downloaded_count,
                    &p.plan.meta,
                )
            })
            .unwrap_or(("预览".to_string(), None, None, 0, 0, &fallback_meta));

        let mut title_line = format!("《{}》", title);
        if let Some(orig) = original_title.as_ref() {
            if !orig.is_empty() {
                title_line.push_str(&format!(" ({})", orig));
            }
        }

        let mut meta_lines: Vec<Line> = Vec::new();
        let mut info_plain_lines: Vec<String> = Vec::new();
        let mut row1: Vec<String> = Vec::new();
        row1.push(format!("章节: {} (已下载 {})", total, downloaded));
        if let Some(done) = meta.finished {
            let label = if done { "完结" } else { "连载" };
            row1.push(format!("状态: {}", label));
        }
        if let Some(author) = author.as_ref() {
            if !author.is_empty() {
                row1.push(format!("作者: {}", author));
            }
        }
        let row1_s = row1.join(" | ");
        meta_lines.push(Line::from(row1_s.clone()));
        info_plain_lines.push(row1_s);

        if let Some(desc) = meta.description.as_ref() {
            if !desc.is_empty() {
                let desc = desc.trim();
                meta_lines.push(Line::from(format!("简介: {}", desc)));
                info_plain_lines.push(format!("简介: {}", desc));
            } else {
                meta_lines.push(Line::from("简介: 暂无"));
                info_plain_lines.push("简介: 暂无".to_string());
            }
        } else {
            meta_lines.push(Line::from("简介: 暂无"));
            info_plain_lines.push("简介: 暂无".to_string());
        }

        let mut row2: Vec<String> = Vec::new();
        if let Some(score) = meta.score {
            row2.push(format!("评分: {:.1}", score));
        }
        if let Some(words) = meta.word_count {
            row2.push(format!("字数: {}", format_word_count(words)));
        }
        if let Some(reads) = meta.read_count_text.as_ref().or(meta.read_count.as_ref()) {
            row2.push(format!("阅读: {}", reads));
        }
        if !row2.is_empty() {
            let row2_s = row2.join(" | ");
            meta_lines.push(Line::from(row2_s.clone()));
            info_plain_lines.push(row2_s);
        }

        let mut row3: Vec<String> = Vec::new();
        if let Some(cat) = meta.category.as_ref() {
            if !cat.is_empty() {
                row3.push(format!("类别: {}", cat));
            }
        }
        if !meta.tags.is_empty() {
            row3.push(format!("标签: {}", meta.tags.join(" | ")));
        }
        if !row3.is_empty() {
            let row3_s = row3.join(" | ");
            meta_lines.push(Line::from(row3_s.clone()));
            info_plain_lines.push(row3_s);
        }

        let mut row4: Vec<String> = Vec::new();
        if let Some(first) = meta.first_chapter_title.as_ref() {
            if !first.is_empty() {
                row4.push(format!("首章: {}", truncate(first, 50)));
            }
        }
        if let Some(last) = meta.last_chapter_title.as_ref() {
            if !last.is_empty() {
                row4.push(format!("末章: {}", truncate(last, 50)));
            }
        }
        if !row4.is_empty() {
            let row4_s = row4.join(" | ");
            meta_lines.push(Line::from(row4_s.clone()));
            info_plain_lines.push(row4_s);
        }

        let mut info_lines = Vec::new();
        info_lines.push(Line::from(Span::styled(
            title_line.clone(),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )));
        info_lines.extend(meta_lines);

        // Plain text copy for scroll range calculation
        let mut info_plain = String::new();
        info_plain.push_str(&title_line);
        for s in &info_plain_lines {
            info_plain.push('\n');
            info_plain.push_str(s);
        }

        let range_focus = app.preview_focus == PreviewFocus::Range;
        let range_style = if range_focus {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let range_line = Paragraph::new(format!("> {}", app.preview_range))
            .style(range_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("下载范围 (空=全部)"),
            );

        let buttons = ["确定", "取消"];
        let button_items: Vec<ListItem> = buttons.iter().map(|b| ListItem::new(*b)).collect();
        let button_style = if app.preview_focus == PreviewFocus::Buttons {
            Style::default().fg(Color::LightCyan)
        } else {
            Style::default()
        };
        let button_list = List::new(button_items)
            .block(Block::default().borders(Borders::ALL).title("操作"))
            .highlight_style(button_style.add_modifier(Modifier::BOLD))
            .highlight_symbol(">> ");

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(6),
                Constraint::Length(3),
                Constraint::Length(4),
            ])
            .split(inner);

        // Scroll range for modal info area (based on chunks[0])
        let full_lines = wrapped_line_count(&info_plain, chunks[0].width);
        let visible_h = chunks[0].height as usize;
        let scrollable = full_lines > visible_h && chunks[0].width > 1;
        let (info_text_area, info_scroll_area, total_lines) = if scrollable {
            let text_w = chunks[0].width.saturating_sub(1).max(1);
            let total = wrapped_line_count(&info_plain, text_w);
            (
                Rect {
                    x: chunks[0].x,
                    y: chunks[0].y,
                    width: text_w,
                    height: chunks[0].height,
                },
                Some(Rect {
                    x: chunks[0].x.saturating_add(text_w),
                    y: chunks[0].y,
                    width: 1,
                    height: chunks[0].height,
                }),
                total,
            )
        } else {
            (chunks[0], None, full_lines)
        };
        let max_scroll = total_lines
            .saturating_sub(info_text_area.height as usize)
            .min(u16::MAX as usize) as u16;
        app.preview_modal_scroll_max = max_scroll;
        app.preview_modal_scroll = app.preview_modal_scroll.min(app.preview_modal_scroll_max);

        frame.render_widget(Clear, modal);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title("预览与下载 (↑↓/滚轮)")
                .title_alignment(Alignment::Center),
            modal,
        );

        let info_para = Paragraph::new(info_lines)
            .wrap(Wrap { trim: true })
            .scroll((app.preview_modal_scroll, 0));
        frame.render_widget(info_para, info_text_area);
        if let Some(sb_area) = info_scroll_area {
            let mut state =
                ScrollbarState::new(total_lines).position(app.preview_modal_scroll as usize);
            let sb = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
            frame.render_stateful_widget(sb, sb_area, &mut state);
        }
        frame.render_widget(range_line, chunks[1]);
        frame.render_stateful_widget(button_list, chunks[2], &mut app.preview_buttons);
        app.last_preview_modal = Some(PreviewModalLayout {
            _modal: modal,
            info: chunks[0],
            range: chunks[1],
            buttons: chunks[2],
        });
    }
}

fn style_log_line(line: &str) -> Line<'static> {
    let mut parts = line.split_whitespace();
    let ts = parts.next().unwrap_or("");
    let level_raw = parts.next().unwrap_or("");
    let level = level_raw.to_ascii_uppercase();
    let rest: Vec<&str> = parts.collect();

    let mut spans: Vec<Span<'static>> = Vec::new();
    if !ts.is_empty() {
        spans.push(Span::styled(
            ts.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    if !level.is_empty() {
        let color = match level.as_str() {
            "ERROR" => Color::Red,
            "WARN" => Color::Yellow,
            "INFO" => Color::Cyan,
            "DEBUG" => Color::Gray,
            "TRACE" => Color::Gray,
            _ => Color::White,
        };
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            level,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    if let Some(target) = rest.first() {
        if !target.is_empty() {
            if !spans.is_empty() {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                (*target).to_string(),
                Style::default().fg(Color::LightBlue),
            ));
        }
    }

    let message = rest.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
    if !message.is_empty() {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(message));
    }

    Line::from(spans)
}

fn drain_log_channel(app: &mut App) {
    if let Some(rx) = app.log_rx.as_ref() {
        let rx = rx.clone();
        for line in rx.try_iter() {
            app.push_log(line);
        }
    }
}

fn sync_prewarm_state(app: &mut App) {
    if app.iid_prewarm_active && !prewarm_state::is_prewarm_in_progress() {
        app.iid_prewarm_active = false;
    }
}

fn tick_prewarm_spinner(app: &mut App) {
    if !app.iid_prewarm_active {
        return;
    }
    if app.prewarm_spinner_last.elapsed() < Duration::from_millis(140) {
        return;
    }
    app.prewarm_spinner_idx = (app.prewarm_spinner_idx + 1) % SPINNER_FRAMES.len();
    app.prewarm_spinner_last = Instant::now();
}

pub(super) fn switch_view(app: &mut App, action: MenuAction) -> Result<()> {
    let idx = MENU_ITEMS.iter().position(|(_, a)| *a == action);
    if let Some(i) = idx {
        app.menu_state.select(Some(i));
    }
    match action {
        MenuAction::Confirm => home::process_input(app)?,
        MenuAction::Config => {
            app.view = View::Config;
            app.status = "进入配置编辑".to_string();
            app.focus = Focus::Input;
        }
        MenuAction::Update => show_update_menu(app)?,
        MenuAction::About => {
            app.view = View::About;
            app.status = "关于".to_string();
        }
        MenuAction::Quit => app.should_quit = true,
    }
    Ok(())
}

pub(super) fn trigger_menu_action(app: &mut App) -> Result<()> {
    let idx = app.menu_state.selected().unwrap_or(0);
    let action = MENU_ITEMS
        .get(idx)
        .map(|(_, a)| *a)
        .unwrap_or(MenuAction::Confirm);
    switch_view(app, action)
}

pub(super) fn start_search_task(app: &mut App, query: String) -> Result<()> {
    info!(target: "ui", "开始搜索: {query}");
    start_spinner(app, "搜索中…");
    let tx = app.worker_tx.clone();
    thread::spawn(move || {
        let result = search_books(&query);
        let _ = tx.send(WorkerMsg::SearchDone(result));
    });
    Ok(())
}

pub(super) fn start_preview_task(app: &mut App, book_id: String, hint: BookMeta) -> Result<()> {
    app.pending_download = None;
    app.messages.clear();
    app.cover_lines.clear();
    app.cover_title.clear();
    app.download_progress = None;
    app.download_cancel_flag = None;
    app.stop_button_area = None;
    app.preview_desc_scroll = 0;
    app.preview_desc_scroll_max = 0;
    app.preview_modal_scroll = 0;
    app.preview_modal_scroll_max = 0;
    app.last_preview_desc_area = None;
    info!(target: "ui", book_id = %book_id, "开始加载目录/预览");
    start_spinner(app, format!("加载目录: {book_id}"));
    let tx = app.worker_tx.clone();
    let cfg = app.config.clone();
    thread::spawn(move || {
        let result = downloader::prepare_download_plan(&cfg, &book_id, hint).map(|plan| {
            let folder = expected_book_folder(&cfg, &plan);
            let downloaded = read_downloaded_count(&folder, &plan.book_id).unwrap_or(0);
            PendingDownload {
                plan,
                downloaded_count: downloaded,
            }
        });
        let _ = tx.send(WorkerMsg::PreviewReady(result));
    });
    Ok(())
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

fn confirm_preview(app: &mut App) -> Result<()> {
    let pending = match app.pending_download.clone() {
        Some(p) => p,
        None => return Ok(()),
    };

    let total = pending.plan.chapters.len();
    let input = app.preview_range.trim();
    let range = if input.is_empty() {
        None
    } else {
        match parse_range_input(input, total) {
            Ok(r) => r,
            Err(err) => {
                app.status = format!("范围无效: {err}");
                return Ok(());
            }
        }
    };

    app.preview_range.clear();
    app.preview_buttons.select(Some(0));
    app.view = View::Preview;
    app.focus = Focus::Input;
    app.input.clear();

    start_download_task(app, pending, range)
}

fn cancel_preview(app: &mut App) {
    // If preview downloaded cover into a fresh folder, clean it up on cancel.
    cleanup_preview_cover_artifacts(app);

    app.pending_download = None;
    app.preview_range.clear();
    app.preview_buttons.select(Some(0));
    app.preview_modal_open = false;
    app.download_progress = None;
    app.preview_desc_scroll = 0;
    app.preview_desc_scroll_max = 0;
    app.preview_modal_scroll = 0;
    app.preview_modal_scroll_max = 0;
    app.last_preview_desc_area = None;
    app.view = View::Home;
    app.focus = Focus::Input;
    app.status = "已取消预览".to_string();
}

fn cleanup_preview_cover_artifacts(app: &mut App) {
    if !app.config.auto_clear_dump {
        return;
    }
    let Some(pending) = app.pending_download.as_ref() else {
        return;
    };
    let Some(book_name) = pending.plan.meta.book_name.as_deref() else {
        return;
    };

    // NOTE: preview currently downloads cover into the same book folder name as status folder.
    // Only delete when the folder contains *only* cover-like files and no status.json.
    let dir = crate::base_system::book_paths::book_folder_path(
        &app.config,
        &pending.plan.book_id,
        Some(book_name),
    );
    if !dir.exists() {
        return;
    }
    if dir.join("status.json").exists() {
        return;
    }

    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return;
    };

    let safe_name = safe_fs_name(book_name, "_", 120);
    let mut entries: Vec<std::path::PathBuf> = Vec::new();
    for ent in read_dir.flatten() {
        entries.push(ent.path());
    }
    if entries.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let is_cover_like = |p: &std::path::Path| {
        if p.is_dir() {
            return false;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        let Some(ext) = p.extension().and_then(|s| s.to_str()) else {
            return false;
        };
        let ext = ext.to_ascii_lowercase();
        let is_img = matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif");
        if !is_img {
            return false;
        }
        stem == safe_name || stem.eq_ignore_ascii_case("cover")
    };

    // Abort if there are non-cover files.
    if entries.iter().any(|p| !is_cover_like(p)) {
        return;
    }

    for p in &entries {
        let _ = std::fs::remove_file(p);
    }

    // Remove dir if empty.
    if crate::base_system::file_cleaner::is_empty_dir(&dir).unwrap_or(false) {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn poll_worker(app: &mut App) -> Result<()> {
    while let Ok(msg) = app.worker_rx.try_recv() {
        stop_spinner(app);
        match msg {
            WorkerMsg::SearchDone(res) => match res {
                Ok(results) => {
                    if results.is_empty() {
                        app.status = "未找到匹配书籍".to_string();
                        app.results.clear();
                        app.list_state.select(None);
                        app.focus = Focus::Input;
                    } else {
                        app.status = format!(
                            "找到 {} 本书，使用上下键选择，Enter 预览/下载。",
                            results.len()
                        );
                        app.results = results;
                        app.list_state.select(Some(0));
                        app.focus = Focus::Results;
                    }
                }
                Err(err) => {
                    app.status = format!("搜索失败: {err}");
                    app.push_message(format!("搜索失败: {err}"));
                    warn!(target: "ui", "搜索失败: {err}");
                }
            },
            WorkerMsg::PreviewReady(res) => match res {
                Ok(pending) => {
                    let title = pending
                        .plan
                        .meta
                        .book_name
                        .clone()
                        .unwrap_or_else(|| pending.plan.book_id.clone());
                    let total = pending.plan.chapters.len();
                    let downloaded = pending.downloaded_count;
                    let _desc = pending.plan.meta.description.clone().unwrap_or_default();
                    let meta = &pending.plan.meta;
                    let mut meta_parts: Vec<String> = Vec::new();
                    if let Some(score) = meta.score {
                        meta_parts.push(format!("评分 {:.1}", score));
                    }
                    if let Some(words) = meta.word_count {
                        meta_parts.push(format!("字数 {}", format_word_count(words)));
                    }
                    if let Some(reads) = meta.read_count_text.as_ref().or(meta.read_count.as_ref())
                    {
                        meta_parts.push(format!("阅读 {}", reads));
                    }
                    if let Some(cat) = meta.category.as_ref() {
                        meta_parts.push(format!("分类 {}", cat));
                    }
                    if let Some(short) = meta.book_short_name.as_ref() {
                        meta_parts.push(format!("别名 {}", truncate(short, 80)));
                    }

                    // Reuse directory-derived metadata to enrich the selected search result.
                    // This keeps home preview info complete without extra API calls.
                    upsert_result_detail_from_plan(app, &pending.plan.book_id, &pending.plan.meta);

                    app.pending_download = Some(pending);
                    app.view = View::Preview;
                    app.preview_focus = PreviewFocus::Range;
                    app.preview_buttons.select(Some(0));
                    app.preview_range.clear();
                    app.preview_modal_open = true;
                    app.input.clear();
                    app.download_progress = Some(ProgressSnapshot {
                        group_done: 0,
                        group_total: total.div_ceil(25),
                        saved_chapters: downloaded,
                        chapter_total: total,
                        comment_fetch: 0,
                        comment_total: if app.config.enable_segment_comments {
                            total
                        } else {
                            0
                        },
                        comment_saved: 0,
                    });
                    app.status =
                        format!("预览: 《{}》 共 {} 章，已下载 {}", title, total, downloaded);
                    // 预览详情改由弹窗显示，不再推送到状态/消息框
                }
                Err(err) => {
                    app.status = format!("加载目录失败: {err}");
                    app.push_message(format!("加载目录失败: {err}"));
                    warn!(target: "ui", "加载目录失败: {err}");
                }
            },
            WorkerMsg::UpdateScanned(res) => match res {
                Ok((updates, no_updates)) => {
                    if updates.is_empty() && no_updates.is_empty() {
                        app.status = "未发现本地小说，先下载一本试试".to_string();
                        app.view = View::Home;
                    } else {
                        app.update_entries = updates;
                        app.update_no_updates = no_updates;
                        app.show_no_update = app.update_entries.is_empty();
                        if (app.show_no_update && !app.update_no_updates.is_empty())
                            || (!app.show_no_update && !app.update_entries.is_empty())
                        {
                            app.update_state.select(Some(0));
                        } else {
                            app.update_state.select(None);
                        }
                        let has = app.update_entries.len();
                        let none = app.update_no_updates.len();
                        app.status = format!("扫描完成：有更新 {has} 本，无更新 {none} 本");
                        info!(target: "ui", updates = has, no_updates = none, "扫描完成");
                        app.view = View::Update;
                    }
                }
                Err(err) => {
                    app.status = format!("扫描更新失败: {err}");
                    app.push_message(format!("扫描更新失败: {err}"));
                    warn!(target: "ui", "扫描更新失败: {err}");
                }
            },
            WorkerMsg::DownloadDone { book_id, result } => match result {
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
                }
            },
            WorkerMsg::DownloadProgress(snap) => {
                app.download_progress = Some(snap);
            }
        }
    }
    Ok(())
}

pub(super) fn current_cfg_value(app: &App, field: ConfigField) -> String {
    match field {
        ConfigField::SavePath => app.config.save_path.clone(),
        ConfigField::NovelFormat => app.config.novel_format.clone(),
        ConfigField::FirstLineIndentEm => format!("{:.2}", app.config.first_line_indent_em),
        ConfigField::BulkFiles => app.config.bulk_files.to_string(),
        ConfigField::AutoClearDump => app.config.auto_clear_dump.to_string(),
        ConfigField::OldCli => app.config.old_cli.to_string(),
        ConfigField::EnableSegmentComments => app.config.enable_segment_comments.to_string(),
        ConfigField::UseOfficialApi => app.config.use_official_api.to_string(),
        ConfigField::ApiEndpoints => app.config.api_endpoints.join(","),
        ConfigField::MaxWorkers => app.config.max_workers.to_string(),
        ConfigField::RequestTimeout => app.config.request_timeout.to_string(),
        ConfigField::MaxRetries => app.config.max_retries.to_string(),
        ConfigField::MinConnectTimeout => format!("{:.2}", app.config.min_connect_timeout),
        ConfigField::ForceExitTimeout => app.config.force_exit_timeout.to_string(),
        ConfigField::GracefulExit => app.config.graceful_exit.to_string(),
        ConfigField::MinWait => app.config.min_wait_time.to_string(),
        ConfigField::MaxWait => app.config.max_wait_time.to_string(),
        ConfigField::EnableAudiobook => app.config.enable_audiobook.to_string(),
        ConfigField::AudiobookVoice => app.config.audiobook_voice.clone(),
        ConfigField::AudiobookRate => app.config.audiobook_rate.clone(),
        ConfigField::AudiobookVolume => app.config.audiobook_volume.clone(),
        ConfigField::AudiobookPitch => app.config.audiobook_pitch.clone(),
        ConfigField::AudiobookFormat => app.config.audiobook_format.clone(),
        ConfigField::AudiobookConcurrency => app.config.audiobook_concurrency.to_string(),
        ConfigField::SegmentCommentsTopN => app.config.segment_comments_top_n.to_string(),
        ConfigField::SegmentCommentsWorkers => app.config.segment_comments_workers.to_string(),
        ConfigField::DownloadCommentImages => app.config.download_comment_images.to_string(),
        ConfigField::DownloadCommentAvatars => app.config.download_comment_avatars.to_string(),
        ConfigField::MediaDownloadWorkers => app.config.media_download_workers.to_string(),
        ConfigField::BlockedMediaDomains => app.config.blocked_media_domains.join(","),
        ConfigField::ForceConvertImagesToJpeg => {
            app.config.force_convert_images_to_jpeg.to_string()
        }
        ConfigField::JpegRetryConvert => app.config.jpeg_retry_convert.to_string(),
        ConfigField::JpegQuality => app.config.jpeg_quality.to_string(),
        ConfigField::ConvertHeicToJpeg => app.config.convert_heic_to_jpeg.to_string(),
        ConfigField::KeepHeicOriginal => app.config.keep_heic_original.to_string(),
        ConfigField::MediaLimitPerChapter => app.config.media_limit_per_chapter.to_string(),
        ConfigField::MediaMaxDimensionPx => app.config.media_max_dimension_px.to_string(),
        ConfigField::MediaTotalLimitMb => app.config.media_total_limit_mb.to_string(),
    }
}

pub(super) fn start_cfg_edit(app: &mut App) {
    let Some(cat_idx) = app.cfg_cat_state.selected() else {
        return;
    };
    let Some(entry_idx) = app.cfg_entry_state.selected() else {
        return;
    };
    let Some(category) = app.cfg_categories.get(cat_idx) else {
        return;
    };
    if entry_idx >= category.entries.len() {
        return;
    }
    let entry = &category.entries[entry_idx];
    app.cfg_editing = Some((cat_idx, entry_idx));
    app.cfg_edit_buffer = current_cfg_value(app, entry.field);
    app.status = format!("正在编辑 [{}]: {}", category.title, entry.title);
}

pub(super) fn apply_cfg_edit(app: &mut App, cat_idx: usize, entry_idx: usize) -> Result<()> {
    let Some(category) = app.cfg_categories.get(cat_idx) else {
        return Ok(());
    };
    if entry_idx >= category.entries.len() {
        return Ok(());
    }
    let entry = &category.entries[entry_idx];
    let raw = app.cfg_edit_buffer.trim();

    let mut note: Option<String> = None;

    match entry.field {
        ConfigField::SavePath => {
            app.config.save_path = raw.to_string();
        }
        ConfigField::NovelFormat => {
            let lower = raw.to_ascii_lowercase();
            if lower != "txt" && lower != "epub" {
                app.status = "仅支持 txt 或 epub".to_string();
                return Ok(());
            }
            app.config.novel_format = lower;
            if app.config.novel_format == "txt" && app.config.enable_segment_comments {
                app.config.enable_segment_comments = false;
                note = Some("已关闭段评以兼容 txt".to_string());
            }
        }
        ConfigField::FirstLineIndentEm => {
            let val: f32 = raw.parse().map_err(|_| anyhow!("请输入数字"))?;
            if val.is_sign_negative() {
                app.status = "缩进不能为负".to_string();
                return Ok(());
            }
            app.config.first_line_indent_em = val;
        }
        ConfigField::BulkFiles => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.bulk_files = val;
        }
        ConfigField::AutoClearDump => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.auto_clear_dump = val;
        }
        ConfigField::OldCli => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.old_cli = val;
        }
        ConfigField::EnableSegmentComments => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            if val && !app.config.novel_format.eq_ignore_ascii_case("epub") {
                app.status = "段评仅支持 epub，请先将格式改为 epub".to_string();
                return Ok(());
            }
            app.config.enable_segment_comments = val;
        }
        ConfigField::UseOfficialApi => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.use_official_api = val;
        }
        ConfigField::ApiEndpoints => {
            let list = parse_string_list(raw);
            app.config.api_endpoints = list;
        }
        ConfigField::MaxWorkers => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入正整数"))?;
            if val == 0 {
                app.status = "最大线程数需大于 0".to_string();
                return Ok(());
            }
            app.config.max_workers = val;
        }
        ConfigField::RequestTimeout => {
            let val: u64 = raw.parse().map_err(|_| anyhow!("请输入秒数"))?;
            if val == 0 {
                app.status = "超时时间需大于 0".to_string();
                return Ok(());
            }
            app.config.request_timeout = val;
        }
        ConfigField::MaxRetries => {
            let val: u32 = raw.parse().map_err(|_| anyhow!("请输入整数"))?;
            app.config.max_retries = val;
        }
        ConfigField::MinConnectTimeout => {
            let val: f64 = raw.parse().map_err(|_| anyhow!("请输入数字"))?;
            if val <= 0.0 {
                app.status = "连接超时需大于 0".to_string();
                return Ok(());
            }
            app.config.min_connect_timeout = val;
        }
        ConfigField::ForceExitTimeout => {
            let val: u64 = raw.parse().map_err(|_| anyhow!("请输入秒数"))?;
            app.config.force_exit_timeout = val;
        }
        ConfigField::GracefulExit => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.graceful_exit = val;
        }
        ConfigField::MinWait => {
            let val: u64 = raw.parse().map_err(|_| anyhow!("请输入整数毫秒"))?;
            if val > app.config.max_wait_time {
                app.status = "最小等待时间不能超过最大等待时间".to_string();
                return Ok(());
            }
            app.config.min_wait_time = val;
        }
        ConfigField::MaxWait => {
            let val: u64 = raw.parse().map_err(|_| anyhow!("请输入整数毫秒"))?;
            if val < app.config.min_wait_time {
                app.status = "最大等待时间需要不小于最小等待时间".to_string();
                return Ok(());
            }
            app.config.max_wait_time = val;
        }
        ConfigField::EnableAudiobook => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.enable_audiobook = val;
        }
        ConfigField::AudiobookVoice => {
            app.config.audiobook_voice = raw.to_string();
        }
        ConfigField::AudiobookRate => {
            app.config.audiobook_rate = raw.to_string();
        }
        ConfigField::AudiobookVolume => {
            app.config.audiobook_volume = raw.to_string();
        }
        ConfigField::AudiobookPitch => {
            app.config.audiobook_pitch = raw.to_string();
        }
        ConfigField::AudiobookFormat => {
            let lower = raw.to_ascii_lowercase();
            if lower != "mp3" && lower != "wav" {
                app.status = "格式仅支持 mp3 或 wav".to_string();
                return Ok(());
            }
            app.config.audiobook_format = lower;
        }
        ConfigField::AudiobookConcurrency => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入正整数"))?;
            if val == 0 {
                app.status = "并发章节数需大于 0".to_string();
                return Ok(());
            }
            app.config.audiobook_concurrency = val;
        }
        ConfigField::SegmentCommentsTopN => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入整数"))?;
            if val == 0 {
                app.status = "评论数上限需大于 0".to_string();
                return Ok(());
            }
            app.config.segment_comments_top_n = val;
        }
        ConfigField::SegmentCommentsWorkers => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入正整数"))?;
            if val == 0 {
                app.status = "段评线程数需大于 0".to_string();
                return Ok(());
            }
            app.config.segment_comments_workers = val;
        }
        ConfigField::DownloadCommentImages => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.download_comment_images = val;
        }
        ConfigField::DownloadCommentAvatars => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.download_comment_avatars = val;
        }
        ConfigField::MediaDownloadWorkers => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入正整数"))?;
            if val == 0 {
                app.status = "媒体线程数需大于 0".to_string();
                return Ok(());
            }
            app.config.media_download_workers = val;
        }
        ConfigField::BlockedMediaDomains => {
            app.config.blocked_media_domains = parse_string_list(raw);
        }
        ConfigField::ForceConvertImagesToJpeg => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.force_convert_images_to_jpeg = val;
        }
        ConfigField::JpegRetryConvert => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.jpeg_retry_convert = val;
        }
        ConfigField::JpegQuality => {
            let val: u8 = raw
                .parse()
                .map_err(|_| anyhow!("请输入 0-100 之间的整数"))?;
            if val > 100 {
                app.status = "JPEG 质量需在 0-100 之间".to_string();
                return Ok(());
            }
            app.config.jpeg_quality = val;
        }
        ConfigField::ConvertHeicToJpeg => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.convert_heic_to_jpeg = val;
        }
        ConfigField::KeepHeicOriginal => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.keep_heic_original = val;
        }
        ConfigField::MediaLimitPerChapter => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入整数"))?;
            app.config.media_limit_per_chapter = val;
        }
        ConfigField::MediaMaxDimensionPx => {
            let val: u32 = raw.parse().map_err(|_| anyhow!("请输入整数"))?;
            app.config.media_max_dimension_px = val;
        }
        ConfigField::MediaTotalLimitMb => {
            let val: u32 = raw.parse().map_err(|_| anyhow!("请输入整数"))?;
            app.config.media_total_limit_mb = val;
        }
    }

    let path = Path::new(Config::FILE_NAME);
    write_with_comments(&app.config, path).map_err(|e| anyhow!(e.to_string()))?;
    match note {
        Some(extra) => app.status = format!("已保存: {}（{}）", entry.title, extra),
        None => app.status = format!("已保存: {}", entry.title),
    }
    Ok(())
}

fn parse_bool(input: &str) -> Option<bool> {
    match input.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" | "on" => Some(true),
        "false" | "0" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

fn parse_string_list(input: &str) -> Vec<String> {
    input
        .split([',', ';', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

pub(super) fn format_word_count(words: usize) -> String {
    if words >= 10_000 {
        format!("{:.1} 万字", words as f64 / 10_000.0)
    } else {
        format!("{} 字", words)
    }
}

pub(super) fn truncate(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= limit {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}
