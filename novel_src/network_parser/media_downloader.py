import os, hashlib, threading, requests
from pathlib import Path
from urllib.parse import urlparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from typing import List, Tuple


class MediaDownloader:
    """评论区媒体下载器：收集图片/头像并并发下载到状态目录。

    依赖：
    - config: 读取下载相关配置（超时、重试、格式转换、并发度、屏蔽域名等）
    - logger: 输出日志
    - status_folder: 保存 images 子目录
    """

    def __init__(self, config, logger, status_folder: Path):
        self.config = config
        self.logger = logger
        self.status_folder: Path = status_folder
        # 去重/并发控制
        self._seen_urls: set[str] = set()
        self._inflight: set[str] = set()
        self._dl_lock = threading.Lock()
        # 会话级体积统计（字节）
        self._session_bytes = 0

    # --------- 供外部使用的工具 ---------
    def get_cached_media_filename(self, url: str) -> str | None:
        """在本地缓存中查找与 URL 对应的文件名。

        策略：
        - 同时计算原始URL与规范化URL的 sha1 作为“基名”；
        - 在 images 目录下，以这两个基名分别尝试匹配任意支持的扩展名；
        - 命中即返回对应文件名，避免重复下载。
        """
        try:
            img_dir = self.status_folder / "images"
            if not img_dir.exists():
                return None
            exts = [
                ".jpg",
                ".jpeg",
                ".png",
                ".gif",
                ".webp",
                ".avif",
                ".heic",
            ]
            names = []
            for u in {url, self._normalize_media_url(url)}:
                try:
                    name = hashlib.sha1(u.encode("utf-8")).hexdigest()
                    names.append(name)
                except Exception:
                    continue
            # 先尝试按路径扩展快速命中，再回退到“任意扩展”匹配
            fast_candidates = []
            for u in {url, self._normalize_media_url(url)}:
                try:
                    p = urlparse(u)
                    ext = os.path.splitext((p.path or ""))[1].lower()
                    if ext not in exts:
                        ext = ".jpg"
                    fast_candidates.append((hashlib.sha1(u.encode("utf-8")).hexdigest(), ext))
                except Exception:
                    continue
            for name, ext in fast_candidates:
                fn = f"{name}{ext}"
                if (img_dir / fn).exists():
                    return fn
            # 扫描任意扩展
            for name in names:
                for ext in exts:
                    fn = f"{name}{ext}"
                    if (img_dir / fn).exists():
                        return fn
            return None
        except Exception:
            return None

    # --------- 收集与下载入口 ---------
    def prefetch(self, seg_data: dict, top_n: int = 10) -> Tuple[int, int, int, int]:
        """并发预取段评中的图片与头像，仅处理每段前 top_n 条评论。

        返回 (unique_count, completed, img_cnt, avatar_cnt)
        """
        try:
            allow_images = bool(getattr(self.config, "download_comment_images", True))
            allow_avatars = bool(getattr(self.config, "download_comment_avatars", True))
            paras = seg_data.get("paras") if isinstance(seg_data, dict) else None
            if not isinstance(paras, dict):
                return (0, 0, 0, 0)

            urls: List[str] = []
            img_cnt = 0
            avatar_cnt = 0
            for _k, _meta in paras.items():
                detail = (_meta or {}).get("detail") or {}
                lst = detail.get("data_list") if isinstance(detail, dict) else None
                if not isinstance(lst, list) or not lst:
                    continue
                for item in lst[: max(0, int(top_n))]:
                    # 评论图片
                    if allow_images:
                        for u in self._extract_image_urls(item):
                            urls.append(u)
                            img_cnt += 1
                    # 头像
                    if allow_avatars:
                        av = self._extract_avatar_url(item)
                        if av:
                            urls.append(av)
                            avatar_cnt += 1
            # 去重
            unique = []
            seen = set()
            for u in urls:
                if u not in seen:
                    seen.add(u)
                    unique.append(u)
            if not unique:
                try:
                    self.logger.debug(
                        f"[媒体] 章节 {seg_data.get('chapter_id')} 无可下载资源 (图片={img_cnt}, 头像={avatar_cnt})"
                    )
                except Exception:
                    pass
                return (0, 0, img_cnt, avatar_cnt)

            try:
                workers = int(getattr(self.config, "media_download_workers", 8))
            except Exception:
                workers = 4
            try:
                self.logger.debug(
                    f"[媒体] 章节 {seg_data.get('chapter_id')} 收集图片={img_cnt} 头像={avatar_cnt} 去重后={len(unique)}"
                )
            except Exception:
                pass

            # 每章上限：0 表示不限制
            try:
                per_chapter_cap = int(getattr(self.config, "media_limit_per_chapter", 0))
            except Exception:
                per_chapter_cap = 0
            capped_unique = unique[:per_chapter_cap] if per_chapter_cap and per_chapter_cap > 0 else unique

            completed = 0
            with ThreadPoolExecutor(max_workers=max(1, workers)) as ex:
                futures = [ex.submit(self._download_comment_image, u) for u in capped_unique]
                for f in as_completed(futures):
                    try:
                        _ = f.result()
                    except Exception:
                        _ = None
                    completed += 1

            try:
                self.logger.info(
                    f"[媒体] 章节 {seg_data.get('chapter_id')} 段评媒体下载完成: 计划={len(unique)} 实际完成={completed} (图片={img_cnt}, 头像={avatar_cnt})"
                )
            except Exception:
                pass

            return (len(capped_unique), completed, img_cnt, avatar_cnt)
        except Exception:
            return (0, 0, 0, 0)

    # --------- 内部工具 ---------
    @staticmethod
    def _extract_image_urls(obj) -> List[str]:
        urls: List[str] = []

        def _add(s: str | None):
            if isinstance(s, str) and s.startswith("http"):
                urls.append(s)

        try:
            if not isinstance(obj, dict):
                return []
            content = (
                ((obj.get("comment") or {}).get("common") or {}).get("content")
                if isinstance(obj.get("comment"), dict)
                else None
            )
            if not isinstance(content, dict):
                content = (
                    ((obj.get("common") or {}).get("content"))
                    if isinstance(obj.get("common"), dict)
                    else obj.get("content")
                )
            if isinstance(content, dict):
                idl = content.get("image_data_list")
                if isinstance(idl, dict):
                    lst = idl.get("image_data")
                    if isinstance(lst, list):
                        for it in lst:
                            if isinstance(it, dict):
                                _add(
                                    it.get("expand_web_url")
                                    or it.get("web_uri")
                                    or it.get("url")
                                    or it.get("src")
                                )
            # 去重
            dedup = []
            seen = set()
            for u in urls:
                if u not in seen:
                    seen.add(u)
                    dedup.append(u)
            return dedup
        except Exception:
            return []

    @staticmethod
    def _extract_avatar_url(item) -> str | None:
        try:
            # 优先固定路径提取
            for key in [
                "comment.common.user_info.base_info.user_avatar",
                "common.user_info.base_info.user_avatar",
                "user_info.base_info.user_avatar",
                "comment.user_info.base_info.user_avatar",
                "user.avatar",
                "avatar_url",
                "avatar",
            ]:
                cur = item
                ok = True
                for part in key.split("."):
                    if isinstance(cur, dict):
                        cur = cur.get(part)
                    else:
                        ok = False
                        break
                if ok and isinstance(cur, str) and cur.startswith("http"):
                    return cur
        except Exception:
            pass
        return None

    def _normalize_media_url(self, url: str) -> str:
        try:
            p = urlparse(url)
            if not p.scheme or not p.netloc:
                return url
            return f"{p.scheme.lower()}://{p.netloc}{p.path}"
        except Exception:
            return url

    def _download_comment_image(self, url: str) -> str | None:
        """下载图片到状态目录 images 下，返回文件名（不含路径），失败返回 None。包含超时和重试。"""
        try:
            canon_url = self._normalize_media_url(url)
            with self._dl_lock:
                if canon_url in self._seen_urls or canon_url in self._inflight:
                    return None
                self._inflight.add(canon_url)
            # 屏蔽域名
            try:
                blocked = list(getattr(self.config, "blocked_media_domains", []))
            except Exception:
                blocked = []
            if any(b and (b in url) for b in blocked):
                self.logger.debug(f"跳过被屏蔽域名的图片: {url}")
                return None
            img_dir: Path = self.status_folder / "images"
            img_dir.mkdir(parents=True, exist_ok=True)
            # 计算原始 & 规范化 两套文件名，优先复用任一已存在；
            # 同时也检查“同哈希任意扩展”的已存在文件（处理上次已转换为 PNG/WEBP/AVIF/HEIC 的情况）。
            parsed_canon = urlparse(canon_url)
            path_canon = parsed_canon.path or ""
            ext_canon = os.path.splitext(path_canon)[1].lower()
            if ext_canon not in [".jpg", ".jpeg", ".png", ".gif", ".webp"]:
                ext_canon = ""
            if not ext_canon:
                ext_canon = ".jpg"
            canon_name = hashlib.sha1(canon_url.encode("utf-8")).hexdigest()
            canon_file = f"{canon_name}{ext_canon}"
            canon_path = img_dir / canon_file

            # 原始 URL 哈希（兼容旧版本已缓存的基于原始URL+query文件）
            parsed_orig = urlparse(url)
            path_orig = parsed_orig.path or ""
            ext_orig = os.path.splitext(path_orig)[1].lower()
            if ext_orig not in [".jpg", ".jpeg", ".png", ".gif", ".webp"]:
                ext_orig = ""
            if not ext_orig:
                ext_orig = ".jpg"
            orig_name = hashlib.sha1(url.encode("utf-8")).hexdigest()
            orig_file = f"{orig_name}{ext_orig}"
            orig_path = img_dir / orig_file

            # 命中任一缓存则直接返回
            if canon_path.exists():
                with self._dl_lock:
                    self._seen_urls.add(canon_url)
                try:
                    self.logger.debug(f"[媒体缓存] 命中规范化文件 {canon_file}")
                except Exception:
                    pass
                return canon_file
            if orig_path.exists():
                with self._dl_lock:
                    self._seen_urls.add(canon_url)
                try:
                    self.logger.debug(f"[媒体缓存] 复用旧文件 {orig_file} (来源原始URL哈希)")
                except Exception:
                    pass
                return orig_file

            # 进一步：检查同哈希的任意扩展文件是否已存在（如上次转成 .png/.webp/.avif/.heic）
            try:
                any_exts = [
                    ".jpg",
                    ".jpeg",
                    ".png",
                    ".gif",
                    ".webp",
                    ".avif",
                    ".heic",
                ]
                for name in (canon_name, orig_name):
                    for ext in any_exts:
                        fn = f"{name}{ext}"
                        if (img_dir / fn).exists():
                            with self._dl_lock:
                                self._seen_urls.add(canon_url)
                            try:
                                self.logger.debug(f"[媒体缓存] 命中同哈希缓存 {fn}")
                            except Exception:
                                pass
                            return fn
            except Exception:
                pass

            # 重新准备下载所需的标准变量
            parsed = urlparse(canon_url)
            path = parsed.path or ""
            ext = os.path.splitext(path)[1].lower()
            if ext not in [".jpg", ".jpeg", ".png", ".gif", ".webp"]:
                ext = ""
            name = canon_name
            if not ext:
                ext = ".jpg"
            file_name = f"{name}{ext}"
            out_path = img_dir / file_name

            accept_hdr = "image/jpeg,image/jpg,image/png,image/gif,*/*;q=0.8"
            headers = {
                "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
                "Accept": accept_hdr,
                "Referer": f"{parsed.scheme}://{parsed.netloc}/" if parsed.scheme and parsed.netloc else canon_url,
            }

            # 配置化超时与重试
            try:
                timeout = float(getattr(self.config, "media_download_timeout", 5.0))
            except Exception:
                timeout = 5.0
            try:
                max_retries = int(getattr(self.config, "media_download_retries", 4))
            except Exception:
                max_retries = 4
            try:
                backoff = float(getattr(self.config, "media_retry_backoff", 0.8))
            except Exception:
                backoff = 0.8

            import time as _t

            attempt = 0
            last_err = None
            while attempt < max_retries:
                try:
                    self.logger.debug(
                        f"下载评论图片: {url} (尝试{attempt+1}/{max_retries})"
                    )
                    resp = requests.get(url, headers=headers, timeout=timeout)
                    sc = resp.status_code
                    if sc == 200 and resp.content:
                        # 会话总量上限：0 表示不限制
                        try:
                            total_cap_mb = int(getattr(self.config, "media_total_limit_mb", 0))
                        except Exception:
                            total_cap_mb = 0
                        if total_cap_mb and total_cap_mb > 0:
                            with self._dl_lock:
                                if (self._session_bytes + len(resp.content)) > total_cap_mb * 1024 * 1024:
                                    try:
                                        self.logger.info("[媒体] 已达会话媒体总量上限，跳过后续媒体下载。")
                                    except Exception:
                                        pass
                                    return None
                        # ---- 内容类型与扩展处理 ----
                        ctype = resp.headers.get("Content-Type", "").lower()
                        url_lower = url.lower()
                        heic_url_markers = [
                            ".heic",
                            "/heic",
                            "format=heic",
                            "heic=1",
                            "image/heic",
                            "x-oss-process=image/format,heic",
                        ]
                        is_heic_hint = any(m in url_lower for m in heic_url_markers) or (
                            "heic" in ctype or "heif" in ctype
                        )
                        if not is_heic_hint:
                            try:
                                head_bytes = resp.content[:512]
                                pos = head_bytes.find(b"ftyp")
                                if pos != -1:
                                    brand_window = head_bytes[pos + 4 : pos + 16]
                                    if any(b in brand_window for b in [b"heic", b"heif", b"mif1", b"msf1"]):
                                        is_heic_hint = True
                            except Exception:
                                pass
                        debug_formats = getattr(self.config, "log_image_format_debug", False)
                        if is_heic_hint and debug_formats:
                            self.logger.debug(f"检测到 HEIC 格式: url={url} ctype={ctype}")
                        if ext == ".jpg" and "png" in ctype:
                            file_name2 = f"{name}.png"
                            out_path2 = img_dir / file_name2
                        elif ext == ".jpg" and "gif" in ctype:
                            file_name2 = f"{name}.gif"
                            out_path2 = img_dir / file_name2
                        elif ext == ".jpg" and "webp" in ctype:
                            file_name2 = f"{name}.webp"
                            out_path2 = img_dir / file_name2
                        elif ext == ".jpg" and "avif" in ctype:
                            file_name2 = f"{name}.avif"
                            out_path2 = img_dir / file_name2
                        elif is_heic_hint:
                            file_name2 = f"{name}.heic"
                            out_path2 = img_dir / file_name2
                        else:
                            file_name2 = file_name
                            out_path2 = out_path
                        data_bytes = resp.content
                        # 强制转 JPEG / 兼容转换
                        try:
                            force_jpeg = bool(getattr(self.config, "force_convert_images_to_jpeg", False)) or bool(
                                getattr(self.config, "force_convert_images_to_webp", False)
                            )
                            need_heic_convert = is_heic_hint and getattr(self.config, "convert_heic_to_jpeg", True)
                            if force_jpeg or need_heic_convert:
                                from io import BytesIO

                                buf_in = BytesIO(data_bytes)
                                converted = False
                                try:
                                    import pillow_heif  # type: ignore

                                    pillow_heif.register_heif_opener()
                                except Exception:
                                    pass
                                try:
                                    from PIL import Image

                                    with Image.open(buf_in) as im:
                                        im = im.convert("RGB")
                                        qj = int(getattr(self.config, "jpeg_quality", 90))
                                        buf_out = BytesIO()
                                        im.save(buf_out, format="JPEG", quality=max(1, min(100, qj)))
                                        data_bytes = buf_out.getvalue()
                                        file_name2 = f"{name}.jpg"
                                        out_path2 = img_dir / file_name2
                                        converted = True
                                except Exception:
                                    converted = False
                                if is_heic_hint and not converted:
                                    if not getattr(self.config, "keep_heic_original", False):
                                        if debug_formats:
                                            self.logger.debug(f"HEIC 转码失败已丢弃: url={url}")
                                        return None
                                    else:
                                        if not file_name2.endswith(".heic"):
                                            file_name2 = f"{name}.heic"
                                            out_path2 = img_dir / file_name2
                            elif getattr(self.config, "jpeg_retry_convert", True) and not (
                                "jpeg" in ctype or file_name2.endswith(".jpg") or file_name2.endswith(".jpeg")
                            ):
                                from io import BytesIO
                                from PIL import Image

                                buf_in = BytesIO(data_bytes)
                                try:
                                    with Image.open(buf_in) as im:
                                        im = im.convert("RGB")
                                        qj = int(getattr(self.config, "jpeg_quality", 90))
                                        buf_out = BytesIO()
                                        im.save(buf_out, format="JPEG", quality=max(1, min(100, qj)))
                                        data_bytes = buf_out.getvalue()
                                        file_name2 = f"{name}.jpg"
                                        out_path2 = img_dir / file_name2
                                except Exception:
                                    pass
                        except Exception:
                            pass
                        # 尺寸压缩：最长边不超过 media_max_dimension_px，且转成 JPEG
                        try:
                            max_dim = int(getattr(self.config, "media_max_dimension_px", 1280))
                        except Exception:
                            max_dim = 1280
                        if isinstance(max_dim, int) and max_dim > 0:
                            try:
                                from io import BytesIO
                                from PIL import Image
                                with Image.open(BytesIO(data_bytes)) as im:
                                    im_format = "JPEG"
                                    im = im.convert("RGB")
                                    w, h = im.size
                                    scale = 1.0
                                    if max(w, h) > max_dim and max_dim > 0:
                                        scale = max_dim / float(max(w, h))
                                    if scale < 1.0:
                                        new_size = (int(w * scale), int(h * scale))
                                        im = im.resize(new_size)
                                    qj = int(getattr(self.config, "jpeg_quality", 85))
                                    buf_out = BytesIO()
                                    im.save(buf_out, format=im_format, quality=max(1, min(100, qj)))
                                    data_bytes = buf_out.getvalue()
                                    file_name2 = f"{name}.jpg"
                                    out_path2 = img_dir / file_name2
                            except Exception:
                                pass
                        with open(out_path2, "wb") as f:
                            f.write(data_bytes)
                        with self._dl_lock:
                            self._session_bytes += len(data_bytes)
                        # 标记 seen
                        try:
                            with self._dl_lock:
                                self._seen_urls.add(canon_url)
                        except Exception:
                            pass
                        return file_name2
                    # 可重试状态码
                    if sc in (429, 500, 502, 503, 504):
                        last_err = RuntimeError(f"status={sc}")
                    else:
                        self.logger.debug(f"图片下载失败 status={sc} url={url}")
                        return None
                except requests.Timeout as e:
                    last_err = e
                except Exception as e:
                    last_err = e
                attempt += 1
                _t.sleep(backoff * attempt)
            if last_err:
                try:
                    self.logger.debug(f"图片下载失败(重试耗尽): {last_err}")
                except Exception:
                    pass
            return None
        except Exception as e:
            try:
                self.logger.debug(f"图片下载失败: {e}")
            except Exception:
                pass
            return None
        finally:
            # 移除 inflight 标记
            try:
                with self._dl_lock:
                    self._inflight.discard(canon_url)
            except Exception:
                pass
