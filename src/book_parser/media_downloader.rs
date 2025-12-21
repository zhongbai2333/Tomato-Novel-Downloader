use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Avatar,
    CommentImage,
}

#[derive(Debug, Default, Clone)]
pub struct MediaDownloadReport {
    pub unique: usize,
    pub completed: usize,
    pub image_count: usize,
    pub avatar_count: usize,
}

#[derive(Debug, Clone)]
pub struct MediaDownloader {
    root: PathBuf,
    workers: usize,
}

impl MediaDownloader {
    pub fn new(root: PathBuf, workers: usize) -> Self {
        Self {
            root,
            workers: workers.max(1),
        }
    }

    /// 占位的预取逻辑：只做数量统计，未实际下载。
    /// 返回 (unique_cnt, completed, img_cnt, avatar_cnt)
    pub fn prefetch(&self, data: &Value, top_n: usize) -> (usize, usize, usize, usize) {
        let mut unique = 0usize;
        let mut image_count = 0usize;
        let mut avatar_count = 0usize;

        // 按段落统计 medias/avatars 数量
        if let Some(paras) = data.get("paras").and_then(|v| v.as_object()) {
            for para in paras.values().take(top_n) {
                if let Some(imgs) = para.get("images").and_then(|v| v.as_array()) {
                    image_count += imgs.len();
                }
                if para.get("avatar").is_some() {
                    avatar_count += 1;
                }
                unique += 1;
            }
        }

        // 占位：未真正下载，completed 视为 0。
        (unique, 0, image_count, avatar_count)
    }

    pub fn get_cached_media_filename(&self, _url: &str) -> Option<String> {
        // 占位：未建立缓存体系。
        None
    }
}
