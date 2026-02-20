//! TUI（ratatui + crossterm）主循环与页面路由。
//!
//! 负责终端初始化（raw mode / mouse capture）、事件循环、页面切换与全局状态管理。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::AtomicBool,
    mpsc::{self, Receiver, Sender},
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
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
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use serde_json::Value;
#[cfg(feature = "official-api")]
use tomato_novel_official_api::SearchClient;
use tracing::{info, warn};

mod about;
mod clipboard;
mod config;
mod config_model;
mod cover;
mod download;
mod home;
mod preview;
mod update;

use update::show_update_menu;

use crate::base_system::context::{Config, safe_fs_name};
use crate::base_system::json_extract;
use crate::base_system::logging::take_broadcast_rx;
use crate::download::downloader::{BookMeta, ChapterRange, DownloadPlan, ProgressSnapshot};
use crate::prewarm_state;

pub(super) use config_model::{
    AUDIOBOOK_VOICE_PRESETS, ConfigCategory, ConfigEntry, apply_cfg_edit, build_config_categories,
    cfg_combo_presets, cfg_field_is_bool, cfg_field_is_combo, current_cfg_value, start_cfg_edit,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigFocus {
    Category,
    Entry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfigComboFocus {
    List,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewFocus {
    Range,
    Buttons,
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
    DownloadDone {
        book_id: String,
        result: Result<()>,
    },
    DownloadProgress(ProgressSnapshot),
    AskBookName {
        options: Vec<crate::download::downloader::BookNameOption>,
        respond_to: std::sync::mpsc::Sender<Option<String>>,
    },
    PreviewReady(Box<Result<PendingDownload>>),
    UpdateScanned(Result<(Vec<UpdateEntry>, Vec<UpdateEntry>)>),
    AppUpdateChecked(Result<crate::base_system::app_update::UpdateCheckReport>),
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
    switch_to_old_cli_requested: bool,
    view: View,
    previous_view: View,
    menu_state: ListState,

    // config state
    cfg_categories: Vec<ConfigCategory>,
    cfg_cat_state: ListState,
    cfg_cat_hover: Option<usize>,
    cfg_entry_state: ListState,
    cfg_button_state: ListState,
    cfg_focus: ConfigFocus,
    cfg_editing: Option<(usize, usize)>,
    cfg_edit_buffer: String,
    cfg_bool_state: ListState,
    cfg_combo_state: ListState,
    cfg_combo_focus: ConfigComboFocus,
    last_config_layout: Option<[Rect; 3]>,
    last_config_button: Option<Rect>,
    last_config_bool_area: Option<Rect>,
    last_config_combo_list_area: Option<Rect>,
    last_config_combo_input_area: Option<Rect>,

    // segment comments confirmation modal (to avoid accidental enable)
    segment_comments_confirm_open: bool,
    segment_comments_confirm_ctx: Option<(usize, usize)>,
    segment_comments_confirm_state: ListState,
    last_segment_comments_confirm_options: Option<Rect>,

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

    // app update (program update)
    app_update_report: Option<crate::base_system::app_update::UpdateCheckReport>,

    // app self update (download & replace binary)
    self_update_requested: bool,
    self_update_auto_yes: bool,

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

    // book name modal (post-download)
    book_name_modal_open: bool,
    book_name_modal_state: ListState,
    book_name_modal_options: Vec<crate::download::downloader::BookNameOption>,
    book_name_modal_sender: Option<std::sync::mpsc::Sender<Option<String>>>,
    last_book_name_modal_list: Option<Rect>,
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
        let mut cfg_bool_state = ListState::default();
        cfg_bool_state.select(Some(0));
        let mut cfg_combo_state = ListState::default();
        cfg_combo_state.select(Some(0));
        let mut preview_buttons = ListState::default();
        preview_buttons.select(Some(0));

        let mut book_name_modal_state = ListState::default();
        book_name_modal_state.select(Some(0));

        let mut segment_comments_confirm_state = ListState::default();
        // Default to "Cancel" to reduce accidental enable.
        segment_comments_confirm_state.select(Some(1));

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
            switch_to_old_cli_requested: false,
            view: View::Home,
            previous_view: View::Home,
            menu_state,
            cfg_categories,
            cfg_cat_state,
            cfg_cat_hover: None,
            cfg_entry_state,
            cfg_button_state,
            cfg_focus: ConfigFocus::Entry,
            cfg_editing: None,
            cfg_edit_buffer: String::new(),
            cfg_bool_state,
            cfg_combo_state,
            cfg_combo_focus: ConfigComboFocus::List,
            last_config_layout: None,
            last_config_button: None,
            last_config_bool_area: None,
            last_config_combo_list_area: None,
            last_config_combo_input_area: None,
            segment_comments_confirm_open: false,
            segment_comments_confirm_ctx: None,
            segment_comments_confirm_state,
            last_segment_comments_confirm_options: None,
            update_entries: Vec::new(),
            update_no_updates: Vec::new(),
            update_state,
            show_no_update: false,
            last_update_layout: None,
            last_update_exit_button: None,
            about_btn_state,
            last_about_buttons: None,
            app_update_report: None,
            self_update_requested: false,
            self_update_auto_yes: false,
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

            book_name_modal_open: false,
            book_name_modal_state,
            book_name_modal_options: Vec::new(),
            book_name_modal_sender: None,
            last_book_name_modal_list: None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiExit {
    Quit,
    SwitchToOldCli,
    SelfUpdate { auto_yes: bool },
}

pub fn run(config: Config) -> Result<TuiExit> {
    let (worker_tx, worker_rx) = mpsc::channel();
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
    crossterm_execute!(stdout, EnableMouseCapture).context("enable mouse capture")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("init terminal")?;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_loop(&mut terminal, config, worker_tx, worker_rx)
    }));

    disable_raw_mode().ok();
    crossterm_execute!(terminal.backend_mut(), DisableMouseCapture).ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    match result {
        Ok(r) => r,
        Err(panic_payload) => {
            std::panic::resume_unwind(panic_payload);
        }
    }
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    config: Config,
    worker_tx: Sender<WorkerMsg>,
    worker_rx: Receiver<WorkerMsg>,
) -> Result<TuiExit> {
    let mut app = App::new(config, worker_tx, worker_rx);

    // 每次启动检查程序更新（异步，不阻塞 UI）。
    start_app_update_check(&mut app);

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

    if app.switch_to_old_cli_requested {
        Ok(TuiExit::SwitchToOldCli)
    } else if app.self_update_requested {
        Ok(TuiExit::SelfUpdate {
            auto_yes: app.self_update_auto_yes,
        })
    } else {
        Ok(TuiExit::Quit)
    }
}

pub(super) fn start_app_update_check(app: &mut App) {
    let tx = app.worker_tx.clone();
    thread::spawn(move || {
        let result =
            crate::base_system::app_update::check_update_report_blocking(env!("CARGO_PKG_VERSION"));
        let _ = tx.send(WorkerMsg::AppUpdateChecked(result));
    });
}

fn draw_ui(frame: &mut ratatui::Frame, app: &mut App) {
    match app.view {
        View::Home => home::draw_home(frame, app),
        View::Config => config::draw_config(frame, app),
        View::Update => update::draw_update(frame, app),
        View::About => about::draw_about(frame, app),
        View::Cover => cover::draw_cover(frame, app),
        View::Preview => preview::draw_preview(frame, app),
    }

    if app.book_name_modal_open {
        render_book_name_modal(frame, app);
    }
}

fn handle_event(app: &mut App) -> Result<bool> {
    if !event::poll(Duration::from_millis(200)).context("poll event")? {
        return Ok(true);
    }

    let evt = event::read().context("read event")?;
    if app.book_name_modal_open {
        handle_book_name_modal_event(app, evt)?;
        return Ok(!app.should_quit);
    }
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
    preview::handle_event_preview(app, event)
}

fn handle_book_name_modal_event(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Up => {
                let len = app.book_name_modal_options.len();
                if len > 0 {
                    let cur = app.book_name_modal_state.selected().unwrap_or(0);
                    let next = if cur == 0 { len - 1 } else { cur - 1 };
                    app.book_name_modal_state.select(Some(next));
                }
            }
            KeyCode::Down => {
                let len = app.book_name_modal_options.len();
                if len > 0 {
                    let cur = app.book_name_modal_state.selected().unwrap_or(0);
                    let next = (cur + 1) % len;
                    app.book_name_modal_state.select(Some(next));
                }
            }
            KeyCode::Enter => {
                let idx = app.book_name_modal_state.selected().unwrap_or(0);
                let chosen = app
                    .book_name_modal_options
                    .get(idx)
                    .map(|o| o.value.clone());
                if let Some(tx) = app.book_name_modal_sender.take() {
                    let _ = tx.send(chosen);
                }
                app.book_name_modal_open = false;
                app.book_name_modal_options.clear();
                app.last_book_name_modal_list = None;
            }
            _ => {}
        },
        Event::Mouse(me) if matches!(me.kind, MouseEventKind::Down(MouseButton::Left)) => {
            if let Some(area) = app.last_book_name_modal_list
                && pos_in(area, me.column, me.row)
            {
                let idx = me.row.saturating_sub(area.y + 1) as usize;
                if idx < app.book_name_modal_options.len() {
                    app.book_name_modal_state.select(Some(idx));
                    let chosen = app
                        .book_name_modal_options
                        .get(idx)
                        .map(|o| o.value.clone());
                    if let Some(tx) = app.book_name_modal_sender.take() {
                        let _ = tx.send(chosen);
                    }
                    app.book_name_modal_open = false;
                    app.book_name_modal_options.clear();
                    app.last_book_name_modal_list = None;
                }
            }
        }
        Event::Mouse(me) if matches!(me.kind, MouseEventKind::Moved) => {
            if let Some(area) = app.last_book_name_modal_list
                && pos_in(area, me.column, me.row)
            {
                let idx = me.row.saturating_sub(area.y + 1) as usize;
                if idx < app.book_name_modal_options.len() {
                    app.book_name_modal_state.select(Some(idx));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(feature = "official-api")]
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

#[cfg(not(feature = "official-api"))]
fn search_books(_query: &str) -> Result<Vec<SearchItem>> {
    anyhow::bail!("当前构建未启用 official-api feature，搜索功能不可用")
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
    preview::parse_range_input(input, total)
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

#[cfg(feature = "docker")]
const ABOUT_BUTTONS: &[&str] = &["打开Github仓库", "返回"];

#[cfg(not(feature = "docker"))]
const ABOUT_BUTTONS: &[&str] = &[
    "打开Github仓库",
    "检查程序更新",
    "执行自更新",
    "不再提醒该版本",
    "返回",
];

fn current_category(app: &App) -> Option<(usize, &ConfigCategory)> {
    let idx = app.cfg_cat_state.selected()?;
    app.cfg_categories.get(idx).map(|c| (idx, c))
}

pub(super) fn current_cfg_entries(app: &App) -> Option<&[ConfigEntry]> {
    current_category(app).map(|(_, c)| c.entries.as_slice())
}

pub(super) fn ensure_entry_selection(app: &mut App) {
    let mut entry_state = ListState::default();
    if let Some(entries) = current_cfg_entries(app)
        && !entries.is_empty()
    {
        let idx = app
            .cfg_entry_state
            .selected()
            .unwrap_or(0)
            .min(entries.len().saturating_sub(1));
        entry_state.select(Some(idx));
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
    let main = layout.first().copied().unwrap_or(area);
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

fn render_book_name_modal(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.size();
    let w = (area.width as f32 * 0.70) as u16;
    let h: u16 = 12;
    let modal = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w.max(40).min(area.width.saturating_sub(2)),
        height: h.min(area.height.saturating_sub(2)).max(8),
    };

    frame.render_widget(Clear, modal);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("下载完成：选择书名")
        .border_style(Style::default().fg(Color::Green));
    frame.render_widget(block, modal);

    let inner = Rect {
        x: modal.x + 1,
        y: modal.y + 1,
        width: modal.width.saturating_sub(2),
        height: modal.height.saturating_sub(2),
    };

    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    let hint = Paragraph::new(vec![Line::from("↑↓ 选择 / Enter 确认")]).wrap(Wrap { trim: true });
    frame.render_widget(hint, parts[0]);

    let items: Vec<ListItem> = app
        .book_name_modal_options
        .iter()
        .map(|o| ListItem::new(format!("{}: {}", o.label, o.value)))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("候选书名"))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, parts[1], &mut app.book_name_modal_state);
    app.last_book_name_modal_list = Some(parts[1]);

    let footer = Paragraph::new(Line::from("选择后将用于最终文件名（下载临时目录不变）"))
        .wrap(Wrap { trim: true });
    frame.render_widget(footer, parts[2]);
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

    if let Some(target) = rest.first()
        && !target.is_empty()
    {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            (*target).to_string(),
            Style::default().fg(Color::LightBlue),
        ));
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
    preview::start_preview_task(app, book_id, hint)
}

pub(super) fn start_download_task(
    app: &mut App,
    pending: PendingDownload,
    range: Option<ChapterRange>,
) -> Result<()> {
    download::start_download_task(app, pending, range)
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
            WorkerMsg::PreviewReady(res) => match *res {
                Ok(pending) => preview::apply_preview_ready(app, pending),
                Err(err) => preview::apply_preview_error(app, err),
            },
            WorkerMsg::AskBookName {
                options,
                respond_to,
            } => {
                if options.is_empty() {
                    let _ = respond_to.send(None);
                } else {
                    app.book_name_modal_open = true;
                    app.book_name_modal_options = options;
                    app.book_name_modal_state.select(Some(0));
                    app.book_name_modal_sender = Some(respond_to);
                    app.status = "请选择书名（下载已完成）".to_string();
                }
            }
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
            WorkerMsg::DownloadDone { book_id, result } => {
                download::apply_download_done(app, book_id, result);
            }
            WorkerMsg::DownloadProgress(snap) => download::apply_download_progress(app, snap),
            WorkerMsg::AppUpdateChecked(res) => match res {
                Ok(report) => {
                    let notify = crate::base_system::app_update::should_notify_startup(&report);
                    if notify {
                        app.status = format!(
                            "发现新版本 {}（当前 {}），在 About 页面可查看/不再提醒",
                            report.latest.tag_name, report.current_tag
                        );
                        app.push_message(format!(
                            "新版本可用: {} (当前 {})",
                            report.latest.tag_name, report.current_tag
                        ));
                    }
                    app.app_update_report = Some(report);
                }
                Err(err) => {
                    // 不影响使用：仅记录日志。
                    warn!(target: "ui", "检查程序更新失败: {err}");
                }
            },
        }
    }
    Ok(())
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

pub(super) fn pos_in(area: Rect, col: u16, row: u16) -> bool {
    col >= area.x
        && col < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

pub(super) fn list_inner_area(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

pub(super) fn list_index_from_mouse_row(
    list_area: Rect,
    mouse_row: u16,
    state: &ListState,
    items_len: usize,
) -> Option<usize> {
    if items_len == 0 {
        return None;
    }

    let inner = list_inner_area(list_area);
    if inner.height == 0 {
        return None;
    }

    let rel = mouse_row.checked_sub(inner.y)? as usize;
    if rel >= inner.height as usize {
        return None;
    }

    let idx = state.offset().saturating_add(rel);
    (idx < items_len).then_some(idx)
}
