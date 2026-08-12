package com.tomato.downloader.ui

import android.content.Intent
import android.net.Uri
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Block
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.FileDownload
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.MenuBook
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.UploadFile
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import com.tomato.downloader.core.BookStore
import com.tomato.downloader.core.CustomWebStore
import com.tomato.downloader.core.PortStore
import com.tomato.downloader.ui.theme.TerminalSurface
import com.tomato.downloader.ui.theme.Green
import com.tomato.downloader.ui.theme.TerminalBg
import com.tomato.downloader.ui.theme.TerminalBorder

private const val AUTHOR_QQ = "898954782"

private fun openQQProfile(context: android.content.Context) {
    try {
        // 优先跳转 QQ 个人资料卡
        val profileUri = Uri.parse("mqqapi://card/show_pslcard?src_type=internal&version=1&uin=$AUTHOR_QQ&card_type=person&source=qrcode")
        context.startActivity(Intent(Intent.ACTION_VIEW, profileUri))
    } catch (e: Exception) {
        try {
            // 备用：跳转到 QQ 临时会话
            val chatUri = Uri.parse("mqqwpa://im/chat?chat_type=wpa&uin=$AUTHOR_QQ")
            context.startActivity(Intent(Intent.ACTION_VIEW, chatUri))
        } catch (e2: Exception) {
            try {
                // 最终备用：浏览器打开 QQ 空间主页
                val webUri = Uri.parse("https://user.qzone.qq.com/$AUTHOR_QQ")
                context.startActivity(Intent(Intent.ACTION_VIEW, webUri))
            } catch (e3: Exception) {
                Toast.makeText(context, "无法打开 QQ，请手动添加 QQ：$AUTHOR_QQ", Toast.LENGTH_SHORT).show()
            }
        }
    }
}

