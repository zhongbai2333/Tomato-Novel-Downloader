//! 无 UI 配置编辑器。
//!
//! 提供交互式菜单修改 `config.yml`。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};

use crate::base_system::config::{ConfigSpec, write_with_comments};
use crate::base_system::context::Config;

#[derive(Debug, Clone, Copy)]
enum ConfigValueType {
    Bool,
    Int,
    Float,
    String,
    List,
    Selection,
}

#[derive(Debug, Clone, Copy)]
enum ConfigField {
    SavePath,
    NovelFormat,
    BulkFiles,
    AutoClearDump,
    AllowOverwriteFiles,
    PreferredBookNameField,
    EnableAudiobook,
    AudiobookVoice,
    AudiobookRate,
    AudiobookVolume,
    AudiobookPitch,
    AudiobookConcurrency,
    AudiobookFormat,
    MaxWorkers,
    RequestTimeout,
    MaxRetries,
    MinWaitTime,
    MaxWaitTime,
    MinConnectTimeout,
    UseOfficialApi,
    ApiEndpoints,
    EnableSegmentComments,
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
    FirstLineIndentEm,
    OldCli,
}

#[derive(Debug, Clone, Copy)]
struct ConfigOption {
    name: &'static str,
    field: ConfigField,
    ty: ConfigValueType,
}

