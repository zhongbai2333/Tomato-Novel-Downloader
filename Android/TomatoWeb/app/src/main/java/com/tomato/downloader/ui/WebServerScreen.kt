package com.tomato.downloader.ui

import android.content.Intent
import android.net.Uri
import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.HorizontalDivider
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.key
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.OpenInBrowser
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.StopCircle
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material.icons.outlined.Wifi
import androidx.compose.material.icons.outlined.WifiOff
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.foundation.BorderStroke
import androidx.compose.material3.OutlinedCard
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
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import com.tomato.downloader.core.PasswordStore
import com.tomato.downloader.core.PortStore
import com.tomato.downloader.core.ServerManager
import com.tomato.downloader.service.WebServerService
import com.tomato.downloader.ui.theme.Amber
import com.tomato.downloader.ui.theme.Green
import com.tomato.downloader.ui.theme.Red
import com.tomato.downloader.ui.theme.TerminalBg
import com.tomato.downloader.ui.theme.TerminalBorder
import com.tomato.downloader.ui.theme.TerminalSurface
import kotlinx.coroutines.launch

@Composable
fun WebServerScreen(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val keyboardController = LocalSoftwareKeyboardController.current
    val state by ServerManager.state.collectAsState()
    val localUrl by ServerManager.localUrl.collectAsState()
    val lanUrl by ServerManager.lanUrl.collectAsState()
    // 公网真实出口 IP（运营商分配，如 106.226.205.206）
    val publicIp by ServerManager.publicIp.collectAsState()
    val publicIpLoading by ServerManager.publicIpLoading.collectAsState()
    // 公网 IP 归属地 + 国旗 emoji（获取到后直接显示在 IP 下方，无需点击切换）
    val publicGeo by ServerManager.publicGeo.collectAsState()
    val publicGeoFlag by ServerManager.publicGeoFlag.collectAsState()
    val publicGeoLoading by ServerManager.publicGeoLoading.collectAsState()
    val port by PortStore.port.collectAsState()
    val savedPw by PasswordStore.password.collectAsState()
    val scope = rememberCoroutineScope()

    // 进入页面即刷新：局域网 IP + 公网真实 IP
    LaunchedEffect(Unit) {
        ServerManager.refreshLanUrl()
        ServerManager.refreshPublicIp()
    }

    // 公网访问地址：有IP时才显示完整URL（没有端口也不能访问）
    val publicUrl = if (publicIp.isNotEmpty()) "http://$publicIp:$port/" else ""

    var enabled by remember(savedPw) { mutableStateOf(savedPw != null) }
    var password by remember(savedPw) { mutableStateOf(savedPw ?: "") }
    var passwordVisible by remember { mutableStateOf(false) }
    // 密码编辑弹窗开关：避免键盘与卡片距离过远产生的空白
    var showPasswordDialog by remember { mutableStateOf(false) }

    Column(
        modifier = modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp)
    ) {
        // —— 未启动服务时：顶部单独展示「公网真实 IP」——
        // 启动服务后：隐藏该条，显示下方正常的 URL 卡片组（4 张完整地址卡片）
        if (state != ServerManager.ServerState.RUNNING) {
            Card(
                modifier = Modifier.fillMaxWidth(),
                colors = CardDefaults.cardColors(containerColor = TerminalSurface),
                shape = RoundedCornerShape(14.dp)
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp)
                ) {
                    Icon(
                        Icons.Outlined.Wifi,
                        contentDescription = null,
                        tint = Green,
                        modifier = Modifier.size(22.dp)
                    )
                    Column(Modifier.weight(1f)) {
                        Text(
                            "你的公网 IP（WAN）",
                            color = Color(0xFF8B949E),
                            fontSize = 11.sp
                        )
                        Spacer(Modifier.height(2.dp))
                        // 第 1 行：纯 IP（大字）
                        Text(
                            text = when {
                                publicIp.isEmpty() && publicIpLoading -> "正在获取…"
                                publicIp.isEmpty() -> "未获取，请点击右侧刷新"
                                else -> publicIp
                            },
                            color = if (publicIp.isNotEmpty()) MaterialTheme.colorScheme.onSurface else Color(0xFF6E7681),
                            fontFamily = FontFamily.Monospace,
                            fontSize = 16.sp,
                            fontWeight = FontWeight.SemiBold
                        )
                        // 第 2 行：归属地。解析中显示 loading，失败/无信息时提示可手动刷新
                        val geoHint = when {
                            publicGeo.isNotEmpty() -> {
                                buildString {
                                    if (publicGeoFlag.isNotEmpty()) {
                                        append(publicGeoFlag)
                                        append("  ")
                                    }
                                    append(publicGeo)
                                } to Green
                            }
                            publicIp.isNotEmpty() && publicGeoLoading -> "正在解析归属地…" to Color(0xFF8B949E)
                            publicIp.isNotEmpty() -> "归属地未获取，点击右侧刷新重试" to Color(0xFF8B949E)
                            else -> null to null
                        }
                        if (geoHint.first != null) {
                            Spacer(Modifier.height(2.dp))
                            Text(
                                text = geoHint.first!!,
                                color = geoHint.second!!,
                                fontSize = 12.sp,
                                fontWeight = FontWeight.Medium
                            )
                        }
                    }
                    ActionIcon(
                        Icons.Outlined.Refresh,
                        desc = "刷新公网IP",
                        compact = true,
                        enabled = !publicIpLoading,
                        onClick = { ServerManager.refreshPublicIp() }
                    )
                }
            }
        }

        StatusCard(state, port)

        // —— 访问地址：将 3 个地址紧凑组合进一张卡片，避免重复卡片 padding 造成大间隔 ——
        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = TerminalBg),
            shape = RoundedCornerShape(14.dp)
        ) {
            Column(
                modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
                verticalArrangement = Arrangement.spacedBy(0.dp)
            ) {
                UrlCard(
                    title = "本机访问（Local）",
                    url = localUrl,
                    available = state == ServerManager.ServerState.RUNNING,
                    compact = true
                )
                HorizontalDivider(
                    modifier = Modifier.padding(horizontal = 12.dp),
                    color = TerminalBorder.copy(alpha = 0.5f),
                    thickness = 0.8.dp
                )
                UrlCard(
                    title = "局域网访问（LAN）",
                    url = lanUrl,
                    available = state == ServerManager.ServerState.RUNNING && lanUrl.isNotEmpty(),
                    compact = true
                )
                HorizontalDivider(
                    modifier = Modifier.padding(horizontal = 12.dp),
                    color = TerminalBorder.copy(alpha = 0.5f),
                    thickness = 0.8.dp
                )
                // 监听地址（0.0.0.0:用户端口）—— 进程实际绑定的网络接口
                UrlCard(
                    title = "监听地址（Bind）",
                    url = "0.0.0.0:$port",
                    available = state == ServerManager.ServerState.RUNNING,
                    alwaysAllowCopy = true,
                    emptyHint = if (state == ServerManager.ServerState.RUNNING) null else "服务未启动，启动后将监听该地址",
                    compact = true
                )
            }
        }

        // 密码锁设置卡片
        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = TerminalSurface),
            shape = RoundedCornerShape(14.dp)
        ) {
            Column(
                Modifier.fillMaxWidth().padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp)
            ) {
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    // 修复：限制最大宽度，让副标题有足够空间但不要挤占开关
                    Column(Modifier.weight(1f).padding(end = 12.dp)) {
                        Text("访问密码", color = MaterialTheme.colorScheme.onSurface, fontWeight = FontWeight.SemiBold, fontSize = 15.sp)
                        Text(
                            // 修复：精简副标题文字，避免折成两行导致与 Switch 高度失衡
                            "启用后访问 Web 需输入密码，对应 Tomato 的 --password 参数",
                            color = Color(0xFF8B949E), fontSize = 11.sp, lineHeight = 15.sp,
                            maxLines = 1
                        )
                    }
                    GlassSwitch(
                        checked = enabled,
                        onCheckedChange = { newValue ->
                            if (!newValue && savedPw != null) {
                                scope.launch {
                                    PasswordStore.setPassword(context, null)
                                    password = ""
                                    Toast.makeText(context, "已关闭密码保护", Toast.LENGTH_SHORT).show()
                                    restartServerIfRunning(context, state)
                                }
                            }
                            enabled = newValue
                        }
                    )
                }

                if (enabled) {
                    // 只读展示框：点击弹窗输入，避免底部输入框与键盘距离远产生的大片空白
                    OutlinedCard(
                        onClick = { showPasswordDialog = true },
                        modifier = Modifier.fillMaxWidth(),
                        shape = RoundedCornerShape(10.dp),
                        colors = androidx.compose.material3.CardDefaults.outlinedCardColors(
                            containerColor = TerminalBg,
                            contentColor = MaterialTheme.colorScheme.onSurface
                        ),
                        border = BorderStroke(1.dp, Color(0xFF30363D))
                    ) {
                        Row(
                            Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 14.dp),
                            verticalAlignment = Alignment.CenterVertically
                        ) {
                            Column(Modifier.weight(1f)) {
                                Text(
                                    if (savedPw == null) "访问密码（未设置）" else "访问密码",
                                    color = Color(0xFF6E7681), fontSize = 12.sp
                                )
                                Spacer(Modifier.height(2.dp))
                                Text(
                                    if (password.isBlank()) "点击设置密码" else "•".repeat(password.length.coerceAtMost(12)),
                                    color = MaterialTheme.colorScheme.onSurface,
                                    fontSize = 15.sp,
                                    fontWeight = FontWeight.Medium
                                )
                            }
                            Text("编辑", color = Green, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                        }
                    }

                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(
                            onClick = {
                                if (password.isBlank()) {
                                    // 未填时引导进入弹窗
                                    showPasswordDialog = true
                                    return@Button
                                }
                                scope.launch {
                                    PasswordStore.setPassword(context, password)
                                    Toast.makeText(context, "密码已保存", Toast.LENGTH_SHORT).show()
                                    // 服务已启用时，修改密码后需重启生效
                                    restartServerIfRunning(context, state)
                                }
                            },
                            colors = ButtonDefaults.buttonColors(containerColor = Green),
                            modifier = Modifier.weight(1f)
                        ) { Text(if (savedPw == null) "保存密码" else "更新密码") }

                        if (savedPw != null) {
                            TextButton(
                                onClick = {
                                    scope.launch {
                                        PasswordStore.setPassword(context, null)
                                        password = ""
                                        enabled = false
                                        Toast.makeText(context, "已取消密码保护", Toast.LENGTH_SHORT).show()
                                        // 服务已启用时，修改密码后需重启生效
                                        restartServerIfRunning(context, state)
                                    }
                                }
                            ) { Text("清除", color = Red) }
                        }
                    }

                    if (savedPw != null) {
                        Text(
                            "✓ 当前已设置密码保护，启动服务时会自动注入",
                            color = Green, fontSize = 11.sp
                        )
                    } else if (enabled) {
                        Text(
                            "提示：点击上方编辑框设置密码，保存后才会真正启用保护",
                            color = Color(0xFF8B949E), fontSize = 11.sp
                        )
                    }
                }
            }
        }

        // —— 密码编辑弹窗：弹窗内部带 TextField，键盘紧贴输入框，不会产生空白 ——
        if (showPasswordDialog) {
            var dialogPw by remember(password) { mutableStateOf(password) }
            var dialogVisible by remember(passwordVisible) { mutableStateOf(passwordVisible) }
            AlertDialog(
                containerColor = TerminalSurface,
                titleContentColor = MaterialTheme.colorScheme.onSurface,
                textContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                onDismissRequest = { showPasswordDialog = false },
                title = { Text("设置访问密码") },
                text = {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(
                            if (savedPw == null) "设置密码后，访问 Web UI 需要输入密码。" else "修改密码，留空清除并保存后可取消保护。",
                            fontSize = 12.sp
                        )
                        Spacer(Modifier.height(4.dp))
                        key(dialogVisible) {
                            OutlinedTextField(
                                value = dialogPw,
                                onValueChange = { dialogPw = it },
                                modifier = Modifier.fillMaxWidth(),
                                placeholder = {
                                    Text(
                                        if (savedPw == null) "请输入访问密码" else "输入新密码进行修改",
                                        color = Color(0xFF6E7681)
                                    )
                                },
                                visualTransformation = if (dialogVisible) VisualTransformation.None else PasswordVisualTransformation(),
                                keyboardOptions = KeyboardOptions(
                                    keyboardType = if (dialogVisible) KeyboardType.Text else KeyboardType.Password,
                                    imeAction = ImeAction.Done
                                ),
                                keyboardActions = KeyboardActions(onDone = { keyboardController?.hide() }),
                                trailingIcon = {
                                    IconButton(onClick = {
                                        keyboardController?.hide()
                                        dialogVisible = !dialogVisible
                                    }) {
                                        Icon(
                                            if (dialogVisible) Icons.Outlined.Visibility else Icons.Outlined.VisibilityOff,
                                            contentDescription = if (dialogVisible) "隐藏密码" else "显示密码",
                                            tint = Color(0xFF8B949E)
                                        )
                                    }
                                },
                                singleLine = true,
                                colors = OutlinedTextFieldDefaults.colors(
                                    focusedContainerColor = TerminalBg,
                                    unfocusedContainerColor = TerminalBg,
                                    focusedBorderColor = Green,
                                    unfocusedBorderColor = Color(0xFF30363D)
                                ),
                                shape = RoundedCornerShape(10.dp)
                            )
                        }
                    }
                },
                confirmButton = {
                    TextButton(onClick = {
                        if (dialogPw.isBlank() && savedPw != null) {
                            // 留空且已有密码：视为清除
                            scope.launch {
                                PasswordStore.setPassword(context, null)
                                password = ""
                                enabled = false
                                Toast.makeText(context, "已取消密码保护", Toast.LENGTH_SHORT).show()
                                // 服务已启用时，修改密码后需重启生效
                                restartServerIfRunning(context, state)
                            }
                            showPasswordDialog = false
                            return@TextButton
                        }
                        if (dialogPw.isBlank()) {
                            Toast.makeText(context, "密码不能为空", Toast.LENGTH_SHORT).show()
                            return@TextButton
                        }
                        scope.launch {
                            PasswordStore.setPassword(context, dialogPw)
                            password = dialogPw
                            Toast.makeText(context, "密码已保存", Toast.LENGTH_SHORT).show()
                            // 服务已启用时，修改密码后需重启生效
                            restartServerIfRunning(context, state)
                        }
                        showPasswordDialog = false
                    }) { Text("保存", color = Green, fontWeight = FontWeight.SemiBold) }
                },
                dismissButton = {
                    TextButton(onClick = { showPasswordDialog = false }) {
                        Text("取消", color = Color(0xFF8B949E))
                    }
                }
            )
        }

        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (state == ServerManager.ServerState.RUNNING) {
                Button(
                    onClick = {
                        ServerManager.stop()
                        context.stopService(Intent(context, WebServerService::class.java))
                    },
                    colors = ButtonDefaults.buttonColors(containerColor = Red),
                    modifier = Modifier.weight(1f)
                ) {
                    Icon(Icons.Outlined.StopCircle, null); Text("  停止服务")
                }
            } else {
                Button(
                    onClick = {
                        // Bug1 修复：UI 层先检查二进制是否存在，不存在直接提示用户不启动服务
                        if (!ServerManager.isInstalled(context)) {
                            Toast.makeText(context, "请先点击「获取 Web」下载二进制后再启动服务", Toast.LENGTH_LONG).show()
                            return@Button
                        }
                        // 启用自定义 Web 时，点击「启动服务」即提示用户当前使用的文件
                        if (ServerManager.isUsingCustomWeb(context)) {
                            val fileName = com.tomato.downloader.core.CustomWebStore.name.value
                                .ifBlank { "未知" }
                            Toast.makeText(
                                context,
                                "您现在使用的是自定义Web，Web文件名为: $fileName",
                                Toast.LENGTH_LONG
                            ).show()
                        }
                        val i = Intent(context, WebServerService::class.java)
                            .setAction(WebServerService.ACTION_START)
                        ContextCompat.startForegroundService(context, i)
                    },
                    colors = ButtonDefaults.buttonColors(containerColor = Color(0xFF22C55E)),
                    modifier = Modifier.weight(1f),
                    enabled = state != ServerManager.ServerState.INSTALLING
                ) {
                    Icon(Icons.Outlined.PlayCircle, null); Text("  启动服务")
                }
            }
        }
        Spacer(Modifier.height(4.dp))
    }
}

