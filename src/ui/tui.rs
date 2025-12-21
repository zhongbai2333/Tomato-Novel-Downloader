use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
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
use image::{DynamicImage, GenericImageView, imageops::FilterType};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::prelude::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use serde_json::Value;
use tomato_novel_official_api::{DirectoryClient, SearchClient};

use crate::base_system::config::{ConfigSpec, write_with_comments};
use crate::base_system::context::{Config, safe_fs_name};
use crate::download::downloader;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Confirm,
    Config,
    Update,
    About,
    Quit,
}

#[derive(Debug, Clone, Copy)]
enum ConfigField {
    SavePath,
    NovelFormat,
    EnableSegmentComments,
    UseOfficialApi,
    MaxWorkers,
    MinWait,
    MaxWait,
}

#[derive(Debug, Clone)]
struct ConfigEntry {
    title: &'static str,
    field: ConfigField,
}

#[derive(Debug, Clone)]
struct UpdateEntry {
    book_id: String,
    book_name: String,
    folder: PathBuf,
    label: String,
    new_count: usize,
    has_update: bool,
}

#[derive(Debug)]
enum WorkerMsg {
    SearchDone(Result<Vec<SearchItem>>),
    DownloadDone { book_id: String, result: Result<()> },
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
}

impl BookDetail {
    fn has_data(&self) -> bool {
        self.description.is_some() || !self.tags.is_empty() || self.chapter_count.is_some()
    }
}

struct App {
    input: String,
    focus: Focus,
    status: String,
    messages: Vec<String>,
    results: Vec<SearchItem>,
    list_state: ListState,
    config: Config,
    should_quit: bool,
    view: View,
    previous_view: View,
    cfg_entries: Vec<ConfigEntry>,
    cfg_state: ListState,
    cfg_editing: Option<usize>,
    cfg_edit_buffer: String,
    menu_state: ListState,
    update_entries: Vec<UpdateEntry>,
    update_no_updates: Vec<UpdateEntry>,
    update_state: ListState,
    show_no_update: bool,
    last_home_layout: Option<[Rect; 5]>,
    last_config_layout: Option<[Rect; 3]>,
    last_update_layout: Option<[Rect; 3]>,
    worker_tx: Sender<WorkerMsg>,
    worker_rx: Receiver<WorkerMsg>,
    spinner_active: bool,
    spinner_text: String,
    spinner_idx: usize,
    spinner_last: Instant,
    cover_lines: Vec<String>,
    cover_title: String,
}

impl App {
    fn new(config: Config, worker_tx: Sender<WorkerMsg>, worker_rx: Receiver<WorkerMsg>) -> Self {
        let cfg_entries = vec![
            ConfigEntry {
                title: "保存路径",
                field: ConfigField::SavePath,
            },
            ConfigEntry {
                title: "小说格式(txt/epub)",
                field: ConfigField::NovelFormat,
            },
            ConfigEntry {
                title: "是否下载段评",
                field: ConfigField::EnableSegmentComments,
            },
            ConfigEntry {
                title: "使用官方API",
                field: ConfigField::UseOfficialApi,
            },
            ConfigEntry {
                title: "最大线程数",
                field: ConfigField::MaxWorkers,
            },
            ConfigEntry {
                title: "最小等待时间(ms)",
                field: ConfigField::MinWait,
            },
            ConfigEntry {
                title: "最大等待时间(ms)",
                field: ConfigField::MaxWait,
            },
        ];

        let mut cfg_state = ListState::default();
        cfg_state.select(Some(0));

        let mut menu_state = ListState::default();
        menu_state.select(Some(0));

        let mut update_state = ListState::default();
        update_state.select(Some(0));

        Self {
            input: String::new(),
            focus: Focus::Input,
            status: String::from("输入书名/ID/链接，Enter 确认，Tab 切换焦点，q 退出。"),
            messages: Vec::new(),
            results: Vec::new(),
            list_state: ListState::default(),
            config,
            should_quit: false,
            view: View::Home,
            previous_view: View::Home,
            cfg_entries,
            cfg_state,
            cfg_editing: None,
            cfg_edit_buffer: String::new(),
            menu_state,
            update_entries: Vec::new(),
            update_no_updates: Vec::new(),
            update_state,
            show_no_update: false,
            last_home_layout: None,
            last_config_layout: None,
            last_update_layout: None,
            worker_tx,
            worker_rx,
            spinner_active: false,
            spinner_text: String::new(),
            spinner_idx: 0,
            spinner_last: Instant::now(),
            cover_lines: Vec::new(),
            cover_title: String::new(),
        }
    }

