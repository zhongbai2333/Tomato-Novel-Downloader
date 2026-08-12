package com.tomato.downloader.core

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File

/**
 * 自定义 Web 二进制管理：允许用户通过系统文件管理器选择自己的 Web 二进制文件，
 * 复制到应用私有可执行目录（codeCacheDir）并赋予执行权限，替代默认从 GitHub 下载的二进制。
 *
 * 持久化（SharedPreferences）：
 *  - custom_web_enabled: 是否启用自定义 Web
 *  - custom_web_name:    用户选择的文件名（仅用于 UI 显示）
 *
 * 自定义二进制文件存放在 codeCacheDir/custom_web_binary（无扩展名），
 * 与默认二进制同目录，确保可执行权限一致。
 */
object CustomWebStore {

    private const val PREFS_NAME = "tomato_web_prefs"
    private const val KEY_ENABLED = "custom_web_enabled"
    private const val KEY_NAME = "custom_web_name"
    private const val CUSTOM_BINARY_NAME = "custom_web_binary"

    private val _enabled = MutableStateFlow(false)
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    private val _name = MutableStateFlow("")
    val name: StateFlow<String> = _name.asStateFlow()

    fun init(context: Context) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        _enabled.value = prefs.getBoolean(KEY_ENABLED, false)
        _name.value = prefs.getString(KEY_NAME, "") ?: ""
        // 如果标记为启用但文件不存在（被清理），重置状态
        if (_enabled.value && !customBinaryFile(context).exists()) {
            _enabled.value = false
            _name.value = ""
        }
    }

    /** 自定义二进制文件路径（codeCacheDir/custom_web_binary）。 */
    fun customBinaryFile(context: Context): File {
        return File(context.codeCacheDir, CUSTOM_BINARY_NAME)
    }

    /** 自定义二进制是否已复制到本地。 */
    fun isCustomInstalled(context: Context): Boolean = customBinaryFile(context).exists()

    /**
     * 从 SAF Uri 复制文件到 codeCacheDir，赋予可执行权限，并持久化状态。
     * 调用方应在 IO 线程执行。
     * @return 成功返回 true，失败返回 false。
     */
    fun setCustomWeb(context: Context, uri: Uri): Boolean {
        return try {
            val dir = context.codeCacheDir
            dir.mkdirs()
            val dest = File(dir, CUSTOM_BINARY_NAME)

            // 复制文件
            context.contentResolver.openInputStream(uri)?.use { input ->
                dest.outputStream().use { output ->
                    input.copyTo(output)
                }
            } ?: return false

            // 赋予可执行权限
            dest.setExecutable(true, false)

            // 获取文件名（用于显示）
            val fileName = queryFileName(context, uri)

            // 持久化
            val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE).edit()
            prefs.putBoolean(KEY_ENABLED, true)
            prefs.putString(KEY_NAME, fileName)
            prefs.apply()
            _enabled.value = true
            _name.value = fileName
            true
        } catch (e: Exception) {
            false
        }
    }

    /** 切换开关状态（不删除文件，只是启用/禁用）。 */
    fun setEnabled(context: Context, enabled: Boolean) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE).edit()
        prefs.putBoolean(KEY_ENABLED, enabled)
        prefs.apply()
        _enabled.value = enabled
    }

    /** 彻底清除自定义 Web：删除文件 + 重置状态。 */
    fun clearCustomWeb(context: Context) {
        val dest = customBinaryFile(context)
        if (dest.exists()) dest.delete()
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE).edit()
        prefs.remove(KEY_ENABLED)
        prefs.remove(KEY_NAME)
        prefs.apply()
        _enabled.value = false
        _name.value = ""
    }

    /** 从 SAF Uri 查询文件显示名。 */
    private fun queryFileName(context: Context, uri: Uri): String {
        try {
            context.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
                val nameIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (nameIndex >= 0 && cursor.moveToFirst()) {
                    return cursor.getString(nameIndex) ?: uri.lastPathSegment ?: "unknown"
                }
            }
        } catch (_: Exception) { }
        return uri.lastPathSegment ?: "unknown"
    }
}