pub(super) fn show_config_menu(config: &mut Config) -> Result<()> {
    // 参照 old_main.py 的 option_defs 顺序
    const OPTS: &[ConfigOption] = &[
        ConfigOption {
            name: "保存路径",
            field: ConfigField::SavePath,
            ty: ConfigValueType::String,
        },
        ConfigOption {
            name: "小说保存格式(txt/epub)",
            field: ConfigField::NovelFormat,
            ty: ConfigValueType::String,
        },
        ConfigOption {
            name: "是否以散装形式保存小说",
            field: ConfigField::BulkFiles,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "是否自动清理缓存文件",
            field: ConfigField::AutoClearDump,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "是否允许覆盖已存在的文件",
            field: ConfigField::AllowOverwriteFiles,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "优先书名字段",
            field: ConfigField::PreferredBookNameField,
            ty: ConfigValueType::Selection,
        },
        ConfigOption {
            name: "是否生成有声小说",
            field: ConfigField::EnableAudiobook,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "有声小说发音人",
            field: ConfigField::AudiobookVoice,
            ty: ConfigValueType::String,
        },
        ConfigOption {
            name: "有声小说语速(如+0%)",
            field: ConfigField::AudiobookRate,
            ty: ConfigValueType::String,
        },
        ConfigOption {
            name: "有声小说音量(如+0%)",
            field: ConfigField::AudiobookVolume,
            ty: ConfigValueType::String,
        },
        ConfigOption {
            name: "有声小说音调(如+2Hz/-1st, 可留空)",
            field: ConfigField::AudiobookPitch,
            ty: ConfigValueType::String,
        },
        ConfigOption {
            name: "有声小说并发数",
            field: ConfigField::AudiobookConcurrency,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "有声小说格式(mp3/wav)",
            field: ConfigField::AudiobookFormat,
            ty: ConfigValueType::String,
        },
        ConfigOption {
            name: "最大线程数",
            field: ConfigField::MaxWorkers,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "请求超时(秒)",
            field: ConfigField::RequestTimeout,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "最大重试次数",
            field: ConfigField::MaxRetries,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "最小等待时间(ms)",
            field: ConfigField::MinWaitTime,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "最大等待时间(ms)",
            field: ConfigField::MaxWaitTime,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "最小连接超时时间",
            field: ConfigField::MinConnectTimeout,
            ty: ConfigValueType::Float,
        },
        ConfigOption {
            name: "是否使用官方API",
            field: ConfigField::UseOfficialApi,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "自定义API列表(逗号分隔)",
            field: ConfigField::ApiEndpoints,
            ty: ConfigValueType::List,
        },
        ConfigOption {
            name: "是否下载段评",
            field: ConfigField::EnableSegmentComments,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "段评每段最多条数",
            field: ConfigField::SegmentCommentsTopN,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "段评并发线程数",
            field: ConfigField::SegmentCommentsWorkers,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "是否下载评论区图片",
            field: ConfigField::DownloadCommentImages,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "是否下载评论区头像",
            field: ConfigField::DownloadCommentAvatars,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "评论图片下载线程数",
            field: ConfigField::MediaDownloadWorkers,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "图片域名黑名单(逗号分隔)",
            field: ConfigField::BlockedMediaDomains,
            ty: ConfigValueType::List,
        },
        ConfigOption {
            name: "强制所有图片转JPEG",
            field: ConfigField::ForceConvertImagesToJpeg,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "非JPEG尝试转JPEG",
            field: ConfigField::JpegRetryConvert,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "JPEG质量(0-100)",
            field: ConfigField::JpegQuality,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "HEIC转JPEG",
            field: ConfigField::ConvertHeicToJpeg,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "保留原始HEIC文件",
            field: ConfigField::KeepHeicOriginal,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "每章媒体数量上限(0为不限制)",
            field: ConfigField::MediaLimitPerChapter,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "图片最长边像素上限(>0生效)",
            field: ConfigField::MediaMaxDimensionPx,
            ty: ConfigValueType::Int,
        },
        ConfigOption {
            name: "EPUB首行缩进(em)",
            field: ConfigField::FirstLineIndentEm,
            ty: ConfigValueType::Float,
        },
        ConfigOption {
            name: "是否使用老版本命令行界面(需重启)",
            field: ConfigField::OldCli,
            ty: ConfigValueType::Bool,
        },
    ];

    loop {
        println!("\n=== 配置选项 ===");
        for (idx, opt) in OPTS.iter().enumerate() {
            let mut name = opt.name.to_string();
            if matches!(opt.field, ConfigField::EnableSegmentComments)
                && config.novel_format.eq_ignore_ascii_case("txt")
            {
                name.push_str("（TXT 不支持）");
            }
            println!(
                "{}. {}: {}",
                idx + 1,
                name,
                config_value_display(config, opt.field)
            );
        }
        println!("0. 返回主菜单");

        let choice = super::read_line("\n请选择要修改的配置项编号: ")?;
        let choice = choice.trim();
        if choice == "0" {
            break;
        }
        let Ok(idx) = choice.parse::<usize>() else {
            println!("请输入数字编号");
            continue;
        };
        if idx == 0 || idx > OPTS.len() {
            println!("编号超出范围");
            continue;
        }
        let opt = OPTS[idx - 1];
        let cur = config_value_display(config, opt.field);

        let new_text = if matches!(opt.ty, ConfigValueType::Selection) {
            // 选项模式：列出可选值让用户选择
            match show_selection_prompt(opt.field, &cur)? {
                Some(v) => v,
                None => {
                    println!("已取消修改");
                    continue;
                }
            }
        } else {
            let input = super::read_line(&format!(
                "当前 {} = {}\n输入新值(留空取消): ",
                opt.name, cur
            ))?;
            let trimmed = input.trim().to_string();
            if trimmed.is_empty() {
                println!("已取消修改");
                continue;
            }
            trimmed
        };

        apply_config_edit(config, opt, &new_text)?;

        // 持久化到 config.yml
        write_with_comments(config, Path::new(<Config as ConfigSpec>::FILE_NAME))
            .map_err(|e| anyhow!(e.to_string()))?;
        println!(
            "已更新 {} = {}",
            opt.name,
            config_value_display(config, opt.field)
        );
    }

    Ok(())
}

