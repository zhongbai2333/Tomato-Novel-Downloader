//! 书籍下载过程的状态管理与落盘。

use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;
use tracing::{debug, info};

use crate::base_system::context::{Config, safe_fs_name};

pub type DownloadedMap = HashMap<String, (String, Option<String>)>;

pub struct BookManager {
    pub config: Config,
    pub book_name: String,
    pub book_id: String,
    pub author: String,
    pub tags: String,
    pub description: String,
    pub finished: Option<bool>,
    pub end: bool,
    pub chapter_count: Option<usize>,
    pub word_count: Option<usize>,
    pub score: Option<f32>,
    pub read_count_text: Option<String>,
    pub category: Option<String>,
    /// 原始书名（用于"下载完后选择"功能）
    pub original_book_name: Option<String>,
    /// 短书名（用于"下载完后选择"功能）
    pub book_short_name: Option<String>,
    /// 是否已在下载完成后确认过书名
    pub book_name_selected_after_download: bool,
    pub downloaded: DownloadedMap,
    pub ignore_updates: bool,
    has_download_activity: bool,
    status_folder: PathBuf,
    status_file: PathBuf,
    status_folder_preexisting: bool,
}

const RESUME_JOURNAL_FILE: &str = "downloaded_chapters.jsonl";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ResumeJournalRecord {
    id: String,
    title: String,
    content: String,
}

impl BookManager {
    pub fn new(mut config: Config, book_id: &str, book_name: &str) -> std::io::Result<Self> {
        let target = match config.status_folder_path(book_name, book_id, None) {
            Ok(p) => p,
            Err(_) => {
                let fallback = config.default_save_dir().join(format!(
                    "{}_{}",
                    book_id,
                    safe_fs_name(book_name, "_", 120)
                ));
                fs::create_dir_all(&fallback)?;
                fallback
            }
        };

        config.mark_status_folder_claimed(&target);
        let status_folder_preexisting = !config.status_folder_was_created_this_session(&target);
        let status_file = target.join("status.json");

        Ok(Self {
            config,
            book_name: String::new(),
            book_id: String::new(),
            author: String::new(),
            tags: String::new(),
            description: String::new(),
            finished: None,
            end: false,
            chapter_count: None,
            word_count: None,
            score: None,
            read_count_text: None,
            category: None,
            original_book_name: None,
            book_short_name: None,
            book_name_selected_after_download: false,
            downloaded: HashMap::new(),
            ignore_updates: false,
            has_download_activity: false,
            status_folder: target,
            status_file,
            status_folder_preexisting,
        })
    }

