# -------------------------------
# config.py - 配置管理模块
# 职责：集中管理所有配置项和路径
# -------------------------------
import os
import re
import shutil
from logging import Logger
from pathlib import Path
from typing import Optional, List, Dict

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
    enable_audiobook: bool = Field(
        default=False, description="是否使用 Edge TTS 生成有声小说"
    )
    audiobook_voice: str = Field(
        default="zh-CN-XiaoxiaoNeural", description="Edge TTS 发音人"
    )
    audiobook_rate: str = Field(
        default="+0%", description="Edge TTS 语速调整，例如 +0%、-10%"
    )
    audiobook_volume: str = Field(
        default="+0%", description="Edge TTS 音量调整，例如 +0%、-10%"
    )
    audiobook_pitch: str = Field(
        default="", description="Edge TTS 音调调整，例如 +2Hz 或 -1st (留空表示默认)"
    )
    audiobook_format: str = Field(
        default="mp3", description="有声小说输出格式，可选 mp3 或 wav"
    )
    audiobook_concurrency: int = Field(
        default=24, description="Edge TTS 有声小说并发生成的最大章节数"
    )

    # 路径配置
    save_path: str = Field(default="", description="保存路径")

    # API配置
    use_official_api: bool = Field(default=True, description="使用官方API")
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
    download_comment_avatars: bool = Field(
        default=True, description="是否下载评论区头像"
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
    # 段落首行缩进 (em)；0 表示不缩进
    first_line_indent_em: float = Field(
        default=2.0, description="EPUB 段落首行缩进 em 数 (0 表示无缩进)"
    )

    # 媒体体积与数量控制
    media_limit_per_chapter: int = Field(
        default=0, description="每章最多下载的媒体数（0 表示不限制）"
    )
    media_max_dimension_px: int = Field(
        default=1280, description="图片最长边像素上限，>0 时会缩放并转成 JPEG 以节省体积"
    )
    media_total_limit_mb: int = Field(
        default=0, description="本次会话媒体总下载上限（MB，0 表示不限制）"
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

    _last_status_folder_was_new: bool = False
    _last_status_folder_claimed: bool = False
    _last_status_folder_path: Optional[Path] = None
    _status_folder_registry: Optional[List[Dict[str, object]]] = None

    # ---------------- 状态目录注册表 ----------------
    def _get_status_registry(self) -> List[Dict[str, object]]:
        registry = getattr(self, "_status_folder_registry", None)
        if registry is None:
            registry = []
            self._status_folder_registry = registry
        return registry

    def _register_status_folder(self, path: Path, existed_before: bool):
        registry = self._get_status_registry()
        path_obj = Path(path)
        path_key = str(path_obj)
        entry = None
        for item in registry:
            if Path(item.get("path")) == path_obj:
                entry = item
                break
        prior_new_unclaimed = bool(entry and entry.get("is_new") and not entry.get("claimed"))
        is_new_this_session = prior_new_unclaimed or (not existed_before)
        if entry:
            entry["is_new"] = bool(entry.get("is_new")) or is_new_this_session
            if is_new_this_session:
                entry["claimed"] = False
        else:
            registry.append({
                "path": path_key,
                "is_new": is_new_this_session,
                "claimed": False,
            })
        self._last_status_folder_path = path_obj
        self._last_status_folder_was_new = is_new_this_session
        self._last_status_folder_claimed = False

    def mark_status_folder_claimed(self, path: Path):
        registry = self._get_status_registry()
        path_obj = Path(path)
        for entry in registry:
            if Path(entry.get("path")) == path_obj:
                entry["claimed"] = True
        if self._last_status_folder_path and Path(self._last_status_folder_path) == path_obj:
            self._last_status_folder_claimed = True

    def mark_status_folder_removed(self, path: Path):
        registry = self._get_status_registry()
        path_obj = Path(path)
        self._status_folder_registry = [
            entry for entry in registry if Path(entry.get("path")) != path_obj
        ]
        if self._last_status_folder_path and Path(self._last_status_folder_path) == path_obj:
            self._last_status_folder_path = None
            self._last_status_folder_was_new = False
            self._last_status_folder_claimed = False

    def pending_unclaimed_status_folders(self, exclude: Optional[Path] = None) -> List[Path]:
        registry = self._get_status_registry()
        exclude_path = Path(exclude) if exclude else None
        pending: List[Path] = []
        for entry in registry:
            path_val = entry.get("path")
            if not path_val:
                continue
            path_obj = Path(path_val)
            if exclude_path and path_obj == exclude_path:
                continue
            if entry.get("is_new") and not entry.get("claimed"):
                pending.append(path_obj)
        return pending

    def status_folder_was_created_this_session(self, path: Path) -> bool:
        registry = self._get_status_registry()
        path_obj = Path(path)
        for entry in registry:
            if Path(entry.get("path")) == path_obj:
                return bool(entry.get("is_new"))
        if self._last_status_folder_path and Path(self._last_status_folder_path) == path_obj:
            return bool(self._last_status_folder_was_new)
        return False

    def status_folder_path(self, book_name: str, book_id: str, save_dir: str = None) -> Path:
        """生成书籍专属状态文件路径"""
        if not save_dir:
            save_dir = self.default_save_dir
        previous_path = getattr(self, "_last_status_folder_path", None)
        previous_new = bool(getattr(self, "_last_status_folder_was_new", False))
        previous_claimed = bool(getattr(self, "_last_status_folder_claimed", False))
        # 清理非法字符确保文件名安全
        safe_book_id = re.sub(r"[^a-zA-Z0-9_]", "_", book_id)
        safe_book_name = self.safe_fs_name(book_name)
        path = Path(save_dir) / f"{safe_book_id}_{safe_book_name}"
        existed_before = path.exists()
        path.mkdir(parents=True, exist_ok=True)
        if previous_path and previous_new and not previous_claimed:
            try:
                old_path = Path(previous_path)
                if old_path != path and old_path.exists():
                    shutil.rmtree(old_path)
            except Exception:
                pass
        self.folder_path = path
        self._register_status_folder(path, existed_before)
        return self.folder_path

    # ---------------- 文件名安全工具 ----------------
    @staticmethod
    def safe_fs_name(name: str, replacement: str = "_", max_len: int = 120) -> str:
        """将任意字符串转换为跨平台安全的文件/目录名。

        规则:
          1. 替换 Windows 非法字符 <>:"/\\|?* 为 replacement
          2. 去除控制字符 (0-31)
          3. 去除前后空格与点 (Windows 不允许以点/空格结尾)
          4. 处理保留字 (CON, PRN, AUX, NUL, COM1.., LPT1..)
          5. 限制长度 (默认 120) 保留扩展名空间
        """
        if not isinstance(name, str):
            name = str(name)
        # 替换非法字符
        name = re.sub(r'[<>:"/\\|?*]', replacement, name)
        # 去除控制字符
        name = ''.join(ch for ch in name if 31 < ord(ch) < 127 or ord(ch) >= 128)
        # Windows 结尾的点和空格无效
        name = name.strip().rstrip('. ')
        if not name:
            name = 'unnamed'
        # 保留字处理
        RESERVED = {"CON","PRN","AUX","NUL","COM1","COM2","COM3","COM4","COM5","COM6","COM7","COM8","COM9","LPT1","LPT2","LPT3","LPT4","LPT5","LPT6","LPT7","LPT8","LPT9"}
        upper = name.upper()
        if upper in RESERVED:
            name = f"_{name}"
        # 长度限制
        if len(name) > max_len:
            name = name[:max_len].rstrip('. ')
        return name


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
