package com.tomato.downloader.core

import android.content.Context
import android.net.Uri
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 已下载书籍管理器：扫描应用私有目录下的常见书籍格式（.txt/.epub/.pdf），供「关于」页展示。
 *
 * Tomato-Novel-Downloader 下载的书默认存放在：
 *  - 进程工作目录 = ServerManager.startServer() 中传入的 workDir = codeCacheDir 或探测到的 execDir
 *  - 通常子目录为 Downloads/番茄小说/ 或 Downloads/TomatoNovel/ 等
 * 为兼容性，我们递归扫描以下位置的所有书籍文件：
 *  [应用 filesDir, codeCacheDir, cacheDir, getExternalFilesDir(null)]
 */
object BookStore {

    /** 支持的书籍文件扩展名（小写）。 */
    private val BOOK_EXTS = setOf("txt", "epub", "pdf", "mobi", "azw3", "docx", "md")

    /** 书籍信息 */
    data class BookInfo(
        val file: File,
        val name: String,
        val ext: String,
        val sizeBytes: Long,
        val lastModified: Long
    ) {
        val sizeText: String get() = formatSize(sizeBytes)
        val dateText: String get() = formatDate(lastModified)
        val path: String get() = file.absolutePath
    }

    /** 返回当前所有已发现的书籍文件（按修改时间倒序），随时可调用刷新。 */
    fun scanBooks(context: Context): List<BookInfo> {
        val candidates = mutableListOf<File>()

        // 1. 可执行目录（ServerManager workDir，番茄下载器的默认 cwd）
        ServerManager.execDir.value?.let { candidates.add(it) }
        candidates.add(context.codeCacheDir)

        // 2. 应用私有主目录
        candidates.add(context.filesDir)
        candidates.add(context.cacheDir)

        // 3. 外部存储（若有）
        try { context.getExternalFilesDir(null)?.let { candidates.add(it) } } catch (_: Exception) {}

        // 4. 常见子路径（提升扫描速度）
        val roots = candidates + candidates.flatMap { parent ->
            listOf(
                File(parent, "Downloads"),
                File(parent, "Downloads/番茄小说"),
                File(parent, "Downloads/TomatoNovel"),
                File(parent, "番茄小说"),
                File(parent, "TomatoNovel"),
                File(parent, "novels"),
                File(parent, "books")
            )
        }

        val found = hashMapOf<String, BookInfo>() // 去重（同路径只记录一次）
        for (root in roots) {
            if (!root.exists() || !root.isDirectory) continue
            try {
                root.walkTopDown()
                    .onFail { _, _ -> } // 忽略访问失败
                    .maxDepth(6)        // 限制深度避免性能问题
                    .filter { it.isFile }
                    .forEach { f ->
                        val ext = f.extension.lowercase(Locale.ROOT)
                        if (ext in BOOK_EXTS && f.length() > 0L) {
                            val abs = f.absolutePath
                            if (abs !in found) {
                                found[abs] = BookInfo(
                                    file = f,
                                    name = f.nameWithoutExtension,
                                    ext = ext.uppercase(),
                                    sizeBytes = f.length(),
                                    lastModified = f.lastModified()
                                )
                            }
                        }
                    }
            } catch (_: Exception) { /* 安全忽略 */ }
        }
        return found.values.sortedByDescending { it.lastModified }
    }

    /** 删除单本书（用户删除已下载书籍时调用）。 */
    fun deleteBook(book: BookInfo): Boolean = runCatching { book.file.delete() }.getOrDefault(false)

    /**
     * 把书籍复制到用户通过 SAF 选择器指定的目标 Uri（对应 ACTION_CREATE_DOCUMENT）。
     *
     * 为什么用 Uri 而不是 File path：Android 10+ 分区存储下，应用无法直接写入外部存储任意目录；
     * 通过 Storage Access Framework 让用户选择位置，系统会为该 Uri 授予临时写入权限。
     *
     * @return 复制成功返回 true；失败返回 false（可配合 Toast 提示用户）。
     */
    fun copyToUri(context: Context, book: BookInfo, uri: Uri): Boolean = runCatching {
        context.contentResolver.openOutputStream(uri)?.use { out ->
            book.file.inputStream().use { it.copyTo(out) }
        } != null
    }.getOrDefault(false)

    /** 复制到应用外部存储公共目录（兜底：SAF 不可用时的快捷路径选择）。
     *  目标文件会自动加序号避免覆盖。
     */
    fun copyToFile(context: Context, book: BookInfo, destDir: File): File? {
        if (!destDir.exists() && !destDir.mkdirs()) return null
        val baseName = book.name.ifBlank { "book" }
        val ext = book.ext.lowercase(Locale.ROOT)
        var target = File(destDir, "$baseName.$ext")
        var i = 1
        while (target.exists()) {
            target = File(destDir, "${baseName}_${i++}.$ext")
        }
        return runCatching {
            book.file.copyTo(target, overwrite = false)
            target
        }.getOrNull()
    }

    /** 常用快捷目标目录（用户点击"复制到..."时作为快捷选项展示）。 */
    fun quickDestDirs(context: Context): List<Pair<String, File>> {
        val out = mutableListOf<Pair<String, File>>()
        runCatching {
            // Android/data/<pkg>/files/Download 等外部公共目录
            val externalRoot = context.getExternalFilesDir(null)
            if (externalRoot != null) {
                val download = File(externalRoot, "Download").apply { mkdirs() }
                out += "应用外部 · Download" to download
                val books = File(externalRoot, "Books").apply { mkdirs() }
                out += "应用外部 · Books" to books
            }
        }
        runCatching {
            // 共享外部存储根的 Documents/Download（Android 9- 可直接写；10+ 多数 ROM 也仍允许 SAF 授权路径兜底）
            val ext = android.os.Environment.getExternalStorageDirectory()
            if (ext != null) {
                val docDir = File(ext, "Documents/TomatoNovel").apply { mkdirs() }
                if (docDir.canWrite()) out += "内部存储 · Documents/TomatoNovel" to docDir
                val dlDir = File(ext, "Download/TomatoNovel").apply { mkdirs() }
                if (dlDir.canWrite()) out += "内部存储 · Download/TomatoNovel" to dlDir
            }
        }
        // 兜底：应用私有 filesDir/books（保证一定可写）
        val privateBooks = File(context.filesDir, "books_export").apply { mkdirs() }
        out += "应用私有 · books_export" to privateBooks
        return out
    }

    // ---------- 辅助 ----------

    private fun formatSize(bytes: Long): String = when {
        bytes < 1024 -> "$bytes B"
        bytes < 1024 * 1024 -> "%.1f KB".format(bytes / 1024.0)
        bytes < 1024 * 1024 * 1024 -> "%.2f MB".format(bytes / (1024.0 * 1024))
        else -> "%.2f GB".format(bytes / (1024.0 * 1024 * 1024))
    }

    private fun formatDate(ts: Long): String = runCatching {
        SimpleDateFormat("yyyy-MM-dd HH:mm", Locale.getDefault()).format(Date(ts))
    }.getOrDefault("-")
}
