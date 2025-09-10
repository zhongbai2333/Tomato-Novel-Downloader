import os
from ebooklib import epub
from pathlib import Path

from ..base_system.context import GlobalContext


class EpubGenerator:
    def __init__(
        self,
        identifier,
        title,
        language="en",
        author=None,
        description=None,
        publisher=None,
    ):
        """
        初始化EPUB书籍对象
        :param identifier: 书籍唯一标识符
        :param title: 书籍标题
        :param language: 语言代码（默认'en'）
        :param author: 作者（可选）
        :param publisher: 出版社（可选）
        """
        self.book = epub.EpubBook()

        # 设置基本元数据
        cover_path = str(
            GlobalContext.get_config().get_status_folder_path / f"{title}.jpg"
        )
        self.book.set_identifier(identifier)
        self.book.set_title(title)
        self.book.set_language(language)
        try:
            with open(cover_path, "rb") as cover_file:
                cover_content = cover_file.read()  # 获取二进制内容
        except FileNotFoundError:
            GlobalContext.get_logger().error(f"封面文件未找到: {cover_path}")
            cover_content = b""  # 返回空内容或处理错误
        except Exception as e:
            GlobalContext.get_logger().error(f"读取封面失败: {str(e)}")
            cover_content = b""
        self.book.set_cover("cover.jpg", cover_content, False)
        # self.book.get_item_with_id("cover").title = "封面"

        # 添加可选元数据
        if author:
            self.book.add_author(author)
        if publisher:
            self.book.add_metadata("DC", "publisher", publisher)
        if description:
            self.book.add_metadata("DC", "description", description)

        self.chapters = []
        self._file_counter = 0  # 用于生成自动文件名
        # 注入通用样式，统一正文颜色和段评标识
        # 动态首行缩进值（em）。允许 0 代表关闭缩进。
        try:
            indent_em = float(GlobalContext.get_config().first_line_indent_em)
            if indent_em < 0:
                indent_em = 0.0
        except Exception:
            indent_em = 2.0
        indent_rule = (
            f"text-indent:{indent_em}em;" if indent_em > 0 else "text-indent:0;"
        )
        css = f"""
            body {{ color:#000 !important; line-height:1.5; }}
            /* 统一段落：首行缩进(可配置) + 控制段间距 */
            p {{ color:#000 !important; {indent_rule} margin:0 0 .8em 0; line-height:1.5; }}
            p.no-indent {{ text-indent:0; }}
            a.seg-link {{ color: inherit; text-decoration: none; }}
            a.seg-link:hover {{ text-decoration: underline; }}
            .seg-count {{ color:#999; font-size:0.75em; margin-left:.3em; text-decoration: none; }}
            h3 {{ background:#f5f5f5; padding:.4em .6em; color:#333; font-weight:500; }}
            .para-title .para-index {{ font-weight:600; margin-right:.25em; color:#333; }}
            .para-title .para-src {{ color:#333; }}
            .para-title small {{ color:#666; }}
            .back-to-chapter {{ margin:.2em 0 .8em 0; }}
            .back-to-chapter a {{ font-size:.85em; color:#666; text-decoration:none; }}
            .back-to-chapter a:hover {{ color:#333; text-decoration:underline; }}
            .seg-images {{ margin:.25em 0 .5em 0; display:block; }}
            .seg-images img {{ max-width:100%; height:auto; max-height:220px; margin-right:.4em; margin-bottom:.2em; border-radius:2px; object-fit:contain; }}
            .avatar {{ width:36px; height:36px; border-radius:50%; object-fit:cover; vertical-align:middle; margin-right:.5em; }}
            .seg-meta {{ color:#666; display:block; text-align:right; }}
            li.seg-item {{ border-bottom:1px solid #ddd; padding:.5em 0 .6em 0; }}
            li.seg-item:last-child {{ border-bottom:none; }}
        """.encode(
            "utf-8"
        )
        self.style = epub.EpubItem(
            uid="style_main",
            file_name="styles/main.css",
            media_type="text/css",
            content=css,
        )
        self.book.add_item(self.style)

    def add_chapter(self, title, content, file_name=None, id=None):
        """
        添加章节到书籍
        :param title: 章节标题
        :param content: HTML内容（不带<html>标签）
        :param file_name: 自定义文件名（可选）
        """
        # 生成自动文件名（如果未提供）
        if not file_name:
            file_name = f"chapter_{self._file_counter}.xhtml"
            self._file_counter += 1

        # 创建章节对象
        chapter = epub.EpubHtml(
            title=title, file_name=file_name, lang=self.book.language
        )
        chapter.content = content
        chapter.add_item(self.style)

        # 添加到书籍
        self.book.add_item(chapter)
        self.chapters.append(chapter)

    def add_aux_page(self, title, content, file_name, include_in_spine: bool = True):
        """添加辅助页面；可选加入 spine/TOC（默认加入，解决阅读器无法访问的问题）。"""
        page = epub.EpubHtml(title=title, file_name=file_name, lang=self.book.language)
        page.content = content
        page.add_item(self.style)
        self.book.add_item(page)
        if include_in_spine:
            # 追加到章节序列，后续 toc/spine 统一使用 self.chapters
            self.chapters.append(page)

    def add_img(self, file_path: str):
        img_name = os.path.basename(file_path)
        img_uid = os.path.splitext(img_name)[0]
        with open(file_path, "rb") as f:
            img_content = f.read()
        # 根据扩展名设置正确的 MIME，避免部分阅读器不显示
        ext = os.path.splitext(img_name)[1].lower()
        if ext in (".jpg", ".jpeg"):
            mime = "image/jpeg"
        elif ext == ".png":
            mime = "image/png"
        elif ext == ".gif":
            mime = "image/gif"
        elif ext == ".webp":
            mime = "image/webp"  # 强制转码场景下主格式
        elif ext == ".avif":
            mime = "image/avif"
        else:
            mime = "image/jpeg"
        # 1. 用 EpubItem 将二进制图片打包
        img_item = epub.EpubItem(
            uid=img_uid,
            file_name=f"images/{img_name}",
            media_type=mime,
            content=img_content,
        )
        self.book.add_item(img_item)
        # 2. （可选）加入 manifest，确保 toc/导航也能识别
        # self.book.spine.append(img_item)
        # 無用，因後續已複寫 self.book.spine

    def generate(self, output_path, toc=None):
        """
        生成EPUB文件
        :param output_path: 输出文件路径
        :param toc: 自定义目录结构（可选）
        """
        # 导入所有插图
        img_path = GlobalContext.get_config().get_status_folder_path / "images"
        img_list = self.list_files(img_path)
        for file in img_list:
            # file 已是绝对路径，直接传入
            self.add_img(str(file))

        # 添加导航文件 (NCX and Nav) - 需要在TOC和Spine设置之前添加
        self.book.add_item(epub.EpubNcx())
        self.book.add_item(epub.EpubNav(title="目录"))  # 設默認物件 title 為 "目录"

        # 设置默认目录（如果未提供）
        if not toc:
            self.book.toc = self.chapters
        else:
            self.book.toc = toc

        self.book.spine = self.chapters

        # 生成文件
        epub.write_epub(output_path, self.book)

    def add_metadata(self, namespace, name, value):
        """
        添加自定义元数据
        :param namespace: 命名空间（如'DC'）
        :param name: 元数据名称
        :param value: 元数据值
        """
        self.book.add_metadata(namespace, name, value)

    def list_files(self, dir_path: str | Path):
        """
        返回目录下所有文件的绝对路径列表（不含子目录里的文件）。
        目录不存在时返回空列表，并给出提示。
        """
        p = Path(dir_path)

        if not p.exists():
            return []

        if not p.is_dir():
            return []

        # 只要文件，不要子目录，可按需改成 p.rglob('*') 递归
        files = [f.resolve() for f in p.iterdir() if f.is_file()]
        return files
