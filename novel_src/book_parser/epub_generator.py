from ebooklib import epub

from ..base_system.context import GlobalContext

class EpubGenerator:
    def __init__(self, identifier, title, language="en", author=None, description=None, publisher=None):
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
        cover_path = str(GlobalContext.get_config().default_save_dir / f"{title}.jpg")
        self.book.set_identifier(identifier)
        self.book.set_title(title)
        self.book.set_language(language)
        try:
            with open(cover_path, 'rb') as cover_file:
                cover_content = cover_file.read()  # 获取二进制内容
        except FileNotFoundError:
            GlobalContext.get_logger().error(f"封面文件未找到: {cover_path}")
            cover_content = b''  # 返回空内容或处理错误
        except Exception as e:
            GlobalContext.get_logger().error(f"读取封面失败: {str(e)}")
            cover_content = b''
        self.book.set_cover("cover.jpg", cover_content)

        # 添加可选元数据
        if author:
            self.book.add_author(author)
        if publisher:
            self.book.add_metadata("DC", "publisher", publisher)
        if description:
            self.book.add_metadata("DC", "description", description)

        style = '''
        @namespace epub "http://www.idpf.org/2007/ops";
        body { font-family: "Noto Serif CJK SC", SimSun, serif; }
        h1 { text-align: center; margin: 1em 0; }
        p { text-indent: 2em; margin: 0.5em 0; }
        '''
        nav_css = epub.EpubItem(
            uid="style_nav",
            file_name="style/nav.css",
            media_type="text/css",
            content=style
        )
        self.book.add_item(nav_css)

        self.chapters = []
        self._file_counter = 1  # 用于生成自动文件名

    def add_chapter(self, title, content, file_name=None):
        """
        添加章节到书籍
        :param title: 章节标题
        :param content: HTML内容（不带<html>标签）
        :param file_name: 自定义文件名（可选）
        """
        # 生成自动文件名（如果未提供）
        if not file_name:
            file_name = f"chap_{self._file_counter:02d}.xhtml"
            self._file_counter += 1

        # 创建章节对象
        chapter = epub.EpubHtml(
            title=title, file_name=file_name, lang=self.book.language
        )
        chapter.content = content

        # 添加到书籍
        self.book.add_item(chapter)
        self.chapters.append(chapter)

    def generate(self, output_path, toc=None):
        """
        生成EPUB文件
        :param output_path: 输出文件路径
        :param toc: 自定义目录结构（可选）
        """
        # 设置默认目录（如果未提供）
        if not toc:
            self.book.toc = [(epub.Section("目录"), self.chapters)]
        else:
            self.book.toc = toc

        # 添加导航文件
        self.book.add_item(epub.EpubNcx())
        self.book.add_item(epub.EpubNav())

        # 设置书脊（spine）
        self.book.spine = ["nav"] + self.chapters

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
