# -------------------------------
# book_manager.py - 书籍管理模块
# -------------------------------
import os
import json
from pathlib import Path
from typing import Dict

from ..base_system.context import GlobalContext
from .epub_generator import EpubGenerator


class BookManager:
    """书籍文件管理类"""

    def __init__(
        self,
        save_path: str,
        book_id: str,
        book_name: str,
        author: str,
        tags: list,
        description: str,
    ):
        # 书本信息缓存
        self.save_dir = Path(save_path)
        self.book_id = book_id
        self.book_name = book_name
        self.author = author
        self.tags = "|".join(tags)
        self.description = description

        # 初始化
        self.config = GlobalContext.get_config()
        self.logger = GlobalContext.get_logger()

        # 缓存
        self.downloaded: Dict[list] = {}

        # 状态文件路径
        self.status_file = self.config.status_file_path(save_path, book_id)

        self._load_download_status()

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
                self.logger.error(f"状态文件保存失败: {e}")

    def save_chapter(self, chapter: Dict, title: str, content: str):
        """保存章节内容（统一入口）"""
        self.downloaded[chapter["id"]] = [title, content]
        self.logger.debug(f"章节 {chapter['id']} 缓存成功")

    def save_error_chapter(self, chapter_id):
        """保存下载错误章节"""
        self.downloaded[chapter_id] = ["Error", "Error"]
        self.logger.debug(f"章节 {chapter_id} 下载错误记录缓存成功")

    def finalize_spawn(self, result):
        """生成最终文件"""
        output_file = self.save_dir / f"{self.book_name}.{self.config.novel_format}"
        if output_file.exists():
            os.remove(output_file)
        if self.config.novel_format == "epub":
            # 生成EPUB骨架
            epub = EpubGenerator(
                self.book_id, self.book_name, "zh-CN", self.author, self.description, "番茄小说"
            )

            epub.add_chapter("简介", f"<h1>简介</h1><p>{self.tags}</p><p>{self.description}</p>", "description.xhtml")

            for chapter in self.downloaded.values():
                epub.add_chapter(chapter[0], chapter[1])

            epub.generate(output_file)
            self.logger.info(
                f"EPUB生成完成: {self.save_dir / f'{self.book_name}.epub'}"
            )
        else:
            with output_file.open("w", encoding="utf-8") as f:
                f.write(
                    f"书名: {self.book_name}\n作者: {self.author}\n标签: {self.tags}\n简介: {self.description}\n\n"
                )
                for chapter in self.downloaded.values():
                    f.write(f"\n\n{chapter[0]}\n{chapter[1]}")
            self.logger.info(f"TXT生成完成: {output_file}")
        if result == 0 and self.config.auto_clear_dump:
            cover_path = self.save_dir / f"{self.book_name}.jpg"
            if self.status_file.exists():
                os.remove(self.status_file)
                self.logger.debug(f"断点缓存文件已清理！{self.status_file}")
            if cover_path.exists():
                os.remove(cover_path)
                self.logger.debug(f"封面文件已清理！{cover_path}")
