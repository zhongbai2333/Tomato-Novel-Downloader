use serde_json::Value;

pub type JsonMap = serde_json::Map<String, Value>;

pub fn collect_maps<'a>(raw: &'a Value) -> Vec<&'a JsonMap> {
    let mut maps = Vec::new();
    if let Some(map) = raw.as_object() {
        maps.push(map);
        if let Some(info) = map.get("book_info").and_then(|v| v.as_object()) {
            maps.push(info);
        }
        if let Some(info) = map.get("bookInfo").and_then(|v| v.as_object()) {
            maps.push(info);
        }
        if let Some(info) = map.get("book_data").and_then(|v| v.as_object()) {
            maps.push(info);
        }
        if let Some(info) = map.get("data").and_then(|v| v.as_object()) {
            maps.push(info);
        }
        if let Some(info) = map.get("meta").and_then(|v| v.as_object()) {
            maps.push(info);
        }
    }
    maps
}

pub fn pick_string(map: &JsonMap, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(val) = map.get(*key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            } else if let Some(n) = val.as_i64() {
                return Some(n.to_string());
            } else if let Some(n) = val.as_u64() {
                return Some(n.to_string());
            } else if let Some(n) = val.as_f64() {
                if n.is_finite() {
                    return Some(n.to_string());
                }
            }
        }
    }
    None
}

pub fn pick_tags(map: &JsonMap) -> Vec<String> {
    let candidates = [
        "tags",
        "book_tags",
        "tag",
        "category",
        "categories",
        "classify_tags",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            let out = tags_from_value(val);
            if !out.is_empty() {
                return out;
            }
        }
    }
    Vec::new()
}

pub fn pick_tags_opt(map: &JsonMap) -> Option<Vec<String>> {
    let tags = pick_tags(map);
    if tags.is_empty() { None } else { Some(tags) }
}

pub fn pick_cover(map: &JsonMap) -> Option<String> {
    let candidates = [
        "cover",
        "cover_url",
        "pic_url",
        "thumb_url",
        "thumb",
        "coverUrl",
        "picUrl",
        "book_cover",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(url) = val.as_str() {
                let trimmed = url.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn pick_detail_cover(map: &JsonMap) -> Option<String> {
    let candidates = [
        "detail_page_thumb_url",
        "detail_thumb",
        "detail_cover",
        "detail_cover_url",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn pick_finished(map: &JsonMap) -> Option<bool> {
    // Stronger signals first.
    if let Some(ts) = pick_string(map, &["creation_latest_finish_time", "finish_time"]) {
        if let Ok(n) = ts.trim().parse::<i64>()
            && n > 0
        {
            return Some(true);
        }
    }

    // update_status: 1=still updating; 0 often means not updating (finished or stopped) so treat as unknown.
    if let Some(s) = pick_string(map, &["update_status", "updateStatus"]) {
        if let Ok(n) = s.trim().parse::<i64>() {
            if n == 1 {
                return Some(false);
            }
        }
    }

    let candidates = [
        "is_finish",
        "is_finished",
        "finish_status",
        "finishstate",
        "finish_state",
        "is_end",
        "isEnd",
        "finish",
        "finished",
        "book_status",
        "status",
        "serial_status",
        "serialStatus",
        "finishStatus",
    ];

    let parse_num = |key: &str, n: i64| -> Option<bool> {
        match key {
            // These fields are typically enums: 1=连载, 2=完结.
            "status" | "book_status" | "serial_status" | "serialStatus" | "finish_status"
            | "finishStatus" => match n {
                2 => Some(true),
                // Many APIs use 0/1 as non-terminal states or always "1"; do not treat them as definitive.
                0 | 1 => None,
                _ => None,
            },
            // Binary flags.
            _ => match n {
                1 | 2 => Some(true),
                0 => Some(false),
                _ => None,
            },
        }
    };

    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(b) = val.as_bool() {
                return Some(b);
            }
            if let Some(n) = val.as_i64() {
                if let Some(b) = parse_num(key, n) {
                    return Some(b);
                }
            }
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if let Ok(n) = trimmed.parse::<i64>() {
                    if let Some(b) = parse_num(key, n) {
                        return Some(b);
                    }
                }
                let lower = trimmed.to_ascii_lowercase();
                if ["true", "yes", "finished", "end", "completed", "serial_end"]
                    .contains(&lower.as_str())
                {
                    return Some(true);
                }
                if [
                    "false",
                    "no",
                    "ongoing",
                    "serialize",
                    "serializing",
                    "serial",
                ]
                .contains(&lower.as_str())
                {
                    return Some(false);
                }

                // String enums for status-like keys.
                if matches!(
                    key,
                    "status"
                        | "book_status"
                        | "serial_status"
                        | "serialStatus"
                        | "finish_status"
                        | "finishStatus"
                ) {
                    if lower == "2" {
                        return Some(true);
                    }
                    // Treat 0/1 as unknown for these keys.
                }
            }
        }
    }

    None
}

pub fn pick_chapter_count(map: &JsonMap) -> Option<usize> {
    let candidates = [
        "item_cnt",
        "book_item_cnt",
        "chapter_num",
        "chapter_count",
        "chapter_total_cnt",
        "serial_count",
        "content_chapter_number",
        "content_count",
        "total_chapter_count",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(n) = val.as_u64() {
                return Some(n as usize);
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

pub fn pick_word_count(map: &JsonMap) -> Option<usize> {
    let candidates = ["word_number", "word_count", "word_cnt", "words"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(n) = val.as_u64() {
                return Some(n as usize);
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

pub fn pick_score(map: &JsonMap) -> Option<f32> {
    let candidates = ["score", "book_score", "rating"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(n) = val.as_f64() {
                return Some(n as f32);
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<f32>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

pub fn pick_read_count(map: &JsonMap) -> Option<String> {
    let candidates = ["read_count", "read_count_all", "readcnt", "pv"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            } else if let Some(n) = val.as_u64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

pub fn pick_read_count_text(map: &JsonMap) -> Option<String> {
    let candidates = ["read_cnt_text", "read_count_text"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn pick_book_short_name(map: &JsonMap) -> Option<String> {
    let candidates = ["book_short_name", "short_name", "short_title"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn pick_original_book_name(map: &JsonMap) -> Option<String> {
    let candidates = ["original_book_name", "origin_title", "original_title"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn pick_first_chapter_title(map: &JsonMap) -> Option<String> {
    let candidates = [
        "first_chapter_title",
        "first_catalog_title",
        "firstItemTitle",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn pick_last_chapter_title(map: &JsonMap) -> Option<String> {
    let candidates = [
        "last_chapter_title",
        "latest_catalog_title",
        "lastItemTitle",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn pick_category(map: &JsonMap) -> Option<String> {
    let candidates = ["category", "category_name", "book_category", "classify"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn pick_cover_primary_color(map: &JsonMap) -> Option<String> {
    let candidates = ["cover_primary_color", "primary_color", "cover_color"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn tags_from_value(value: &Value) -> Vec<String> {
    match value {
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(s) => s
            .split(['|', ',', ';', ' '])
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .collect(),
        _ => Vec::new(),
    }
}
