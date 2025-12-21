use regex::Regex;

/// 将 [笑] 形式的简单表情替换为 emoji。
pub fn convert_bracket_emojis(text: &str) -> String {
    if !text.contains('[') {
        return text.to_string();
    }
    let map = emoji_map();
    let re = Regex::new(r"\[([\u4e00-\u9fa5]{1,4})\]").unwrap();
    re.replace_all(text, |caps: &regex::Captures| {
        let key = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        map.get(key).cloned().unwrap_or_else(|| caps[0].to_string())
    })
    .to_string()
}

pub fn to_cjk_numeral(n: i32) -> String {
    let digits = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    if n <= 0 {
        return n.to_string();
    }
    if n < 10 {
        return digits[n as usize].to_string();
    }
    if n == 10 {
        return "十".to_string();
    }
    if n < 20 {
        return format!("十{}", digits[(n - 10) as usize]);
    }
    if n < 100 {
        let shi = n / 10;
        let ge = n % 10;
        if ge == 0 {
            format!("{}十", digits[shi as usize])
        } else {
            format!("{}十{}", digits[shi as usize], digits[ge as usize])
        }
    } else {
        n.to_string()
    }
}

/// 提取指定段落的纯文本摘要（用于段评回链）。
pub fn extract_para_snippet(chapter_html: &str, target_idx: usize) -> String {
    let re = Regex::new(r"(<p[^>]*>)(.*?)(</p>)").unwrap();
    let mut idx = 0usize;
    for cap in re.captures_iter(chapter_html) {
        if idx == target_idx {
            let inner = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            let inner_text = strip_tags(inner).trim().to_string();
            if inner_text.is_empty() {
                return String::new();
            }
            let cut = ["。", "！", "？", ".", "!", "?", "；", "…"]
                .iter()
                .filter_map(|sep| inner_text.find(sep).map(|p| p + sep.len()))
                .min()
                .unwrap_or_else(|| inner_text.len().min(20));
            return inner_text[..cut].trim().to_string();
        }
        idx += 1;
    }
    String::new()
}

/// 统计段评 meta 中的条数。
pub fn segment_meta_count(meta: &serde_json::Value) -> usize {
    if !meta.is_object() {
        return 0;
    }
    if let Some(c) = meta.get("count").and_then(|v| v.as_i64()) {
        if c > 0 {
            return c as usize;
        }
    }
    meta.get("detail")
        .and_then(|d| d.get("data_list"))
        .and_then(|v| v.as_array())
        .map(|arr| arr.len())
        .unwrap_or(0)
}

pub fn inject_segment_links(content_html: &str, comments_file: &str, seg_counts: &serde_json::Map<String, serde_json::Value>) -> String {
    let re = Regex::new(r"(<p[^>]*>)(.*?)(</p>)").unwrap();
    let mut out = String::new();
    let mut last_end = 0usize;
    for m in re.find_iter(content_html) {
        out.push_str(&content_html[last_end..m.start()]);
        let caps = re.captures(m.as_str()).unwrap();
        let open_tag = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let inner = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let close_tag = caps.get(3).map(|m| m.as_str()).unwrap_or("");
        let idx = out.matches("<p").count();
        let cnt = seg_counts
            .get(&idx.to_string())
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let mut tag = open_tag.to_string();
        if cnt > 0 && !open_tag.contains("id=") {
            tag = format!("{} id=\"p-{}\">", &open_tag[..open_tag.len() - 1], idx);
        }
        out.push_str(&tag);
        out.push_str(inner);
        if cnt > 0 {
            out.push_str(&format!(
                " <a class=\"seg-count\" href=\"{}#para-{}\">({})</a>",
                comments_file, idx, cnt
            ));
        }
        out.push_str(close_tag);
        last_end = m.end();
    }
    out.push_str(&content_html[last_end..]);
    out
}

fn strip_tags(raw: &str) -> String {
    let re = Regex::new(r"<[^>]+>").unwrap();
    re.replace_all(raw, "").to_string()
}

fn emoji_map() -> std::collections::HashMap<&'static str, String> {
    use std::collections::HashMap;
    let mut m = HashMap::new();
    m.insert("笑", "😄".to_string());
    m.insert("哭", "😭".to_string());
    m.insert("汗", "😅".to_string());
    m.insert("怒", "😡".to_string());
    m.insert("痛", "😣".to_string());
    m.insert("赞", "👍".to_string());
    m.insert("踩", "👎".to_string());
    m.insert("惊", "😲".to_string());
    m.insert("疑", "🤔".to_string());
    m.insert("色", "😍".to_string());
    m.insert("呆", "😐".to_string());
    m.insert("坏", "😈".to_string());
    m.insert("奸笑", "😏".to_string());
    m.insert("舔屏", "🤤".to_string());
    m.insert("委屈", "🥺".to_string());
    m.insert("飞吻", "😘".to_string());
    m.insert("酷", "😎".to_string());
    m.insert("送心", "💖".to_string());
    m.insert("我也强推", "💯".to_string());
    m.insert("惊呆", "😲".to_string());
    m.insert("偷笑", "🤭".to_string());
    m.insert("翻白眼", "🙄".to_string());
    m.insert("石化", "🗿".to_string());
    m
}
