# -------------------------------
# main.py - 主程序模块
# -------------------------------
import os
import time
import requests
from ascii_magic import AsciiArt

from .context import GlobalContext
from .network import NetworkClient
from .book_manager import BookManager
from .downloader import ChapterDownloader
from .parser import ContentParser
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
        "7": {"name": "优雅退出模式[True/False]", "field": "graceful_exit", "type": bool},
        "8": {"name": "小说保存格式[txt/epub]", "field": "novel_format", "type": str},
        "9": {"name": "是否自动清理缓存文件[True/False]", "field": "auto_clear_dump", "type": bool},
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

        # 更新配置
        setattr(config, field, converted)
        print(f"{opt['name']} 已更新为 {converted}")

        # 立即保存配置
        try:
            config.save()
            print("配置已保存")
        except Exception as e:
            print(f"保存配置失败: {str(e)}")


def main():
    """命令行入口函数"""
    logger = GlobalContext.get_logger()
    config = GlobalContext.get_config()

    logger.info(
        f"""欢迎使用番茄小说下载器! v{VERSION}
项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader-Lite
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
                book_id = input(
                    "\n请输入小说ID（输入s进入配置菜单 输入q退出）："
                ).strip()
            except KeyboardInterrupt:
                break
            if book_id.lower() == "q":
                break
            if book_id.lower() == "s":  # 新增配置命令
                show_config_menu(config)
                continue

            # 输入验证
            if not book_id.isdigit():
                logger.info("错误：小说ID必须为纯数字")
                continue

            # 获取保存路径
            save_path = input(
                f"保存路径（默认：{config.default_save_dir}）："
            ).strip() or str(config.default_save_dir)

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
                    response.raise_for_status()
                except requests.RequestException as e:
                    logger.error(f"获取书籍信息失败: {str(e)}")
                    continue

                # 解析书籍信息
                try:
                    book_name, author, description, tags, chapter_count = (
                        ContentParser.parse_book_info(response.text)
                    )
                    cover_path = config.default_save_dir / f"{book_name}.jpg"
                    logger.info(f"\n书名: {book_name}")
                    logger.info(f"作者: {author}")
                    logger.info(f"是否完结: {tags[0]} | 共 {chapter_count} 章")
                    logger.info(f"标签: {'|'.join(tags[1:])}")
                    logger.info(f"简介: {description[:50]}...")  # 显示前50字符
                    preview_ascii(cover_path, 100)
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

                # 用户确认
                confirm = input("\n是否开始下载？(Y/n): ").strip().lower()
                if confirm not in ("", "y", "yes"):
                    if cover_path.exists():
                        os.remove(cover_path)
                        logger.debug(f"封面文件已清理！{cover_path}")
                    continue

                # 初始化下载器
                downloader = ChapterDownloader(book_id, network)

                # --- 获取章节列表 ---
                logger.info("\n正在获取章节列表...")
                chapters = downloader.fetch_chapter_list()
                if not chapters:
                    logger.error("错误：无法获取章节列表")
                    continue

                # 显示章节统计
                total = len(chapters)
                downloaded_count = len(manager.downloaded)
                logger.info(f"共发现 {total} 章，已下载 {downloaded_count} 章")

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
                if result["failed"] > 0:
                    logger.warning("失败章节已保存到 failed_chapters.txt")

            except Exception as e:
                logger.error(f"处理过程中发生错误: {str(e)}")
                continue

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
