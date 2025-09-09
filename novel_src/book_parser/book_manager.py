import os
import json
import re
import html
import hashlib
import threading
from urllib.parse import urlparse
from pathlib import Path
from typing import Dict, List
from concurrent.futures import ThreadPoolExecutor, as_completed
import requests

from ..base_system.context import GlobalContext
from ..base_system.storage_system import FileCleaner
from .epub_generator import EpubGenerator

# 默认章节模板内容（当启用模板功能但指定文件不存在时自动生成）
DEFAULT_CHAPTER_TEMPLATE = (
    "{title}\n\n"  # 标题占位符
    "{{for p in paragraphs}}    {p}\n"  # 每段前 4 空格，后续逻辑会替换为全角或 &nbsp;
    "{{end}}"
)

class BookManager(object):
    """书籍存储控制器"""
    def __init__(self, save_path: str, book_id: str, book_name: str, author: str, tags: list, description: str):
        # 书本信息缓存
        self.save_dir = Path(save_path)
        self.book_id = book_id
        self.book_name = book_name
        self.author = author
        self.end = True if (tags and tags[0] == "已完结") else False
        self.tags = "|".join(tags)
        self.description = description

        # 初始化
        self.config = GlobalContext.get_config()
        self.logger = GlobalContext.get_logger()

        # 缓存
        self.downloaded: Dict[str, List[str]] = {}

        # 状态文件路径
        filename = f"chapter_status_{book_id}.json"
        self.status_folder = self.config.get_status_folder_path
        self.status_file = self.status_folder / filename

        self._load_download_status()
        # 标记：段评媒体是否已在下载阶段预取，避免 finalize 再次处理
        self._media_prefetched = False
    # 已移除段评图片进度条显示（静默下载）
        # 段评媒体预取执行器（在保存段评时并发启动）
        try:
            from concurrent.futures import ThreadPoolExecutor as _TP
            self._media_prefetch_executor = _TP(max_workers=2)
        except Exception:
            self._media_prefetch_executor = None

    def _load_download_status(self):
        """加载完整的下载状态"""
        try:
            if self.status_file.exists():
                with self.status_file.open("r", encoding="utf-8") as f:
                    data = json.load(f)
                    self.book_name = data.get("book_name", "")
                    self.author = data.get("author", "")
                    self.tags = data.get("tags", "")
                    self.description = data.get("description", "")
                    self.downloaded = data.get("downloaded", {})
        except Exception as e:
            self.logger.error(f"状态文件加载失败: {e}")
            self.downloaded = {}

    def save_chapter(self, chapter_id: str, title: str, content: str):
        """保存章节内容，支持散装保存（EPUB 下生成完整 XHTML）"""
        # 在写入缓存前可应用自定义章节模板
        processed_content = content
        try:
            if getattr(self.config, 'enable_chapter_template', False):
                tpl_path = getattr(self.config, 'chapter_template_file', 'chapter_template.txt')
                tpl_file = Path(tpl_path)
                # 支持相对路径（相对于运行目录 / 配置状态目录）
                if not tpl_file.exists():
                    alt = self.status_folder / tpl_path
                    if alt.exists():
                        tpl_file = alt
                if tpl_file.exists():
                    raw_tpl = tpl_file.read_text(encoding='utf-8', errors='ignore')
                    processed_content = self._render_chapter_template(raw_tpl, title, content)
                else:
                    # 模板不存在：尝试自动生成默认模板
                    try:
                        tpl_file.parent.mkdir(parents=True, exist_ok=True)
                        tpl_file.write_text(DEFAULT_CHAPTER_TEMPLATE, encoding='utf-8')
                        self.logger.info(f"章节模板未找到，已自动生成默认模板: {tpl_file}")
                        processed_content = self._render_chapter_template(DEFAULT_CHAPTER_TEMPLATE, title, content)
                    except Exception as ge:
                        self.logger.warning(f"章节模板未找到且自动生成失败: {tpl_path} ({ge})")
        except Exception as e:
            self.logger.debug(f"章节模板处理失败: {e}")

        # ---- 模板输出后处理 ----
        try:
            if getattr(self.config, 'enable_chapter_template', False):
                # 1) EPUB（非散装）场景下 EpubGenerator 通常会再包一层 <h1>标题，避免首行重复标题
                if (not self.config.bulk_files) and self.config.novel_format == 'epub':
                    lines = [l for l in processed_content.split('\n')]
                    # 跳过前导空行找第一行实际内容
                    first_idx = 0
                    while first_idx < len(lines) and lines[first_idx].strip() == '':
                        first_idx += 1
                    if first_idx < len(lines) and lines[first_idx].strip() == title.strip():
                        lines.pop(first_idx)
                        processed_content = '\n'.join(lines).lstrip('\n')
                # 2) 将用户以四个空格期望的“中文首行缩进”转换为全角空格（或 &emsp;&emsp;）避免 HTML 渲染吞掉空格
                # 两个全角空格“　　”在大多数阅读器中更稳定。
                processed_content = re.sub(r'(?m)^( {4})(\S)', r'　　\2', processed_content)
                # 3) 若需要直接保留模板中写下的前导空格（包含循环里写的    {p} 形式），自动转为 &nbsp; 保证 EPUB 阅读器不会折叠
                if self.config.novel_format == 'epub':
                    # 仅转换每一行行首连续空格，避免破坏行内普通空格。
                    def _lead_space_to_nbsp(m):
                        return '&nbsp;' * len(m.group(1))
                    processed_content = re.sub(r'(?m)^( +)', _lead_space_to_nbsp, processed_content)
        except Exception:
            pass

        self.downloaded[chapter_id] = [title, processed_content]
        if self.config.bulk_files:
            bulk_dir = self.save_dir / self.book_name
            bulk_dir.mkdir(parents=True, exist_ok=True)

            if self.config.novel_format == "epub":
                suffix = ".xhtml"
                # 使用 processed_content 而不是原始 content，且不再强制加入标题（让模板自行控制）
                xhtml_template = f'''<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>{title}</title>
  <meta charset="utf-8" />
  <style>p{{margin:0 0 0.8em 0;}}</style>
</head>
<body>
{processed_content}
</body>
</html>'''
                file_content = xhtml_template
            else:
                suffix = ".txt"
                # TXT 散装文件若模板已含标题，避免重复：检测首行（忽略空行）是否已是标题
                _lines = processed_content.split('\n')
                _i = 0
                while _i < len(_lines) and _lines[_i].strip() == '':
                    _i += 1
                if _i < len(_lines) and _lines[_i].strip() == title.strip():
                    file_content = processed_content
                else:
                    file_content = f"{title}\n\n{processed_content}"

            # 把 title 中的非法文件名字符替换掉
            safe_title = "".join(c for c in title if c.isalnum() or c in "-_ ")
            filename = f"{safe_title}{suffix}"
            file_path = bulk_dir / filename

            with file_path.open("w", encoding="utf-8") as f:
                f.write(file_content)

            self.logger.debug(f"章节散装保存：{file_path}")
        else:
            self.save_download_status()

        self.logger.debug(f"章节 {chapter_id} 缓存成功")

    def _render_chapter_template(self, template: str, title: str, raw_content: str) -> str:
        """渲染自定义章节模板。
        支持占位符:
          {title} 章节标题
          {content} 原始内容（不做额外缩进）
          {text} 整体内容前可统一加缩进（即保持原换行）
          {paragraphs} 按段落处理拼接（段落分隔: 空行或换行）
        额外支持一个简易循环：
          {{for p in paragraphs}}...{p}...{{end}}
        在循环中使用 {p} 占位符代表单个段落文本（可含前缀缩进）
        特殊缩进规则：
          模板中出现 "{text}" 前若有 4 个或更多空格紧贴，可视为缩进指令，对整段应用同样前缀。
          模板中出现 "{paragraph}" 类似；不过建议在循环语法中使用。
        """
        try:
            # 统一行结束
            raw = raw_content.replace('\r\n', '\n').replace('\r', '\n')
            # 切分段落（简单规则：按单个换行拆，保留空行）
            paragraphs = [p for p in raw.split('\n')]
            # 基础上下文
            ctx = {
                'title': title,
                'content': raw,
            }
            # 处理缩进：若模板中直接使用 {text}，不改；
            # 若用户写成 "    {text}" 则把前导空格作为整块缩进
            def apply_block_indent(placeholder: str, block: str) -> str:
                pattern = re.compile(rf'^(?P<indent>[ \t]+)\{{{placeholder}\}}$', re.MULTILINE)
                def repl(m):
                    ind = m.group('indent')
                    new_lines = [ind + ln if ln.strip() else ln for ln in block.split('\n')]
                    return '\n'.join(new_lines)
                return re.sub(pattern, repl, template)

            rendered = template
            # 简易循环实现
            loop_pattern = re.compile(r'\{\{for p in paragraphs\}\}(.*?)\{\{end\}\}', re.DOTALL)
            def loop_repl(m):
                body = m.group(1)
                out_parts = []
                for para in paragraphs:
                    # 处理单段缩进： 若 body 中出现独立行 "    {paragraph}" -> 应用缩进到该段
                    b = body
                    # {paragraph} 直接替换为 para（保持原样）
                    b = b.replace('{paragraph}', para)
                    # {p} 占位符
                    b = b.replace('{p}', para)
                    out_parts.append(b)
                return ''.join(out_parts)
            rendered = re.sub(loop_pattern, loop_repl, rendered)

            # paragraphs 拼接（保持原行结构）
            rendered = rendered.replace('{paragraphs}', '\n'.join(paragraphs))
            # text 语义：整体内容，若使用 "    {text}" 应套缩进
            if '{text}' in rendered:
                rendered = rendered.replace('{text}', raw)
            # content 保留原始
            rendered = rendered.replace('{content}', raw)
            # title
            rendered = rendered.replace('{title}', title)
            return rendered
        except Exception as e:
            self.logger.debug(f"章节模板渲染异常: {e}")
            return raw_content

    def save_segment_comments(self, chapter_id: str, payload: dict):
        """
        保存某个章节的段评数据到状态目录下的 JSON 文件。
        文件路径: <status_folder>/segment_comments/<chapter_id>.json
        """
        try:
            seg_dir = self.status_folder / "segment_comments"
            seg_dir.mkdir(parents=True, exist_ok=True)
            out_path = seg_dir / f"{chapter_id}.json"
            with out_path.open("w", encoding="utf-8") as f:
                json.dump(payload, f, ensure_ascii=False, indent=2)
            self.logger.debug(f"段评已保存: {out_path}")
            # 用户可见提示：告知段评保存完成（用于进度条外的心跳感知）
            try:
                paras = payload.get("paras") if isinstance(payload, dict) else None
                seg_cnt = 0
                cmts_total = 0
                if isinstance(paras, dict):
                    for _k, _v in paras.items():
                        try:
                            c = int((_v or {}).get("count", 0))
                        except Exception:
                            c = 0
                        if c > 0:
                            seg_cnt += 1
                            cmts_total += c
                self.logger.info(f"[段评] 章节 {chapter_id} 已保存：含 {seg_cnt} 段有评论，共 {cmts_total} 条（前 {getattr(self.config,'segment_comments_top_n',10)} 条已写入展示）")
            except Exception:
                pass
            # 同步启动段评媒体（图片+头像）预取：与保存同时进行
            if getattr(self.config, "enable_segment_comments", False):
                try:
                    top_n = int(getattr(self.config, "segment_comments_top_n", 10))
                except Exception:
                    top_n = 10
                # 仅当配置允许下载评论图片或头像（头像始终会被尝试）才启动
                allow_images = bool(getattr(self.config, "download_comment_images", True))
                if allow_images or True:  # 头像始终处理
                    if self._media_prefetch_executor is not None:
                        try:
                            self._media_prefetch_executor.submit(self._prefetch_media, payload, top_n)
                        except Exception:
                            # 回退直接调用（阻塞当前线程）
                            self._prefetch_media(payload, top_n)
                    else:
                        # 无执行器（极端情况）直接调用
                        self._prefetch_media(payload, top_n)
        except Exception as e:
            self.logger.debug(f"段评保存失败: {e}")

    def save_error_chapter(self, chapter_id, title):
        """保存下载错误章节"""
        self.downloaded[chapter_id] = [title, "Error"]
        self.save_download_status()
        self.logger.debug(f"章节 {chapter_id} 下载错误记录缓存成功")

    def finalize_spawn(self, chapters, result):
        """生成最终文件"""
        if not self.config.bulk_files:
            # 等待所有异步图片预取任务完成，保证生成 EPUB 时图片已落地
            try:
                if hasattr(self, "_media_prefetch_executor") and self._media_prefetch_executor:
                    self._media_prefetch_executor.shutdown(wait=True)
            except Exception:
                pass
            output_file = self.save_dir / f"{self.book_name}.{self.config.novel_format}"
            if output_file.exists():
                os.remove(output_file)
            if self.config.novel_format == "epub":
                # 生成EPUB骨架
                epub = EpubGenerator(
                    self.book_id,
                    self.book_name,
                    "zh-CN",
                    self.author,
                    self.description,
                    "番茄小说",
                )

                epub.add_chapter(
                    "书籍简介",
                    f"<h1>书籍简介</h1><p><small>{self.tags}</small></p><p>{re.sub(r'\n+', '</p><p>', self.description)}</p>",
                )

                for chapter in chapters:
                    chapter_id = chapter["id"]
                    title = self.downloaded.get(chapter_id, [chapter["title"], None])[0]
                    content = self.downloaded.get(
                        chapter_id,
                        [None, "<p>Download Faild or Didn't Download Finish!</p>"],
                    )[1]

                    # 若启用段评功能，尝试为该章节生成段评页面，并在章节末尾加入链接
                    seg_link = ""
                    # 为章节建立稳定文件名，便于段评页面回链
                    chapter_file = f"chapter_{chapter_id}.xhtml"
                    if getattr(self.config, "enable_segment_comments", False):
                        seg_data = self._load_segment_comments_json(chapter_id)
                        if seg_data is not None:
                            # 若未在下载阶段预取媒体，则此处兜底预取一次
                            if not self._media_prefetched:
                                try:
                                    try:
                                        top_n = int(getattr(self.config, "segment_comments_top_n", 10))
                                    except Exception:
                                        top_n = 10
                                    self._prefetch_media(seg_data, top_n)
                                except Exception:
                                    pass
                            comments_file = f"comments_{chapter_id}.xhtml"
                            comments_title = f"{title} - 段评"
                            # 在修改正文之前，保留一份原始 HTML 供段标题提取首句
                            _orig_html_for_snippet = content if isinstance(content, str) else ""
                            comments_content = self._render_segment_comments_xhtml(
                                title,
                                chapter_id,
                                seg_data,
                                back_to_chapter=chapter_file,
                                chapter_html=_orig_html_for_snippet,
                            )
                            try:
                                # 生成辅助页面（不进 spine）
                                epub.add_aux_page(comments_title, comments_content, comments_file)
                                # 统计有评论的段数量，用于链接提示
                                paras = seg_data.get("paras") if isinstance(seg_data, dict) else None
                                seg_para_count = 0
                                seg_counts = {}
                                if isinstance(paras, dict):
                                    for _k, _v in paras.items():
                                        try:
                                            c = int((_v or {}).get("count", 0))
                                        except Exception:
                                            c = 0
                                        if c > 0:
                                            seg_counts[str(_k)] = c
                                            seg_para_count += 1
                                hint = f"（{seg_para_count}段有评论）" if seg_para_count > 0 else ""
                                seg_link = f"\n<p class=\"segment-comments-link\"><a href=\"{comments_file}\">查看本章段评{hint}</a></p>"
                                # 将正文中“有评论的段落”转换为可点击区域，点击跳转到对应段评
                                if isinstance(content, str) and isinstance(paras, dict) and seg_para_count > 0:
                                    try:
                                        # 将计数字典传入，便于在段尾追加灰色小数字
                                        content = self._inject_segment_links(content, comments_file, seg_counts)
                                    except Exception:
                                        pass
                            except Exception as e:
                                # 段评失败不影响主流程
                                self.logger.debug(f"段评页面生成失败: {e}")

                    # 章节加入可点击的段评入口
                    content_with_link = content + seg_link if isinstance(content, str) else content

                    epub.add_chapter(
                        title,
                        content_with_link,
                        file_name=chapter_file,
                    )
                epub.generate(output_file)
                self.logger.info(
                    f"EPUB生成完成: {self.save_dir / f'{self.book_name}.epub'}"
                )
            else:
                with output_file.open("w", encoding="utf-8") as f:
                    f.write(
                        f"书名: {self.book_name}\n作者: {self.author}\n标签: {self.tags}\n简介: {self.description}\n\n"
                    )
                    for chapter in chapters:
                        chapter_id = chapter["id"]
                        title = self.downloaded.get(chapter_id, [chapter["title"], None])[0]
                        content = self.downloaded.get(
                            chapter_id,
                            [None, "Download Faild or Didn't Download Finish!"],
                        )[1]
                        f.write(f"\n\n{title}\n{content}")
                self.logger.info(f"TXT生成完成: {output_file}")
        if result == 0 and self.config.auto_clear_dump and self.end:
            cover_path = self.status_folder / f"{self.book_name}.jpg"
            if self.status_file.exists():
                os.remove(self.status_file)
                self.logger.debug(f"断点缓存文件已清理！{self.status_file}")
            if cover_path.exists():
                os.remove(cover_path)
                self.logger.debug(f"封面文件已清理！{cover_path}")
            FileCleaner.clean_dump_folder(self.config.get_status_folder_path)

    def save_download_status(self):
        """保存完整下载状态"""
        if self.downloaded:
            data = {
                "book_name": self.book_name,
                "author": self.author,
                "tags": self.tags,
                "description": self.description,
                "downloaded": self.downloaded,
            }
            try:
                with self.status_file.open("w", encoding="utf-8") as f:
                    json.dump(data, f, ensure_ascii=False, indent=2)
            except Exception as e:
                self.logger.warning(f"状态文件保存失败或无需保存: {e}")

    # ================= 段评 → XHTML 渲染 =================
    def _load_segment_comments_json(self, chapter_id: str):
        """读取某章段评 JSON，存在则返回字典，不存在或异常返回 None。"""
        try:
            seg_path = self.status_folder / "segment_comments" / f"{chapter_id}.json"
            if not seg_path.exists():
                return None
            with seg_path.open("r", encoding="utf-8") as f:
                return json.load(f)
        except Exception as e:
            self.logger.debug(f"段评JSON读取失败: {e}")
            return None

    def _safe_get(self, obj, keys, default=""):
        """从 obj 中按 keys 顺序取第一个非空字段，keys 可为 ['a','b','c'] 或嵌套 'user.nick'。"""
        for k in keys:
            try:
                cur = obj
                for part in k.split('.'):
                    if isinstance(cur, dict):
                        cur = cur.get(part)
                    else:
                        cur = None
                        break
                if cur not in (None, ""):
                    return cur
            except Exception:
                continue
        return default

    def _deep_find_str(self, obj, candidate_keys=("text", "message", "content", "title")):
        """在嵌套 dict/list 中寻找第一个非空字符串，优先匹配指定 key 名。"""
        try:
            # 命中优先 key
            if isinstance(obj, dict):
                for k in candidate_keys:
                    v = obj.get(k)
                    if isinstance(v, str) and v.strip():
                        return v
                # 深搜
                for v in obj.values():
                    r = self._deep_find_str(v, candidate_keys)
                    if isinstance(r, str) and r.strip():
                        return r
            elif isinstance(obj, list):
                for v in obj:
                    r = self._deep_find_str(v, candidate_keys)
                    if isinstance(r, str) and r.strip():
                        return r
        except Exception:
            pass
        return ""

    def _deep_find_int(self, obj, candidate_keys=("digg_count", "like_count", "praise_count", "likes")):
        """在嵌套结构中寻找第一个可转为 int 的数值，优先匹配指定 key 名。"""
        try:
            if isinstance(obj, dict):
                for k in candidate_keys:
                    if k in obj:
                        try:
                            return int(obj[k])
                        except Exception:
                            pass
                for v in obj.values():
                    r = self._deep_find_int(v, candidate_keys)
                    if isinstance(r, int) and r >= 0:
                        return r
            elif isinstance(obj, list):
                for v in obj:
                    r = self._deep_find_int(v, candidate_keys)
                    if isinstance(r, int) and r >= 0:
                        return r
        except Exception:
            pass
        return 0

    # ===== 表情与图片处理 =====
    _EMOJI_MAP = {
        "奸笑": "🤪",
        "你细品": "🍵",
        "微笑": "🙂",
        "笑哭": "😂",
        "大笑": "😄",
        "偷笑": "🤭",
        "苦笑": "😅",
        "大哭": "😭",
        "哭": "😢",
        "再见": "👋",
        "害羞": "😊",
        "OK": "👌",
        "OK手势": "👌",
        "OK啦": "👌",
        "赞": "👍",
        "鼓掌": "👏",
        "握手": "🤝",
        "强": "💪",
        "酷": "😎",
        "色": "😘",
        "亲亲": "😘",
        "生气": "😠",
        "发怒": "😡",
        "惊讶": "😮",
        "吐舌": "😛",
        "捂脸": "🤦",
        "思考": "🤔",
        "睡": "😴",
        "疑问": "❓",
        "心": "❤️",
        "心碎": "💔",
    }

    def _convert_bracket_emojis(self, text: str) -> str:
        """将形如 [偷笑] 的表情代码替换为 emoji。"""
        if not isinstance(text, str) or "[" not in text:
            return text
        def _repl(m):
            key = m.group(1).strip()
            return self._EMOJI_MAP.get(key, m.group(0))
        try:
            return re.sub(r"\[([^\[\]]+)\]", _repl, text)
        except Exception:
            return text

    def _extract_image_urls(self, obj) -> List[str]:
        """仅从段评内容的 content.image_data_list.image_data[*] 提取图片 URL，避免抓取头像/封面等无关链接。"""
        urls: List[str] = []

        def _add_candidate(s: str | None):
            if isinstance(s, str) and s.startswith("http"):
                urls.append(s)

        try:
            if not isinstance(obj, dict):
                return []

            # 优先从标准路径提取：comment.common.content.image_data_list
            content = (
                ((obj.get("comment") or {}).get("common") or {}).get("content")
                if isinstance(obj.get("comment"), dict)
                else None
            )
            if not isinstance(content, dict):
                # 兼容位置：common.content / content
                content = ((obj.get("common") or {}).get("content")) if isinstance(obj.get("common"), dict) else obj.get("content")

            if isinstance(content, dict):
                idl = content.get("image_data_list")
                if isinstance(idl, dict):
                    items = idl.get("image_data")
                    if isinstance(items, list):
                        for it in items:
                            if isinstance(it, dict):
                                _add_candidate(
                                    it.get("expand_web_url")
                                    or it.get("web_uri")
                                    or it.get("url")
                                    or it.get("src")
                                )

            # 去重
            seen = set()
            dedup = []
            for u in urls:
                if u not in seen:
                    seen.add(u)
                    dedup.append(u)
            return dedup
        except Exception:
            return []

    def _extract_avatar_url(self, item) -> str | None:
        """从评论对象中提取用户头像 URL（若存在）。"""
        try:
            url = self._safe_get(
                item,
                [
                    "comment.common.user_info.base_info.user_avatar",
                    "common.user_info.base_info.user_avatar",
                    "user_info.base_info.user_avatar",
                    "comment.user_info.base_info.user_avatar",
                    "user.avatar",
                    "avatar_url",
                    "avatar",
                ],
                "",
            )
            if isinstance(url, str) and url.startswith("http"):
                return url
        except Exception:
            pass
        return None

    def _prefetch_media(self, seg_data: dict, top_n: int = 10) -> None:
        """并发预取段评中的图片与头像，仅处理每段前 top_n 条评论。"""
        try:
            # 若配置不允许下载评论图片，仅预取头像
            allow_images = bool(getattr(self.config, "download_comment_images", True))
            paras = seg_data.get("paras") if isinstance(seg_data, dict) else None
            if not isinstance(paras, dict):
                return
            urls = []
            img_cnt = 0
            avatar_cnt = 0
            for _k, _meta in paras.items():
                detail = (_meta or {}).get("detail") or {}
                lst = detail.get("data_list") if isinstance(detail, dict) else None
                if not isinstance(lst, list) or not lst:
                    continue
                for item in lst[: max(0, int(top_n))]:
                    # 评论图片（受开关控制）
                    if allow_images:
                        for u in self._extract_image_urls(item):
                            urls.append(u)
                            img_cnt += 1
                    # 头像
                    av = self._extract_avatar_url(item)
                    if av:
                        urls.append(av)
                        avatar_cnt += 1
            # 去重
            unique = []
            seen = set()
            for u in urls:
                if u not in seen:
                    seen.add(u)
                    unique.append(u)
            if not unique:
                # 提示没有可下载媒体，便于用户判断为何进度条不动
                try:
                    self.logger.debug(
                        f"[媒体] 章节 {seg_data.get('chapter_id')} 无可下载资源 (图片={img_cnt}, 头像={avatar_cnt}, allow_images={allow_images})"
                    )
                except Exception:
                    pass
                return
            # 并发下载
            try:
                workers = int(getattr(self.config, "media_download_workers", 8))
            except Exception:
                workers = 4
            # 初始化 / 增量更新 媒体进度条 total（若注入）
            # 已移除 UI 进度条，无需 total 更新
            self._media_prefetched = True
            try:
                self.logger.debug(
                    f"[媒体] 章节 {seg_data.get('chapter_id')} 收集图片={img_cnt} 头像={avatar_cnt} 去重后={len(unique)}"
                )
            except Exception:
                pass
            with ThreadPoolExecutor(max_workers=max(1, workers)) as ex:
                futures = [ex.submit(self._download_comment_image, u) for u in unique]
                for f in as_completed(futures):
                    try:
                        _ = f.result()
                    except Exception:
                        pass
                    # 静默，不再更新 UI
        except Exception:
            pass

    def _download_comment_image(self, url: str) -> str | None:
        """下载图片到状态目录 images 下，返回文件名（不含路径），失败返回 None。包含超时和重试。"""
        try:
            # 屏蔽域名
            try:
                blocked = list(getattr(self.config, "blocked_media_domains", []))
            except Exception:
                blocked = []
            if any(b and (b in url) for b in blocked):
                self.logger.debug(f"跳过被屏蔽域名的图片: {url}")
                return None
            img_dir: Path = self.status_folder / "images"
            img_dir.mkdir(parents=True, exist_ok=True)
            parsed = urlparse(url)
            path = parsed.path or ""
            ext = os.path.splitext(path)[1].lower()
            if ext not in [".jpg", ".jpeg", ".png", ".gif", ".webp"]:
                ext = ""
            name = hashlib.sha1(url.encode("utf-8")).hexdigest()
            if not ext:
                ext = ".jpg"
            file_name = f"{name}{ext}"
            out_path = img_dir / file_name
            if out_path.exists():
                return file_name

            accept_hdr = "image/jpeg,image/jpg,image/png,image/gif,*/*;q=0.8"
            headers = {
                "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
                "Accept": accept_hdr,
                "Referer": f"{parsed.scheme}://{parsed.netloc}/",
            }

            # 配置化超时与重试
            try:
                timeout = float(getattr(self.config, "media_download_timeout", 5.0))
            except Exception:
                timeout = 5.0
            try:
                max_retries = int(getattr(self.config, "media_download_retries", 4))
            except Exception:
                max_retries = 4
            try:
                backoff = float(getattr(self.config, "media_retry_backoff", 0.8))
            except Exception:
                backoff = 0.8

            import time as _t
            attempt = 0
            last_err = None
            while attempt < max_retries:
                try:
                    self.logger.debug(f"下载评论图片: {url} (尝试{attempt+1}/{max_retries})")
                    resp = requests.get(url, headers=headers, timeout=timeout)
                    sc = resp.status_code
                    if sc == 200 and resp.content:
                        # ---- 内容类型与扩展处理 ----
                        ctype = resp.headers.get("Content-Type", "").lower()
                        url_lower = url.lower()
                        # URL 迹象（更宽松）
                        heic_url_markers = [".heic", "/heic", "format=heic", "heic=1", "image/heic", "x-oss-process=image/format,heic"]
                        is_heic_hint = any(m in url_lower for m in heic_url_markers) or ("heic" in ctype or "heif" in ctype)
                        # Magic 检测：在前 512 字节 搜索 ftyp + 品牌
                        if not is_heic_hint:
                            try:
                                head_bytes = resp.content[:512]
                                pos = head_bytes.find(b"ftyp")
                                if pos != -1:
                                    brand_window = head_bytes[pos+4:pos+16]
                                    if any(b in brand_window for b in [b"heic", b"heif", b"mif1", b"msf1"]):
                                        is_heic_hint = True
                            except Exception:
                                pass
                        debug_formats = getattr(self.config, "log_image_format_debug", False)
                        if is_heic_hint and debug_formats:
                            self.logger.debug(f"检测到 HEIC 格式: url={url} ctype={ctype}")
                        if ext == ".jpg" and "png" in ctype:
                            file_name2 = f"{name}.png"; out_path2 = img_dir / file_name2
                        elif ext == ".jpg" and "gif" in ctype:
                            file_name2 = f"{name}.gif"; out_path2 = img_dir / file_name2
                        elif ext == ".jpg" and "webp" in ctype:
                            file_name2 = f"{name}.webp"; out_path2 = img_dir / file_name2
                        elif ext == ".jpg" and "avif" in ctype:
                            file_name2 = f"{name}.avif"; out_path2 = img_dir / file_name2
                        elif is_heic_hint:
                            file_name2 = f"{name}.heic"; out_path2 = img_dir / file_name2
                        else:
                            file_name2 = file_name; out_path2 = out_path
                        data_bytes = resp.content
                        # 强制统一转成 JPEG（新配置）或旧 webp 配置兼容映射
                        try:
                            force_jpeg = bool(getattr(self.config, "force_convert_images_to_jpeg", False)) or bool(getattr(self.config, "force_convert_images_to_webp", False))
                            need_heic_convert = is_heic_hint and getattr(self.config, "convert_heic_to_jpeg", True)
                            if force_jpeg or need_heic_convert:
                                from io import BytesIO
                                buf_in = BytesIO(data_bytes)
                                converted = False
                                # 注册 heic 解码
                                try:
                                    import pillow_heif  # type: ignore
                                    pillow_heif.register_heif_opener()
                                except Exception:
                                    pass
                                try:
                                    from PIL import Image
                                    with Image.open(buf_in) as im:
                                        im = im.convert("RGB")
                                        qj = int(getattr(self.config, "jpeg_quality", 90))
                                        buf_out = BytesIO()
                                        im.save(buf_out, format="JPEG", quality=max(1, min(100, qj)))
                                        data_bytes = buf_out.getvalue()
                                        file_name2 = f"{name}.jpg"; out_path2 = img_dir / file_name2
                                        converted = True
                                except Exception:
                                    converted = False
                                if is_heic_hint and not converted:
                                    if not getattr(self.config, "keep_heic_original", False):
                                        if debug_formats:
                                            self.logger.debug(f"HEIC 转码失败已丢弃: url={url}")
                                        return None
                                    else:
                                        if not file_name2.endswith(".heic"):
                                            file_name2 = f"{name}.heic"; out_path2 = img_dir / file_name2
                            elif getattr(self.config, "jpeg_retry_convert", True) and not ("jpeg" in ctype or file_name2.endswith('.jpg') or file_name2.endswith('.jpeg')):
                                from io import BytesIO
                                from PIL import Image
                                buf_in = BytesIO(data_bytes)
                                try:
                                    with Image.open(buf_in) as im:
                                        im = im.convert("RGB")
                                        qj = int(getattr(self.config, "jpeg_quality", 90))
                                        buf_out = BytesIO()
                                        im.save(buf_out, format="JPEG", quality=max(1, min(100, qj)))
                                        data_bytes = buf_out.getvalue()
                                        file_name2 = f"{name}.jpg"; out_path2 = img_dir / file_name2
                                except Exception:
                                    pass
                        except Exception:
                            pass
                        with open(out_path2, "wb") as f:
                            f.write(data_bytes)
                        return file_name2
                    # 可重试状态码
                    if sc in (429, 500, 502, 503, 504):
                        last_err = RuntimeError(f"status={sc}")
                    else:
                        self.logger.debug(f"图片下载失败 status={sc} url={url}")
                        return None
                except requests.Timeout as e:
                    last_err = e
                except Exception as e:
                    last_err = e
                attempt += 1
                _t.sleep(backoff * attempt)
            if last_err:
                try:
                    self.logger.debug(f"图片下载失败(重试耗尽): {last_err}")
                except Exception:
                    pass
            return None
        except Exception as e:
            try:
                self.logger.debug(f"图片下载失败: {e}")
            except Exception:
                pass
            return None

    def _find_probable_author(self, obj) -> str:
        """启发式在嵌套结构中寻找最可能的用户名/作者名。"""
        try:
            from collections import deque
            q = deque([obj])
            while q:
                x = q.popleft()
                if isinstance(x, dict):
                    for k, v in x.items():
                        if isinstance(v, str) and v.strip():
                            kl = str(k).lower()
                            if (
                                "user" in kl
                                or "author" in kl
                                or "nick" in kl
                                or kl.endswith("name")
                                or kl in {"name", "uname", "screen_name", "nickname", "nick_name", "user_name"}
                            ):
                                s = v.strip()
                                if 1 <= len(s) <= 32:
                                    return s
                        if isinstance(v, (dict, list)):
                            q.append(v)
                elif isinstance(x, list):
                    for v in x:
                        if isinstance(v, (dict, list)):
                            q.append(v)
            return ""
        except Exception:
            return ""

    def _to_cjk_numeral(self, n: int) -> str:
        """将 1..99 转换为中文数字（简易，满足段序号场景）。"""
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
            shi = n // 10
            ge = n % 10
            return digits[shi] + "十" + (digits[ge] if ge else "")
        return str(n)

    def _extract_para_snippet(self, chapter_html: str, target_idx: int) -> str:
        """从章节 HTML 按 <p> 顺序提取第 target_idx 段的首句；失败返回空串。"""
        if not isinstance(chapter_html, str) or target_idx < 0:
            return ""
        try:
            pattern = re.compile(r"(<p\b[^>]*>)(.*?)(</p>)", re.I | re.S)
            idx = 0
            for m in pattern.finditer(chapter_html):
                if idx == target_idx:
                    inner = m.group(2)
                    inner_text = re.sub(r"<[^>]+>", "", inner)
                    inner_text = html.unescape(inner_text).strip()
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

    def _render_segment_comments_xhtml(self, chapter_title: str, chapter_id: str, data: dict, back_to_chapter: str | None = None, chapter_html: str | None = None) -> str:
        """将段评 JSON 渲染为一个简单可读、符合 EPUB 的 HTML 片段。"""
        # 头部
        parts: List[str] = []
        parts.append(f"<h2>{html.escape(chapter_title)} - 段评</h2>")
        paras = data.get("paras") if isinstance(data, dict) else None
        if not isinstance(paras, dict) or not paras:
            parts.append("<p>暂无段评数据。</p>")
            return "\n".join(parts)

    # 准备提取段落首句的工具

        # 按段索引排序输出
        for key in sorted(paras.keys(), key=lambda x: int(x) if str(x).isdigit() else 0):
            meta = paras.get(key) or {}
            try:
                cnt = int(meta.get("count", 0))
            except Exception:
                cnt = 0
            if cnt <= 0:
                # 该段无评论
                continue
            # 展示层将 0 基索引改为 1 基人类可读
            try:
                disp_idx = int(key) + 1
            except Exception:
                disp_idx = key
            # 构造标题：一、"段落首句…" (cnt)
            try:
                idx_int = int(key)
            except Exception:
                idx_int = -1
            snippet = self._extract_para_snippet(chapter_html or "", idx_int) if chapter_html else ""
            cjk_idx = self._to_cjk_numeral(int(disp_idx) if str(disp_idx).isdigit() else 0)
            if snippet:
                title_html = (
                    f"<span class=\"para-title\"><span class=\"para-index\">{html.escape(cjk_idx)}、</span> "
                    f"<span class=\"para-src\">&quot;{html.escape(snippet)}&quot;</span> <small>({cnt})</small></span>"
                )
            else:
                # 回退：保留原来的“第 N 段 (cnt)”
                title_html = f"<span class=\"para-title\">第 {html.escape(str(disp_idx))} 段 <small>({cnt})</small></span>"
            parts.append(f"<h3 id=\"para-{html.escape(str(key))}\">{title_html}</h3>")
            if back_to_chapter:
                parts.append(f"<p class=\"back-to-chapter\"><a href=\"{html.escape(back_to_chapter)}#p-{html.escape(str(key))}\">返回本章第 {html.escape(str(disp_idx))} 段</a></p>")
            detail = meta.get("detail") or {}
            lst = detail.get("data_list") if isinstance(detail, dict) else None
            if not isinstance(lst, list) or not lst:
                parts.append("<p>该段暂无可展示的评论。</p>")
                continue
            # 限制仅展示每段前 N 条
            try:
                top_n = int(getattr(self.config, "segment_comments_top_n", 10))
            except Exception:
                top_n = 10
            show_list = lst[: max(0, int(top_n))]
            parts.append("<ol>")
            for item in show_list:
                # 取评论文本（覆盖常见嵌套路径）
                text = self._safe_get(
                    item,
                    [
                        "common.content.text",
                        "content.text",
                        "common.comment.content.text",
                        "comment.content.text",
                        # 次级：可能拿到一个 content dict，继续取其中的 text
                        "common.content",
                        "comment.content",
                        "text",
                        "msg",
                        "message",
                    ],
                    "",
                )
                # 如果拿到的是 dict，尽量向内取 text；仍非字符串再序列化兜底
                if isinstance(text, dict):
                    inner_text = self._safe_get(text, ["text", "message"], "")
                    text = inner_text if isinstance(inner_text, str) and inner_text else text
                # 结构仍未取到有效文本时，做一次深度回退
                if not isinstance(text, str) or not text.strip():
                    text = self._deep_find_str(item)
                if isinstance(text, (dict, list)):
                    text = json.dumps(text, ensure_ascii=False)
                # 表情替换（先替换再转义，保留 emoji）
                text = self._convert_bracket_emojis(str(text))
                text = html.escape(text)
                # 提取并下载图片
                img_urls = self._extract_image_urls(item)
                img_tags = []
                for u in img_urls[:6]:  # 每条评论最多插入 6 张以防过多
                    fn = self._download_comment_image(u)
                    if fn:
                        img_tags.append(f'<img src="images/{html.escape(fn)}" alt="img" />')
                # 取作者：覆盖常见路径 + 更稳健回退
                author = self._safe_get(
                    item,
                    [
                        # 常见
                        "common.user_info.base_info.user_name",
                        "user_info.base_info.user_name",
                        "common.user_info.base_info.nickname",
                        "user_info.base_info.nickname",
                        "common.user.nick_name",
                        "common.user.nickname",
                        "user.nick_name",
                        "user.nickname",
                        "user.name",
                        # 变体
                        "user_info.user_name",
                        "user_info.nickname",
                        "common.user_info.user_name",
                        "common.user_info.nickname",
                        "common.user_name",
                        "screen_name",
                        "uname",
                        "nick",
                        # 通用
                        "author",
                        "nickname",
                        "user_name",
                        "name",
                    ],
                    "",
                )
                if not isinstance(author, str) or not author.strip():
                    author = self._deep_find_str(item, ("user_name", "nickname", "nick_name", "name", "screen_name", "uname", "nick"))
                if not isinstance(author, str) or not author.strip():
                    author = self._find_probable_author(item)
                if not isinstance(author, str) or not author.strip():
                    author = "匿名"
                author = html.escape(str(author))
                # 点赞/热度（覆盖常见嵌套路径，优先 comment.stat.digg_count）
                like = self._safe_get(
                    item,
                    [
                        # 正确所在位置
                        "comment.stat.digg_count",
                        "comment.stat.like_count",
                        "comment.stat.praise_count",
                        # 其它可能位置（兼容历史/变体）
                        "stat.digg_count",
                        "common.digg_count",
                        "digg_count",
                        "like_count",
                        "praise_count",
                        "likes",
                    ],
                    0,
                )
                try:
                    like = int(like)
                except Exception:
                    like = 0
                if like == 0:
                    # 仅在 comment 子对象内做回退搜索，避免拿到无关的 0 值
                    sub = item.get("comment") if isinstance(item, dict) else None
                    like = self._deep_find_int(sub if isinstance(sub, (dict, list)) else item)
                # 时间（若有）
                ts = self._safe_get(
                    item,
                    [
                        "common.create_timestamp",
                        "create_timestamp",
                        "create_time",
                        "ctime",
                        "time",
                    ],
                    "",
                )
                # 尝试将时间戳格式化成人类可读
                try:
                    import time as _t
                    if isinstance(ts, (int, float)):
                        # 绝大多数为秒级时间戳
                        if ts > 1e12:
                            # 毫秒
                            ts = int(ts / 1000)
                        else:
                            ts = int(ts)
                        ts = _t.strftime("%Y-%m-%d %H:%M", _t.localtime(ts))
                except Exception:
                    pass
                ts = html.escape(str(ts)) if ts else ""
                # 头像
                avatar_url = self._extract_avatar_url(item)
                avatar_img = ""
                if avatar_url:
                    fn_av = self._download_comment_image(avatar_url)
                    if fn_av:
                        avatar_img = f'<img class="avatar" src="images/{html.escape(fn_av)}" alt="avatar" /> '

                meta_line = f"<small class=\"seg-meta\">{avatar_img}作者：{author}"
                if ts:
                    meta_line += f" | 时间：{ts}"
                meta_line += f" | 赞：{like}</small>"
                if img_tags:
                    parts.append(f"<li class=\"seg-item\"><p>{text}</p><div class=\"seg-images\">{''.join(img_tags)}</div><p>{meta_line}</p></li>")
                else:
                    parts.append(f"<li class=\"seg-item\"><p>{text}</p><p>{meta_line}</p></li>")
            parts.append("</ol>")

        # 简单结尾
        try:
            n = int(getattr(self.config, "segment_comments_top_n", 10))
        except Exception:
            n = 10
        parts.append(f"<p><small>仅展示每段前 {n} 条评论（若有），实际总数以接口为准。</small></p>")
        return "\n".join(parts)

    def _inject_segment_links(self, content_html: str, comments_file: str, seg_counts: dict) -> str:
        """
        将正文中“有评论的段落”在段尾追加一个灰色小数字（评论数），点击跳转至对应段评锚点；同时为这些段落加上 id="p-<idx>"
        说明：
        - 逐个匹配 <p>…</p>，按出现顺序作为段索引 0,1,2,...
        - 若该索引在 seg_counts 内且 >0，则在段尾追加 <a class="seg-count">(N)</a>
          并为 <p> 增加 id="p-idx"（若原本无 id）。
        - 简化实现，未拆分为句级链接；保持正文颜色不变。
        """
        try:
            # 注意：这里使用 \b 是正则“单词边界”，raw-string 下无需再双反斜杠
            pattern = re.compile(r"(<p\b[^>]*>)(.*?)(</p>)", re.I | re.S)
            idx = 0
            out = []
            last = 0
            for m in pattern.finditer(content_html):
                out.append(content_html[last:m.start()])
                open_tag, inner, close_tag = m.group(1), m.group(2), m.group(3)
                # 仅对有评论的段落处理
                cnt = 0
                try:
                    cnt = int(seg_counts.get(str(idx), 0))
                except Exception:
                    cnt = 0
                if cnt > 0:
                    # 若无 id，追加 id="p-idx"
                    if not re.search(r"\bid\s*=", open_tag, re.I):
                        open_tag = open_tag[:-1] + f' id="p-{idx}">'
                    # 在段尾追加灰色可点击数字
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
