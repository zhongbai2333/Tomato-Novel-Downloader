import re
import os
import sys
import time
import shutil
from urllib.parse import urlparse, parse_qs
from ascii_magic import AsciiArt

from .base_system.context import GlobalContext, Config
from .base_system.storge_system import FileCleaner
from .book_parser.book_manager import BookManager
from .network_parser.network import NetworkClient
from .network_parser.downloader import ChapterDownloader
from .constants import VERSION


def show_config_menu(config: Config):
    """显示配置菜单"""
    print("\n=== 配置选项 ===")
    options = {
        "1": {"name": "保存路径", "field": "save_path", "type": str},
        "2": {"name": "最大线程数", "field": "max_workers", "type": int},
        "3": {"name": "请求超时(秒)", "field": "request_timeout", "type": int},
        "4": {"name": "最大重试次数", "field": "max_retries", "type": int},
        "5": {"name": "最小等待时间(ms)", "field": "min_wait_time", "type": int},
        "6": {"name": "最大等待时间(ms)", "field": "max_wait_time", "type": int},
        "7": {
            "name": "优雅退出模式[True/False]",
            "field": "graceful_exit",
            "type": bool,
        },
        "8": {"name": "小说保存格式[txt/epub]", "field": "novel_format", "type": str},
        "9": {
            "name": "是否自动清理缓存文件[True/False]",
            "field": "auto_clear_dump",
            "type": bool,
        },
        "A": {
            "name": "是否使用官方API[True/False]",
            "field": "use_official_api",
            "type": bool,
        },
        "0": {"name": "返回主菜单"},
    }

    while True:
        print("\n当前配置：")
        for key in sorted(options.keys()):
            if key == "0":
                continue
            opt = options[key]
            current_value = getattr(config, opt["field"], "N/A")
            print(f"{key}. {opt['name']}: {current_value}")
        print("0. 返回主菜单")

        choice = input("\n请选择要修改的配置项：").strip()

        if choice == "0":
            break

        if choice not in options:
            print("无效选项，请重新输入")
            continue

        if choice == "0":
            break

        # 获取配置项元数据
        opt = options[choice]
        field = opt["field"]
        value_type = opt["type"]
        current_value = getattr(config, field)

        # 显示当前值并获取新值
        print(f"\n当前 {opt['name']}: {current_value}")
        new_value = input("请输入新值（留空取消修改）: ").strip()

        if not new_value:
            print("修改已取消")
            continue

        # 验证并转换值类型
        try:
            if value_type == bool:
                converted = new_value.lower() in ("true", "1", "yes")
            else:
                converted = value_type(new_value)
        except ValueError:
            print(f"无效值类型，需要 {value_type.__name__}")
            continue

        # 特殊验证逻辑
        if field == "max_workers":
            if converted < 1 or converted > 16:
                print("线程数必须在1-16之间")
                continue
        elif field in ("min_wait_time", "max_wait_time"):
            if converted < 0:
                print("等待时间不能为负数")
                continue
        elif field == "request_timeout":
            if converted < 5:
                print("请求超时时间不能小于5秒")
                continue
        elif field == "novel_format":
            if converted not in ["txt", "epub"]:
                print("格式应为txt或epub")
                continue
        elif field == "save_path":
            converted = converted.rstrip("/")

        # 更新配置
        setattr(config, field, converted)
        print(f"{opt['name']} 已更新为 {converted}")

        # 立即保存配置
        try:
            config.save()
            print("配置已保存")
        except Exception as e:
            print(f"保存配置失败: {str(e)}")


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
    manager = None
    downloader = None

    logger.info(
        f"""欢迎使用番茄小说下载器! v{VERSION}
项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader
Fork From: https://github.com/Dlmily/Tomato-Novel-Downloader-Lite
作者: zhongbai233 (https://github.com/zhongbai2333)
项目早期代码: Dlmily (https://github.com/Dlmily)

项目说明: 此项目基于Dlmily的项目Fork而来, 我对其进行重构 + 优化, 添加更对功能, 包括: EPUB下载支持、更好的断点传输、更好的错误管理等特性
本项目完全基于第三方API, 未使用官方API, 由于官方API地址一直在变动, 我也无精力一直适配官方API, 如有需要可以查看Dlmily的项目
本项目仅供 Python 网络爬虫技术、网页数据处理及相关研究的学习用途。请勿将其用于任何违反法律法规或侵犯他人权益的活动。
"""
    )

    try:
        while True:
            user_input = input(
                "\n请输入 小说ID/书本链接（分享链接）/书本名字 （输入s进入配置菜单 输入q退出）："
            ).strip()

            if user_input == "":
                continue
            if user_input.lower() == "q":
                break
            if user_input.lower() == "s":
                show_config_menu(config)
                continue

            book_id = None

            # 1. 首先尝试从文本中提取 URL
            urls = re.findall(r"(https?://[^\s]+)", user_input)
            if urls:
                url_str = urls[0]  # 取第一个找到的链接
                parsed = urlparse(url_str)
                # 尝试解析 /page/<book_id> 模式
                m = re.search(r"/page/(\d+)", parsed.path)
                if m:
                    book_id = m.group(1)
                else:
                    # 从 query 参数中尝试获取 book_id 或 bookId
                    qs = parse_qs(parsed.query)
                    bid_list = qs.get("book_id") or qs.get("bookId")
                    if bid_list:
                        book_id = bid_list[0]

                if not book_id:
                    logger.info("错误：无法从链接中解析出 book_id，请检查链接格式")
                    continue

            # 2. 如果没有提取到 URL，再判断是否为纯数字 ID
            if not book_id:
                if user_input.isdigit():
                    book_id = user_input
                else:
                    # 3. 作为书名处理，调用搜索接口获取book_id
                    book_name = user_input
                    found_id = network.search_book(book_name)
                    if found_id and found_id == "0000":
                        continue
                    elif found_id:
                        book_id = found_id
                    else:
                        logger.error("API获取信息异常!")
                        continue

            # 获取保存路径
            save_path = input(
                f"保存路径（默认：{config.default_save_dir}）："
            ).strip() or str(config.default_save_dir)

            book_name, author, description, tags, chapter_count = network.get_book_info(
                book_id
            )
            if book_name is None:
                continue

            folder_path = config.get_status_folder_path
            cover_path = folder_path / f"{book_name}.jpg"
            preview_ascii(cover_path)
            logger.info(f"\n书名: {book_name}")
            logger.info(f"作者: {author}")
            logger.info(f"是否完结: {tags[0]} | 共 {chapter_count} 章")
            logger.info(f"标签: {'|'.join(tags[1:])}")
            logger.info(f"简介: {description[:50]}...")  # 显示前50字符

            manager = BookManager(
                save_path, book_id, book_name, author, tags, description
            )

            log_system.add_safe_exit_func(manager.save_download_status)

            # 用户确认
            confirm = input("\n是否开始下载？(Y/n): ").strip().lower()
            if confirm not in ("", "y", "yes"):
                if cover_path.exists():
                    os.remove(cover_path)
                    logger.debug(f"封面文件已清理！{cover_path}")
                    if FileCleaner.is_empty_dir(folder_path):
                        FileCleaner.clean_dump_folder(folder_path)
                return None

            chapter_list = network.fetch_chapter_list(book_id)
            if chapter_list is None:
                continue

            total = len(chapter_list)
            keys = [
                key for key, value in manager.downloaded.items() if value == [key, "Error"]
            ]
            downloaded_failed = len(keys)
            downloaded_count = len(manager.downloaded) - len(keys)
            logger.info(
                f"共发现 {total} 章，下载失败 {downloaded_failed} 章，已下载 {downloaded_count} 章"
            )
            # 检查已有下载
            if downloaded_count > 0:
                choice = input(
                    "检测到已有下载记录：\n1. 继续下载\n2. 重新下载\n请选择(默认1): "
                ).strip()
                if choice == "2":
                    manager.downloaded.clear()
                    logger.info("已清除下载记录，将重新下载全部章节")

            # --- 执行下载 ---
            logger.info("\n开始下载...")
            start_time = time.time()

            downloader = ChapterDownloader(book_id, network)
            # 执行下载流程
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
