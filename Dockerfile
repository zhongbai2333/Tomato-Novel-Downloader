# 使用官方的 Ubuntu 镜像作为基础镜像
FROM ubuntu:latest

# 设置环境变量，避免一些不必要的交互
ENV DEBIAN_FRONTEND=noninteractive
# 建议固定Python包索引源，以加速下载和提高稳定性
ENV PIP_INDEX_URL=https://pypi.org/simple

# 1. 更新并安装所有必要的系统工具、库和 Python 环境
RUN apt-get update && apt-get install -y \
    python3 \
    python3-pip \
    python3-dev \
    curl \
    wget \
    unzip \
    libssl-dev \
    libffi-dev \
    build-essential \
    pkg-config \
    # 以下是一些常见 Python 包（如 Pillow, cryptography）可能需要的系统库
    libjpeg-dev zlib1g-dev libfreetype6-dev \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# 2. 首先仅升级 pip 自身，使用国内镜像源并设置超时和重试
RUN python3 -m pip install --upgrade pip \
    --index-url https://pypi.tuna.tsinghua.edu.cn/simple \
    --default-timeout=100 \
    --retries 5

# 3. 然后安装 setuptools 和 wheel
RUN pip3 install --no-cache-dir setuptools wheel \
    --index-url https://pypi.tuna.tsinghua.edu.cn/simple \
    --default-timeout=100

# 4. 分批次安装 Python 依赖包，将基础包和可能耗时的包分开
RUN pip3 install --no-cache-dir \
    requests \
    beautifulsoup4 \
    lxml \
    tqdm \
    pycryptodome \
    PyYAML \
    pillow \
    --index-url https://pypi.tuna.tsinghua.edu.cn/simple \
    --default-timeout=100

RUN pip3 install --no-cache-dir \
    EbookLib \
    fake-useragent \
    colorama \
    portalocker \
    urwid \
    pyperclip \
    edge-tts \
    ascii-magic \
    pillow_heif \
    --index-url https://pypi.tuna.tsinghua.edu.cn/simple \
    --default-timeout=100

# 5. 下载指定的二进制文件
RUN wget -q https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases/download/v1.8.5/TomatoNovelDownloader-Linux_amd64-v1.8.5 -O /usr/local/bin/TomatoNovelDownloader \
    && chmod +x /usr/local/bin/TomatoNovelDownloader

# 6. 设置容器的工作目录和入口
WORKDIR /workspace
ENTRYPOINT ["/bin/bash"]
