use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use tomato_novel_official_api::{ChapterRef, DirectoryClient, FanqieClient, SearchClient};

use crate::base_system::context::{Config, safe_fs_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Txt,
    Epub,
}

impl OutputFormat {
    fn from_config(cfg: &Config) -> Self {
        match cfg.novel_format.to_lowercase().as_str() {
            "epub" => OutputFormat::Epub,
            _ => OutputFormat::Txt,
        }
    }
}

pub fn run(config: &mut Config) -> Result<()> {
    println!("旧版命令行模式 (old_cli)，输入 q 退出，输入 s 进入简易配置提示。\n");

    loop {
        let prompt = format!(
            "请输入 小说ID/链接/书名 (默认保存到 {}，s 配置，q 退出): ",
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
            show_config_hint(config)?;
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
            "保存路径 (回车使用默认 {}): ",
            config.default_save_dir().display()
        ))?;
        if !save_dir_input.trim().is_empty() {
            config.save_path = save_dir_input.trim().to_string();
        }

        println!("开始下载 book_id={}", book_id);
        match download_book(&book_id, config) {
            Ok(()) => println!("下载完成\n"),
            Err(err) => println!("下载失败: {}\n", err),
        }
    }

    Ok(())
}

fn show_config_hint(config: &Config) -> Result<()> {
    println!("配置文件位于当前目录的 config.yml，已载入的关键配置：");
    println!("- 保存路径: {}", config.default_save_dir().display());
    println!("- 小说格式: {}", config.novel_format);
    println!("- 段评: {}", config.enable_segment_comments);
    println!("- 使用官方API: {}", config.use_official_api);
    println!("如需完整修改，请直接编辑 config.yml 后重启。\n");
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
    let output_dir = config.default_save_dir();
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("创建输出目录失败: {}", output_dir.display()))?;

    let directory = DirectoryClient::new().context("初始化目录客户端失败")?;
    let dir = directory
        .fetch_directory(book_id)
        .with_context(|| format!("获取目录失败: book_id={}", book_id))?;

    let chapters = dir.chapters;
    if chapters.is_empty() {
        return Err(anyhow!("目录为空"));
    }

    let client = FanqieClient::new().context("初始化内容客户端失败")?;
    let mut chapter_texts: Vec<(ChapterRef, String)> = Vec::with_capacity(chapters.len());
    for chunk in chapters.chunks(25) {
        let ids = chunk
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let value = fetch_with_cooldown_retry(&client, &ids, false)
            .with_context(|| format!("拉取章节内容失败: {}", ids))?;
        let texts = extract_contents_map(&value);
        for ch in chunk {
            let text = texts.get(&ch.id).cloned().unwrap_or_default();
            chapter_texts.push((ch.clone(), normalize_text(&text)));
        }
    }

    match OutputFormat::from_config(config) {
        OutputFormat::Txt => {
            write_txt(book_id, &output_dir, &chapter_texts).context("写入 TXT 失败")?
        }
        OutputFormat::Epub => {
            write_epub(book_id, &output_dir, &chapter_texts).context("写入 EPUB 失败")?
        }
    }

    println!("已保存到 {}", output_dir.display());
    Ok(())
}

fn fetch_with_cooldown_retry(client: &FanqieClient, ids: &str, epub_mode: bool) -> Result<Value> {
    let mut delay = Duration::from_millis(1100);
    for attempt in 0..6 {
        match client.get_contents(ids, epub_mode) {
            Ok(v) => return Ok(v),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("Cooldown") || msg.contains("CooldownNotReached") {
                    std::thread::sleep(delay);
                    delay = std::cmp::min(delay * 2, Duration::from_secs(8));
                    continue;
                }
                if attempt == 0 {
                    if msg.contains("tomato_novel_network_core") || msg.contains("Library") {
                        return Err(anyhow!(
                            "{}\n\n提示：请先构建 Tomato-Novel-Network-Core，并将动态库放到当前目录或设置 FANQIE_NETWORK_CORE_DLL 指向其绝对路径。",
                            msg
                        ));
                    }
                }
                return Err(anyhow!(msg));
            }
        }
    }
    Err(anyhow!("Cooldown exceeded retries"))
}

fn extract_contents_map(value: &Value) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Some(data) = value.get("data").and_then(|v| v.as_object()) else {
        return out;
    };
    for (cid, info) in data {
        if let Some(text) = info.get("content").and_then(|c| c.as_str()) {
            out.insert(cid.clone(), text.to_string());
        }
    }
    out
}

fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn write_txt(book_id: &str, out_dir: &Path, chapters: &[(ChapterRef, String)]) -> Result<()> {
    let filename = format!("{}_{}.txt", safe_fs_name(book_id, "_", 120), "book");
    let path = out_dir.join(filename);
    let mut f = File::create(&path).with_context(|| format!("创建文件失败: {}", path.display()))?;
    writeln!(f, "book_id={}", book_id)?;
    writeln!(f)?;
    for (ch, content) in chapters {
        writeln!(f, "{}\n", ch.title)?;
        writeln!(f, "{}\n", content.trim())?;
        writeln!(f, "\n----------------------------------------\n")?;
    }
    Ok(())
}

fn write_epub(book_id: &str, out_dir: &Path, chapters: &[(ChapterRef, String)]) -> Result<()> {
    let filename = format!("{}_{}.epub", safe_fs_name(book_id, "_", 120), "book");
    let path = out_dir.join(filename);
    let file = File::create(&path).with_context(|| format!("创建文件失败: {}", path.display()))?;

    let zip = epub_builder::ZipLibrary::new().map_err(|e| anyhow!(e.to_string()))?;
    let mut epub = epub_builder::EpubBuilder::new(zip).map_err(|e| anyhow!(e.to_string()))?;
    epub.metadata("title", book_id).ok();
    epub.metadata("author", "unknown").ok();

    for (idx, (ch, content)) in chapters.iter().enumerate() {
        let xhtml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{}</title></head><body><h2>{}</h2><pre>{}</pre></body></html>",
            escape_html(&ch.title),
            escape_html(&ch.title),
            escape_html(content)
        );
        let name = format!("chapter_{:04}.xhtml", idx + 1);
        epub.add_content(
            epub_builder::EpubContent::new(name, xhtml.as_bytes())
                .title(ch.title.as_str())
                .reftype(epub_builder::ReferenceType::Text),
        )
        .map_err(|e| anyhow!(format!("添加章节失败 {}: {}", ch.id, e)))?;
    }

    epub.generate(file).map_err(|e| anyhow!(e.to_string()))?;
    Ok(())
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn parse_book_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let re_path = regex::Regex::new(r"/page/(\\d+)").ok();
    if let Some(re) = re_path.as_ref() {
        if let Some(caps) = re.captures(trimmed) {
            return Some(caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string());
        }
    }
    let re_qs = regex::Regex::new(r"(?i)(book_id|bookId)=([0-9]+)").ok();
    if let Some(re) = re_qs.as_ref() {
        if let Some(caps) = re.captures(trimmed) {
            return Some(caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string());
        }
    }
    None
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush().ok();
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line)
}
