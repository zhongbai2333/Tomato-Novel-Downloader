import asyncio
import re
import signal
import threading
from concurrent.futures import (
    ThreadPoolExecutor,
    Future,
    TimeoutError,
    CancelledError,
    wait,
)
from pathlib import Path
from typing import List, Tuple

from contextlib import suppress

from tqdm import tqdm

from ..base_system.progress import build_tqdm_common_kwargs, refresh_progress_bars

def generate_audiobook(manager, chapters: List[dict]) -> bool:
    """Convert downloaded chapters to audio files using edge-tts when enabled."""
    config = getattr(manager, "config", None)
    logger = getattr(manager, "logger", None)
    if not config or not getattr(config, "enable_audiobook", False):
        return True

    try:
        import edge_tts  # type: ignore
    except ImportError:  # pragma: no cover - runtime guard
        if logger:
            logger.error("edge-tts 未安装，无法生成有声小说。请先通过 pip 安装 edge-tts。")
        return False

    voice = getattr(config, "audiobook_voice", "zh-CN-XiaoxiaoNeural")
    rate = getattr(config, "audiobook_rate", "+0%")
    volume = getattr(config, "audiobook_volume", "+0%")
    pitch = _normalize_pitch(getattr(config, "audiobook_pitch", ""), logger)
    fmt = str(getattr(config, "audiobook_format", "mp3")).strip().lower() or "mp3"
    if fmt not in {"mp3", "wav"}:
        if logger:
            logger.warning(f"音频格式 {fmt} 不受支持，已回退为 mp3。")
        fmt = "mp3"

    safe_name = config.safe_fs_name(manager.book_name or manager.book_id or "book")
    output_dir = Path(getattr(config, "output_dir", "."))
    output_dir.mkdir(parents=True, exist_ok=True)
    audio_dir = output_dir / f"{safe_name}_audio"
    audio_dir.mkdir(parents=True, exist_ok=True)

    chapters_data: List[Tuple[str, str]] = []
    for index, chapter in enumerate(chapters or []):
        cid = chapter.get("id")
        stored = manager.downloaded.get(cid)
        if stored is None:
            stored = manager.downloaded.get(str(cid))
        if not stored or len(stored) < 2:
            continue
        title = stored[0] or chapter.get("title") or f"章节 {index+1}"
        content = stored[1]
        if not isinstance(content, str) or not content.strip():
            continue
        chapters_data.append((str(title), content))

    if not chapters_data:
        if logger:
            logger.info("无可用章节内容，跳过有声小说生成。")
        return True

    concurrency = max(1, int(getattr(config, "audiobook_concurrency", 2)))
    concurrency = min(concurrency, len(chapters_data)) if chapters_data else concurrency

    if logger:
        logger.info(
            f"开始生成有声小说，共 {len(chapters_data)} 章 -> {audio_dir}，并发={concurrency}"
        )

    loop, loop_thread = _ensure_event_loop()

    stop_event = threading.Event()
    aborted = False

    def _request_stop():
        if stop_event.is_set():
            return
        stop_event.set()
        _schedule_loop_shutdown(loop)

    sigint_installed = False
    sigint_count = 0
    orig_sigint = None
    if threading.current_thread() is threading.main_thread():
        try:
            orig_sigint = signal.getsignal(signal.SIGINT)
        except Exception:
            orig_sigint = None

        def _handle_sigint(signum, frame):
            nonlocal sigint_count
            sigint_count += 1
            if sigint_count == 1:
                if logger:
                    logger.warning("[TTS] 接收到 Ctrl-C，正在停止有声小说生成（再次按下将强制退出）...")
                _request_stop()
            else:
                handler = orig_sigint
                try:
                    if handler not in (None, signal.SIG_DFL, signal.SIG_IGN):
                        signal.signal(signal.SIGINT, handler)
                    else:
                        signal.signal(signal.SIGINT, signal.SIG_DFL)
                except Exception:
                    pass
                if callable(handler):
                    handler(signum, frame)
                elif handler is signal.SIG_IGN:
                    return
                else:
                    raise KeyboardInterrupt

        try:
            signal.signal(signal.SIGINT, _handle_sigint)
            sigint_installed = True
        except Exception:
            sigint_installed = False

    def _job(args):
        idx, title, content = args
        if stop_event.is_set():
            return idx, title, RuntimeError("cancelled")
        sanitized = _sanitize_for_tts(title, content)
        if not sanitized.strip():
            return idx, title, None
        file_name = f"{idx:04d}-{config.safe_fs_name(title)}.{fmt}"
        file_path = audio_dir / file_name
        if logger:
            logger.info(f"[TTS] 正在生成章节 {idx}: {file_path.name}")
        coro = _synthesize_to_file(edge_tts, sanitized, file_path, voice, rate, volume, pitch)
        try:
            task_future = asyncio.run_coroutine_threadsafe(coro, loop)
            cancelled = False
            while True:
                if stop_event.is_set() and not cancelled:
                    task_future.cancel()
                    cancelled = True
                try:
                    task_future.result(timeout=0.5)
                    break
                except TimeoutError:
                    continue
                except (asyncio.CancelledError, CancelledError):
                    return idx, title, RuntimeError("cancelled")
            return idx, title, None
        except Exception as exc:  # noqa: BLE001
            return idx, title, exc

    cols, progress_kwargs = build_tqdm_common_kwargs()
    progress = tqdm(
        total=len(chapters_data),
        desc="有声小说生成",
        unit="章",
        **progress_kwargs,
    )
    refresh_progress_bars((progress,), columns=cols)
    errors = []

    try:
        with ThreadPoolExecutor(max_workers=concurrency) as executor:
            futures: list[Future] = [
                executor.submit(_job, (idx, title, content))
                for idx, (title, content) in enumerate(chapters_data, start=1)
            ]
            pending = set(futures)
            try:
                while pending:
                    if stop_event.is_set():
                        aborted = True
                        break
                    done, pending = wait(pending, timeout=0.5)
                    for future in done:
                        try:
                            idx, title, exc = future.result()
                        except KeyboardInterrupt:
                            _request_stop()
                            aborted = True
                            pending.clear()
                            break
                        progress.update(1)
                        if exc:
                            if isinstance(exc, RuntimeError) and str(exc) == "cancelled":
                                continue
                            errors.append((idx, title, exc))
                            if logger and not stop_event.is_set():
                                logger.error(f"[TTS] 章节 {idx}《{title}》生成失败：{exc}")
                    else:
                        continue
                    break
            finally:
                if stop_event.is_set() or aborted:
                    aborted = True
                    for fut in futures:
                        if not fut.done():
                            fut.cancel()
                    _request_stop()
    finally:
        progress.close()
        if sigint_installed:
            try:
                signal.signal(signal.SIGINT, orig_sigint or signal.SIG_DFL)
            except Exception:
                pass
    _shutdown_loop(loop, loop_thread, logger)

    if logger:
        if aborted and stop_event.is_set():
            logger.warning("有声小说生成已取消（收到 Ctrl-C）")
        elif errors:
            logger.warning(f"有声小说生成完成（部分失败 {len(errors)} 章）：{audio_dir}")
        else:
            logger.info(f"有声小说生成完成：{audio_dir}")

    return not aborted