/**
 * 若服务正在运行，关闭并再次启动，用于修改密码/端口后让新参数立即生效。
 * 关闭 → 稍等 300ms → 再启动，确保进程与端口完全释放。
 */
private suspend fun restartServerIfRunning(
    context: android.content.Context,
    state: ServerManager.ServerState
) {
    if (state != ServerManager.ServerState.RUNNING) return
    ServerManager.stop()
    context.stopService(android.content.Intent(context, com.tomato.downloader.service.WebServerService::class.java))
    // 稍等释放端口和资源
    kotlinx.coroutines.delay(300)
    val i = android.content.Intent(context, com.tomato.downloader.service.WebServerService::class.java)
        .setAction(com.tomato.downloader.service.WebServerService.ACTION_START)
    androidx.core.content.ContextCompat.startForegroundService(context, i)
}

@Composable
private fun StatusCard(state: ServerManager.ServerState, port: Int) {
    val (color, text) = when (state) {
        ServerManager.ServerState.RUNNING -> Green to "Web 服务运行中"
        ServerManager.ServerState.INSTALLING -> Amber to "正在安装..."
        ServerManager.ServerState.ERROR -> Red to "服务异常"
        else -> Color(0xFF6B7280) to "服务未启动"
    }
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = TerminalSurface),
        shape = RoundedCornerShape(14.dp)
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    if (state == ServerManager.ServerState.RUNNING) Icons.Outlined.Wifi else Icons.Outlined.WifiOff,
                    contentDescription = null, tint = color, modifier = Modifier.size(24.dp)
                )
                Text("  $text", color = color, fontWeight = FontWeight.SemiBold, fontSize = 15.sp)
            }
            Text("端口 $port", color = Color(0xFF8B949E), fontFamily = FontFamily.Monospace, fontSize = 12.sp)
        }
    }
}

