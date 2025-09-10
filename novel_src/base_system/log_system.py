import logging
import zipfile
import datetime
import atexit
import time
import sys
import shutil
import signal
import ctypes
from ctypes import wintypes
from pathlib import Path
from logging.handlers import RotatingFileHandler
from logging import Formatter


class TqdmLoggingHandler(logging.StreamHandler):
    def __init__(self, tqdm_instance):
        super().__init__()
        self.tqdm_instance = tqdm_instance

    def emit(self, record):
        msg = self.format(record)
        try:
            # 仅用 tqdm.write 输出日志，避免手动 clear 破坏多进度条定位，
            # 这在 Windows 下偶发会导致“进度条被顶上去”。
            self.tqdm_instance.write(msg)
        except (KeyboardInterrupt, SystemExit):
            raise
        except Exception:
            pass


class ColoredMultiLineFormatter(Formatter):
    """支持颜色和多行对齐的日志格式化器"""

    COLOR_CODES = {
        "DEBUG": "\033[36m",  # 青色
        "INFO": "\033[32m",  # 绿色
        "WARNING": "\033[33m",  # 黄色
        "ERROR": "\033[31m",  # 红色
        "CRITICAL": "\033[1;31m",  # 加粗红色
    }
    RESET_CODE = "\033[0m"

    def __init__(self, use_color=False):
        super().__init__()
        self.use_color = use_color

    def _colorize_level(self, level_name):
        """为日志级别添加颜色"""
        if self.use_color and level_name in self.COLOR_CODES:
            return f"{self.COLOR_CODES[level_name]}{level_name}{self.RESET_CODE}"
        return level_name

    def format(self, record):
        """核心格式化方法"""
        # 生成基础组件
        timestamp = datetime.datetime.fromtimestamp(record.created).strftime(
            "%Y-%m-%d %H:%M:%S"
        )
        colored_level = self._colorize_level(record.levelname)
        thread_name = record.threadName

        # 构建前缀模板
        header_prefix = f"[{timestamp}][{colored_level}][{thread_name}]"
        plain_prefix = f"[{timestamp}][{record.levelname}][{thread_name}]"
        line_prefix = " " * len(plain_prefix)  # 基于无颜色文本计算空格

        # 处理消息内容
        message = super().format(record)
        lines = message.splitlines()

        # 重组多行内容
        formatted_lines = []
        for i, line in enumerate(lines):
            if i == 0:
                formatted_lines.append(f"{header_prefix} {line}")
            else:
                formatted_lines.append(f"{line_prefix} {line}")

        return "\n".join(formatted_lines)


