package com.tomato.downloader.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.ArrowBack
import androidx.compose.material.icons.outlined.MenuBook
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.tomato.downloader.core.BookStore
import com.tomato.downloader.ui.theme.Green
import com.tomato.downloader.ui.theme.TerminalBg
import com.tomato.downloader.ui.theme.TerminalSurface
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.nio.charset.Charset
import java.util.zip.ZipInputStream

/** 书籍预览章节（已解析为纯文本的章节） */
data class PreviewChapter(val title: String, val content: String)

/** 加载状态 */
sealed class PreviewState {
    object Loading : PreviewState()
    data class Success(val chapters: List<PreviewChapter>, val bookTitle: String) : PreviewState()
    data class Error(val message: String) : PreviewState()
}

/**
 * 书籍预览界面：支持 txt 和 epub 格式的应用内阅读。
 *
 * EPUB 解析方案：EPUB 本质是 ZIP 包，内含 XHTML 章节文件。
 * 用 java.util.zip.ZipInputStream 遍历，找到 content.opf 确定 spine 阅读顺序，
 * 然后用正则去除 XHTML 标签提取纯文本。零外部依赖。
 */
@Composable
fun BookPreviewScreen(book: BookStore.BookInfo, onBack: () -> Unit) {
    var state by remember { mutableStateOf<PreviewState>(PreviewState.Loading) }

    LaunchedEffect(book.path) {
        state = PreviewState.Loading
        try {
            state = withContext(Dispatchers.IO) { loadBook(book) }
        } catch (e: Exception) {
            state = PreviewState.Error("加载失败：${e.message ?: "未知错误"}")
        }
    }

    Scaffold(
        containerColor = TerminalBg,
        topBar = {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(TerminalSurface)
                    .padding(horizontal = 8.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically
            ) {
                IconButton(onClick = onBack) {
                    Icon(
                        Icons.Outlined.ArrowBack,
                        contentDescription = "返回",
                        tint = Color(0xFFE6EDF3)
                    )
                }
                Column(Modifier.weight(1f)) {
                    Text(
                        text = (state as? PreviewState.Success)?.bookTitle ?: book.name,
                        color = Color(0xFFE6EDF3),
                        fontWeight = FontWeight.SemiBold,
                        fontSize = 15.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis
                    )
                    Text(
                        text = "${book.ext} · ${book.sizeText}",
                        color = Color(0xFF8B949E),
                        fontSize = 11.sp
                    )
                }
                Icon(
                    Icons.Outlined.MenuBook,
                    contentDescription = null,
                    tint = Green,
                    modifier = Modifier.size(22.dp)
                )
            }
        }
    ) { innerPadding ->
        when (val s = state) {
            PreviewState.Loading -> Box(
                Modifier.fillMaxSize().padding(innerPadding),
                contentAlignment = Alignment.Center
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator(color = Green, strokeWidth = 2.dp, modifier = Modifier.size(36.dp))
                    Spacer(Modifier.height(12.dp))
                    Text("正在解析书籍…", color = Color(0xFF8B949E), fontSize = 13.sp)
                }
            }

            is PreviewState.Error -> Box(
                Modifier.fillMaxSize().padding(innerPadding).padding(24.dp),
                contentAlignment = Alignment.Center
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text("❌", fontSize = 32.sp)
                    Spacer(Modifier.height(8.dp))
                    Text(s.message, color = Color(0xFFFF6B6B), fontSize = 13.sp)
                }
            }

            is PreviewState.Success -> {
                val listState = rememberLazyListState()
                LazyColumn(
                    state = listState,
                    modifier = Modifier.fillMaxSize().padding(innerPadding),
                    contentPadding = PaddingValues(horizontal = 16.dp, vertical = 16.dp),
                    verticalArrangement = Arrangement.spacedBy(24.dp)
                ) {
                    items(s.chapters, key = { it.title + it.content.hashCode() }) { chapter ->
                        Column {
                            Box(
                                Modifier
                                    .fillMaxWidth()
                                    .background(color = Color(0x1422C55E), shape = RoundedCornerShape(8.dp))
                                    .padding(horizontal = 12.dp, vertical = 8.dp)
                            ) {
                                Text(
                                    text = chapter.title,
                                    color = Green,
                                    fontWeight = FontWeight.Bold,
                                    fontSize = 15.sp
                                )
                            }
                            Spacer(Modifier.height(12.dp))
                            Text(
                                text = chapter.content,
                                color = Color(0xFFD0D7DE),
                                fontSize = 15.sp,
                                lineHeight = 26.sp
                            )
                        }
                    }
                    item { Spacer(Modifier.height(48.dp)) }
                }
            }
        }
    }
}

