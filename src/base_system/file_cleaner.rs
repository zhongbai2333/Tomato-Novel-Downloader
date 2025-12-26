//! 缓存与临时文件清理。

use std::fs;
use std::io;
use std::path::Path;

pub fn clean_dump_folder(path: impl AsRef<Path>, ignore_errors: bool) -> io::Result<()> {
    let path = path.as_ref();
    let result = fs::remove_dir_all(path);
    if ignore_errors { Ok(()) } else { result }
}

pub fn is_empty_dir(path: impl AsRef<Path>) -> io::Result<bool> {
    let path = path.as_ref();
    let mut entries = fs::read_dir(path)?;
    Ok(entries.next().is_none())
}
