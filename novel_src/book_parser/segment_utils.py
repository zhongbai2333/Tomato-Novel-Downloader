import re, html, time as _t
from typing import List

# 基础表情映射（可按需扩充）
_EMOJI_MAP = {
    "笑": "😄",
    "哭": "😭",
    "汗": "😅",
    "怒": "😡",
    "痛": "😣",
    "赞": "👍",
    "踩": "👎",
    "惊": "😲",
    "疑": "🤔",
    "色": "😍",
    "呆": "😐",
    "坏": "😈",
    "奸笑": "😏",  # smirk
    "舔屏": "🤤",  # drooling face
    "委屈": "🥺",  # pleading face
    "飞吻": "😘",  # face blowing a kiss
    "爱慕": "🥰",  # smiling face with hearts
    "学会了": "✅",  # got it / learned
    "什么": "❓",  # question
    "大笑": "😂",  # face with tears of joy
    "撇嘴": "😒",  # unamused / pout
    "吃瓜": "🍉",  # melon-eating onlooker
    "震惊": "😱",  # screaming in fear
    "笑哭": "😂",  # tears of joy
    "捂脸": "🤦",  # facepalm
    "微笑": "🙂",  # slight smile
    "思考": "🤔",  # thinking
    "害羞": "😊",  # blushing smile
    "OK": "🆗",  # OK button
    "酷": "😎",  # cool
    "送心": "💖",  # sparkling heart
    "我也强推": "💯",  # strongly recommend
    "惊呆": "😲",  # astonished
    "偷笑": "🤭",  # face with hand over mouth
    "翻白眼": "🙄",  # rolling eyes
    "石化": "🗿",  # moai / petrified
}


def convert_bracket_emojis(text: str) -> str:
    """将 [笑] 形式的简单表情替换为 emoji。"""
    if not isinstance(text, str) or "[" not in text:
        return text

    def repl(m):
        key = m.group(1)
        return _EMOJI_MAP.get(key, m.group(0))

    return re.sub(r"\[([\u4e00-\u9fa5]{1,4})\]", repl, text)


def to_cjk_numeral(n: int) -> str:
    digits = "零一二三四五六七八九"
    if n <= 0:
        return str(n)
    if n < 10:
        return digits[n]
    if n == 10:
        return "十"
    if n < 20:
        return "十" + digits[n - 10]
    if n < 100:
        shi, ge = divmod(n, 10)
        return digits[shi] + "十" + (digits[ge] if ge else "")
    return str(n)


def extract_para_snippet(chapter_html: str, target_idx: int) -> str:
    if not isinstance(chapter_html, str) or target_idx < 0:
        return ""
    try:
        pattern = re.compile(r"(<p\b[^>]*>)(.*?)(</p>)", re.I | re.S)
        idx = 0
        import html as _h

        for m in pattern.finditer(chapter_html):
            if idx == target_idx:
                inner = m.group(2)
                inner_text = re.sub(r"<[^>]+>", "", inner)
                inner_text = _h.unescape(inner_text).strip()
                if not inner_text:
                    return ""
                cut_points = []
                for sep in ["。", "！", "？", ".", "!", "?", "；", "…"]:
                    p = inner_text.find(sep)
                    if p != -1:
                        cut_points.append(p + 1)
                end = min(cut_points) if cut_points else min(len(inner_text), 20)
                return inner_text[:end].strip()
            idx += 1
    except Exception:
        return ""
    return ""


# 结构化解析已移至 network 层，作者名优先使用 user.name，无则置为“匿名”。
def segment_meta_count(meta) -> int:
    try:
        if not isinstance(meta, dict):
            return 0
        c = 0
        try:
            c = int(meta.get("count", 0))
        except Exception:
            c = 0
        if c > 0:
            return c
        dl = (
            ((meta.get("detail") or {}).get("data_list"))
            if isinstance(meta.get("detail"), dict)
            else None
        )
        if isinstance(dl, list):
            return len(dl)
        return 0
    except Exception:
        return 0


