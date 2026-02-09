//! TUI 配置模型与编辑逻辑。
//!
//! 将 `Config` 映射为可展示/可编辑的字段列表，并负责写回 `config.yml`。

use std::path::Path;

use anyhow::{Result, anyhow};

use crate::base_system::config::{ConfigSpec, write_with_comments};
use crate::base_system::context::Config;

use super::App;

#[derive(Debug, Clone, Copy)]
pub(in crate::ui) enum ConfigField {
    SavePath,
    NovelFormat,
    BulkFiles,
    AutoClearDump,
    AutoOpenDownloadedFiles,
    AllowOverwriteFiles,
    PreferredBookNameField,
    OldCli,
    FirstLineIndentEm,
    EnableSegmentComments,
    UseOfficialApi,
    ApiEndpoints,
    MaxWorkers,
    RequestTimeout,
    MaxRetries,
    MinConnectTimeout,
    MinWait,
    MaxWait,
    EnableAudiobook,
    AudiobookVoice,
    AudiobookRate,
    AudiobookVolume,
    AudiobookPitch,
    AudiobookFormat,
    AudiobookConcurrency,
    AudiobookTtsProvider,
    AudiobookTtsApiUrl,
    AudiobookTtsApiToken,
    AudiobookTtsModel,
    SegmentCommentsTopN,
    SegmentCommentsWorkers,
    DownloadCommentImages,
    DownloadCommentAvatars,
    MediaDownloadWorkers,
    BlockedMediaDomains,
    ForceConvertImagesToJpeg,
    JpegRetryConvert,
    JpegQuality,
    ConvertHeicToJpeg,
    KeepHeicOriginal,
    MediaLimitPerChapter,
    MediaMaxDimensionPx,
}

#[derive(Debug, Clone)]
pub(in crate::ui) struct ConfigEntry {
    pub(in crate::ui) title: &'static str,
    pub(in crate::ui) field: ConfigField,
}

#[derive(Debug, Clone)]
pub(in crate::ui) struct ConfigCategory {
    pub(in crate::ui) title: &'static str,
    pub(in crate::ui) entries: Vec<ConfigEntry>,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::ui) struct VoicePreset {
    pub(in crate::ui) name: &'static str,
    pub(in crate::ui) label: &'static str,
}

pub(in crate::ui) const AUDIOBOOK_VOICE_PRESETS: &[VoicePreset] = &[
    VoicePreset {
        name: "zh-CN-XiaoxiaoNeural",
        label: "zh-CN-XiaoxiaoNeural (女)",
    },
    VoicePreset {
        name: "zh-CN-XiaoyiNeural",
        label: "zh-CN-XiaoyiNeural (女)",
    },
    VoicePreset {
        name: "zh-CN-YunjianNeural",
        label: "zh-CN-YunjianNeural (男)",
    },
    VoicePreset {
        name: "zh-CN-YunxiNeural",
        label: "zh-CN-YunxiNeural (男)",
    },
    VoicePreset {
        name: "zh-CN-YunxiaNeural",
        label: "zh-CN-YunxiaNeural (男)",
    },
    VoicePreset {
        name: "zh-CN-YunyangNeural",
        label: "zh-CN-YunyangNeural (男)",
    },
    VoicePreset {
        name: "zh-CN-liaoning-XiaobeiNeural",
        label: "zh-CN-liaoning-XiaobeiNeural (女)",
    },
    VoicePreset {
        name: "zh-CN-shaanxi-XiaoniNeural",
        label: "zh-CN-shaanxi-XiaoniNeural (女)",
    },
    VoicePreset {
        name: "zh-HK-HiuGaaiNeural",
        label: "zh-HK-HiuGaaiNeural (女)",
    },
    VoicePreset {
        name: "zh-HK-HiuMaanNeural",
        label: "zh-HK-HiuMaanNeural (女)",
    },
    VoicePreset {
        name: "zh-HK-WanLungNeural",
        label: "zh-HK-WanLungNeural (男)",
    },
    VoicePreset {
        name: "zh-TW-HsiaoChenNeural",
        label: "zh-TW-HsiaoChenNeural (女)",
    },
];

