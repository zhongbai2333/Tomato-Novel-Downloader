import json
import requests
from bs4 import BeautifulSoup
from typing import Tuple, Dict

from ..base_system.context import GlobalContext


class ContentParser(object):
    """内容解析处理器"""

    @staticmethod
    def extract_api_content(response_data: dict) -> Dict[str, Tuple[str, str]]:
        """解析 API 返回的多章数据，批量处理

        Args:
            response_data: API 返回的 JSON 数据，包含 data: {id: {content, title}}

        Returns:
            dict: {章节ID: (清洗或格式化后的内容, 标题)}
        """
        chapters: Dict[str, Tuple[str, str]] = {}
        data = response_data.get("data", {})
        # 遍历所有章节项
        for cid, info in data.items():
            raw_content = info.get("content", "")
            title = info.get("title", "").strip()
            if GlobalContext.get_config().novel_format == "txt":
                processed = ContentParser._clean_content(raw_content)
            else:
                processed = ContentParser.clean_for_ebooklib(raw_content, title)
            chapters[cid] = (processed, title)
        return chapters

    @staticmethod
    def _clean_content(raw_content: str) -> str:
        """统一内容清洗方法——纯文本"""
        soup = BeautifulSoup(raw_content, "html.parser")
        if soup.header:
            soup.header.decompose()
        article = soup.article
        if not article:
            return ""
        paras = []
        for p in article.find_all("p"):
            text = p.get_text().strip()
            if text:
                paras.append("　　" + text + "\n")
        return "\n".join(paras)

    @staticmethod
    def clean_for_ebooklib(html_content: str, ch_title: str) -> str:
        """纯净 XHTML 内容，保留插图及占位块"""
        soup = BeautifulSoup(html_content, "html.parser")
        # 标题处理
        header = soup.find("header")
        if header:
            h1 = header.find("div", class_="tt-title")
            if h1:
                ch_title = h1.get_text(strip=True)
            header.decompose()

        content_parts = [f"<h1>{ch_title}</h1>"]
        article = soup.find("article")
        if article:
            for elem in article.children:
                # 忽略非标签节点
                if not getattr(elem, "name", None):
                    continue
                # 保留单独的 <img> 标签、包含 <img> 的块，及带有 data-fanqie-type 属性的占位 <div>
                if (
                    elem.name == "img"
                    or elem.find("img")
                    or (elem.name == "div" and elem.has_attr("data-fanqie-type"))
                ):
                    content_parts.append(str(elem))
                elif elem.name == "p":
                    # 转换 <br> 为换行
                    for br in elem.find_all("br"):
                        br.replace_with("\n")
                    text = elem.get_text().strip()
                    if text:
                        content_parts.append(f"<p>{text}</p>")
        return "\n".join(content_parts)

    @classmethod
    def parse_book_info(
        cls, html: str, book_id: str
    ) -> Tuple[str, str, str, list, int]:
        from pathlib import Path
        import re

        soup = BeautifulSoup(html, "html.parser")
        title = soup.find("h1").get_text(strip=True) if soup.find("h1") else "未知书名"
        author_div = soup.find("div", class_="author-name")
        author = (
            author_div.find("span", class_="author-name-text").get_text(strip=True)
            if author_div
            else "未知作者"
        )
        desc_div = soup.find("div", class_="page-abstract-content")
        description = (
            desc_div.find("p").get_text(strip=True)
            if desc_div and desc_div.find("p")
            else "无简介"
        )
        tags = [
            span.get_text(strip=True)
            for span in soup.find_all("div", class_="info-label")
            for span in span.find_all("span")
        ]
        chap_header = soup.find("div", class_="page-directory-header").find("h3")
        chapter_count = (
            int(re.search(r"\d+", chap_header.get_text(strip=True)).group())
            if chap_header
            else 0
        )
        # 封面
        script = next(
            (
                s
                for s in soup.find_all("script", type="application/ld+json")
                if s.string and "images" in s.string
            ),
            None,
        )
        image_url = None
        if script:
            try:
                data = json.loads(script.string)
                if isinstance(data.get("images"), list):
                    image_url = data["images"][0]
            except:
                pass
        img_path = (
            Path(
                GlobalContext.get_config().status_folder_path(
                    book_id=book_id, book_name=title
                )
            )
            / f"{title}.jpg"
        )
        if image_url:
            resp = requests.get(image_url)
            if resp.ok:
                img_path.write_bytes(resp.content)
        return title, author, description, tags, chapter_count
