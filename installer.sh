#!/usr/bin/env bash
#
# 文件名：installer.sh
# 功能：
#   1. 自动通过 GitHub API 获取 Tomato-Novel-Downloader 最新版本
#   2. 询问用户安装路径（默认脚本执行路径）
#   3. 支持 2 种下载方式：
#        (1) 直连 GitHub
#        (2) gh-proxy（https://gh-proxy.org/ + GitHub 下载链接）加速
#   4. Termux 环境下自动安装 glibc 运行依赖并生成 run.sh
#   5. Linux / macOS (arm64 & Intel x86_64) 下下载对应架构二进制并赋予执行权限
#
# 使用方法：
#   chmod +x installer.sh
#   ./installer.sh
#
set -e

#####################################
# 0. 通用辅助函数
#####################################

log_info()  { printf "\033[1;32m[INFO]\033[0m %s\n" "$*"; }
log_warn()  { printf "\033[1;33m[WARN]\033[0m %s\n" "$*"; }
log_error() { printf "\033[1;31m[ERR ]\033[0m %s\n" "$*" >&2; }

command_exists() { command -v "$1" >/dev/null 2>&1; }

#####################################
# 1. 询问安装目录
#####################################
DEFAULT_DIR="$(pwd)"
echo ""
echo "请输入安装目录（默认：${DEFAULT_DIR}）："
read -r INPUT_DIR
if [ -z "$INPUT_DIR" ]; then
    INSTALL_DIR="${DEFAULT_DIR}"
else
    INSTALL_DIR="${INPUT_DIR}"
fi

if [ ! -d "$INSTALL_DIR" ]; then
    echo "目录 ${INSTALL_DIR} 不存在，是否创建？[Y/n]:"
    read -r CREATE_CONFIRM
    CREATE_CONFIRM="${CREATE_CONFIRM:-y}"
    if [[ "$CREATE_CONFIRM" =~ ^([Yy][Ee][Ss]|[Yy])$ ]]; then
        mkdir -p "$INSTALL_DIR"
        log_info "已创建目录：${INSTALL_DIR}"
    else
        log_warn "未创建目录，安装退出。"
        exit 1
    fi
fi

#####################################
# 2. 检测 Termux 环境
#####################################
IS_TERMUX=false
if [ -n "$PREFIX" ] && [[ "$PREFIX" == *"com.termux"* ]]; then
    IS_TERMUX=true
fi

#####################################
# 3. 获取最新 Release tag_name
#####################################
echo ""
log_info "正在从 GitHub API 获取最新版本信息..."
GITHUB_API_URL="https://api.github.com/repos/zhongbai2333/Tomato-Novel-Downloader/releases/latest"

if command_exists curl; then
    TAG_NAME=$(curl -s "${GITHUB_API_URL}" | grep -m1 '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
elif command_exists wget; then
    TAG_NAME=$(wget -qO- "${GITHUB_API_URL}" | grep -m1 '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
else
    log_error "系统中未检测到 curl 或 wget，请先安装其中之一。"
    exit 1
fi

if [ -z "$TAG_NAME" ]; then
    log_error "无法从 GitHub API 获取 tag_name，请检查网络或仓库是否存在。"
    exit 1
fi

VERSION="${TAG_NAME#v}"
log_info "最新版本：${TAG_NAME}（VERSION=${VERSION}）"

#####################################
# 4. 选择下载加速方式
#####################################
echo ""
echo "请选择下载方式（输入序号，默认 1）："
echo "  1) 直连 GitHub"
echo "  2) 使用 gh-proxy (https://gh-proxy.org/) 加速"
read -r ACCEL_CHOICE
ACCEL_CHOICE="${ACCEL_CHOICE:-1}"

case "$ACCEL_CHOICE" in
    1) ACCEL_METHOD="direct" ;;
    2) ACCEL_METHOD="gh-proxy" ;;
    *) log_warn "无效输入，使用默认直连。"; ACCEL_METHOD="direct" ;;
esac

log_info "选择的下载方式：${ACCEL_METHOD}"

#####################################
# 5. 检测系统与架构
#####################################
PLATFORM="$(uname)"
ARCH="$(uname -m)"
BINARY_NAME=""