pub(in crate::ui) const BOOK_NAME_FIELD_PRESETS: &[VoicePreset] = &[
    VoicePreset {
        name: "book_name",
        label: "默认书名",
    },
    VoicePreset {
        name: "original_book_name",
        label: "原始书名",
    },
    VoicePreset {
        name: "book_short_name",
        label: "短书名",
    },
    VoicePreset {
        name: "ask_after_download",
        label: "下载完后选择",
    },
];

pub(in crate::ui) fn cfg_field_is_combo(field: ConfigField) -> bool {
    matches!(
        field,
        ConfigField::AudiobookVoice | ConfigField::PreferredBookNameField
    )
}

pub(in crate::ui) fn cfg_combo_presets(field: ConfigField) -> Option<&'static [VoicePreset]> {
    match field {
        ConfigField::AudiobookVoice => Some(AUDIOBOOK_VOICE_PRESETS),
        ConfigField::PreferredBookNameField => Some(BOOK_NAME_FIELD_PRESETS),
        _ => None,
    }
}

pub(in crate::ui) fn build_config_categories() -> Vec<ConfigCategory> {
    vec![
        ConfigCategory {
            title: "基础与格式",
            entries: vec![
                ConfigEntry {
                    title: "保存路径",
                    field: ConfigField::SavePath,
                },
                ConfigEntry {
                    title: "小说格式(txt/epub)",
                    field: ConfigField::NovelFormat,
                },
                ConfigEntry {
                    title: "首行缩进(em)",
                    field: ConfigField::FirstLineIndentEm,
                },
                ConfigEntry {
                    title: "散装文件保存",
                    field: ConfigField::BulkFiles,
                },
                ConfigEntry {
                    title: "自动清理缓存",
                    field: ConfigField::AutoClearDump,
                },
                ConfigEntry {
                    title: "下载完成后自动打开",
                    field: ConfigField::AutoOpenDownloadedFiles,
                },
                ConfigEntry {
                    title: "允许覆盖已存在文件",
                    field: ConfigField::AllowOverwriteFiles,
                },
                ConfigEntry {
                    title: "优先书名字段",
                    field: ConfigField::PreferredBookNameField,
                },
                ConfigEntry {
                    title: "旧版 CLI UI",
                    field: ConfigField::OldCli,
                },
            ],
        },
        ConfigCategory {
            title: "网络与调度",
            entries: vec![
                ConfigEntry {
                    title: "最大线程数",
                    field: ConfigField::MaxWorkers,
                },
                ConfigEntry {
                    title: "请求超时(s)",
                    field: ConfigField::RequestTimeout,
                },
                ConfigEntry {
                    title: "最大重试次数",
                    field: ConfigField::MaxRetries,
                },
                ConfigEntry {
                    title: "最小连接超时(s)",
                    field: ConfigField::MinConnectTimeout,
                },
                ConfigEntry {
                    title: "最小等待时间(ms)",
                    field: ConfigField::MinWait,
                },
                ConfigEntry {
                    title: "最大等待时间(ms)",
                    field: ConfigField::MaxWait,
                },
            ],
        },
        ConfigCategory {
            title: "API",
            entries: vec![
                ConfigEntry {
                    title: "使用官方API",
                    field: ConfigField::UseOfficialApi,
                },
                ConfigEntry {
                    title: "API 列表(逗号分隔)",
                    field: ConfigField::ApiEndpoints,
                },
            ],
        },
        ConfigCategory {
            title: "段评",
            entries: vec![
                ConfigEntry {
                    title: "启用段评",
                    field: ConfigField::EnableSegmentComments,
                },
                ConfigEntry {
                    title: "每段评论数上限",
                    field: ConfigField::SegmentCommentsTopN,
                },
                ConfigEntry {
                    title: "段评并发线程数",
                    field: ConfigField::SegmentCommentsWorkers,
                },
            ],
        },
        ConfigCategory {
            title: "媒体下载",
            entries: vec![
                ConfigEntry {
                    title: "下载评论图片",
                    field: ConfigField::DownloadCommentImages,
                },
                ConfigEntry {
                    title: "下载评论头像",
                    field: ConfigField::DownloadCommentAvatars,
                },
                ConfigEntry {
                    title: "媒体下载线程数",
                    field: ConfigField::MediaDownloadWorkers,
                },
                ConfigEntry {
                    title: "阻止的图片域名",
                    field: ConfigField::BlockedMediaDomains,
                },
                ConfigEntry {
                    title: "强制转成 JPEG",
                    field: ConfigField::ForceConvertImagesToJpeg,
                },
                ConfigEntry {
                    title: "失败重试再转 JPEG",
                    field: ConfigField::JpegRetryConvert,
                },
                ConfigEntry {
                    title: "JPEG 质量(0-100)",
                    field: ConfigField::JpegQuality,
                },
                ConfigEntry {
                    title: "HEIC 转 JPEG",
                    field: ConfigField::ConvertHeicToJpeg,
                },
                ConfigEntry {
                    title: "保留 HEIC 原图",
                    field: ConfigField::KeepHeicOriginal,
                },
                ConfigEntry {
                    title: "单章节媒体上限",
                    field: ConfigField::MediaLimitPerChapter,
                },
                ConfigEntry {
                    title: "媒体最大尺寸(px)",
                    field: ConfigField::MediaMaxDimensionPx,
                },
            ],
        },
        ConfigCategory {
            title: "有声书",
            entries: vec![
                ConfigEntry {
                    title: "启用有声书",
                    field: ConfigField::EnableAudiobook,
                },
                ConfigEntry {
                    title: "发音人",
                    field: ConfigField::AudiobookVoice,
                },
                ConfigEntry {
                    title: "TTS 服务类型(edge/third_party)",
                    field: ConfigField::AudiobookTtsProvider,
                },
                ConfigEntry {
                    title: "第三方 TTS API 地址",
                    field: ConfigField::AudiobookTtsApiUrl,
                },
                ConfigEntry {
                    title: "第三方 TTS Token",
                    field: ConfigField::AudiobookTtsApiToken,
                },
                ConfigEntry {
                    title: "第三方 TTS 模型",
                    field: ConfigField::AudiobookTtsModel,
                },
                ConfigEntry {
                    title: "语速调整",
                    field: ConfigField::AudiobookRate,
                },
                ConfigEntry {
                    title: "音量调整",
                    field: ConfigField::AudiobookVolume,
                },
                ConfigEntry {
                    title: "音调调整",
                    field: ConfigField::AudiobookPitch,
                },
                ConfigEntry {
                    title: "输出格式(mp3/wav)",
                    field: ConfigField::AudiobookFormat,
                },
                ConfigEntry {
                    title: "并发生成章节数",
                    field: ConfigField::AudiobookConcurrency,
                },
            ],
        },
    ]
}