// ======================== 解析逻辑 ========================

private fun loadBook(book: BookStore.BookInfo): PreviewState {
    val file = book.file
    if (!file.exists() || !file.canRead()) return PreviewState.Error("文件不存在或无法读取")
    return when (book.ext.lowercase()) {
        "txt" -> loadTxt(file, book.name)
        "epub" -> loadEpub(file)
        else -> PreviewState.Error("暂不支持预览 ${book.ext} 格式")
    }
}

/**
 * TXT 解析：
 * 1. 自动检测编码（UTF-8 / GBK）
 * 2. 优先按"第X章"等正则切分章节
 * 3. 无章节标记则按 ~2000 字分段
 */
private fun loadTxt(file: File, fallbackTitle: String): PreviewState {
    val rawBytes = file.readBytes()
    val text = detectEncoding(rawBytes).let { String(rawBytes, it) }
    val firstLine = text.lineSequence().firstOrNull { it.isNotBlank() }?.trim()
    val bookTitle = firstLine?.takeIf { it.length <= 50 } ?: fallbackTitle

    val chapterRegex = Regex(
        """^\s*(第[零一二三四五六七八九十百千万0-9]+[章节回卷]|Chapter\s+\d+|chapter\s+\d+).*""",
        RegexOption.MULTILINE
    )
    val matches = chapterRegex.findAll(text).toList()

    val chapters = if (matches.size >= 2) {
        matches.mapIndexed { index, match ->
            val start = match.range.first
            val end = if (index + 1 < matches.size) matches[index + 1].range.first else text.length
            val chunk = text.substring(start, end).trim()
            val firstNl = chunk.indexOf('\n')
            val title = if (firstNl > 0) chunk.substring(0, firstNl).trim() else chunk.take(30)
            val content = if (firstNl > 0) chunk.substring(firstNl).trim() else ""
            PreviewChapter(title.ifBlank { "第${index + 1}节" }, content.ifBlank { chunk })
        }
    } else {
        // 无章节标记：按段落累积 ~2000 字一段
        val cleanText = text.replace("\r\n", "\n").replace("\r", "\n")
        val paragraphs = cleanText.split("\n").filter { it.isNotBlank() }
        val chunks = mutableListOf<MutableList<String>>()
        var current = mutableListOf<String>()
        var currentLen = 0
        for (para in paragraphs) {
            current.add(para.trim())
            currentLen += para.length
            if (currentLen >= 2000) { chunks.add(current); current = mutableListOf(); currentLen = 0 }
        }
        if (current.isNotEmpty()) chunks.add(current)
        chunks.mapIndexed { index, lines ->
            PreviewChapter("${bookTitle} · 第${index + 1}段", lines.joinToString("\n\n"))
        }
    }

    return if (chapters.isEmpty()) PreviewState.Error("文件内容为空")
    else PreviewState.Success(chapters, bookTitle)
}

/**
 * EPUB 解析（纯标准库）：
 * EPUB = ZIP 包。结构：
 *   META-INF/container.xml → 指向 OPF 文件路径
 *   *.opf → 包含 <manifest>（资源列表）和 <spine>（阅读顺序）
 *   章节 XHTML 文件
 *
 * 流程：
 * 1. 解压 ZIP，读取所有 entry 到内存 Map
 * 2. 解析 container.xml 找到 OPF 路径
 * 3. 解析 OPF：从 manifest 拿 id→href 映射，从 spine 拿阅读顺序 id 列表
 * 4. 按 spine 顺序读取 XHTML，正则去标签提纯文本
 */
