//! 下载相关的数据模型定义。
//!
//! 包含下载结果、下载模式、书籍元数据、下载计划、进度快照等核心数据结构。

use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;

#[cfg(feature = "official-api")]
pub use tomato_novel_official_api::ChapterRef;

#[cfg(feature = "official-api")]
use tomato_novel_official_api::DirectoryMeta;

#[cfg(not(feature = "official-api"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChapterRef {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DownloadResult {
    pub success: u32,
    pub failed: u32,
    pub canceled: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadMode {
    Resume,
    Full,
    FailedOnly,
    RangeIgnoreHistory,
}

pub enum RetryFailed {
    Never,
    Decide(Box<dyn FnMut(usize) -> bool + Send>),
}

pub struct DownloadFlowOptions {
    pub mode: DownloadMode,
    pub range: Option<ChapterRange>,
    pub retry_failed: RetryFailed,
    pub stage_callback: Option<Box<dyn FnMut(DownloadResult) + Send>>,
    pub book_name_asker: Option<BookNameAsker>,
}

pub type BookNameAsker =
    Box<dyn FnMut(&crate::book_parser::book_manager::BookManager) -> Option<String> + Send>;

#[derive(Debug, Clone, Default)]
pub struct BookMeta {
    pub book_name: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub cover_url: Option<String>,
    pub detail_cover_url: Option<String>,
    pub finished: Option<bool>,
    pub chapter_count: Option<usize>,
    pub word_count: Option<usize>,
    pub score: Option<f32>,
    pub read_count: Option<String>,
    pub read_count_text: Option<String>,
    pub book_short_name: Option<String>,
    pub original_book_name: Option<String>,
    pub first_chapter_title: Option<String>,
    pub last_chapter_title: Option<String>,
    pub category: Option<String>,
    pub cover_primary_color: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BookNameOption {
    pub label: String,
    pub value: String,
}

#[cfg(feature = "official-api")]
impl From<DirectoryMeta> for BookMeta {
    fn from(value: DirectoryMeta) -> Self {
        Self {
            book_name: value.book_name,
            author: value.author,
            description: value.description,
            tags: value.tags,
            cover_url: value
                .cover_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .or(value.cover_url),
            detail_cover_url: value.detail_cover_url,
            finished: value.finished,
            chapter_count: value.chapter_count,
            word_count: value.word_count,
            score: value.score,
            read_count: value.read_count,
            read_count_text: value.read_count_text,
            book_short_name: value.book_short_name,
            original_book_name: value.original_book_name,
            first_chapter_title: value.first_chapter_title,
            last_chapter_title: value.last_chapter_title,
            category: value.category,
            cover_primary_color: value.cover_primary_color,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadPlan {
    pub book_id: String,
    pub meta: BookMeta,
    pub chapters: Vec<ChapterRef>,
    pub _raw: Value,
}

#[derive(Debug, Clone, Copy)]
pub struct ChapterRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct ProgressSnapshot {
    pub group_done: usize,
    pub group_total: usize,
    pub saved_chapters: usize,
    pub chapter_total: usize,
    pub save_phase: SavePhase,
    pub comment_fetch: usize,
    pub comment_total: usize,
    pub comment_saved: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SavePhase {
    #[default]
    TextSave,
    Audiobook,
}

// ── 元数据合并工具函数 ──────────────────────────────────────────────────

pub(crate) fn merge_meta(primary: BookMeta, fallback: BookMeta) -> BookMeta {
    BookMeta {
        book_name: primary.book_name.or(fallback.book_name),
        author: primary.author.or(fallback.author),
        description: primary.description.or(fallback.description),
        tags: if primary.tags.is_empty() {
            fallback.tags
        } else {
            primary.tags
        },
        cover_url: primary.cover_url.or(fallback.cover_url),
        detail_cover_url: primary.detail_cover_url.or(fallback.detail_cover_url),
        finished: primary.finished.or(fallback.finished),
        chapter_count: primary.chapter_count.or(fallback.chapter_count),
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

pub(crate) fn merge_tag_lists(primary: &[String], fallback: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tag in primary.iter().chain(fallback.iter()) {
        let t = tag.trim();
        if t.is_empty() {
            continue;
        }
        if seen.insert(t.to_string()) {
            out.push(t.to_string());
        }
    }
    out
}

pub(crate) fn drop_tag_equals_category(tags: &[String], category: &Option<String>) -> Vec<String> {
    let Some(cat) = category
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    else {
        return tags.to_vec();
    };
    tags.iter().filter(|t| t.trim() != cat).cloned().collect()
}

/// Merge metadata with special handling for book_name: prefer hint (what user saw) over dir API
pub(crate) fn merge_meta_prefer_hint_name(dir_meta: BookMeta, hint_meta: BookMeta) -> BookMeta {
    BookMeta {
        // Special: prefer hint's book_name to maintain UI consistency
        book_name: hint_meta.book_name.or(dir_meta.book_name),
        // For all other fields, prefer directory (authoritative) over hint
        author: dir_meta.author.or(hint_meta.author),
        description: dir_meta.description.or(hint_meta.description),
        tags: if dir_meta.tags.is_empty() {
            hint_meta.tags
        } else {
            dir_meta.tags
        },
        cover_url: dir_meta.cover_url.or(hint_meta.cover_url),
        detail_cover_url: dir_meta.detail_cover_url.or(hint_meta.detail_cover_url),
        finished: dir_meta.finished.or(hint_meta.finished),
        chapter_count: dir_meta.chapter_count.or(hint_meta.chapter_count),
        word_count: dir_meta.word_count.or(hint_meta.word_count),
        score: dir_meta.score.or(hint_meta.score),
        read_count: dir_meta.read_count.or(hint_meta.read_count),
        read_count_text: dir_meta.read_count_text.or(hint_meta.read_count_text),
        book_short_name: dir_meta.book_short_name.or(hint_meta.book_short_name),
        original_book_name: dir_meta.original_book_name.or(hint_meta.original_book_name),
        first_chapter_title: dir_meta
            .first_chapter_title
            .or(hint_meta.first_chapter_title),
        last_chapter_title: dir_meta.last_chapter_title.or(hint_meta.last_chapter_title),
        category: dir_meta.category.or(hint_meta.category),
        cover_primary_color: dir_meta
            .cover_primary_color
            .or(hint_meta.cover_primary_color),
    }
}
