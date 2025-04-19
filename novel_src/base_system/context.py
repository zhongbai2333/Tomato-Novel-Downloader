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
    max_workers: int = Field(default=3, description="最大并发线程数")
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
    save_path: str = Field(default="", description="保存路径")

    # API配置
    use_official_api: bool = Field(default=True, description="使用官方API")
    iid: str = Field(default="", description="自动生成")
    iid_spawn_time: str = Field(default="", description="iid生成时间戳")
    api_endpoints: list = Field(
        default=[],
        description="API列表",
    )

    @property
    def default_save_dir(self) -> Path:
        """获取默认保存目录路径对象"""
        if self.save_path:
            return Path(self.save_path)
        else:
            return Path(os.getcwd())

    @property
    def get_status_folder_path(self) -> Path:
        """获取已经生成的Folder_path"""
        try:
            return self.folder_path
        except Exception:
            return None

    def status_folder_path(self, book_name: str, book_id: str, save_dir: str = None) -> Path:
        """生成书籍专属状态文件路径"""
        if not save_dir:
            save_dir = self.default_save_dir
        # 清理非法字符确保文件名安全
        safe_book_id = re.sub(r"[^a-zA-Z0-9_]", "_", book_id)
        self.folder_path = Path(save_dir) / f"{safe_book_id}_{book_name}"
        self.folder_path.mkdir(parents=True, exist_ok=True)
        return self.folder_path


class GlobalContext(object):
    def __init__(self, debug: bool, config: BaseConfig = Config):
        """创建公用Context"""
        global _log_system, _config
        _log_system = LogSystem(debug=debug)
        try:
            _config = config.load()
        except ConfigError as e:
            _log_system.logger.error(f"配置操作失败: {str(e)}")

    @staticmethod
    def get_logger() -> Logger:
        """获取logger"""
        return _log_system.logger

    @staticmethod
    def get_log_system() -> LogSystem:
        """获取log_system"""
        return _log_system

    @staticmethod
    def get_config() -> Config:
        """获取Config"""
        return _config
