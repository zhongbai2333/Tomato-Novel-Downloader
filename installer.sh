#!/usr/bin/env bash
#
# 文件名：installer.sh
# 功能：
#   1. 自动通过 GitHub API 获取 Tomato-Novel-Downloader 最新版本
#   2. 询问用户安装路径（默认脚本执行路径）
#   3. 支持 2 种下载方式：
#        (1) 直连 GitHub
#        (2) 笒鬼鬼 API（https://api.cenguigui.cn/api/github）解析加速
#   4. 可在用户选择使用笒鬼鬼 API 且未安装 jq 时，交互式尝试安装 jq（支持常见包管理器），失败继续使用纯 Bash 兜底解析
#   5. Termux 环境下自动安装 glibc 运行依赖并生成 run.sh
#   6. Linux / macOS (arm64 & Intel x86_64) 下下载对应架构二进制并赋予执行权限
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

# 纯 Bash URL 编码（保留 / : - _ . ~ 方便阅读；其余编码）
urlencode() {
    local raw="${1:?}"
    local out="" c
    local i len=${#raw}
    for (( i=0; i<len; i++ )); do
        c="${raw:i:1}"
        case "$c" in
            [a-zA-Z0-9._~/:=-]) out+="$c" ;;
            *) printf -v hex '%%%02X' "'$c"; out+="$hex" ;;
        esac
    done
    printf '%s' "$out"
}

# JSON 字段提取（优先 jq，失败则使用简单 grep+sed 兜底）
json_get_field() {
    local json="$1" field="$2"
    if command_exists jq; then
        printf "%s" "$json" | jq -r --arg f "$field" '.[$f] // .data[$f] // empty' 2>/dev/null || true
        return
    fi
    printf "%s" "$json" \
      | grep -o "\"$field\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" \
      | head -n1 \
      | sed -E 's/.*:"([^"]*)".*/\1/'
}

# 询问并尝试安装 jq（仅在用户选择 cenguigui 且缺 jq 时调用）
maybe_install_jq() {
    if command_exists jq; then
        return 0
    fi
    echo ""
    log_warn "未检测到 jq，将使用 sed/grep 兜底解析（可能不够稳健）。"
    echo "是否尝试自动安装 jq？[Y/n]（macOS 无 Homebrew 会自动放弃安装）"
    read -r REPLY_JQ
    REPLY_JQ="${REPLY_JQ:-y}"
    if [[ ! "$REPLY_JQ" =~ ^([Yy][Ee][Ss]|[Yy])$ ]]; then
        log_info "跳过 jq 安装。"
        return 0
    fi

    # 检测各平台包管理器
    local install_cmd="" need_root=true
    if command_exists pkg && $IS_TERMUX; then
        install_cmd="pkg update -y && pkg install -y jq"
        need_root=false
    elif command_exists apt-get; then
        install_cmd="apt-get update -y && apt-get install -y jq"
    elif command_exists apt; then
        install_cmd="apt update -y && apt install -y jq"
    elif command_exists dnf; then
        install_cmd="dnf install -y jq"
    elif command_exists yum; then
        install_cmd="yum install -y jq"
    elif command_exists pacman; then
        install_cmd="pacman -Sy --noconfirm jq"
    elif command_exists apk; then
        install_cmd="apk add --no-cache jq"
    elif command_exists brew; then
        install_cmd="brew install jq"
        need_root=false
    else
        log_warn "未识别到可用包管理器，无法自动安装 jq。继续使用兜底解析。"
        return 0
    fi

    # 判断是否需要 sudo
    if $need_root && [ "$EUID" -ne 0 ]; then
        if command_exists sudo; then
            install_cmd="sudo sh -c '$install_cmd'"
        else
            log_warn "需要 root 权限安装 jq，但未检测到 sudo。请手动安装后重运行。"
            return 0
        fi
    fi

    log_info "尝试自动安装 jq ..."
    if bash -c "$install_cmd"; then
        if command_exists jq; then
            log_info "jq 安装成功。"
        else
            log_warn "执行安装命令后仍未检测到 jq。"
        fi
    else
        log_warn "jq 安装命令执行失败，继续使用兜底解析。"
    fi
}

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
echo "  2) 使用 笒鬼鬼 API (api.cenguigui.cn) 自动解析 downUrl"
read -r ACCEL_CHOICE
ACCEL_CHOICE="${ACCEL_CHOICE:-1}"

case "$ACCEL_CHOICE" in
    1) ACCEL_METHOD="direct" ;;
    2) ACCEL_METHOD="cenguigui" ;;
    *) log_warn "无效输入，使用默认直连。"; ACCEL_METHOD="direct" ;;
esac

log_info "选择的下载方式：${ACCEL_METHOD}"

# 如果用户选择 cenguigui 且 jq 缺失，尝试安装（可跳过）
if [ "$ACCEL_METHOD" = "cenguigui" ] && ! command_exists jq; then
    maybe_install_jq
fi

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
            elif [[ "$ARCH" == "armv7l" || "$ARCH" == "armv7" || "$ARCH" == "armhf" ]]; then
                BINARY_NAME="TomatoNovelDownloader-Linux_armv7l-v${VERSION}"
            else
                log_error "不支持的 Linux 架构 [${ARCH}]！仅支持 x86_64/amd64, aarch64/arm64, armv7l/armv7/armhf。"
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

resolve_cenguigui_url() {
    local orig="$1"
    # URL 编码（虽然当前 orig 不含空格，但为稳健保留）
    local enc
    enc="$(urlencode "$orig")"
    local api="https://api.cenguigui.cn/api/github/?type=json&url=${enc}"
    log_info "通过 笒鬼鬼 API 解析：$api"
    local json=""
    if command_exists curl; then
        json=$(curl -s --connect-timeout 10 "$api" || true)
    else
        json=$(wget -qO- "$api" || true)
    fi
    if [ -z "$json" ]; then
        log_warn "笒鬼鬼 API 无响应。"
        return 1
    fi
    local code
    code=$(json_get_field "$json" "code")
    if [ "$code" != "200" ]; then
        log_warn "笒鬼鬼 API 返回异常 code=${code:-空}"
        return 1
    fi
    local downUrl
    downUrl=$(json_get_field "$json" "downUrl")
    downUrl="${downUrl//\\//}"  # 去除可能的转义反斜杠
    if [ -z "$downUrl" ]; then
        log_warn "未获取到 downUrl 字段。"
        return 1
    fi
    printf "%s" "$downUrl"
}

case "$ACCEL_METHOD" in
    direct)
        log_info "使用直连：$ORIGINAL_URL"
        ;;
    cenguigui)
        if RESOLVED_URL=$(resolve_cenguigui_url "$ORIGINAL_URL"); then
            DOWNLOAD_URL="$RESOLVED_URL"
            log_info "笒鬼鬼 API 解析成功：$DOWNLOAD_URL"
        else
            log_warn "笒鬼鬼 API 解析失败，回退直连。"
            DOWNLOAD_URL="$ORIGINAL_URL"
        fi
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