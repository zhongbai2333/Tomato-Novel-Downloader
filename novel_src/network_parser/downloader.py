# -------------------------------
# downloader.py - 核心下载模块
# 职责：实现多线程下载和任务管理
# -------------------------------
import re
import time
import json
import requests
import random
import traceback
import threading
import signal
import queue
from concurrent.futures import ThreadPoolExecutor, as_completed
from tqdm import tqdm
from typing import List, Dict, Optional, Tuple

from .network import NetworkClient
from ..book_parser.book_manager import BookManager
from ..book_parser.parser import ContentParser
from ..base_system.context import GlobalContext
from ..base_system.log_system import TqdmLoggingHandler


class APIManager:
    def __init__(self, api_endpoints, config, network_status):
        self.api_queue = queue.Queue()
        self.config = config
        self.network_status = network_status  # 存储各个 API 的状态信息，例如 failure_count、response_time、cooldown_until 等
        for ep in api_endpoints:
            self.api_queue.put(ep)

    def get_api(self, timeout=1.0):
        """从队列中获取一个可用的 API 端点，如果队列为空则等待"""
        while True:
            try:
                ep = self.api_queue.get(timeout=timeout)
            except queue.Empty:
                # 队列暂时为空，等待后重试
                time.sleep(0.05)
                continue

            # 检查该 API 是否处于冷却状态
            cooldown = self.network_status.get(ep, {}).get("cooldown_until", 0)
            if time.time() < cooldown:
                # 当前 API 尚未冷却结束，放回队列后再取下一个
                self.api_queue.put(ep)
                time.sleep(0.05)
                continue
            return ep

    def release_api(self, ep):
        """任务结束后，把 API 放回队列"""
        self.api_queue.put(ep)


