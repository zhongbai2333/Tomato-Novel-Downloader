# 番茄小说下载器~~精简版~~

番茄小说下载器*不精简*版本，由于项目[fanqienovel-downloader](https://github.com/ying-ck/fanqienovel-downloader)一直不更新，于是我根据Dimily的项目Fork并重构

目前完全使用`Rust`重写了整个项目，与原Fork项目几乎没有关系了（~~虽说原本的Python版本也没几行是原项目的了~~）

我对其进行重构 + 优化，添加更多功能，包括：EPUB 下载支持、更好的断点续传、更好的错误管理、书本搜索、Web UI 等特性。

本项目支持两种构建模式：

- 默认模式（`official-api`）：保留 Official-API 能力（搜索/目录/段评等），同时也兼容第三方正文模式。
- No-Official-API 模式（`no-official-api`）：**不依赖 Official-API crate**；目录/书信息走网页解析；**正文强制使用第三方 API 地址池**。

为了保证第三方API安全，部分第三方接口相关代码并不开源，包括地址和token，敬请谅解，谢谢！

为方便视障人士使用，我保留了老的CLI界面，接下来是启用方法：

在第一次打开程序时 按三下 `o` 并回车 或者 按一下下方向键并按三下 `o` 都可以启用老版本CLI界面

注意：切换成功应该会发出 `灯` 的一声

---

## 我该如何使用？

根据自己的系统版本在[Releases](https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases)列表下载可执行文件，并运行
你可以通过输入书籍id以及需要保存的路径来进行下载

### 命令行模式（非交互）

如果你需要在自动化脚本中使用下载器（例如为 Kindle 自动更新番茄小说），可以使用命令行参数直接下载或更新书籍：

- 直接下载指定书籍：

    ```sh
    Tomato-Novel-Downloader.exe --download <book_id>
    ```

    示例：

    ```sh
    Tomato-Novel-Downloader.exe --download 7318247498772674083
    ```

- 更新指定书籍：

    ```sh
    Tomato-Novel-Downloader.exe --update <book_id>
    ```

    示例：

    ```sh
    Tomato-Novel-Downloader.exe --update 7318247498772674083
    ```

注意：

- 命令行模式为非交互模式，会直接开始下载/更新，无需手动输入
- 使用配置文件（`config.yml`）中的默认保存路径和下载设置
- `--download` 和 `--update` 都会智能检测已下载的章节，并提供继续下载或重新下载的选项
- 如果书籍已存在，两个命令的行为相同：检测新章节并更新
- 如果书籍不存在，两个命令都会开始全新下载
- 只接受 book_id，不支持搜索功能

### Web UI 服务器模式（--server）

如果你希望在局域网用浏览器操作（搜索、发起下载、查看任务、下载文件/打包下载文件夹），可以启用 Web UI：

- 启动 Web UI：

    ```sh
    Tomato-Novel-Downloader.exe --server
    ```

- 监听地址（默认 `127.0.0.1:18423`）：

    通过环境变量修改监听地址，例如局域网访问：

    ```sh
    TOMATO_WEB_ADDR=0.0.0.0:18423
    ```

    IPv6 监听示例（注意 IPv6 需要方括号）：

    ```sh
    TOMATO_WEB_ADDR=[::]:18423
    ```

    同时监听多个地址（用逗号或分号分隔），例如同时监听 IPv4 + IPv6：

    ```sh
    TOMATO_WEB_ADDR=0.0.0.0:18423,[::]:18423
    ```

- 密码锁模式（防止陌生人使用）：

    ```sh
    Tomato-Novel-Downloader.exe --server --password 你的密码
    ```

    或者使用环境变量：

    ```sh
    TOMATO_WEB_PASSWORD=你的密码
    ```

- 数据目录（用于 Docker 部署或集中管理配置/日志）：

    通过 `--data-dir` 参数指定数据目录，程序会将 `config.yml` 和 `logs` 文件夹放在该目录下：

    ```sh
    Tomato-Novel-Downloader.exe --server --data-dir /data
    ```

    Docker 使用示例：

    ```sh
    docker run -v /host/data:/data my-tomato-image --server --data-dir /data
    ```

    这样可以方便地挂载数据目录，实现配置和日志的持久化。

Web UI 提供的功能（纯 HTML，无需额外前端构建）：

- 搜索书籍并创建下载任务
- 任务列表/进度刷新/取消任务
- 下载库按目录浏览（不再把所有文件递归平铺）
- 文件直接下载
- 文件夹一键打包为 zip 下载（保持目录结构，适配音频等“文件夹内包含文件夹”的情况）
- 配置页面：可在线修改部分下载输出相关配置（会写回 `config.yml`）

注意：Web UI 主要面向自建/局域网使用；如果要暴露到公网，建议放在反向代理/HTTPS 后面，并务必开启密码锁。

---

## Docker 镜像

已提供 Web UI 版本的 Docker 镜像：

- 镜像地址：[DockerHub](https://hub.docker.com/r/zhongbai233/tomato-novel-downloader-webui)
- Tags 说明：
  - `latest`：默认 **glibc** 版本（常规服务器/桌面环境）
  - `latest-musl`：**musl** 版本，适用于 **软路由 / NAS** 等轻量系统

示例（映射端口与持久化数据目录，使用 glibc 版本）：

```sh
docker run -d \
    --name tomato-novel-webui \
    -p 18423:18423 \
    -v /host/data:/data \
    -e TOMATO_WEB_ADDR=0.0.0.0:18423 \
    -e TOMATO_WEB_PASSWORD=你的密码 \
    zhongbai233/tomato-novel-downloader-webui:latest --server --data-dir /data
```

如果你使用软路由或 NAS 系统，请使用 musl 版本：

```sh
docker run -d \
    --name tomato-novel-webui \
    -p 18423:18423 \
    -v /host/data:/data \
    -e TOMATO_WEB_ADDR=0.0.0.0:18423 \
    -e TOMATO_WEB_PASSWORD=你的密码 \
    zhongbai233/tomato-novel-downloader-webui:latest-musl --server --data-dir /data
```

可通过 `TOMATO_WEB_ADDR`、`TOMATO_WEB_PASSWORD` 与 `--data-dir` 控制监听地址、密码与数据目录（见上文 Web UI 说明）。

---

## 构建模式（Cargo Features）

本项目提供两个互斥的 feature：`official-api` 与 `no-official-api`（两者不能同时启用）。

### 默认模式：official-api（默认启用）

- 构建（默认就会启用）：

```sh
cargo build --release
```

- 行为：
  - 搜索功能可用（TUI / Web UI / 老 CLI 的搜索入口）。
  - 段评（EPUB 段评页/资源抓取）可用（取决于配置项）。
  - 正文获取可通过配置在“官方/第三方”之间切换（`use_official_api`）。

### No-Official-API 模式：no-official-api（Issue #187）

- 构建：

```sh
cargo build --release --no-default-features --features no-official-api
```

- 行为差异（重点）：
  - **不依赖** `tomato-novel-official-api` crate，可在缺少 Official-API 环境时编译。
  - 目录与书本信息：使用网页解析（`FanqieWebNetwork`）。
  - **正文获取：强制第三方模式**（忽略/不使用 `use_official_api=true` 的官方分支）。
  - 搜索功能：不可用（会返回提示/报错）。
  - 段评：不可用（会被强制关闭）。

---

## Edge TTS 有声小说生成

从当前版本开始，程序内置了 [msedge-tts](https://github.com/hs-cn/msedge-tts) 语音合成功能，可在下载文本后自动生成对应的有声小说：

- 在配置菜单（新 UI 或老 CLI 均可）中启用 `是否生成有声小说`，即可在每次下载完成后生成音频文件。
- 默认发音人是 `zh-CN-XiaoxiaoNeural`，可以通过配置项自定义语速、音量、音调以及输出格式（`mp3` 或 `wav`）。音调值请使用 `+2Hz`、`-1st` 这类带单位的写法，若留空或填写 0 将忽略音调调整。
- 可在“有声小说并发数”中调整 Edge TTS 并发任务数量（默认 2），生成时会显示进度条；请根据网络状况和机器性能选择适当的并发度。
- 音频会存放在输出目录下的 `{书名}_audio` 文件夹中，并按章节顺序命名，例如 `0001-第一章.mp3`。
- msedge-tts 需要联网调用微软的在线服务，请确保运行环境可正常访问外网。

如遇到生成失败，可在日志中查看详细错误信息。

---

## 常见问题

1. 之前就已经有了一个下载器，为什么还要再做一个？

    ~~本程序的初衷就是极致简化番茄小说下载器的代码，使程序更加易于操作与运行，并且更加稳定和快速！~~
    本程序由于重构导致文件体积较大，无法做到原项目的简易，但是此项目胜在傻瓜式操作，无需多余配置，立即使用

2. 手机端可以正常运行吗？

    **仅限安卓设备（Termux）**可以运行。
    但由于 **TUI/CLI 界面对小屏幕不太友好**，手机端更推荐使用 **Web UI 模式（--server）**：在 Termux 里启动服务，然后用手机浏览器操作（或让同一局域网的其它设备访问）。

    Release 里提供 Android arm64 构建产物：`TomatoNovelDownloader-Android_arm64-[当前版本号]`，可直接在 Termux 中运行。

    另外：如果你希望在 TUI 中使用 `Ctrl+V` 从系统剪贴板粘贴，需要安装 Termux API：

    - 安装 App：Termux:API
    - 安装命令：`pkg install termux-api`
    - 验证：`termux-clipboard-get` 可正常输出内容

    为了防止有些零基础的小白下载到了此程序，我们为您准备了一些教程：

    下载termux(链接:(<https://github.com/termux/termux-app/releases>) 并安装，然后运行部署脚本：

    ```sh
    bash <(curl -sL https://raw.githubusercontent.com/zhongbai2333/Tomato-Novel-Downloader/main/installer.sh)
    ```

    国内用户可使用：

    ```sh
    bash <(curl -sL https://dl.zhongbai233.com/installer.sh)
    ```

    安装完成后，推荐用 Web UI 启动（示例）：

    ```sh
    TOMATO_WEB_ADDR=0.0.0.0:18423 TOMATO_WEB_PASSWORD=你的密码 tomato-novel-downloader --server
    ```

    然后在浏览器打开：

    - 本机：`http://127.0.0.1:18423/`
    - 局域网其它设备：`http://<手机的局域网IP>:18423/`

3. 电脑端该如何运行？

    `Windows` 双击运行`TomatoNovelDownloader-Win64-[当前版本号].exe`

    `Linux` 和 `MacOS` 使用终端运行，可以使用一键部署脚本：

    ```sh
    bash <(curl -sL https://raw.githubusercontent.com/zhongbai2333/Tomato-Novel-Downloader/main/installer.sh)
    ```

    国内用户可使用：

    ```sh
    bash <(curl -sL https://dl.zhongbai233.com/installer.sh)
    ```

4. 小说id是什么？在哪里获取？

    推荐两种方式：

    - 直接使用 Web UI 的“搜索书籍”，不需要手动找 ID。
    - 如果你已经有分享链接/书籍信息，通常会包含一段很长的数字（Book ID）。复制该数字即可。

5. 我是纯小白，程序在哪里下载啊

    直接点击此链接(<https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases>)先找到最新版本，然后在最新版本中找到”Assets”并点击来展开内容(如果已展开就不必进行此操作)。在展开的内容中找到对应程序，点击下载即可

## 注意事项（必看）

由于使用的是api，所以未来不知道有哪一天突然失效，如果真的出现了，请立即在“Issues”页面中回复！

如果您在使用本程序的时候出现了下载章节失败的情况，也许并不是api失效了，可能是因为调用api人数过多，导致api暂时关闭，如果遇到了这种情况，请稍后再试，另外，您需要下载的小说api可能会因没有更新所以下载失败。

千万不要想着耍小聪明：“欸，我改一下线程数不就能快速下载了吗？”请打消这种念头！因为这样会加大服务器压力！！！

另外，在使用本程序时，请不要使用任何vpn或网络代理等一切影响网络正常使用的程序！

如果您也没有遇到以上的这种情况，请检查要下载的小说章节数量有多少，不建议大于1500章！(保守估计)

>划重点：切记！不能将此程序用于违法用途，例如将下载到的小说进行转载、给不良人员分享此程序使用等。本开发者严禁不支持这样做！！！并且请不要将api进行转载使用，除非您已经与开发者协商过，否则后果自负！下载到的小说仅供自行阅读，看完之后请立即删除文件，以免造成侵权，如果您还是偷尝禁果，需自行承担由此引发的任何法律责任和风险。程序的作者及项目贡献者不对因使用本程序所造成的任何损失、损害或法律后果负责！

## 免责声明

  本程序仅供 Rust 网络爬虫技术、网页数据处理及相关研究的学习用途。请勿将其用于任何违反法律法规或侵犯他人权益的活动。
  
  使用本程序的用户需自行承担由此引发的任何法律责任和风险。程序的作者及项目贡献者不对因使用本程序所造成的任何损失、损害或法律后果负责。
  
  在使用本程序之前，请确保您遵守适用的法律法规以及目标网站的使用政策。如有任何疑问或顾虑，请咨询专业法律顾问。

## 感谢

感谢用户选择此程序，如果喜欢可以加star，如果有什么对本程序的建议，请在“Issues”页面提出。您的喜欢就是我更新的最大动力❤️

项目前期 · 感谢原作者Dimily的基础项目

项目前期 · 感谢来自Github用户@helloplhm-qwq的api！

项目前期 · 感谢来自QQ用户@终忆的api！

项目前期 · 感谢来自Github用户@jingluopro的api！！