fn config_value_display(config: &Config, field: ConfigField) -> String {
    match field {
        ConfigField::SavePath => config.save_path.clone(),
        ConfigField::NovelFormat => config.novel_format.clone(),
        ConfigField::BulkFiles => config.bulk_files.to_string(),
        ConfigField::AutoClearDump => config.auto_clear_dump.to_string(),
        ConfigField::AllowOverwriteFiles => config.allow_overwrite_files.to_string(),
        ConfigField::PreferredBookNameField => {
            book_name_field_to_chinese(&config.preferred_book_name_field).to_string()
        }
        ConfigField::EnableAudiobook => config.enable_audiobook.to_string(),
        ConfigField::AudiobookVoice => config.audiobook_voice.clone(),
        ConfigField::AudiobookRate => config.audiobook_rate.clone(),
        ConfigField::AudiobookVolume => config.audiobook_volume.clone(),
        ConfigField::AudiobookPitch => config.audiobook_pitch.clone(),
        ConfigField::AudiobookConcurrency => config.audiobook_concurrency.to_string(),
        ConfigField::AudiobookFormat => config.audiobook_format.clone(),
        ConfigField::MaxWorkers => config.max_workers.to_string(),
        ConfigField::RequestTimeout => config.request_timeout.to_string(),
        ConfigField::MaxRetries => config.max_retries.to_string(),
        ConfigField::MinWaitTime => config.min_wait_time.to_string(),
        ConfigField::MaxWaitTime => config.max_wait_time.to_string(),
        ConfigField::MinConnectTimeout => config.min_connect_timeout.to_string(),
        ConfigField::UseOfficialApi => config.use_official_api.to_string(),
        ConfigField::ApiEndpoints => config.api_endpoints.join(","),
        ConfigField::EnableSegmentComments => config.enable_segment_comments.to_string(),
        ConfigField::SegmentCommentsTopN => config.segment_comments_top_n.to_string(),
        ConfigField::SegmentCommentsWorkers => config.segment_comments_workers.to_string(),
        ConfigField::DownloadCommentImages => config.download_comment_images.to_string(),
        ConfigField::DownloadCommentAvatars => config.download_comment_avatars.to_string(),
        ConfigField::MediaDownloadWorkers => config.media_download_workers.to_string(),
        ConfigField::BlockedMediaDomains => config.blocked_media_domains.join(","),
        ConfigField::ForceConvertImagesToJpeg => config.force_convert_images_to_jpeg.to_string(),
        ConfigField::JpegRetryConvert => config.jpeg_retry_convert.to_string(),
        ConfigField::JpegQuality => config.jpeg_quality.to_string(),
        ConfigField::ConvertHeicToJpeg => config.convert_heic_to_jpeg.to_string(),
        ConfigField::KeepHeicOriginal => config.keep_heic_original.to_string(),
        ConfigField::MediaLimitPerChapter => config.media_limit_per_chapter.to_string(),
        ConfigField::MediaMaxDimensionPx => config.media_max_dimension_px.to_string(),
        ConfigField::FirstLineIndentEm => config.first_line_indent_em.to_string(),
        ConfigField::OldCli => config.old_cli.to_string(),
    }
}

fn apply_config_edit(config: &mut Config, opt: ConfigOption, text: &str) -> Result<()> {
    match opt.ty {
        ConfigValueType::Bool => {
            let v = matches!(
                text.to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "y"
            );
            set_bool(config, opt.field, v)?;
        }
        ConfigValueType::Int => {
            let v: i64 = text
                .parse()
                .map_err(|_| anyhow!("类型转换失败：需要整数"))?;
            set_int(config, opt.field, v)?;
        }
        ConfigValueType::Float => {
            let v: f64 = text
                .parse()
                .map_err(|_| anyhow!("类型转换失败：需要小数"))?;
            set_float(config, opt.field, v)?;
        }
        ConfigValueType::String => {
            set_string(config, opt.field, text)?;
        }
        ConfigValueType::Selection => {
            set_string(config, opt.field, text)?;
        }
        ConfigValueType::List => {
            let parts: Vec<String> = text
                .split([',', '\n'])
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            set_list(config, opt.field, parts)?;
        }
    }
    Ok(())
}

