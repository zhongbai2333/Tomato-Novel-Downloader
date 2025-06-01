import json
import requests
import urllib3
from bs4 import BeautifulSoup
from typing import Tuple, Dict

from ..base_system.context import GlobalContext

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
requests.packages.urllib3.disable_warnings()

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
                processed = ContentParser._clean_for_ebooklib(raw_content, title)
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
    def _clean_for_ebooklib(html_content: str, ch_title: str) -> str:
        """
        纯净 XHTML 内容，保留插图及占位块。
        本版改用 find_all(..., recursive=True) 来递归查找 <img>、<p>、带 data-fanqie-type 的 <div>，
        而不是只遍历 article.children。
        """
        # 1. 先用 BeautifulSoup 解析原始字符串
        soup = BeautifulSoup(html_content, "html.parser")

        # 2. 把 <header> 整段删掉，并从 header 里尝试提取章节标题
        header = soup.find("header")
        if header:
            h1 = header.find("div", class_="tt-title")
            if h1:
                ch_title = h1.get_text(strip=True)
            header.decompose()

        # 3. 构造输出内容列表，第一行放 <h1>章节名</h1>
        content_parts = [f"<h1>{ch_title}</h1>"]

        # 4. 定位到 <article> 节点
        article = soup.find("article")
        if not article:
            # 如果没有 article，就仅返回标题
            return "\n".join(content_parts)

        # 5. 递归地在 article 中找到所有 <img>、<p>、和带 data-fanqie-type 的 <div>
        #    注意：find_all(..., recursive=True) 会在任意嵌套深度都查到子孙节点
        #    按照原意，优先保留：<img> 或者 包含 <img> 的块，或者 data-fanqie-type 占位 <div>，
        #    其次处理 <p> 标签，把里面的 <br> 换成换行，再拼成 <p>纯文本</p>。
        for elem in article.find_all(recursive=True):
            # A. 如果它是 <img> 标签，直接保持整个 <img>；或者如果它的子孙里有 <img>，也保持整个父级块
            if elem.name == "img":
                content_parts.append(str(elem))
                # 既然整张 <img> 都加过了，就跳过它的后代
                continue

            # B. 任何包含 <img> 的块级元素，比如 <div>、<figure>、<section> 等
            #    只要 elem.find("img") 不为 None，就保留整个 elem 的原始 HTML
            if elem.find("img"):
                # 检查是否已经把这个块加过了——避免同一个父级和子级都加一遍
                # 这里简单做法：只保留最外层包含 <img> 的块
                parent_has_been_added = False
                parent_cursor = elem.parent
                while parent_cursor and parent_cursor != article:
                    if parent_cursor.name in ("div", "section", "figure", "article") and parent_cursor.find("img") is not None:
                        parent_has_been_added = True
                        break
                    parent_cursor = parent_cursor.parent

                if not parent_has_been_added:
                    content_parts.append(str(elem))
                continue

            # C. 如果它是 <div> 并且带 data-fanqie-type 属性，就保留整个 <div>
            if elem.name == "div" and elem.has_attr("data-fanqie-type"):
                content_parts.append(str(elem))
                continue

            # D. 如果它是 <p> 标签：
            #    1) 把所有 <br> 换成换行符 "\n"
            #    2) 取纯文本（strip 后如果非空，就拼成 <p>文本</p>）
            if elem.name == "p":
                for br in elem.find_all("br"):
                    br.replace_with("\n")
                text = elem.get_text().strip()
                if text:
                    content_parts.append(f"<p>{text}</p>")

        # 6. 最终把所有部分 join 到一起，返回干净的 XHTML 片段
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
            resp = requests.get(image_url, verify=False)
            if resp.ok:
                img_path.write_bytes(resp.content)
        return title, author, description, tags, chapter_count