def _sanitize_for_tts(title: str, content: str) -> str:
    combined = f"{title}。\n{content}"
    combined = combined.replace("\u3000", " ").replace("&nbsp;", " ")
    combined = re.sub(r"<[^>]+>", " ", combined)
    combined = combined.replace("\r", "\n")
    combined = re.sub(r"\n{2,}", "\n", combined)
    combined = re.sub(r"[\t\f\v]+", " ", combined)
    combined = re.sub(r" {2,}", " ", combined)
    return combined.strip()


async def _synthesize_to_file(edge_tts, text: str, file_path: Path, voice: str, rate: str, volume: str, pitch: str | None):
    file_path.parent.mkdir(parents=True, exist_ok=True)
    temp_path = file_path.with_suffix(f"{file_path.suffix}.partial")
    with suppress(FileNotFoundError):
        temp_path.unlink()
    with suppress(FileNotFoundError):
        file_path.unlink()

    kwargs = {
        "text": text,
        "voice": voice,
        "rate": rate,
        "volume": volume,
    }
    if pitch:
        kwargs["pitch"] = pitch

    communicator = edge_tts.Communicate(**kwargs)

    try:
        await communicator.save(str(temp_path))
        temp_path.replace(file_path)
    except asyncio.CancelledError:
        with suppress(FileNotFoundError):
            temp_path.unlink()
        raise
    except LookupError as exc:
        with suppress(FileNotFoundError):
            temp_path.unlink()
        raise RuntimeError(
            "edge-tts 无法建立到语音服务的连接，请检查网络或系统代理设置。"
        ) from exc
    except Exception:
        with suppress(FileNotFoundError):
            temp_path.unlink()
        raise


