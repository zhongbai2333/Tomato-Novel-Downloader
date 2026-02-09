//! 段评缓存共享数据结构与工具函数。
//!
//! 被 `book_parser::finalize_epub` 和 `download::segment_pool` 共同引用。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde_json::Value;

// ── 段评缓存类型 ─────────────────────────────────────────────────

#[cfg(feature = "official-api")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SegmentCommentsParaCache {
    pub(crate) count: u64,
    #[serde(default)]
    pub(crate) detail: Option<tomato_novel_official_api::ReviewResponse>,
}

#[cfg(feature = "official-api")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SegmentCommentsChapterCache {
    #[allow(dead_code)]
    pub(crate) chapter_id: String,
    #[allow(dead_code)]
    pub(crate) book_id: String,
    pub(crate) item_version: String,
    pub(crate) top_n: usize,
    #[serde(default)]
    pub(crate) paras: BTreeMap<String, SegmentCommentsParaCache>,
}

// ── 共享工具函数 ─────────────────────────────────────────────────

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}part",
        path.extension().and_then(|s| s.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, bytes)?;
    // best-effort replace
    let _ = std::fs::remove_file(path);
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub(crate) fn extract_item_version_map(directory_raw: &Value) -> HashMap<String, String> {
    fn pick_string_or_number(v: Option<&Value>) -> Option<String> {
        match v {
            Some(Value::String(s)) => {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    }

    let mut out = HashMap::new();
    let candidates = [
        directory_raw.get("catalog_data"),
        directory_raw.get("item_data_list"),
        directory_raw.get("items"),
    ];

    for arr in candidates {
        let Some(arr) = arr.and_then(Value::as_array) else {
            continue;
        };

        for item in arr {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let id = pick_string_or_number(
                obj.get("item_id")
                    .or_else(|| obj.get("catalog_id"))
                    .or_else(|| obj.get("id")),
            );
            let version = pick_string_or_number(
                obj.get("item_version")
                    .or_else(|| obj.get("version"))
                    .or_else(|| obj.get("item_version_code"))
                    .or_else(|| obj.get("item_version_str")),
            );
            let (Some(id), Some(version)) = (id, version) else {
                continue;
            };
            if !id.is_empty() && !version.is_empty() {
                out.insert(id, version);
            }
        }
    }

    out
}

pub(crate) fn extract_para_counts_from_stats(stats: &Value) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();

    fn pick_i64(v: Option<&Value>) -> Option<i64> {
        match v {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
            _ => None,
        }
    }

    fn push_from_array(out: &mut serde_json::Map<String, Value>, arr: &[Value]) {
        for item in arr {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let idx = pick_i64(
                obj.get("para_index")
                    .or_else(|| obj.get("para_idx"))
                    .or_else(|| obj.get("index"))
                    .or_else(|| obj.get("para_id"))
                    .or_else(|| obj.get("paraId")),
            );
            let cnt = pick_i64(
                obj.get("count")
                    .or_else(|| obj.get("comment_count"))
                    .or_else(|| obj.get("commentCount"))
                    .or_else(|| obj.get("idea_count"))
                    .or_else(|| obj.get("total")),
            );

            let (Some(idx), Some(cnt)) = (idx, cnt) else {
                continue;
            };
            if idx < 0 || cnt <= 0 {
                continue;
            }

            out.insert(
                idx.to_string(),
                Value::Number(serde_json::Number::from(cnt as u64)),
            );
        }
    }

    fn push_from_index_object_map(
        out: &mut serde_json::Map<String, Value>,
        obj: &serde_json::Map<String, Value>,
    ) {
        for (k, v) in obj {
            let Ok(idx) = k.parse::<i64>() else {
                continue;
            };
            if idx < 0 {
                continue;
            }

            let cnt = match v {
                Value::Object(m) => pick_i64(
                    m.get("count")
                        .or_else(|| m.get("comment_count"))
                        .or_else(|| m.get("commentCount"))
                        .or_else(|| m.get("idea_count"))
                        .or_else(|| m.get("total")),
                ),
                Value::Number(_) | Value::String(_) => pick_i64(Some(v)),
                _ => None,
            };

            let Some(cnt) = cnt else {
                continue;
            };
            if cnt <= 0 {
                continue;
            }

            out.insert(
                idx.to_string(),
                Value::Number(serde_json::Number::from(cnt as u64)),
            );
        }
    }

    // Try object-map forms early.
    if let Some(obj) = stats.as_object() {
        push_from_index_object_map(&mut out, obj);
        if !out.is_empty() {
            return out;
        }
    }
    if let Some(obj) = stats.get("data").and_then(Value::as_object) {
        push_from_index_object_map(&mut out, obj);
        if !out.is_empty() {
            return out;
        }
    }

    // Extra robust:
    // - Some variants return { paras: {"0": {count:..}, ... } }
    if let Some(paras) = stats.get("paras").and_then(|v| v.as_object()) {
        for (k, v) in paras {
            let cnt = pick_i64(v.get("count").or_else(|| v.get("comment_count")));
            if let (Ok(idx), Some(cnt)) = (k.parse::<i64>(), cnt)
                && idx >= 0
                && cnt > 0
            {
                out.insert(
                    idx.to_string(),
                    Value::Number(serde_json::Number::from(cnt as u64)),
                );
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    let candidates = [
        stats.get("data_list"),
        stats.get("list"),
        stats.get("idea_list"),
        stats.get("ideas"),
        stats.get("detail").and_then(|d| d.get("data_list")),
        stats.get("detail").and_then(|d| d.get("list")),
    ];

    for v in candidates {
        if let Some(arr) = v.and_then(Value::as_array) {
            push_from_array(&mut out, arr);
        }
    }

    // Fallback: sometimes stats is nested under {"data": ...}
    if out.is_empty()
        && let Some(inner) = stats.get("data")
    {
        if let Some(arr) = inner.get("data_list").and_then(Value::as_array) {
            push_from_array(&mut out, arr);
        }
        if let Some(arr) = inner.get("list").and_then(Value::as_array) {
            push_from_array(&mut out, arr);
        }
    }

    out
}
