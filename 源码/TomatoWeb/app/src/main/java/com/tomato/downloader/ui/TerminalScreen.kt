package com.tomato.downloader.ui

import android.content.Intent
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CleaningServices
import androidx.compose.material.icons.outlined.Download
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material.icons.outlined.StopCircle
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import com.tomato.downloader.core.CustomWebStore
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import com.tomato.downloader.core.ServerManager
import com.tomato.downloader.service.WebServerService
import com.tomato.downloader.ui.theme.Amber
import com.tomato.downloader.ui.theme.Green
import com.tomato.downloader.ui.theme.Red
import com.tomato.downloader.ui.theme.TerminalBg
import com.tomato.downloader.ui.theme.TerminalBorder
import kotlinx.coroutines.launch

@Composable
fun TerminalScreen(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val state by ServerManager.state.collectAsState()
    // 直接使用 ServerManager 中统一管理的日志 StateFlow（清理后切页面也不会再回来）
    val logs by ServerManager.logLines.collectAsState()
    val listState = rememberLazyListState()

    LaunchedEffect(logs.size) {
        if (logs.isNotEmpty()) listState.animateScrollToItem(logs.lastIndex)
    }

    Column(modifier = modifier.fillMaxSize().padding(horizontal = 12.dp)) {
        StatusHeader(state)
        Surface(
            modifier = Modifier.fillMaxWidth().weight(1f).padding(top = 8.dp),
            color = TerminalBg,
            shape = RoundedCornerShape(12.dp),
            border = androidx.compose.foundation.BorderStroke(1.dp, TerminalBorder)
        ) {
            if (logs.isEmpty()) {
                Box(Modifier.fillMaxSize().padding(16.dp), contentAlignment = Alignment.Center) {
                    Text(
                        "点击下方「获取 Web」开始...\n将运行安装脚本并启动 Web 服务",
                        color = Color(0xFF8B949E),
                        fontFamily = FontFamily.Monospace,
                        lineHeight = 22.sp
                    )
                }
            } else {
                val clipboard = LocalClipboardManager.current
                LazyColumn(
                    state = listState,
                    modifier = Modifier.fillMaxSize().padding(12.dp),
                    verticalArrangement = Arrangement.spacedBy(2.dp)
                ) {
                    items(logs) { line ->
                        Text(
                            formatLog(line),
                            fontFamily = FontFamily.Monospace,
                            fontSize = 12.sp,
                            lineHeight = 16.sp,
                            modifier = Modifier.pointerInput(line) {
                                detectTapGestures(
                                    onLongPress = {
                                        clipboard.setText(AnnotatedString(line))
                                        Toast.makeText(context, "已复制该行", Toast.LENGTH_SHORT).show()
                                    }
                                )
                            }
                        )
                    }
                }
            }
        }
        // 自定义 Web 已启用时，拦截「获取 Web」并弹窗提示
        var showCustomWebBlockDialog by remember { mutableStateOf(false) }

        if (showCustomWebBlockDialog) {
            AlertDialog(
                onDismissRequest = { showCustomWebBlockDialog = false },
                title = { Text("提示") },
                text = { Text("您现在正在使用自定义Web，请先关闭自定义Web后再使用获取Web功能。") },
                confirmButton = {
                    TextButton(onClick = { showCustomWebBlockDialog = false }) {
                        Text("我知道了", color = Green)
                    }
                }
            )
        }

        TerminalActionBar(
            state = state,
            onGetWeb = {
                // 自定义 Web 开关已打开时，禁止使用「获取 Web」
                if (CustomWebStore.enabled.value) {
                    showCustomWebBlockDialog = true
                    return@TerminalActionBar
                }
                scope.launch {
                    // 每次点击都视为要更新：先停止旧服务 → 强制重新下载 → 启动新服务
                    if (ServerManager.state.value == ServerManager.ServerState.RUNNING) {
                        ServerManager.stop()
                        context.stopService(Intent(context, WebServerService::class.java))
                    }
                    // force=true：强制重新下载，不管是否已安装
                    ServerManager.install(context, force = true)
                    if (ServerManager.state.value != ServerManager.ServerState.ERROR) {
                        val i = Intent(context, WebServerService::class.java)
                            .setAction(WebServerService.ACTION_START)
                        ContextCompat.startForegroundService(context, i)
                    }
                }
            },
            onRedownload = {
                scope.launch { ServerManager.install(context, force = true) }
            },
            onStop = {
                ServerManager.stop()
                context.stopService(Intent(context, WebServerService::class.java))
            },
            // 真正清空 ServerManager 中的日志，切页面再回来也不会再出现
            onClear = { ServerManager.clearLogs() }
        )
    }
}