def _normalize_pitch(value, logger=None) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    if not text:
        return None
    lower = text.lower()
    if lower in {"default", "auto", "none"}:
        return None
    if lower.endswith("%"):
        if logger:
            logger.warning("[TTS] pitch 不支持百分比格式，已忽略该设置。")
        return None
    match = re.fullmatch(r"([+-]?)(\d+(?:\.\d+)?)(hz|st)", lower)
    if match:
        sign, magnitude, unit = match.groups()
        units_map = {"hz": "Hz", "st": "st"}
        unit_fmt = units_map.get(unit, unit)
        try:
            magnitude_float = float(magnitude)
        except ValueError:
            magnitude_float = 0.0
        if magnitude_float == 0:
            return None
        if not sign:
            sign = "+"
        normalized = f"{sign}{magnitude}{unit_fmt}"
        return normalized
    if logger:
        logger.warning(
            "[TTS] 未识别的 pitch 值 '%s'，已忽略该设置。请使用示例 +2Hz 或 -1st。",
            text,
        )
    return None


def _ensure_event_loop() -> tuple[asyncio.AbstractEventLoop, threading.Thread]:
    loop = asyncio.new_event_loop()

    def _runner():
        asyncio.set_event_loop(loop)
        loop.run_forever()

    thread = threading.Thread(target=_runner, name="edge-tts-loop", daemon=True)
    thread.start()
    return loop, thread


def _schedule_loop_shutdown(loop: asyncio.AbstractEventLoop):
    def _drain_and_stop():
        pending = [task for task in asyncio.all_tasks(loop) if not task.done()]
        for task in pending:
            task.cancel()

        async def _wait_and_stop():
            if pending:
                with suppress(Exception):
                    await asyncio.gather(*pending, return_exceptions=True)
            loop.stop()

        if pending:
            asyncio.ensure_future(_wait_and_stop())
        else:
            loop.stop()

    try:
        loop.call_soon_threadsafe(_drain_and_stop)
    except Exception:
        try:
            loop.call_soon_threadsafe(loop.stop)
        except Exception:
            pass


def _shutdown_loop(
    loop: asyncio.AbstractEventLoop,
    thread: threading.Thread | None,
    logger=None,
):
    if loop is None:
        return
    try:
        if loop.is_running():
            _schedule_loop_shutdown(loop)
    except Exception:
        pass

    if thread and thread.is_alive():
        thread.join(timeout=10)
        if thread.is_alive():
            if logger:
                try:
                    logger.warning("edge-tts 事件循环未在超时时间内停止，已跳过关闭以避免异常。")
                except Exception:
                    pass
            return

    if loop.is_running():
        if logger:
            try:
                logger.warning("edge-tts 事件循环仍在运行，已跳过关闭以避免异常。")
            except Exception:
                pass
        return

    if loop.is_closed():
        return

    try:
        loop.close()
    except RuntimeError as exc:
        if "running event loop" in str(exc) and logger:
            try:
                logger.warning("edge-tts 事件循环关闭失败：%s", exc)
            except Exception:
                pass
        else:
            raise
