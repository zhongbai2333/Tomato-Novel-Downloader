# -------------------------------
# network.py - 网络请求模块
# 职责：处理所有HTTP请求相关逻辑
# -------------------------------
import json
import requests
from typing import Optional, Dict, List
from fake_useragent import UserAgent

from ..base_system.context import GlobalContext
from ..book_parser.parser import ContentParser


class NetworkClient:
    """网络请求客户端"""

    def __init__(self):
        self.logger = GlobalContext.get_logger()
        self.config = GlobalContext.get_config()
        self._api_status: Dict[str, dict] = {}  # API状态跟踪字典
        self._init_api_status()

    def _init_api_status(self):
        """初始化API状态跟踪器"""
        for endpoint in self.config.api_endpoints:
            self._api_status[endpoint] = {
                "failure_count": 0,
                "last_success": 0.0,
                "response_time": float("inf"),
            }

    def get_headers(self, cookie: Optional[str] = None) -> Dict[str, str]:
        """生成随机请求头

        Args:
            cookie: 可选Cookie字符串

        Returns:
            包含随机User-Agent的请求头字典
        """
        ua = UserAgent(
            browsers=["Chrome", "Edge"],  # 限定主流浏览器
            os=["Windows"],  # 仅Windows系统
            platforms=["desktop"],  # 仅桌面端
            fallback="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36 Edg/134.0.0.0",  # 备用UA
        )
        headers = {
            "User-Agent": ua.random,
            "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            "Accept-Language": "zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7",
            "Cache-Control": "max-age=0",
            "Connection": "keep-alive",
            "Upgrade-Insecure-Requests": "1",
        }
        self.logger.debug(f"Header: {headers}")
        if cookie:
            headers["Cookie"] = cookie
        return headers

    def search_book(self, book_name: str) -> str:
        for endpoint in self.config.api_endpoints:
            api = endpoint + f"/search?query={book_name}&offset=0"
            try:
                response = requests.get(
                    url=api,
                    headers=self.get_headers(),
                    timeout=self.config.request_timeout,
                )
                response.raise_for_status()
            except requests.RequestException as e:
                self.logger.error(f"通过端点 {endpoint} 搜索失败: {str(e)}")
                continue
            data = response.json()
            search_datas = data["search_tabs"][5]["data"]
            book_id_list = []
            for num, search_res in enumerate(search_datas):
                book_info = search_res["book_data"][0]
                self.logger.info(
                    f"{num + 1}. 书名: {book_info['book_name']} | 初始书名: {book_info['original_book_name']} | ID: {book_info['book_id']} | 作者: {book_info['author']}"
                )
                book_id_list.append(book_info["book_id"])
            while True:
                num = input("请输入序号 (输入q返回重新搜索)：")
                if num == "q":
                    return "0000"
                if 1 <= int(num) <= len(book_id_list):
                    return book_id_list[int(num) - 1]
                else:
                    self.logger.warning("输入错误!")
        return None

    def get_book_info(self, book_id: str) -> tuple:
        book_info_url = f"https://fanqienovel.com/page/{book_id}"

        # 发送请求
        try:
            response = requests.get(
                book_info_url,
                headers=self.get_headers(),
                timeout=self.config.request_timeout,
            )
            if response.status_code == 404:
                self.logger.error(f"小说ID {book_id} 不存在！")
                return None
            response.raise_for_status()
            book_name, author, description, tags, chapter_count = (
                ContentParser.parse_book_info(response.text, book_id)
            )
            return book_name, author, description, tags, chapter_count
        except requests.RequestException as e:
            self.logger.error(f"获取书籍信息失败: {str(e)}")
            return None
        except Exception as e:
            self.logger.error(f"解析书籍信息失败: {str(e)}")
            return None

    def fetch_chapter_list(self, book_id: str) -> Optional[List[Dict]]:
        """从API获取章节列表"""
        api_url = (
            f"https://fanqienovel.com/api/reader/directory/detail?bookId={book_id}"
        )
        try:
            self.logger.debug(f"开始获取章节列表，URL: {api_url}")
            response = requests.get(
                api_url, headers=self.get_headers(), timeout=self.config.request_timeout
            )
            self.logger.debug(
                f"章节列表响应状态: {response.status_code} 长度: {len(response.text)}字节"
            )

            response.raise_for_status()
            return self._parse_chapter_data(response.json())
        except Exception as e:
            self.logger.error(f"获取章节列表失败: {str(e)}", exc_info=True)
            if "response" in locals():
                self.logger.debug(f"错误响应内容: {response.text[:200]}...")
            return None

    def _parse_chapter_data(self, response_data: dict) -> List[Dict]:
        """解析章节API响应"""
        self.logger.debug(f"开始解析章节数据，响应码: {response_data.get('code')}")

        if response_data.get("code") != 0:
            self.logger.error(
                f"API错误数据: {json.dumps(response_data, ensure_ascii=False)[:200]}..."
            )
            raise ValueError(f"API错误: {response_data.get('message')}")

        chapters = response_data["data"]["allItemIds"]
        self.logger.info(f"解析到{len(chapters)}个章节ID，示例: {chapters[:3]}...")
        return [
            {"id": chapter_id, "title": f"第{idx+1}章", "index": idx}
            for idx, chapter_id in enumerate(chapters)
        ]
