# 番茄小说下载器~~精简版~~

番茄小说下载器*不精简*版本，由于项目[fanqienovel-downloader](https://github.com/ying-ck/fanqienovel-downloader)一直不更新，于是我根据Dimily的项目Fork并重构

我对其进行重构 + 优化，添加更对功能，包括：EPUB下载支持、更好的断点传输、更好的错误管理、书本搜索等特性

本项目~~完全~~基于第三方API，~~未~~使用官方API

为了保证第三方API安全，关于helloplhm-qwq的API调用相关代码并不开源，包括地址和token，敬请谅解，谢谢！

**由于官方API失效，1.4版本升级1.5版本升级指南：**

 > 输入 s -> 设置面板 -> 关闭 官方API 模式

为方便视障人士使用，我保留了老的CLI界面，接下来是启用方法：

在第一次打开程序时 按三下 `o` 并回车 或者 按一下下方向键并按三下 `o` 都可以启用老版本CLI界面

注意：切换成功应该会发出 `灯` 的一声

---

## 我该如何使用？

根据自己的系统版本在[Releases](https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases)列表下载可执行文件，并运行
你可以通过输入书籍id以及需要保存的路径来进行下载

---

## 常见问题

1. 之前就已经有了一个下载器，为什么还要再做一个？

    ~~本程序的初衷就是极致简化番茄小说下载器的代码，使程序更加易于操作与运行，并且更加稳定和快速！~~
    本程序由于重构导致文件体积较大，无法做到原项目的简易，但是此项目胜在傻瓜式操作，无需多余配置，立即使用

2. 手机端可以正常运行吗？

    **仅限安卓设备**可以正常运行，为了防止有些零基础的小白下载到了此程序，我们为您准备了一些教程：

    下载termux(链接:(<https://github.com/termux/termux-app/releases>) 并安装，然后运行部署脚本：

    ```sh
    bash <(curl -sL https://raw.githubusercontent.com/zhongbai2333/Tomato-Novel-Downloader/main/installer.sh)
    ```

    国内用户可使用：

    ```sh
    RAW_URL="https://raw.githubusercontent.com/zhongbai2333/Tomato-Novel-Downloader/main/installer.sh"; API="https://api.cenguigui.cn/api/github/?type=json&url=$RAW_URL"; DL_URL="$(curl -fsSL "$API" | sed -n 's/.*\"downUrl\":\"\([^\"]*\)\".*/\1/p')"; DL_URL="${DL_URL//\\//}"; [ -z "$DL_URL" ] && DL_URL="$RAW_URL"; bash <(curl -fsSL "$DL_URL")
    ```

3. 电脑端该如何运行？

    `Windows` 双击运行`TomatoNovelDownloader-Win64-[当前版本号].exe`

    `Linux` 和 `MacOS` 使用终端运行，可以使用一键部署脚本：

    ```sh
    bash <(curl -sL https://raw.githubusercontent.com/zhongbai2333/Tomato-Novel-Downloader/main/installer.sh)
    ```

    国内用户可使用：

    ```sh
    RAW_URL="https://raw.githubusercontent.com/zhongbai2333/Tomato-Novel-Downloader/main/installer.sh"; API="https://api.cenguigui.cn/api/github/?type=json&url=$RAW_URL"; DL_URL="$(curl -fsSL "$API" | sed -n 's/.*\"downUrl\":\"\([^\"]*\)\".*/\1/p')"; DL_URL="${DL_URL//\\//}"; [ -z "$DL_URL" ] && DL_URL="$RAW_URL"; bash <(curl -fsSL "$DL_URL")
    ```

4. 小说id是什么？在哪里获取？

    首先你需要找到自己想下载的小说的详情页(例如<https://fanqienovel.com/page/7143038691944959011> )，链接中“7143038691944959011”就是小说id

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

  本程序仅供 Python 网络爬虫技术、网页数据处理及相关研究的学习用途。请勿将其用于任何违反法律法规或侵犯他人权益的活动。
  
  使用本程序的用户需自行承担由此引发的任何法律责任和风险。程序的作者及项目贡献者不对因使用本程序所造成的任何损失、损害或法律后果负责。
  
  在使用本程序之前，请确保您遵守适用的法律法规以及目标网站的使用政策。如有任何疑问或顾虑，请咨询专业法律顾问。

## 感谢

感谢用户选择此程序，如果喜欢可以加star，如果有什么对本程序的建议，请在“Issues”页面提出。您的喜欢就是我更新的最大动力❤️

感谢原作者Dimily的基础项目

感谢来自Github用户@helloplhm-qwq的api！

项目前期 · 感谢来自QQ用户@终忆的api！

项目前期 · 感谢来自Github用户@jingluopro的api！！

---

## 自定义章节模板 (实验功能)

你可以通过一个外部模板文件自由控制章节内容的最终呈现（仅在保存到缓存/散装文件/EPUB 章节前进行替换）。

### 启用方式

1. 在 `config.yml` 中设置：

```yaml
enable_chapter_template: true
chapter_template_file: chapter_template.txt
```

1. 或在交互界面 / 旧 CLI 配置菜单中勾选 “启用自定义章节模板”，并指定模板文件路径。

### 占位符说明

| 占位符 | 说明 |
| ------ | ---- |
| `{title}` | 当前章节标题 |
| `{content}` | 原始正文（不做额外处理） |
| `{text}` | 与 `{content}` 相同，预留未来整体加工用 |
| `{paragraphs}` | 将正文按换行切分后重新拼接（逐行） |
| `{{for p in paragraphs}} ... {p} ... {{end}}` | 段落循环，循环体内 `{p}` 为每个段落文本 |
| `{paragraph}` | 在循环体里等同 `{p}` |

> 拆分规则：当前实现按“换行”分割，不会自动去除空行。

### 基本示例

#### 1. 段首统一缩进 4 个空格

模板：

```text
{title}

{{for p in paragraphs}}    {p}
{{end}}
```

效果：每一行（段）前都会多 4 个空格。

#### 2. 仅正文第一段缩进

```text
{title}

    {preamble}
{{for p in paragraphs}}{p}
{{end}}
```

将正文第一行提取前置：目前没有内置 `{preamble}`，可手动：

```text
{title}

    {content}
```

然后让上游内容本身第一行带缩进（或后续版本再扩展 `{first_paragraph}`）。

#### 3. 包裹段落为 HTML 段（适合 EPUB 自定义文本模式）

```html
{title}

{{for p in paragraphs}}<p style="text-indent:2em;">{p}</p>
{{end}}
```

#### 4. 添加章节分隔线

```text
========== {title} ==========

{{for p in paragraphs}}    {p}
{{end}}

---------- END ----------
```

### 回退策略

模板文件不存在或解析异常时，会记录日志并回退到原始正文，不影响下载流程。

### 注意

当前模板引擎是轻量自研（正则 + 简单循环），不支持条件判断 / 嵌套循环 / 变量赋值；后续若需求复杂可考虑引入 Jinja2（需体积权衡）。

欢迎在 Issues 中反馈需要的新占位符。
