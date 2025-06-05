#!/usr/bin/env bash
#
# 文件名：install_and_run.sh
# 功能：自动从 GitHub 获取 TomatoNovelDownloader 最新版本，
#       询问用户安装路径（默认脚本执行路径），
#       提示用户是否使用 moeyy 代理下载 Release 资产，
#       在 Termux 环境下安装 glibc-repo、glibc-runner 并生成 run.sh；
#       在普通 Linux（x86_64/ARM64）下只下载对应架构的二进制并赋予执行权限即可运行。
#
# 使用方法：
#   chmod +x install_and_run.sh
#   ./install_and_run.sh
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

## ———— 5. 检测系统架构 & 确定要下载的二进制文件名 ————
ARCH="$(uname -m)"
case "$ARCH" in
    aarch64|arm64)
        BINARY_NAME="TomatoNovelDownloader-Linux_arm64-v${VERSION}"
        ;;
    x86_64|amd64)
        BINARY_NAME="TomatoNovelDownloader-Linux_amd64-v${VERSION}"
        ;;
    *)
        echo "错误：不支持的架构 [${ARCH}]！仅支持 aarch64/arm64 和 x86_64/amd64。"
        exit 1
        ;;
esac

## ———— 6. 拼接下载 URL（根据是否使用代理） ————
ORIGINAL_URL="https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases/download/${TAG_NAME}/${BINARY_NAME}"
if $USE_PROXY; then
    DOWNLOAD_URL="https://github.moeyy.xyz/${ORIGINAL_URL}"
else
    DOWNLOAD_URL="${ORIGINAL_URL}"
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
    wget -q --show-progress -O "${TARGET_BINARY_PATH}" "${DOWNLOAD_URL}"
elif command -v curl >/dev/null 2>&1; then
    curl -L -# -o "${TARGET_BINARY_PATH}" "${DOWNLOAD_URL}"
fi

if [ ! -f "$TARGET_BINARY_PATH" ]; then
    echo "错误：下载失败，请检查网络或 URL 是否正确。"
    exit 1
fi

chmod +x "$TARGET_BINARY_PATH"
echo "下载并赋予执行权限：${TARGET_BINARY_PATH}"

## ———— 8. 如果是 Termux 环境，安装 glibc-repo 和 glibc-runner，并在安装目录生成 run.sh ————
if $IS_TERMUX; then
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

else
    ## ———— 9. 普通 Linux 环境（x86_64/ARM64）仅提示如何运行，不生成 run.sh ————
    echo ""
    echo "检测到普通 Linux 环境（非 Termux）。"
    echo "安装完成，二进制已保存到："
    echo "    ${TARGET_BINARY_PATH}"
    echo ""
    echo "只需直接使用以下命令启动："
    echo ""
    echo "    cd ${INSTALL_DIR}"
    echo "    ./$(printf "%q" "${BINARY_NAME}")"
fi

exit 0
