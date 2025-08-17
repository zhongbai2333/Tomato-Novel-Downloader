# -------------------------------
# downloader.py - 核心下载模块
# 职责：实现多线程下载和任务管理
# -------------------------------
import time
import requests
import shutil
import random
import threading
import signal
import queue
import urllib3
from concurrent.futures import ThreadPoolExecutor, as_completed
from tqdm import tqdm
from typing import List, Dict, Optional, Tuple

from .network import NetworkClient
from client_mod import fetch_batch_chapter
from fanqie_mod import get_iid, get_contents
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
        """
        执行多线程下载任务（支持三种模式）：
          1. config.use_official_api → 官方批量（每 10 章一组）
          2. config.use_helloplhm_qwq_api → 新增 helloplhm_qwq 批量（每 300 章一组）
          3. 否则 → 非官方单章
        """
        # 备份并关闭非 tqdm handler
        orig_handlers = self.logger.handlers.copy()
        for h in orig_handlers:
            if not isinstance(h, TqdmLoggingHandler):
                self.logger.removeHandler(h)

        results = {"success": 0, "failed": 0, "canceled": 0}

        # ============ 准备要下载的章节列表 & 分组 ============

        # 1. 官方批量模式（修改：每 50 章一组；get_contents 接受 List[str]）
        if self.config.use_official_api:
            to_download = [
                ch
                for ch in chapters
                if (ch["id"] not in book_manager.downloaded)
                or (book_manager.downloaded.get(ch["id"])[1] == "Error")
            ]
            # 按 50 章一组（原为 10）
            groups = [
                to_download[i : i + 50] for i in range(0, len(to_download), 50)
            ]
            tasks_count = len(to_download)
            max_workers = self.config.max_workers

            def get_submit(exe):
                # 把每组提交给线程池
                return {
                    exe.submit(self._download_official_batch, grp): grp
                    for grp in groups
                }

            desc = f"下载《{book_name}》（官方批量）"

        # 2. 新增 helloplhm_qwq 批量模式
        elif self.config.use_helloplhm_qwq_api:
            # 筛选出需要下载的章节
            to_download = [
                ch
                for ch in chapters
                if (ch["id"] not in book_manager.downloaded)
                or (book_manager.downloaded.get(ch["id"])[1] == "Error")
            ]
            # 提取所有待下载的 ID
            id_list = [ch["id"] for ch in to_download]
            # 每 300 个 ID 划分一组
            id_groups: List[List[str]] = [
                id_list[i : i + 300] for i in range(0, len(id_list), 300)
            ]
            # 对应于每个 ID 列表，我们还要记住“这一组对应哪几个章节”，
            # 以便将来存储和统计时知道哪些 ID 与哪个章节绑定。这里可以用一个 map。
            # 但为了简单，我们只传递 ID 列表给任务，后面结果处理时只关注 ID 即可。
            tasks_count = len(id_list)
            max_workers = 1

            def get_submit(exe):
                # 把每个 300 ID 的列表提交给线程池
                return {
                    exe.submit(self._download_helloplhm_batch, grp): grp
                    for grp in id_groups
                }

            desc = f"下载《{book_name}》（helloplhm_qwq 批量）"

        # 3. 非官方单章模式
        else:
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
            cols, _ = shutil.get_terminal_size(fallback=(80, 24))
            with tqdm(
                total=len(futures),
                desc=desc,
                ncols=cols,
                dynamic_ncols=False,
                leave=True,
            ) as pbar:
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
                        # --- 情况 1：官方批量 ---
                        if self.config.use_official_api:
                            # future.result() 返回： Dict[str, Tuple[content, title]]
                            batch_out: Dict[str, Tuple[str, str]] = future.result()
                            for cid, (content, title) in batch_out.items():
                                if content == "Error":
                                    book_manager.save_error_chapter(cid, cid)
                                    results["failed"] += 1
                                else:
                                    book_manager.save_chapter(cid, title, content)
                                    results["success"] += 1

                        # --- 情况 2：helloplhm_qwq 批量 ---
                        elif self.config.use_helloplhm_qwq_api:
                            # future.result() 返回： Dict[str, Tuple[content, title]]
                            batch_out: Dict[str, Tuple[str, str]] = future.result()
                            for cid, (content, title) in batch_out.items():
                                if content == "Error":
                                    book_manager.save_error_chapter(cid, cid)
                                    results["failed"] += 1
                                else:
                                    book_manager.save_chapter(cid, title, content)
                                    results["success"] += 1

                        # --- 情况 3：非官方单章 ---
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
                    except Exception as e:
                        # 无论哪种模式，只要抛异常，都当作失败
                        self.logger.error(f"[异常] 处理任务时出错：{e}")
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
                content, title = ContentParser.extract_api_content({chapter_id: data})[
                    chapter_id
                ]
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
    def _download_official_batch(
        self, chapters: List[dict]
    ) -> Dict[str, Tuple[str, str]]:
        """
        官方批量接口：现在一次最多 50 章，get_contents 接受 List[str]。
        返回：{ chapter_id: (content 或 "Error", title) }
        """
        id_list = [ch["id"] for ch in chapters]
        # 生成请求 ID 便于日志追踪（取第一个 ID 的前 4 位）
        joined = "-".join(id_list)[:4]
        req_id = f"{joined}-{random.randint(1000,9999)}"
        self.logger.debug(f"[{req_id}] 批量下载 {len(id_list)} 章")

        # 随机延迟（官方接口最快 500ms，强制下限 500）
        min_wait = max(500, self.config.min_wait_time)
        max_wait = max(min_wait + 50, self.config.max_wait_time)  # 防止范围过小
        dt = random.randint(min_wait, max_wait)
        time.sleep(dt / 1000)

        start = time.time()
        # 直接传入 List[str]
        print(id_list)
        raw = get_contents(id_list)
        parsed = ContentParser.extract_api_content(raw)
        # 统一构建输出，缺失或空内容标记为 Error
        out: Dict[str, Tuple[str, str]] = {}
        for cid in id_list:
            if cid not in parsed or not parsed[cid][0]:
                out[cid] = ("Error", cid)
            else:
                out[cid] = parsed[cid]

        elapsed = time.time() - start
        self.logger.info(f"[{req_id}] 批量完成 ({elapsed:.2f}s)")
        return out

    # --- 新增 helloplhm_qwq 批量下载方法 ---
    def _download_helloplhm_batch(
        self, id_list: List[str]
    ) -> Dict[str, Tuple[str, str]]:
        """
        通过 helloplhm_qwq API 一次性下载最多 300 个章节。
        输入：id_list → ["id1", "id2", ...]（长度 <= 300）
        返回：{ id: (content 或 "Error", title 或 id) }
        """
        # 生成请求 ID，便于日志追踪
        joined = "-".join(id_list)[:4]
        req_id = f"{joined}-{random.randint(1000,9999)}"
        self.logger.debug(f"[{req_id}] helloplhm_qwq 批量下载 {len(id_list)} 章")

        # 随机延迟以分散压力
        dt = random.randint(
            self.config.min_wait_time if self.config.min_wait_time >= 1000 else 1000,
            self.config.max_wait_time if self.config.min_wait_time >= 1200 else 1200,
        )
        time.sleep(dt / 1000)

        try:
            # 调用外部函数，一次性传入 ID 列表
            raw_result = fetch_batch_chapter(id_list)
            # 这里假设 raw_result 是一个形如 { "id1": {...}, "id2": {...} } 的原始返回，需要交由 ContentParser 解析
            parsed: Dict[str, Tuple[str, str]] = ContentParser.extract_api_content(
                raw_result
            )
            # parsed 中的格式为 { "id": (content, title) }
            # 如果 content 为空，可以视作下载失败
            out: Dict[str, Tuple[str, str]] = {}
            for cid in id_list:
                if cid not in parsed or not parsed[cid][0]:
                    out[cid] = ("Error", cid)  # 如果没拿到内容，就用 Error
                else:
                    out[cid] = parsed[cid]
            self.logger.info(
                f"[{req_id}] helloplhm_qwq 批量下载完成 ({len(id_list)} 章)"
            )
            return out

        except Exception as e:
            # 如果整个批次调用出错，则把这一批次的 ID 全部标记为 Error
            self.logger.error(f"[{req_id}] helloplhm_qwq 批量下载异常：{e}")
            return {cid: ("Error", cid) for cid in id_list}