fn set_bool(config: &mut Config, field: ConfigField, v: bool) -> Result<()> {
    match field {
        ConfigField::BulkFiles => config.bulk_files = v,
        ConfigField::AutoClearDump => config.auto_clear_dump = v,
        ConfigField::AllowOverwriteFiles => config.allow_overwrite_files = v,
        ConfigField::EnableAudiobook => config.enable_audiobook = v,
        ConfigField::UseOfficialApi => config.use_official_api = v,
        ConfigField::EnableSegmentComments => {
            if v && config.novel_format.eq_ignore_ascii_case("txt") {
                config.novel_format = "epub".to_string();
                println!("已自动将保存格式切换为 EPUB 以启用段评功能。");
            }
            config.enable_segment_comments = v;
        }
        ConfigField::DownloadCommentImages => config.download_comment_images = v,
        ConfigField::DownloadCommentAvatars => config.download_comment_avatars = v,
        ConfigField::ForceConvertImagesToJpeg => config.force_convert_images_to_jpeg = v,
        ConfigField::JpegRetryConvert => config.jpeg_retry_convert = v,
        ConfigField::ConvertHeicToJpeg => config.convert_heic_to_jpeg = v,
        ConfigField::KeepHeicOriginal => config.keep_heic_original = v,
        ConfigField::OldCli => config.old_cli = v,
        _ => return Err(anyhow!("该字段不是 bool")),
    }
    Ok(())
}

fn set_int(config: &mut Config, field: ConfigField, v: i64) -> Result<()> {
    match field {
        ConfigField::MaxWorkers => {
            if v <= 0 {
                return Err(anyhow!("最大线程数必须大于 0"));
            }
            config.max_workers = v as usize;
        }
        ConfigField::RequestTimeout => {
            if v <= 0 {
                return Err(anyhow!("请求超时必须大于 0"));
            }
            config.request_timeout = v as u64;
        }
        ConfigField::MaxRetries => {
            if v < 0 {
                return Err(anyhow!("最大重试次数不能为负"));
            }
            config.max_retries = v as u32;
        }
        ConfigField::MinWaitTime => {
            if v < 0 {
                return Err(anyhow!("最小等待时间不能为负"));
            }
            let v = v as u64;
            if v > config.max_wait_time {
                return Err(anyhow!("最小等待时间不能超过最大等待时间"));
            }
            config.min_wait_time = v;
        }
        ConfigField::MaxWaitTime => {
            if v < 0 {
                return Err(anyhow!("最大等待时间不能为负"));
            }
            let v = v as u64;
            if v < config.min_wait_time {
                return Err(anyhow!("最大等待时间不能小于最小等待时间"));
            }
            config.max_wait_time = v;
        }
        ConfigField::AudiobookConcurrency => {
            if v <= 0 {
                return Err(anyhow!("有声小说并发数必须大于 0"));
            }
            config.audiobook_concurrency = v as usize;
        }
        ConfigField::SegmentCommentsTopN => {
            if v <= 0 {
                return Err(anyhow!("段评条数上限必须大于 0"));
            }
            config.segment_comments_top_n = v as usize;
        }
        ConfigField::SegmentCommentsWorkers => {
            if v <= 0 {
                return Err(anyhow!("段评线程数必须大于 0"));
            }
            config.segment_comments_workers = v as usize;
        }
        ConfigField::MediaDownloadWorkers => {
            if v <= 0 {
                return Err(anyhow!("媒体线程数必须大于 0"));
            }
            config.media_download_workers = v as usize;
        }
        ConfigField::JpegQuality => {
            if !(0..=100).contains(&v) {
                return Err(anyhow!("JPEG质量需在 0-100 之间"));
            }
            config.jpeg_quality = v as u8;
        }
        ConfigField::MediaLimitPerChapter => {
            if v < 0 {
                return Err(anyhow!("每章媒体数量上限不能为负"));
            }
            config.media_limit_per_chapter = v as usize;
        }
        ConfigField::MediaMaxDimensionPx => {
            if v < 0 {
                return Err(anyhow!("图片最长边像素上限不能为负"));
            }
            config.media_max_dimension_px = v as u32;
        }
        _ => return Err(anyhow!("该字段不是 int")),
    }
    Ok(())
}

