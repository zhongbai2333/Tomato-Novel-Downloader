# Tomato-Novel-Downloader-New（Rust）

这是番茄小说下载器的 Rust 重构版（MVP）。它不再复用旧的 Python 逻辑

目前正在测试

当前支持：搜索、按 book_id 拉取目录与正文、导出 txt/epub。

## 运行前置条件（重要）

已改为编译期静态链接：`Tomato-Novel-Official-API` 通过 Cargo 依赖直接复用 `Tomato-Novel-Network-Core`，不再需要运行时放置/配置动态库。

## 构建

```bash
cargo build --release
```

## 用法

查看帮助：

```bash
cargo run -- --help
```

搜索：

```bash
cargo run -- search "三体"
```

下载（txt）：

```bash
cargo run -- download --book-id 7143038691944959011 --out-dir ./out --format txt
```

下载（epub）：

```bash
cargo run -- download --book-id 7143038691944959011 --out-dir ./out --format epub
```

调试：按 chapter_ids 拉取正文（输出 JSON）：

```bash
cargo run -- fetch-contents --chapter-ids 123 456 789
```

## 说明

- 目录接口来自 `fanqienovel.com/api/reader/directory/detail`（在 core 侧实现为 `book_directory_detail` operation）。
- 正文拉取每次最多 25 个章节，会自动分块请求，并在遇到 cooldown 时进行简单退避重试。