@Composable
fun AboutScreen(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val keyboardController = LocalSoftwareKeyboardController.current
    val version = remember(context) {
        try {
            context.packageManager.getPackageInfo(context.packageName, 0).versionName ?: ""
        } catch (e: Exception) { "" }
    }

    // —— 书籍预览：点击预览按钮后切到全屏阅读界面 ——
    var previewingBook by remember { mutableStateOf<BookStore.BookInfo?>(null) }

    // 预览界面是全屏覆盖的，独立于滚动内容
    if (previewingBook != null) {
        BookPreviewScreen(
            book = previewingBook!!,
            onBack = { previewingBook = null }
        )
        return
    }

    // —— 复制到…对话框与 SAF 系统文件选择器 ——
    //  点击复制图标 → 弹出选择对话框（快捷目录 + 自定义位置）
    //  → 选快捷目录：BookStore.copyToFile 直接写
    //  → 选自 SAF：CreateDocument() 启动系统文件选择器，拿到 Uri 后写
    var pendingCopyBook by remember { mutableStateOf<BookStore.BookInfo?>(null) }
    val copyLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.CreateDocument("*/*")
    ) { uri: Uri? ->
        val book = pendingCopyBook
        if (uri != null && book != null) {
            if (BookStore.copyToUri(context, book, uri)) {
                Toast.makeText(context, "已导出到你选择的位置", Toast.LENGTH_SHORT).show()
            } else {
                Toast.makeText(context, "导出失败：写入 Uri 异常", Toast.LENGTH_SHORT).show()
            }
        }
        pendingCopyBook = null
    }

    Column(
        modifier = modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp)
    ) {
        AboutCard("Web For Tomato", "版本 $version") {
            Text(
                "一个类似终端的安卓应用：点击「获取 Web」即运行安装脚本，" +
                    "将 Tomato-Novel-Downloader 的 Web 服务二进制下载到应用私有目录并赋予执行权限，" +
                    "随后以 --server 模式启动，在本机与局域网暴露 Web UI。",
                color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 13.sp, lineHeight = 20.sp
            )
        }

        // —— 自定义端口设置（持久性 + 恢复默认 + 提示下次启动生效）——
        val currentPort by PortStore.port.collectAsState()
        var portText by remember { mutableStateOf(currentPort.toString()) }
        // 弹窗开关：避免内联输入框与键盘距离远产生的大片空白
        var showPortDialog by remember { mutableStateOf(false) }

        // 当外部端口变化（例如恢复默认）时，同步更新显示
        LaunchedEffect(currentPort) {
            portText = currentPort.toString()
        }

        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = TerminalSurface),
            shape = RoundedCornerShape(14.dp)
        ) {
            Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("服务端口", color = MaterialTheme.colorScheme.onSurface, fontWeight = FontWeight.Bold, fontSize = 16.sp)
                    Spacer(Modifier.width(8.dp))
                    Text(
                        "默认 ${PortStore.DEFAULT_PORT} · 建议 1024-65535",
                        color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 11.sp
                    )
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    // 只读展示框：点击弹窗编辑
                    OutlinedCard(
                        onClick = { showPortDialog = true },
                        modifier = Modifier.weight(1f),
                        shape = RoundedCornerShape(10.dp),
                        colors = CardDefaults.outlinedCardColors(
                            containerColor = TerminalBg,
                            contentColor = MaterialTheme.colorScheme.onSurface
                        ),
                        border = BorderStroke(
                            1.dp,
                            if (PortStore.isDefault()) TerminalBorder else Green
                        )
                    ) {
                        Column(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp)) {
                            Text(
                                "自定义端口（点击编辑）",
                                color = Color(0xFF6E7681),
                                fontSize = 11.sp
                            )
                            Spacer(Modifier.height(2.dp))
                            Row(
                                Modifier.fillMaxWidth(),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.SpaceBetween
                            ) {
                                Text(
                                    portText,
                                    color = MaterialTheme.colorScheme.onSurface,
                                    fontSize = 17.sp,
                                    fontWeight = FontWeight.SemiBold,
                                    fontFamily = FontFamily.Monospace
                                )
                                Text(
                                    "编辑",
                                    color = Green,
                                    fontSize = 13.sp,
                                    fontWeight = FontWeight.SemiBold
                                )
                            }
                        }
                    }
                    Spacer(Modifier.width(8.dp))
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        // 恢复默认按钮：图标 + 文字
                        IconButton(
                            onClick = { PortStore.resetDefault(context) },
                            enabled = !PortStore.isDefault()
                        ) {
                            Icon(Icons.Outlined.Refresh, contentDescription = "恢复默认",
                                tint = if (PortStore.isDefault()) Color(0xFF6E7681) else Green)
                        }
                        Text(
                            "恢复默认",
                            fontSize = 10.sp,
                            color = if (PortStore.isDefault()) Color(0xFF6E7681) else Color(0xFF8B949E)
                        )
                    }
                }
                Text(
                    when {
                        PortStore.isDefault() -> "当前使用默认端口，建议保留"
                        else -> "当前使用自定义端口，重启服务生效"
                    },
                    fontSize = 11.sp,
                    color = if (PortStore.isDefault()) Color(0xFF8B949E) else Green
                )
            }
        }

        // —— 端口编辑弹窗：弹窗内输入端口，避免键盘在底部卡片下方产生大片空白 ——
        if (showPortDialog) {
            var dialogPort by remember(portText) { mutableStateOf(portText) }
            var dialogError by remember { mutableStateOf<String?>(null) }
            AlertDialog(
                containerColor = TerminalSurface,
                titleContentColor = MaterialTheme.colorScheme.onSurface,
                textContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                onDismissRequest = { showPortDialog = false },
                title = { Text("修改服务端口") },
                text = {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(
                            "端口范围 1024 ~ 65535，默认 ${PortStore.DEFAULT_PORT}。修改后需重启服务生效。",
                            fontSize = 12.sp
                        )
                        Spacer(Modifier.height(4.dp))
                        OutlinedTextField(
                            value = dialogPort,
                            onValueChange = { input ->
                                val filtered = input.filter { it.isDigit() }.take(5)
                                dialogPort = filtered
                                dialogError = when {
                                    filtered.isEmpty() -> "端口不能为空"
                                    else -> {
                                        val n = filtered.toIntOrNull()
                                        if (n != null && PortStore.isValidPort(n)) null
                                        else "端口范围必须为 1024 ~ 65535"
                                    }
                                }
                            },
                            modifier = Modifier.fillMaxWidth(),
                            singleLine = true,
                            isError = dialogError != null,
                            keyboardOptions = KeyboardOptions(
                                keyboardType = KeyboardType.Phone,
                                imeAction = ImeAction.Done
                            ),
                            keyboardActions = KeyboardActions(
                                onDone = { keyboardController?.hide() }
                            ),
                            label = { Text("自定义端口") },
                            colors = OutlinedTextFieldDefaults.colors(
                                focusedContainerColor = TerminalBg,
                                unfocusedContainerColor = TerminalBg,
                                focusedBorderColor = Green,
                                unfocusedBorderColor = TerminalBorder,
                                errorBorderColor = Color(0xFFFF6B6B),
                                errorTextColor = Color(0xFFFF6B6B)
                            ),
                            supportingText = {
                                if (dialogError != null) Text(dialogError!!, fontSize = 11.sp, color = Color(0xFFFF6B6B))
                            }
                        )
                    }
                },
                confirmButton = {
                    TextButton(
                        onClick = {
                            val n = dialogPort.toIntOrNull()
                            if (n == null || !PortStore.isValidPort(n)) {
                                dialogError = "端口范围必须为 1024 ~ 65535"
                                return@TextButton
                            }
                            val ok = PortStore.setPort(context, n)
                            if (ok) {
                                portText = n.toString()
                                Toast.makeText(context, "端口已保存为 $n（下次启动服务生效）", Toast.LENGTH_SHORT).show()
                            } else {
                                Toast.makeText(context, "保存失败：端口无效", Toast.LENGTH_SHORT).show()
                            }
                            showPortDialog = false
                        },
                        enabled = dialogError == null && dialogPort.isNotEmpty() && dialogPort.toIntOrNull() != currentPort
                    ) {
                        Text("保存", color = Green, fontWeight = FontWeight.SemiBold)
                    }
                },
                dismissButton = {
                    TextButton(onClick = { showPortDialog = false }) {
                        Text("取消", color = Color(0xFF8B949E))
                    }
                }
            )
        }

        // —— 自定义加载 Web：允许用户选择自己的 Web 二进制替代默认下载的 ——
        CustomWebCard()

        // —— 已下载书籍列表：Web 服务下载的书可以随时在这查看 / 刷新 / 删除 ——
        var books by remember { mutableStateOf<List<BookStore.BookInfo>>(emptyList()) }
        var scanning by remember { mutableStateOf(false) }
        fun rescan() {
            scanning = true
            android.os.Handler(android.os.Looper.getMainLooper()).post {
                val result = BookStore.scanBooks(context)
                books = result
                scanning = false
            }
        }
        LaunchedEffect(Unit) { rescan() }

        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = TerminalSurface),
            shape = RoundedCornerShape(14.dp)
        ) {
            Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                    modifier = Modifier.fillMaxWidth()
                ) {
                    Column {
                        Text(
                            "已下载书籍",
                            color = MaterialTheme.colorScheme.onSurface,
                            fontWeight = FontWeight.Bold,
                            fontSize = 16.sp
                        )
                        Text(
                            if (books.isEmpty()) "尚未扫描到书籍（点击「获取 Web」后，在 Web UI 中下载书籍）"
                            else "共 ${books.size} 本 · 点击刷新扫描最新",
                            color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 11.sp,
                            modifier = Modifier.padding(top = 2.dp)
                        )
                    }
                    // 刷新按钮（带扫描中禁用）
                    IconButton(
                        onClick = { rescan() },
                        enabled = !scanning
                    ) {
                        Icon(
                            Icons.Outlined.Refresh,
                            contentDescription = "刷新书籍列表",
                            tint = if (scanning) Color(0xFF6E7681) else Green
                        )
                    }
                }

                // 书籍列表 / 空态展示
                if (scanning) {
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .height(120.dp),
                        contentAlignment = Alignment.Center
                    ) {
                        Text("正在扫描本地书籍...", color = Color(0xFF8B949E), fontSize = 12.sp)
                    }
                } else if (books.isEmpty()) {
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .height(120.dp),
                        contentAlignment = Alignment.Center
                    ) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Text("📚", fontSize = 28.sp)
                            Spacer(Modifier.height(6.dp))
                            Text(
                                "还没有下载的书籍\n在 Web UI 里下载后回来刷新看看",
                                color = Color(0xFF8B949E),
                                fontSize = 12.sp,
                                lineHeight = 18.sp
                            )
                        }
                    }
                } else {
                    // 限定高度内嵌 LazyColumn（外层 Column 本身是 scrollable 的）
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .height(if (books.size <= 3) (books.size * 78).dp else 240.dp)
                    ) {
                        LazyColumn(
                            verticalArrangement = Arrangement.spacedBy(6.dp)
                        ) {
                            items(books, key = { it.path }) { book ->
                                BookRow(
                                    book = book,
                                    onPreview = { previewingBook = book },
                                    onCopyTo = { pendingCopyBook = book },
                                    onDelete = {
                                        if (BookStore.deleteBook(book)) {
                                            books = books - book
                                            Toast.makeText(context, "已删除 ${book.name}", Toast.LENGTH_SHORT).show()
                                        } else {
                                            Toast.makeText(context, "删除失败", Toast.LENGTH_SHORT).show()
                                        }
                                    }
                                )
                            }
                        }
                    }
                }
            }
        }

        // —— 「复制到…」目标选择对话框 ——
        if (pendingCopyBook != null) {
            val book = pendingCopyBook!!
            val quickDirs = remember(book) { BookStore.quickDestDirs(context) }
            AlertDialog(
                containerColor = TerminalSurface,
                titleContentColor = MaterialTheme.colorScheme.onSurface,
                textContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                onDismissRequest = { pendingCopyBook = null },
                title = { Text("复制《${book.name.ifBlank { "未命名" }}.${book.ext}》到…") },
                text = {
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text("选择目标位置", fontSize = 12.sp)
                        Spacer(Modifier.height(4.dp))
                        // 自定义位置（SAF：系统文件选择器，可选任意目录）
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    val base = book.name.ifBlank { "book" }
                                    val ext = book.ext.lowercase(java.util.Locale.ROOT)
                                    runCatching { copyLauncher.launch("$base.$ext") }
                                        .onFailure {
                                            Toast.makeText(context, "无法启动文件选择器", Toast.LENGTH_SHORT).show()
                                            pendingCopyBook = null
                                        }
                                }
                                .padding(vertical = 10.dp, horizontal = 6.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(10.dp)
                        ) {
                            Icon(
                                Icons.Outlined.Folder, null,
                                tint = Green, modifier = Modifier.size(20.dp)
                            )
                            Column(Modifier.weight(1f)) {
                                Text(
                                    "自定义位置（任意目录）",
                                    color = MaterialTheme.colorScheme.onSurface,
                                    fontSize = 13.sp,
                                    fontWeight = FontWeight.SemiBold
                                )
                                Text(
                                    "打开系统文件管理器，任选内部存储/SD 卡等位置",
                                    color = Color(0xFF8B949E), fontSize = 11.sp
                                )
                            }
                        }
                        // 快捷目录
                        quickDirs.forEach { (label, dir) ->
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable {
                                        val dest = BookStore.copyToFile(context, book, dir)
                                        if (dest != null) {
                                            Toast.makeText(context, "已复制到 ${dest.absolutePath}", Toast.LENGTH_LONG).show()
                                        } else {
                                            Toast.makeText(context, "复制失败，请换个目录或使用「自定义位置」", Toast.LENGTH_SHORT).show()
                                        }
                                        pendingCopyBook = null
                                    }
                                    .padding(vertical = 10.dp, horizontal = 6.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(10.dp)
                            ) {
                                Icon(
                                    Icons.Outlined.FileDownload, null,
                                    tint = Color(0xFF6E7681), modifier = Modifier.size(20.dp)
                                )
                                Column(Modifier.weight(1f)) {
                                    Text(
                                        label,
                                        color = MaterialTheme.colorScheme.onSurface,
                                        fontSize = 13.sp
                                    )
                                    Text(
                                        dir.absolutePath,
                                        color = Color(0xFF8B949E), fontSize = 10.sp,
                                        maxLines = 1,
                                        overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis
                                    )
                                }
                            }
                        }
                    }
                },
                confirmButton = {},
                dismissButton = {
                    TextButton(onClick = { pendingCopyBook = null }) {
                        Text("取消", color = Color(0xFF8B949E))
                    }
                }
            )
        }

        LinkCard("参考项目", "Tomato-Novel-Downloader", "https://github.com/zhongbai2333/Tomato-Novel-Downloader")
        AboutCard("作者", null) {
            Text(
                "林九思",
                color = MaterialTheme.colorScheme.primary,
                fontWeight = FontWeight.SemiBold,
                fontSize = 14.sp,
                modifier = Modifier.clickable { openQQProfile(context) }
            )
            Text(
                "点击跳转 QQ 主页 · QQ：$AUTHOR_QQ",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 11.sp,
                modifier = Modifier.padding(top = 2.dp)
            )
        }
        AboutCard("说明", null) {
            Text(
                "本软件旨在解决对于番茄解析困难的用户，如有侵权，联系QQ$AUTHOR_QQ。",
                color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 12.sp, lineHeight = 19.sp
            )
        }
    }
}

