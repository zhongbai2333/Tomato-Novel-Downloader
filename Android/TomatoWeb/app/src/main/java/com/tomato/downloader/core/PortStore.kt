package com.tomato.downloader.core

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * 自定义端口持久化（SharedPreferences），与 PasswordStore 同模式。
 * 默认端口 18423；用户可在「关于」页自定义并保存。
 * ServerManager 每次启动服务时会读取当前端口以注入环境变量与命令行参数。
 */
object PortStore {

    const val DEFAULT_PORT = 18423

    private const val PREFS_NAME = "tomato_web_prefs"
    private const val KEY_PORT = "web_port"

    private val _port = MutableStateFlow(DEFAULT_PORT)
    val port: StateFlow<Int> = _port.asStateFlow()

    /** 当前端口号（快捷读取） */
    val currentPort: Int get() = _port.value

    fun init(context: Context) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val stored = prefs.getInt(KEY_PORT, -1)
        _port.value = if (isValidPort(stored)) stored else DEFAULT_PORT
    }

    /** 设置端口；保存前校验（1024~65535）。返回 true 表示保存成功。 */
    fun setPort(context: Context, port: Int): Boolean {
        if (!isValidPort(port)) return false
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE).edit()
        prefs.putInt(KEY_PORT, port)
        prefs.apply()
        _port.value = port
        // 同步刷新 ServerManager 中的 URL（立即更新 UI 显示）
        ServerManager.onPortChanged(port)
        return true
    }

    /** 恢复默认端口（18423）。 */
    fun resetDefault(context: Context) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE).edit()
        prefs.remove(KEY_PORT)
        prefs.apply()
        _port.value = DEFAULT_PORT
        ServerManager.onPortChanged(DEFAULT_PORT)
    }

    fun isDefault(): Boolean = _port.value == DEFAULT_PORT

    /** 校验端口范围（1024-65535，避开系统端口）。 */
    fun isValidPort(port: Int): Boolean = port in 1024..65535
}