    fn push_message(&mut self, msg: impl Into<String>) {
        self.messages.push(msg.into());
        if self.messages.len() > 8 {
            let overflow = self.messages.len() - 8;
            self.messages.drain(0..overflow);
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
    let mut stdout = io::stdout();
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
        poll_worker(&mut app)?;

        terminal.draw(|f| draw_ui(f, &mut app))?;

        if !handle_event(&mut app)? {
            break;
        }
    }

    Ok(())
}

fn handle_event(app: &mut App) -> Result<bool> {
    if !event::poll(Duration::from_millis(200)).context("poll event")? {
        return Ok(true);
    }

    let evt = event::read().context("read event")?;
    match app.view {
        View::Home => handle_event_home(app, evt)?,
        View::Config => handle_event_config(app, evt)?,
        View::Update => handle_event_update(app, evt)?,
        View::About => handle_event_about(app, evt)?,
        View::Cover => handle_event_cover(app, evt)?,
    }

    Ok(!app.should_quit)
}

fn handle_event_home(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Paste(s) => {
            if app.focus == Focus::Input {
                app.input.push_str(&s);
            }
        }
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') => {
                if app.focus == Focus::Input {
                    app.input.push('q');
                } else {
                    app.should_quit = true;
                }
            }
            KeyCode::Char('c') => {
                if app.focus == Focus::Input {
                    app.input.push('c');
                } else {
                    switch_view(app, MenuAction::Config)?;
                }
            }
            KeyCode::Char('u') => {
                if app.focus == Focus::Input {
                    app.input.push('u');
                } else {
                    switch_view(app, MenuAction::Update)?;
                }
            }
            KeyCode::Char('a') => {
                if app.focus == Focus::Input {
                    app.input.push('a');
                } else {
                    switch_view(app, MenuAction::About)?;
                }
            }
            KeyCode::Esc => {
                app.focus = Focus::Input;
                app.results.clear();
                app.list_state.select(None);
            }
            KeyCode::Tab => cycle_focus(app),
            KeyCode::Backspace => {
                if app.focus == Focus::Input {
                    app.input.pop();
                }
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Ok(mut clip) = arboard::Clipboard::new() {
                    if let Ok(text) = clip.get_text() {
                        app.input.push_str(&text);
                    }
                }
            }
            KeyCode::Char('p') => {
                if app.focus == Focus::Input {
                    app.input.push('p');
                } else if app.focus == Focus::Results {
                    if let Some(idx) = app.list_state.selected() {
                        if let Some(item) = app.results.get(idx).cloned() {
                            show_cover(app, &item.book_id, &item.title, None)?;
                        }
                    }
                }
            }
            KeyCode::Char('i') => {
                if app.focus == Focus::Input {
                    app.input.push('i');
                } else if app.focus == Focus::Results {
                    if let Some(idx) = app.list_state.selected() {
                        ensure_book_detail(app, idx)?;
                        if let Some(item) = app.results.get(idx) {
                            app.status = format!("已加载详情: 《{}》", item.title);
                        }
                    }
                }
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if app.focus == Focus::Input {
                    app.input.push(c);
                }
            }
            KeyCode::Up => match app.focus {
                Focus::Results => app.select_prev(),
                Focus::Menu => select_prev_menu(app),
                Focus::Input => {}
            },
            KeyCode::Down => match app.focus {
                Focus::Results => app.select_next(),
                Focus::Menu => select_next_menu(app),
                Focus::Input => {}
            },
            KeyCode::Enter => match app.focus {
                Focus::Input => process_input(app)?,
                Focus::Results => {
                    if app.list_state.selected().is_some() {
                        download_selected(app)?;
                    }
                }
                Focus::Menu => trigger_menu_action(app)?,
            },
            _ => {}
        },
        Event::Mouse(me) => handle_mouse_home(app, me)?,
        Event::Resize(_, _) => {}
        _ => {}
    }

    Ok(())
}

fn handle_event_config(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if let Some(edit_idx) = app.cfg_editing {
                match key.code {
                    KeyCode::Esc => {
                        app.cfg_editing = None;
                        app.cfg_edit_buffer.clear();
                        app.status = "取消修改".to_string();
                    }
                    KeyCode::Enter => {
                        if let Err(err) = apply_cfg_edit(app, edit_idx) {
                            app.status = format!("保存失败: {err}");
                        } else {
                            app.cfg_editing = None;
                            app.cfg_edit_buffer.clear();
                        }
                    }
                    KeyCode::Backspace => {
                        app.cfg_edit_buffer.pop();
                    }
                    KeyCode::Char(c) => {
                        app.cfg_edit_buffer.push(c);
                    }
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('c') => {
                        app.view = View::Home;
                        app.status = "返回主菜单".to_string();
                    }
                    KeyCode::Up => {
                        if !app.cfg_entries.is_empty() {
                            app.cfg_state.select(Some(
                                app.cfg_state
                                    .selected()
                                    .map(|i| {
                                        if i == 0 {
                                            app.cfg_entries.len() - 1
                                        } else {
                                            i - 1
                                        }
                                    })
                                    .unwrap_or(0),
                            ));
                        }
                    }
                    KeyCode::Down => {
                        if !app.cfg_entries.is_empty() {
                            app.cfg_state.select(Some(
                                app.cfg_state
                                    .selected()
                                    .map(|i| {
                                        if i + 1 >= app.cfg_entries.len() {
                                            0
                                        } else {
                                            i + 1
                                        }
                                    })
                                    .unwrap_or(0),
                            ));
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(idx) = app.cfg_state.selected() {
                            start_cfg_edit(app, idx);
                        }
                    }
                    _ => {}
                }
            }
        }
        Event::Resize(_, _) => {}
        _ => {}
    }

    Ok(())
}