@Composable
private fun AboutCard(title: String, subtitle: String?, content: @Composable () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = TerminalSurface),
        shape = RoundedCornerShape(14.dp)
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(title, color = MaterialTheme.colorScheme.onSurface, fontWeight = FontWeight.Bold, fontSize = 16.sp)
            if (subtitle != null) Text(subtitle, color = MaterialTheme.colorScheme.primary, fontSize = 12.sp)
            content()
        }
    }
}

/** 单本书条目：书名 · 格式标签 · 大小/时间 · 复制到… · 删除按钮 */
@Composable
private fun BookRow(
    book: BookStore.BookInfo,
    onPreview: () -> Unit,
    onCopyTo: () -> Unit,
    onDelete: () -> Unit
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = TerminalBg),
        shape = RoundedCornerShape(10.dp),
        border = androidx.compose.foundation.BorderStroke(0.8.dp, TerminalBorder)
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 10.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            // 左：格式 Badge（TXT / EPUB / PDF...）
            Box(
                Modifier
                    .size(42.dp)
                    .background(
                        brush = androidx.compose.ui.graphics.Brush.verticalGradient(
                            0f to Color(0x3322C55E), 1f to Color(0x1A16A34F)
                        ),
                        shape = RoundedCornerShape(8.dp)
                    ),
                contentAlignment = Alignment.Center
            ) {
                Text(
                    text = book.ext.take(4),
                    color = Green,
                    fontWeight = FontWeight.Black,
                    fontSize = 11.sp
                )
            }
            // 中：书名 + 元信息
            Column(Modifier.weight(1f)) {
                Text(
                    text = book.name.ifBlank { "(未命名)" },
                    color = Color(0xFFE6EDF3),
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 13.sp,
                    maxLines = 1,
                    overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis
                )
                Spacer(Modifier.height(2.dp))
                Text(
                    text = "${book.sizeText} · ${book.dateText}",
                    color = Color(0xFF8B949E),
                    fontSize = 11.sp
                )
            }
            // 右：预览按钮（应用内直接阅读 txt / epub）
            val canPreview = book.ext.lowercase() in setOf("txt", "epub")
            IconButton(
                onClick = onPreview,
                enabled = canPreview,
                modifier = Modifier.size(34.dp)
            ) {
                Icon(
                    if (canPreview) Icons.Outlined.MenuBook else Icons.Outlined.Block,
                    contentDescription = "预览 ${book.name}",
                    tint = if (canPreview) Green else Color(0xFF6E7681),
                    modifier = Modifier.size(19.dp)
                )
            }
            // 右：复制到…按钮（把书复制到用户选的内部存储任意位置）
            IconButton(
                onClick = onCopyTo,
                modifier = Modifier.size(34.dp)
            ) {
                Icon(
                    Icons.Outlined.FileDownload,
                    contentDescription = "复制 ${book.name} 到其他位置",
                    tint = Green,
                    modifier = Modifier.size(19.dp)
                )
            }
            // 右：删除按钮
            IconButton(
                onClick = onDelete,
                modifier = Modifier.size(34.dp)
            ) {
                Icon(
                    Icons.Outlined.Delete,
                    contentDescription = "删除 ${book.name}",
                    tint = Color(0xFFFF6B6B),
                    modifier = Modifier.size(19.dp)
                )
            }
        }
    }
}

