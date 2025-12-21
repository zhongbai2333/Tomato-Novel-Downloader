use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tracing::{debug, info};

use crate::base_system::context::Config;

pub type DownloadedMap = HashMap<String, (String, Option<String>)>;

pub struct BookManager {
    pub config: Config,
    pub book_name: String,
    pub book_id: String,
    pub author: String,
    pub tags: String,
    pub description: String,
    pub end: bool,
    pub downloaded: DownloadedMap,
    has_download_activity: bool,
    status_folder: PathBuf,
    status_file: PathBuf,
    status_folder_preexisting: bool,
}

impl BookManager {
    pub fn new(mut config: Config) -> Self {
        // 选择状态目录
        let existing_folder = config
            .get_status_folder_path()
            .or_else(|| Some(config.default_save_dir().join("_session")));

        let status_folder = existing_folder.unwrap_or_else(|| PathBuf::from("._session"));
        let _ = fs::create_dir_all(&status_folder);
        let status_file = status_folder.join("status.json");

        // 注册状态目录
        config.mark_status_folder_claimed(&status_folder);
        let status_folder_preexisting =
            !config.status_folder_was_created_this_session(&status_folder);

        Self {
            config,
            book_name: String::new(),
            book_id: String::new(),
            author: String::new(),
            tags: String::new(),
            description: String::new(),
            end: false,
            downloaded: HashMap::new(),
            has_download_activity: false,
            status_folder,
            status_file,
            status_folder_preexisting,
        }
    }

    /// 尝试加载已存在的下载状态。
    pub fn load_existing_status(&mut self, book_id: &str, book_name: &str) -> bool {
        // 防御：确保状态目录存在
        let _ = fs::create_dir_all(&self.status_folder);

        let data = self
            .read_json_file(&self.status_file)
            .or_else(|| self.read_legacy_file(book_id));

        let Some(data) = data else {
            return false;
        };

        if let Some(dl) = data.get("downloaded").and_then(|v| v.as_object()) {
            for (cid, pair) in dl {
                if let Some(arr) = pair.as_array() {
                    let title = arr.get(0).and_then(|v| v.as_str()).unwrap_or("");
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
        }

        self.description = if self.description.is_empty() {
            data.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        } else {
            self.description.clone()
        };

        info!(target: "book_manager", "loaded resume state: chapters={}", self.downloaded.len());
        true
    }

    pub fn save_chapter(&mut self, chapter_id: &str, title: &str, content: &str) {
        self.downloaded.insert(
            chapter_id.to_string(),
            (title.to_string(), Some(content.to_string())),
        );
        self.has_download_activity = true;
    }

    pub fn save_error_chapter(&mut self, chapter_id: &str, title: &str) {
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
            "end": self.end,
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

    pub fn cleanup_status_folder(&mut self) -> std::io::Result<()> {
        if self.status_folder_preexisting {
            return Ok(());
        }
        if self.has_download_activity {
            return Ok(());
        }
        fs::remove_dir_all(&self.status_folder)
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
