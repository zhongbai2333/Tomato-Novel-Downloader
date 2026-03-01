# AI Not-Fix / Duplicate 参考案例

> 目的：给 Issue Assistant 提供“通常不准备修复”的历史样例与处理策略。
>
> 注意：**重复判断只与当前 Open Issues 对比**。关闭历史仅用于“策略参考”，不直接作为重复目标。

## 处理总规则

1. 若新 Issue 与 **当前 Open Issues** 中某条语义高度重复：
   - `negative = Duplicate`
   - 类型按语义保留（BUG / Question / Enhancement / Documentation）
2. 若未与 Open Issues 重复，但命中下方“Not-Fix 策略样例”的同类诉求：
   - `negative = WontFix`
3. Question 为中性咨询类：
   - 可判定 `Needs-Info`，但不应进入 Release Notes 的代码变更分组。

## Not-Fix 策略样例（用户提供）

- #90  [Not Fix] 一键更新
  - URL: <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/90>
  - 结论：不计划实现一键更新/批量自动更新队列
- #230 [Question] 有没有 macOS 10.15 系统可用版本
  - URL: <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/230>
  - 结论：受 Runner/平台限制，不计划支持该场景
- #265 [Not Fix] 能不能增加功能，API?
  - URL: <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/265>
  - 结论：不计划在该项目提供此方向
- #243 [Question] 下载 VIP 书籍，不能全本下载吗？
  - URL: <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/243>
  - 结论：不支持需登录/VIP能力
- #242 [Question] 为什么下载失败（AdGuard/VPN 拦截 IID 相关）
  - URL: <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/242>
  - 结论：环境/网络拦截导致，通常非代码缺陷
- #233 [Not Fix] txt 增加空行关闭开关
  - URL: <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/233>
  - 结论：不计划新增该格式开关

## 高频重复簇（补充）

以下均与 #90 相关、通常按重复处理：

- #93  <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/93>
- #126 <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/126>
- #128 <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/128>
- #139 <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/139>
- #162 <https://github.com/zhongbai2333/Tomato-Novel-Downloader/issues/162>

## 关键词命中表（增强稳健性）

> 用法建议：
>
> 1. 先做语义相似度判断（优先级最高）。
> 2. 若语义判断不稳定，再用本表做“关键词补偿判定”。
> 3. 若命中关键词且 **Open Issues** 有同类，优先 `Duplicate`；若 Open 无同类，按表中默认策略。

| 关键词（任一命中即可） | 归类意图 | 默认策略（Open 无同类时） | 锚点案例 |
| --- | --- | --- | --- |
| 一键更新 / 批量更新 / 全部更新 / 自动更新所有 | 更新队列/批量自动化请求 | `WontFix`（若 Open 有同类则 `Duplicate`） | #90 |
| 更新队列 / 自动下载模块 / 无人值守更新 | 与项目目标冲突的自动化队列能力 | `WontFix`（若 Open 有同类则 `Duplicate`） | #90, #93 |
| VIP全本 / VIP章节 / 会员章节 / 登录下载 | 需账号或付费能力 | `WontFix`（若 Open 有同类则 `Duplicate`） | #243 |
| API封装 / 提供API / HTTP接口 / SDK接口 | 将本项目作为通用 API 服务输出 | `WontFix`（若 Open 有同类则 `Duplicate`） | #265 |
| macOS 10.15 / Intel Mac / x86_64 Mac 版本 | 受构建环境限制的平台支持诉求 | `WontFix`（若 Open 有同类则 `Duplicate`） | #230 |
| AdGuard / VPN / snssdk 被拦截 / IID 获取失败 | 网络或本地环境导致的问题（通常非代码缺陷） | `Question` + `Needs-Info` 或 `WontFix` | #242 |
| txt 空行 / 段落空行 / 去空行开关 | 文本格式个性化开关诉求 | `WontFix`（若 Open 有同类则 `Duplicate`） | #233 |

### 关键词命中时的补充规则

- 若用户同时提供明确崩溃日志、可稳定复现步骤，并且指向代码缺陷，`BUG` 仍可优先于关键词规则。
- `Question` 属中性标签，不进入 Release Notes 的代码改动分组。
- 关键词只用于“辅助稳定”，不应覆盖维护者的最终人工判断。

## 给 AI 的判定提示（简版）

- 先与 **Open Issues** 比较相似度，再决定是否 Duplicate。
- 命中本文件中的 Not-Fix 同类请求，但 Open 无重复时，可判 WontFix。
- 输出保持简体中文，标题按类型前缀规范生成。