class LogSystem(object):
    def __init__(self, debug: bool = False):
        self.logs_dir = Path("logs")
        self.latest_log = self.logs_dir / "latest.log"
        self.debug = debug
        # 新增信号处理初始化
        self._exit_flag = False  # 防止重复退出
        self._func_list = []
        self._setup_signal_handlers()
        self._setup_directories()
        self._configure_logging()
        self._setup_exception_hook()
        atexit.register(self.safe_exit)

    def _setup_exception_hook(self):
        """记录所有未捕获异常"""
        def handle_exception(exc_type, exc_value, exc_traceback):
            if issubclass(exc_type, KeyboardInterrupt):
                # 用户中断不记录
                sys.__excepthook__(exc_type, exc_value, exc_traceback)
                return
            self.logger.error("未捕获异常：", exc_info=(exc_type, exc_value, exc_traceback))

        sys.excepthook = handle_exception

    def _setup_signal_handlers(self):
        """设置跨平台信号/事件处理"""
        # 通用处理
        atexit.register(self.safe_exit)

        # Windows控制台事件
        if sys.platform == 'win32':
            self._setup_windows_handlers()
        # Unix信号处理
        else:
            self._setup_unix_handlers()

    def _setup_windows_handlers(self):
        """Windows控制台事件处理"""
        kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)
        PHANDLER_ROUTINE = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.DWORD)

        @PHANDLER_ROUTINE
        def _win_handler(event):
            if event in (2, 5, 6):  # CTRL_CLOSE_EVENT
                self.safe_exit()
                sys.exit(0)
            return False

        if not kernel32.SetConsoleCtrlHandler(_win_handler, True):
            raise ctypes.WinError(ctypes.get_last_error())

        # 保持handler引用防止GC
        self._win_handler = _win_handler

    def _setup_unix_handlers(self):
        """Unix信号处理"""
        signals = (signal.SIGTERM, signal.SIGHUP, signal.SIGINT)
        for sig in signals:
            signal.signal(sig, self._unix_signal_handler)

    def _unix_signal_handler(self, signum, frame):
        """Unix信号处理回调"""
        self.safe_exit()
        sys.exit(128 + signum)

    def _setup_directories(self):
        """创建日志目录并清理空文件"""
        self.logs_dir.mkdir(exist_ok=True)
        # 清理可能存在的空文件
        if self.latest_log.exists() and self.latest_log.stat().st_size == 0:
            self.latest_log.unlink()

    def _configure_logging(self):
        """配置日志系统"""
        self.logger = logging.getLogger("AppLogger")
        self.logger.setLevel(logging.DEBUG)
        self.logger.propagate = False  # 防止重复日志

        # 文件处理器（无颜色）
        file_formatter = ColoredMultiLineFormatter(use_color=False)
        self.file_handler = RotatingFileHandler(
            self.latest_log,
            maxBytes=10*1024*1024,
            backupCount=0,
            encoding='utf-8',
            delay=True
        )
        self.file_handler.setFormatter(file_formatter)
        self.file_handler.setLevel(logging.DEBUG)

        # 控制台处理器（带颜色）
        console_formatter = ColoredMultiLineFormatter(use_color=True)
        console_handler = logging.StreamHandler()
        console_handler.setFormatter(console_formatter)
        console_handler.setLevel(logging.DEBUG if self.debug else logging.INFO)

        # 添加处理器
        self.logger.addHandler(self.file_handler)
        self.logger.addHandler(console_handler)

    def add_safe_exit_func(self, func):
        self._func_list.append(func)

    def safe_exit(self):
        """安全退出处理（增加防重入机制）"""
        if self._exit_flag:
            return
        self._exit_flag = True

        try:
            for func in self._func_list:
                func()
            # 关闭所有日志处理器
            self._close_handlers()

            # 等待文件句柄释放（Windows需要更长时间）
            wait_time = 1.0 if sys.platform == "win32" else 0.3
            time.sleep(wait_time)

            # 执行归档
            self.archive_logs()
        except Exception as e:
            print(f"退出处理失败: {str(e)}")

    def enable_tqdm_handler(self, tqdm_instance):
        """启用绑定到指定tqdm实例的日志处理器"""
        self.tqdm_handler = TqdmLoggingHandler(tqdm_instance)
        console_formatter = ColoredMultiLineFormatter(use_color=True)
        self.tqdm_handler.setFormatter(console_formatter)
        self.tqdm_handler.setLevel(logging.DEBUG if self.debug else logging.INFO)
        self.logger.addHandler(self.tqdm_handler)

    def disable_tqdm_handler(self):
        """禁用临时tqdm处理器"""
        if hasattr(self, 'tqdm_handler'):
            self.logger.removeHandler(self.tqdm_handler)
            del self.tqdm_handler

    def _close_handlers(self):
        """关闭并移除所有文件处理器"""
        for handler in self.logger.handlers[:]:
            if isinstance(handler, logging.FileHandler):
                handler.close()
                self.logger.removeHandler(handler)
        logging.shutdown()

    def archive_logs(self):
        """安全归档日志文件"""
        if not self.latest_log.exists() or self.latest_log.stat().st_size == 0:
            return

        try:
            timestamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
            archive_name = f"log_{timestamp}.zip"
            archive_path = self.logs_dir / archive_name

            # 使用临时副本进行压缩
            temp_log = self.logs_dir / f"temp_{timestamp}.log"
            shutil.copyfile(self.latest_log, temp_log)

            with zipfile.ZipFile(archive_path, "w", zipfile.ZIP_DEFLATED) as zipf:
                zipf.write(temp_log, arcname=f"{timestamp}.log")

            temp_log.unlink()
            self.latest_log.unlink()
            print(f"日志已归档至: {archive_path}")

        except Exception as e:
            print(f"归档日志失败: {str(e)}")
            # 保留日志文件供下次启动处理
            if temp_log.exists():
                temp_log.unlink()
