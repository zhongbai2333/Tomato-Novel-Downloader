"""书评(评论列表) API 解析模块。

针对示例接口:
https://api5-normal-sinfonlinea.fqnovel.com/novel/commentapi/comment/list/{group_id}/v1?aid=1967&iid=...

示例返回(裁剪):
{
  "code": 0,
  "data": {
      "common_list_info": {"cursor": "10", "has_more": true, "total": 44},
      "data_list": [ { "comment": { ... }, "data_type": 4 }, ...],
      "extra": {"para_content": "..."}
  }
}

本模块提供结构化解析, 避免以往的 “深度遍历猜测字段名” 方式, 明确字段路径; 方便后续扩展/单元测试。

输出目标(每条评论标准化字段):
ReviewItemDict = {
  "comment_id": str,
  "text": str,                # 主文本内容
  "images": [ {"url": str, "width": int|None, "height": int|None} ],
  "comment_type": int|None,
  "content_type": int|None,
  "created_ts": int|None,     # 原始时间戳 (秒)
  "created_iso": str|None,    # ISO8601 字符串
  "digg_count": int,
  "reply_count": int,
  "group_id": str|None,       # 小说/内容 id
  "book_id": str|None,
  "para_src_content": str|None,  # 段落原文 (若有)
  "position": {               # 精确定位(若返回)
       "start_para_index": int|None,
       "end_para_index": int|None,
       "start_word_pos": int|None,
       "end_word_pos": int|None
  },
  "user": {
       "user_id": str|None,
       "name": str|None,
       "avatar": str|None,
       "gender": int|None,
       "description": str|None,
       "titles": [str],         # user_title_info[].zh_title 或 title_text
       "sticker": str|None      # 若有勋章贴纸名称
  }
}

parse_comment_list_api(raw_json) -> (meta_dict, reviews_list)
meta_dict = {"cursor": str|None, "has_more": bool, "total": int|None, "para_content": str|None}

设计要点:
1. 显式字段路径 (便于维护): comment.common.content.text / comment.stat.digg_count 等。
2. 对缺失字段容错, 统一返回空或 None。
3. 图片结构: content.image_data_list.image_data[] 下的 expand_web_url 或 web_uri 任选其一; 优先 expand_web_url。
4. 兼容 content_type != 0 时仍然提取 text (有些类型返回图片+文字)。
5. 只做轻量解析, 不做网络/下载行为; 与段评解析分离。

后续拓展建议:
- 可添加 dataclass ReviewItem 以获得类型提示; 当前保持单纯 dict 以减少依赖。
- 可加入简单的 schema 校验 (pydantic) 但会引入额外依赖, 暂不启用。
"""
from __future__ import annotations

from typing import Any, Dict, List, Tuple
from datetime import datetime, timezone

JsonDict = Dict[str, Any]
ReviewItem = Dict[str, Any]


def _get(d: Any, path: str, default=None):
    """按 'a.b.c' 路径安全取值."""
    cur = d
    for p in path.split("."):
        if isinstance(cur, dict) and p in cur:
            cur = cur[p]
        else:
            return default
    return cur


def _to_int(v, default=0):
    try:
        if v in (None, ""):
            return default
        return int(v)
    except Exception:
        return default


def _to_str(v, default=None):
    if v is None:
        return default
    if isinstance(v, str):
        return v
    try:
        return str(v)
    except Exception:
        return default


def _extract_images(content: JsonDict) -> List[Dict[str, Any]]:
    images: List[Dict[str, Any]] = []
    if not isinstance(content, dict):
        return images
    idl = content.get("image_data_list")
    if not isinstance(idl, dict):
        return images
    arr = idl.get("image_data")
    if not isinstance(arr, list):
        return images
    for it in arr:
        if not isinstance(it, dict):
            continue
        url = (
            _to_str(it.get("expand_web_url"))
            or _to_str(it.get("web_uri"))
            or _to_str(it.get("url"))
        )
        if not url:
            continue
        images.append(
            {
                "url": url,
                "width": _to_int(it.get("width"), default=None),
                "height": _to_int(it.get("height"), default=None),
                "format": _to_str(it.get("format"), default=None),
            }
        )
    return images