fn set_float(config: &mut Config, field: ConfigField, v: f64) -> Result<()> {
    match field {
        ConfigField::MinConnectTimeout => {
            if v <= 0.0 {
                return Err(anyhow!("最小连接超时时间必须大于 0"));
            }
            config.min_connect_timeout = v;
        }
        ConfigField::FirstLineIndentEm => {
            if v < 0.0 {
                return Err(anyhow!("缩进不能为负"));
            }
            config.first_line_indent_em = v as f32;
        }
        _ => return Err(anyhow!("该字段不是 float")),
    }
    Ok(())
}

fn set_string(config: &mut Config, field: ConfigField, v: &str) -> Result<()> {
    match field {
        ConfigField::SavePath => {
            let p = v.trim();
            if p.is_empty() {
                return Err(anyhow!("保存路径不能为空"));
            }
            fs::create_dir_all(p).with_context(|| format!("创建目录失败: {}", p))?;
            config.save_path = p.to_string();
        }
        ConfigField::NovelFormat => {
            let lower = v.trim().to_ascii_lowercase();
            if lower != "txt" && lower != "epub" {
                return Err(anyhow!("保存格式仅支持 txt/epub"));
            }
            if lower == "txt" && config.enable_segment_comments {
                config.enable_segment_comments = false;
                println!("已自动关闭段评以兼容 TXT 格式。");
            }
            config.novel_format = lower;
        }
        ConfigField::AudiobookVoice => config.audiobook_voice = v.to_string(),
        ConfigField::AudiobookRate => config.audiobook_rate = v.to_string(),
        ConfigField::AudiobookVolume => config.audiobook_volume = v.to_string(),
        ConfigField::AudiobookPitch => config.audiobook_pitch = v.to_string(),
        ConfigField::AudiobookFormat => {
            let lower = v.trim().to_ascii_lowercase();
            if lower != "mp3" && lower != "wav" {
                return Err(anyhow!("有声小说格式仅支持 mp3/wav"));
            }
            config.audiobook_format = lower;
        }
        ConfigField::PreferredBookNameField => {
            // 尝试从中文转换，如果失败则尝试直接使用英文
            let field_name = if let Some(english) = chinese_to_book_name_field(v.trim()) {
                english
            } else {
                // 如果不是中文，检查是否是有效的英文字段名
                let lower = v.trim().to_ascii_lowercase();
                if lower == "book_name"
                    || lower == "original_book_name"
                    || lower == "book_short_name"
                    || lower == "ask_after_download"
                {
                    lower
                } else {
                    return Err(anyhow!(
                        "优先书名字段仅支持：默认书名、原始书名、短书名、下载完后选择"
                    ));
                }
            };
            config.preferred_book_name_field = field_name;
        }
        _ => return Err(anyhow!("该字段不是 string")),
    }
    Ok(())
}

fn set_list(config: &mut Config, field: ConfigField, v: Vec<String>) -> Result<()> {
    match field {
        ConfigField::ApiEndpoints => config.api_endpoints = v,
        ConfigField::BlockedMediaDomains => config.blocked_media_domains = v,
        _ => return Err(anyhow!("该字段不是 list")),
    }
    Ok(())
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

/// 选项模式：展示可选项让用户选择编号
fn show_selection_prompt(field: ConfigField, current: &str) -> Result<Option<String>> {
    match field {
        ConfigField::PreferredBookNameField => {
            const OPTIONS: &[(&str, &str)] = &[
                ("默认书名", "book_name"),
                ("原始书名", "original_book_name"),
                ("短书名", "book_short_name"),
                ("下载完后选择", "ask_after_download"),
            ];
            println!("\n当前: {}", current);
            for (idx, (label, _)) in OPTIONS.iter().enumerate() {
                println!("  {}. {}", idx + 1, label);
            }
            println!("  0. 取消");
            let choice = super::read_line("请选择: ")?;
            let choice = choice.trim();
            if choice == "0" || choice.is_empty() {
                return Ok(None);
            }
            let Ok(idx) = choice.parse::<usize>() else {
                println!("请输入数字编号");
                return Ok(None);
            };
            if idx == 0 || idx > OPTIONS.len() {
                println!("编号超出范围");
                return Ok(None);
            }
            Ok(Some(OPTIONS[idx - 1].0.to_string()))
        }
        _ => Ok(None),
    }
}
