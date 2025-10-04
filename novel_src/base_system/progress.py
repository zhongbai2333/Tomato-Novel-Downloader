"""统一的 tqdm 进度条工具"""
from __future__ import annotations

import shutil
from typing import Iterable, Optional


DEFAULT_BAR_FORMAT = "[{elapsed}] {desc} {percentage:3.0f}%|{bar}| {n_fmt}/{total_fmt} ETA:{remaining}"
_FALLBACK_COLS = 120


def get_terminal_columns(fallback: int = _FALLBACK_COLS) -> int:
    """获取当前终端列数，提供统一的回退值。"""
    try:
        cols, _ = shutil.get_terminal_size(fallback=(fallback, 30))
    except Exception:
        cols = fallback
    if not isinstance(cols, int) or cols <= 0:
        cols = fallback
    return cols


def build_tqdm_common_kwargs(
    *,
    leave: bool = True,
    mininterval: float = 0.25,
    columns: Optional[int] = None,
    bar_format: str = DEFAULT_BAR_FORMAT,
) -> tuple[int, dict]:
    """构建统一的 tqdm 初始化参数，并返回列数。"""
    cols = columns if columns is not None else get_terminal_columns()
    kwargs = {
        "mininterval": mininterval,
        "dynamic_ncols": False,
        "leave": leave,
        "ncols": cols,
        "bar_format": bar_format,
    }
    return cols, kwargs


def refresh_progress_bars(bars: Iterable, columns: Optional[int] = None) -> int:
    """刷新一组进度条的列宽，保持样式一致。"""
    cols = columns if columns is not None else get_terminal_columns()
    for bar in bars:
        if bar is None:
            continue
        try:
            bar.ncols = cols
            bar.refresh(nolock=True)
        except Exception:
            pass
    return cols
