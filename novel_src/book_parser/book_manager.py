import json, threading
from pathlib import Path
from typing import List


from ..base_system.context import GlobalContext
from . import finalize_utils
from ..network_parser.media_downloader import MediaDownloader


class BookManager:
    def __init__(self, config: GlobalContext, logger):
        self.config = config
        self.logger = logger
        # 基本元数据（稍后在 main 中补全）
        self.book_name = ""
        self.book_id = ""
        self.author = ""
        self.tags = ""
        self.description = ""
        self.end = False
        self.downloaded = {}
        self._has_download_activity = False
        self._cleanup_pending = False
        # 状态目录：优先使用 Config 的 get_status_folder_path（属性），否则回退到默认
        try:
            existing_folder = getattr(self.config, "get_status_folder_path", None)
            # 兼容历史：若异常定义为可调用，则调用一次
            if callable(existing_folder):
                existing_folder = existing_folder()
        except Exception:
            existing_folder = None
        if not existing_folder:
            try:
                existing_folder = Path(self.config.default_save_dir) / "_session"
            except Exception:
                existing_folder = Path("._session")
        self.status_folder = Path(existing_folder)
        self.status_folder.mkdir(parents=True, exist_ok=True)
        self.status_file = self.status_folder / "status.json"
        try:
            self.config.mark_status_folder_claimed(self.status_folder)
        except Exception:
            try:
                self.config._last_status_folder_claimed = True
            except Exception:
                pass
        try:
            self._status_folder_preexisting = not bool(
                self.config.status_folder_was_created_this_session(self.status_folder)
            )
        except Exception:
            self._status_folder_preexisting = True
        # 媒体进度结构
        self._media_progress_lock = threading.Lock()
        self._media_progress_done = set()
        self._media_prefetched = False
        self._media_downloader = None

    # -------- 断点续传：加载既有状态 --------
    def load_existing_status(self, book_id: str, book_name: str):
        """尝试加载已存在的下载状态（支持新版 status.json 与旧版 chapter_status_{id}.json）。

        逻辑:
          1. 优先读取 self.status_file (status.json)
          2. 若不存在则尝试旧文件 chapter_status_{book_id}.json
          3. downloaded 字段兼容：
             - 新格式: { downloaded: {chapter_id: [title, content_or_None]} }
             - 旧格式: { chapter_id: [title, content_or_None] }
          4. 填充元数据（若文件中存在且当前未设置）
        """
        try:
            # 防御：确保状态目录正确（可能用户直接传不同 book_name/book_id）
            try:
                if not self.status_folder.exists():
                    self.status_folder.mkdir(parents=True, exist_ok=True)
            except Exception:
                pass
            data = None
            if self.status_file.exists():
                try:
                    with self.status_file.open("r", encoding="utf-8") as f:
                        data = json.load(f)
                except Exception:
                    data = None
            if data is None:
                legacy_path = self.status_folder / f"chapter_status_{book_id}.json"
                if legacy_path.exists():
                    try:
                        with legacy_path.open("r", encoding="utf-8") as f:
                            legacy_data = json.load(f)
                        # 旧文件可能无 downloaded 包裹
                        if isinstance(legacy_data, dict) and "downloaded" not in legacy_data:
                            data = {"downloaded": legacy_data}
                        else:
                            data = legacy_data
                    except Exception:
                        data = None
            if not data:
                return False
            dl = data.get("downloaded") if isinstance(data, dict) else None
            if isinstance(dl, dict):
                self.downloaded.update(dl)
            # 元数据填充（若为空）
            self.book_id = self.book_id or str(data.get("book_id", book_id))
            self.book_name = self.book_name or str(data.get("book_name", book_name))
            self.author = self.author or str(data.get("author", ""))
            self.tags = self.tags or str(data.get("tags", ""))
            try:
                self.end = bool(data.get("end", self.end))
            except Exception:
                pass
            self.description = self.description or str(data.get("description", ""))
            try:
                self.logger.info(
                    f"[断点续传] 已加载历史进度: chapters={len(self.downloaded)}"
                )
            except Exception:
                pass
            return True
        except Exception as e:
            try:
                self.logger.debug(f"加载历史进度失败: {e}")
            except Exception:
                pass
            return False

    # -------- finalize 输出（委托 finalize_utils） --------
    def finalize(self, chapters: List[dict], result: int = 0):
        cleanup_deferred = False
        tts_completed = True
        try:
            cleanup_deferred = finalize_utils.run_finalize(self, chapters, result)
            self._cleanup_pending = cleanup_deferred
        except Exception as e:
            self.logger.error(f"finalize 生成失败: {e}")
        try:
            from .audio_generator import generate_audiobook

            result_flag = generate_audiobook(self, chapters)
            if result_flag is False:
                tts_completed = False
        except Exception as e:
            tts_completed = False
            try:
                self.logger.error(f"有声小说生成失败: {e}")
            except Exception:
                pass
        finally:
            if cleanup_deferred and tts_completed:
                try:
                    finalize_utils.perform_deferred_cleanup(self)
                    self._cleanup_pending = False
                except Exception as e:
                    try:
                        self.logger.error(f"自动清理失败: {e}")
                    except Exception:
                        pass
            elif cleanup_deferred and not tts_completed:
                try:
                    self.logger.info("已保留缓存文件，可在修复问题后继续生成有声小说。")
                except Exception:
                    pass

    # -------- 兼容旧 downloader 接口：保存章节与进度 --------
    def save_chapter(self, chapter_id: str, title: str, content: str):
        """保存单章正文。content 为已清洗 HTML/文本。"""
        try:
            self.downloaded[chapter_id] = [title, content]
            self._has_download_activity = True
        except Exception:
            pass

    def save_error_chapter(self, chapter_id: str, title: str):
        """记录失败章节，占位避免重复下载。"""
        try:
            self.downloaded[chapter_id] = [title, None]
            self._has_download_activity = True
        except Exception:
            pass

    def save_download_status(self):
        """持久化当前下载状态，供断点续传使用。结构兼容旧版本。"""
        try:
            data = {
                "book_id": self.book_id,
                "book_name": self.book_name,
                "author": self.author,
                "tags": self.tags,
                "description": self.description,
                "end": self.end,
                "downloaded": self.downloaded,
            }
            self.status_folder.mkdir(parents=True, exist_ok=True)
            with self.status_file.open("w", encoding="utf-8") as f:
                json.dump(data, f, ensure_ascii=False, indent=2)
        except Exception as e:
            try:
                self.logger.debug(f"保存下载状态失败: {e}")
            except Exception:
                pass

    # -------- 段评保存（供 downloader 调用） --------
    def save_segment_comments(self, chapter_id: str, data: dict):
        """保存某章节的段评 JSON，并触发媒体预取与进度标记。"""
        try:
            if not getattr(self.config, "enable_segment_comments", False):
                return
            seg_dir = self.status_folder / "segment_comments"
            seg_dir.mkdir(parents=True, exist_ok=True)
            out_file = seg_dir / f"{chapter_id}.json"
            with out_file.open("w", encoding="utf-8") as f:
                json.dump(data, f, ensure_ascii=False)
            # 日志统计
            try:
                paras = (data or {}).get("paras") or {}
                commented_cnt = sum(
                    1
                    for v in paras.values()
                    if isinstance(v, dict) and int((v.get("count") or 0)) > 0
                )
                self.logger.info(
                    f"[段评] 章节 {chapter_id} 保存 (有评论段数={commented_cnt})"
                )
            except Exception:
                pass
            # 预取媒体（段评图片/头像）
            try:
                self._ensure_media_downloader()
                if self._media_downloader:
                    topn = int(getattr(self.config, "segment_comments_top_n", 10))
                    unique_cnt, completed, img_cnt, avatar_cnt = (
                        self._media_downloader.prefetch(data, top_n=topn)
                    )
                    self._media_prefetched = True
                    try:
                        self.logger.debug(
                            f"[媒体] 预取结果: unique={unique_cnt} completed={completed} img={img_cnt} avatar={avatar_cnt} top_n={topn}"
                        )
                    except Exception:
                        pass
            except Exception:
                pass
            # 进度标记
            try:
                self._media_progress_mark(chapter_id, "segment_comments")
            except Exception:
                pass
        except Exception:
            try:
                self.logger.debug(f"保存段评失败: chapter_id={chapter_id}")
            except Exception:
                pass

    # -------- 段评/媒体辅助 --------
    def _load_segment_comments_json(self, chapter_id: str):
        try:
            p = self.status_folder / "segment_comments" / f"{chapter_id}.json"
            if not p.exists():
                return None
            with p.open("r", encoding="utf-8") as f:
                return json.load(f)
        except Exception:
            return None

    def _media_progress_mark(self, chapter_id: str | None, reason: str = "") -> bool:
        if not getattr(self.config, "enable_segment_comments", False):
            return False
        cid = chapter_id or "_"
        try:
            with self._media_progress_lock:
                if cid in self._media_progress_done:
                    return False
                self._media_progress_done.add(cid)
            mp = getattr(self, "media_progress", None)
            if mp:
                try:
                    mp.update(1)
                except Exception:
                    pass
            return True
        except Exception:
            return False

    def _media_progress_finalize_fill(self):
        if not getattr(self.config, "enable_segment_comments", False):
            return
        try:
            for cid in self.downloaded.keys():
                if cid not in self._media_progress_done:
                    self._media_progress_mark(cid, "fill")
        except Exception:
            pass

    # 兼容旧接口：保留必要的透传给渲染层
    def _get_cached_media_filename(self, url: str) -> str | None:
        try:
            self._ensure_media_downloader()
            if self._media_downloader:
                return self._media_downloader.get_cached_media_filename(url)
        except Exception:
            pass
        return None

    def _ensure_media_downloader(self):
        if self._media_downloader is None:
            try:
                self._media_downloader = MediaDownloader(
                    self.config, self.logger, self.status_folder
                )
            except Exception:
                self._media_downloader = None