pub(in crate::ui) fn current_cfg_value(app: &App, field: ConfigField) -> String {
    match field {
        ConfigField::SavePath => app.config.save_path.clone(),
        ConfigField::NovelFormat => app.config.novel_format.clone(),
        ConfigField::FirstLineIndentEm => format!("{:.2}", app.config.first_line_indent_em),
        ConfigField::BulkFiles => app.config.bulk_files.to_string(),
        ConfigField::AutoClearDump => app.config.auto_clear_dump.to_string(),
        ConfigField::AutoOpenDownloadedFiles => app.config.auto_open_downloaded_files.to_string(),
        ConfigField::AllowOverwriteFiles => app.config.allow_overwrite_files.to_string(),
        ConfigField::PreferredBookNameField => {
            book_name_field_to_chinese(&app.config.preferred_book_name_field).to_string()
        }
        ConfigField::OldCli => app.config.old_cli.to_string(),
        ConfigField::EnableSegmentComments => app.config.enable_segment_comments.to_string(),
        ConfigField::UseOfficialApi => app.config.use_official_api.to_string(),
        ConfigField::ApiEndpoints => app.config.api_endpoints.join(","),
        ConfigField::MaxWorkers => app.config.max_workers.to_string(),
        ConfigField::RequestTimeout => app.config.request_timeout.to_string(),
        ConfigField::MaxRetries => app.config.max_retries.to_string(),
        ConfigField::MinConnectTimeout => format!("{:.2}", app.config.min_connect_timeout),
        ConfigField::MinWait => app.config.min_wait_time.to_string(),
        ConfigField::MaxWait => app.config.max_wait_time.to_string(),
        ConfigField::EnableAudiobook => app.config.enable_audiobook.to_string(),
        ConfigField::AudiobookVoice => app.config.audiobook_voice.clone(),
        ConfigField::AudiobookRate => app.config.audiobook_rate.clone(),
        ConfigField::AudiobookVolume => app.config.audiobook_volume.clone(),
        ConfigField::AudiobookPitch => app.config.audiobook_pitch.clone(),
        ConfigField::AudiobookFormat => app.config.audiobook_format.clone(),
        ConfigField::AudiobookConcurrency => app.config.audiobook_concurrency.to_string(),
        ConfigField::AudiobookTtsProvider => app.config.audiobook_tts_provider.clone(),
        ConfigField::AudiobookTtsApiUrl => app.config.audiobook_tts_api_url.clone(),
        ConfigField::AudiobookTtsApiToken => app.config.audiobook_tts_api_token.clone(),
        ConfigField::AudiobookTtsModel => app.config.audiobook_tts_model.clone(),
        ConfigField::SegmentCommentsTopN => app.config.segment_comments_top_n.to_string(),
        ConfigField::SegmentCommentsWorkers => app.config.segment_comments_workers.to_string(),
        ConfigField::DownloadCommentImages => app.config.download_comment_images.to_string(),
        ConfigField::DownloadCommentAvatars => app.config.download_comment_avatars.to_string(),
        ConfigField::MediaDownloadWorkers => app.config.media_download_workers.to_string(),
        ConfigField::BlockedMediaDomains => app.config.blocked_media_domains.join(","),
        ConfigField::ForceConvertImagesToJpeg => {
            app.config.force_convert_images_to_jpeg.to_string()
        }
        ConfigField::JpegRetryConvert => app.config.jpeg_retry_convert.to_string(),
        ConfigField::JpegQuality => app.config.jpeg_quality.to_string(),
        ConfigField::ConvertHeicToJpeg => app.config.convert_heic_to_jpeg.to_string(),
        ConfigField::KeepHeicOriginal => app.config.keep_heic_original.to_string(),
        ConfigField::MediaLimitPerChapter => app.config.media_limit_per_chapter.to_string(),
        ConfigField::MediaMaxDimensionPx => app.config.media_max_dimension_px.to_string(),
    }
}