    /// 尝试加载已存在的下载状态。
    pub fn load_existing_status(&mut self, book_id: &str, book_name: &str) -> bool {
        // 防御：确保状态目录存在
        let _ = fs::create_dir_all(&self.status_folder);

        let data = self
            .read_json_file(&self.status_file)
            .or_else(|| self.read_legacy_file(book_id));

        // 如果 status.json 不存在/损坏，仍尝试用追加日志恢复已下载章节。
        let Some(data) = data else {
            let journal_loaded = self.merge_resume_journal();
            if journal_loaded {
                if self.book_id.is_empty() {
                    self.book_id = book_id.to_string();
                }
                if self.book_name.is_empty() {
                    self.book_name = book_name.to_string();
                }
                info!(target: "book_manager", "loaded resume journal only: chapters={}", self.downloaded.len());
                return true;
            }
            return false;
        };

        if let Some(dl) = data.get("downloaded").and_then(|v| v.as_object()) {
            for (cid, pair) in dl {
                if let Some(arr) = pair.as_array() {
                    let title = arr.first().and_then(|v| v.as_str()).unwrap_or("");
                    let content = arr.get(1).and_then(|v| v.as_str()).map(|s| s.to_string());
                    self.downloaded
                        .insert(cid.clone(), (title.to_string(), content));
                }
            }
        }

        self.book_id = if self.book_id.is_empty() {
            data.get("book_id")
                .and_then(|v| v.as_str())
                .unwrap_or(book_id)
                .to_string()
        } else {
            self.book_id.clone()
        };

        self.book_name = if self.book_name.is_empty() {
            data.get("book_name")
                .and_then(|v| v.as_str())
                .unwrap_or(book_name)
                .to_string()
        } else {
            self.book_name.clone()
        };

        self.author = if self.author.is_empty() {
            data.get("author")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        } else {
            self.author.clone()
        };

        self.tags = if self.tags.is_empty() {
            data.get("tags")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        } else {
            self.tags.clone()
        };

        if let Some(end) = data.get("end").and_then(|v| v.as_bool()) {
            self.end = end;
            if self.finished.is_none() {
                self.finished = Some(end);
            }
        }

        if self.finished.is_none()
            && let Some(b) = data.get("finished").and_then(|v| v.as_bool())
        {
            self.finished = Some(b);
        }

        if self.chapter_count.is_none() {
            self.chapter_count = data
                .get("chapter_count")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
        }
        if self.word_count.is_none() {
            self.word_count = data
                .get("word_count")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
        }
        if self.score.is_none() {
            self.score = data.get("score").and_then(|v| {
                v.as_f64()
                    .map(|n| n as f32)
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f32>().ok()))
            });
        }
        if self.read_count_text.is_none() {
            self.read_count_text = data
                .get("read_count_text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
        if self.category.is_none() {
            self.category = data
                .get("category")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }

        self.description = if self.description.is_empty() {
            data.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        } else {
            self.description.clone()
        };

        // 加载忽略更新状态
        self.ignore_updates = data
            .get("ignore_updates")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // 追加日志比 status.json 更“实时”：合并后可覆盖 status.json 未及时写入的最后几章。
        let _ = self.merge_resume_journal();

        info!(target: "book_manager", "loaded resume state: chapters={}", self.downloaded.len());
        true
    }

    pub fn save_chapter(&mut self, chapter_id: &str, title: &str, content: &str) {
        debug!(target: "book_manager", chapter_id, title, bytes = content.len(), "保存章节内容");
        self.downloaded.insert(
            chapter_id.to_string(),
            (title.to_string(), Some(content.to_string())),
        );
        self.has_download_activity = true;
    }

    /// 追加式持久化单章内容（JSONL）。用于断点续传：即使进程突然退出，也能恢复已下载章节内容。
    pub fn append_downloaded_chapter(&self, chapter_id: &str, title: &str, content: &str) {
        if chapter_id.trim().is_empty() || content.is_empty() {
            return;
        }

        let record = ResumeJournalRecord {
            id: chapter_id.to_string(),
            title: title.to_string(),
            content: content.to_string(),
        };

        if let Err(e) = fs::create_dir_all(&self.status_folder) {
            debug!(target: "book_manager", error = ?e, "create status folder failed (resume journal)");
            return;
        }

        let path = self.resume_journal_path();
        let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                debug!(target: "book_manager", error = ?e, "open resume journal failed");
                return;
            }
        };
        let line = match serde_json::to_string(&record) {
            Ok(s) => s,
            Err(e) => {
                debug!(target: "book_manager", error = ?e, "serialize resume journal failed");
                return;
            }
        };
        if let Err(e) = writeln!(file, "{line}") {
            debug!(target: "book_manager", error = ?e, "write resume journal failed");
            return;
        }
        let _ = file.flush();
    }

    pub fn save_error_chapter(&mut self, chapter_id: &str, title: &str) {
        debug!(target: "book_manager", chapter_id, title, "记录异常章节");
        self.downloaded
            .insert(chapter_id.to_string(), (title.to_string(), None));
        self.has_download_activity = true;
    }

    pub fn save_download_status(&self) {
        let data = serde_json::json!({
            "book_id": self.book_id,
            "book_name": self.book_name,
            "author": self.author,
            "tags": self.tags,
            "description": self.description,
            "finished": self.finished,
            "end": self.end,
            "chapter_count": self.chapter_count,
            "word_count": self.word_count,
            "score": self.score,
            "read_count_text": self.read_count_text,
            "category": self.category,
            "ignore_updates": self.ignore_updates,
            "downloaded": self.downloaded_as_json(),
        });

        if let Err(e) = fs::create_dir_all(&self.status_folder) {
            debug!(error = ?e, "create status folder failed");
            return;
        }
        match fs::write(
            &self.status_file,
            serde_json::to_string_pretty(&data).unwrap_or_default(),
        ) {
            Ok(_) => {}
            Err(e) => debug!(target: "book_manager", error = ?e, "write status.json failed"),
        }
    }

    /// 切换忽略更新状态并保存
    pub fn toggle_ignore_updates(&mut self) -> bool {
        self.ignore_updates = !self.ignore_updates;
        self.save_download_status();
        self.ignore_updates
    }

    pub fn book_folder(&self) -> &Path {
        &self.status_folder
    }

    fn resume_journal_path(&self) -> PathBuf {
        self.status_folder.join(RESUME_JOURNAL_FILE)
    }

    /// 合并追加日志中的章节内容到 `downloaded`。返回是否成功加载到至少 1 条记录。
    fn merge_resume_journal(&mut self) -> bool {
        let path = self.resume_journal_path();
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return false,
        };

        let mut loaded_any = false;
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let Ok(line) = line else { continue };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let rec: ResumeJournalRecord = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue, // tolerate partial last line / corruption
            };
            if rec.id.trim().is_empty() || rec.content.is_empty() {
                continue;
            }

            loaded_any = true;
            let should_insert = match self.downloaded.get(&rec.id) {
                None => true,
                Some((_t, c)) => c.is_none(),
            };
            if should_insert {
                self.downloaded
                    .insert(rec.id, (rec.title, Some(rec.content)));
            }
        }

        loaded_any
    }

    pub fn default_save_dir(&self) -> PathBuf {
        self.config.default_save_dir()
    }

    pub fn cleanup_status_folder(&mut self) -> std::io::Result<()> {
        if self.status_folder_preexisting {
            return Ok(());
        }
        if self.has_download_activity {
            return Ok(());
        }
        fs::remove_dir_all(&self.status_folder)
    }

    pub fn delete_status_folder(&mut self) -> std::io::Result<()> {
        if self.status_folder.exists() {
            fs::remove_dir_all(&self.status_folder)?;
            self.config.mark_status_folder_removed(&self.status_folder);
        }
        Ok(())
    }

    fn downloaded_as_json(&self) -> serde_json::Map<String, Value> {
        self.downloaded
            .iter()
            .map(|(k, (title, content))| {
                let arr = if let Some(c) = content {
                    serde_json::json!([title, c])
                } else {
                    serde_json::json!([title, Value::Null])
                };
                (k.clone(), arr)
            })
            .collect()
    }

    fn read_json_file(&self, path: &Path) -> Option<Value> {
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn read_legacy_file(&self, book_id: &str) -> Option<Value> {
        let legacy_path = self
            .status_folder
            .join(format!("chapter_status_{}.json", book_id));
        self.read_json_file(&legacy_path).map(|data| {
            if data.get("downloaded").is_none() {
                // 旧格式：直接包裹 downloaded
                let mut map = serde_json::Map::new();
                map.insert("downloaded".to_string(), data);
                Value::Object(map)
            } else {
                data
            }
        })
    }
}
