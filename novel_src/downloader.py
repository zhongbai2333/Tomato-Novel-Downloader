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
from concurrent.futures import ThreadPoolExecutor, as_completed
from tqdm import tqdm
from typing import List, Dict, Optional, Tuple

from .network import NetworkClient
from .book_manager import BookManager
from .parser import ContentParser
from .context import GlobalContext
from .log_system import TqdmLoggingHandler


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
            with ThreadPoolExecutor(max_workers=self.config.max_workers) as executor:
                # 创建未来任务列表
                futures = {
                    executor.submit(self._download_single, ch): ch
                    for ch in chapters
                    if (
                        ch["id"] not in book_manager.downloaded
                        or book_manager.downloaded.get(ch["id"]) == ["Error", "Error"]
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
                                book_manager.save_error_chapter(title)
                                results["failed"] += 1
                            else:
                                book_manager.save_chapter(ch, title, content)  # 统一保存入口
                                results["success"] += 1
                        except KeyboardInterrupt:
                            book_manager.save_download_status()
                            raise
                        except Exception as e:
                            results["failed"] += 1
                        progress.update()
                    book_manager.save_download_status()
                    book_manager.finalize_spawn(len(futures) - results["success"])

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

    def _download_single(self, chapter: Dict) -> Tuple[str, str]:
        """单个章节下载实现（带智能路由和自动重试）

        Args:
            chapter: 章节字典包含 id/title/index

        Returns:
            tuple: (清洗后的内容, 章节标题)

        Raises:
            Exception: 所有尝试失败时抛出
        """
        chapter_id = chapter["id"]
        retry_count = 0
        error_log = []
        request_id = f"{chapter_id[:4]}-{random.randint(1000,9999)}"  # 生成追踪ID

        self.logger.debug(f"[{request_id}] 开始下载章节 {chapter['title']}")

        while retry_count < self.config.max_retries:
            if self._stop_event.is_set():
                self.logger.debug("检测到停止信号，中止下载")
                raise KeyboardInterrupt()
            # 智能选择API端点
            sorted_endpoints = sorted(
                self.config.api_endpoints,
                key=lambda ep: (
                    self.network._api_status[ep]["response_time"],
                    self.network._api_status[ep]["failure_count"],
                ),
            )

            self.logger.debug(
                f"[{request_id}] 第{retry_count+1}次重试，可用端点: {sorted_endpoints[:2]}..."
            )

            for endpoint in sorted_endpoints:
                # 跳过连续失败超过阈值的端点
                if self.network._api_status[endpoint]["failure_count"] > 5:
                    self.logger.debug(f"[{request_id}] 跳过故障端点: {endpoint}")
                    continue

                try:
                    # 格式化请求URL（隐藏敏感参数）
                    formatted_url = (
                        endpoint.format(chapter_id=chapter_id, book_id=self.book_id)
                        if "{book_id}" in endpoint
                        else endpoint.format(chapter_id=chapter_id)
                    )
                    # 过滤敏感信息
                    safe_url = re.sub(r"(key=)[^&]+", r"\1***", formatted_url)
                    self.logger.debug(f"[{request_id}] 构造请求URL: {safe_url}")

                    # 添加随机延迟防止封禁（50-300ms）
                    delay = random.randint(
                        self.config.min_wait_time,  # 默认50
                        self.config.max_wait_time,  # 默认300
                    )
                    self.logger.debug(f"[{request_id}] 添加随机延迟: {delay}ms")
                    time.sleep(delay / 1000)

                    # 发送请求并计时
                    start_time = time.time()
                    self.logger.debug(f"[{request_id}] 发送请求到: {safe_url}")
                    try:
                        # 设置带超时的请求
                        response = requests.get(
                            formatted_url,
                            headers=self.network.get_headers(),
                            timeout=(
                                3.05,
                                self.config.request_timeout,
                            ),  # 连接超时3秒，读取超时使用配置
                        )
                        response_time = time.time() - start_time

                        # 更新API状态
                        self.network._api_status[endpoint]["response_time"] = (
                            self.network._api_status[endpoint]["response_time"] * 0.7
                        ) + (
                            response_time * 0.3
                        )  # 平滑响应时间

                        # 处理响应
                        self.logger.debug(
                            f"[{request_id}] 收到响应 | "
                            f"状态码: {response.status_code} | "
                            f"耗时: {response_time:.2f}s | "
                            f"长度: {len(response.content)}字节"
                        )

                        # 处理重定向
                        if 300 <= response.status_code < 400:
                            new_location = response.headers.get("Location", "")
                            self.logger.warning(
                                f"[{request_id}] 重定向到: {new_location[:50]}..."
                            )
                            raise requests.exceptions.TooManyRedirects()

                        response.raise_for_status()

                        # 解析JSON数据
                        try:
                            data = response.json()
                        except json.JSONDecodeError as e:
                            self.logger.error(
                                f"[{request_id}] JSON解析失败 | "
                                f"响应头: {dict(response.headers)} | "
                                f"内容摘要: {response.text[:200]}..."
                            )
                            raise

                        # 调试日志（采样记录完整响应）
                        if random.random() < 0.05:  # 1%采样率
                            self.logger.debug(
                                f"[{request_id}] 原始响应数据: {json.dumps(data, ensure_ascii=False)[:300]}..."
                            )

                        # 解析内容
                        content, title = ContentParser.extract_api_content(data)
                        self.logger.debug(
                            f"[{request_id}] 解析结果 | "
                            f"标题: {title} | "
                            f"内容长度: {len(content)}字符 | "
                            f"首行: {content.splitlines()[0][:30]}..."
                        )

                        if not content:
                            self.logger.warning(f"[{request_id}] 空内容警告")
                            raise ValueError("API返回空内容")

                        # 成功时更新状态
                        self.network._api_status[endpoint]["failure_count"] = 0
                        self.network._api_status[endpoint]["last_success"] = time.time()
                        self.logger.info(
                            f"[{request_id}] 下载成功 | "
                            f"{title} | "
                            f"用时: {response_time:.2f}s | "
                            f"端点: {endpoint.split('//')[1].split('/')[0]}"
                        )

                        return content, title
                    except requests.exceptions.Timeout:
                        if self._stop_event.is_set():
                            raise KeyboardInterrupt("用户中断导致的超时")

                except requests.exceptions.RequestException as e:
                    error_type = type(e).__name__

                    # 新增 traceback 记录
                    tb_str = traceback.format_exc()  # 获取完整错误堆栈字符串
                    error_stack = f"{endpoint} {error_type}:\n{tb_str}"

                    error_log.append(error_stack)
                    self.network._api_status[endpoint]["failure_count"] += 1

                    # 在日志中新增堆栈信息
                    self.logger.warning(
                        f"[{request_id}] 请求异常 | "
                        f"端点: {endpoint} | "
                        f"错误类型: {error_type} | "
                        f"消息: {str(e)}\n"
                    )
                    self.logger.debug(f"数据包：{response.text} | 堆栈跟踪:\n{tb_str}")

                    # 特殊处理429错误
                    if (
                        isinstance(e, requests.exceptions.HTTPError)
                        and e.response.status_code == 429
                    ):
                        cool_down = random.randint(10, 30)
                        self.logger.warning(
                            f"[{request_id}] 触发限流，冷却{cool_down}秒"
                        )
                        time.sleep(cool_down)

                    return "Error", chapter_id

                except Exception as e:
                    error_type = type(e).__name__
                    error_log.append(f"{endpoint} {error_type}: {str(e)}")
                    self.network._api_status[endpoint]["failure_count"] += 1
                    self.logger.error(
                        f"[{request_id}] 处理异常 | "
                        f"端点: {endpoint} | "
                        f"错误类型: {error_type} | "
                        f"堆栈: {traceback.format_exc()}"
                    )

                # 失败后短暂等待（指数退避）
                backoff_time = 0.5 * (2**retry_count)
                self.logger.debug(f"[{request_id}] 错误等待 {backoff_time:.1f}s")
                time.sleep(backoff_time)

            retry_count += 1
            self.logger.warning(f"[{request_id}] 第{retry_count}次重试...")

        if self._stop_event.is_set():
            raise KeyboardInterrupt()

        # 所有尝试失败后抛出异常
        final_error = (
            f"[{request_id}] 章节下载失败 | "
            f"尝试次数: {retry_count} | "
            f"最后错误: {error_log[-1] if error_log else '未知错误'}"
        )
        self.logger.error(final_error)
        raise Exception(final_error)
