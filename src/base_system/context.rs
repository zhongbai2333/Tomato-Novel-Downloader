//! 全局配置结构（Config）与默认值。
//!
//! 该模块同时提供生成 `config.yml` 的字段元信息。

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::config::{ConfigSpec, FieldMeta};

pub const OUTPUT_FORMAT_TXT: &str = "txt";
pub const OUTPUT_FORMAT_EPUB: &str = "epub";
pub const OUTPUT_FORMAT_PDF: &str = "pdf";
pub const OUTPUT_FORMAT_BULK_TXT: &str = "bulk_txt";
pub const OUTPUT_FORMAT_ASK_AFTER_DOWNLOAD: &str = "ask_after_download";

pub fn output_format_choices() -> &'static [(&'static str, &'static str)] {
    static CHOICES: [(&str, &str); 5] = [
        (OUTPUT_FORMAT_TXT, "txt 格式"),
        (OUTPUT_FORMAT_EPUB, "epub 格式"),
        (OUTPUT_FORMAT_PDF, "pdf 格式"),
        (OUTPUT_FORMAT_BULK_TXT, "散装文件"),
        (OUTPUT_FORMAT_ASK_AFTER_DOWNLOAD, "下载后选择"),
    ];
    &CHOICES
}

pub fn output_format_label(choice: &str) -> &'static str {
    let normalized = choice.trim().to_ascii_lowercase();
    output_format_choices()
        .iter()
        .find(|(value, _)| *value == normalized)
        .map(|(_, label)| *label)
        .unwrap_or("txt 格式")
}

pub fn output_format_value_from_label(label: &str) -> Option<&'static str> {
    output_format_choices()
        .iter()
        .find(|(_, candidate)| *candidate == label)
        .map(|(value, _)| *value)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // 程序配置
    #[serde(default = "default_false")]
    pub old_cli: bool,

    // 网络配置
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_max_wait_time")]
    pub max_wait_time: u64,
    #[serde(default = "default_min_wait_time")]
    pub min_wait_time: u64,
    #[serde(default = "default_min_connect_timeout")]
    pub min_connect_timeout: f64,

    // 保存配置
    #[serde(default = "default_novel_format")]
    pub novel_format: String,
    #[serde(default = "default_false")]
    pub bulk_files: bool,
    #[serde(default = "default_true")]
    pub auto_clear_dump: bool,
    #[serde(default = "default_false")]
    pub auto_open_downloaded_files: bool,
    #[serde(default = "default_false")]
    pub enable_audiobook: bool,
    #[serde(default = "default_audiobook_voice")]
    pub audiobook_voice: String,
    #[serde(default = "default_audiobook_rate")]
    pub audiobook_rate: String,
    #[serde(default = "default_audiobook_volume")]
    pub audiobook_volume: String,
    #[serde(default = "default_audiobook_pitch")]
    pub audiobook_pitch: String,
    #[serde(default = "default_audiobook_format")]
    pub audiobook_format: String,
    #[serde(default = "default_audiobook_concurrency")]
    pub audiobook_concurrency: usize,
    #[serde(default = "default_audiobook_tts_provider")]
    pub audiobook_tts_provider: String,
    #[serde(default = "default_string")]
    pub audiobook_tts_api_url: String,
    #[serde(default = "default_string")]
    pub audiobook_tts_api_token: String,
    #[serde(default = "default_string")]
    pub audiobook_tts_model: String,

    // 路径配置
    #[serde(default)]
    pub save_path: String,

    // API 配置
    #[serde(default = "default_true")]
    pub use_official_api: bool,
    #[serde(default)]
    pub api_endpoints: Vec<String>,

    // 段评配置
    #[serde(default = "default_false")]
    pub enable_segment_comments: bool,
    #[serde(default = "default_segment_comments_top_n")]
    pub segment_comments_top_n: usize,
    #[serde(default = "default_segment_comments_workers")]
    pub segment_comments_workers: usize,

    // 媒体配置
    #[serde(default = "default_true")]
    pub download_comment_images: bool,
    #[serde(default = "default_true")]
    pub download_comment_avatars: bool,
    #[serde(default = "default_media_download_workers")]
    pub media_download_workers: usize,
    #[serde(default = "default_blocked_media_domains")]
    pub blocked_media_domains: Vec<String>,
    #[serde(default = "default_false")]
    pub force_convert_images_to_jpeg: bool,
    #[serde(default = "default_true")]
    pub jpeg_retry_convert: bool,
    #[serde(default = "default_jpeg_quality")]
    pub jpeg_quality: u8,
    #[serde(default = "default_true")]
    pub convert_heic_to_jpeg: bool,
    #[serde(default = "default_false")]
    pub keep_heic_original: bool,
    #[serde(default = "default_first_line_indent_em")]
    pub first_line_indent_em: f32,
    #[serde(default = "default_media_limit_per_chapter")]
    pub media_limit_per_chapter: usize,
    #[serde(default = "default_media_max_dimension_px")]
    pub media_max_dimension_px: u32,

    // 文件管理配置
    #[serde(default = "default_true")]
    pub allow_overwrite_files: bool,
    #[serde(default = "default_preferred_book_name_field")]
    pub preferred_book_name_field: String,
    /// 下载完成后询问用户选择输出格式（txt/epub）
    #[serde(default = "default_false")]
    pub ask_format_after_download: bool,
    /// PDF 字体文件路径，留空自动检测系统字体
    #[serde(default)]
    pub pdf_font_path: Option<String>,
    #[serde(skip)]
    folder_path: Option<PathBuf>,
    #[serde(skip)]
    last_status_was_new: bool,
    #[serde(skip)]
    last_status_claimed: bool,
    #[serde(skip)]
    status_registry: Vec<StatusEntry>,
}

