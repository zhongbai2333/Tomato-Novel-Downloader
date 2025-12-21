use serde_json::Value;
use tracing::info;

use super::book_manager::BookManager;

/// 占位的有声书生成器，返回是否生成成功。
pub fn generate_audiobook(_manager: &BookManager, chapters: &[Value]) -> bool {
    info!(target: "book_manager", "skip audiobook generation: chapters={}", chapters.len());
    false
}
