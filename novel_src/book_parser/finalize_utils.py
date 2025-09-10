"""finalize 输出相关的所有步骤拆分到此模块。

提供对外函数: run_finalize(manager, chapters, result).
"""

from pathlib import Path
import json
from .epub_generator import EpubGenerator
from .segment_utils import (
    render_segment_comments_xhtml,
    segment_meta_count,
    inject_segment_links,
)


def run_finalize(manager, chapters: list[dict], result: int = 0):
    # 以 novel_format 为准；兼容旧字段 output_format
    fmt = (
        getattr(manager.config, "novel_format", None)
        or getattr(manager.config, "output_format", "epub")
    ).lower()
    output_file = _prepare_output_path(manager, fmt)
    if fmt == "epub":
        _finalize_epub(manager, chapters, output_file)
    else:
        _finalize_txt(manager, chapters, output_file)
    _maybe_cleanup_after_finalize(manager, result)


# -------- 公共路径 --------
def _prepare_output_path(manager, fmt: str) -> Path:
    output_dir = Path(getattr(manager.config, "output_dir", "."))
    output_dir.mkdir(parents=True, exist_ok=True)
    suffix = "epub" if fmt == "epub" else "txt"
    return output_dir / f"{manager.book_name}.{suffix}"


# -------- TXT 输出 --------
def _finalize_txt(manager, chapters: list[dict], output_file: Path):
    try:
        with output_file.open("w", encoding="utf-8") as f:
            f.write(
                f"书名: {manager.book_name}\n作者: {manager.author}\n标签: {manager.tags}\n简介: {manager.description}\n"
            )
            for ch in chapters:
                cid = ch.get("id")
                title = manager.downloaded.get(cid, [ch.get("title"), None])[0]
                content = manager.downloaded.get(
                    cid, [None, "Download Faild or Didn't Download Finish!"]
                )[1]
                f.write(f"\n\n{title}\n{content}")
        manager.logger.info(f"TXT生成完成: {output_file}")
    except Exception as e:
        manager.logger.error(f"TXT 输出失败: {e}")


# -------- EPUB 主流程 --------
def _finalize_epub(manager, chapters: list[dict], output_file: Path):
    publisher = None
    if isinstance(manager.tags, str) and manager.tags.strip():
        publisher = ", ".join([t for t in manager.tags.split("|") if t])
    epub = EpubGenerator(
        manager.book_id or manager.book_name or "id",
        manager.book_name or "",
        "zh",
        manager.author or None,
        manager.description or None,
        publisher,
    )
    # 在所有章节前添加“介绍页”作为 EPUB 第一页
    try:
        intro_html = _build_intro_page_html(
            manager,
            total_chapters=len(chapters or []),
        )
        epub.add_aux_page("介绍", intro_html, "intro.xhtml", include_in_spine=True)
    except Exception:
        pass
    # 仅在 EPUB 模式并且启用段评时处理段评
    seg_enabled = (getattr(manager.config, "novel_format", "epub").lower() == "epub") and getattr(manager.config, "enable_segment_comments", False)
    seg_json_map = _scan_segment_json(manager) if seg_enabled else {}
    segment_pages, chapter_ids_set = _add_chapters_and_segment_pages(
        manager, epub, chapters, seg_json_map
    )
    inserted_pages = 0
    # 统一在“所有正文章节”之后，批量追加段评页面（避免章节与段评交替出现）
    if seg_enabled and segment_pages:
        for fname, ptitle, pcontent in segment_pages:
            try:
                # 段评页不纳入 TOC/Spine，仅通过链接访问
                epub.add_aux_page(ptitle, pcontent, fname, include_in_spine=False)
                inserted_pages += 1
            except Exception:
                pass
    if seg_enabled:
        inserted_pages = _insert_missing_segment_pages(
            manager, epub, chapters, seg_json_map, chapter_ids_set, inserted_pages
        )
    _log_epub_segment_stats(manager, epub, seg_json_map, inserted_pages)
    # 仅在段评启用时做媒体进度补齐
    if seg_enabled:
        try:
            manager._media_progress_finalize_fill()
        except Exception:
            pass
    epub.generate(output_file)
    manager.logger.info(f"EPUB生成完成: {output_file}")


