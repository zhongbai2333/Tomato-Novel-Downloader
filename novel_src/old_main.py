import re
import os
import sys
import time
import json
import shutil
from pathlib import Path
from typing import Optional, List, Tuple
from logging import Logger
from urllib.parse import urlparse, parse_qs
from ascii_magic import AsciiArt

from .base_system.context import GlobalContext, Config
from .base_system.storage_system import FileCleaner
from .book_parser.book_manager import BookManager
from .network_parser.network import NetworkClient
from .network_parser.downloader import ChapterDownloader
from .update import UpdateManager
from .constants import VERSION


def show_config_menu(config: Config):
    """显示配置菜单 (已同步新版 main 中的所有配置字段)。

    支持类型:
      bool / int / float / str / list(逗号或换行分隔)
    互斥逻辑:
      use_official_api 与 use_helloplhm_qwq_api
    自动调整:
      启用 helloplhm_qwq API 时: max_workers=1,min_wait_time>=1000,max_wait_time>=1200
    """

    # 定义所有可编辑配置项 (顺序即菜单顺序)
    option_defs = [
        {"name": "保存路径", "field": "save_path", "type": str},
        {"name": "小说保存格式(txt/epub)", "field": "novel_format", "type": str},
        {"name": "是否以散装形式保存小说", "field": "bulk_files", "type": bool},
        {"name": "优雅退出模式", "field": "graceful_exit", "type": bool},
        {"name": "是否自动清理缓存文件", "field": "auto_clear_dump", "type": bool},
        # 网络
        {"name": "最大线程数", "field": "max_workers", "type": int},
        {"name": "请求超时(秒)", "field": "request_timeout", "type": int},
        {"name": "最大重试次数", "field": "max_retries", "type": int},
        {"name": "最小等待时间(ms)", "field": "min_wait_time", "type": int},
        {"name": "最大等待时间(ms)", "field": "max_wait_time", "type": int},
        {"name": "最小连接超时时间", "field": "min_connect_timeout", "type": float},
        {"name": "强制退出等待时间(秒)", "field": "force_exit_timeout", "type": int},
        # API
        {"name": "是否使用官方API", "field": "use_official_api", "type": bool},
        {"name": "是否使用 helloplhm_qwq API", "field": "use_helloplhm_qwq_api", "type": bool},
        {"name": "自定义API列表(逗号分隔)", "field": "api_endpoints", "type": list},
        # 段评
        {"name": "是否下载段评", "field": "enable_segment_comments", "type": bool},
        {"name": "段评每段最多条数", "field": "segment_comments_top_n", "type": int},
        {"name": "段评并发线程数", "field": "segment_comments_workers", "type": int},
        # 段评媒体
        {"name": "是否下载评论区图片", "field": "download_comment_images", "type": bool},
        {"name": "是否下载评论区头像", "field": "download_comment_avatars", "type": bool},
        {"name": "评论图片下载线程数", "field": "media_download_workers", "type": int},
        {"name": "图片域名黑名单(逗号分隔)", "field": "blocked_media_domains", "type": list},
        # 图片处理
        {"name": "强制所有图片转JPEG", "field": "force_convert_images_to_jpeg", "type": bool},
        {"name": "非JPEG尝试转JPEG", "field": "jpeg_retry_convert", "type": bool},
        {"name": "JPEG质量(0-100)", "field": "jpeg_quality", "type": int},
        {"name": "HEIC转JPEG", "field": "convert_heic_to_jpeg", "type": bool},
        {"name": "保留原始HEIC文件", "field": "keep_heic_original", "type": bool},
    # 媒体体积控制
    {"name": "每章媒体数量上限(0为不限制)", "field": "media_limit_per_chapter", "type": int},
    {"name": "图片最长边像素上限(>0生效)", "field": "media_max_dimension_px", "type": int},
    {"name": "会话媒体总下载上限(MB,0不限制)", "field": "media_total_limit_mb", "type": int},
        # 段落缩进
        {"name": "EPUB首行缩进(em)", "field": "first_line_indent_em", "type": float},
        # 旧界面切换
        {"name": "是否使用老版本命令行界面(需重启)", "field": "old_cli", "type": bool},
    ]

    def _show_menu():
        print("\n=== 配置选项 ===")
        for idx, opt in enumerate(option_defs, start=1):
            val = getattr(config, opt["field"], "<N/A>")
            if isinstance(val, list):
                display = ",".join(map(str, val))
            else:
                display = val
            name = opt['name']
            if opt["field"] == "enable_segment_comments" and getattr(config, "novel_format", "epub") == "txt":
                name = f"{name}（TXT 不支持）"
            print(f"{idx}. {name}: {display}")
        print("0. 返回主菜单")

    while True:
        _show_menu()
        choice = input("\n请选择要修改的配置项编号: ").strip()
        if choice == "0":
            break
        if not choice.isdigit():
            print("请输入数字编号")
            continue
        idx = int(choice)
        if not (1 <= idx <= len(option_defs)):
            print("编号超出范围")
            continue
        opt = option_defs[idx - 1]
        field = opt["field"]
        typ = opt["type"]
        cur_val = getattr(config, field, None)
        if isinstance(cur_val, list):
            cur_display = ",".join(cur_val)
        else:
            cur_display = cur_val
        new_text = input(f"当前 {opt['name']} = {cur_display}\n输入新值(留空取消): ").strip()
        if new_text == "":
            print("已取消修改")
            continue

        # 解析与转换
        try:
            if typ is bool:
                new_val = new_text.lower() in ("true", "1", "yes", "y")
            elif typ is int:
                new_val = int(new_text)
            elif typ is float:
                new_val = float(new_text)
            elif typ is list:
                parts = [p.strip() for p in re.split(r"[,\n]", new_text) if p.strip()]
                new_val = parts
            else:
                new_val = new_text
        except ValueError:
            print("类型转换失败")
            continue

        # 校验
        if field == "novel_format" and new_val not in ("txt", "epub"):
            print("小说保存格式必须为 txt 或 epub")
            continue
        if field in ("max_workers", "max_retries", "segment_comments_top_n", "segment_comments_workers", "media_download_workers", "media_limit_per_chapter", "media_max_dimension_px", "media_total_limit_mb", "force_exit_timeout"):
            if new_val < 0:
                print("该数值不能为负")
                continue
        if field in ("min_wait_time", "max_wait_time") and new_val < 0:
            print("等待时间不能为负")
            continue
        if field == "jpeg_quality" and not (0 <= new_val <= 100):
            print("JPEG质量需在0-100之间")
            continue
        if field == "min_connect_timeout" and new_val <= 0:
            print("最小连接超时时间需>0")
            continue
        if field == "save_path" and new_val:
            new_val = new_val.rstrip("/\\")
            try:
                Path(new_val).mkdir(parents=True, exist_ok=True)
            except Exception as e:
                print(f"创建目录失败: {e}")
                continue

        # 互斥与自动调整
        if field == "use_official_api" and new_val:
            if getattr(config, "use_helloplhm_qwq_api", False):
                config.use_helloplhm_qwq_api = False
                print("已自动关闭 helloplhm_qwq API")
        if field == "use_helloplhm_qwq_api" and new_val:
            msgs = []
            if getattr(config, "use_official_api", False):
                config.use_official_api = False
                msgs.append("关闭 官方API")
            if getattr(config, "max_workers", None) != 1:
                config.max_workers = 1; msgs.append("max_workers=1")
            if getattr(config, "min_wait_time", 0) < 1000:
                config.min_wait_time = 1000; msgs.append("min_wait_time>=1000")
            if getattr(config, "max_wait_time", 0) < 1200:
                config.max_wait_time = 1200; msgs.append("max_wait_time>=1200")
            if msgs:
                print("启用 helloplhm_qwq API 已自动调整: " + "; ".join(msgs))

        # novel_format 与 段评互斥：
        # 1) 当用户把 novel_format 设为 txt，则强制关闭段评并提示
        if field == "novel_format" and new_val == "txt":
            if getattr(config, "enable_segment_comments", False):
                config.enable_segment_comments = False
                print("TXT 格式不支持段评，已自动关闭段评功能。")
        # 2) 当用户开启段评而当前为 txt，自动切换到 epub 并提示
        if field == "enable_segment_comments" and bool(new_val):
            if getattr(config, "novel_format", "epub") == "txt":
                config.novel_format = "epub"
                print("已自动将保存格式切换为 EPUB 以启用段评功能。")

        setattr(config, field, new_val)
        try:
            config.save()
            short_val = new_val
            if isinstance(new_val, list) and len(new_val) > 8:
                short_val = ",".join(new_val[:8]) + f"...(共{len(new_val)}项)"
            elif isinstance(new_val, list):
                short_val = ",".join(new_val)
            print(f"已更新 {opt['name']} = {short_val}")
        except Exception as e:
            print(f"保存配置失败: {e}")


