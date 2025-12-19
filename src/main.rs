use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use epub_builder::{EpubBuilder, EpubContent, ReferenceType, ZipLibrary};
use serde_json::Value;
use tomato_novel_official_api::{
    ChapterRef, DirectoryClient, FanqieClient, SearchClient,
};

#[derive(Debug, Parser)]
#[command(name = "tomato-novel-downloader")]
#[command(about = "Tomato Novel Downloader (Rust, Official-API + Network-Core)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 搜索书籍（返回 book_id / title / author）
    Search {
        query: String,
    },

    /// 下载整本书：book_id -> 目录(allItemIds) -> 批量拉取章节内容
    Download {
        #[arg(long)]
        book_id: String,

        /// 输出目录（默认当前目录）
        #[arg(long, default_value = ".")]
        out_dir: PathBuf,

        /// 输出格式
        #[arg(long, value_enum, default_value_t = OutputFormat::Txt)]
        format: OutputFormat,

        /// 是否以“epub富内容模式”请求（会触发额外 rich 拉取，当前仅做最小处理）
        #[arg(long, default_value_t = false)]
        epub_mode: bool,
    },

    /// 按章节 ID 列表下载（逗号分隔），输出为 JSON
    FetchContents {
        #[arg(long)]
        chapter_ids: String,

        #[arg(long, default_value_t = false)]
        epub_mode: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Txt,
    Epub,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Search { query } => cmd_search(&query),
        Commands::Download {
            book_id,
            out_dir,
            format,
            epub_mode,
        } => cmd_download(&book_id, &out_dir, format, epub_mode),
        Commands::FetchContents {
            chapter_ids,
            epub_mode,
        } => cmd_fetch_contents(&chapter_ids, epub_mode),
    }
}

fn cmd_search(query: &str) -> Result<()> {
    let client = SearchClient::new().context("init SearchClient")?;
    let resp = client.search_books(query).context("search_books")?;
    if resp.books.is_empty() {
        println!("(no results)");
        return Ok(());
    }
    for (idx, b) in resp.books.iter().enumerate() {
        println!(
            "{}. book_id={} title={} author={}",
            idx + 1,
            b.book_id,
            b.title.as_deref().unwrap_or(""),
            b.author.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

fn cmd_fetch_contents(chapter_ids: &str, epub_mode: bool) -> Result<()> {
    let client = FanqieClient::new().context("init FanqieClient")?;
    let value = client
        .get_contents(chapter_ids, epub_mode)
        .context("get_contents")?;
    println!("{}", serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()));
    Ok(())
}

fn cmd_download(book_id: &str, out_dir: &Path, format: OutputFormat, epub_mode: bool) -> Result<()> {
    fs::create_dir_all(out_dir).with_context(|| format!("create out_dir: {}", out_dir.display()))?;

    let directory = DirectoryClient::new().context("init DirectoryClient")?;
    let dir = directory
        .fetch_directory(book_id)
        .with_context(|| format!("fetch directory for book_id={}", book_id))?;

    let chapters = dir.chapters;
    let client = FanqieClient::new().context("init FanqieClient")?;

    let mut chapter_texts: Vec<(ChapterRef, String)> = Vec::with_capacity(chapters.len());
    for chunk in chapters.chunks(25) {
        let ids = chunk
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let value = fetch_with_cooldown_retry(&client, &ids, epub_mode)
            .with_context(|| format!("fetch contents chunk: {}", ids))?;
        let texts = extract_contents_map(&value);
        for ch in chunk {
            let text = texts.get(&ch.id).cloned().unwrap_or_default();
            chapter_texts.push((ch.clone(), normalize_text(&text)));
        }
    }

    match format {
        OutputFormat::Txt => write_txt(book_id, out_dir, &chapter_texts),
        OutputFormat::Epub => write_epub(book_id, out_dir, &chapter_texts),
    }
}

fn fetch_with_cooldown_retry(client: &FanqieClient, ids: &str, epub_mode: bool) -> Result<Value> {
    // FanqieClient 内部有冷却检查；遇到 CooldownNotReached 时做小退避重试。
    let mut delay = Duration::from_millis(1100);
    for attempt in 0..6 {
        match client.get_contents(ids, epub_mode) {
            Ok(v) => return Ok(v),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("Cooldown") || msg.contains("CooldownNotReached") {
                    thread::sleep(delay);
                    delay = std::cmp::min(delay * 2, Duration::from_secs(8));
                    continue;
                }
                if attempt == 0 {
                    // 常见的 runtime 加载错误：找不到 Network-Core 动态库
                    if msg.contains("tomato_novel_network_core") || msg.contains("Library") {
                        return Err(anyhow::anyhow!(
                            "{}\n\n提示：请先构建 Tomato-Novel-Network-Core，并将动态库放到当前目录或设置 FANQIE_NETWORK_CORE_DLL 指向其绝对路径。",
                            msg
                        ));
                    }
                }
                return Err(anyhow::anyhow!(msg));
            }
        }
    }
    Err(anyhow::anyhow!("Cooldown exceeded retries"))
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
    // 目前返回的 content 可能包含 \n 或少量 HTML；这里做最小处理：统一换行。
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn write_txt(book_id: &str, out_dir: &Path, chapters: &[(ChapterRef, String)]) -> Result<()> {
    let path = out_dir.join(format!("{}_book.txt", book_id));
    let mut f = File::create(&path).with_context(|| format!("create {}", path.display()))?;
    writeln!(f, "book_id={}", book_id)?;
    writeln!(f)?;
    for (ch, content) in chapters {
        writeln!(f, "{}\n", ch.title)?;
        writeln!(f, "{}\n", content.trim())?;
        writeln!(f, "\n----------------------------------------\n")?;
    }
    println!("written: {}", path.display());
    Ok(())
}

fn write_epub(book_id: &str, out_dir: &Path, chapters: &[(ChapterRef, String)]) -> Result<()> {
    let path = out_dir.join(format!("{}_book.epub", book_id));
    let file = File::create(&path).with_context(|| format!("create {}", path.display()))?;

    let zip = ZipLibrary::new().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut epub = EpubBuilder::new(zip).map_err(|e| anyhow::anyhow!(e.to_string()))?;
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
            EpubContent::new(name, xhtml.as_bytes())
                .title(ch.title.as_str())
                .reftype(ReferenceType::Text),
        )
        .map_err(|e| anyhow::anyhow!(format!("add chapter {}: {}", ch.id, e)))?;
    }

    epub.generate(file).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    println!("written: {}", path.display());
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
