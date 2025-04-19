import os
import re
import time
import urllib.parse
from pathlib import Path
from typing import Dict, List

import requests

from ..base_system.context import GlobalContext
from .downloader import (
    FqReq,
    _ensure_fresh_iid,
    FqVariable,
)
from .get_version_code import GetVersionCode

__all__ = [
    "fetch_chapter_for_epub",
]

API_BASE_URL = "https://api5-normal-sinfonlineb.fqnovel.com/reading"
AID = "1967"

# ---------------------------------------------------------------------------
# 正则
# ---------------------------------------------------------------------------
_IMG_BLOCK_RE = re.compile(
    r"<div[^>]*data-fanqie-type=\"image\"[^>]*>.*?</div>",
    re.DOTALL | re.IGNORECASE,
)
_IMG_TAG_RE = re.compile(r"<img[^>]*src=\"([^\"]+)\"", re.IGNORECASE)

# ---------------------------------------------------------------------------
# 内部下载
# ---------------------------------------------------------------------------


def _download_image(url: str, save_path: Path, *, timeout: int = 15) -> None:
    if save_path.exists():
        return
    clean_url = url.replace("&amp;", "&")
    with requests.get(clean_url, stream=True, timeout=timeout, verify=False) as r:
        r.raise_for_status()
        save_path.write_bytes(r.content)


# ---------------------------------------------------------------------------
# 对外主函数
# ---------------------------------------------------------------------------


def fetch_chapter_for_epub(chapter_ids: str) -> Dict:
    """
    获取章节正文（epub 专用），支持传入以逗号分隔的多个章节 ID。
    返回结构: {"data": {id: {...}, ...}}
    """
    cfg = GlobalContext.get_config()
    _ensure_fresh_iid()
    # chapter_ids 已经是 'id1,id2,...' 格式
    ids_str = chapter_ids

    # 初始化请求变量
    fq_var = FqVariable(
        install_id=cfg.iid,
        server_device_id=str(int(cfg.iid) - 4096),
        aid=AID,
        update_version_code=GetVersionCode.get(),
    )
    fq = FqReq(fq_var)
    try:
        # step1: 干净正文 → 批量请求，解密
        clean = fq._batch_fetch(ids_str)
        clean = fq._decrypt_contents(clean)

        # 获取所有章节 ID 列表
        id_list = list(clean.get("data", {}).keys())
        # 如果非 epub 或没有任何占位 div，则直接返回完整 clean
        if cfg.novel_format.lower() != "epub":
            return clean
        # 检查是否至少一个章节含占位
        dirty_needed = [
            cid
            for cid in id_list
            if _IMG_BLOCK_RE.search(clean["data"][cid]["content"])
        ]
        if not dirty_needed:
            return clean

        # step2: 含插画请求（batch_full + version_code）
        params = {
            "item_ids": ids_str,
            "version_code": fq_var.update_version_code,
            "update_version_code": fq_var.update_version_code,
            "aid": fq_var.aid,
            "key_register_ts": "0",
        }
        headers = {"Cookie": f"install_id={fq_var.install_id}"}
        r = fq.session.get(
            f"{API_BASE_URL}/reader/batch_full/v",
            params=params,
            headers=headers,
            timeout=15,
            verify=False,
        )
        r.raise_for_status()
        dirty_raw = r.json()
        dirty = fq._decrypt_contents(dirty_raw)

        # step3: 针对每个需要插图的章节依次下载并替换
        images_dir = Path(cfg.get_status_folder_path) / "images"
        images_dir.mkdir(parents=True, exist_ok=True)

        for cid in dirty_needed:
            clean_html = clean["data"][cid]["content"]
            dirty_html = dirty["data"][cid]["content"]
            # 提取该章节所有图片 URL
            img_urls = _IMG_TAG_RE.findall(dirty_html)
            # 逐张下载
            for idx, url in enumerate(img_urls, 1):
                save_path = images_dir / f"{cid}_{idx}.jpeg"
                _download_image(url, save_path)
            # 用本地 <img> 替换占位 <div>
            counter = 0

            def repl(m):
                nonlocal counter
                counter += 1
                return f'<img src="images/{cid}_{counter}.jpeg" alt="" />'

            new_html = _IMG_BLOCK_RE.sub(repl, clean_html)
            clean["data"][cid]["content"] = new_html

        return clean
    finally:
        fq.close()