#[derive(Debug, Clone)]
struct StatusEntry {
    path: PathBuf,
    is_new: bool,
    claimed: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            old_cli: default_false(),
            max_workers: default_max_workers(),
            request_timeout: default_request_timeout(),
            max_retries: default_max_retries(),
            max_wait_time: default_max_wait_time(),
            min_wait_time: default_min_wait_time(),
            min_connect_timeout: default_min_connect_timeout(),
            novel_format: default_novel_format(),
            bulk_files: default_false(),
            auto_clear_dump: default_true(),
            auto_open_downloaded_files: default_false(),
            enable_audiobook: default_false(),
            audiobook_voice: default_audiobook_voice(),
            audiobook_rate: default_audiobook_rate(),
            audiobook_volume: default_audiobook_volume(),
            audiobook_pitch: default_audiobook_pitch(),
            audiobook_format: default_audiobook_format(),
            audiobook_concurrency: default_audiobook_concurrency(),
            audiobook_tts_provider: default_audiobook_tts_provider(),
            audiobook_tts_api_url: default_string(),
            audiobook_tts_api_token: default_string(),
            audiobook_tts_model: default_string(),
            save_path: String::new(),
            use_official_api: default_true(),
            api_endpoints: Vec::new(),
            enable_segment_comments: default_false(),
            segment_comments_top_n: default_segment_comments_top_n(),
            segment_comments_workers: default_segment_comments_workers(),
            download_comment_images: default_true(),
            download_comment_avatars: default_true(),
            media_download_workers: default_media_download_workers(),
            blocked_media_domains: default_blocked_media_domains(),
            force_convert_images_to_jpeg: default_false(),
            jpeg_retry_convert: default_true(),
            jpeg_quality: default_jpeg_quality(),
            convert_heic_to_jpeg: default_true(),
            keep_heic_original: default_false(),
            first_line_indent_em: default_first_line_indent_em(),
            media_limit_per_chapter: default_media_limit_per_chapter(),
            media_max_dimension_px: default_media_max_dimension_px(),
            pdf_font_path: None,
            allow_overwrite_files: default_true(),
            preferred_book_name_field: default_preferred_book_name_field(),
            ask_format_after_download: default_false(),
            folder_path: None,
            last_status_was_new: false,
            last_status_claimed: false,
            status_registry: Vec::new(),
        }
    }
}

impl ConfigSpec for Config {
    const FILE_NAME: &'static str = "config.yml";