@Composable
private fun CustomWebCard() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val customEnabled by CustomWebStore.enabled.collectAsState()
    val customName by CustomWebStore.name.collectAsState()
    val serverState by com.tomato.downloader.core.ServerManager.state.collectAsState()
    var importing by remember { mutableStateOf(false) }

    // SAF 文件选择器：OpenDocument 支持 */* 任意文件
    val filePicker = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.OpenDocument()
    ) { uri: Uri? ->
        if (uri != null) {
            importing = true
            scope.launch {
                val ok = withContext(Dispatchers.IO) {
                    CustomWebStore.setCustomWeb(context, uri)
                }
                importing = false
                if (ok) {
                    Toast.makeText(context, "自定义 Web 已加载，重启服务生效", Toast.LENGTH_LONG).show()
                } else {
                    Toast.makeText(context, "加载失败：无法读取该文件", Toast.LENGTH_SHORT).show()
                }
            }
        }
    }

    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = TerminalSurface),
        shape = RoundedCornerShape(14.dp)
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween
            ) {
                Column(Modifier.weight(1f)) {
                    Text(
                        "自定义加载 Web",
                        color = MaterialTheme.colorScheme.onSurface,
                        fontWeight = FontWeight.Bold,
                        fontSize = 16.sp
                    )
                    Text(
                        "使用你自己的二进制替代默认下载的",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 11.sp
                    )
                }
                GlassSwitch(
                    checked = customEnabled,
                    onCheckedChange = { checked ->
                        if (checked) {
                            if (CustomWebStore.isCustomInstalled(context)) {
                                CustomWebStore.setEnabled(context, true)
                            } else {
                                runCatching { filePicker.launch(arrayOf("*/*")) }
                                    .onFailure {
                                        Toast.makeText(context, "无法启动文件选择器", Toast.LENGTH_SHORT).show()
                                    }
                            }
                        } else {
                            CustomWebStore.setEnabled(context, false)
                        }
                    }
                )
            }

            // 开关打开后：显示文件信息和操作按钮
            if (customEnabled) {
                // 文件名 / 加载中状态
                if (importing) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp)
                    ) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(18.dp),
                            strokeWidth = 2.dp,
                            color = Green
                        )
                        Text("正在加载…", color = Color(0xFF8B949E), fontSize = 12.sp)
                    }
                } else if (customName.isNotEmpty()) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        Icon(
                            Icons.Outlined.UploadFile, null,
                            tint = Green, modifier = Modifier.size(18.dp)
                        )
                        Column(Modifier.weight(1f)) {
                            Text("当前文件", color = Color(0xFF6E7681), fontSize = 10.sp)
                            Text(
                                customName,
                                color = MaterialTheme.colorScheme.onSurface,
                                fontSize = 13.sp,
                                fontWeight = FontWeight.Medium,
                                maxLines = 1,
                                overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis
                            )
                        }
                    }
                }

                // 操作按钮行
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp)
                ) {
                    // 更换文件
                    OutlinedCard(
                        onClick = {
                            runCatching { filePicker.launch(arrayOf("*/*")) }
                                .onFailure {
                                    Toast.makeText(context, "无法启动文件选择器", Toast.LENGTH_SHORT).show()
                                }
                        },
                        modifier = Modifier.weight(1f),
                        shape = RoundedCornerShape(10.dp),
                        colors = CardDefaults.outlinedCardColors(
                            containerColor = TerminalBg,
                            contentColor = Green
                        ),
                        border = BorderStroke(1.dp, Green)
                    ) {
                        Row(
                            modifier = Modifier.fillMaxWidth().padding(vertical = 10.dp, horizontal = 12.dp),
                            horizontalArrangement = Arrangement.Center,
                            verticalAlignment = Alignment.CenterVertically,
                            content = {
                                Icon(Icons.Outlined.Folder, null, modifier = Modifier.size(16.dp))
                                Spacer(Modifier.width(6.dp))
                                Text("选择文件", fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                            }
                        )
                    }
                    // 清除自定义
                    OutlinedCard(
                        onClick = {
                            CustomWebStore.clearCustomWeb(context)
                            Toast.makeText(context, "已清除自定义 Web，将使用默认二进制", Toast.LENGTH_SHORT).show()
                        },
                        modifier = Modifier.weight(1f),
                        shape = RoundedCornerShape(10.dp),
                        colors = CardDefaults.outlinedCardColors(
                            containerColor = TerminalBg,
                            contentColor = Color(0xFFFF6B6B)
                        ),
                        border = BorderStroke(1.dp, Color(0xFFFF6B6B))
                    ) {
                        Row(
                            modifier = Modifier.fillMaxWidth().padding(vertical = 10.dp, horizontal = 12.dp),
                            horizontalArrangement = Arrangement.Center,
                            verticalAlignment = Alignment.CenterVertically,
                            content = {
                                Icon(Icons.Outlined.Delete, null, modifier = Modifier.size(16.dp))
                                Spacer(Modifier.width(6.dp))
                                Text("清除", fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                            }
                        )
                    }
                }
            }

            // 提示
            Text(
                when {
                    customEnabled && serverState == com.tomato.downloader.core.ServerManager.ServerState.RUNNING ->
                        "自定义 Web 已启用，需停止并重新启动服务才能生效"
                    customEnabled -> "启用后将使用你选择的二进制启动服务"
                    else -> "关闭后使用默认从 GitHub 下载的二进制"
                },
                fontSize = 11.sp,
                color = if (customEnabled) Green else Color(0xFF8B949E)
            )
        }
    }
}

@Composable
private fun LinkCard(title: String, name: String, url: String) {
    val context = LocalContext.current
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = TerminalSurface),
        shape = RoundedCornerShape(14.dp)
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text(title, color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 12.sp)
            Text(name, color = MaterialTheme.colorScheme.onSurface, fontWeight = FontWeight.SemiBold, fontSize = 15.sp)
            Text(
                url,
                color = MaterialTheme.colorScheme.primary,
                fontSize = 12.sp,
                modifier = Modifier.fillMaxWidth().padding(top = 4.dp)
            )
        }
    }
}
