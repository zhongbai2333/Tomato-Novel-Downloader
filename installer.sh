#!/usr/bin/env bash
#
# 文件名：installer.sh
# 功能：自动从 GitHub 获取 TomatoNovelDownloader 最新版本，
#       询问用户安装路径（默认脚本执行路径），
#       提示用户是否使用 moeyy 代理下载 Release 资产，
#       在 Termux 环境下安装 glibc-repo、glibc-runner 并生成 run.sh；
#       在 Linux（x86_64/ARM64）/macOS（x86_64/ARM64）下只下载对应架构的二进制并赋予执行权限即可运行。
#
# 使用方法：
#   chmod +x installer.sh
#   ./installer.sh
#

set -e

## ———— 1. 询问安装目录（默认脚本执行路径） ————
DEFAULT_DIR="$(pwd)"
echo ""
echo "请输入安装目录（默认：${DEFAULT_DIR}）："
read -r INPUT_DIR
if [ -z "$INPUT_DIR" ]; then
    INSTALL_DIR="${DEFAULT_DIR}"
else
    INSTALL_DIR="${INPUT_DIR}"
fi

# 如果目录不存在，则尝试创建
if [ ! -d "$INSTALL_DIR" ]; then
    echo "目录 ${INSTALL_DIR} 不存在，是否创建？[Y/n]:"
    read -r CREATE_CONFIRM
    CREATE_CONFIRM="${CREATE_CONFIRM:-y}"
    if [[ "$CREATE_CONFIRM" =~ ^([Yy][Ee][Ss]|[Yy])$ ]]; then
        mkdir -p "$INSTALL_DIR"
        echo "已创建目录：${INSTALL_DIR}"
    else
        echo "未创建目录，安装退出。"
        exit 1
    fi
fi

## ———— 2. 检测 Termux 环境 ————
IS_TERMUX=false
if [ -n "$PREFIX" ] && [[ "$PREFIX" == *"com.termux"* ]]; then
    IS_TERMUX=true
fi

## ———— 3. 通过 GitHub API 获取最新 Release 的 tag_name ————
echo ""
echo "正在从 GitHub API 获取最新版本信息..."
GITHUB_API_URL="https://api.github.com/repos/zhongbai2333/Tomato-Novel-Downloader/releases/latest"

if command -v curl >/dev/null 2>&1; then
    TAG_NAME=$(curl -s "${GITHUB_API_URL}" \
               | grep -m1 '"tag_name":' \
               | sed -E 's/.*"([^"]+)".*/\1/')
elif command -v wget >/dev/null 2>&1; then
    TAG_NAME=$(wget -qO- "${GITHUB_API_URL}" \
               | grep -m1 '"tag_name":' \
               | sed -E 's/.*"([^"]+)".*/\1/')
else
    echo "错误：系统中未检测到 curl 或 wget，请先安装其中之一。"
    exit 1
fi

if [ -z "$TAG_NAME" ]; then
    echo "错误：无法从 GitHub API 获取 tag_name，请检查网络或仓库是否存在。"
    exit 1
fi

VERSION="${TAG_NAME#v}"
echo "最新版本：${TAG_NAME}（VERSION=${VERSION}）"

## ———— 4. 提示用户是否使用 moeyy 代理下载 ————
echo ""
echo "是否在中国大陆使用 moeyy 代理下载？"
echo -n "请输入 [Y/n]（默认 n，不使用代理）："
read -r USE_PROXY_INPUT
USE_PROXY_INPUT="${USE_PROXY_INPUT:-n}"
USE_PROXY=false
if [[ "$USE_PROXY_INPUT" =~ ^([Yy][Ee][Ss]|[Yy])$ ]]; then
    USE_PROXY=true
fi

if $USE_PROXY; then
    echo "已选择：使用 moeyy 代理下载。"
else
    echo "已选择：不使用代理，直接访问 GitHub 下载。"
fi

## ———— 5. 检测系统类型与架构 & 确定对应的二进制文件名 ————
PLATFORM="$(uname)"
ARCH="$(uname -m)"
BINARY_NAME=""

case "$PLATFORM" in
    "Linux")
        # 如果是 Termux 环境，仍按 Linux_arm64 来下载
        if $IS_TERMUX; then
            BINARY_NAME="TomatoNovelDownloader-Linux_arm64-v${VERSION}"
        else
            # 普通 Linux，分别处理 x86_64 / aarch64
            if [[ "$ARCH" == "x86_64" || "$ARCH" == "amd64" ]]; then
                BINARY_NAME="TomatoNovelDownloader-Linux_amd64-v${VERSION}"
            elif [[ "$ARCH" == "aarch64" || "$ARCH" == "arm64" ]]; then
                BINARY_NAME="TomatoNovelDownloader-Linux_arm64-v${VERSION}"
            else
                echo "错误：不支持的 Linux 架构 [${ARCH}]！仅支持 x86_64/amd64 及 aarch64/arm64。"
                exit 1
            fi
        fi
        ;;
    "Darwin")
        # macOS 系统
        # 区分 Apple Silicon (arm64) 和 Intel (x86_64)
        if [[ "$ARCH" == "arm64" ]]; then
            BINARY_NAME="TomatoNovelDownloader-macOS_arm64-v${VERSION}"
        else
            echo "错误：不支持的 macOS 架构 [${ARCH}]！仅支持 arm64。"
            exit 1
        fi
        ;;
    *)
        echo "错误：不支持的平台 [${PLATFORM}]！仅支持 Linux、macOS（Darwin）以及 Termux。"
        exit 1
        ;;