class ChapterDownloader:
    """章节下载调度器"""

    def __init__(self, book_id: str, network_client: NetworkClient):
        self.book_id = book_id
        self.network = network_client
        self.headers = self.network.get_headers()
        self.logger = GlobalContext.get_logger()
        self.log_system = GlobalContext.get_log_system()
        self.config = GlobalContext.get_config()

        # 新增中断控制属性
        self._stop_event = threading.Event()
        self._original_sigint_handler = signal.getsignal(signal.SIGINT)

        self.api_manager = APIManager(
            api_endpoints=self.config.api_endpoints,
            config=self.config,
            network_status=self.network._api_status,
        )

        # 注册信号处理函数
        signal.signal(signal.SIGINT, self._handle_signal)

    def _handle_signal(self, signum, frame):
        """处理中断信号"""
        self.logger.warning("接收到中断信号，正在尝试优雅退出...")
        self._stop_event.set()

        # 恢复默认信号处理以备强制退出
        signal.signal(signal.SIGINT, self._original_sigint_handler)

    def fetch_chapter_list(self) -> Optional[List[Dict]]:
        """从API获取章节列表"""
        api_url = (
            f"https://fanqienovel.com/api/reader/directory/detail?bookId={self.book_id}"
        )
        try:
            self.logger.debug(f"开始获取章节列表，URL: {api_url}")
            response = requests.get(
                api_url, headers=self.headers, timeout=self.config.request_timeout
            )
            self.logger.debug(
                f"章节列表响应状态: {response.status_code} 长度: {len(response.text)}字节"
            )

            response.raise_for_status()
            return self._parse_chapter_data(response.json())
        except Exception as e:
            self.logger.error(f"获取章节列表失败: {str(e)}", exc_info=True)
            if "response" in locals():
                self.logger.debug(f"错误响应内容: {response.text[:200]}...")
            return None

    def _parse_chapter_data(self, response_data: dict) -> List[Dict]:
        """解析章节API响应"""
        self.logger.debug(f"开始解析章节数据，响应码: {response_data.get('code')}")

        if response_data.get("code") != 0:
            self.logger.error(
                f"API错误数据: {json.dumps(response_data, ensure_ascii=False)[:200]}..."
            )
            raise ValueError(f"API错误: {response_data.get('message')}")

        chapters = response_data["data"]["allItemIds"]
        self.logger.info(f"解析到{len(chapters)}个章节ID，示例: {chapters[:3]}...")
        return [
            {"id": chapter_id, "title": f"第{idx+1}章", "index": idx}
            for idx, chapter_id in enumerate(chapters)
        ]

    def download_book(
        self, book_manager: BookManager, book_name: str, chapters: List[Dict]
    ) -> Dict[str, int]:
        """执行多线程下载任务"""
        original_handlers = self.logger.handlers.copy()

        # 临时关闭非tqdm处理器
        for handler in original_handlers:
            if not isinstance(handler, TqdmLoggingHandler):
                self.logger.removeHandler(handler)
        results = {"success": 0, "failed": 0, "canceled": 0}

        try:
            max_workers = min(self.config.max_workers, len(self.config.api_endpoints))
            with ThreadPoolExecutor(max_workers=max_workers) as executor:
                # 创建未来任务列表
                # 提交所有章节任务，不用关心 API 数量，由 APIManager 动态调度
                futures = {
                    executor.submit(self._download_single, ch): ch
                    for ch in chapters
                    if (
                        ch["id"] not in book_manager.downloaded
                        or book_manager.downloaded.get(ch["id"]) == [ch["id"], "Error"]
                    )
                }

                # 使用可中断的进度条
                with tqdm(total=len(futures), desc=f"下载《{book_name}》") as progress:
                    self.log_system.enable_tqdm_handler(progress)
                    for future in as_completed(futures):
                        if self._stop_event.is_set():
                            self._cancel_pending(futures)
                            results["canceled"] = len(futures)
                            break

                        ch = futures[future]
                        try:
                            content, title = future.result()
                            if content == "Error":
                                book_manager.save_error_chapter(title, chapters[title]["title"])
                                results["failed"] += 1
                            else:
                                book_manager.save_chapter(
                                    ch, title, content
                                )  # 统一保存入口
                                results["success"] += 1
                        except KeyboardInterrupt:
                            book_manager.save_download_status()
                            raise
                        except Exception as e:
                            results["failed"] += 1
                        progress.update()
                    book_manager.save_download_status()
                    book_manager.finalize_spawn(
                        chapters, len(futures) - results["success"]
                    )

        except KeyboardInterrupt:
            self.logger.warning("用户主动中断下载")
            results["canceled"] = len(futures) - results["success"] - results["failed"]
            book_manager.save_download_status()

        finally:
            # 确保移除临时处理器
            self.log_system.disable_tqdm_handler()
            # 恢复原有处理器
            for handler in original_handlers:
                self.logger.addHandler(handler)
            # 保存状态并清理资源\
            signal.signal(signal.SIGINT, self._original_sigint_handler)

        return results

    def _cancel_pending(self, futures):
        """取消未完成的任务"""
        for future in futures:
            if not future.done():
                future.cancel()

    def _download_single(self, chapter: dict) -> Tuple[str, str]:
        chapter_id = chapter["id"]
        retry_count = 0
        request_id = f"{chapter_id[:4]}-{random.randint(1000,9999)}"
        attempted_apis = set()  # 针对该章节记录已尝试过的 API

        self.logger.debug(f"[{request_id}] 开始下载章节 {chapter['title']}")

        while retry_count < self.config.max_retries:
            if self._stop_event.is_set():
                self.logger.debug("检测到停止信号，中止下载")
                return "Error", chapter_id

            # 从 API 管理器中获取一个可用的 API，不重复使用已尝试失败的 API
            selected_endpoint = None
            for _ in range(len(self.config.api_endpoints)):
                ep = self.api_manager.get_api()
                if ep in attempted_apis:
                    # 使用后立即释放，再尝试下一个
                    self.api_manager.release_api(ep)
                    continue
                selected_endpoint = ep
                break

            if not selected_endpoint:
                self.logger.error(f"[{request_id}] 没有可用的API来下载该章节")
                return "Error", chapter_id

            # 记录该 API 已尝试
            attempted_apis.add(selected_endpoint)
            try:
                dl_endpoint = selected_endpoint + "/content?item_id={chapter_id}"
                formatted_url = dl_endpoint.format(chapter_id=chapter_id)
                safe_url = re.sub(r"(key=)[^&]+", r"\1***", formatted_url)
                self.logger.debug(
                    f"[{request_id}] 使用 API {selected_endpoint} 请求 URL: {safe_url}"
                )

                # 保留随机延迟（50-300ms）
                delay = random.randint(
                    self.config.min_wait_time, self.config.max_wait_time
                )
                self.logger.debug(f"[{request_id}] 添加随机延迟: {delay}ms")
                time.sleep(delay / 1000)

                start_time = time.time()
                response = requests.get(
                    formatted_url,
                    headers=self.network.get_headers(),
                    timeout=(3.05, self.config.request_timeout),
                )
                response_time = time.time() - start_time

                # 平滑更新该 API 的响应时间
                status = self.network._api_status[selected_endpoint]
                status["response_time"] = (
                    status["response_time"] * 0.7 + response_time * 0.3
                )
                self.logger.debug(
                    f"[{request_id}] 收到响应，状态码: {response.status_code}"
                )

                if 300 <= response.status_code < 400:
                    self.logger.warning(f"[{request_id}] 重定向处理")
                    return "Error", chapter_id

                response.raise_for_status()

                try:
                    data = response.json()
                except json.JSONDecodeError:
                    self.logger.error(f"[{request_id}] JSON解析失败")
                    return "Error", chapter_id

                content, title = ContentParser.extract_api_content(data)
                if not content:
                    self.logger.warning(f"[{request_id}] 获得空内容")
                    return "Error", chapter_id

                # 成功后重置该 API 的错误计数
                status["failure_count"] = 0
                status["last_success"] = time.time()
                self.logger.info(
                    f"[{request_id}] 成功下载章节 {title}，用时 {response_time:.2f}s"
                )

                return content, title

            except requests.exceptions.Timeout:
                self.logger.warning(f"[{request_id}] 请求超时")
            except requests.exceptions.RequestException as e:
                # 累计该 API 的错误次数
                self.network._api_status[selected_endpoint]["failure_count"] += 1
                if self.network._api_status[selected_endpoint]["failure_count"] > 5:
                    # 达到错误阈值后设置较长冷却时间
                    cooldown = random.randint(10, 30)
                    self.network._api_status[selected_endpoint]["cooldown_until"] = (
                        time.time() + cooldown
                    )
                    self.logger.warning(
                        f"[{request_id}] API {selected_endpoint} 达到错误阈值，冷却 {cooldown} 秒"
                    )
                self.logger.error(
                    f"[{request_id}] 使用 API {selected_endpoint} 请求异常: {str(e)}"
                )
            except Exception as e:
                self.network._api_status[selected_endpoint]["failure_count"] += 1
                self.logger.error(
                    f"[{request_id}] 使用 API {selected_endpoint} 发生异常: {str(e)}"
                )
            finally:
                # 无论成功或失败，都释放该 API 供其他任务使用
                self.api_manager.release_api(selected_endpoint)

            # 指数退避等待后重试
            backoff_time = 0.5 * (2**retry_count)
            self.logger.debug(f"[{request_id}] 等待 {backoff_time:.1f}s 后重试")
            time.sleep(backoff_time)
            retry_count += 1

        self.logger.error(f"[{request_id}] 下载失败，多次重试均未成功")
        return "Error", chapter_id
