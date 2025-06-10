import os
import json
from pathlib import Path
from typing import Dict, List

from ..base_system.context import GlobalContext
from ..base_system.storage_system import FileCleaner
from .epub_generator import EpubGenerator

class BookManager(object):
    """书籍存储控制器"""
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
        self.end = True if tags[0] == "已完结" else False
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
        self.downloaded[chapter_id] = [title, content]
        if self.config.bulk_files:
            bulk_dir = self.save_dir / self.book_name
            bulk_dir.mkdir(parents=True, exist_ok=True)

            if self.config.novel_format == "epub":
                suffix = ".xhtml"
                # 简易的 XHTML 模板，生成时可根据需要补充 meta/css 等
                xhtml_template = f'''<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>{title}</title>
</head>
<body>
  {content}
</body>
</html>'''
                file_content = xhtml_template
            else:
                suffix = ".txt"
                file_content = f"{title}\n\n{content}"

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

    def save_error_chapter(self, chapter_id, title):
        """保存下载错误章节"""
        self.downloaded[chapter_id] = [title, "Error"]
        self.save_download_status()
        self.logger.debug(f"章节 {chapter_id} 下载错误记录缓存成功")

    def finalize_spawn(self, chapters, result):
        """生成最终文件"""
        if not self.config.bulk_files:
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
                    "简介",
                    f"<h1>简介</h1><p>{self.tags}</p><p>{self.description}</p>",
                    "description.xhtml",
                )

                for chapter in chapters:
                    chapter_id = chapter["id"]
                    epub.add_chapter(
                        self.downloaded.get(chapter_id, [chapter["title"], None])[0],
                        self.downloaded.get(
                            chapter_id,
                            [None, "<p>Download Faild or Didn't Download Finish!</p>"],
                        )[1],
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