esac

## ———— 6. 拼接下载 URL（根据是否使用代理） ————
ORIGINAL_URL="https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases/download/${TAG_NAME}/${BINARY_NAME}"
if $USE_PROXY; then
    # 把原始 URL 放到 moeyy 代理前缀后面
    DOWNLOAD_URL="https://github.moeyy.xyz/${ORIGINAL_URL}"
else
    DOWNLOAD_URL="$ORIGINAL_URL"
fi

echo ""
echo "准备下载：${BINARY_NAME}"
echo "下载链接：${DOWNLOAD_URL}"
echo "安装目标目录：${INSTALL_DIR}"

## ———— 7. 下载二进制文件到安装目录 ————
TARGET_BINARY_PATH="${INSTALL_DIR}/${BINARY_NAME}"
if [ -f "$TARGET_BINARY_PATH" ]; then
    echo "注意：目标目录已有同名文件 ${TARGET_BINARY_PATH}，将会覆盖。"
    rm -f "$TARGET_BINARY_PATH"
fi

if command -v wget >/dev/null 2>&1; then
    # 推荐加上 -4 强制使用 IPv4，以避免 IPv6 路由不通导致卡顿
    wget -4 -q --show-progress -O "${TARGET_BINARY_PATH}" "${DOWNLOAD_URL}"
elif command -v curl >/dev/null 2>&1; then
    # 同样加上 -4 强制 IPv4，如果 SSL 证书有问题，可以临时加 --no-check-certificate
    curl -4 -L -o "${TARGET_BINARY_PATH}" "${DOWNLOAD_URL}"
else
    echo "错误：系统中未检测到 wget 或 curl，请先安装其中之一。"
    exit 1
fi

if [ ! -f "$TARGET_BINARY_PATH" ]; then
    echo "错误：下载失败，请检查网络、代理或 URL 是否正确。"
    exit 1
fi

chmod +x "$TARGET_BINARY_PATH"
echo "下载并赋予执行权限：${TARGET_BINARY_PATH}"

## ———— 8. 根据平台完成后续操作 —— 
if $IS_TERMUX; then
    # Termux 专门逻辑：安装 glibc-repo、glibc-runner 并生成 run.sh
    echo ""
    echo "检测到 Termux 环境，开始安装 glibc-repo 和 glibc-runner..."
    pkg update -y
    pkg install glibc-repo -y
    pkg install glibc-runner -y

    echo ""
    echo "在安装目录生成 run.sh（用于 Termux 下通过 glibc-runner 启动）..."
    RUN_SH_PATH="${INSTALL_DIR}/run.sh"
    cat > "$RUN_SH_PATH" <<EOF
#!/usr/bin/env bash
# Termux 环境下使用 glibc-runner 运行 TomatoNovelDownloader
SCRIPT_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
exec glibc-runner "\${SCRIPT_DIR}/${BINARY_NAME}"
EOF

    chmod +x "$RUN_SH_PATH"
    echo "已生成：${RUN_SH_PATH}"
    echo ""
    echo "安装完成。"
    echo "请在安装目录执行："
    echo "    cd ${INSTALL_DIR}"
    echo "    ./run.sh"
    echo "来启动程序。"

elif [[ "$PLATFORM" == "Linux" ]]; then
    # 普通 Linux，直接提示如何运行
    echo ""
    echo "检测到普通 Linux 环境。"
    echo "安装完成，二进制已保存到："
    echo "    ${TARGET_BINARY_PATH}"
    echo ""
    echo "只需直接使用以下命令启动："
    echo ""
    echo "    cd ${INSTALL_DIR}"
    echo "    ./$(printf "%q" "${BINARY_NAME}")"
elif [[ "$PLATFORM" == "Darwin" ]]; then
    # macOS 下，直接提示如何运行
    echo ""
    echo "检测到 macOS 环境。"
    echo "安装完成，二进制已保存到："
    echo "    ${TARGET_BINARY_PATH}"
    echo ""
    echo "只需直接使用以下命令启动："
    echo ""
    echo "    cd ${INSTALL_DIR}"
    echo "    ./$(printf "%q" "${BINARY_NAME}")"
fi

exit 0
