package com.tomato.downloader.core

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * 知情协议 / 用户协议 同意状态管理。
 *
 * - 首次启动：未同意 → MainActivity 弹出协议对话框，必须 5 秒倒计时结束后才能"同意"；
 *             点击"拒绝"立即 finish 退出应用；
 * - 同意后：写入 SharedPreferences，后续启动均不再弹窗。
 */
object ConsentStore {

    private const val PREFS_NAME = "tomato_consent_prefs"
    private const val KEY_AGREED = "agreed_privacy_v1"

    private val _agreed = MutableStateFlow(false)
    val agreed: StateFlow<Boolean> = _agreed.asStateFlow()

    /** Application.onCreate 时调用，从磁盘恢复同意状态。 */
    fun init(context: Context) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        _agreed.value = prefs.getBoolean(KEY_AGREED, false)
    }

    /** 用户点击「同意」后持久化标记，下一次启动不再弹窗。 */
    fun markAgreed(context: Context) {
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putBoolean(KEY_AGREED, true)
            .apply()
        _agreed.value = true
    }

    // —— 协议正文（统一集中管理，避免写散在 UI 文件里） ——

    const val DIALOG_TITLE = "知情协议与免责声明"

    const val DIALOG_BODY: String =
        """欢迎使用 Web For Tomato。
在使用本软件前，请您仔细阅读以下条款：

1. 功能说明
本软件仅负责：下载并在本地启动 Tomato-Novel-Downloader
提供的官方 Web 服务二进制（以 '--server' 模式运行），
让您在本机或局域网浏览器中访问 Web UI 进行搜索 / 下载。
所有搜索、解析、下载行为均由开源项目 Tomato-Novel-Downloader
完成，本软件不控制其数据源，也不存储 / 上传任何书籍内容。

2. 版权声明
书籍 / 作品版权归原作者及版权方所有。请您仅在合法授权范围内
使用搜索结果，遵守所在地区版权法规。如因此产生任何法律纠纷，
均由使用者本人承担，与本软件作者无关。

3. 使用风险
本软件为非商业免费开源工具，按"现状"提供，不保证：
- 服务运行过程的稳定性、无中断或无错误；
- 第三方数据源的可用性、内容正确性或合法性。
手机 ROM 对私有目录执行权限（W^X）、局域网策略、
浏览器后台策略等都可能影响使用体验。

4. 权限与隐私
本软件仅申请：网络访问（用于下载二进制与服务端联网）、
Wi-Fi 状态查询（用于显示局域网 IP）、前台服务（保持 Web 服务运行）。
本软件不收集、不上传任何个人信息。书籍文件默认保存在
应用私有目录，卸载应用时会一并删除。

5. 协议确认
点击「同意并继续」即视为您已完整阅读并接受以上条款。
如您不同意，请点击「拒绝」，本软件将立即退出且不可使用。"""
}