private fun loadEpub(file: File): PreviewState {
    val entries = mutableMapOf<String, ByteArray>()
    ZipInputStream(file.inputStream()).use { zis ->
        var entry = zis.nextEntry
        while (entry != null) {
            if (!entry.isDirectory) {
                val name = entry.name
                val data = zis.readBytes()
                entries[name] = data
                // 同时存小写 key 方便查找
                entries[name.lowercase()] = data
            }
            entry = zis.nextEntry
        }
    }

    if (entries.isEmpty()) return PreviewState.Error("EPUB 文件为空")

    // 1. 找 OPF 路径
    val containerXml = entries["META-INF/container.xml"]
        ?: entries["meta-inf/container.xml"]
        ?: return PreviewState.Error("找不到 container.xml")
    val opfPath = extractOpfPath(String(containerXml, Charset.forName("UTF-8")))
        ?: return PreviewState.Error("无法解析 container.xml")
    val opfDir = opfPath.substringBeforeLast('/', "")

    // 2. 解析 OPF
    val opfData = findEntry(entries, opfPath)
        ?: return PreviewState.Error("找不到 OPF 文件: $opfPath")
    val opfText = String(opfData, detectEncoding(opfData))

    // 书名
    val bookTitle = Regex("""<dc:title[^>]*>([^<]+)</dc:title>""", RegexOption.IGNORE_CASE)
        .find(opfText)?.groupValues?.get(1)?.trim()?.takeIf { it.isNotEmpty() }
        ?: file.nameWithoutExtension

    // manifest: id → href
    val manifestRegex = Regex("""<item\b[^>]*\bid\s*=\s*["']([^"']+)["'][^>]*\bhref\s*=\s*["']([^"']+)["'][^>]*/>""", RegexOption.IGNORE_CASE)
    val manifest = mutableMapOf<String, String>()
    // 也匹配 href 在 id 前的情况
    val manifestRegex2 = Regex("""<item\b[^>]*\bhref\s*=\s*["']([^"']+)["'][^>]*\bid\s*=\s*["']([^"']+)["'][^>]*/>""", RegexOption.IGNORE_CASE)
    manifestRegex.findAll(opfText).forEach { m ->
        manifest[m.groupValues[1]] = m.groupValues[2]
    }
    manifestRegex2.findAll(opfText).forEach { m ->
        val href = m.groupValues[1]
        val id = m.groupValues[2]
        if (id !in manifest) manifest[id] = href
    }

    // spine: 有序的 idref 列表
    val spineRegex = Regex("""<itemref\b[^>]*\bidref\s*=\s*["']([^"']+)["']""", RegexOption.IGNORE_CASE)
    val spineIds = spineRegex.findAll(opfText).map { it.groupValues[1] }.toList()

    if (spineIds.isEmpty()) return PreviewState.Error("EPUB spine 为空")

    // 3. 按 spine 顺序读取 XHTML
    val chapters = mutableListOf<PreviewChapter>()
    spineIds.forEachIndexed { index, id ->
        val href = manifest[id] ?: return@forEachIndexed
        val fullPath = if (opfDir.isNotEmpty()) "$opfDir/$href" else href
        val data = findEntry(entries, fullPath) ?: return@forEachIndexed
        val html = String(data, detectEncoding(data))
        val text = htmlToText(html)
        if (text.isNotBlank()) {
            val title = extractTitleFromHtml(html) ?: "第${index + 1}节"
            chapters.add(PreviewChapter(title, text))
        }
    }

    return if (chapters.isEmpty()) PreviewState.Error("EPUB 解析后无可用内容")
    else PreviewState.Success(chapters, bookTitle)
}

/** 从 container.xml 中提取 OPF 文件路径 */
private fun extractOpfPath(containerXml: String): String? {
    val regex = Regex("""<rootfile\b[^>]*\bfull-path\s*=\s*["']([^"']+)["']""", RegexOption.IGNORE_CASE)
    return regex.find(containerXml)?.groupValues?.get(1)
}

