//! 基础设施模块。
//!
//! 统一放置配置、日志、重试、路径与通用工具，供下载/解析/UI 层复用。

#![allow(dead_code)]

pub mod book_id;
pub mod book_paths;
pub mod config;
pub mod context;
pub mod cooldown_retry;
pub mod file_cleaner;
pub mod json_extract;
pub mod logging;
pub mod self_update;
