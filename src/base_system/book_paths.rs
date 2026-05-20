//! 本地路径规划与命名。

use std::path::{Path, PathBuf};

use crate::base_system::context::{Config, safe_fs_name};

pub const COVER_FILE_STEM: &str = "cover";
pub const COVER_IMAGE_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// 缓存目录名只使用稳定的 `book_id`。
///
/// 书名可能来自搜索结果、详情页、用户下载后选择，也可能被平台改名；如果把书名放进缓存路径，
/// 同一本书会因为名字变化产生多个缓存目录。最终导出的小说文件仍然使用书名命名，缓存层只认 ID。
pub fn book_folder_name(book_id: &str, _book_name: Option<&str>) -> String {
    safe_fs_name(book_id, "_", 120)
}

pub fn book_folder_path(config: &Config, book_id: &str, book_name: Option<&str>) -> PathBuf {
    config
        .default_save_dir()
        .join(book_folder_name(book_id, book_name))
}

#[allow(dead_code)]
pub fn legacy_book_folder_name(book_id: &str, book_name: Option<&str>) -> String {
    let safe_book_id = safe_fs_name(book_id, "_", 120);
    let safe_book_name = safe_fs_name(book_name.unwrap_or(book_id), "_", 120);
    format!("{}_{}", safe_book_id, safe_book_name)
}

pub fn canonical_cover_path(folder: &Path, ext: &str) -> PathBuf {
    folder.join(format!("{COVER_FILE_STEM}.{ext}"))
}

pub fn cover_file_candidates(folder: &Path, book_name: Option<&str>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for ext in &COVER_IMAGE_EXTENSIONS {
        candidates.push(canonical_cover_path(folder, ext));
    }

    if let Some(name) = book_name.filter(|s| !s.trim().is_empty()) {
        let safe_name = safe_fs_name(name, "_", 120);
        for ext in &COVER_IMAGE_EXTENSIONS {
            candidates.push(folder.join(format!("{safe_name}.{ext}")));
        }
        for ext in &COVER_IMAGE_EXTENSIONS {
            candidates.push(folder.join(format!("{name}.{ext}")));
        }
    }

    candidates
}

pub fn find_existing_cover_file(folder: &Path, book_name: Option<&str>) -> Option<PathBuf> {
    cover_file_candidates(folder, book_name)
        .into_iter()
        .find(|p| p.exists())
}

/// 将旧版“书名作为封面文件名”的封面迁移为稳定的 `cover.*`。
pub fn migrate_legacy_cover_file(folder: &Path, book_name: Option<&str>) -> Option<PathBuf> {
    if !folder.exists() {
        return None;
    }

    for ext in &COVER_IMAGE_EXTENSIONS {
        let canonical = canonical_cover_path(folder, ext);
        if canonical.exists() {
            return Some(canonical);
        }
    }

    let legacy = find_existing_cover_file(folder, book_name)?;
    let ext = legacy.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
    let canonical = canonical_cover_path(folder, ext);
    match std::fs::rename(&legacy, &canonical) {
        Ok(_) => Some(canonical),
        Err(_) => Some(legacy),
    }
}

#[cfg(test)]
mod tests {
    use super::{book_folder_name, legacy_book_folder_name};

    #[test]
    fn cache_folder_name_is_stable_across_book_name_changes() {
        assert_eq!(book_folder_name("123", Some("旧书名")), "123");
        assert_eq!(book_folder_name("123", Some("新书名")), "123");
        assert_ne!(
            legacy_book_folder_name("123", Some("旧书名")),
            legacy_book_folder_name("123", Some("新书名"))
        );
    }
}
