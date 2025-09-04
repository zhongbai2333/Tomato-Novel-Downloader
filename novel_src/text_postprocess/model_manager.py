import os
from pathlib import Path
from typing import Optional


class ModelManager:
    """管理本地模型资产与自动下载（占位：依赖 huggingface_hub 时再扩展）。"""

    @staticmethod
    def get_models_dir(default_base: Path, override: Optional[str]) -> Path:
        if override:
            p = Path(override)
        else:
            p = Path(default_base)
        p.mkdir(parents=True, exist_ok=True)
        return p

    @staticmethod
    def ensure_local_file(models_dir: Path, repo_id: str, filename: str, auto_download: bool, logger) -> Optional[Path]:
        """确保某个 repo/file 存在本地；如果不存在并允许自动下载，则尝试下载。
        为避免强依赖，此处仅占位：如需下载会提示用户安装 huggingface_hub。
        """
        if not repo_id or not filename:
            return None
        local_path = models_dir / repo_id.replace("/", "__") / filename
        if local_path.exists():
            return local_path
        if not auto_download:
            return None

        try:
            import importlib
            # 若配置了镜像站，则设置 HF_ENDPOINT 环境变量
            try:
                from ..base_system.context import GlobalContext
                cfg = GlobalContext.get_config()
                hf_endpoint = getattr(cfg, "hf_endpoint", "").strip()
                if hf_endpoint:
                    # 允许传入裸域名或完整 URL
                    os.environ["HF_ENDPOINT"] = hf_endpoint if hf_endpoint.startswith("http") else f"https://{hf_endpoint}"
            except Exception:
                pass
            hf = importlib.import_module("huggingface_hub")
            hf_hub_download = getattr(hf, "hf_hub_download")
        except Exception:
            logger.warning("缺少 huggingface_hub，无法自动下载模型，请安装后重试或手动放置 GGUF 文件。")
            return None

        local_path.parent.mkdir(parents=True, exist_ok=True)
        try:
            fp = hf_hub_download(repo_id=repo_id, filename=filename, local_dir=str(local_path.parent))
            return Path(fp)
        except Exception as e:
            logger.warning(f"模型下载失败：{repo_id}/{filename}，{e}")
            return None