def load_download_status(status_path: Path) -> dict:
    """
    读取并返回 status_path 对应的 JSON 数据，失败时返回空 dict。
    """
    try:
        with status_path.open("r", encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        return {}
    except json.JSONDecodeError as e:
        # 如果日志中需要详细信息，可以：logger.error(f"JSON 解析错误：{e}")
        return {}


def update_menu(
    config: Config, logger: Logger, network: NetworkClient
) -> Optional[str]:
    """
    检查本地保存目录下可更新的小说，并让用户选择要更新的书籍 ID。
    返回选中书籍的 book_id，或在取消/无可更新时返回 None。
    """
    save_dir: Path = Path(config.default_save_dir)
    subdirs = list_subdirs(save_dir)

    if not subdirs:
        logger.info("没有可供更新的小说")
        return None

    # 分离有更新和无更新列表
    update_choices: List[Tuple[str, str]] = []
    no_update_choices: List[Tuple[str, str]] = []
    for folder in subdirs:
        if "_" not in folder:
            continue
        book_id, book_name = folder.split("_", 1)
        if not book_id.isdigit():
            continue

        folder_path = save_dir / folder

        # 1. 先尝试新版 status.json
        status_json_path = folder_path / "status.json"
        status_data = {}
        downloaded = {}
        if status_json_path.exists():
            status_data = load_download_status(status_json_path)
            downloaded = status_data.get("downloaded") or {}
        else:
            # 2. 回退旧版 chapter_status_{book_id}.json
            legacy_path = folder_path / f"chapter_status_{book_id}.json"
            if legacy_path.exists():
                legacy_data = load_download_status(legacy_path)
                # 旧文件有可能直接就是 {chapter_id: [...]} 结构
                if "downloaded" in legacy_data:
                    downloaded = legacy_data.get("downloaded") or {}
                else:
                    downloaded = legacy_data
            else:
                # 两种文件都不存在，跳过
                continue

        # 拉取最新章节目录（放在读取本地状态后，若远端失败可更好诊断）
        try:
            chapters = network.fetch_chapter_list(book_id)
        except Exception as e:
            logger.error(f"获取章节列表失败：{book_id}，原因：{e}")
            continue

        # 计算新增章节数量
        try:
            downloaded_count = len(downloaded)
            new_count = max(len(chapters) - downloaded_count, 0)
        except Exception:
            new_count = 0

        desc = f"《{book_name}》({book_id}) — 新章节：{new_count}"
        if new_count > 0:
            update_choices.append((book_id, desc))
        else:
            no_update_choices.append((book_id, desc))

    if not update_choices and not no_update_choices:
        logger.info("没有合法格式的小说")
        return None

    def select_from_list(choices: List[Tuple[str, str]], title: str) -> Optional[str]:
        """通用子菜单选择函数"""
        while True:
            logger.info(f"===== {title} =====")
            for idx, (_id, desc) in enumerate(choices, start=1):
                logger.info(f"{idx}. {desc}")
            logger.info("q. 取消并返回上级菜单")
            user_input = input("请输入编号：").strip().lower()
            if user_input == "q":
                return None
            if not user_input.isdigit():
                print("错误：请输入数字编号或 q 返回。")
                continue
            idx = int(user_input)
            if 1 <= idx <= len(choices):
                logger.info(f"已选择：{choices[idx-1][1]}")
                return choices[idx - 1][0]
            print(f"错误：请输入 1 到 {len(choices)} 之间的数字，或 q 返回。")

    # 主菜单循环
    while True:
        logger.info("===== 可供更新的小说列表 =====")
        for idx, (_id, desc) in enumerate(update_choices, start=1):
            logger.info(f"{idx}. {desc}")
        opt_no_update = None
        if no_update_choices:
            opt_no_update = len(update_choices) + 1
            logger.info(f"{opt_no_update}. 无更新 ({len(no_update_choices)})")
        logger.info("q. 退出")

        user_input = input("请输入编号：").strip().lower()
        if user_input == "q":
            logger.info("已取消更新")
            return None
        if not user_input.isdigit():
            print("错误：请输入数字编号或 q 退出。")
            continue

        sel = int(user_input)
        # 选择有更新的书
        if 1 <= sel <= len(update_choices):
            logger.info(f"已选择更新：{update_choices[sel-1][1]}")
            return update_choices[sel - 1][0]
        # 进入无更新子菜单
        if opt_no_update and sel == opt_no_update:
            picked = select_from_list(no_update_choices, "无更新的书籍")
            if picked:
                return picked
            # 返回主菜单，继续循环
            continue

        print(
            f"错误：请输入 1 到 {opt_no_update or len(update_choices)} 之间的数字，或 q 退出。"
        )


def list_subdirs(path):
    """
    返回指定目录下所有一级子文件夹的名称列表（不含文件）。
    """
    return [
        name for name in os.listdir(path) if os.path.isdir(os.path.join(path, name))
    ]


def preview_ascii(image_path):
    try:
        cols, _ = shutil.get_terminal_size(fallback=(80, 24))
        code_cols = int((cols - 8) / 2)
        print("=" * code_cols + "封面预览" + "=" * code_cols)
        # 转换为ASCII
        art = AsciiArt.from_image(image_path)
        art.to_terminal(columns=cols)
    except Exception as e:
        print(f"生成预览失败：{e}")


def main() -> None:
    """主程序"""
    logger = GlobalContext.get_logger()
    config = GlobalContext.get_config()
    log_system = GlobalContext.get_log_system()
    network = NetworkClient()
    update = UpdateManager()
    manager = None
    downloader = None

    if not log_system.debug:
        update.check_for_updates()

    logger.info(
        f"""欢迎使用番茄小说下载器! v{VERSION}
项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader
Fork From: https://github.com/Dlmily/Tomato-Novel-Downloader-Lite
作者: zhongbai233 (https://github.com/zhongbai2333)
项目早期代码: Dlmily (https://github.com/Dlmily)

项目说明: 此项目基于Dlmily的项目Fork而来, 我对其进行重构 + 优化, 添加更对功能, 包括: EPUB下载支持、更好的断点传输、更好的错误管理等特性
本项目[完全]基于第三方API, [未]使用官方API, 如有需要可以查看Dlmily的项目
本项目仅供 Python 网络爬虫技术、网页数据处理及相关研究的学习用途。请勿将其用于任何违反法律法规或侵犯他人权益的活动。
"""
    )

    try:
        while True:
            # 先清空上一轮的 book_id，避免 UnboundLocalError
            book_id: Optional[str] = None

            user_input = input(
                "\n请输入 小说ID/书本链接（分享链接）/书本名字 "
                "（输入s进入配置菜单 输入u进入更新菜单 输入q退出）："
            ).strip()

            if user_input == "":
                continue
            if user_input.lower() == "q":
                break
            if user_input.lower() == "s":
                show_config_menu(config)
                continue

            # 更新菜单
            if user_input.lower() == "u":
                book_id = update_menu(config, logger, network)
                if book_id is None:
                    # 用户取消更新，回到循环开头
                    continue

            # 如果还没拿到 ID，就尝试从分享链接中提取
            if not book_id:
                urls = re.findall(r"(https?://[^\s]+)", user_input)
                if urls:
                    url_str = urls[0]
                    parsed = urlparse(url_str)
                    m = re.search(r"/page/(\d+)", parsed.path)
                    if m:
                        book_id = m.group(1)
                    else:
                        qs = parse_qs(parsed.query)
                        bid_list = qs.get("book_id") or qs.get("bookId")
                        if bid_list:
                            book_id = bid_list[0]
                    if not book_id:
                        logger.info("错误：无法从链接中解析出 book_id，请检查链接格式")
                        continue

            # 如果还是没有 ID，而且输入全为数字，就当作纯数字 ID
            if not book_id and user_input.isdigit():
                book_id = user_input

            # 走到这里，如果还没拿到 book_id，就当作书名，调用新版 search_book 返回多条结果
            if not book_id:
                book_name = user_input
                # 显示简单加载提示，避免用户误以为卡死重复回车
                print("[正在搜索，请稍候...]", end="", flush=True)
                try:
                    results = network.search_book(book_name)
                except Exception as e:
                    print()  # 换行
                    logger.error(f"搜索小说失败：{e}")
                    continue
                print(" 完成")

                if not results:
                    logger.info("未搜索到对应书籍")
                    continue

                # 打印搜索结果，让用户选择
                print("\n===== 搜索结果 =====")
                for idx, item in enumerate(results, start=1):
                    print(
                        f"{idx}. {item['title']} | 作者: {item['author']} | ID: {item['book_id']}"
                    )
                print("q. 取消")

                sel = input("请输入编号选择要下载的书籍：").strip().lower()
                if sel == "q":
                    continue
                if not sel.isdigit() or not (1 <= int(sel) <= len(results)):
                    print("输入编号无效，已取消本次搜索")
                    continue

                book_id = results[int(sel) - 1]["book_id"]

            # 获取保存路径
            save_path = input(
                f"保存路径（默认：{config.default_save_dir}）："
            ).strip() or str(config.default_save_dir)

            book_name, author, description, tags, chapter_count = network.get_book_info(
                book_id
            )
            if book_name is None:
                continue

            # 先生成状态目录（与新 main 保持一致，便于封面与下载器使用）
            try:
                folder_path = config.status_folder_path(book_name, book_id, save_path)
            except Exception:
                folder_path = Path(config.default_save_dir) / f"{book_id}_{book_name}"
                folder_path.mkdir(parents=True, exist_ok=True)
            try:
                safe_name = config.safe_fs_name(book_name)
            except Exception:
                safe_name = book_name.replace(':', '_')
            cover_path = folder_path / f"{safe_name}.jpg"
            # 仅在封面文件已存在时才进行 ASCII 预览，避免 FileNotFoundError
            if cover_path.exists():
                preview_ascii(cover_path)
            else:
                logger.debug(f"封面尚未下载，跳过预览: {cover_path}")
            logger.info(f"\n书名: {book_name}")
            logger.info(f"作者: {author}")
            logger.info(f"是否完结: {tags[0]} | 共 {chapter_count} 章")
            logger.info(f"标签: {'|'.join(tags[1:])}")
            logger.info(f"简介: {description[:50]}...")  # 显示前50字符

            # 适配新版 BookManager 构造 (config, logger)，其余元数据手动赋值
            manager = BookManager(config, logger)
            manager.book_name = book_name or ""
            manager.book_id = book_id or ""
            manager.author = author or ""
            try:
                manager.tags = "|".join(tags) if isinstance(tags, (list, tuple)) else str(tags or "")
                if isinstance(tags, (list, tuple)) and tags:
                    manager.end = (str(tags[0]).find("完结") != -1)
            except Exception:
                manager.tags = ""
            manager.description = description or ""
            try:
                setattr(config, "output_dir", save_path)
            except Exception:
                pass

            # 加载既有下载进度（断点续传）
            try:
                if manager.load_existing_status(book_id, book_name):
                    logger.info("已检测到历史下载记录，可继续下载或选择重新下载。")
            except Exception:
                logger.debug("加载历史进度时出现异常，忽略继续")

            log_system.add_safe_exit_func(manager.save_download_status)

            # ========== 断点续传 / 下载模式选择 ==========
            all_chapters = network.fetch_chapter_list(book_id)
            if all_chapters is None:
                continue
            total = len(all_chapters)
            # 失败章节: content 为空或第二项为 None / 'Error'
            failed_chapter_ids = [
                cid for cid, val in manager.downloaded.items() if not val or len(val) < 2 or val[1] in (None, "Error")
            ]
            downloaded_ok = len(manager.downloaded) - len(failed_chapter_ids)
            logger.info(
                f"共发现 {total} 章，下载失败 {len(failed_chapter_ids)} 章，已下载 {downloaded_ok} 章"
            )

            # 如果没有任何历史记录且没有失败章节，直接询问范围或全部
            def _select_range(ch_list: list[dict]) -> list[dict]:
                rng = input("输入章节范围 形如 10~200 (留空表示全部): ").strip()
                if not rng:
                    return ch_list
                if "~" not in rng:
                    print("范围格式错误，应为 a~b")
                    return ch_list
                try:
                    a, b = map(int, rng.split("~", 1))
                except Exception:
                    print("范围解析失败，使用全部章节")
                    return ch_list
                a = max(a, 1)
                b = min(b, len(ch_list))
                if a > b:
                    a, b = b, a
                subset = [c for c in ch_list if a - 1 <= int(c.get("index", -1)) <= b - 1]
                logger.info(f"已选择章节范围: {a}~{b} -> {len(subset)} 章")
                return subset or ch_list

            mode = None
            if downloaded_ok > 0 or failed_chapter_ids:
                print("\n===== 下载模式选择 =====")
                print("1. 继续下载未完成章节")
                print("2. 全部重新下载")
                if failed_chapter_ids:
                    print("3. 仅重新下载失败章节")
                print("4. 指定章节范围重新下载 (忽略历史记录)")
                print("q. 取消")
                sel = input("请选择(默认1): ").strip().lower()
                if sel in ("", "1"):
                    mode = "resume"
                elif sel == "2":
                    mode = "full"
                elif sel == "3" and failed_chapter_ids:
                    mode = "failed"
                elif sel == "4":
                    mode = "range"
                elif sel == "q":
                    # 取消执行，清理可能生成的空封面
                    if cover_path.exists() and FileCleaner.is_empty_dir(folder_path):
                        try:
                            cover_path.unlink()
                        except Exception:
                            pass
                    continue
                else:
                    mode = "resume"
            else:
                # 没有历史记录
                mode = "range_or_all"

            # 根据模式准备章节列表
            if mode == "full":
                manager.downloaded.clear()
                chapter_list = all_chapters
                logger.info("将重新下载全部章节")
            elif mode == "failed":
                # 仅失败章节
                failed_set = set(failed_chapter_ids)
                chapter_list = [c for c in all_chapters if str(c.get("id")) in failed_set]
                logger.info(f"将重新下载失败章节: {len(chapter_list)} 章")
            elif mode == "range":
                manager.downloaded.clear()
                chapter_list = _select_range(all_chapters)
            elif mode == "range_or_all":
                chapter_list = _select_range(all_chapters)
            else:  # resume
                # 继续：过滤掉已成功的章节
                existing_ids_ok = {
                    cid for cid, val in manager.downloaded.items() if val and len(val) >= 2 and val[1] not in (None, "Error")
                }
                chapter_list = [c for c in all_chapters if str(c.get("id")) not in existing_ids_ok]
                logger.info(f"继续下载剩余章节: {len(chapter_list)} 章 (已完成 {downloaded_ok})")

            if not chapter_list:
                logger.info("没有需要下载的章节，操作结束。")
                continue

            # --- 执行下载 ---
            logger.info("\n开始下载...")
            start_time = time.time()
            downloader = ChapterDownloader(book_id, network)
            result = downloader.download_book(manager, book_name, chapter_list)

            # 显示统计信息
            time_cost = time.time() - start_time
            logger.info(f"\n下载完成！用时 {time_cost:.1f} 秒")
            logger.info(f"成功: {result['success']} 章")
            logger.info(f"失败: {result['failed']} 章")
            logger.info(f"取消: {result['canceled']} 章")

            while True and not result is None:
                if result["failed"] > 0:
                    num = input("是否重新下载错误章节？[Y/n]: ").lower()
                    if num == "n":
                        logger.warning("失败章节已保存到缓存文件")
                        break
                    result = downloader.download_book(manager, book_name, chapter_list)
                else:
                    break

    except (KeyboardInterrupt, EOFError):
        logger.info("\n操作已取消")
        sys.exit(0)
