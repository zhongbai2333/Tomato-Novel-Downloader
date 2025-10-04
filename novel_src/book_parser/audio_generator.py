import asyncio
import re
from pathlib import Path
from typing import Iterable, List, Tuple


_MAX_CHARS_PER_SEGMENT = 2800


def generate_audiobook(manager, chapters: List[dict]):
    """Convert downloaded chapters to audio files using edge-tts when enabled."""
    config = getattr(manager, "config", None)
    logger = getattr(manager, "logger", None)
    if not config or not getattr(config, "enable_audiobook", False):
        return

    try:
        import edge_tts  # type: ignore
    except ImportError:  # pragma: no cover - runtime guard
        if logger:
            logger.error("edge-tts 未安装，无法生成有声小说。请先通过 pip 安装 edge-tts。")
        return

    voice = getattr(config, "audiobook_voice", "zh-CN-XiaoxiaoNeural")
    rate = getattr(config, "audiobook_rate", "+0%")
    volume = getattr(config, "audiobook_volume", "+0%")
    pitch = getattr(config, "audiobook_pitch", "+0%")
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
        return

    async def _render_all():
        for idx, (title, content) in enumerate(chapters_data, start=1):
            sanitized = _sanitize_for_tts(title, content)
            if not sanitized.strip():
                continue
            file_name = f"{idx:04d}-{config.safe_fs_name(title)}.{fmt}"
            file_path = audio_dir / file_name
            if logger:
                logger.info(f"[TTS] 正在生成章节 {idx}: {file_path.name}")
            try:
                await _synthesize_to_file(edge_tts, sanitized, file_path, voice, rate, volume, pitch)
            except Exception as exc:  # noqa: BLE001
                if logger:
                    logger.error(f"[TTS] 章节 {idx} 生成失败：{exc}")

    if logger:
        logger.info(f"开始生成有声小说，共 {len(chapters_data)} 章 -> {audio_dir}")

    try:
        asyncio.run(_render_all())
    except RuntimeError as exc:
        if "asyncio.run() cannot be called" in str(exc):
            loop = asyncio.new_event_loop()
            try:
                loop.run_until_complete(_render_all())
            finally:
                loop.close()
        else:
            raise

    if logger:
        logger.info(f"有声小说生成完成：{audio_dir}")


def _sanitize_for_tts(title: str, content: str) -> str:
    combined = f"{title}。\n{content}"
    combined = combined.replace("\u3000", " ").replace("&nbsp;", " ")
    combined = re.sub(r"<[^>]+>", " ", combined)
    combined = combined.replace("\r", "\n")
    combined = re.sub(r"\n{2,}", "\n", combined)
    combined = re.sub(r"[\t\f\v]+", " ", combined)
    combined = re.sub(r" {2,}", " ", combined)
    return combined.strip()


async def _synthesize_to_file(edge_tts, text: str, file_path: Path, voice: str, rate: str, volume: str, pitch: str):
    file_path.parent.mkdir(parents=True, exist_ok=True)
    if file_path.exists():
        file_path.unlink()

    async with _AsyncFileWriter(file_path) as writer:
        for chunk_text in _chunk_text(text, _MAX_CHARS_PER_SEGMENT):
            communicator = edge_tts.Communicate(
                text=chunk_text,
                voice=voice,
                rate=rate,
                volume=volume,
                pitch=pitch,
            )
            async for chunk in communicator.stream():
                if chunk["type"] == "audio":
                    await writer.write(chunk["data"])
            await asyncio.sleep(0)


def _chunk_text(text: str, limit: int) -> Iterable[str]:
    text = text.strip()
    if len(text) <= limit:
        yield text
        return

    current = []
    current_len = 0
    paragraphs = [p.strip() for p in text.splitlines() if p.strip()]
    for para in paragraphs:
        sentences = [s for s in re.split(r"(?<=[。！？!?])", para) if s]
        if not sentences:
            sentences = [para]
        for sentence in sentences:
            if not sentence:
                continue
            sentence = sentence.strip()
            if not sentence:
                continue
            if current_len + len(sentence) > limit and current:
                yield "".join(current)
                current = [sentence]
                current_len = len(sentence)
            elif len(sentence) > limit:
                if current:
                    yield "".join(current)
                    current = []
                    current_len = 0
                for i in range(0, len(sentence), limit):
                    piece = sentence[i : i + limit]
                    if piece:
                        yield piece
            else:
                current.append(sentence)
                current_len += len(sentence)
    if current:
        yield "".join(current)


class _AsyncFileWriter:
    def __init__(self, path: Path, chunk_size: int = 8192):
        self.path = path
        self.chunk_size = chunk_size
        self._loop = None
        self._handle = None

    async def __aenter__(self):
        self._loop = asyncio.get_running_loop()
        self._handle = open(self.path, "wb")
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        if self._handle:
            await self._loop.run_in_executor(None, self._handle.close)
            self._handle = None

    async def write(self, data: bytes):
        if not self._handle:
            raise RuntimeError("文件未打开")
        await self._loop.run_in_executor(None, self._handle.write, data)
