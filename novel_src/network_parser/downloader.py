# -------------------------------
# downloader.py - 核心下载模块
# 职责：实现多线程下载和任务管理
# -------------------------------
import time
import logging
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
        self._sigint_count = 0
        self._orig_handler = signal.getsignal(signal.SIGINT)
        signal.signal(signal.SIGINT, self._handle_signal)

        self.api_manager = APIManager(
            api_endpoints=self.config.api_endpoints,
            config=self.config,
            network_status=self.network._api_status,
        )
        # 批量下载进度条引用（官方/第三方批量模式使用）
        self._batch_progress_bar = None
        self._batch_bar_lock = threading.Lock()

    def _handle_signal(self, signum, frame):
        # 两段式退出：第一次优雅退出，第二次强制退出
        self._sigint_count += 1
        if self._sigint_count == 1:
            self.logger.warning("接收到 Ctrl-C，正在优雅退出（再次按下将强制退出）...")
            self._stop_event.set()
            # 保持自定义 handler 有效，便于第二次 Ctrl-C 捕获
        else:
            self.logger.error("第二次 Ctrl-C，强制退出")
            # 抛出 KeyboardInterrupt，让上层立即中断（download_book 会保存状态并向上抛出）
            raise KeyboardInterrupt

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
        # 仅暂时移除控制台类处理器（StreamHandler），保留文件处理器，以确保 tqdm 期间仍写入日志文件
        orig_handlers = self.logger.handlers.copy()
        removed_handlers = []
        for h in orig_handlers:
            # 仅移除“指向控制台的普通 StreamHandler”，保留文件处理器与已添加的 TqdmLoggingHandler
            # 问题修复：FileHandler 继承自 StreamHandler，之前条件会把文件日志处理器也移除，导致 tqdm 期间日志不写入文件。
            if (
                isinstance(h, logging.StreamHandler)
                and not isinstance(h, TqdmLoggingHandler)
                and not isinstance(h, logging.FileHandler)  # 保留文件日志处理器
            ):
                self.logger.removeHandler(h)
                removed_handlers.append(h)

        results = {"success": 0, "failed": 0, "canceled": 0}
        # 段评并发执行器与任务列表（仅在启用段评时使用）
        # 章节级段评并发执行器（控制并行处理多少章节的段评）
        chapter_comment_executor: Optional[ThreadPoolExecutor] = None
        comment_futures = []
        if getattr(self.config, "enable_segment_comments", False):
            chapter_cw = max(1, int(getattr(self.config, "segment_comments_chapter_workers", 4)))
            chapter_comment_executor = ThreadPoolExecutor(max_workers=chapter_cw)

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
                to_download[i : i + 25] for i in range(0, len(to_download), 25)
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

        # 预先创建进度条，避免首批极快完成导致未创建无法更新
        # 使用 dynamic_ncols 让 tqdm 自动填满整行；自定义 bar_format 统一显示
        # 重新计算终端宽度，确保 tqdm 占满（Windows 下 tqdm 对 dynamic_ncols 偶尔失效）
        try:
            cols, _ = shutil.get_terminal_size(fallback=(120, 30))
        except Exception:
            cols = 120
        # 预留尾部信息长度，大致估算
        info_tail = 32  # ETA/计数等
        # desc + 百分比固定区（含空格）估算 20
        base_fixed = 20 + info_tail
        # bar 宽度：最少 10
        bar_total = max(10, cols - base_fixed)
        bar_fmt = f"[{{elapsed}}] {{desc}} {{percentage:3.0f}}%|{{bar:{bar_total}}}| {{n_fmt}}/{{total_fmt}} ETA:{{remaining}}"
        common_kwargs = dict(
            mininterval=0.25,
            dynamic_ncols=False,  # 我们手工控制 ncols
            leave=True,
            ncols=cols,
            bar_format=bar_fmt,
        )
        if self.config.use_official_api:
            download_bar = tqdm(total=len(groups), desc="章节下载(批)", position=0, **common_kwargs)
            self._batch_progress_bar = download_bar
        elif self.config.use_helloplhm_qwq_api:
            download_bar = tqdm(total=len(id_groups), desc="章节下载(批)", position=0, **common_kwargs)
            self._batch_progress_bar = download_bar
        else:
            download_bar = tqdm(total=tasks_count, desc="章节下载", position=0, **common_kwargs)
            self._batch_progress_bar = None
        save_bar = tqdm(total=tasks_count, desc="正文/段评保存", position=1, **common_kwargs)
        # 缓冲保存进度更新，减少闪烁
        pending_save_updates = 0
        save_update_batch = 1 if tasks_count <= 80 else 3

        def _save_bar_incr():
            nonlocal pending_save_updates
            pending_save_updates += 1
            if pending_save_updates >= save_update_batch:
                save_bar.update(pending_save_updates)
                pending_save_updates = 0
        # 已移除段评图片进度条，后台静默下载
        book_manager.media_progress = None
        self.log_system.enable_tqdm_handler(download_bar)

        with ThreadPoolExecutor(max_workers=max_workers) as exe:
            futures = get_submit(exe)
            # 批量模式不再提前章节级推进，由完成后 +1

            for future in as_completed(futures):
                if self._stop_event.is_set():
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
                                if chapter_comment_executor:
                                    comment_futures.append(chapter_comment_executor.submit(self._maybe_fetch_segment_comments, book_manager, cid))
                                results["success"] += 1
                            _save_bar_incr()
                        # 批进度 +1 在批量函数已处理
                        continue
                    elif self.config.use_helloplhm_qwq_api:
                        batch_out: Dict[str, Tuple[str, str]] = future.result()
                        for cid, (content, title) in batch_out.items():
                            if content == "Error":
                                book_manager.save_error_chapter(cid, cid)
                                results["failed"] += 1
                            else:
                                book_manager.save_chapter(cid, title, content)
                                if chapter_comment_executor:
                                    comment_futures.append(chapter_comment_executor.submit(self._maybe_fetch_segment_comments, book_manager, cid))
                                results["success"] += 1
                            _save_bar_incr()
                        continue
                    else:
                        content, title = future.result()
                        cid = task["id"]
                        if content == "Error":
                            book_manager.save_error_chapter(cid, task["title"])
                            results["failed"] += 1
                        else:
                            book_manager.save_chapter(cid, title, content)
                            if chapter_comment_executor:
                                comment_futures.append(chapter_comment_executor.submit(self._maybe_fetch_segment_comments, book_manager, cid))
                            results["success"] += 1
                        _save_bar_incr()
                        download_bar.update(1)
                except KeyboardInterrupt:
                    book_manager.save_download_status()
                    raise
                except Exception as e:
                    self.logger.error(f"[异常] 处理任务时出错：{e}")
                    if not (self.config.use_official_api or self.config.use_helloplhm_qwq_api):
                        try:
                            download_bar.update(1)
                        except Exception:
                            pass
        # 刷新剩余缓冲
        if pending_save_updates:
            save_bar.update(pending_save_updates)
            pending_save_updates = 0
        # 等待段评章节级并发任务完成
        if comment_futures:
            for cf in as_completed(comment_futures):
                try:
                    _ = cf.result()
                except Exception:
                    pass
        if chapter_comment_executor:
            chapter_comment_executor.shutdown(wait=True)
        # 保存状态 & 后处理（with 结束后）
        canceled = tasks_count - results["success"] - results["failed"]
        results["canceled"] = max(0, canceled)
        book_manager.save_download_status()
        book_manager.finalize_spawn(chapters, canceled + results["failed"])

        # 恢复被移除的控制台 handler；文件 handler 始终未移除
        self.log_system.disable_tqdm_handler()
        for h in removed_handlers:
            self.logger.addHandler(h)
        signal.signal(signal.SIGINT, self._orig_handler)

        return results

    # ================= 段评辅助 =================
    def _maybe_fetch_segment_comments(self, book_manager: BookManager, chapter_id: str):
        """
        当配置开启时，抓取该章节的段评统计，并对有评论的段落获取前 N 条评论，保存为 JSON。
        失败不影响主流程。
        """
        try:
            if not getattr(self.config, "enable_segment_comments", False):
                return
            # 统计
            stats_wrap = self.network.fetch_para_comment_stats(chapter_id)
            if not stats_wrap or not isinstance(stats_wrap, dict):
                return
            item_version = stats_wrap.get("item_version") or "1"
            stats = stats_wrap.get("stats") if isinstance(stats_wrap.get("stats"), dict) else None
            if not stats:
                return
            # 逐段取评论（仅 count>0 的段），并发提速
            top_n = max(0, int(getattr(self.config, "segment_comments_top_n", 10)))
            sort = 1
            workers = max(1, int(getattr(self.config, "segment_comments_workers", 4)))
            full = {"chapter_id": chapter_id, "book_id": self.book_id, "item_version": item_version, "paras": {}}

            # 收集需要请求的段落索引并预填统计
            to_fetch: List[int] = []
            for key, meta in stats.items():
                try:
                    idx = int(key)
                except Exception:
                    continue
                try:
                    cnt = int(meta.get("count", 0)) if isinstance(meta, dict) else 0
                except Exception:
                    cnt = 0
                if cnt <= 0:
                    continue
                to_fetch.append(idx)
                full["paras"][str(idx)] = {
                    "count": cnt,
                    "hot": meta.get("hot") if isinstance(meta, dict) else None,
                    "infos": meta.get("infos") if isinstance(meta, dict) else None,
                    "detail": None,
                }

            if not to_fetch:
                book_manager.save_segment_comments(chapter_id, full)
                return

            with ThreadPoolExecutor(max_workers=workers) as ex:
                fut_map = {
                    ex.submit(
                        self.network.fetch_para_comments,
                        chapter_id,
                        self.book_id,
                        idx,
                        item_version,
                        top_n,
                        sort,
                    ): idx
                    for idx in to_fetch
                }
                for fut in as_completed(fut_map):
                    idx = fut_map[fut]
                    try:
                        detail = fut.result()
                    except Exception:
                        detail = None
                    full["paras"][str(idx)]["detail"] = detail
            # 保存
            book_manager.save_segment_comments(chapter_id, full)
        except Exception:
            # 全部静默
            pass

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
    # 不在开始时更新进度（避免与完成语义冲突）

        # 随机延迟（官方接口最快 500ms，强制下限 500）
        min_wait = max(500, self.config.min_wait_time)
        max_wait = max(min_wait + 50, self.config.max_wait_time)  # 防止范围过小
        dt = random.randint(min_wait, max_wait)
        time.sleep(dt / 1000)

        start = time.time()
        # 直接传入 List[str]
        raw = get_contents(id_list, epub=self.config.novel_format=="epub", get_status_folder_path=self.config.get_status_folder_path / "images")
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
        # 批量完成后 +1（与下载(批) 进度条联动）
        try:
            if self._batch_progress_bar is not None:
                with self._batch_bar_lock:
                    self._batch_progress_bar.update(1)
        except Exception:
            pass
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
    # 开始不更新；完成后再 +1

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
            try:
                if self._batch_progress_bar is not None:
                    with self._batch_bar_lock:
                        self._batch_progress_bar.update(1)
            except Exception:
                pass
            return out

        except Exception as e:
            # 如果整个批次调用出错，则把这一批次的 ID 全部标记为 Error
            self.logger.error(f"[{req_id}] helloplhm_qwq 批量下载异常：{e}")
            try:
                if self._batch_progress_bar is not None:
                    with self._batch_bar_lock:
                        self._batch_progress_bar.update(1)
            except Exception:
                pass
            return {cid: ("Error", cid) for cid in id_list}