def _build_intro_page_html(manager, total_chapters: int) -> str:
    """构造书籍介绍页 HTML（仅 body 片段）。"""
    name = manager.book_name or ""
    author = manager.author or "未知作者"
    tags = []
    if isinstance(manager.tags, str) and manager.tags.strip():
        tags = [t for t in manager.tags.split("|") if t]
    tags_text = " | ".join(tags) if tags else "无标签"
    status_text = "已完结" if getattr(manager, "end", False) else "连载中"
    desc = manager.description or "无简介"
    # 简介适度转义/换行
    try:
        import html as _html
        desc = _html.escape(desc).replace("\n", "<br/>")
    except Exception:
        pass
    html = f"""
    <h2 style='margin:0.2em 0 0.6em 0;'>{name}</h2>
    <p class='no-indent'><strong>作者：</strong>{author}</p>
    <p class='no-indent'><strong>状态：</strong>{status_text}；<strong>章节数：</strong>{total_chapters}</p>
    <p class='no-indent'><strong>标签：</strong>{tags_text}</p>
    <h3>简介</h3>
    <p>{desc}</p>
    """
    return html


# -------- 扫描段评 JSON --------
def _scan_segment_json(manager) -> dict:
    seg_json_map: dict[str, dict] = {}
    seg_dir_scan = manager.status_folder / "segment_comments"
    if seg_dir_scan.exists():
        for jf in seg_dir_scan.glob("*.json"):
            try:
                with jf.open("r", encoding="utf-8") as fr:
                    seg_json_map[jf.stem] = json.load(fr)
            except Exception:
                continue
    try:
        manager.logger.info(f"[段评] 预扫描 JSON 文件数量={len(seg_json_map)}")
    except Exception:
        pass
    return seg_json_map


# -------- 添加章节与段评页面 --------
def _add_chapters_and_segment_pages(
    manager, epub, chapters: list[dict], seg_json_map: dict
):
    """添加正文章节，并收集对应段评页面，稍后统一在末尾追加。"""
    segment_pages: list[tuple[str, str, str]] = []  # (fname, title, content)
    chapter_ids_set: set[str] = set()
    for ch in chapters:
        cid_raw = ch.get("id")
        cid = str(cid_raw)
        chapter_ids_set.add(cid)
        title, content = manager.downloaded.get(cid_raw, [ch.get("title"), None])
        seg_link = ""
        pending_segment_page = None
        if getattr(manager.config, "enable_segment_comments", False):
            seg_data = seg_json_map.get(cid) or manager._load_segment_comments_json(
                cid_raw
            )
            if isinstance(seg_data, dict):
                pending_segment_page, seg_link, content = (
                    _prepare_segment_page_for_chapter(
                        manager, cid, title, content, seg_data
                    )
                )
        epub.add_chapter(
            title,
            ((content or "") + seg_link) if isinstance(content, str) else content,
            file_name=f"chapter_{cid}.xhtml",
        )
        if pending_segment_page:
            try:
                segment_pages.append(pending_segment_page)
                manager.logger.info(
                    f"[段评] 收集段评页面 {pending_segment_page[0]} (章节 {cid})"
                )
            except Exception:
                pass
    return segment_pages, chapter_ids_set


# -------- 单章段评页面准备 --------
def _prepare_segment_page_for_chapter(
    manager, cid: str, title, content, seg_data: dict
):
    seg_link = ""
    pending_segment_page = None
    try:
        paras = seg_data.get("paras") if isinstance(seg_data, dict) else None
        seg_counts: dict[str, int] = {}
        seg_para_count = 0
        seg_para_count_detail: list[str] = []
        comments_file = f"segment_{cid}.xhtml"
        if isinstance(paras, dict) and paras:
            for k2, v2 in paras.items():
                raw_cnt = segment_meta_count(v2)
                if raw_cnt > 0:
                    seg_counts[str(k2)] = raw_cnt
                    seg_para_count += 1
                    seg_para_count_detail.append(f"{k2}:{raw_cnt}")
            if seg_para_count > 0 and isinstance(content, str):
                try:
                    content = inject_segment_links(content, comments_file, seg_counts)
                except Exception:
                    pass
            if seg_para_count > 0:
                seg_link = f"\n<p class='segment-comments-link'><a href='{comments_file}'>查看本章段评（{seg_para_count}段有评论）</a></p>"
            try:
                chapter_html = content if isinstance(content, str) else ""
                comments_content = render_segment_comments_xhtml(
                    manager,
                    title or "",
                    cid,
                    seg_data,
                    back_to_chapter=f"chapter_{cid}.xhtml",
                    chapter_html=chapter_html,
                )
                pending_segment_page = (
                    f"segment_{cid}.xhtml",
                    f"段评 - {title or cid}",
                    comments_content,
                )
                manager.logger.debug(
                    f"[段评] 章节{cid} 准备段评页面 seg_para_count={seg_para_count} 详情={','.join(seg_para_count_detail) if seg_para_count_detail else '空'}"
                )
            except Exception:
                pending_segment_page = None
    except Exception:
        pass
    return pending_segment_page, seg_link, content


