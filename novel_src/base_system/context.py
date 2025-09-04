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

    # 文本后处理（演示）——统一在解析阶段执行
    enable_postprocess: bool = Field(default=False, description="启用文本清洗与标点恢复（演示版，解析阶段执行）")
    pp_punct_density_threshold: float = Field(default=0.01, description="触发标点恢复的标点密度阈值，如 0.01")
    pp_long_sentence_min_len: int = Field(default=60, description="触发标点恢复的最小行长度")
    pp_insert_chunk: int = Field(default=30, description="演示级标点恢复：每多少字插入一个逗号")

    # HuggingFace 镜像站配置（用于自动下载 GGUF 模型）
    hf_endpoint: str = Field(
        default="https://hf-mirror.com",
        description="Hugging Face 镜像地址或域名，如 https://hf-mirror.com 或 hf-cdn.sufy.com",
    )

    # 本地 LLM 两级管线（可选，基于 llama.cpp GGUF）
    enable_llm_pipeline: bool = Field(default=False, description="启用两级 LLM 管线（A 检测 0.6B + B 精修 3B）")
    llm_auto_model_download: bool = Field(default=True, description="缺失时自动从 Hugging Face 下载 GGUF 模型")
    llm_models_dir: str = Field(default="", description="模型缓存目录（默认使用 status_folder_path/models）")
    llm_detector_repo: str = Field(
        default="Qwen/Qwen3-0.6B-GGUF",
        description="A 检测模型的 HuggingFace repo id（例如 user/repo）",
    )
    llm_detector_filename: str = Field(
        default="Qwen3-0.6B-Q8_0.gguf",
        description="A 检测模型 GGUF 文件名（例如 model.Q4_K_M.gguf）",
    )
    llm_refiner_repo: str = Field(
        default="Qwen/Qwen3-4B-GGUF", description="B 精修模型的 HuggingFace repo id"
    )
    llm_refiner_filename: str = Field(
        default="Qwen3-4B-Q5_K_M.gguf", description="B 精修模型 GGUF 文件名"
    )
    llm_detector_threshold: float = Field(default=0.55, description="A 阶段转发到 B 的置信度阈值")
    llm_max_forward_ratio: float = Field(default=0.2, description="允许转发到 B 的最大片段比例上限（0~1）")
    llm_ctx_window: int = Field(default=2048, description="llama.cpp 上下文窗口")
    llm_max_tokens: int = Field(default=128, description="精修阶段生成上限 tokens")
    llm_temperature: float = Field(default=0.1, description="精修阶段温度，建议低温抑制改写")
    # 性能与硬件相关
    llm_n_threads: int = Field(default=0, description="llama.cpp 推理线程数，0 表示自动 (os.cpu_count())")
    llm_use_gpu_auto: bool = Field(default=True, description="自动尝试 GPU 加速（需要 GPU 构建的 llama.cpp）")
    llm_n_gpu_layers: int = Field(default=-1, description="GPU 加速层数：-1 表示尽可能多，0 表示仅 CPU")
    llm_debug_verbose: bool = Field(default=False, description="启用详细 LLM 调试日志（每次推理起止），默认关闭")

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
