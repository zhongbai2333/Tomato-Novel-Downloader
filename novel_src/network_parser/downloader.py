# -------------------------------
# downloader.py - 核心下载模块
# 职责：实现多线程下载和任务管理
# -------------------------------
import time
import requests
import random
import threading
import signal
import queue
import urllib3
from concurrent.futures import ThreadPoolExecutor, as_completed
from tqdm import tqdm
from typing import List, Dict, Optional, Tuple

from .network import NetworkClient
from ..offical_tools.downloader import download_chapter_official, spawn_iid
from ..offical_tools.epub_downloader import fetch_chapter_for_epub
from ..book_parser.book_manager import BookManager
from ..book_parser.parser import ContentParser
from ..base_system.context import GlobalContext
from ..base_system.log_system import TqdmLoggingHandler

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
requests.packages.urllib3.disable_warnings()

class APIManager:
    def __init__(self, api_endpoints, config, network_status):
        self.api_queue = queue.Queue()
        self.config = config
        self.network_status = network_status
        for ep in api_endpoints:
            self.api_queue.put(ep)

    def get_api(self, timeout=1.0):
        """获取一个可用 API 端点（轮询冷却/失败状态）"""
        while True:
            try:
                ep = self.api_queue.get(timeout=timeout)
            except queue.Empty:
                time.sleep(0.05)
                continue
            st = self.network_status.get(ep, {})
            if time.time() < st.get("cooldown_until", 0):
                self.api_queue.put(ep)
                time.sleep(0.05)
                continue
            return ep

    def release_api(self, ep):
        """把 API 放回队列"""
        self.api_queue.put(ep)


