# -------------------------------
# config.py - 配置管理模块
# 职责：集中管理所有配置项和路径
# -------------------------------
import os
import re
from logging import Logger
from pathlib import Path

from .storage_system import BaseConfig, Field, ConfigError
from .log_system import LogSystem


class Config(BaseConfig):
    """Config 配置文件"""

    # 程序配置
    old_cli: bool = Field(default=False, description="是否使用老版本命令行界面")

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
        default="epub", description="保存小说格式, 可选: [txt, epub]"
    )
    bulk_files: bool = Field(default=False, description="是否以散装形式保存小说")
    auto_clear_dump: bool = Field(default=True, description="是否自动清理缓存文件")

    # 路径配置
    save_path: str = Field(default="", description="保存路径")

    # API配置
    use_official_api: bool = Field(default=True, description="使用官方API")
    use_helloplhm_qwq_api: bool = Field(
        default=False, description="使用helloplhm_qwq API"
    )
    api_endpoints: list = Field(
        default=[],
        description="API列表",
    )

    # 段评配置
    enable_segment_comments: bool = Field(
        default=False, description="是否下载段评（段落评论）"
    )
    segment_comments_top_n: int = Field(
        default=10, description="每段最多保存的评论数"
    )
    segment_comments_workers: int = Field(
        default=32, description="段评抓取的并发线程数（每章内）"
    )
    # 段评媒体配置
    download_comment_images: bool = Field(
        default=True, description="是否下载评论区图片（不含头像）"
    )
    media_download_workers: int = Field(
        default=8, description="评论图片/头像下载并发线程数"
    )
    blocked_media_domains: list = Field(
        default=["p-passport-sign.bytedance.net"],
        description="拒绝下载的图片域名（包含匹配）",
    )
    # 图片统一格式配置（新版：改为强制转 JPEG；旧字段 force_convert_images_to_webp 已弃用）
    force_convert_images_to_jpeg: bool = Field(
        default=False, description="是否强制将所有下载图片转码为 JPEG（最大兼容性）"
    )
    # JPEG 优先下载/转码
    jpeg_retry_convert: bool = Field(
        default=True, description="若返回非 JPEG 且可解码则转码为 JPEG 保存"
    )
    jpeg_quality: int = Field(
        default=90, description="JPEG 转码质量 (0-100)"
    )
    # HEIC/HEIF 处理
    convert_heic_to_jpeg: bool = Field(
        default=True, description="检测到 HEIC/HEIF 时转码为 JPEG（需 pillow-heif）"
    )
    keep_heic_original: bool = Field(
        default=False, description="为 True 时即便无法转码也保留 .heic/.heif 原文件（阅读器可能不显示）"
    )
    # 自定义章节模板
    enable_chapter_template: bool = Field(
        default=False, description="是否启用自定义章节模板"
    )
    chapter_template_file: str = Field(
        default="chapter_template.txt", description="章节模板文件路径"
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