case "$PLATFORM" in
    Linux)
        if $IS_TERMUX; then
            BINARY_NAME="TomatoNovelDownloader-Linux_arm64-v${VERSION}"
        else
            if [[ "$ARCH" == "x86_64" || "$ARCH" == "amd64" ]]; then
                BINARY_NAME="TomatoNovelDownloader-Linux_amd64-v${VERSION}"
            elif [[ "$ARCH" == "aarch64" || "$ARCH" == "arm64" ]]; then
                BINARY_NAME="TomatoNovelDownloader-Linux_arm64-v${VERSION}"
            else
                log_error "不支持的 Linux 架构 [${ARCH}]！仅支持 x86_64/amd64 与 aarch64/arm64。"
                exit 1
            fi
        fi
        ;;
    Darwin)
        if [[ "$ARCH" == "arm64" ]]; then
            BINARY_NAME="TomatoNovelDownloader-macOS_arm64-v${VERSION}"
        elif [[ "$ARCH" == "x86_64" || "$ARCH" == "amd64" ]]; then
            BINARY_NAME="TomatoNovelDownloader-macOS_amd64-v${VERSION}"
        else
            log_error "不支持的 macOS 架构 [${ARCH}]！仅支持 arm64 / x86_64。"
            exit 1
        fi
        ;;
    *)
        log_error "不支持的平台 [${PLATFORM}]！仅支持 Linux、macOS（Darwin）以及 Termux。"
        exit 1
        ;;
esac

#####################################
# 6. 生成原始下载 URL + 根据加速方式得到最终下载链接
#####################################
ORIGINAL_URL="https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases/download/${TAG_NAME}/${BINARY_NAME}"
DOWNLOAD_URL="$ORIGINAL_URL"

case "$ACCEL_METHOD" in
    direct)
        log_info "使用直连：$ORIGINAL_URL"
        ;;
    gh-proxy)
        DOWNLOAD_URL="https://gh-proxy.org/${ORIGINAL_URL}"
        log_info "使用 gh-proxy 加速：$DOWNLOAD_URL"
        ;;
esac

echo ""
log_info "准备下载：${BINARY_NAME}"
echo "下载链接：${DOWNLOAD_URL}"
echo "安装目标目录：${INSTALL_DIR}"

#####################################
# 7. 下载二进制
#####################################
TARGET_BINARY_PATH="${INSTALL_DIR}/${BINARY_NAME}"
if [ -f "$TARGET_BINARY_PATH" ]; then
    log_warn "目标目录已有同名文件，将会覆盖：${TARGET_BINARY_PATH}"
    rm -f "$TARGET_BINARY_PATH"
fi

downloader=""
if command_exists wget; then
    downloader="wget -4 -q --show-progress -O \"${TARGET_BINARY_PATH}\" \"${DOWNLOAD_URL}\""
elif command_exists curl; then
    downloader="curl -4 -L -o \"${TARGET_BINARY_PATH}\" \"${DOWNLOAD_URL}\""
else
    log_error "未检测到 wget 或 curl，请先安装其中之一。"
    exit 1
fi

log_info "开始下载..."
# shellcheck disable=SC2086
eval $downloader || {
    log_error "下载失败，请检查网络、代理或 URL。"
    exit 1
}

if [ ! -f "$TARGET_BINARY_PATH" ] || [ ! -s "$TARGET_BINARY_PATH" ]; then
    log_error "下载的文件不存在或为空。"
    exit 1
fi

chmod +x "$TARGET_BINARY_PATH"
log_info "下载完成并赋予可执行权限：${TARGET_BINARY_PATH}"

#####################################
# 8. 平台后续操作
#####################################
if $IS_TERMUX; then
    echo ""
    log_info "检测到 Termux 环境，安装 glibc-repo 与 glibc-runner..."
    pkg update -y
    pkg install -y glibc-repo
    pkg install -y glibc-runner

    echo ""
    log_info "生成 run.sh..."
    RUN_SH_PATH="${INSTALL_DIR}/run.sh"
    cat > "$RUN_SH_PATH" <<EOF
#!/usr/bin/env bash
# Termux 环境下使用 glibc-runner 运行 TomatoNovelDownloader
SCRIPT_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
exec glibc-runner "\${SCRIPT_DIR}/${BINARY_NAME}"
EOF
    chmod +x "$RUN_SH_PATH"
    log_info "已生成：${RUN_SH_PATH}"

    echo ""
    echo "安装完成，请执行："
    echo "    cd ${INSTALL_DIR}"
    echo "    ./run.sh"
elif [[ "$PLATFORM" == "Linux" ]]; then
    echo ""
    log_info "检测到 Linux 环境。"
    echo "安装完成，文件位于：${TARGET_BINARY_PATH}"
    echo "运行方式："
    echo "    cd ${INSTALL_DIR}"
    echo "    ./$(printf "%q" "${BINARY_NAME}")"
elif [[ "$PLATFORM" == "Darwin" ]]; then
    echo ""
    log_info "检测到 macOS 环境。"
    echo "安装完成，文件位于：${TARGET_BINARY_PATH}"
    echo "运行方式："
    echo "    cd ${INSTALL_DIR}"
    echo "    ./$(printf "%q" "${BINARY_NAME}")"
fi

log_info "全部完成。"
exit 0