    fn fields() -> &'static [FieldMeta] {
        static FIELDS: [FieldMeta; 44] = [
            FieldMeta {
                name: "old_cli",
                description: "是否使用老版本命令行界面",
            },
            FieldMeta {
                name: "max_workers",
                description: "最大并发线程数",
            },
            FieldMeta {
                name: "request_timeout",
                description: "请求超时时间（秒）",
            },
            FieldMeta {
                name: "max_retries",
                description: "最大重试次数",
            },
            FieldMeta {
                name: "max_wait_time",
                description: "最大冷却时间, 单位ms",
            },
            FieldMeta {
                name: "min_wait_time",
                description: "最小冷却时间, 单位ms",
            },
            FieldMeta {
                name: "min_connect_timeout",
                description: "最小连接超时时间",
            },
            FieldMeta {
                name: "novel_format",
                description: "保存小说格式, 可选: [txt, epub, pdf]",
            },
            FieldMeta {
                name: "bulk_files",
                description: "是否以散装形式保存小说",
            },
            FieldMeta {
                name: "auto_clear_dump",
                description: "是否自动清理缓存文件",
            },
            FieldMeta {
                name: "auto_open_downloaded_files",
                description: "下载完成后自动用默认应用打开生成的小说文件/文件夹（txt/epub）",
            },
            FieldMeta {
                name: "enable_audiobook",
                description: "是否使用 Edge TTS 生成有声小说",
            },
            FieldMeta {
                name: "audiobook_voice",
                description: "Edge TTS 发音人",
            },
            FieldMeta {
                name: "audiobook_rate",
                description: "Edge TTS 语速调整，例如 +0%、-10%",
            },
            FieldMeta {
                name: "audiobook_volume",
                description: "Edge TTS 音量调整，例如 +0%、-10%",
            },
            FieldMeta {
                name: "audiobook_pitch",
                description: "Edge TTS 音调调整（留空表示默认）",
            },
            FieldMeta {
                name: "audiobook_format",
                description: "有声小说输出格式，可选 mp3 或 wav",
            },
            FieldMeta {
                name: "audiobook_concurrency",
                description: "Edge TTS 有声小说并发生成的最大章节数",
            },
            FieldMeta {
                name: "audiobook_tts_provider",
                description: "TTS 服务类型，可选 edge/third_party",
            },
            FieldMeta {
                name: "audiobook_tts_api_url",
                description: "第三方 TTS API 地址（可填写本地服务，如 http://localhost:8000）",
            },
            FieldMeta {
                name: "audiobook_tts_api_token",
                description: "第三方 TTS API Token（如无可留空）",
            },
            FieldMeta {
                name: "audiobook_tts_model",
                description: "第三方 TTS 模型名称或 ID",
            },
            FieldMeta {
                name: "save_path",
                description: "保存路径",
            },
            FieldMeta {
                name: "use_official_api",
                description: "使用官方API",
            },
            FieldMeta {
                name: "api_endpoints",
                description: "API列表",
            },
            FieldMeta {
                name: "enable_segment_comments",
                description: "是否下载段评（段落评论）",
            },
            FieldMeta {
                name: "segment_comments_top_n",
                description: "每段最多保存的评论数",
            },
            FieldMeta {
                name: "segment_comments_workers",
                description: "段评抓取的并发线程数（每章内）",
            },
            FieldMeta {
                name: "download_comment_images",
                description: "是否下载评论区图片（不含头像）",
            },
            FieldMeta {
                name: "download_comment_avatars",
                description: "是否下载评论区头像",
            },
            FieldMeta {
                name: "media_download_workers",
                description: "评论图片/头像下载并发线程数",
            },
            FieldMeta {
                name: "blocked_media_domains",
                description: "拒绝下载的图片域名（包含匹配）",
            },
            FieldMeta {
                name: "force_convert_images_to_jpeg",
                description: "是否强制将所有下载图片转码为 JPEG",
            },
            FieldMeta {
                name: "jpeg_retry_convert",
                description: "若返回非 JPEG 且可解码则转码为 JPEG 保存",
            },
            FieldMeta {
                name: "jpeg_quality",
                description: "JPEG 转码质量 (0-100)",
            },
            FieldMeta {
                name: "convert_heic_to_jpeg",
                description: "检测到 HEIC/HEIF 时转码为 JPEG",
            },
            FieldMeta {
                name: "keep_heic_original",
                description: "无法转码时是否保留 .heic/.heif",
            },
            FieldMeta {
                name: "first_line_indent_em",
                description: "EPUB 段落首行缩进 em 数",
            },
            FieldMeta {
                name: "media_limit_per_chapter",
                description: "每章最多下载的媒体数（0 表示不限制）",
            },
            FieldMeta {
                name: "media_max_dimension_px",
                description: "图片最长边像素上限，>0 时缩放并转成 JPEG",
            },
            FieldMeta {
                name: "allow_overwrite_files",
                description: "是否允许覆盖已存在的文件",
            },
            FieldMeta {
                name: "preferred_book_name_field",
                description: "优先使用的书名字段 (book_name/original_book_name/book_short_name/ask_after_download)",
            },
            FieldMeta {
                name: "pdf_font_path",
                description: "PDF 字体文件路径, 留空自动检测系统 CJK 字体",
            },
            FieldMeta {
                name: "ask_format_after_download",
                description: "是否在下载完成后询问用户选择输出格式（true 时可选 txt/epub/pdf/散装文件）",
            },
        ];
        &FIELDS
    }
}