@Composable
private fun StatusHeader(state: ServerManager.ServerState) {
    val (dot, label) = when (state) {
        ServerManager.ServerState.IDLE -> Color(0xFF6B7280) to "就绪"
        ServerManager.ServerState.INSTALLING -> Amber to "安装中"
        ServerManager.ServerState.RUNNING -> Green to "运行中"
        ServerManager.ServerState.STOPPED -> Color(0xFF6B7280) to "已停止"
        ServerManager.ServerState.ERROR -> Red to "错误"
    }
    Row(
        modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
        verticalAlignment = Alignment.CenterVertically
    ) {
        Box(
            Modifier.size(10.dp).background(dot, CircleShape)
        )
        Text(
            "  $label",
            color = dot,
            fontFamily = FontFamily.Monospace,
            fontSize = 13.sp
        )
    }
}

@Composable
private fun TerminalActionBar(
    state: ServerManager.ServerState,
    onGetWeb: () -> Unit,
    onRedownload: () -> Unit,
    onStop: () -> Unit,
    onClear: () -> Unit
) {
    Surface(
        modifier = Modifier.fillMaxWidth().padding(top = 10.dp, bottom = 8.dp),
        color = Color.Transparent
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            when (state) {
                ServerManager.ServerState.RUNNING -> {
                    Button(
                        onClick = onStop,
                        colors = ButtonDefaults.buttonColors(containerColor = Red),
                        modifier = Modifier.weight(1f)
                    ) {
                        Icon(Icons.Outlined.StopCircle, null)
                        Text("  停止服务")
                    }
                }
                ServerManager.ServerState.INSTALLING -> {
                    Button(onClick = {}, enabled = false, modifier = Modifier.weight(1f)) {
                        CircularProgressIndicator(
                            Modifier.size(16.dp), strokeWidth = 2.dp, color = Color.White
                        )
                        Text("  安装中...")
                    }
                }
                else -> {
                    Button(
                        onClick = onGetWeb,
                        colors = ButtonDefaults.buttonColors(containerColor = GreenDark2()),
                        modifier = Modifier.weight(1f)
                    ) {
                        Icon(Icons.Outlined.PlayCircle, null)
                        Text("  获取 Web")
                    }
                }
            }
            OutlinedButton(onClick = onRedownload, enabled = state != ServerManager.ServerState.INSTALLING) {
                Icon(Icons.Outlined.Download, contentDescription = "重新下载")
            }
            OutlinedButton(onClick = onClear) {
                Icon(Icons.Outlined.CleaningServices, contentDescription = "清理日志")
            }
        }
    }
}

@Composable
private fun GreenDark2(): Color = Color(0xFF22C55E)

private fun formatLog(line: String): AnnotatedString = buildAnnotatedString {
    val tagColor: Color? = when {
        line.startsWith("[INFO]") -> Green
        line.startsWith("[WARN]") -> Amber
        line.startsWith("[ERR") -> Red
        else -> null
    }
    if (tagColor != null && line.contains(']')) {
        val tagEnd = line.indexOf(']') + 1
        withStyle(SpanStyle(color = tagColor)) { append(line.substring(0, tagEnd)) }
        append(line.substring(tagEnd))
    } else {
        append(line)
    }
}