def inject_segment_links(
    content_html: str, comments_file: str, seg_counts: dict
) -> str:
    try:
        pattern = re.compile(r"(<p\b[^>]*>)(.*?)(</p>)", re.I | re.S)
        idx = 0
        out = []
        last = 0
        for m in pattern.finditer(content_html):
            out.append(content_html[last : m.start()])
            open_tag, inner, close_tag = m.group(1), m.group(2), m.group(3)
            cnt = 0
            try:
                cnt = int(seg_counts.get(str(idx), 0))
            except Exception:
                cnt = 0
            if cnt > 0:
                if not re.search(r"\bid\s*=", open_tag, re.I):
                    open_tag = open_tag[:-1] + f' id="p-{idx}">'  # 保留其它属性
                badge = (
                    f' <a class="seg-count" href="{html.escape(comments_file)}#para-{idx}" '
                    f'title="查看本段评论">({cnt})</a>'
                )
                inner = inner + badge
            out.append(open_tag + inner + close_tag)
            last = m.end()
            idx += 1
        out.append(content_html[last:])
        return "".join(out)
    except Exception:
        return content_html


def render_segment_comments_xhtml(
    manager,
    chapter_title: str,
    chapter_id: str,
    data: dict,
    back_to_chapter: str | None = None,
    chapter_html: str | None = None,
) -> str:
    """生成段评 XHTML 内容。

    修复问题：之前 for 循环被放在 return 之后，导致永远不执行；同时缺少 top_n_cfg、时间模块与解析器导入。
    """
    parts: List[str] = []
    parts.append(f"<h2>{html.escape(chapter_title)} - 段评</h2>")

    # 读取配置的展示条数
    try:
        top_n_cfg = int(
            getattr(manager, "segment_comments_top_n", None)
            or manager.config.get("segment_comments_top_n", 10)
        )
    except Exception:
        top_n_cfg = 10
    if top_n_cfg <= 0:
        top_n_cfg = 10

    paras = data.get("paras") if isinstance(data, dict) else None
    if not isinstance(paras, dict) or not paras:
        parts.append("<p>暂无段评数据。</p>")
        return "\n".join(parts)

    for key in sorted(paras.keys(), key=lambda x: int(x) if str(x).isdigit() else 0):
        meta = paras.get(key) or {}
        count = segment_meta_count(meta)
        if count <= 0:
            continue

        # 先拿到 detail，读取 network 层清洗的 para_content
        detail = meta.get("detail") if isinstance(meta, dict) else None

        # 段落显示序号 & 摘要
        try:
            disp_idx = int(key) + 1
        except Exception:
            disp_idx = key
        # 使用 para_content 作为片段展示（替代正文分割）
        raw_pc = (
            (detail or {}).get("para_content") if isinstance(detail, dict) else None
        )
        snippet = raw_pc.strip() if isinstance(raw_pc, str) else ""
        cjk_idx = (
            to_cjk_numeral(int(disp_idx)) if str(disp_idx).isdigit() else str(disp_idx)
        )
        if snippet:
            title_html = (
                f'<span class="para-title"><span class="para-index">{html.escape(cjk_idx)}、</span> '
                f'<span class="para-src">&quot;{html.escape(snippet)}&quot;</span> <small>({count})</small></span>'
            )
        else:
            title_html = f'<span class="para-title">第 {html.escape(str(disp_idx))} 段 <small>({count})</small></span>'
        parts.append(f'<h3 id="para-{html.escape(str(key))}">{title_html}</h3>')
        if back_to_chapter:
            parts.append(
                f'<p class="back-to-chapter"><a href="{html.escape(back_to_chapter)}#p-{html.escape(str(key))}">返回本章第 {html.escape(str(disp_idx))} 段</a></p>'
            )
        data_list = (
            (detail or {}).get("data_list") if isinstance(detail, dict) else None
        )
        if not isinstance(data_list, list) or not data_list:
            parts.append("<p>该段暂无可展示的评论。</p>")
            continue

        show_list = data_list[: max(0, top_n_cfg)]

        # 优先使用 network 层提供的结构化 reviews（如果 detail 中带回来了）
        reviews = (detail or {}).get("reviews") if isinstance(detail, dict) else None

        # 轻量回退：无 reviews 时，从原始 show_list 抽取最基本字段
        if not isinstance(reviews, list) or not reviews:
            tmp_reviews = []
            for entry in show_list:
                if not isinstance(entry, dict):
                    continue
                comment = entry.get("comment") if isinstance(entry, dict) else None
                if not isinstance(comment, dict):
                    continue
                common = comment.get("common") if isinstance(comment, dict) else {}
                content = common.get("content") if isinstance(common, dict) else {}
                user_info = common.get("user_info") if isinstance(common, dict) else {}
                base_info = (
                    user_info.get("base_info") if isinstance(user_info, dict) else {}
                )

                text = ""
                if isinstance(content, dict):
                    t = content.get("text")
                    if isinstance(t, str):
                        text = t
                created_ts = None
                ts_val = (
                    common.get("create_timestamp") if isinstance(common, dict) else None
                )
                if isinstance(ts_val, (int, float)):
                    created_ts = int(ts_val)
                stat = comment.get("stat") if isinstance(comment, dict) else {}
                try:
                    digg = int((stat or {}).get("digg_count") or 0)
                except Exception:
                    digg = 0
                avatar = (
                    base_info.get("user_avatar")
                    if isinstance(base_info, dict)
                    else None
                )
                uname = (
                    base_info.get("user_name") if isinstance(base_info, dict) else None
                )

                # 极简图片提取
                images = []
                if isinstance(content, dict):
                    idl = content.get("image_data_list")
                    arr = idl.get("image_data") if isinstance(idl, dict) else None
                    if isinstance(arr, list):
                        for im in arr[:6]:
                            if isinstance(im, dict):
                                u = (
                                    im.get("expand_web_url")
                                    or im.get("web_uri")
                                    or im.get("url")
                                )
                                if isinstance(u, str) and u:
                                    images.append({"url": u})

                tmp_reviews.append(
                    {
                        "text": text,
                        "created_ts": created_ts,
                        "digg_count": digg,
                        "user": {"name": uname, "avatar": avatar},
                        "images": images,
                    }
                )
            reviews = tmp_reviews

        if not reviews:
            parts.append("<p>该段暂无可展示的评论。</p>")
            continue

        parts.append("<ol>")
        for rv in reviews:
            user = rv.get("user") or {}
            author = user.get("name") or "匿名"
            author = html.escape(str(author))
            ts = rv.get("created_ts")
            ts_fmt = ""
            if isinstance(ts, (int, float)) and ts > 0:
                if ts > 1e12:  # 毫秒 → 秒
                    ts = int(ts / 1000)
                ts_fmt = _t.strftime("%Y-%m-%d %H:%M", _t.localtime(int(ts)))
            like = rv.get("digg_count") or 0
            try:
                like = int(like)
            except Exception:
                like = 0
            text = rv.get("text") or ""
            text = convert_bracket_emojis(str(text))
            text = html.escape(text)

            # 头像
            avatar_url = user.get("avatar")
            avatar_img = ""
            if isinstance(avatar_url, str) and avatar_url.startswith("http"):
                fn_av = getattr(manager, "_get_cached_media_filename", lambda x: None)(
                    avatar_url
                )
                if fn_av:
                    avatar_img = f'<img class="avatar" src="images/{html.escape(fn_av)}" alt="avatar" /> '

            # 图片
            img_tags = []
            for im in (rv.get("images") or [])[:6]:
                url = im.get("url") if isinstance(im, dict) else None
                if not isinstance(url, str) or not url:
                    continue
                fn_img = getattr(manager, "_get_cached_media_filename", lambda x: None)(
                    url
                )
                if fn_img:
                    img_tags.append(
                        f'<img src="images/{html.escape(fn_img)}" alt="img" />'
                    )
                else:
                    img_tags.append(f'<img src="{html.escape(url)}" alt="img" />')

            meta_line = f'<small class="seg-meta">{avatar_img}作者：{author}'
            if ts_fmt:
                meta_line += f" | 时间：{html.escape(ts_fmt)}"
            meta_line += f" | 赞：{like}</small>"

            if img_tags:
                parts.append(
                    f"<li class=\"seg-item\"><p>{text}</p><div class=\"seg-images\">{''.join(img_tags)}</div><p>{meta_line}</p></li>"
                )
            else:
                parts.append(
                    f'<li class="seg-item"><p>{text}</p><p>{meta_line}</p></li>'
                )
        parts.append("</ol>")

    parts.append(
        f"<p><small>仅展示每段前 {top_n_cfg} 条评论（若有），实际总数以接口为准。</small></p>"
    )
    return "\n".join(parts)
