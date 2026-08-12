package com.tomato.downloader.core

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * 简单的密码持久化（SharedPreferences）。
 * Tomato-Novel-Downloader 通过 TOMATO_WEB_PASSWORD 环境变量或 --password 参数启用密码锁。
 */
object PasswordStore {

    private const val PREFS_NAME = "tomato_web_prefs"
    private const val KEY_PASSWORD = "web_password"

    private val _password = MutableStateFlow<String?>(null)
    val password: StateFlow<String?> = _password.asStateFlow()

    fun init(context: Context) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        _password.value = prefs.getString(KEY_PASSWORD, null)?.takeIf { it.isNotBlank() }
    }

    fun setPassword(context: Context, pw: String?) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE).edit()
        val clean = pw?.takeIf { it.isNotBlank() }
        if (clean == null) prefs.remove(KEY_PASSWORD) else prefs.putString(KEY_PASSWORD, clean)
        prefs.apply()
        _password.value = clean
    }

    fun hasPassword(): Boolean = !_password.value.isNullOrBlank()
}
