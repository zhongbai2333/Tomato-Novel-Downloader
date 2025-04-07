import argparse
import sys

from novel_src.constants import VERSION
from novel_src.base_system.context import GlobalContext
from novel_src.main import main as n_main


def main():
    # 参数解析器配置
    parser = argparse.ArgumentParser(
        description="Tomato Novel Downloader",
        formatter_class=argparse.RawTextHelpFormatter,
        add_help=False,
    )

    parser.add_argument("--debug", action="store_true", help="启用调试模式（详细输出）")
    parser.add_argument("--version", action="store_true", help="显示程序版本信息")
    parser.add_argument("-h", "--help", action="store_true", help="显示帮助信息并退出")

    # 参数解析
    args = parser.parse_args()

    # 处理系统参数
    if args.help:
        parser.print_help()
        sys.exit(0)

    if args.version:
        print(f"Tomato Novel Downloader v{VERSION}")
        sys.exit(0)

    # 主逻辑分发
    debug_mode = args.debug
    GlobalContext(debug_mode)

    n_main()

if __name__ == "__main__":
    main()
