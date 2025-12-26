//! 解析与导出模块入口。
//!
//! 负责将章节内容转换为 txt/epub，并进行媒体处理、有声书生成等后处理。

pub mod audio_generator;
pub mod book_manager;
pub mod epub_generator;
pub mod finalize_utils;
pub mod parser;
pub mod segment_utils;