pub(in crate::ui) fn cfg_field_is_bool(field: ConfigField) -> bool {
    matches!(
        field,
        ConfigField::BulkFiles
            | ConfigField::AutoClearDump
            | ConfigField::AutoOpenDownloadedFiles
            | ConfigField::AllowOverwriteFiles
            | ConfigField::OldCli
            | ConfigField::EnableSegmentComments
            | ConfigField::UseOfficialApi
            | ConfigField::EnableAudiobook
            | ConfigField::DownloadCommentImages
            | ConfigField::DownloadCommentAvatars
            | ConfigField::ForceConvertImagesToJpeg
            | ConfigField::JpegRetryConvert
            | ConfigField::ConvertHeicToJpeg
            | ConfigField::KeepHeicOriginal
    )
}

fn cfg_field_current_bool(app: &App, field: ConfigField) -> Option<bool> {
    let val = match field {
        ConfigField::BulkFiles => app.config.bulk_files,
        ConfigField::AutoClearDump => app.config.auto_clear_dump,
        ConfigField::AutoOpenDownloadedFiles => app.config.auto_open_downloaded_files,
        ConfigField::AllowOverwriteFiles => app.config.allow_overwrite_files,
        ConfigField::OldCli => app.config.old_cli,
        ConfigField::EnableSegmentComments => app.config.enable_segment_comments,
        ConfigField::UseOfficialApi => app.config.use_official_api,
        ConfigField::EnableAudiobook => app.config.enable_audiobook,
        ConfigField::DownloadCommentImages => app.config.download_comment_images,
        ConfigField::DownloadCommentAvatars => app.config.download_comment_avatars,
        ConfigField::ForceConvertImagesToJpeg => app.config.force_convert_images_to_jpeg,
        ConfigField::JpegRetryConvert => app.config.jpeg_retry_convert,
        ConfigField::ConvertHeicToJpeg => app.config.convert_heic_to_jpeg,
        ConfigField::KeepHeicOriginal => app.config.keep_heic_original,
        _ => return None,
    };
    Some(val)
}