impl Config {
    pub fn configured_output_format_choice(&self) -> &'static str {
        if self.bulk_files && self.novel_format.eq_ignore_ascii_case(OUTPUT_FORMAT_TXT) {
            return OUTPUT_FORMAT_BULK_TXT;
        }

        match self.novel_format.trim().to_ascii_lowercase().as_str() {
            OUTPUT_FORMAT_EPUB => OUTPUT_FORMAT_EPUB,
            OUTPUT_FORMAT_PDF => OUTPUT_FORMAT_PDF,
            _ => OUTPUT_FORMAT_TXT,
        }
    }

    pub fn current_output_format_choice(&self) -> &'static str {
        if self.ask_format_after_download {
            return OUTPUT_FORMAT_ASK_AFTER_DOWNLOAD;
        }
        self.configured_output_format_choice()
    }

    pub fn apply_output_format_choice(&mut self, choice: &str) -> Result<(), String> {
        let normalized = choice.trim().to_ascii_lowercase();
        match normalized.as_str() {
            OUTPUT_FORMAT_TXT => {
                self.novel_format = OUTPUT_FORMAT_TXT.to_string();
                self.bulk_files = false;
                self.ask_format_after_download = false;
            }
            OUTPUT_FORMAT_EPUB => {
                self.novel_format = OUTPUT_FORMAT_EPUB.to_string();
                self.bulk_files = false;
                self.ask_format_after_download = false;
            }
            OUTPUT_FORMAT_PDF => {
                self.novel_format = OUTPUT_FORMAT_PDF.to_string();
                self.bulk_files = false;
                self.ask_format_after_download = false;
            }
            OUTPUT_FORMAT_BULK_TXT => {
                self.novel_format = OUTPUT_FORMAT_TXT.to_string();
                self.bulk_files = true;
                self.ask_format_after_download = false;
            }
            OUTPUT_FORMAT_ASK_AFTER_DOWNLOAD => {
                self.ask_format_after_download = true;
                self.bulk_files = false;
                if self.novel_format.trim().is_empty() {
                    self.novel_format = default_novel_format();
                }
            }
            _ => {
                return Err("保存格式仅支持 txt/epub/pdf/散装文件/下载后选择".to_string());
            }
        }

        self.normalize_output_format_fields();
        Ok(())
    }

    pub fn normalize_output_format_fields(&mut self) {
        let mut normalized = self.novel_format.trim().to_ascii_lowercase();
        if normalized == OUTPUT_FORMAT_BULK_TXT {
            normalized = OUTPUT_FORMAT_TXT.to_string();
            self.bulk_files = true;
        }
        if normalized != OUTPUT_FORMAT_TXT
            && normalized != OUTPUT_FORMAT_EPUB
            && normalized != OUTPUT_FORMAT_PDF
        {
            normalized = default_novel_format();
        }

        self.novel_format = normalized;
        if self.ask_format_after_download || self.novel_format != OUTPUT_FORMAT_TXT {
            self.bulk_files = false;
        }
    }

    /// 解析 PDF 字体路径：用户指定 > 系统自动检测
    pub fn resolve_pdf_font_path(&self) -> Option<PathBuf> {
        if let Some(ref p) = self.pdf_font_path {
            let p = PathBuf::from(p);
            if p.exists() {
                return Some(p);
            }
        }
        // 按操作系统自动检测常见 CJK 字体
        let candidates: &[&str] = if cfg!(target_os = "windows") {
            // genpdf/rusttype 不支持 .ttc 集合字体，优先使用 .ttf 单字体
            &[
                r"C:\Windows\Fonts\simhei.ttf",
                r"C:\Windows\Fonts\simkai.ttf",
                r"C:\Windows\Fonts\msyh.ttf",
                r"C:\Windows\Fonts\simsun.ttf",
            ]
        } else if cfg!(target_os = "macos") {
            &[
                "/System/Library/Fonts/PingFang.ttc",
                "/Library/Fonts/Arial Unicode.ttf",
            ]
        } else {
            &[
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/wenquanyi/wqy-microhei.ttc",
                "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            ]
        };
        candidates.iter().map(PathBuf::from).find(|p| p.exists())
    }

    pub fn default_save_dir(&self) -> PathBuf {
        if self.save_path.trim().is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            PathBuf::from(&self.save_path)
        }
    }

    pub fn find_existing_status_folder_by_book_id(
        &self,
        book_id: &str,
        save_dir: Option<&Path>,
    ) -> io::Result<Option<PathBuf>> {
        let save_dir = save_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_save_dir());
        if !save_dir.exists() {
            return Ok(None);
        }

        let canonical_path = Self::canonical_status_folder_path(&save_dir, book_id);
        if canonical_path.is_dir() && Self::status_folder_has_book_record(&canonical_path, book_id)
        {
            return Ok(Some(canonical_path));
        }

        Ok(Self::legacy_status_folders_by_book_id(&save_dir, book_id)?
            .into_iter()
            .next())
    }

    pub fn migrate_status_folder_to_stable(
        &self,
        book_id: &str,
        save_dir: Option<&Path>,
    ) -> io::Result<PathBuf> {
        let save_dir = save_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_save_dir());
        let canonical_path = Self::canonical_status_folder_path(&save_dir, book_id);
        let legacy_folders = if save_dir.exists() {
            Self::legacy_status_folders_by_book_id(&save_dir, book_id)?
        } else {
            Vec::new()
        };

        if canonical_path.exists() {
            if canonical_path.is_dir() {
                for legacy in legacy_folders {
                    if legacy != canonical_path {
                        let _ = merge_dir_contents(&legacy, &canonical_path);
                    }
                }
            }
            return Ok(canonical_path);
        }

        let mut legacy_iter = legacy_folders.into_iter();
        let Some(first_legacy) = legacy_iter.next() else {
            return Ok(canonical_path);
        };

        match fs::rename(&first_legacy, &canonical_path) {
            Ok(_) => {
                for legacy in legacy_iter {
                    let _ = merge_dir_contents(&legacy, &canonical_path);
                }
                Ok(canonical_path)
            }
            Err(_) => Ok(first_legacy),
        }
    }

    fn canonical_status_folder_path(save_dir: &Path, book_id: &str) -> PathBuf {
        let safe_book_id = safe_fs_name(book_id, "_", 120);
        save_dir.join(safe_book_id)
    }

    fn legacy_status_folders_by_book_id(
        save_dir: &Path,
        book_id: &str,
    ) -> io::Result<Vec<PathBuf>> {
        if !save_dir.exists() {
            return Ok(Vec::new());
        }

        let safe_book_id = safe_fs_name(book_id, "_", 120);
        let prefix = format!("{}_", safe_book_id);
        let mut folders = Vec::new();

        for entry in fs::read_dir(save_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let Some(name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };

            if !name.starts_with(&prefix) {
                continue;
            }

            if Self::status_folder_has_book_record(&path, book_id) {
                folders.push(path);
            }
        }

        folders.sort();
        Ok(folders)
    }

    fn status_folder_has_book_record(path: &Path, book_id: &str) -> bool {
        let status = path.join("status.json");
        if status.exists() {
            if let Ok(raw) = fs::read_to_string(&status)
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
                && let Some(id) = value.get("book_id").and_then(|v| v.as_str())
            {
                return id == book_id;
            }
            return true;
        }

        path.join(format!("chapter_status_{}.json", book_id))
            .exists()
            || path.join("downloaded_chapters.jsonl").exists()
    }

    /// 根据用户配置的首选字段选择书名
    pub fn pick_preferred_book_name(
        &self,
        book_meta: &crate::download::downloader::BookMeta,
    ) -> Option<String> {
        match self.preferred_book_name_field.as_str() {
            "original_book_name" => book_meta
                .original_book_name
                .clone()
                .or_else(|| book_meta.book_name.clone()),
            "book_short_name" => book_meta
                .book_short_name
                .clone()
                .or_else(|| book_meta.book_name.clone()),
            // ask_after_download: 下载期间使用默认书名，生成文件前再询问
            "ask_after_download" => book_meta.book_name.clone(),
            _ => book_meta.book_name.clone(), // 默认使用 book_name
        }
    }

    /// 是否设置了“下载完后选择书名”
    pub fn is_ask_after_download(&self) -> bool {
        self.preferred_book_name_field == "ask_after_download"
    }

    pub fn get_status_folder_path(&self) -> Option<PathBuf> {
        self.folder_path.clone()
    }

    pub fn mark_status_folder_claimed(&mut self, path: &Path) {
        for entry in &mut self.status_registry {
            if entry.path == path {
                entry.claimed = true;
            }
        }
        if let Some(last) = &self.folder_path
            && last == path
        {
            self.last_status_claimed = true;
        }
    }

    pub fn mark_status_folder_removed(&mut self, path: &Path) {
        self.status_registry.retain(|e| e.path != path);
        if let Some(last) = &self.folder_path
            && last == path
        {
            self.folder_path = None;
            self.last_status_was_new = false;
            self.last_status_claimed = false;
        }
    }

    pub fn status_folder_was_created_this_session(&self, path: &Path) -> bool {
        for entry in &self.status_registry {
            if entry.path == path {
                return entry.is_new;
            }
        }
        if let Some(last) = &self.folder_path
            && last == path
        {
            return self.last_status_was_new;
        }
        false
    }

    pub fn status_folder_path(
        &mut self,
        _book_name: &str,
        book_id: &str,
        save_dir: Option<&Path>,
    ) -> io::Result<PathBuf> {
        let save_dir = save_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_save_dir());
        let path = self.migrate_status_folder_to_stable(book_id, Some(&save_dir))?;
        let existed_before = path.exists() && Self::status_folder_has_book_record(&path, book_id);
        if !path.exists() {
            fs::create_dir_all(&path)?;
        }

        if let Some(prev) = &self.folder_path
            && self.last_status_was_new
            && !self.last_status_claimed
            && prev != &path
        {
            let _ = fs::remove_dir_all(prev);
        }

        self.folder_path = Some(path.clone());
        self.register_status_folder(&path, existed_before);
        Ok(path)
    }

    fn register_status_folder(&mut self, path: &Path, existed_before: bool) {
        let entry = self.status_registry.iter_mut().find(|e| e.path == path);

        let is_new_this_session =
            (!existed_before) || entry.as_ref().is_some_and(|e| e.is_new && !e.claimed);

        match entry {
            Some(e) => {
                if is_new_this_session {
                    e.is_new = true;
                    e.claimed = false;
                }
            }
            None => self.status_registry.push(StatusEntry {
                path: path.to_path_buf(),
                is_new: is_new_this_session,
                claimed: false,
            }),
        }

        self.last_status_was_new = is_new_this_session;
        self.last_status_claimed = false;
    }
}

