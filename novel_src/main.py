# -------------------------------
# main.py - 主程序模块
# -------------------------------
import re
import os
import time
import requests
from urllib.parse import urlparse, parse_qs
from ascii_magic import AsciiArt

from .base_system.context import GlobalContext
from .network_parser.network import NetworkClient
from .book_parser.book_manager import BookManager
from .network_parser.downloader import ChapterDownloader
from .book_parser.parser import ContentParser
from .constants import VERSION


def show_config_menu(config):
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


def search_book(book_name: str, network: NetworkClient, config, logger) -> str:
    for endpoint in config.api_endpoints:
        api = endpoint + f"/search?query={book_name}&offset=0"
        try:
            response = requests.get(
                url=api, headers=network.get_headers(), timeout=config.request_timeout
            )
            response.raise_for_status()
        except requests.RequestException as e:
            logger.error(f"通过端点 {endpoint} 搜索失败: {str(e)}")
            continue
        data = response.json()
        search_datas = data["search_tabs"][5]["data"]
        book_id_list = []
        for num, search_res in enumerate(search_datas):
            book_info = search_res["book_data"][0]
            logger.info(
                f"{num + 1}. 书名: {book_info['book_name']} | 初始书名: {book_info['original_book_name']} | ID: {book_info['book_id']} | 作者: {book_info['author']}"
            )
            book_id_list.append(book_info["book_id"])
        while True:
            num = input("请输入序号 (输入q返回重新搜索)：")
            if num == "q":
                return "0000"
            if 1 <= int(num) <= len(book_id_list):
                return book_id_list[int(num) - 1]
            else:
                logger.warning("输入错误!")
    return None


def download_book(logger, config, network, log_system, book_id: str, save_path: str):
    try:
        # --- 获取书籍信息 ---
        logger.info("\n正在获取书籍信息...")
        book_info_url = f"https://fanqienovel.com/page/{book_id}"

        # 发送请求
        try:
            response = requests.get(
                book_info_url,
                headers=network.get_headers(),
                timeout=config.request_timeout,
            )
            if response.status_code == 404:
                logger.error(f"小说ID {book_id} 不存在！")
                return None
            response.raise_for_status()
        except requests.RequestException as e:
            logger.error(f"获取书籍信息失败: {str(e)}")
            return None

        # 解析书籍信息
        try:
            book_name, author, description, tags, chapter_count = (
                ContentParser.parse_book_info(response.text)
            )
            cover_path = config.default_save_dir / f"{book_name}.jpg"
            preview_ascii(cover_path, 100)
            logger.info(f"\n书名: {book_name}")
            logger.info(f"作者: {author}")
            logger.info(f"是否完结: {tags[0]} | 共 {chapter_count} 章")
            logger.info(f"标签: {'|'.join(tags[1:])}")
            logger.info(f"简介: {description[:50]}...")  # 显示前50字符
        except Exception as e:
            cover_path = config.default_save_dir / f"None.jpg"
            logger.error(f"解析书籍信息失败: {str(e)}")
            book_name = f"未知书籍_{book_id}"
            author = "未知作者"
            description = "无简介"

        # 初始化书籍管理器
        manager = BookManager(
            save_path,
            book_id,
            book_name,
            author,
            tags,
            description,
        )  # 添加book_name参数

        log_system.add_safe_exit_func(manager.save_download_status)

        # 用户确认
        confirm = input("\n是否开始下载？(Y/n): ").strip().lower()
        if confirm not in ("", "y", "yes"):
            if cover_path.exists():
                os.remove(cover_path)
                logger.debug(f"封面文件已清理！{cover_path}")
            return None

        # 初始化下载器
        downloader = ChapterDownloader(book_id, network)

        # --- 获取章节列表 ---
        logger.info("\n正在获取章节列表...")
        chapters = downloader.fetch_chapter_list()
        if not chapters:
            logger.error("错误：无法获取章节列表")
            return None

        # 显示章节统计
        total = len(chapters)
        keys = [
            key
            for key, value in manager.downloaded.items()
            if value == [key, "Error"]
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

        # 执行下载流程
        result = downloader.download_book(manager, book_name, chapters)

        # 显示统计信息
        time_cost = time.time() - start_time
        logger.info(f"\n下载完成！用时 {time_cost:.1f} 秒")
        logger.info(f"成功: {result['success']} 章")
        logger.info(f"失败: {result['failed']} 章")
        logger.info(f"取消: {result['canceled']} 章")
        return result
    except Exception as e:
        logger.error(f"处理过程中发生错误: {str(e)}")
        return None


def main():
    """命令行入口函数"""
    logger = GlobalContext.get_logger()
    config = GlobalContext.get_config()
    log_system = GlobalContext.get_log_system()

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

    # 初始化核心组件
    network = NetworkClient()
    manager = None  # 类型注解: Optional[BookManager]

    try:
        while True:
            # 用户输入处理
            try:
                user_input = input(
                    "\n请输入 小说ID/书本链接（分享链接）/书本名字 （输入s进入配置菜单 输入q退出）："
                ).strip()

                if user_input.lower() == "q":
                    break
                if user_input.lower() == "s":
                    show_config_menu(config)
                    continue
                if user_input == "":
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
                        found_id = search_book(book_name, network, config, logger)
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

                result = download_book(
                    logger, config, network, log_system, book_id, save_path
                )

                while True and not result is None:
                    if result["failed"] > 0:
                        num = input("是否重新下载错误章节？[Y/n]: ").lower()
                        if num == "n":
                            logger.warning("失败章节已保存到缓存文件")
                            break
                        result = download_book(
                            logger, config, network, log_system, book_id, save_path
                        )
                    else:
                        break

            except KeyboardInterrupt:
                break

    except KeyboardInterrupt:
        if manager:
            try:
                manager.save_download_status()
                logger.info("下载状态已保存")
            except Exception as e:
                logger.error(f"保存状态失败: {str(e)}")
        logger.info("\n操作已取消")


def preview_ascii(image_path, columns=100):
    try:
        print("=" * 46 + "封面预览" + "=" * 46)
        # 转换为ASCII
        art = AsciiArt.from_image(image_path)
        art.to_terminal(columns=columns)
    except Exception as e:
        print(f"生成预览失败：{e}")