pub(in crate::ui) fn start_cfg_edit(app: &mut App) {
    let Some(cat_idx) = app.cfg_cat_state.selected() else {
        return;
    };
    let Some(entry_idx) = app.cfg_entry_state.selected() else {
        return;
    };
    let Some(category) = app.cfg_categories.get(cat_idx) else {
        return;
    };
    if entry_idx >= category.entries.len() {
        return;
    }
    let entry = &category.entries[entry_idx];
    app.cfg_editing = Some((cat_idx, entry_idx));
    app.cfg_edit_buffer = current_cfg_value(app, entry.field);
    if cfg_field_is_bool(entry.field) {
        let selected = match cfg_field_current_bool(app, entry.field) {
            Some(true) => Some(0),
            Some(false) => Some(1),
            None => Some(0),
        };
        app.cfg_bool_state.select(selected);
    }
    if cfg_field_is_combo(entry.field) {
        app.cfg_combo_focus = super::ConfigComboFocus::List;
        if let Some(presets) = cfg_combo_presets(entry.field) {
            let idx = presets
                .iter()
                .position(|p| p.name.eq_ignore_ascii_case(&app.cfg_edit_buffer))
                .or(Some(0));
            app.cfg_combo_state.select(idx);
        }
    }
    app.status = format!("正在编辑 [{}]: {}", category.title, entry.title);
}

