# -------------------------------
# config.py - 配置管理模块
# 职责：集中管理所有配置项和路径
# -------------------------------
import os
import re
from logging import Logger
from pathlib import Path

from .storge_system import BaseConfig, Field, ConfigError
from .log_system import LogSystem


class Config(BaseConfig):
    """Config 配置文件"""

    # 网络配置
    max_workers: int = Field(default=1, description="最大并发线程数")
    request_timeout: int = Field(default=15, description="请求超时时间（秒）")
    max_retries: int = Field(default=3, description="最大重试次数")
    max_wait_time: int = Field(default=1200, description="最大冷却时间, 单位ms")
    min_wait_time: int = Field(default=1000, description="最小冷却时间, 单位ms")
    min_connect_timeout: float = Field(default=3.05, description="最小连接超时时间")
    force_exit_timeout: int = Field(default=5, description="强制退出等待时间")
    graceful_exit: bool = Field(default=True, description="是否启用优雅退出")

    # 保存配置
    novel_format: str = Field(
        default="txt", description="保存小说格式, 可选: [txt, epub]"
    )
    auto_clear_dump: bool = Field(default=True, description="是否自动清理缓存文件")

    # 路径配置
    status_filename: str = Field(
        default="chapter_status.json", description="下载状态文件名"
    )
    cookie_filename: str = Field(
        default="session_cookie.json", description="Cookie存储文件名"
    )
    save_path: str = Field(default="", description="保存路径")

    # API配置
    api_endpoints: list = Field(
        default=[
            "http://rehaofan.jingluo.love/content?item_id={chapter_id}",
            "http://yuefanqie.jingluo.love/content?item_id={chapter_id}",
            "http://apifq.jingluo.love/content?item_id={chapter_id}",
            "http://fan.jingluo.love/content?item_id={chapter_id}",
            "https://lsjk.zyii.xyz:3666/content?item_id={chapter_id}",
        ],
        description="API列表",
    )

    @property
    def default_save_dir(self) -> Path:
        """获取默认保存目录路径对象"""
        return Path(self.save_path) or Path(os.getcwd())

    def status_file_path(self, save_dir: str, book_id: str) -> Path:
        """生成书籍专属状态文件路径"""
        # 清理非法字符确保文件名安全
        safe_book_id = re.sub(r"[^a-zA-Z0-9_]", "_", book_id)
        filename = f"chapter_status_{safe_book_id}.json"
        return Path(save_dir) / filename


class GlobalContext(object):
    def __init__(self, debug: bool):
        global _log_system, _config
        _log_system = LogSystem(debug=debug)
        try:
            _config = Config.load()
        except ConfigError as e:
            _log_system.logger.error(f"配置操作失败: {str(e)}")

    @staticmethod
    def get_logger() -> Logger:
        return _log_system.logger
    
    @staticmethod
    def get_log_system() -> LogSystem:
        return _log_system

    @staticmethod
    def get_config() -> Config:
        return _config