# -------- 补插遗漏段评页面 --------
def _insert_missing_segment_pages(
    manager, epub, chapters, seg_json_map, chapter_ids_set, inserted_pages
):
    try:
        seg_dir2 = manager.status_folder / "segment_comments"
        if not seg_dir2.exists():
            return inserted_pages
        # 统计已存在的页面文件名：包含 spine(章节序列) 与非线性清单项
        existing_names = set()
        try:
            existing_names.update(
                {getattr(chap, "file_name", None) for chap in epub.chapters}
            )
        except Exception:
            pass
        try:
            for item in epub.book.get_items():
                fn = getattr(item, "file_name", None)
                if isinstance(fn, str):
                    existing_names.add(fn)
        except Exception:
            pass
        for jf in seg_dir2.glob("*.json"):
            chap_id = jf.stem
            fname = f"segment_{chap_id}.xhtml"
            if fname in existing_names:
                continue
            seg_data = seg_json_map.get(chap_id)
            if seg_data is None:
                try:
                    with jf.open("r", encoding="utf-8") as fr:
                        seg_data = json.load(fr)
                except Exception:
                    continue
            chap_title = _resolve_chapter_title(manager, chapters, chap_id)
            chap_html = manager.downloaded.get(chap_id, [None, ""])[1] or ""
            try:
                page_html = render_segment_comments_xhtml(
                    manager,
                    chap_title or "",
                    chap_id,
                    seg_data,
                    back_to_chapter=f"chapter_{chap_id}.xhtml",
                    chapter_html=chap_html if isinstance(chap_html, str) else "",
                )
                # 段评页不纳入 TOC/Spine，仅通过链接访问
                epub.add_aux_page(
                    f"段评 - {chap_title}", page_html, fname, include_in_spine=False
                )
                inserted_pages += 1
                manager.logger.info(f"[段评] 补插遗漏段评页面 {fname}")
            except Exception:
                pass
        unmatched = [k for k in seg_json_map.keys() if k not in chapter_ids_set]
        if unmatched:
            manager.logger.info(
                f"[段评] 存在未匹配章节 id 的段评 JSON: {unmatched[:8]}{'...' if len(unmatched)>8 else ''}"
            )
    except Exception:
        pass
    return inserted_pages


# -------- 日志汇总 --------
def _log_epub_segment_stats(manager, epub, seg_json_map: dict, inserted_pages: int):
    try:
        manager.logger.info(
            f"[段评] 页面插入汇总: JSON={len(seg_json_map)} 插入={inserted_pages}"
        )
        names = [getattr(chap, "file_name", "?") for chap in epub.chapters]
        manager.logger.info(
            f"[段评] EPUB 章节文件统计: 总数={len(names)}"
        )
    except Exception:
        pass


# -------- 章节标题解析 --------
def _resolve_chapter_title(manager, chapters: list[dict], chap_id: str):
    chap_title = None
    if chap_id in manager.downloaded:
        chap_title = manager.downloaded[chap_id][0]
    if not chap_title:
        for ch in chapters:
            if str(ch.get("id")) == chap_id:
                chap_title = ch.get("title")
                break
    if not chap_title:
        chap_title = chap_id
    return chap_title


# -------- finalize 后清理 --------
def _maybe_cleanup_after_finalize(manager, result: int):
    from ..base_system.storage_system import FileCleaner  # 延迟导入避免循环

    if not (
        result == 0
        and getattr(manager.config, "auto_clear_dump", False)
        and manager.end
    ):
        return
    cover_path = manager.status_folder / f"{manager.book_name}.jpg"
    if manager.status_file.exists():
        try:
            manager.status_file.unlink()
            manager.logger.debug(f"断点缓存文件已清理: {manager.status_file}")
        except Exception:
            pass
    if cover_path.exists():
        try:
            cover_path.unlink()
            manager.logger.debug(f"封面文件已清理: {cover_path}")
        except Exception:
            pass
    try:
        FileCleaner.clean_dump_folder(manager.config.get_status_folder_path)
    except Exception:
        pass
