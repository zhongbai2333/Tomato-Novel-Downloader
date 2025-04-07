# -------------------------------
# network.py - 网络请求模块
# 职责：处理所有HTTP请求相关逻辑
# -------------------------------
import random
import time
import requests
from typing import Optional, Dict, Tuple
from fake_useragent import UserAgent

from ..base_system.context import GlobalContext


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

    def load_or_create_cookie(self) -> str:
        """管理Cookie的生命周期

        Returns:
            有效的Cookie字符串

        Raises:
            RuntimeError: 当无法获取有效Cookie时
        """
        # 尝试加载现有Cookie
        cookie_file = self.config.default_save_dir / self.config.cookie_filename
        if cookie_file.exists():
            try:
                return cookie_file.read_text(encoding="utf-8").strip()
            except Exception as e:
                self.logger.error(f"读取Cookie文件失败: {e}")

        # 生成新Cookie
        for _ in range(self.config.max_retries):
            new_cookie = f"novel_web_id={random.randint(10**18, 10**19-1)}"
            try:
                resp = requests.get(
                    "https://fanqienovel.com",
                    headers=self.get_headers(),
                    cookies={"novel_web_id": new_cookie},
                    timeout=self.config.request_timeout,
                )
                if resp.ok:
                    cookie_file.write_text(new_cookie, encoding="utf-8")
                    return new_cookie
            except Exception as e:
                self.logger.error(f"Cookie验证失败: {e}")
                time.sleep(1)

        raise RuntimeError("无法获取有效Cookie，请检查网络连接")
