# -------------------------------
# parser.py - 内容解析模块
# 职责：处理HTML/JSON数据的解析和清洗
# -------------------------------
import os
import re
import json
import requests
from bs4 import BeautifulSoup
from typing import Tuple

from .context import GlobalContext


class ContentParser:
    """内容解析处理器"""

    @staticmethod
    def extract_api_content(response_data: dict) -> Tuple[str, str]:
        """解析API响应数据

        Args:
            response_data: API返回的JSON数据

        Returns:
            tuple: (清理后的内容, 章节标题)
        """
        content = response_data.get("data", {}).get("content", "")
        title = response_data.get("data", {}).get("title", "")

        # 统一内容清洗流程
        return (
            ContentParser._clean_content(content)
            if GlobalContext.get_config().novel_format == "txt"
            else ContentParser.clean_for_ebooklib(content, title)
        ), title.strip()

    @staticmethod
    def _clean_content(raw_content: str) -> str:
        """统一内容清洗方法

        Args:
            raw_content: 原始内容文本

        Returns:
            格式化后的标准内容
        """
        # 创建BeautifulSoup对象
        soup = BeautifulSoup(raw_content, "html.parser")

        # 删除header部分
        if soup.header:
            soup.header.decompose()

        # 获取article内容
        article = soup.article
        if not article:
            return ""

        # 提取所有段落并处理格式
        processed_paragraphs = []
        for p in article.find_all("p"):
            # 获取纯文本并去除首尾空白
            text = p.get_text().strip()
            # 过滤空段落
            if text:
                # 添加首行缩进（4个全角空格）
                processed_paragraphs.append("　　" + text + "\n")

        # 合并段落并用换行连接
        return "\n".join(processed_paragraphs)

    @staticmethod
    def clean_for_ebooklib(html_content: str, ch_title: str) -> str:
        """ebooklib专用的纯净XHTML内容"""
        soup = BeautifulSoup(html_content, "html.parser")

        # 提取章节标题
        chapter_title = ch_title
        header = soup.find("header")
        if header:
            title_div = header.find("div", class_="tt-title")
            if title_div:
                chapter_title = title_div.get_text(strip=True)
            header.decompose()

        # 构建核心内容
        content = []
        content.append(f"<h1>{chapter_title}</h1>")  # 章节标题

        # 处理正文段落
        article = soup.find("article")
        if article:
            for p in article.find_all("p"):
                text = p.get_text(strip=True)
                if text:  # 过滤空段落
                    # 保留原始换行（转换<br>为换行符）
                    for br in p.find_all("br"):
                        br.replace_with("\n")
                    # 生成标准段落
                    content.append(f"<p>{p.get_text().strip()}</p>")

        return "\n".join(content)

    @classmethod
    def parse_book_info(cls, html: str) -> Tuple[str, str, str, list, int]:
        """解析书籍信息页面

        Args:
            html: 书籍页面的HTML内容

        Returns:
            tuple: (书名, 作者, 简介)
        """
        soup = BeautifulSoup(html, "html.parser")

        # 提取书名
        title = soup.find("h1").text.strip() if soup.find("h1") else "未知书名"

        # 提取作者
        author_div = soup.find("div", class_="author-name")
        author = (
            author_div.find("span", class_="author-name-text").text.strip()
            if author_div
            else "未知作者"
        )

        # 提取简介
        desc_div = soup.find("div", class_="page-abstract-content")
        description = (
            desc_div.find("p").text.strip()
            if desc_div and desc_div.find("p")
            else "无简介"
        )

        tag_div = soup.find("div", class_="info-label")
        tags = []
        if tag_div:
            spans = tag_div.find_all("span")
            tags = [span.get_text(strip=True) for span in spans]

        pages_header_div = soup.find("div", class_="page-directory-header")
        pages_num = pages_header_div.find("h3")
        # 提取整个h3文本内容
        raw_text = pages_num.get_text(strip=True)  # 输出："目录62章"

        # 使用正则表达式提取数字
        chapter_number = re.search(r"\d+", raw_text)

        chapter_count = int(chapter_number.group())

        # 查找包含JSON数据的<script>标签
        script_tags = soup.find_all("script", type="application/ld+json")

        # 遍历所有符合条件的script标签，提取目标JSON
        image_url = None
        for script in script_tags:
            try:
                data = json.loads(script.string)
                if (
                    "images" in data
                    and isinstance(data["images"], list)
                    and len(data["images"]) > 0
                ):
                    image_url = data["images"][0]  # 取第一个图片URL
                    break
            except json.JSONDecodeError:
                continue

        img_path = GlobalContext.get_config().default_save_dir / "{}.jpg".format(title)
        if img_path.exists():
            os.remove(img_path)
        with open(img_path, "wb") as fd:
            image = requests.get(url=image_url).content
            fd.write(image)

        return title, author, description, tags, chapter_count
