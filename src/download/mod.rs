//! 下载流程模块入口。
//!
//! 子模块：
//! - `models`        — 数据模型（BookMeta / DownloadPlan / ProgressSnapshot 等）
//! - `progress`      — 进度上报与 CLI 进度条
//! - `segment_pool`  — 段评并发下载工作池
//! - `third_party`   — 第三方 API 地址解析 / 请求 / 重试
//! - `plan`          — 下载计划准备与元数据搜索
//! - `downloader`    — 下载主流程编排

pub mod downloader;
pub mod models;
pub mod plan;
pub mod progress;
pub(crate) mod segment_pool;
pub(crate) mod third_party;

// ── 向后兼容重导出 ──────────────────────────────────────────────
// 外部代码通过 `crate::download::downloader::Xxx` 引用的类型
// 现在实际定义在子模块中，但仍可通过 `downloader` 路径访问。
