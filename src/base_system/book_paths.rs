use std::path::PathBuf;

use crate::base_system::context::{Config, safe_fs_name};

pub fn book_folder_name(book_id: &str, book_name: Option<&str>) -> String {
    let safe_book_id = safe_fs_name(book_id, "_", 120);
    let safe_book_name = safe_fs_name(book_name.unwrap_or(book_id), "_", 120);
    format!("{}_{}", safe_book_id, safe_book_name)
}

pub fn book_folder_path(config: &Config, book_id: &str, book_name: Option<&str>) -> PathBuf {
    config
        .default_save_dir()
        .join(book_folder_name(book_id, book_name))
}