def _extract_user(common: JsonDict) -> Dict[str, Any]:
    user_info = _get(common, "user_info") or {}
    base_info = user_info.get("base_info") if isinstance(user_info, dict) else {}
    user_tag = user_info.get("user_tag") if isinstance(user_info, dict) else {}

    titles: List[str] = []
    try:
        info_list = user_tag.get("user_title_info") if isinstance(user_tag, dict) else None
        if isinstance(info_list, list):
            for t in info_list:
                if not isinstance(t, dict):
                    continue
                # 优先 zh_title -> label.title_text -> en_title
                title = (
                    _to_str(t.get("zh_title"))
                    or _to_str(t.get("title"))
                    or _to_str(t.get("en_title"))
                )
                if title:
                    titles.append(title)
    except Exception:
        pass

    sticker_name = None
    try:
        sticker = user_tag.get("sticker") if isinstance(user_tag, dict) else None
        if isinstance(sticker, dict):
            inner = sticker.get("sticker")
            if isinstance(inner, dict):
                sticker_name = _to_str(inner.get("name"))
    except Exception:
        pass

    return {
        "user_id": _to_str(base_info.get("user_id") or user_info.get("user_id")),
        "name": _to_str(base_info.get("user_name")),
        "avatar": _to_str(base_info.get("user_avatar")),
        "gender": base_info.get("gender"),
        "description": _to_str(base_info.get("description"), default=""),
        "titles": titles,
        "sticker": sticker_name,
    }


def _extract_position(expand: JsonDict) -> Dict[str, Any]:
    pos = expand.get("comment_pos") if isinstance(expand, dict) else None
    if not isinstance(pos, dict):
        return {
            "start_para_index": None,
            "end_para_index": None,
            "start_word_pos": None,
            "end_word_pos": None,
        }
    return {
        "start_para_index": _to_int(pos.get("start_para_index"), default=None),
        "end_para_index": _to_int(pos.get("end_para_index"), default=None),
        "start_word_pos": _to_int(pos.get("start_word_pos"), default=None),
        "end_word_pos": _to_int(pos.get("end_word_pos"), default=None),
    }


def _ts_to_iso(ts: int | None) -> str | None:
    if not ts:
        return None
    try:
        return datetime.fromtimestamp(int(ts), tz=timezone.utc).isoformat()
    except Exception:
        return None


def parse_comment_list_api(raw: JsonDict) -> Tuple[Dict[str, Any], List[ReviewItem]]:
    """解析书评列表 API 响应。

    参数:
        raw: 已反序列化的 JSON (dict)
    返回:
        (meta, reviews)
        meta = {cursor, has_more, total, para_content}
        reviews = list[ReviewItem]
    """
    data = raw.get("data") if isinstance(raw, dict) else {}
    meta_info = data.get("common_list_info") if isinstance(data, dict) else {}
    cursor = _to_str(meta_info.get("cursor")) if isinstance(meta_info, dict) else None
    has_more = bool(meta_info.get("has_more")) if isinstance(meta_info, dict) else False
    total = _to_int(meta_info.get("total"), default=None)

    extra = data.get("extra") if isinstance(data, dict) else {}
    para_content = _to_str(extra.get("para_content"), default=None)

    reviews: List[ReviewItem] = []
    arr = data.get("data_list") if isinstance(data, dict) else []
    if isinstance(arr, list):
        for entry in arr:
            if not isinstance(entry, dict):
                continue
            comment = entry.get("comment")
            if not isinstance(comment, dict):
                continue
            common = comment.get("common") if isinstance(comment, dict) else {}
            expand = comment.get("expand") if isinstance(comment, dict) else {}
            stat = comment.get("stat") if isinstance(comment, dict) else {}
            content = common.get("content") if isinstance(common, dict) else {}

            text = _to_str(content.get("text"), default="") if isinstance(content, dict) else ""
            images = _extract_images(content)
            created_ts = _to_int(common.get("create_timestamp"), default=None)

            item: ReviewItem = {
                "comment_id": _to_str(comment.get("comment_id")),
                "text": text,
                "images": images,
                "comment_type": common.get("comment_type"),
                "content_type": common.get("content_type"),
                "created_ts": created_ts,
                "created_iso": _ts_to_iso(created_ts),
                "digg_count": _to_int(stat.get("digg_count"), default=0),
                "reply_count": _to_int(stat.get("reply_count"), default=0),
                "group_id": _to_str(common.get("group_id")),
                "book_id": _to_str(expand.get("book_id")),
                "para_src_content": _to_str(expand.get("para_src_content"), default=None),
                "position": _extract_position(expand),
                "user": _extract_user(common),
            }
            reviews.append(item)

    meta = {
        "cursor": cursor,
        "has_more": has_more,
        "total": total,
        "para_content": para_content,
    }
    return meta, reviews