fn merge_dir_contents(src: &Path, dst: &Path) -> io::Result<()> {
    merge_dir_contents_at_depth(src, dst, true)
}

fn merge_dir_contents_at_depth(src: &Path, dst: &Path, root_level: bool) -> io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if root_level && src_path.is_file() && is_cover_image_file(&src_path) {
            merge_root_cover_file(&src_path, dst)?;
            continue;
        }

        if !dst_path.exists() {
            if fs::rename(&src_path, &dst_path).is_err() {
                if src_path.is_dir() {
                    fs::create_dir_all(&dst_path)?;
                    merge_dir_contents_at_depth(&src_path, &dst_path, false)?;
                    let _ = fs::remove_dir_all(&src_path);
                } else {
                    fs::copy(&src_path, &dst_path)?;
                    fs::remove_file(&src_path)?;
                }
            }
            continue;
        }

        if src_path.is_dir() && dst_path.is_dir() {
            merge_dir_contents_at_depth(&src_path, &dst_path, false)?;
            let _ = fs::remove_dir(&src_path);
        } else if src_path.is_file() && dst_path.is_file() {
            merge_conflicting_file(&src_path, &dst_path)?;
        } else if src_path.is_dir() {
            let _ = fs::remove_dir_all(&src_path);
        } else {
            let _ = fs::remove_file(&src_path);
        }
    }

    let _ = fs::remove_dir(src);
    Ok(())
}