class ChapterDownloader:
    """章节下载调度器"""

    def __init__(self, book_id: str, network_client: NetworkClient):
        self.book_id = book_id
        self.network = network_client
        self.logger = GlobalContext.get_logger()
        self.log_system = GlobalContext.get_log_system()
        self.config = GlobalContext.get_config()

        # 中断控制
        self._stop_event = threading.Event()
        self._orig_handler = signal.getsignal(signal.SIGINT)
        signal.signal(signal.SIGINT, self._handle_signal)

        self.api_manager = APIManager(
            api_endpoints=self.config.api_endpoints,
            config=self.config,
            network_status=self.network._api_status,
        )

    def _handle_signal(self, signum, frame):
        self.logger.warning("接收到 Ctrl-C，准备优雅退出...")
        self._stop_event.set()
        signal.signal(signal.SIGINT, self._orig_handler)

    def download_book(
        self,
        book_manager: BookManager,
        book_name: str,
        chapters: List[Dict],
    ) -> Dict[str, int]:
        """执行多线程下载任务（支持官方批量和非官方单章两种模式）"""
        # 官方 API 需要先 spawn_iid
        if self.config.use_official_api and not self.config.iid:
            spawn_iid()

        # 备份并关闭非 tqdm handler
        orig_handlers = self.logger.handlers.copy()
        for h in orig_handlers:
            if not isinstance(h, TqdmLoggingHandler):
                self.logger.removeHandler(h)

        results = {"success": 0, "failed": 0, "canceled": 0}

        # ============ 准备要下载的章节列表 & 分组 ============
        if self.config.use_official_api:
            # 官方批量：先把所有还未下载或下载失败的章节分成 10 一组
            to_download = [
                ch
                for ch in chapters
                if (ch["id"] not in book_manager.downloaded)
                or (book_manager.downloaded.get(ch["id"])[1] == "Error")
            ]
            # 按 10 章一组
            groups = [to_download[i : i + 10] for i in range(0, len(to_download), 10)]
            # 本次任务中实际要下载的章节总数
            tasks_count = len(to_download)
            max_workers = self.config.max_workers

            def get_submit(exe):
                # 把每组提交给线程池
                return {
                    exe.submit(self._download_official_batch, grp): grp
                    for grp in groups
                }

            desc = f"下载《{book_name}》（官方批量）"
        else:
            # 非官方：逐章下载
            to_download = [
                ch
                for ch in chapters
                if (ch["id"] not in book_manager.downloaded)
                or (book_manager.downloaded.get(ch["id"])[1] == "Error")
            ]
            tasks_count = len(to_download)
            max_workers = min(self.config.max_workers, len(self.config.api_endpoints))

            def get_submit(exe):
                return {exe.submit(self._download_single, ch): ch for ch in to_download}

            desc = f"下载《{book_name}》"

        # ============ 并发执行 ============

        with ThreadPoolExecutor(max_workers=max_workers) as exe:
            futures = get_submit(exe)
            with tqdm(total=len(futures), desc=desc) as pbar:
                self.log_system.enable_tqdm_handler(pbar)

                for future in as_completed(futures):
                    if self._stop_event.is_set():
                        # 优雅取消剩余任务
                        for f in futures:
                            if not f.done():
                                f.cancel()
                        break

                    task = futures[future]
                    try:
                        if self.config.use_official_api:
                            batch_out: Dict[str, Tuple[str, str]] = future.result()
                            for cid, (content, title) in batch_out.items():
                                if content == "Error":
                                    book_manager.save_error_chapter(cid, cid)
                                    results["failed"] += 1
                                else:
                                    book_manager.save_chapter(cid, title, content)
                                    results["success"] += 1
                        else:
                            content, title = future.result()
                            cid = task["id"]
                            if content == "Error":
                                book_manager.save_error_chapter(cid, task["title"])
                                results["failed"] += 1
                            else:
                                book_manager.save_chapter(cid, title, content)
                                results["success"] += 1
                    except KeyboardInterrupt:
                        # 第二次 Ctrl-C：真正退出
                        book_manager.save_download_status()
                        raise
                    except Exception:
                        results["failed"] += 1

                    pbar.update()

        # ============ 统计取消数 ============

        canceled = tasks_count - results["success"] - results["failed"]
        results["canceled"] = max(0, canceled)

        # 保存状态 & 后处理
        book_manager.save_download_status()
        book_manager.finalize_spawn(chapters, canceled + results["failed"])

        # 恢复日志 handler 和 SIGINT
        self.log_system.disable_tqdm_handler()
        for h in orig_handlers:
            self.logger.addHandler(h)
        signal.signal(signal.SIGINT, self._orig_handler)

        return results

    def _cancel_pending(self, futures):
        for f in futures:
            if not f.done():
                f.cancel()

    # --- 非官方单章下载 ---
    def _download_single(self, chapter: dict) -> Tuple[str, str]:
        chapter_id = chapter["id"]
        req_id = f"{chapter_id[:4]}-{random.randint(1000,9999)}"
        self.logger.debug(f"[{req_id}] 下载 {chapter['title']}")

        retry = 0
        tried = set()
        while retry < self.config.max_retries:
            if self._stop_event.is_set():
                return "Error", chapter_id

            # 选 API
            ep = None
            for _ in range(len(self.config.api_endpoints)):
                cand = self.api_manager.get_api()
                if cand in tried:
                    self.api_manager.release_api(cand)
                    continue
                ep = cand
                break
            if not ep:
                return "Error", chapter_id
            tried.add(ep)

            try:
                url = f"{ep}/content?item_id={chapter_id}"
                # 随机延迟
                dt = random.randint(
                    self.config.min_wait_time, self.config.max_wait_time
                )
                time.sleep(dt / 1000)
                st = time.time()
                resp = requests.get(
                    url,
                    headers=self.network.get_headers(),
                    timeout=(3.05, self.config.request_timeout),
                    verify=False,
                )
                rt = time.time() - st

                # 更新 API 状态
                stt = self.network._api_status[ep]
                stt["response_time"] = stt["response_time"] * 0.7 + rt * 0.3

                if resp.status_code >= 300:
                    return "Error", chapter_id
                resp.raise_for_status()
                data = resp.json()
                content, title = ContentParser.extract_api_content({chapter_id: data})[chapter_id]
                if not content:
                    return "Error", chapter_id

                # 重置失败计数
                stt["failure_count"] = 0
                stt["last_success"] = time.time()
                self.logger.info(f"[{req_id}] 成功: {title} ({rt:.2f}s)")
                return content, title

            except requests.Timeout:
                self.logger.warning(f"[{req_id}] 超时重试")
            except Exception as e:
                stt = self.network._api_status[ep]
                stt["failure_count"] += 1
                if stt["failure_count"] > 5:
                    stt["cooldown_until"] = time.time() + random.randint(10, 30)
                self.logger.error(f"[{req_id}] 异常: {e}")
            finally:
                self.api_manager.release_api(ep)

            time.sleep(0.5 * (2**retry))
            retry += 1

        self.logger.error(f"[{req_id}] 重试失败")
        return "Error", chapter_id

    # --- 官方批量 10 章 下载 ---
    def _download_official_batch(self, chapters: List[dict]) -> List[Tuple[str, str]]:
        ids = ",".join(ch["id"] for ch in chapters)
        req_id = f"{ids[:4]}-{random.randint(1000,9999)}"
        self.logger.debug(f"[{req_id}] 批量下载 {len(chapters)} 章")

        # 随机延迟
        dt = random.randint(self.config.min_wait_time, self.config.max_wait_time)
        time.sleep(dt / 1000)

        start = time.time()
        d = fetch_chapter_for_epub(ids)
        chapters_dict = ContentParser.extract_api_content(d)
        out = chapters_dict

        elapsed = time.time() - start
        self.logger.info(f"[{req_id}] 批量完成 ({elapsed:.2f}s)")
        return out
