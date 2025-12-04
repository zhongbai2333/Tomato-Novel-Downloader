# 使用官方的 Ubuntu 镜像作为基础镜像
FROM ubuntu:20.04

# 设置环境变量，避免一些不必要的交互
ENV DEBIAN_FRONTEND=noninteractive

# 更新并安装必要的工具和库
RUN apt-get update && apt-get install -y \
    python3-pip \
    python3-dev \
    curl \
    wget \
    unzip \
    libssl-dev \
    libffi-dev \
    build-essential \
    && apt-get clean

# 安装 Python 依赖包
RUN pip3 install --upgrade pip setuptools wheel
RUN pip3 install \
    ascii-magic \
    beautifulsoup4 \
    certifi \
    charset-normalizer \
    colorama \
    EbookLib \
    fake-useragent \
    idna \
    lxml \
    pillow \
    pycryptodome \
    PyYAML \
    requests \
    six \
    soupsieve \
    tqdm \
    typing_extensions \
    urllib3 \
    portalocker \
    urwid \
    pyperclip \
    pillow_heif \
    edge-tts

# 下载并解压指定的二进制文件
RUN wget -q https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases/download/v1.8.5/TomatoNovelDownloader-Linux_amd64-v1.8.5 -O /usr/local/bin/TomatoNovelDownloader
RUN chmod +x /usr/local/bin/TomatoNovelDownloader

# 设置容器的工作目录
WORKDIR /usr/local/bin

# 设置容器启动时默认执行的命令
ENTRYPOINT ["./TomatoNovelDownloader"]

# 设置容器启动时的默认参数（可以根据需要修改）
CMD ["--help"]
