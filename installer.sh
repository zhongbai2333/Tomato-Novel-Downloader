#!/usr/bin/env bash
# 
# 文件名：installer.sh
# 功能：
#   1. 自动通过 GitHub API 获取 Tomato-Novel-Downloader 最新版本
#   2. 询问用户安装路径（默认脚本执行路径；Termux 下默认 $HOME）
#   3. 支持 2 种下载方式：
#        (1) 直连 GitHub
#        (2) 项目加速源（https://dl.zhongbai233.com/）加速
#   4. Termux 环境下生成 run.sh（默认 --server）
#   5. Linux / macOS (arm64 & Intel x86_64) 下下载对应架构二进制并赋予执行权限
# 
# 使用方法：

#   chmod +x installer.sh
#   ./installer.sh

set -e

#####################################
# 0. 通用辅助函数
#####################################

log_info()  { printf "\033[1;32m[INFO]\033[0m %s\n" "$*"; }
log_warn()  { printf "\033[1;33m[WARN]\033[0m %s\n" "$*"; }
log_error() { printf "\033[1;31m[ERR ]\033[0m %s\n" "$*" >&2; }

command_exists() { command -v "$1" >/dev/null 2>&1; }

IS_TERMUX=false
if [ -n "${PREFIX:-}" ]; then
    case "${PREFIX}" in
        *com.termux*|*bin.mt.plus*|*com.duoduo.mt*)
            IS_TERMUX=true
            ;;
    esac
fi

IS_MUSL=false
if command_exists ldd; then
    if ldd --version 2>&1 | grep -qi musl; then
        IS_MUSL=true
    fi
fi
# Fallback: common musl loader paths
if [ -e /lib/ld-musl-x86_64.so.1 ] || [ -e /lib/ld-musl-aarch64.so.1 ] || [ -e /lib/ld-musl-armhf.so.1 ]; then
    IS_MUSL=true
fi

DEFAULT_DIR="$(pwd)"
if $IS_TERMUX; then
    DEFAULT_DIR="${HOME}"
fi

echo ""
echo "请输入安装目录（默认：${DEFAULT_DIR}）："
read -r INPUT_DIR
if [ -z "${INPUT_DIR}" ]; then
    INSTALL_DIR="${DEFAULT_DIR}"
else
    INSTALL_DIR="${INPUT_DIR}"
fi

if $IS_TERMUX; then
    case "$INSTALL_DIR" in
        "$HOME"*|"$PREFIX"*)
            ;;
        *)
            echo ""
            log_warn "检测到 Termux：你选择的安装目录可能无法执行（可能出现 Permission denied）。"
            log_warn "建议安装到 Termux 目录内（HOME 或 PREFIX）："
            echo "  - ${HOME}"
            echo "  - ${PREFIX}"
            echo ""
            echo "是否仍然继续使用该目录？(y/N)："
            read -r CONFIRM_DIR
            case "$CONFIRM_DIR" in
                y|Y) ;;
                *)
                    INSTALL_DIR="${HOME}"
                    log_info "已改为安装到：${INSTALL_DIR}"
                    ;;
            esac
            ;;
    esac
fi

if [ ! -d "$INSTALL_DIR" ]; then
    echo ""
    log_warn "目录不存在，是否创建：${INSTALL_DIR} ? (y/N)："
    read -r CREATE_DIR
    case "$CREATE_DIR" in
        y|Y)
            mkdir -p "$INSTALL_DIR"
            log_info "已创建目录：${INSTALL_DIR}"
            ;;
        *)
            log_warn "未创建目录，安装退出。"
            exit 1
            ;;
    esac
fi

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

if [ -z "${TAG_NAME}" ]; then
    log_error "无法从 GitHub API 获取 tag_name，请检查网络或仓库是否存在。"
    exit 1
fi

VERSION="${TAG_NAME#v}"
log_info "最新版本：${TAG_NAME}（VERSION=${VERSION}）"

echo ""
echo "请选择下载方式（输入序号，默认 1）："
echo "  1) 直连 GitHub"
echo "  2) 使用项目加速源 (https://dl.zhongbai233.com/) 加速"
read -r ACCEL_CHOICE
ACCEL_CHOICE="${ACCEL_CHOICE:-1}"
case "$ACCEL_CHOICE" in
    1) ACCEL_METHOD="direct" ;;
    2) ACCEL_METHOD="accel" ;;
    *) log_warn "无效输入，使用默认直连。"; ACCEL_METHOD="direct" ;;
esac
log_info "选择的下载方式：${ACCEL_METHOD}"