fn is_cover_image_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(OsStr::to_str) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "heic" | "heif"
    )
}

fn merge_root_cover_file(src: &Path, dst_folder: &Path) -> io::Result<()> {
    let ext = src.extension().and_then(OsStr::to_str).unwrap_or("jpg");
    let canonical = dst_folder.join(format!("cover.{ext}"));
    if canonical.exists() {
        fs::remove_file(src)?;
        return Ok(());
    }

    if fs::rename(src, &canonical).is_err() {
        fs::copy(src, &canonical)?;
        fs::remove_file(src)?;
    }
    Ok(())
}

fn merge_conflicting_file(src: &Path, dst: &Path) -> io::Result<()> {
    let file_name = src.file_name().and_then(OsStr::to_str).unwrap_or_default();

    if file_name == "downloaded_chapters.jsonl" {
        let bytes = fs::read(src)?;
        let mut out = fs::OpenOptions::new().append(true).open(dst)?;
        if !bytes.is_empty() {
            out.write_all(b"\n")?;
            out.write_all(&bytes)?;
        }
        fs::remove_file(src)?;
        return Ok(());
    }

    if file_name == "status.json" && status_downloaded_count(src) > status_downloaded_count(dst) {
        fs::copy(src, dst)?;
    }

    let _ = fs::remove_file(src);
    Ok(())
}

