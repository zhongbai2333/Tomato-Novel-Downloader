#!/usr/bin/env python3
"""封面文件迁移脚本

用途:
    早期版本封面命名使用原始书名:   {book_name}.jpg
    新版本统一使用安全文件名:      {safe_fs_name(book_name)}.jpg

    本脚本扫描保存根目录 (默认: 当前工作目录或 --root 指定) 下的形如
    <book_id>_<book_name>/ 结构的书籍文件夹, 如果检测到:
        - 存在旧封面  原名: {book_name}.jpg
        - 不存在新封面 安全名: {safe_fs_name(book_name)}.jpg
      则进行重命名迁移。

安全策略:
    1. 如新文件已存在, 跳过并记录。
    2. 如旧文件不存在或无法访问, 跳过并记录。
    3. 如书名清洗后为空, 使用 'unnamed' 兜底。
    4. 提供 --dry-run 仅预览即将发生的迁移操作。

使用示例:
    python tools/migrate_covers.py --root novels
    python tools/migrate_covers.py --root /abs/path --dry-run

退出码:
    0 正常结束 (即使有跳过)
    1 发生致命错误 (参数或根目录不存在)

Author: automated migration utility
"""
from __future__ import annotations
import argparse
import sys
from pathlib import Path
from typing import List, Tuple

# 复用项目内的 safe_fs_name (避免重复逻辑)
try:
    # 相对导入: tools/ 与 novel_src/ 并列时使用此路径
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'novel_src'))
    from base_system.context import Config  # type: ignore
except Exception:
    Config = None  # 兜底: 若导入失败, 使用内部简化实现


def safe_name(name: str) -> str:
    if Config is not None:
        try:
            return Config.safe_fs_name(name)
        except Exception:
            pass
    # fallback 简化清洗
    import re
    name = re.sub(r'[<>:"/\\|?*]', '_', name).strip().rstrip('. ')
    if not name:
        name = 'unnamed'
    return name


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description='封面文件命名迁移工具')
    p.add_argument('--root', type=str, default='novels', help='保存根目录 (默认: novels)')
    p.add_argument('--dry-run', action='store_true', help='仅预览，不真正重命名')
    return p.parse_args()


def find_book_dirs(root: Path) -> List[Path]:
    if not root.exists():
        return []
    dirs = []
    for p in root.iterdir():
        if p.is_dir() and '_' in p.name:
            parts = p.name.split('_', 1)
            if len(parts) == 2 and parts[0].isdigit():
                dirs.append(p)
    return dirs


def plan_migrations(book_dirs: List[Path]) -> List[Tuple[Path, Path]]:
    plans: List[Tuple[Path, Path]] = []
    for d in book_dirs:
        try:
            _id, raw_name = d.name.split('_', 1)
        except ValueError:
            continue
        old_cover = d / f"{raw_name}.jpg"
        new_cover = d / f"{safe_name(raw_name)}.jpg"
        if old_cover == new_cover:
            # 清洗后名字未变化
            continue
        if old_cover.exists() and not new_cover.exists():
            plans.append((old_cover, new_cover))
    return plans


def execute(plans: List[Tuple[Path, Path]], dry_run: bool) -> None:
    if not plans:
        print('[Info] 没有需要迁移的封面文件。')
        return
    for src, dst in plans:
        if dry_run:
            print(f'[DryRun] {src} -> {dst}')
        else:
            try:
                src.rename(dst)
                print(f'[OK] {src.name} -> {dst.name}')
            except Exception as e:
                print(f'[FAIL] {src} -> {dst} ({e})')


def main():
    args = parse_args()
    root = Path(args.root).resolve()
    if not root.exists():
        print(f'[Error] 根目录不存在: {root}')
        sys.exit(1)
    book_dirs = find_book_dirs(root)
    print(f'[Info] 检测到 {len(book_dirs)} 个候选书籍目录。')
    plans = plan_migrations(book_dirs)
    if plans:
        print(f'[Info] 需要迁移 {len(plans)} 个封面文件:')
    for src, dst in plans:
        print(f'  - {src.name} -> {dst.name}')
    execute(plans, args.dry_run)
    if args.dry_run:
        print('[Info] Dry-run 已完成，未对文件做实际修改。')

if __name__ == '__main__':
    main()
