//! 解析与导出模块入口。
//!
//! 负责将章节内容转换为 txt/epub，并进行媒体处理、有声书生成等后处理。

#[cfg(feature = "tts-native")]
pub mod edge_tts;

#[cfg(any(feature = "tts", feature = "tts-native"))]
pub mod audio_generator;

#[cfg(not(any(feature = "tts", feature = "tts-native")))]
pub mod audio_generator {
    use std::sync::Arc;

    use indicatif::ProgressBar;
    use serde_json::Value;

    use super::book_manager::BookManager;
    use crate::download::downloader::ProgressReporter;

    pub fn generate_audiobook(
        _manager: &BookManager,
        _chapters: &[Value],
        _bar: Option<&ProgressBar>,
        _quiet: bool,
        _progress: Option<&mut ProgressReporter>,
        _cancel: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> bool {
        true
    }
}
pub mod book_manager;
pub mod epub_generator;
pub(crate) mod finalize_epub;
pub mod finalize_utils;
pub(crate) mod html_utils;
pub(crate) mod image_utils;
pub mod parser;
pub(crate) mod segment_comments;
pub(crate) mod segment_shared;
pub mod segment_utils;