fn status_downloaded_count(path: &Path) -> usize {
    let Ok(raw) = fs::read_to_string(path) else {
        return 0;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return 0;
    };
    value
        .get("downloaded")
        .and_then(|v| v.as_object())
        .map(|m| m.len())
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        Config, OUTPUT_FORMAT_ASK_AFTER_DOWNLOAD, OUTPUT_FORMAT_BULK_TXT, OUTPUT_FORMAT_TXT,
    };

    #[test]
    fn status_folder_path_migrates_legacy_folder_when_only_book_name_changes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = Config {
            save_path: temp_dir.path().display().to_string(),
            ..Config::default()
        };

        let old_folder = temp_dir.path().join("123_旧书名");
        std::fs::create_dir_all(&old_folder).unwrap();
        std::fs::write(old_folder.join("status.json"), "{}\n").unwrap();

        let resolved = config.status_folder_path("新书名", "123", None).unwrap();
        let stable_folder = temp_dir.path().join("123");
        assert_eq!(resolved, stable_folder);
        assert!(stable_folder.join("status.json").exists());
        assert!(!old_folder.exists());
        assert!(!temp_dir.path().join("123_新书名").exists());
    }

    #[test]
    fn status_folder_path_merges_legacy_status_into_preview_cover_folder() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = Config {
            save_path: temp_dir.path().display().to_string(),
            ..Config::default()
        };

        let preview_folder = temp_dir.path().join("123");
        std::fs::create_dir_all(&preview_folder).unwrap();
        std::fs::write(preview_folder.join("cover.jpg"), b"cover").unwrap();

        let old_folder = temp_dir.path().join("123_旧书名");
        std::fs::create_dir_all(&old_folder).unwrap();
        std::fs::write(
            old_folder.join("status.json"),
            r#"{"book_id":"123","downloaded":{"1":["第一章","内容"]}}"#,
        )
        .unwrap();

        let resolved = config.status_folder_path("新书名", "123", None).unwrap();

        assert_eq!(resolved, preview_folder);
        assert!(preview_folder.join("cover.jpg").exists());
        assert!(preview_folder.join("status.json").exists());
        assert!(!old_folder.exists());
    }

    #[test]
    fn bulk_output_mode_is_mapped_to_txt_plus_bulk_flag() {
        let mut config = Config::default();
        config
            .apply_output_format_choice(OUTPUT_FORMAT_BULK_TXT)
            .unwrap();

        assert_eq!(config.novel_format, OUTPUT_FORMAT_TXT);
        assert!(config.bulk_files);
        assert!(!config.ask_format_after_download);
        assert_eq!(
            config.current_output_format_choice(),
            OUTPUT_FORMAT_BULK_TXT
        );
    }

    #[test]
    fn ask_after_download_clears_bulk_flag() {
        let mut config = Config {
            bulk_files: true,
            novel_format: OUTPUT_FORMAT_TXT.to_string(),
            ..Config::default()
        };

        config
            .apply_output_format_choice(OUTPUT_FORMAT_ASK_AFTER_DOWNLOAD)
            .unwrap();

        assert!(config.ask_format_after_download);
        assert!(!config.bulk_files);
        assert_eq!(
            config.current_output_format_choice(),
            OUTPUT_FORMAT_ASK_AFTER_DOWNLOAD
        );
    }

    #[test]
    fn configured_output_format_choice_preserves_real_default_when_asking_later() {
        let mut config = Config::default();
        config
            .apply_output_format_choice(OUTPUT_FORMAT_BULK_TXT)
            .unwrap();
        config
            .apply_output_format_choice(OUTPUT_FORMAT_ASK_AFTER_DOWNLOAD)
            .unwrap();

        assert_eq!(
            config.current_output_format_choice(),
            OUTPUT_FORMAT_ASK_AFTER_DOWNLOAD
        );
        assert_eq!(config.configured_output_format_choice(), OUTPUT_FORMAT_TXT);

        config.bulk_files = true;
        assert_eq!(
            config.configured_output_format_choice(),
            OUTPUT_FORMAT_BULK_TXT
        );
    }

    #[test]
    fn safe_fs_name_replaces_windows_double_quote() {
        let sanitized = super::safe_fs_name("第1章 \"你好\"", "_", 120);

        assert!(!sanitized.contains('"'));
        assert!(sanitized.contains('＂'));
    }
}