pub(in crate::ui) fn apply_cfg_edit(app: &mut App, cat_idx: usize, entry_idx: usize) -> Result<()> {
    let Some(category) = app.cfg_categories.get(cat_idx) else {
        return Ok(());
    };
    if entry_idx >= category.entries.len() {
        return Ok(());
    }
    let field = category.entries[entry_idx].field;
    let entry_title = category.entries[entry_idx].title;
    let raw = app.cfg_edit_buffer.trim();

    let mut note: Option<String> = None;

    match field {
        ConfigField::SavePath => {
            app.config.save_path = raw.to_string();
        }
        ConfigField::NovelFormat => {
            let lower = raw.to_ascii_lowercase();
            if lower != "txt" && lower != "epub" {
                app.status = "仅支持 txt 或 epub".to_string();
                return Ok(());
            }
            app.config.novel_format = lower;
            if app.config.novel_format == "txt" && app.config.enable_segment_comments {
                app.config.enable_segment_comments = false;
                note = Some("已关闭段评以兼容 txt".to_string());
            }
        }
        ConfigField::FirstLineIndentEm => {
            let val: f32 = raw.parse().map_err(|_| anyhow!("请输入数字"))?;
            if val.is_sign_negative() {
                app.status = "缩进不能为负".to_string();
                return Ok(());
            }
            app.config.first_line_indent_em = val;
        }
        ConfigField::BulkFiles => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.bulk_files = val;
        }
        ConfigField::AutoClearDump => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.auto_clear_dump = val;
        }
        ConfigField::AutoOpenDownloadedFiles => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.auto_open_downloaded_files = val;
        }
        ConfigField::AllowOverwriteFiles => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.allow_overwrite_files = val;
        }
        ConfigField::PreferredBookNameField => {
            // 尝试从中文转换，如果失败则尝试直接使用英文
            let field_name = if let Some(english) = chinese_to_book_name_field(raw) {
                english
            } else {
                // 如果不是中文，检查是否是有效的英文字段名
                let lower = raw.to_ascii_lowercase();
                if lower == "book_name"
                    || lower == "original_book_name"
                    || lower == "book_short_name"
                    || lower == "ask_after_download"
                {
                    lower
                } else {
                    app.status = "请选择：默认书名、原始书名、短书名 或 下载完后选择".to_string();
                    return Ok(());
                }
            };
            app.config.preferred_book_name_field = field_name;
        }
        ConfigField::OldCli => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.old_cli = val;
        }
        ConfigField::EnableSegmentComments => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            if val && !app.config.novel_format.eq_ignore_ascii_case("epub") {
                app.status = "段评仅支持 epub，请先将格式改为 epub".to_string();
                return Ok(());
            }
            app.config.enable_segment_comments = val;
        }
        ConfigField::UseOfficialApi => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.use_official_api = val;
        }
        ConfigField::ApiEndpoints => {
            let list = parse_string_list(raw);
            app.config.api_endpoints = list;
        }
        ConfigField::MaxWorkers => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入正整数"))?;
            if val == 0 {
                app.status = "最大线程数需大于 0".to_string();
                return Ok(());
            }
            app.config.max_workers = val;
        }
        ConfigField::RequestTimeout => {
            let val: u64 = raw.parse().map_err(|_| anyhow!("请输入秒数"))?;
            if val == 0 {
                app.status = "超时时间需大于 0".to_string();
                return Ok(());
            }
            app.config.request_timeout = val;
        }
        ConfigField::MaxRetries => {
            let val: u32 = raw.parse().map_err(|_| anyhow!("请输入整数"))?;
            app.config.max_retries = val;
        }
        ConfigField::MinConnectTimeout => {
            let val: f64 = raw.parse().map_err(|_| anyhow!("请输入数字"))?;
            if val <= 0.0 {
                app.status = "连接超时需大于 0".to_string();
                return Ok(());
            }
            app.config.min_connect_timeout = val;
        }
        ConfigField::MinWait => {
            let val: u64 = raw.parse().map_err(|_| anyhow!("请输入整数毫秒"))?;
            if val > app.config.max_wait_time {
                app.status = "最小等待时间不能超过最大等待时间".to_string();
                return Ok(());
            }
            app.config.min_wait_time = val;
        }
        ConfigField::MaxWait => {
            let val: u64 = raw.parse().map_err(|_| anyhow!("请输入整数毫秒"))?;
            if val < app.config.min_wait_time {
                app.status = "最大等待时间需要不小于最小等待时间".to_string();
                return Ok(());
            }
            app.config.max_wait_time = val;
        }
        ConfigField::EnableAudiobook => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.enable_audiobook = val;
        }
        ConfigField::AudiobookVoice => {
            app.config.audiobook_voice = raw.to_string();
        }
        ConfigField::AudiobookRate => {
            app.config.audiobook_rate = raw.to_string();
        }
        ConfigField::AudiobookVolume => {
            app.config.audiobook_volume = raw.to_string();
        }
        ConfigField::AudiobookPitch => {
            app.config.audiobook_pitch = raw.to_string();
        }
        ConfigField::AudiobookFormat => {
            let lower = raw.to_ascii_lowercase();
            if lower != "mp3" && lower != "wav" {
                app.status = "格式仅支持 mp3 或 wav".to_string();
                return Ok(());
            }
            app.config.audiobook_format = lower;
        }
        ConfigField::AudiobookConcurrency => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入正整数"))?;
            if val == 0 {
                app.status = "并发章节数需大于 0".to_string();
                return Ok(());
            }
            app.config.audiobook_concurrency = val;
        }
        ConfigField::AudiobookTtsProvider => {
            app.config.audiobook_tts_provider = raw.to_string();
        }
        ConfigField::AudiobookTtsApiUrl => {
            app.config.audiobook_tts_api_url = raw.to_string();
        }
        ConfigField::AudiobookTtsApiToken => {
            app.config.audiobook_tts_api_token = raw.to_string();
        }
        ConfigField::AudiobookTtsModel => {
            app.config.audiobook_tts_model = raw.to_string();
        }
        ConfigField::SegmentCommentsTopN => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入整数"))?;
            if val == 0 {
                app.status = "评论数上限需大于 0".to_string();
                return Ok(());
            }
            app.config.segment_comments_top_n = val;
        }
        ConfigField::SegmentCommentsWorkers => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入正整数"))?;
            if val == 0 {
                app.status = "段评线程数需大于 0".to_string();
                return Ok(());
            }
            app.config.segment_comments_workers = val;
        }
        ConfigField::DownloadCommentImages => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.download_comment_images = val;
        }
        ConfigField::DownloadCommentAvatars => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.download_comment_avatars = val;
        }
        ConfigField::MediaDownloadWorkers => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入正整数"))?;
            if val == 0 {
                app.status = "媒体线程数需大于 0".to_string();
                return Ok(());
            }
            app.config.media_download_workers = val;
        }
        ConfigField::BlockedMediaDomains => {
            app.config.blocked_media_domains = parse_string_list(raw);
        }
        ConfigField::ForceConvertImagesToJpeg => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.force_convert_images_to_jpeg = val;
        }
        ConfigField::JpegRetryConvert => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.jpeg_retry_convert = val;
        }
        ConfigField::JpegQuality => {
            let val: u8 = raw
                .parse()
                .map_err(|_| anyhow!("请输入 0-100 之间的整数"))?;
            if val > 100 {
                app.status = "JPEG 质量需在 0-100 之间".to_string();
                return Ok(());
            }
            app.config.jpeg_quality = val;
        }
        ConfigField::ConvertHeicToJpeg => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.convert_heic_to_jpeg = val;
        }
        ConfigField::KeepHeicOriginal => {
            let val = parse_bool(raw).ok_or_else(|| anyhow!("请输入 true/false"))?;
            app.config.keep_heic_original = val;
        }
        ConfigField::MediaLimitPerChapter => {
            let val: usize = raw.parse().map_err(|_| anyhow!("请输入整数"))?;
            app.config.media_limit_per_chapter = val;
        }
        ConfigField::MediaMaxDimensionPx => {
            let val: u32 = raw.parse().map_err(|_| anyhow!("请输入整数"))?;
            app.config.media_max_dimension_px = val;
        }
    }

    let path = Path::new(Config::FILE_NAME);
    write_with_comments(&app.config, path).map_err(|e| anyhow!(e.to_string()))?;
    match note {
        Some(extra) => app.status = format!("已保存: {}（{}）", entry_title, extra),
        None => app.status = format!("已保存: {}", entry_title),
    }
    Ok(())
}

fn parse_bool(input: &str) -> Option<bool> {
    match input.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" | "on" => Some(true),
        "false" | "0" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

fn parse_string_list(input: &str) -> Vec<String> {
    input
        .split([',', ';', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 将书名字段的英文名转换为中文显示名
fn book_name_field_to_chinese(field: &str) -> &'static str {
    match field {
        "book_name" => "默认书名",
        "original_book_name" => "原始书名",
        "book_short_name" => "短书名",
        "ask_after_download" => "下载完后选择",
        _ => "默认书名",
    }
}

/// 将中文显示名转换为书名字段的英文名
fn chinese_to_book_name_field(chinese: &str) -> Option<String> {
    match chinese {
        "默认书名" => Some("book_name".to_string()),
        "原始书名" => Some("original_book_name".to_string()),
        "短书名" => Some("book_short_name".to_string()),
        "下载完后选择" => Some("ask_after_download".to_string()),
        _ => None,
    }
}