fn handle_event_update(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                app.view = View::Home;
                app.status = "返回主菜单".to_string();
            }
            KeyCode::Char('p') => {
                if let Some(entry) = current_update_entry(app) {
                    show_cover(
                        app,
                        &entry.book_id,
                        &entry.book_name,
                        Some(entry.folder.clone()),
                    )?;
                }
            }
            KeyCode::Char('n') => {
                app.show_no_update = !app.show_no_update;
                if (app.show_no_update && !app.update_no_updates.is_empty())
                    || (!app.show_no_update && !app.update_entries.is_empty())
                {
                    app.update_state.select(Some(0));
                } else {
                    app.update_state.select(None);
                }
            }
            KeyCode::Up => {
                select_prev_update(app);
            }
            KeyCode::Down => {
                select_next_update(app);
            }
            KeyCode::Enter => {
                if let Some(entry) = current_update_entry(app) {
                    app.status = format!("更新: {}", entry.label);
                    start_download_task(app, entry.book_id.clone())?;
                }
            }
            _ => {}
        },
        Event::Mouse(me) => handle_mouse_update(app, me)?,
        Event::Resize(_, _) => {}
        _ => {}
    }
    Ok(())
}

fn handle_event_about(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                app.view = View::Home;
                app.status = "返回主菜单".to_string();
            }
            _ => {}
        },
        Event::Mouse(me) => {
            if let MouseEventKind::Down(MouseButton::Left) = me.kind {
                app.view = View::Home;
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_event_cover(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                app.view = app.previous_view;
                app.status = "返回".to_string();
            }
            _ => {}
        },
        Event::Mouse(me) => {
            if let MouseEventKind::Down(MouseButton::Left) = me.kind {
                app.view = app.previous_view;
            }
        }
        _ => {}
    }
    Ok(())
}

fn process_input(app: &mut App) -> Result<()> {
    let text = app.input.trim();
    if text.is_empty() {
        app.status = String::from("请输入书名、链接或 book_id，按 Enter 开始。");
        return Ok(());
    }

    if let Some(book_id) = parse_book_id(text) {
        app.focus = Focus::Input;
        app.status = format!("准备下载书籍 {book_id} …");
        start_download_task(app, book_id)?;
        app.input.clear();
        app.results.clear();
        app.list_state.select(None);
    } else {
        start_search_task(app, text.to_string())?;
    }

    Ok(())
}