/** 在 entries Map 中查找文件（支持大小写不敏感、去除前导 ./ ） */
private fun findEntry(entries: Map<String, ByteArray>, path: String): ByteArray? {
    if (path in entries) return entries[path]
    val lower = path.lowercase()
    if (lower in entries) return entries[lower]
    // 尝试仅用文件名匹配
    val fileName = path.substringAfterLast('/')
    if (fileName in entries) return entries[fileName]
    val fileNameLower = fileName.lowercase()
    if (fileNameLower in entries) return entries[fileNameLower]
    return null
}

/** 检测字节数组的编码：有 BOM → UTF-8/UTF-16；否则尝试 UTF-8，失败回退 GBK */
private fun detectEncoding(bytes: ByteArray): Charset {
    // BOM 检测
    if (bytes.size >= 3 && bytes[0] == 0xEF.toByte() && bytes[1] == 0xBB.toByte() && bytes[2] == 0xBF.toByte()) {
        return Charset.forName("UTF-8")
    }
    // 尝试 UTF-8 解码：如果无替换字符则认为是 UTF-8
    val utf8Decoded = String(bytes, Charset.forName("UTF-8"))
    if (!utf8Decoded.contains('\uFFFD')) return Charset.forName("UTF-8")
    // 回退 GBK
    return try { Charset.forName("GBK") } catch (_: Exception) { Charset.forName("UTF-8") }
}

/** XHTML → 纯文本：去标签、解码实体、按句号分段 */
private fun htmlToText(html: String): String {
    // 先提取 body 内容（如果有）
    val bodyMatch = Regex("""<body[^>]*>([\s\S]*?)</body>""", RegexOption.IGNORE_CASE).find(html)
    val body = bodyMatch?.groupValues?.get(1) ?: html

    // 移除 script/style/head
    var cleaned = Regex("""<script[\s\S]*?</script>""", RegexOption.IGNORE_CASE).replace(body, "")
    cleaned = Regex("""<style[\s\S]*?</style>""", RegexOption.IGNORE_CASE).replace(cleaned, "")
    cleaned = Regex("""<head[\s\S]*?</head>""", RegexOption.IGNORE_CASE).replace(cleaned, "")

    // <br> → 换行
    cleaned = Regex("""<br\s*/?>""", RegexOption.IGNORE_CASE).replace(cleaned, "\n")
    // <p>, <div> → 换行
    cleaned = Regex("""</?(p|div|h[1-6]|li|tr|blockquote)[^>]*>""", RegexOption.IGNORE_CASE).replace(cleaned, "\n")
    // 去除所有剩余标签
    cleaned = Regex("""<[^>]+>""").replace(cleaned, "")

    // 解码常见 HTML 实体
    cleaned = cleaned
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&ldquo;", "\u201C")
        .replace("&rdquo;", "\u201D")
        .replace("&hellip;", "\u2026")

    // 按句号、问号、感叹号换行，提升阅读体验
    cleaned = cleaned
        .replace("。", "。\n")
        .replace("！", "！\n")
        .replace("？", "？\n")

    // 清理多余空白
    return cleaned.lineSequence()
        .map { it.trim() }
        .filter { it.isNotEmpty() }
        .joinToString("\n")
        .trim()
}

/** 从 XHTML 中提取标题（h1/h2/title 标签） */
private fun extractTitleFromHtml(html: String): String? {
    // h1/h2
    val hTag = Regex("""<h[12][^>]*>([\s\S]*?)</h[12]>""", RegexOption.IGNORE_CASE).find(html)
    if (hTag != null) {
        val title = Regex("""<[^>]+>""").replace(hTag.groupValues[1], "").trim()
        if (title.isNotEmpty() && title.length <= 80) return title
    }
    // <title>
    val titleTag = Regex("""<title[^>]*>([\s\S]*?)</title>""", RegexOption.IGNORE_CASE).find(html)
    if (titleTag != null) {
        val title = titleTag.groupValues[1].trim()
        if (title.isNotEmpty() && title.length <= 80) return title
    }
    return null
}