pub fn safe_fs_name(name: &str, replacement: &str, max_len: usize) -> String {
    let mut cleaned: String = name
        .chars()
        .map(|ch| match ch {
            // Convert forbidden Windows filename characters to Chinese equivalents
            ':' => '：',        // English colon to Chinese colon
            '"' => '＂',        // English quotes to fullwidth quote
            '<' => '《',        // Less than to Chinese left angle quote
            '>' => '》',        // Greater than to Chinese right angle quote
            '/' | '\\' => '、', // Slashes to Chinese comma
            '|' => '｜',        // Pipe to fullwidth pipe
            '?' => '？',        // Question mark to Chinese question mark
            '*' => '＊',        // Asterisk to fullwidth asterisk
            c if (c as u32) < 32 => replacement.chars().next().unwrap_or('_'),
            _ => ch,
        })
        .collect();

    while cleaned.ends_with(' ') || cleaned.ends_with('.') {
        cleaned.pop();
    }

    if cleaned.is_empty() {
        cleaned.push_str("unnamed");
    }

    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let upper = cleaned.to_uppercase();
    if RESERVED.contains(&upper.as_str()) {
        cleaned = format!("_{}", cleaned);
    }

    if cleaned.len() > max_len {
        // 避免在多字节 UTF-8 字符（如中文）中间截断导致 panic
        let mut end = max_len;
        while !cleaned.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        cleaned.truncate(end);
        while cleaned.ends_with(' ') || cleaned.ends_with('.') {
            cleaned.pop();
        }
        if cleaned.is_empty() {
            cleaned.push_str("unnamed");
        }
    }

    cleaned
}

fn default_false() -> bool {
    false
}

fn default_true() -> bool {
    true
}

fn default_max_workers() -> usize {
    1
}

fn default_request_timeout() -> u64 {
    15
}

fn default_max_retries() -> u32 {
    3
}

fn default_max_wait_time() -> u64 {
    1200
}

fn default_min_wait_time() -> u64 {
    1000
}

fn default_min_connect_timeout() -> f64 {
    3.05
}

fn default_novel_format() -> String {
    "epub".to_string()
}

fn default_audiobook_voice() -> String {
    "zh-CN-XiaoxiaoNeural".to_string()
}

fn default_audiobook_rate() -> String {
    "+0%".to_string()
}

fn default_audiobook_volume() -> String {
    "+0%".to_string()
}

fn default_audiobook_pitch() -> String {
    String::new()
}

fn default_audiobook_format() -> String {
    "mp3".to_string()
}

fn default_audiobook_concurrency() -> usize {
    24
}

fn default_audiobook_tts_provider() -> String {
    "edge".to_string()
}

fn default_string() -> String {
    String::new()
}

fn default_segment_comments_top_n() -> usize {
    10
}

fn default_segment_comments_workers() -> usize {
    32
}

fn default_media_download_workers() -> usize {
    8
}

fn default_blocked_media_domains() -> Vec<String> {
    vec!["p-passport-sign.bytedance.net".to_string()]
}

fn default_jpeg_quality() -> u8 {
    90
}

fn default_first_line_indent_em() -> f32 {
    2.0
}

fn default_media_limit_per_chapter() -> usize {
    0
}

fn default_media_max_dimension_px() -> u32 {
    1280
}

fn default_preferred_book_name_field() -> String {
    "book_name".to_string()
}