@Composable
private fun UrlCard(
    title: String,
    url: String,
    available: Boolean,
    emptyHint: String? = null,
    onRefresh: (() -> Unit)? = null,
    alwaysAllowCopy: Boolean = false,
    compact: Boolean = false,
    loading: Boolean = false
) {
    val context = LocalContext.current
    // showContent：允许显示地址/操作按钮
    //   - available=true && url 非空 → 正常显示
    //   - alwaysAllowCopy=true && url 非空 → 即使服务未启动也显示（如监听地址）
    val showContent = url.isNotEmpty() && (available || alwaysAllowCopy)
    // compact=true：外层已包大 Card，此处不再重复嵌套 Card，仅用 Column + 小 padding，大幅减少间隔
    val innerModifier = if (compact)
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp)
    else
        Modifier.fillMaxWidth().padding(14.dp)
    val innerSpacing = if (compact) 6.dp else 8.dp
    val titleSize = if (compact) 11.sp else 12.sp
    val urlSize = if (compact) 13.sp else 15.sp

    @Composable
    fun UrlContent() {
        Column(innerModifier, verticalArrangement = Arrangement.spacedBy(innerSpacing)) {
            Text(title, color = Color(0xFF8B949E), fontSize = titleSize, fontFamily = FontFamily.Monospace)
            if (showContent) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp)
                ) {
                    Text(
                        url,
                        color = if (available) Green else Color(0xFFC9D1D9),
                        fontFamily = FontFamily.Monospace,
                        fontSize = urlSize,
                        fontWeight = FontWeight.Medium,
                        modifier = Modifier.weight(1f)
                    )
                    // 刷新按钮：有 onRefresh 时始终放在 URL 旁边，服务未启动时也能刷新 IP
                    if (onRefresh != null) {
                        ActionIcon(
                            Icons.Outlined.Refresh,
                            "刷新",
                            compact = compact,
                            enabled = !loading,
                            onClick = onRefresh
                        )
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy((-4).dp)) {
                    val clipboard = LocalClipboardManager.current
                    ActionIcon(Icons.Outlined.ContentCopy, "复制", compact = compact) {
                        clipboard.setText(AnnotatedString(url))
                        Toast.makeText(context, "已复制", Toast.LENGTH_SHORT).show()
                    }
                    // 「打开」仅在 available 时提供（监听地址 0.0.0.0 浏览器不能直接访问）
                    if (available) {
                        ActionIcon(Icons.Outlined.OpenInBrowser, "打开", compact = compact) {
                            try {
                                context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
                            } catch (e: Exception) {
                                Toast.makeText(context, "没有可打开的浏览器", Toast.LENGTH_SHORT).show()
                            }
                        }
                        ActionIcon(Icons.Outlined.Share, "分享", compact = compact) {
                            val send = Intent(Intent.ACTION_SEND).apply {
                                type = "text/plain"; putExtra(Intent.EXTRA_TEXT, url)
                            }
                            context.startActivity(Intent.createChooser(send, "分享地址"))
                        }
                    }
                }
            } else {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween
                ) {
                    Text(
                        emptyHint ?: "服务未运行",
                        color = Color(0xFF6B7280),
                        fontFamily = FontFamily.Monospace,
                        fontSize = if (compact) 12.sp else 13.sp,
                        modifier = Modifier.weight(1f)
                    )
                    if (onRefresh != null) {
                        OutlinedButton(
                            onClick = onRefresh,
                            enabled = !loading,
                            modifier = if (compact) Modifier.height(32.dp) else Modifier
                        ) {
                            Icon(
                                Icons.Outlined.Refresh, null,
                                modifier = if (compact) Modifier.size(16.dp) else Modifier
                            )
                            Text(
                                "  刷新 IP",
                                fontSize = if (compact) 11.sp else 14.sp
                            )
                        }
                    }
                }
            }
        }
    }

    if (compact) {
        UrlContent()
    } else {
        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = TerminalBg),
            shape = RoundedCornerShape(14.dp)
        ) { UrlContent() }
    }
}

@Composable
private fun ActionIcon(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    desc: String,
    compact: Boolean = false,
    enabled: Boolean = true,
    onClick: () -> Unit
) {
    val btnSize = if (compact) 32.dp else 40.dp
    val iconSize = if (compact) 18.dp else 22.dp
    IconButton(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.size(btnSize)
    ) {
        Icon(
            icon,
            contentDescription = desc,
            tint = if (enabled) Color(0xFFC9D1D9) else Color(0xFF6E7681),
            modifier = Modifier.size(iconSize)
        )
    }
}
