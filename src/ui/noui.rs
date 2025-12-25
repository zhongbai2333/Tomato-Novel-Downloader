use std::fs::{self};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use tomato_novel_official_api::{ChapterRef, DirectoryClient, SearchClient};

use crate::base_system::config::write_with_comments;
use crate::base_system::context::Config;
use crate::download::downloader as dl;

pub fn run(config: &mut Config) -> Result<()> {
    println!(
        "欢迎使用番茄小说下载器（Rust old_cli）。\n\
输入：小说ID/链接/书名\n\
命令：s 配置菜单 | u 更新菜单 | q 退出\n"
    );

    loop {
        let prompt = format!(
            "请输入 小说ID/书本链接（分享链接）/书本名字（输入s配置 / u更新 / q退出，默认保存到 {}）：",
            config.default_save_dir().display()
        );
        let input = read_line(&prompt)?;
        let text = input.trim();
        if text.is_empty() {
            continue;
        }
        if text.eq_ignore_ascii_case("q") {
            println!("已退出。");
            break;
        }
        if text.eq_ignore_ascii_case("s") {
            show_config_menu(config)?;
            continue;
        }
        if text.eq_ignore_ascii_case("u") {
            if let Some(book_id) = update_menu(config)? {
                println!("已选择更新 book_id={}\n", book_id);
                // 直接进入该书下载流程
                match download_book(&book_id, config) {
                    Ok(()) => println!("下载完成\n"),
                    Err(err) => println!("下载失败: {}\n", err),
                }
            }
            continue;
        }

        // 解析 book_id / 链接 / 搜索
        let mut book_id = parse_book_id(text);
        if book_id.is_none() && text.chars().all(|c| c.is_ascii_digit()) {
            book_id = Some(text.to_string());
        }
        if book_id.is_none() {
            book_id = search_and_pick(text)?;
            if book_id.is_none() {
                continue;
            }
        }

        let book_id = book_id.unwrap();
        let save_dir_input = read_line(&format!(
            "保存路径（默认：{}）：",
            config.default_save_dir().display()
        ))?;
        if !save_dir_input.trim().is_empty() {
            let p = save_dir_input.trim().trim_end_matches(['/', '\\']);
            fs::create_dir_all(p).with_context(|| format!("创建目录失败: {}", p))?;
            config.save_path = p.to_string();
        }

        println!("开始下载 book_id={}", book_id);
        match download_book(&book_id, config) {
            Ok(()) => println!("下载完成\n"),
            Err(err) => println!("下载失败: {}\n", err),
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct UpdateEntry {
    book_id: String,
    label: String,
    new_count: usize,
}

fn update_menu(config: &Config) -> Result<Option<String>> {
    let save_dir = config.default_save_dir();
    if !save_dir.exists() {
        println!(
            "没有可供更新的小说（保存目录不存在）：{}\n",
            save_dir.display()
        );
        return Ok(None);
    }

    let (updates, no_updates) = scan_updates(config, &save_dir)?;
    if updates.is_empty() && no_updates.is_empty() {
        println!("没有可供更新的小说\n");
        return Ok(None);
    }

    loop {
        println!("\n===== 可供更新的小说列表 =====");
        for (idx, u) in updates.iter().enumerate() {
            println!("{}. {}", idx + 1, u.label);
        }
        let opt_no_update = if no_updates.is_empty() {
            None
        } else {
            let n = updates.len() + 1;
            println!("{}. 无更新 ({})", n, no_updates.len());
            Some(n)
        };
        println!("q. 退出\n");

        let sel = read_line("请输入编号：")?;
        let sel = sel.trim().to_ascii_lowercase();
        if sel == "q" {
            println!("已取消更新\n");
            return Ok(None);
        }
        let Ok(n) = sel.parse::<usize>() else {
            println!("错误：请输入数字编号或 q 退出。\n");
            continue;
        };

        if n >= 1 && n <= updates.len() {
            return Ok(Some(updates[n - 1].book_id.clone()));
        }

        if let Some(no_idx) = opt_no_update {
            if n == no_idx {
                if let Some(book_id) = select_from_list(&no_updates, "无更新的书籍")? {
                    return Ok(Some(book_id));
                }
                continue;
            }
        }

        let max = opt_no_update.unwrap_or(updates.len());
        println!("错误：请输入 1 到 {} 之间的数字，或 q 退出。\n", max);
    }
}

fn select_from_list(list: &[UpdateEntry], title: &str) -> Result<Option<String>> {
    loop {
        println!("\n===== {} =====", title);
        for (idx, u) in list.iter().enumerate() {
            println!("{}. {}", idx + 1, u.label);
        }
        println!("q. 取消并返回上级菜单\n");

        let sel = read_line("请输入编号：")?;
        let sel = sel.trim().to_ascii_lowercase();
        if sel == "q" {
            return Ok(None);
        }
        let Ok(n) = sel.parse::<usize>() else {
            println!("错误：请输入数字编号或 q 返回。\n");
            continue;
        };
        if n >= 1 && n <= list.len() {
            return Ok(Some(list[n - 1].book_id.clone()));
        }
        println!("错误：请输入 1 到 {} 之间的数字，或 q 返回。\n", list.len());
    }
}

fn scan_updates(_config: &Config, save_dir: &Path) -> Result<(Vec<UpdateEntry>, Vec<UpdateEntry>)> {
    let mut updates = Vec::new();
    let mut no_updates = Vec::new();

    let client = DirectoryClient::new().context("init DirectoryClient")?;
    let dir_reader =
        fs::read_dir(save_dir).with_context(|| format!("read dir {}", save_dir.display()))?;
    for entry in dir_reader.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let Some((book_id, book_name)) = name.split_once('_') else {
            continue;
        };
        if !book_id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let downloaded_count = read_downloaded_count(&path, book_id).unwrap_or(0);
        let chapter_list = match client.fetch_directory(book_id) {
            Ok(d) => d.chapters,
            Err(_) => Vec::new(),
        };
        if chapter_list.is_empty() {
            continue;
        }
        let total = chapter_list.len();
        let new_count = total.saturating_sub(downloaded_count);
        let label = format!("《{}》({}) — 新章节：{}", book_name, book_id, new_count);
        let entry = UpdateEntry {
            book_id: book_id.to_string(),
            label,
            new_count,
        };
        if entry.new_count > 0 {
            updates.push(entry);
        } else {
            no_updates.push(entry);
        }
    }

    Ok((updates, no_updates))
}

fn read_downloaded_count(folder: &Path, book_id: &str) -> Option<usize> {
    let status_new = folder.join("status.json");
    let status_old = folder.join(format!("chapter_status_{}.json", book_id));
    let path = if status_new.exists() {
        status_new
    } else if status_old.exists() {
        status_old
    } else {
        return None;
    };
    let data = fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&data).ok()?;
    let downloaded = value.get("downloaded")?.as_object()?;
    Some(downloaded.len())
}

#[derive(Debug, Clone, Copy)]
enum ConfigValueType {
    Bool,
    Int,
    Float,
    String,
    List,
}

#[derive(Debug, Clone, Copy)]
enum ConfigField {
    SavePath,
    NovelFormat,
    BulkFiles,
    GracefulExit,
    AutoClearDump,
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
    ForceExitTimeout,
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
    MediaTotalLimitMb,
    FirstLineIndentEm,
    OldCli,
}

#[derive(Debug, Clone, Copy)]
struct ConfigOption {
    name: &'static str,
    field: ConfigField,
    ty: ConfigValueType,
}

fn show_config_menu(config: &mut Config) -> Result<()> {
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
            name: "优雅退出模式",
            field: ConfigField::GracefulExit,
            ty: ConfigValueType::Bool,
        },
        ConfigOption {
            name: "是否自动清理缓存文件",
            field: ConfigField::AutoClearDump,
            ty: ConfigValueType::Bool,
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
            name: "强制退出等待时间(秒)",
            field: ConfigField::ForceExitTimeout,
            ty: ConfigValueType::Int,
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
            name: "会话媒体总下载上限(MB,0不限制)",
            field: ConfigField::MediaTotalLimitMb,
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

        let choice = read_line("\n请选择要修改的配置项编号: ")?;
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
        let new_text = read_line(&format!(
            "当前 {} = {}\n输入新值(留空取消): ",
            opt.name, cur
        ))?;
        let new_text = new_text.trim();
        if new_text.is_empty() {
            println!("已取消修改");
            continue;
        }

        apply_config_edit(config, opt, new_text)?;

        // 持久化到 config.yml
        write_with_comments(
            config,
            Path::new(<Config as crate::base_system::config::ConfigSpec>::FILE_NAME),
        )
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
        ConfigField::GracefulExit => config.graceful_exit.to_string(),
        ConfigField::AutoClearDump => config.auto_clear_dump.to_string(),
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
        ConfigField::ForceExitTimeout => config.force_exit_timeout.to_string(),
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
        ConfigField::MediaTotalLimitMb => config.media_total_limit_mb.to_string(),
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
        ConfigField::GracefulExit => config.graceful_exit = v,
        ConfigField::AutoClearDump => config.auto_clear_dump = v,
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
    if v < 0 {
        return Err(anyhow!("该数值不能为负"));
    }

    match field {
        ConfigField::AudiobookConcurrency => config.audiobook_concurrency = v as usize,
        ConfigField::MaxWorkers => config.max_workers = v as usize,
        ConfigField::RequestTimeout => config.request_timeout = v as u64,
        ConfigField::MaxRetries => config.max_retries = v as u32,
        ConfigField::MinWaitTime => config.min_wait_time = v as u64,
        ConfigField::MaxWaitTime => config.max_wait_time = v as u64,
        ConfigField::ForceExitTimeout => config.force_exit_timeout = v as u64,
        ConfigField::SegmentCommentsTopN => config.segment_comments_top_n = v as usize,
        ConfigField::SegmentCommentsWorkers => config.segment_comments_workers = v as usize,
        ConfigField::MediaDownloadWorkers => config.media_download_workers = v as usize,
        ConfigField::JpegQuality => {
            if v > 100 {
                return Err(anyhow!("JPEG质量需在0-100之间"));
            }
            config.jpeg_quality = v as u8;
        }
        ConfigField::MediaLimitPerChapter => config.media_limit_per_chapter = v as usize,
        ConfigField::MediaMaxDimensionPx => config.media_max_dimension_px = v as u32,
        ConfigField::MediaTotalLimitMb => config.media_total_limit_mb = v as u32,
        _ => return Err(anyhow!("该字段不是 int")),
    }
    Ok(())
}

fn set_float(config: &mut Config, field: ConfigField, v: f64) -> Result<()> {
    match field {
        ConfigField::MinConnectTimeout => {
            if v <= 0.0 {
                return Err(anyhow!("最小连接超时时间需>0"));
            }
            config.min_connect_timeout = v;
        }
        ConfigField::FirstLineIndentEm => config.first_line_indent_em = v as f32,
        _ => return Err(anyhow!("该字段不是 float")),
    }
    Ok(())
}

fn set_string(config: &mut Config, field: ConfigField, v: &str) -> Result<()> {
    match field {
        ConfigField::SavePath => {
            let vv = v.trim().trim_end_matches(['/', '\\']);
            if !vv.is_empty() {
                fs::create_dir_all(vv).with_context(|| format!("创建目录失败: {}", vv))?;
                config.save_path = vv.to_string();
            } else {
                config.save_path.clear();
            }
        }
        ConfigField::NovelFormat => {
            let lowered = v.to_ascii_lowercase();
            if lowered != "txt" && lowered != "epub" {
                return Err(anyhow!("小说保存格式必须为 txt 或 epub"));
            }
            config.novel_format = lowered;
            if config.novel_format == "txt" && config.enable_segment_comments {
                config.enable_segment_comments = false;
                println!("TXT 格式不支持段评，已自动关闭段评功能。\n");
            }
        }
        ConfigField::AudiobookFormat => {
            let lowered = v.to_ascii_lowercase();
            if lowered != "mp3" && lowered != "wav" {
                return Err(anyhow!("有声小说格式必须为 mp3 或 wav"));
            }
            config.audiobook_format = lowered;
        }
        ConfigField::AudiobookVoice => config.audiobook_voice = v.to_string(),
        ConfigField::AudiobookRate => config.audiobook_rate = v.to_string(),
        ConfigField::AudiobookVolume => config.audiobook_volume = v.to_string(),
        ConfigField::AudiobookPitch => config.audiobook_pitch = v.to_string(),
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

fn search_and_pick(keyword: &str) -> Result<Option<String>> {
    let client = SearchClient::new().context("init SearchClient")?;
    let resp = client
        .search_books(keyword)
        .with_context(|| format!("搜索失败: {}", keyword))?;

    if resp.books.is_empty() {
        println!("未搜索到结果\n");
        return Ok(None);
    }

    println!("\n===== 搜索结果 =====");
    for (idx, b) in resp.books.iter().enumerate() {
        println!(
            "{}. 书名: {} | ID: {} | 作者: {}",
            idx + 1,
            b.title.as_deref().unwrap_or(""),
            b.book_id,
            b.author.as_deref().unwrap_or("")
        );
    }
    println!("0. 取消\n");

    let choice = read_line("请输入编号：")?;
    let choice = choice.trim();
    if choice == "0" || choice.eq_ignore_ascii_case("q") {
        return Ok(None);
    }
    if let Ok(idx) = choice.parse::<usize>() {
        if idx >= 1 && idx <= resp.books.len() {
            return Ok(Some(resp.books[idx - 1].book_id.clone()));
        }
    }

    println!("输入无效，已取消\n");
    Ok(None)
}

fn download_book(book_id: &str, config: &Config) -> Result<()> {
    let start_time = Instant::now();

    let plan = dl::prepare_download_plan(config, book_id, dl::BookMeta::default())
        .with_context(|| format!("准备下载计划失败: book_id={}", book_id))?;

    let book_name = plan
        .meta
        .book_name
        .clone()
        .unwrap_or_else(|| plan.book_id.clone());

    // 打印书籍信息（对齐 old_main.py 的信息展示）
    println!("\n书名: {}", book_name);
    if let Some(author) = plan.meta.author.as_deref() {
        println!("作者: {}", author);
    }
    if let Some(finished) = plan.meta.finished {
        println!("是否完结: {}", if finished { "完结" } else { "连载" });
    }
    if let Some(count) = plan.meta.chapter_count {
        println!("章节数: {}", count);
    }
    if !plan.meta.tags.is_empty() {
        println!("标签: {}", plan.meta.tags.join("|"));
    }
    if let Some(desc) = plan.meta.description.as_deref() {
        let mut short = desc.to_string();
        if short.chars().count() > 50 {
            short = short.chars().take(50).collect::<String>() + "...";
        }
        println!("简介: {}", short);
    }

    // 初始化 BookManager 并尝试加载历史状态
    let mut manager = dl::init_manager_from_plan(config, &plan)?;
    let resumed =
        manager.load_existing_status(&manager.book_id.clone(), &manager.book_name.clone());
    if resumed {
        println!("\n已检测到历史下载记录，可继续下载或选择重新下载。\n");
    }

    // 若封面已经下载到状态目录，尝试 ASCII 预览
    if let Some(cover) = find_cover_image(manager.book_folder()) {
        let _ = preview_cover_ascii(&cover);
    }

    let total = plan.chapters.len();
    let (downloaded_ok, failed_count) = count_download_state(&manager, &plan.chapters);
    println!(
        "共发现 {} 章，下载失败 {} 章，已下载 {} 章",
        total, failed_count, downloaded_ok
    );

    let mut range: Option<dl::ChapterRange> = None;
    let mode = if downloaded_ok > 0 || failed_count > 0 {
        select_download_mode(failed_count > 0)?
    } else {
        DownloadMode::RangeOrAll
    };

    match mode {
        DownloadMode::Cancel => {
            let _ = manager.cleanup_status_folder();
            return Ok(());
        }
        DownloadMode::Full => {
            manager.downloaded.clear();
            println!("将重新下载全部章节");
        }
        DownloadMode::RangeIgnoreHistory | DownloadMode::RangeOrAll => {
            range = prompt_range(total)?;
            if matches!(mode, DownloadMode::RangeIgnoreHistory) {
                manager.downloaded.clear();
            }
        }
        DownloadMode::Resume | DownloadMode::FailedOnly => {}
    }

    let chosen_chapters = apply_range(&plan.chapters, range);
    if chosen_chapters.is_empty() {
        println!("范围无效或章节为空\n");
        let _ = manager.cleanup_status_folder();
        return Ok(());
    }

    let mut pending = match mode {
        DownloadMode::FailedOnly => dl::pending_failed(&manager, &chosen_chapters),
        _ => dl::pending_resume(&manager, &chosen_chapters),
    };

    if matches!(mode, DownloadMode::Resume) {
        println!(
            "继续下载剩余章节: {} 章 (已完成 {})",
            pending.len(),
            chosen_chapters.len().saturating_sub(pending.len())
        );
    }

    if pending.is_empty() {
        println!("没有需要下载的章节，操作结束。\n");
        dl::finalize_from_manager(&mut manager, &chosen_chapters)?;
        return Ok(());
    }

    println!("\n开始下载...");
    loop {
        let mut reporter = dl::make_reporter(config, &chosen_chapters, &pending, None);
        let book_name = manager.book_name.clone();
        let result = dl::download_chapters_into_manager(
            config,
            &plan.book_id,
            &book_name,
            &mut manager,
            &pending,
            &mut reporter,
            None,
        )?;
        println!(
            "\n下载完成（阶段）成功: {} 章 | 失败: {} 章 | 取消: {} 章",
            result.success, result.failed, result.canceled
        );

        pending = dl::pending_failed(&manager, &chosen_chapters);
        if pending.is_empty() {
            break;
        }

        let ans = read_line("是否重新下载错误章节？[Y/n]: ")?;
        let ans = ans.trim().to_ascii_lowercase();
        if ans == "n" {
            println!("失败章节已保留在缓存/状态文件中。\n");
            break;
        }
        println!("\n重新下载失败章节: {} 章...", pending.len());
    }

    dl::finalize_from_manager(&mut manager, &chosen_chapters)?;
    println!(
        "\n下载完成！用时 {:.1} 秒",
        start_time.elapsed().as_secs_f32()
    );
    println!("已保存到 {}", manager.default_save_dir().display());
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadMode {
    Resume,
    Full,
    FailedOnly,
    RangeIgnoreHistory,
    RangeOrAll,
    Cancel,
}

fn select_download_mode(has_failed: bool) -> Result<DownloadMode> {
    println!("\n===== 下载模式选择 =====");
    println!("1. 继续下载未完成章节");
    println!("2. 全部重新下载");
    if has_failed {
        println!("3. 仅重新下载失败章节");
    }
    println!("4. 指定章节范围重新下载 (忽略历史记录)");
    println!("q. 取消");
    let sel = read_line("请选择(默认1): ")?;
    let sel = sel.trim().to_ascii_lowercase();
    let mode = match sel.as_str() {
        "" | "1" => DownloadMode::Resume,
        "2" => DownloadMode::Full,
        "3" if has_failed => DownloadMode::FailedOnly,
        "4" => DownloadMode::RangeIgnoreHistory,
        "q" => DownloadMode::Cancel,
        _ => DownloadMode::Resume,
    };
    Ok(mode)
}

fn prompt_range(total: usize) -> Result<Option<dl::ChapterRange>> {
    let text = read_line("输入章节范围 形如 10~200 (留空表示全部): ")?;
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let Some((a, b)) = text.split_once('~') else {
        println!("范围格式错误，应为 a~b，将使用全部章节");
        return Ok(None);
    };
    let Ok(mut start) = a.trim().parse::<usize>() else {
        println!("范围解析失败，将使用全部章节");
        return Ok(None);
    };
    let Ok(mut end) = b.trim().parse::<usize>() else {
        println!("范围解析失败，将使用全部章节");
        return Ok(None);
    };
    if start == 0 {
        start = 1;
    }
    if end == 0 {
        end = 1;
    }
    start = start.min(total).max(1);
    end = end.min(total).max(1);
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    println!("已选择章节范围: {}~{}", start, end);
    Ok(Some(dl::ChapterRange { start, end }))
}

fn apply_range(chapters: &[ChapterRef], range: Option<dl::ChapterRange>) -> Vec<ChapterRef> {
    let total = chapters.len();
    match range {
        None => chapters.to_vec(),
        Some(r) => {
            if r.start == 0 || r.start > r.end {
                return Vec::new();
            }
            let start_idx = r.start.saturating_sub(1);
            let end_idx = r.end.min(total).saturating_sub(1);
            if start_idx >= chapters.len() {
                return Vec::new();
            }
            chapters
                .iter()
                .skip(start_idx)
                .take(end_idx.saturating_sub(start_idx) + 1)
                .cloned()
                .collect()
        }
    }
}

fn count_download_state(
    manager: &crate::book_parser::book_manager::BookManager,
    chapters: &[ChapterRef],
) -> (usize, usize) {
    let mut ok = 0usize;
    let mut failed = 0usize;
    for ch in chapters {
        match manager.downloaded.get(&ch.id) {
            Some((_, Some(_))) => ok += 1,
            Some((_, None)) => failed += 1,
            None => {}
        }
    }
    (ok, failed)
}

fn find_cover_image(folder: &Path) -> Option<PathBuf> {
    let rd = fs::read_dir(folder).ok()?;
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
            return Some(p);
        }
    }
    None
}

fn preview_cover_ascii(image_path: &Path) -> Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let cols = cols.max(40) as u32;
    let rows = rows.max(10) as u32;
    println!(
        "\n{}封面预览{}",
        "=".repeat((cols as usize).saturating_sub(16) / 2),
        "=".repeat((cols as usize).saturating_sub(16) / 2)
    );

    let img = image::open(image_path)
        .with_context(|| format!("打开封面失败: {}", image_path.display()))?;
    let gray = img.to_luma8();

    // 字符宽高比矫正：字符通常更“高”，所以宽度多取一些、并降低高度
    let target_w = cols;
    let target_h = (rows.saturating_sub(6)).max(8);
    let resized = image::imageops::resize(
        &gray,
        target_w,
        target_h,
        image::imageops::FilterType::Triangle,
    );

    const PALETTE: &[u8] = b" .:-=+*#%@";
    for y in 0..resized.height() {
        let mut line = String::with_capacity(resized.width() as usize);
        for x in 0..resized.width() {
            let v = resized.get_pixel(x, y)[0] as usize;
            let idx = v * (PALETTE.len() - 1) / 255;
            line.push(PALETTE[idx] as char);
        }
        println!("{}", line);
    }
    println!();
    Ok(())
}

fn parse_book_id(input: &str) -> Option<String> {
    crate::base_system::book_id::parse_book_id(input)
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush().ok();
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line)
}
