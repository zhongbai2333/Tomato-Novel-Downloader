package com.tomato.downloader

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Scaffold
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import com.tomato.downloader.core.ConsentStore
import kotlinx.coroutines.delay
import com.tomato.downloader.ui.AboutScreen
import com.tomato.downloader.ui.FloatingGlassNav
import com.tomato.downloader.ui.Tab
import com.tomato.downloader.ui.TerminalScreen
import com.tomato.downloader.ui.WebServerScreen
import androidx.compose.material3.MaterialTheme
import com.tomato.downloader.ui.theme.Green
import com.tomato.downloader.ui.theme.TerminalBg
import com.tomato.downloader.ui.theme.TerminalSurface
import com.tomato.downloader.ui.theme.TomatoWebTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // 确保 IME（软键盘）弹出时内容区域真的被 resize，
        // 并让 Compose 能消费 WindowInsets(ime)。否则 Compose 页面底部输入框会被键盘盖住。
        WindowCompat.setDecorFitsSystemWindows(window, false)
        enableEdgeToEdge()
        setContent {
            // 同步：要求 window 立即处理 IME inset（部分 ROM 默认行为与 adjustResize 冲突）
            val view = LocalView.current
            androidx.compose.runtime.DisposableEffect(view) {
                ViewCompat.requestApplyInsets(view)
                onDispose {}
            }
            TomatoWebTheme {
                App()
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun App() {
    val context = LocalContext.current
    val activity = context as? android.app.Activity

    // —— 知情协议对话框：未同意时强制弹出，倒计时 5s 后才能点"同意"；"拒绝"直接退出 ——
    val consentAgreed by ConsentStore.agreed.collectAsState()
    var showDialog by remember { mutableStateOf(!consentAgreed) }
    // 倒计时：从 5 递减到 0，到 0 之前"同意"按钮禁用
    var agreeCountdown by remember { mutableIntStateOf(5) }
    LaunchedEffect(Unit) {
        while (agreeCountdown > 0) {
            delay(1000L)
            agreeCountdown -= 1
        }
    }
    // 状态同步：从磁盘恢复后若用户后来又清空（极端情况）也保持正确
    LaunchedEffect(consentAgreed) {
        if (consentAgreed) showDialog = false
    }

    if (showDialog && !consentAgreed) {
        AlertDialog(
            containerColor = TerminalSurface,
            titleContentColor = MaterialTheme.colorScheme.onSurface,
            textContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
            onDismissRequest = { /* 禁止点击外部/返回关闭 */ },
            title = {
                androidx.compose.material3.Text(
                    ConsentStore.DIALOG_TITLE,
                    fontWeight = FontWeight.Bold
                )
            },
            text = {
                // 协议正文较长，支持滚动 + 长按选中复制
                SelectionContainer {
                    androidx.compose.material3.Text(
                        text = ConsentStore.DIALOG_BODY,
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(420.dp)
                            .verticalScroll(rememberScrollState()),
                        fontSize = 12.5.sp,
                        lineHeight = 20.sp
                    )
                }
            },
            dismissButton = {
                TextButton(
                    onClick = {
                        // 拒绝：直接关闭应用
                        activity?.finishAffinity() ?: activity?.finish()
                        Runtime.getRuntime().exit(0)
                    }
                ) {
                    androidx.compose.material3.Text("拒绝并退出", color = Color(0xFFFF6B6B))
                }
            },
            confirmButton = {
                TextButton(
                    enabled = agreeCountdown <= 0,
                    onClick = {
                        ConsentStore.markAgreed(context)
                        showDialog = false
                    }
                ) {
                    androidx.compose.material3.Text(
                        text = if (agreeCountdown > 0) "同意并继续 (${agreeCountdown}s)" else "同意并继续",
                        color = if (agreeCountdown > 0) Color(0xFF8B949E) else Green,
                        fontWeight = FontWeight.SemiBold
                    )
                }
            }
        )
    }

    var tab by remember { mutableStateOf(Tab.Terminal) }

    // 键盘弹出时的额外空间：取「IME 高度」和「悬浮导航高度」的较大者，
    // 保证键盘不会盖住输入框，同时键盘收起时导航栏空间不被侵占
    val imeBottom = WindowInsets.ime.asPaddingValues().calculateBottomPadding()
    val navReserve = 92.dp
    val bottomInset = if (imeBottom > navReserve) imeBottom else navReserve

    // 顶层 Box：内容铺满全屏，底栏悬浮叠加在底部（液态玻璃效果 + 浮起）
    Box(Modifier.fillMaxSize().background(TerminalBg)) {

        // 内容：TopAppBar + 页面内容（.imePadding 让键盘弹出时整体往上顶）
        Scaffold(
            modifier = Modifier.fillMaxSize().imePadding(),
            containerColor = Color.Transparent,
            contentWindowInsets = WindowInsets(0, 0, 0, 0),
            topBar = {
                TopAppBar(
                    title = { androidx.compose.material3.Text("Web For Tomato") },
                    colors = TopAppBarDefaults.topAppBarColors(
                        containerColor = Color.Transparent,
                        titleContentColor = Color(0xFFE6EDF3)
                    ),
                    windowInsets = WindowInsets.systemBars.union(WindowInsets.displayCutout)
                )
            }
        ) { innerPadding ->
            Box(
                Modifier
                    .fillMaxSize()
                    .padding(innerPadding)
                    .padding(bottom = bottomInset)
            ) {
                when (tab) {
                    Tab.Terminal -> TerminalScreen(Modifier.fillMaxSize())
                    Tab.Web -> WebServerScreen(Modifier.fillMaxSize())
                    Tab.About -> AboutScreen(Modifier.fillMaxSize())
                }
            }
        }

        // 悬浮液态玻璃导航栏（叠加在内容之上，自动适配系统导航条高度）
        FloatingGlassNav(
            current = tab,
            onSelect = { tab = it },
            modifier = Modifier
                .fillMaxWidth()
                .align(Alignment.BottomCenter)
        )
    }
}
