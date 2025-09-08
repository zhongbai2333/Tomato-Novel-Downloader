# -------------------------------
# network.py - 网络请求模块
# 职责：处理所有HTTP请求相关逻辑
# -------------------------------
import json
import time
import random
import requests
import urllib3
from typing import Optional, Dict, List
from pathlib import Path
from fake_useragent import UserAgent

from ..base_system.context import GlobalContext
from ..book_parser.parser import ContentParser
from ..offical_tools.downloader import search_api

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
requests.packages.urllib3.disable_warnings()

class NetworkClient:
    """网络请求客户端"""

    def __init__(self):
        self.logger = GlobalContext.get_logger()
        self.config = GlobalContext.get_config()
        self._api_status: Dict[str, dict] = {}  # API状态跟踪字典
        self._init_api_status()
        # 复用连接以提升性能
        self.session = requests.Session()
        # 目录请求节流
        self._last_dir_fetch_ts = 0.0

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
    # 不在此处打印完整 Header，避免泄露敏感信息/噪音
        if cookie:
            headers["Cookie"] = cookie
        return headers

    def search_book(self, book_name: str) -> List[Dict[str, str]]:
        search_datas = search_api(book_name)
        book_list: List[Dict[str, str]] = []
        for _, search_res in enumerate(search_datas):
            book_list.append({
                "title": search_res["title"],
                "book_id": search_res["book_id"],
                "author": search_res["author"],
            })
        return book_list
    
    def get_json_headers(self, book_id: Optional[str] = None, cookie: Optional[str] = None) -> Dict[str, str]:
        """更贴近浏览器的 JSON/API 请求头，附带 Referer/Origin 等。"""
        ua = UserAgent(
            browsers=["Chrome", "Edge"],
            os=["Windows"],
            platforms=["desktop"],
            fallback="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36 Edg/134.0.0.0",
        )
        referer = f"https://fanqienovel.com/page/{book_id}" if book_id else "https://fanqienovel.com/"
        headers = {
            "User-Agent": ua.random,
            "Accept": "application/json, text/plain, */*",
            "Accept-Language": "zh-CN,zh;q=0.9",
            "Connection": "keep-alive",
            "Referer": referer,
            "Origin": "https://fanqienovel.com",
        }
        if cookie:
            headers["Cookie"] = cookie
        return headers

    def get_book_info(self, book_id: str) -> tuple:
        book_info_url = f"https://fanqienovel.com/page/{book_id}"

        # 发送请求
        try:
            response = self.session.get(
                book_info_url,
                headers=self.get_headers(),
                timeout=self.config.request_timeout,
                verify=False,
            )
            if response.status_code == 404:
                self.logger.error(f"小说ID {book_id} 不存在！")
                return None, None, None, None, None
            response.raise_for_status()
            book_name, author, description, tags, chapter_count = (
                ContentParser.parse_book_info(response.text, book_id)
            )
            return book_name, author, description, tags, chapter_count
        except requests.RequestException as e:
            self.logger.error(f"获取书籍信息失败: {str(e)}")
            return None, None, None, None, None
        except Exception as e:
            self.logger.error(f"解析书籍信息失败: {str(e)}")
            return None, None, None, None, None

    def fetch_chapter_list(self, book_id: str) -> Optional[List[Dict]]:
        """从API获取章节列表"""
        # 无效 book_id 直接返回 None，避免无意义请求
        if not book_id or not str(book_id).isdigit():
            self.logger.warning(f"fetch_chapter_list 跳过无效 book_id: '{book_id}'")
            return None
        api_url = (
            f"https://fanqienovel.com/api/reader/directory/detail?bookId={book_id}"
        )
        # 节流：与上次请求间隔至少 0.8s，降低被限频概率
        now = time.time()
        gap = now - self._last_dir_fetch_ts
        if gap < 0.8:
            time.sleep(0.8 - gap)
        self._last_dir_fetch_ts = time.time()

        retries = max(1, int(getattr(self.config, "max_retries", 3)))
        backoff = 0.6
        last_error = None
        for attempt in range(1, retries + 1):
            try:
                self.logger.debug(f"开始获取章节列表，URL: {api_url}")
                headers = self.get_json_headers(book_id)
                # 屏蔽 Cookie，仅在 DEBUG 下打印关键信息
                if attempt == 1:
                    masked = {k: ("***" if k.lower() == "cookie" else v) for k, v in headers.items()}
                    self.logger.debug(f"目录请求Header(精简): UA={masked.get('User-Agent','')[:32]}..., Referer={masked.get('Referer','')}")
                else:
                    self.logger.debug(f"重试第 {attempt} 次获取目录（可能被限频/风控），URL: {api_url}")
                response = self.session.get(
                    api_url,
                    headers=headers,
                    timeout=self.config.request_timeout,
                    verify=False,
                )
                self.logger.debug(
                    f"章节列表响应状态: {response.status_code} 长度: {len(response.text)}字节"
                )
                # 显式处理 403：可能为风控或限频
                if response.status_code == 403:
                    last_error = requests.HTTPError("403 Forbidden")
                    # 首次遇到 403 时，尝试预热页面以获取必要 Cookie，再退避重试
                    if attempt == 1:
                        try:
                            warm_url = f"https://fanqienovel.com/page/{book_id}"
                            self.session.get(
                                warm_url,
                                headers=self.get_headers(),
                                timeout=self.config.request_timeout,
                                verify=False,
                            )
                            self.logger.debug("已尝试通过页面预热获取 Cookie，准备退避后重试目录 API")
                        except Exception as _e:
                            self.logger.debug(f"页面预热失败: {_e}")
                    # 退避 + 抖动
                    time.sleep(backoff + random.uniform(0.0, 0.4))
                    backoff = min(backoff * 2, 3.0)
                    continue

                response.raise_for_status()
                data = response.json()
                # 成功则缓存原始 JSON，便于下次回退
                try:
                    self._save_dir_cache(book_id, data)
                except Exception:
                    pass
                return self._parse_chapter_data(data)
            except Exception as e:
                last_error = e
                self.logger.error(f"获取章节列表失败: {str(e)}", exc_info=True)
                if "response" in locals():
                    self.logger.debug(f"错误响应内容: {response.text[:200]}...")
                # 非最后一次，退避重试
                if attempt < retries:
                    time.sleep(backoff + random.uniform(0.0, 0.3))
                    backoff = min(backoff * 2, 3.0)
                else:
                    break

        # 重试仍失败：尝试使用本地缓存回退
        cached = self._load_dir_cache(book_id)
        if cached is not None:
            self.logger.debug(f"使用本地缓存的章节目录回退: book_id={book_id}")
            try:
                return self._parse_chapter_data(cached)
            except Exception as e:
                self.logger.debug(f"缓存解析失败: {e}")
        # 回退失败
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
        self.logger.debug(f"解析到{len(chapters)}个章节ID，示例: {chapters[:3]}...")
        return [
            {"id": chapter_id, "title": f"第{idx+1}章", "index": idx}
            for idx, chapter_id in enumerate(chapters)
        ]

    # ---------- 目录缓存 ----------
    def _cache_dir_path(self, book_id: str) -> Path:
        root = getattr(self.config, "get_status_folder_path", None)
        # 容错：若为 None 或 str，规范化为 Path
        try:
            if root is None:
                root = Path.cwd() / "logs"
            elif not isinstance(root, Path):
                root = Path(str(root))
        except Exception:
            root = Path.cwd() / "logs"
        p = root / "dir_cache"
        p.mkdir(parents=True, exist_ok=True)
        return p / f"{book_id}.json"

    def _save_dir_cache(self, book_id: str, resp_json: dict) -> None:
        path = self._cache_dir_path(book_id)
        with path.open("w", encoding="utf-8") as f:
            json.dump(resp_json, f, ensure_ascii=False)

    def _load_dir_cache(self, book_id: str) -> Optional[dict]:
        path = self._cache_dir_path(book_id)
        if not path.exists():
            return None
        try:
            with path.open("r", encoding="utf-8") as f:
                return json.load(f)
        except Exception:
            return None

    # ================= 段评相关 =================
    def fetch_para_comment_stats(self, chapter_id: str, item_version: str = "1") -> Optional[dict]:
        """
        获取每段（para_index）对应的段评统计（count/hot/infos...）。
        返回 {"item_version": <str 或 None>, "stats": <dict: {"0": {...}}>}，失败返回 None。
        """
        aid = 1967
        url = f"https://api5-normal-sinfonlinea.fqnovel.com/novel/commentapi/idea/list/{chapter_id}/v1"
        body = {"item_version": str(item_version)}
        # 带重试（解决偶发 5xx/502）
        retries = max(1, int(getattr(self.config, "max_retries", 3)))
        backoff = 0.5
        for attempt in range(1, retries + 1):
            try:
                resp = self.session.post(
                    url,
                    params={"aid": aid},
                    json=body,
                    headers=self.get_headers(),
                    timeout=self.config.request_timeout,
                    verify=False,
                )
                # 对 5xx 不 raise，让逻辑统一处理
                if resp.status_code >= 500:
                    raise requests.RequestException(f"server {resp.status_code}")
                resp.raise_for_status()
                js = resp.json()
                if js.get("code") == 0 and isinstance(js.get("data"), dict):
                    d = js["data"]
                    inner = d.get("data") if isinstance(d, dict) else None
                    # 尝试多处提取 item_version
                    iv = (
                        d.get("item_version")
                        if isinstance(d, dict)
                        else None
                    )
                    if not iv:
                        extra = d.get("extra") if isinstance(d, dict) else None
                        if isinstance(extra, dict):
                            iv = extra.get("item_version") or extra.get("novel_item_version")
                    if isinstance(inner, dict):
                        return {"item_version": iv, "stats": inner}
                return None
            except Exception as e:
                if attempt >= retries:
                    self.logger.debug(f"段评统计获取失败（已达重试上限）: {e}")
                    return None
                # 指数退避
                try:
                    import time

                    time.sleep(backoff)
                    backoff = min(backoff * 2, 4.0)
                except Exception:
                    pass

    def fetch_para_comments(
        self,
        chapter_id: str,
        book_id: str,
        para_index: int,
        item_version: str = "1",
        limit: int = 10,
        sort: int = 1,
    ) -> Optional[dict]:
        """
        获取某段的详细评论列表，默认取前 limit 条。
        返回包含 total、data_list、cursor 等关键字段的精简 dict。失败返回 None。
        """
        aid = 1967
        url = f"https://api5-normal-sinfonlinea.fqnovel.com/novel/commentapi/comment/list/{chapter_id}/v1"
        body = {
            "business_param": {
                "book_id": str(book_id),
                "item_version": str(item_version),
                "para_index": int(para_index),
            },
            "comment_source": 2,
            "comment_type": 1,
            "group_type": 15,
            "sort": int(sort),
        }

        # 带重试（解决偶发 5xx/502）
        retries = max(1, int(getattr(self.config, "max_retries", 3)))
        backoff = 0.5
        for attempt in range(1, retries + 1):
            try:
                resp = self.session.post(
                    url,
                    params={"aid": aid},
                    json=body,
                    headers=self.get_headers(),
                    timeout=self.config.request_timeout,
                    verify=False,
                )
                if resp.status_code >= 500:
                    raise requests.RequestException(f"server {resp.status_code}")
                resp.raise_for_status()
                js = resp.json()
                if js.get("code") != 0:
                    return None
                data = js.get("data") or {}
                common = data.get("common_list_info") or {}
                total = common.get("total", 0)
                has_more = bool(common.get("has_more"))
                cursor = str(common.get("cursor")) if common.get("cursor") is not None else None
                data_list = data.get("data_list") or []

                # 仅取前 limit 条
                if isinstance(data_list, list) and limit > 0:
                    data_list = data_list[:limit]
                return {
                    "total": total,
                    "has_more": has_more,
                    "cursor": cursor,
                    "para_index": para_index,
                    "data_list": data_list,
                    "extra": data.get("extra"),
                }
            except Exception as e:
                if attempt >= retries:
                    self.logger.debug(f"段评详情获取失败（已达重试上限）: {e}")
                    return None
                try:
                    import time

                    time.sleep(backoff)
                    backoff = min(backoff * 2, 4.0)
                except Exception:
                    pass