PLATFORM="$(uname)"
ARCH="$(uname -m)"
BINARY_NAME=""
case "$PLATFORM" in
    Linux)
        if $IS_TERMUX; then
            # 检测 Termux 架构：aarch64 → arm64, armv7l → arm32
            case "$ARCH" in
                aarch64|arm64)
                    ANDROID_ARCH="arm64"
                    ;;
                armv7l|arm)
                    ANDROID_ARCH="arm32"
                    ;;
                *)
                    log_error "不支持的 Android 架构 [${ARCH}]！仅支持 aarch64/arm64 与 armv7l/arm。"
                    exit 1
                    ;;
            esac
            BINARY_NAME="TomatoNovelDownloader-Android_${ANDROID_ARCH}-v${VERSION}"
            log_info "检测到 Termux（架构：${ANDROID_ARCH}），将使用 Android 原生版本。"
        else
            case "$ARCH" in
                x86_64|amd64)
                    if $IS_MUSL; then
                        BINARY_NAME="TomatoNovelDownloader-Linux_musl_amd64-v${VERSION}"
                    else
                        BINARY_NAME="TomatoNovelDownloader-Linux_amd64-v${VERSION}"
                    fi
                    ;;
                aarch64|arm64)
                    if $IS_MUSL; then
                        BINARY_NAME="TomatoNovelDownloader-Linux_musl_arm64-v${VERSION}"
                    else
                        BINARY_NAME="TomatoNovelDownloader-Linux_arm64-v${VERSION}"
                    fi
                    ;;
                *)
                    log_error "不支持的 Linux 架构 [${ARCH}]！仅支持 x86_64/amd64 与 aarch64/arm64。"
                    exit 1
                    ;;
            esac
        fi
        ;;
    Darwin)
        case "$ARCH" in
            arm64)
                BINARY_NAME="TomatoNovelDownloader-macOS_arm64-v${VERSION}"
                ;;
            x86_64)
                BINARY_NAME="TomatoNovelDownloader-macOS_amd64-v${VERSION}"
                ;;
            *)
                log_error "不支持的 macOS 架构 [${ARCH}]！仅支持 arm64 与 x86_64。"
                exit 1
                ;;
        esac
        ;;
    *)
        log_error "不支持的平台 [${PLATFORM}]！仅支持 Linux、macOS（Darwin）以及 Termux。"
        exit 1
        ;;
esac

ORIGINAL_URL="https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases/download/${TAG_NAME}/${BINARY_NAME}"
DOWNLOAD_URL="$ORIGINAL_URL"
case "$ACCEL_METHOD" in
    direct) ;;
    accel) DOWNLOAD_URL="https://dl.zhongbai233.com/release/${TAG_NAME}/${BINARY_NAME}" ;;
esac

echo ""
log_info "准备下载：${BINARY_NAME}"
echo "下载链接：${DOWNLOAD_URL}"
echo "安装目标目录：${INSTALL_DIR}"

TARGET_BINARY_PATH="${INSTALL_DIR}/${BINARY_NAME}"
if [ -f "$TARGET_BINARY_PATH" ]; then
    log_warn "目标目录已有同名文件，将会覆盖：${TARGET_BINARY_PATH}"
    rm -f "$TARGET_BINARY_PATH"
fi

download_file() {
    if command_exists wget; then
        wget -4 -q --show-progress -O "${TARGET_BINARY_PATH}" "${DOWNLOAD_URL}"
    elif command_exists curl; then
        curl -4 -L -o "${TARGET_BINARY_PATH}" "${DOWNLOAD_URL}"
    else
        log_error "未检测到 wget 或 curl，请先安装其中之一。"
        return 127
    fi
}

log_info "开始下载..."
download_file || {
    log_error "下载失败，请检查网络、代理或 URL。"
    exit 1
}

if [ ! -f "$TARGET_BINARY_PATH" ] || [ ! -s "$TARGET_BINARY_PATH" ]; then
    log_error "下载的文件不存在或为空。"
    exit 1
fi

chmod +x "$TARGET_BINARY_PATH"
log_info "下载完成并赋予可执行权限：${TARGET_BINARY_PATH}"

# 重命名为规范名（不含版本号），与程序自更新后的命名保持一致
CANONICAL_NAME="${BINARY_NAME%-v*}"
if [ "$CANONICAL_NAME" != "$BINARY_NAME" ]; then
    CANONICAL_PATH="${INSTALL_DIR}/${CANONICAL_NAME}"
    mv "${TARGET_BINARY_PATH}" "${CANONICAL_PATH}"
    chmod +x "${CANONICAL_PATH}"
    TARGET_BINARY_PATH="${CANONICAL_PATH}"
    log_info "已重命名为规范名：${CANONICAL_NAME}"
fi

if $IS_TERMUX; then
    echo ""
    log_info "生成 run.sh..."
    RUN_SH_PATH="${INSTALL_DIR}/run.sh"
    cat > "$RUN_SH_PATH" <<EOF
#!/usr/bin/env bash
# Termux / MT 管理器环境：运行 Android 原生 TomatoNovelDownloader（默认启动 Web UI 服务器模式）
# 你可以用环境变量控制监听地址与密码锁：
#   TOMATO_WEB_ADDR=0.0.0.0:18423
#   TOMATO_WEB_PASSWORD=你的密码
SCRIPT_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
termux-open-url "http://127.0.0.1:18423/" >/dev/null 2>&1 || true
exec "\${SCRIPT_DIR}/${CANONICAL_NAME}" --server "\$@"
EOF
    chmod +x "$RUN_SH_PATH"
    log_info "已生成：${RUN_SH_PATH}"

    echo ""
    echo "安装完成，请执行："
    echo "    cd ${INSTALL_DIR}"
    echo "    ./run.sh"
    echo ""
    echo "提示：如果运行时出现 Permission denied，请把安装目录放在 Termux 目录内（建议 ${HOME}）。"
elif [ "$PLATFORM" = "Linux" ]; then
    echo ""
    log_info "检测到 Linux 环境。"
    echo "安装完成，文件位于：${TARGET_BINARY_PATH}"
    echo "运行方式："
    echo "    cd ${INSTALL_DIR}"
    echo "    ./${CANONICAL_NAME}"
elif [ "$PLATFORM" = "Darwin" ]; then
    echo ""
    log_info "检测到 macOS 环境。"
    echo "安装完成，文件位于：${TARGET_BINARY_PATH}"
    echo "运行方式："
    echo "    cd ${INSTALL_DIR}"
    echo "    ./${CANONICAL_NAME}"
fi

log_info "全部完成。"
exit 0