fn download_selected(app: &mut App) -> Result<()> {
    let Some(idx) = app.list_state.selected() else {
        return Ok(());
    };
    if idx >= app.results.len() {
        return Ok(());
    }
    let book = app.results[idx].clone();
    app.focus = Focus::Input;
    start_download_task(app, book.book_id.clone())
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
    let maps = collect_maps(raw);

    let description = maps.iter().find_map(|m| {
        pick_string_field(
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
        .find_map(|m| pick_tags_field(m))
        .unwrap_or_default();
    let chapter_count = maps.iter().find_map(|m| pick_chapter_count(m));

    BookDetail {
        description,
        tags,
        chapter_count,
    }
}

fn collect_maps<'a>(raw: &'a Value) -> Vec<&'a serde_json::Map<String, Value>> {
    let mut maps = Vec::new();
    if let Some(map) = raw.as_object() {
        maps.push(map);
        if let Some(info) = map.get("book_info").and_then(|v| v.as_object()) {
            maps.push(info);
        }
        if let Some(info) = map.get("bookInfo").and_then(|v| v.as_object()) {
            maps.push(info);
        }
    }
    maps
}

fn pick_string_field(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(val) = map.get(*key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            } else if let Some(n) = val.as_i64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

fn pick_tags_field(map: &serde_json::Map<String, Value>) -> Option<Vec<String>> {
    let candidates = [
        "tags",
        "book_tags",
        "tag",
        "category",
        "categories",
        "classify_tags",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            let items = tags_from_value(val);
            if !items.is_empty() {
                return Some(items);
            }
        }
    }
    None
}

fn tags_from_value(value: &Value) -> Vec<String> {
    match value {
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(s) => s
            .split(|c| c == '|' || c == ',' || c == ' ')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn pick_chapter_count(map: &serde_json::Map<String, Value>) -> Option<usize> {
    let candidates = [
        "item_cnt",
        "book_item_cnt",
        "chapter_num",
        "chapter_count",
        "chapter_total_cnt",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(n) = val.as_u64() {
                return Some(n as usize);
            }
            if let Some(n) = val.as_i64() {
                return Some(n.max(0) as usize);
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.trim().parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn parse_book_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    // ① 优先从链接中提取 book_id（保持与 Python 版一致的行为）
    let re_url = regex::Regex::new(r"https?://\S+").ok();
    if let Some(re) = re_url.as_ref() {
        if let Some(url_match) = re.find(trimmed) {
            let url_str = url_match.as_str();

            let re_path = regex::Regex::new(r"/page/(\d+)").ok();
            if let Some(re) = re_path.as_ref() {
                if let Some(caps) = re.captures(url_str) {
                    return Some(caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string());
                }
            }

            let re_qs = regex::Regex::new(r"(?i)[?&](book_id|bookId)=([0-9]+)").ok();
            if let Some(re) = re_qs.as_ref() {
                if let Some(caps) = re.captures(url_str) {
                    return Some(caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string());
                }
            }
        }
    }

    // ② 纯数字直接作为 book_id
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }

    // ③ 在非链接文本中尝试提取路径或查询参数里的 book_id
    let re_path = regex::Regex::new(r"/page/(\d+)").ok();
    if let Some(re) = re_path.as_ref() {
        if let Some(caps) = re.captures(trimmed) {
            return Some(caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string());
        }
    }

    let re_qs = regex::Regex::new(r"(?i)(book_id|bookId)=([0-9]+)").ok();
    if let Some(re) = re_qs.as_ref() {
        if let Some(caps) = re.captures(trimmed) {
            return Some(caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string());
        }
    }

    None
}

fn ensure_book_detail(app: &mut App, idx: usize) -> Result<()> {
    if idx >= app.results.len() {
        return Ok(());
    }

    let has_data = app
        .results
        .get(idx)
        .and_then(|item| item.detail.as_ref())
        .map(|d| d.has_data())
        .unwrap_or(false);
    if has_data {
        return Ok(());
    }

    let fetched = detail_from_directory(&app.results[idx].book_id)?;
    let merged = if let Some(old) = app.results[idx].detail.take() {
        BookDetail {
            description: fetched.description.or(old.description),
            tags: if fetched.tags.is_empty() {
                old.tags
            } else {
                fetched.tags
            },
            chapter_count: fetched.chapter_count.or(old.chapter_count),
        }
    } else {
        fetched
    };

    app.results[idx].detail = Some(merged);
    Ok(())
}

fn detail_from_directory(book_id: &str) -> Result<BookDetail> {
    let client = DirectoryClient::new().context("init DirectoryClient")?;
    let dir = client
        .fetch_directory(book_id)
        .with_context(|| format!("fetch_directory for {book_id}"))?;

    let maps = collect_maps(&dir.raw);
    let description = maps.iter().find_map(|m| {
        pick_string_field(
            m,
            &[
                "description",
                "desc",
                "abstract",
                "intro",
                "summary",
                "book_abstract",
                "recommendation_reason",
            ],
        )
    });
    let tags = maps
        .iter()
        .find_map(|m| pick_tags_field(m))
        .unwrap_or_default();
    let chapter_count = Some(dir.chapters.len());

    Ok(BookDetail {
        description,
        tags,
        chapter_count,
    })
}

fn show_cover(app: &mut App, book_id: &str, title: &str, folder: Option<PathBuf>) -> Result<()> {
    app.previous_view = app.view;
    app.cover_lines.clear();

    let cover_title = format!("《{}》 ({})", title, book_id);
    app.cover_title = cover_title;

    let candidates = cover_candidates(app, book_id, title, folder);
    let Some(path) = candidates.into_iter().find(|p| p.exists()) else {
        app.view = View::Cover;
        app.status = "未找到封面文件".to_string();
        return Ok(());
    };

    let img = image::open(&path).with_context(|| format!("读取封面失败: {}", path.display()))?;
    let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 24));
    let ascii = image_to_ascii(img, term_w, term_h);
    app.cover_lines = if ascii.is_empty() {
        vec!["封面太小，无法显示".to_string()]
    } else {
        ascii
    };
    app.view = View::Cover;
    app.status = format!("封面: {} (按 q 返回)", path.display());
    Ok(())
}

fn cover_candidates(
    app: &App,
    book_id: &str,
    title: &str,
    folder: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let safe_title = safe_fs_name(title, "_", 120);

    if let Some(base) = folder.as_ref() {
        push_cover_candidates(&mut paths, base, title, &safe_title);
    }

    if let Some(status) = app.config.get_status_folder_path() {
        push_cover_candidates(&mut paths, &status, title, &safe_title);
    }

    let default_folder = app
        .config
        .default_save_dir()
        .join(format!("{}_{}", book_id, safe_title));
    push_cover_candidates(&mut paths, &default_folder, title, &safe_title);

    paths
}

fn push_cover_candidates(list: &mut Vec<PathBuf>, base: &Path, title: &str, safe_title: &str) {
    let names = [
        format!("{safe_title}.jpg"),
        format!("{safe_title}.png"),
        format!("{}.jpg", title),
        format!("{}.png", title),
        "cover.jpg".to_string(),
        "cover.png".to_string(),
    ];

    for name in names {
        let path = base.join(&name);
        if !list.contains(&path) {
            list.push(path);
        }
    }
}

fn image_to_ascii(img: DynamicImage, term_w: u16, term_h: u16) -> Vec<String> {
    const PALETTE: &[u8] = b" .:-=+*#%@";
    let max_width = term_w.saturating_sub(6).max(16) as u32;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Vec::new();
    }

    let target_width = std::cmp::min(max_width, w.max(1));
    let mut target_height = h
        .saturating_mul(target_width)
        .saturating_mul(2)
        .saturating_div(w.max(1));
    let max_height = term_h.saturating_sub(4).max(8) as u32;
    if target_height > max_height {
        target_height = max_height;
    }

    let gray = img
        .resize(
            target_width.max(1),
            target_height.max(1),
            FilterType::Triangle,
        )
        .to_luma8();
    let mut lines = Vec::with_capacity(gray.height() as usize);
    for y in 0..gray.height() {
        let mut line = String::with_capacity(gray.width() as usize);
        for x in 0..gray.width() {
            let v = gray.get_pixel(x, y)[0] as f32 / 255.0;
            let idx = (v * (PALETTE.len() as f32 - 1.0)).round() as usize;
            line.push(*PALETTE.get(idx).unwrap_or(&b' ') as char);
        }
        lines.push(line);
    }
    lines
}

const MENU_ITEMS: &[(&str, MenuAction)] = &[
    ("确定", MenuAction::Confirm),
    ("配置", MenuAction::Config),
    ("更新", MenuAction::Update),
    ("关于", MenuAction::About),
    ("退出", MenuAction::Quit),
];

const SPINNER_FRAMES: &[char] = &['|', '/', '-', '\\'];

fn cycle_focus(app: &mut App) {
    app.focus = match app.focus {
        Focus::Input => Focus::Menu,
        Focus::Menu => {
            if app.results.is_empty() {
                Focus::Input
            } else {
                Focus::Results
            }
        }
        Focus::Results => Focus::Input,
    };
}

fn select_next_menu(app: &mut App) {
    let len = MENU_ITEMS.len();
    if len == 0 {
        return;
    }
    let next = app
        .menu_state
        .selected()
        .map(|i| (i + 1) % len)
        .unwrap_or(0);
    app.menu_state.select(Some(next));
}

fn select_prev_menu(app: &mut App) {
    let len = MENU_ITEMS.len();
    if len == 0 {
        return;
    }
    let prev = app
        .menu_state
        .selected()
        .map(|i| if i == 0 { len - 1 } else { i - 1 })
        .unwrap_or(0);
    app.menu_state.select(Some(prev));
}

fn start_spinner(app: &mut App, text: impl Into<String>) {
    app.spinner_active = true;
    app.spinner_text = text.into();
    app.spinner_idx = 0;
    app.spinner_last = Instant::now();
    app.status = format!("{} {}", app.spinner_text, SPINNER_FRAMES[app.spinner_idx]);
}

fn stop_spinner(app: &mut App) {
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

fn switch_view(app: &mut App, action: MenuAction) -> Result<()> {
    let idx = MENU_ITEMS.iter().position(|(_, a)| *a == action);
    if let Some(i) = idx {
        app.menu_state.select(Some(i));
    }
    match action {
        MenuAction::Confirm => process_input(app)?,
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

fn trigger_menu_action(app: &mut App) -> Result<()> {
    let idx = app.menu_state.selected().unwrap_or(0);
    let action = MENU_ITEMS
        .get(idx)
        .map(|(_, a)| *a)
        .unwrap_or(MenuAction::Confirm);
    switch_view(app, action)
}

fn start_search_task(app: &mut App, query: String) -> Result<()> {
    start_spinner(app, "搜索中…");
    let tx = app.worker_tx.clone();
    thread::spawn(move || {
        let result = search_books(&query);
        let _ = tx.send(WorkerMsg::SearchDone(result));
    });
    Ok(())
}

fn start_download_task(app: &mut App, book_id: String) -> Result<()> {
    start_spinner(app, format!("下载中: {book_id}"));
    let tx = app.worker_tx.clone();
    let cfg = app.config.clone();
    thread::spawn(move || {
        let result = downloader::download_book(&cfg, &book_id);
        let msg = WorkerMsg::DownloadDone { book_id, result };
        let _ = tx.send(msg);
    });
    Ok(())
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
                        app.status =
                            format!("找到 {} 本书，使用上下键选择，Enter 下载。", results.len());
                        app.results = results;
                        app.list_state.select(Some(0));
                        app.focus = Focus::Results;
                    }
                }
                Err(err) => {
                    app.status = format!("搜索失败: {err}");
                    app.push_message(format!("搜索失败: {err}"));
                }
            },
            WorkerMsg::DownloadDone { book_id, result } => match result {
                Ok(()) => {
                    app.status = format!("下载完成: {book_id}");
                    app.push_message("下载完成");
                }
                Err(err) => {
                    app.status = format!("下载失败: {err}");
                    app.push_message(format!("下载失败: {err}"));
                }
            },
        }
    }
    Ok(())
}

fn handle_mouse_home(app: &mut App, me: event::MouseEvent) -> Result<()> {
    if let Some(layout) = app.last_home_layout {
        let header = layout[0];
        let input_area = layout[1];
        let menu_area = layout[2];
        let results_area = layout[3];
        let status_area = layout[4];
        let pos_in = |area: Rect, col: u16, row: u16| {
            col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
        };
        if let MouseEventKind::Down(MouseButton::Left) = me.kind {
            if pos_in(input_area, me.column, me.row) {
                app.focus = Focus::Input;
                return Ok(());
            }
            if pos_in(menu_area, me.column, me.row) {
                app.focus = Focus::Menu;
                let idx = me.row.saturating_sub(menu_area.y + 1) as usize;
                if idx < MENU_ITEMS.len() {
                    app.menu_state.select(Some(idx));
                    trigger_menu_action(app)?;
                }
                return Ok(());
            }
            if pos_in(results_area, me.column, me.row) {
                if !app.results.is_empty() {
                    let idx = me.row.saturating_sub(results_area.y + 1) as usize;
                    if idx < app.results.len() {
                        app.list_state.select(Some(idx));
                        app.focus = Focus::Results;
                        start_download_task(app, app.results[idx].book_id.clone())?;
                    }
                }
                return Ok(());
            }
            if pos_in(status_area, me.column, me.row) || pos_in(header, me.column, me.row) {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn select_next_update(app: &mut App) {
    let list = if app.show_no_update {
        &app.update_no_updates
    } else {
        &app.update_entries
    };
    if list.is_empty() {
        app.update_state.select(None);
        return;
    }
    let next = app
        .update_state
        .selected()
        .map(|i| (i + 1) % list.len())
        .unwrap_or(0);
    app.update_state.select(Some(next));
}

fn select_prev_update(app: &mut App) {
    let list = if app.show_no_update {
        &app.update_no_updates
    } else {
        &app.update_entries
    };
    if list.is_empty() {
        app.update_state.select(None);
        return;
    }
    let prev = app
        .update_state
        .selected()
        .map(|i| if i == 0 { list.len() - 1 } else { i - 1 })
        .unwrap_or(0);
    app.update_state.select(Some(prev));
}

fn current_update_entry(app: &App) -> Option<UpdateEntry> {
    let list = if app.show_no_update {
        &app.update_no_updates
    } else {
        &app.update_entries
    };
    let idx = app.update_state.selected()?;
    list.get(idx).cloned()
}

fn handle_mouse_update(app: &mut App, me: event::MouseEvent) -> Result<()> {
    if let Some(layout) = app.last_update_layout {
        let list_area = layout[1];
        let pos_in = |area: Rect, col: u16, row: u16| {
            col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
        };
        if let MouseEventKind::Down(MouseButton::Left) = me.kind {
            if pos_in(list_area, me.column, me.row) {
                let idx = me.row.saturating_sub(list_area.y + 1) as usize;
                let list = if app.show_no_update {
                    &app.update_no_updates
                } else {
                    &app.update_entries
                };
                if idx < list.len() {
                    app.update_state.select(Some(idx));
                    if let Some(entry) = current_update_entry(app) {
                        start_download_task(app, entry.book_id.clone())?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn current_cfg_value(app: &App, field: ConfigField) -> String {
    match field {
        ConfigField::SavePath => app.config.save_path.clone(),
        ConfigField::NovelFormat => app.config.novel_format.clone(),
        ConfigField::EnableSegmentComments => app.config.enable_segment_comments.to_string(),
        ConfigField::UseOfficialApi => app.config.use_official_api.to_string(),
        ConfigField::MaxWorkers => app.config.max_workers.to_string(),
        ConfigField::MinWait => app.config.min_wait_time.to_string(),
        ConfigField::MaxWait => app.config.max_wait_time.to_string(),
    }
}

fn start_cfg_edit(app: &mut App, idx: usize) {
    if idx >= app.cfg_entries.len() {
        return;
    }
    app.cfg_editing = Some(idx);
    app.cfg_edit_buffer = current_cfg_value(app, app.cfg_entries[idx].field);
    app.status = format!("正在编辑: {}", app.cfg_entries[idx].title);
}

fn apply_cfg_edit(app: &mut App, idx: usize) -> Result<()> {
    if idx >= app.cfg_entries.len() {
        return Ok(());
    }
    let entry = &app.cfg_entries[idx];
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
        ConfigField::EnableSegmentComments => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            if val && app.config.novel_format.to_ascii_lowercase() != "epub" {
                app.status = "段评仅支持 epub，请先将格式改为 epub".to_string();
                return Ok(());
            }
            app.config.enable_segment_comments = val;
        }
        ConfigField::UseOfficialApi => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.use_official_api = val;
        }
        ConfigField::MaxWorkers => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入正整数"))?;
            if val == 0 {
                app.status = "最大线程数需大于 0".to_string();
                return Ok(());
            }
            app.config.max_workers = val;
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

fn show_update_menu(app: &mut App) -> Result<()> {
    app.status = "扫描本地小说…".to_string();
    let (updates, no_updates) = scan_updates(&app.config)?;
    if updates.is_empty() && no_updates.is_empty() {
        app.status = "未发现本地小说，先下载一本试试".to_string();
        app.view = View::Home;
        return Ok(());
    }
    app.update_entries = updates;
    app.update_no_updates = no_updates;
    if app.update_entries.is_empty() && !app.update_no_updates.is_empty() {
        app.show_no_update = true;
    } else {
        app.show_no_update = false;
    }
    if (!app.show_no_update && !app.update_entries.is_empty())
        || (app.show_no_update && !app.update_no_updates.is_empty())
    {
        app.update_state.select(Some(0));
    } else {
        app.update_state.select(None);
    }
    app.view = View::Update;
    app.status = "选择有更新的小说，Enter 开始".to_string();
    Ok(())
}

fn scan_updates(config: &Config) -> Result<(Vec<UpdateEntry>, Vec<UpdateEntry>)> {
    let mut updates = Vec::new();
    let mut no_updates = Vec::new();
    let save_dir = config.default_save_dir();
    if !save_dir.exists() {
        return Ok((updates, no_updates));
    }
    let dir_reader =
        fs::read_dir(&save_dir).with_context(|| format!("read dir {}", save_dir.display()))?;
    let client = DirectoryClient::new().context("init DirectoryClient")?;
    for entry in dir_reader.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let (book_id, book_name) = match name.split_once('_') {
            Some((id, n)) if id.chars().all(|c| c.is_ascii_digit()) => {
                (id.to_string(), n.to_string())
            }
            _ => continue,
        };

        let downloaded_count = read_downloaded_count(&path, &book_id).unwrap_or(0);
        let chapter_list = match client.fetch_directory(&book_id) {
            Ok(d) => d.chapters,
            Err(_) => Vec::new(),
        };
        if chapter_list.is_empty() {
            continue;
        }
        let total = chapter_list.len();
        let new_count = total.saturating_sub(downloaded_count);
        let label = format!("《{}》({}) — 新章节: {}", book_name, book_id, new_count);
        let entry = UpdateEntry {
            book_id: book_id.clone(),
            book_name: book_name.clone(),
            folder: path.clone(),
            label,
            new_count,
            has_update: new_count > 0,
        };
        if new_count > 0 {
            updates.push(entry);
        } else {
            no_updates.push(entry);
        }
    }
    Ok((updates, no_updates))
}

fn read_downloaded_count(folder: &Path, book_id: &str) -> Option<usize> {
    let status_new = folder.join("status.json");
    let status_old = folder.join(format!("chapter_status_{}.json", book_id));
    let path = if status_new.exists() {
        status_new
    } else if status_old.exists() {
        status_old
    } else {
        return None;
    };
    let data = fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&data).ok()?;
    let downloaded = value.get("downloaded")?.as_object()?;
    Some(downloaded.len())
}

fn current_selection_detail_lines(app: &App) -> Option<Vec<Line<'static>>> {
    let idx = app.list_state.selected()?;
    let item = app.results.get(idx)?;
    let mut lines = Vec::new();
    lines.push(Line::from(format!(
        "选中: 《{}》 | 作者: {} | ID: {}",
        item.title, item.author, item.book_id
    )));

    if let Some(detail) = item.detail.as_ref() {
        if let Some(count) = detail.chapter_count {
            lines.push(Line::from(format!("章节数: {}", count)));
        }
        if !detail.tags.is_empty() {
            lines.push(Line::from(format!("标签: {}", detail.tags.join(" | "))));
        }
        if let Some(desc) = detail.description.as_ref() {
            lines.push(Line::from(format!("简介: {}", truncate(desc, 220))));
        } else {
            lines.push(Line::from("简介: 暂无，按 i 获取详情"));
        }
    } else {
        lines.push(Line::from("简介: 未加载，按 i 获取详情"));
    }

    Some(lines)
}

fn truncate(text: &str, limit: usize) -> String {
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

fn draw_ui(frame: &mut ratatui::Frame, app: &mut App) {
    match app.view {
        View::Home => draw_home(frame, app),
        View::Config => draw_config(frame, app),
        View::Update => draw_update(frame, app),
        View::About => draw_about(frame, app),
        View::Cover => draw_cover(frame, app),
    }
}

fn draw_home(frame: &mut ratatui::Frame, app: &mut App) {
    let size = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Min(6),
            Constraint::Length(7),
        ])
        .split(size);
    if layout.len() == 5 {
        let mut arr = [Rect::default(); 5];
        arr.copy_from_slice(&layout);
        app.last_home_layout = Some(arr);
    }

    let header_line = Line::from(vec![
        Span::styled(
            "番茄小说下载器 TUI",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  输出目录: "),
        Span::styled(
            app.config.default_save_dir().display().to_string(),
            Style::default().fg(Color::Green),
        ),
        Span::raw("  |  c: 配置, q: 退出"),
    ]);

    let header = Paragraph::new(header_line).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Tomato Novel Downloader"),
    );
    frame.render_widget(header, layout[0]);

    let input_style = if app.focus == Focus::Input {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let input = Paragraph::new(format!("> {}", app.input))
        .style(input_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("输入书名/ID/链接 (Enter 确认, Tab 切换, q 退出, c 配置)"),
        );
    frame.render_widget(input, layout[1]);

    let menu_items: Vec<ListItem> = MENU_ITEMS
        .iter()
        .map(|(label, _)| ListItem::new(*label))
        .collect();
    let menu_style = if app.focus == Focus::Menu {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let menu_list = List::new(menu_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("操作 (Enter 或鼠标点击)"),
        )
        .highlight_style(menu_style.add_modifier(Modifier::BOLD))
        .highlight_symbol(">> ");
    frame.render_stateful_widget(menu_list, layout[2], &mut app.menu_state);

    let items: Vec<ListItem> = if app.results.is_empty() {
        vec![ListItem::new("无搜索结果")]
    } else {
        app.results
            .iter()
            .map(|b| {
                let label = format!("{} | {} | {}", b.title, b.book_id, b.author);
                ListItem::new(label)
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("搜索结果 (上下选择, Enter 下载)"),
        )
        .highlight_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(list, layout[3], &mut app.list_state);

    let mut msg_lines: Vec<Line> = Vec::new();
    if let Some(detail) = current_selection_detail_lines(app) {
        msg_lines.extend(detail);
        msg_lines.push(Line::from(""));
    }
    msg_lines.push(Line::from(app.status.clone()));
    if !app.messages.is_empty() {
        msg_lines.push(Line::from(""));
        msg_lines.extend(
            app.messages
                .iter()
                .rev()
                .take(6)
                .rev()
                .map(|m| Line::from(m.as_str())),
        );
    }

    let messages = Paragraph::new(msg_lines)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("状态 / 消息"));

    frame.render_widget(messages, layout[4]);
}

fn draw_config(frame: &mut ratatui::Frame, app: &mut App) {
    let size = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(9),
        ])
        .split(size);

    let header_line = Line::from(vec![
        Span::styled(
            "配置编辑",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  回车编辑/保存, Esc 取消, q 返回"),
    ]);

    let header = Paragraph::new(header_line).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Tomato Novel Downloader"),
    );
    frame.render_widget(header, layout[0]);

    let items: Vec<ListItem> = app
        .cfg_entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let val = current_cfg_value(app, entry.field);
            let mut content = vec![Span::raw(format!("{}: {}", entry.title, val))];
            if app.cfg_editing == Some(idx) {
                content.push(Span::raw("  [编辑中] "));
                content.push(Span::styled(
                    app.cfg_edit_buffer.clone(),
                    Style::default().fg(Color::Yellow),
                ));
            }
            ListItem::new(Line::from(content))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("配置项 (上下选择, 回车编辑)"),
        )
        .highlight_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(list, layout[1], &mut app.cfg_state);

    let mut msg_lines: Vec<Line> = vec![Line::from(app.status.clone())];
    if app.cfg_editing.is_some() {
        msg_lines.push(Line::from("输入新值后按 Enter 保存，Esc 取消。"));
    } else {
        msg_lines.push(Line::from("上下选择，Enter 修改，q 返回主界面。"));
    }

    let messages = Paragraph::new(msg_lines)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("状态"));

    frame.render_widget(messages, layout[2]);
}

fn draw_update(frame: &mut ratatui::Frame, app: &mut App) {
    let size = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(4),
        ])
        .split(size);
    if layout.len() == 3 {
        let mut arr = [Rect::default(); 3];
        arr.copy_from_slice(&layout);
        app.last_update_layout = Some(arr);
    }

    let header_line = Line::from(vec![
        Span::styled(
            "更新",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  上下选择，Enter 下载，n 切换无更新，q 返回"),
    ]);
    let header =
        Paragraph::new(header_line).block(Block::default().borders(Borders::ALL).title("更新检测"));
    frame.render_widget(header, layout[0]);

    let list = if app.show_no_update {
        &app.update_no_updates
    } else {
        &app.update_entries
    };
    let items: Vec<ListItem> = if list.is_empty() {
        vec![ListItem::new("没有可展示的项目")]
    } else {
        list.iter()
            .map(|u| ListItem::new(u.label.clone()))
            .collect()
    };
    let list_widget = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(if app.show_no_update {
                    "无更新书籍"
                } else {
                    "有更新书籍"
                }),
        )
        .highlight_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(list_widget, layout[1], &mut app.update_state);

    let mut msg_lines = vec![Line::from(app.status.clone())];
    if !app.update_entries.is_empty() {
        msg_lines.push(Line::from(format!(
            "有更新: {} 本",
            app.update_entries.len()
        )));
    }
    if !app.update_no_updates.is_empty() {
        msg_lines.push(Line::from(format!(
            "无更新: {} 本 (按 n 查看)",
            app.update_no_updates.len()
        )));
    }

    let footer = Paragraph::new(msg_lines)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("提示"));
    frame.render_widget(footer, layout[2]);
}

fn draw_cover(frame: &mut ratatui::Frame, app: &mut App) {
    let size = frame.size();
    let title = if app.cover_title.is_empty() {
        "封面预览".to_string()
    } else {
        app.cover_title.clone()
    };

    let lines: Vec<Line> = if app.cover_lines.is_empty() {
        vec![Line::from("未找到封面，按 q 返回")]
    } else {
        app.cover_lines.iter().cloned().map(Line::from).collect()
    };

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, size);
}

fn draw_about(frame: &mut ratatui::Frame, _app: &mut App) {
    let size = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(8)])
        .split(size);

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "关于 / About",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  按 q/Enter 返回"),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Tomato Novel Downloader"),
    );
    frame.render_widget(header, layout[0]);

    let text = "项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader\nFork From: https://github.com/Dlmily/Tomato-Novel-Downloader-Lite\n作者: zhongbai2333\n本项目仅供学习交流使用，请勿用于商业及违法行为。";
    let body = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("项目说明"));
    frame.render_widget(body, layout[1]);
}
