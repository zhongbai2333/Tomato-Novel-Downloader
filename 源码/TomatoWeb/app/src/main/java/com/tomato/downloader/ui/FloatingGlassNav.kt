package com.tomato.downloader.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Code
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.Public
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RadialGradientShader
import androidx.compose.ui.graphics.Shader
import androidx.compose.ui.graphics.ShaderBrush
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.util.fastCoerceIn
import androidx.compose.ui.util.fastRoundToInt
import androidx.compose.ui.util.lerp
import com.tomato.downloader.ui.theme.Green
import kotlinx.coroutines.launch
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.changedToUp
import androidx.compose.ui.input.pointer.isOutOfBounds
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sqrt

@Composable
fun FloatingGlassNav(
    current: Tab,
    onSelect: (Tab) -> Unit,
    modifier: Modifier = Modifier
) {
    val tabs = Tab.entries
    val density = LocalDensity.current
    val scope = rememberCoroutineScope()

    val currentTab by rememberUpdatedState(current)
    val onSelectUpdated by rememberUpdatedState(onSelect)

    var totalWidthPx by remember { mutableFloatStateOf(0f) }
    var tabWidthPx by remember { mutableFloatStateOf(0f) }

    val selectedIndex = current.ordinal
    val indexAnim = remember { Animatable(selectedIndex.toFloat(), visibilityThreshold = 0.001f) }
    LaunchedEffect(selectedIndex) {
        // 点击切换时 indexAnim 过渡，此时 refraction 会是 false → 正常过渡
        indexAnim.animateTo(selectedIndex.toFloat(), spring(1f, 1000f, 0.001f))
    }

    val pressProgress = remember { Animatable(0f, visibilityThreshold = 0.001f) }
    val panelOffset = remember { Animatable(0f, visibilityThreshold = 0.5f) }

    var fingerPosition by remember { mutableStateOf<Offset?>(null) }
    // 是否处于真实拖拽（超过 touchSlop）：只有 true 时才启用折射放大
    var refractionEnabled by remember { mutableStateOf(false) }

    val horizontalPadPx = with(density) { 12.dp.toPx() }

    Box(
        modifier
            .fillMaxWidth()
            .windowInsetsPadding(WindowInsets.navigationBars)
            .padding(horizontal = 16.dp, vertical = 12.dp)
    ) {

        val cornerRadiusDp = 28.dp

        val press = pressProgress.value
        val velocity = panelOffset.value * 0.01f

        Box(
            Modifier
                .fillMaxWidth()
                .onGloballyPositioned { coords ->
                    totalWidthPx = coords.size.width.toFloat()
                    val totalPadPx = horizontalPadPx * 2
                    tabWidthPx = if (tabs.size > 0) (totalWidthPx - totalPadPx) / tabs.size else 0f
                }
                .graphicsLayer {
                    translationX = panelOffset.value
                    val s = lerp(1f, 1.025f, press)
                    scaleX = s + (velocity * -0.05f).fastCoerceIn(-0.03f, 0.03f)
                    scaleY = s + (velocity * 0.02f).fastCoerceIn(-0.02f, 0.02f)
                }
                .shadow(
                    elevation = lerp(4f, 7f, press).dp,
                    shape = RoundedCornerShape(cornerRadiusDp),
                    clip = false,
                    ambientColor = Color.Black.copy(alpha = 0.25f),
                    spotColor = Color.Black.copy(alpha = 0.20f)
                )
                .clip(RoundedCornerShape(cornerRadiusDp))
                .drawWithContent {
                    val w = size.width
                    val h = size.height
                    val cx = w / 2f
                    val cy = h / 2f
                    val refractR = max(w * 0.5f, h * 0.5f)

                    // 1) 玻璃主体
                    drawRect(
                        brush = Brush.verticalGradient(
                            0.00f to Color(0x992A3341),
                            0.50f to Color(0x8F1C2430),
                            1.00f to Color(0x85111820),
                            startY = 0f, endY = h
                        )
                    )

                    // 2) 中心放大区微亮
                    run {
                        val shader: Shader = RadialGradientShader(
                            center = Offset(cx, cy),
                            radius = refractR * 0.7f,
                            colors = listOf(
                                Color.White.copy(alpha = 0.08f),
                                Color.White.copy(alpha = 0.03f),
                                Color.Transparent
                            ),
                            colorStops = listOf(0f, 0.6f, 1f),
                            tileMode = TileMode.Clamp
                        )
                        drawRect(ShaderBrush(shader), blendMode = BlendMode.Plus)
                    }

                    // 3) 顶部内阴影
                    drawRect(
                        brush = Brush.verticalGradient(
                            0.00f to Color.Black.copy(alpha = 0.18f),
                            0.08f to Color.Black.copy(alpha = 0.10f),
                            0.25f to Color.Black.copy(alpha = 0.02f),
                            0.40f to Color.Black.copy(alpha = 0f)
                        )
                    )

                    // 4) 白色边框
                    drawRoundRect(
                        brush = Brush.verticalGradient(
                            0.00f to Color(0x99FFFFFF),
                            0.15f to Color(0x44FFFFFF),
                            0.50f to Color(0x0AFFFFFF),
                            1.00f to Color(0x00FFFFFF)
                        ),
                        style = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.2.dp.toPx()),
                        cornerRadius = androidx.compose.ui.geometry.CornerRadius(cornerRadiusDp.toPx(), cornerRadiusDp.toPx())
                    )

                    // 5) 按压变亮
                    if (press > 0f) {
                        drawRect(Color.White.copy(alpha = 0.04f * press), blendMode = BlendMode.Plus)
                    }

                    // 6) 手指径向光晕
                    fingerPosition?.let { fp ->
                        val fx = fp.x.fastCoerceIn(0f, w)
                        val fy = fp.y.fastCoerceIn(0f, h)
                        val shader: Shader = RadialGradientShader(
                            center = Offset(fx, fy),
                            radius = minOf(w, h) * 1.0f,
                            colors = listOf(
                                Color.White.copy(alpha = 0.10f * (0.5f + press * 0.5f)),
                                Color.White.copy(alpha = 0f)
                            ),
                            colorStops = listOf(0f, 1f),
                            tileMode = TileMode.Clamp
                        )
                        drawRect(ShaderBrush(shader), blendMode = BlendMode.Plus)
                    }

                    drawContent()
                },
            contentAlignment = Alignment.CenterStart
        ) {
            val rowWidth = totalWidthPx
            // 玻璃（指示器）中心
            val glassCenterX = indexAnim.value * tabWidthPx + tabWidthPx * 0.5f + horizontalPadPx
            // 半 Tab 宽度 = 从玻璃中心到玻璃边缘的距离
            val halfTab = tabWidthPx * 0.5f
            // 折射影响范围：玻璃外再扩展 0.8 个 Tab 宽度
            val refractionRangeOuter = tabWidthPx * 0.80f
            // 折射强度：只有拖拽时才启用，点击切换 Tab 时为 0（正常过渡）
            val lensStrength = if (refractionEnabled) (0.18f + press * 0.10f) else 0f

            // ========== 第一层：选中指示器胶囊（绘制在 Tab 下方）==========
            if (tabWidthPx > 0f) {
                val indicatorOffsetPx = indexAnim.value * tabWidthPx
                val indicatorWidthDp = with(density) { tabWidthPx.toDp() }
                val indCorner = 22.dp

                Box(
                    Modifier
                        .padding(start = 12.dp, top = 6.dp, bottom = 6.dp)
                        .graphicsLayer {
                            translationX = indicatorOffsetPx
                            val extraScale = lerp(0f, 0.04f, press)
                            scaleX = (1f + extraScale - (velocity * 0.75f).fastCoerceIn(-0.06f, 0.06f))
                            scaleY = (1f + extraScale + (velocity * 0.25f).fastCoerceIn(-0.06f, 0.06f))
                        }
                        .width(indicatorWidthDp)
                        .height(52.dp)
                        .clip(RoundedCornerShape(indCorner))
                        .drawWithContent {
                            val iw = size.width
                            val ih = size.height
                            val icx = iw / 2f
                            val icy = ih / 2f
                            val iRefractR = max(iw * 0.5f, ih * 0.5f)

                            drawRect(
                                brush = Brush.verticalGradient(
                                    0f to Color(0x2A22C55E),
                                    0.5f to Color(0x2018B157),
                                    1f to Color(0x1816A34F)
                                )
                            )

                            run {
                                val shader: Shader = RadialGradientShader(
                                    center = Offset(icx, icy),
                                    radius = iRefractR * 0.65f,
                                    colors = listOf(
                                        Color.White.copy(alpha = 0.10f),
                                        Color.White.copy(alpha = 0.04f),
                                        Color.Transparent
                                    ),
                                    colorStops = listOf(0f, 0.6f, 1f),
                                    tileMode = TileMode.Clamp
                                )
                                drawRect(ShaderBrush(shader), blendMode = BlendMode.Plus)
                            }

                            drawRect(
                                brush = Brush.verticalGradient(
                                    0.00f to Color.Black.copy(alpha = 0.15f),
                                    0.08f to Color.Black.copy(alpha = 0.08f),
                                    0.25f to Color.Black.copy(alpha = 0.02f),
                                    0.40f to Color.Black.copy(alpha = 0f)
                                )
                            )

                            drawRoundRect(
                                brush = Brush.verticalGradient(
                                    0.00f to Color(0xAAFFFFFF),
                                    0.15f to Color(0x55FFFFFF),
                                    0.50f to Color(0x0AFFFFFF),
                                    1.00f to Color(0x00FFFFFF)
                                ),
                                style = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.1.dp.toPx()),
                                cornerRadius = androidx.compose.ui.geometry.CornerRadius(indCorner.toPx(), indCorner.toPx())
                            )

                            if (press > 0f) {
                                drawRect(
                                    Color(0x3322C55E).copy(alpha = press * 0.20f),
                                    blendMode = BlendMode.Plus
                                )
                            }
                        }
                )
            }

            // ========== 第二层：Row (Tab 图标文字)，绘制在指示器之上 ==========
            Row(
                Modifier
                    .clearAndSetSemantics {}
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 6.dp)
                    .pointerInput(Unit) {
                        inspectDragGestures(
                            onDragStart = { down ->
                                refractionEnabled = true
                                scope.launch { pressProgress.animateTo(1f, spring(1f, 1000f, 0.001f)) }
                                fingerPosition = down.position
                            },
                            onDragEnd = { up, wasDragging ->
                                if (wasDragging) {
                                    refractionEnabled = true
                                    fingerPosition = up.position
                                    val targetIndex = indexAnim.value.fastRoundToInt().coerceIn(0, tabs.size - 1)
                                    scope.launch {
                                        // 先完成吸附动画
                                        val snap = launch {
                                            indexAnim.animateTo(targetIndex.toFloat(), spring(1f, 1000f, 0.001f))
                                        }
                                        launch { panelOffset.animateTo(0f, spring(1f, 300f, 0.5f)) }
                                        launch { pressProgress.animateTo(0f, spring(1f, 1000f, 0.001f)) }
                                        // 吸附完成后关闭折射
                                        snap.join()
                                        refractionEnabled = false
                                    }
                                    if (targetIndex != currentTab.ordinal) onSelectUpdated(tabs[targetIndex])
                                } else {
                                    // 纯点击：refractionEnabled 保持 false（无折射）
                                    scope.launch { pressProgress.animateTo(0f, spring(1f, 1000f, 0.001f)) }
                                }
                                fingerPosition = null
                            },
                            onDragCancel = { wasDragging ->
                                if (wasDragging) {
                                    refractionEnabled = true
                                    val targetIndex = indexAnim.value.fastRoundToInt().coerceIn(0, tabs.size - 1)
                                    scope.launch {
                                        val snap = launch {
                                            indexAnim.animateTo(targetIndex.toFloat(), spring(1f, 1000f, 0.001f))
                                        }
                                        launch { panelOffset.animateTo(0f, spring(1f, 300f, 0.5f)) }
                                        launch { pressProgress.animateTo(0f, spring(1f, 1000f, 0.001f)) }
                                        snap.join()
                                        refractionEnabled = false
                                    }
                                    if (targetIndex != currentTab.ordinal) onSelectUpdated(tabs[targetIndex])
                                }
                                fingerPosition = null
                            },
                            onDrag = { change, dragAmount ->
                                if (tabWidthPx > 0f) {
                                    val deltaIdx = dragAmount.x / tabWidthPx
                                    val lastIndex = (indexAnim.value + deltaIdx).fastCoerceIn(0f, (tabs.size - 1).toFloat())
                                    scope.launch {
                                        indexAnim.snapTo(lastIndex)
                                        panelOffset.snapTo(panelOffset.value + dragAmount.x * 0.15f)
                                    }
                                }
                                fingerPosition = change.position
                            }
                        )
                    }
                    .height(64.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(0.dp)
            ) {
                tabs.forEachIndexed { index, tab ->
                    val isSelected = current == tab

                    val centerX = horizontalPadPx + (index + 0.5f) * tabWidthPx
                    val distFromGlassCenter = abs(centerX - glassCenterX)

                    // 环形脊放大曲线（以玻璃中心为圆心）
                    // 区域 1：玻璃内部（0 ~ 半Tab） → 正弦上升，中心=0，边界=最大
                    // 区域 2：玻璃外部（半Tab ~ 半Tab+外层范围） → 正弦下降，边界=最大，外层末尾=0
                    val t: Float
                    val factor: Float
                    if (distFromGlassCenter <= halfTab) {
                        // 玻璃内：0(中心) → 1(边界)
                        t = distFromGlassCenter / halfTab
                        // sin(π/2 × t)：中心0 → 边界1
                        val sine = kotlin.math.sin(0.5f * Math.PI.toFloat() * t)
                        factor = sine
                    } else {
                        // 玻璃外：0(边界) → 1(外层末尾)
                        val outerDist = distFromGlassCenter - halfTab
                        val tn = (outerDist / refractionRangeOuter).fastCoerceIn(0f, 1f)
                        // sin(π/2 × (1-tn)) → 边界1 → 外层末尾0
                        val sine = kotlin.math.sin(0.5f * Math.PI.toFloat() * (1f - tn))
                        factor = sine * (1f - tn)
                    }

                    val scaleX = 1.0f + lensStrength * factor
                    val scaleY = 1.0f + lensStrength * 0.55f * factor

                    NavItem(
                        label = tab.label,
                        icon = tab.icon,
                        selected = isSelected,
                        scaleX = scaleX,
                        scaleY = scaleY,
                        onClick = { onSelect(tab) },
                        modifier = Modifier.weight(1f, fill = true)
                    )
                }
            }
        }
    }
}

private suspend fun PointerInputScope.inspectDragGestures(
    onDragStart: (down: androidx.compose.ui.input.pointer.PointerInputChange) -> Unit = {},
    onDragEnd: (change: androidx.compose.ui.input.pointer.PointerInputChange, wasDragging: Boolean) -> Unit = { _, _ -> },
    onDragCancel: (wasDragging: Boolean) -> Unit = {},
    onDrag: (change: androidx.compose.ui.input.pointer.PointerInputChange, dragAmount: Offset) -> Unit
) {
    val slop = 8.dp.toPx()
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        var pointer = down.id
        var lastChange: androidx.compose.ui.input.pointer.PointerInputChange = down
        var isDragging = false
        var totalDrag = 0f
        var canceled = false
        var done = false
        while (!done) {
            val event = awaitPointerEvent()
            val c1 = event.changes.firstOrNull { it.id == pointer }
            val c2 = c1 ?: event.changes.firstOrNull { it.pressed }
            val change: androidx.compose.ui.input.pointer.PointerInputChange? = c2
            if (change == null) {
                canceled = true
                done = true
            } else {
                if (change.changedToUp()) {
                    lastChange = change
                    done = true
                } else if (change.isOutOfBounds(size, extendedTouchPadding)) {
                    canceled = true
                    done = true
                } else {
                    val delta = change.positionChange()
                    if (!isDragging) {
                        totalDrag += abs(delta.x) + abs(delta.y)
                        if (totalDrag > slop) {
                            isDragging = true
                            onDragStart(down)
                            down.consume()
                        }
                    }
                    if (isDragging && delta != Offset.Zero) {
                        onDrag(change, delta)
                    }
                    lastChange = change
                    if (isDragging) change.consume()
                    pointer = change.id
                }
            }
        }
        if (canceled) onDragCancel(isDragging) else onDragEnd(lastChange, isDragging)
    }
}

enum class Tab(val label: String, val icon: ImageVector) {
    Terminal("终端", Icons.Outlined.Code),
    Web("Web服务", Icons.Outlined.Public),
    About("关于", Icons.Outlined.Info),
}

@Composable
private fun NavItem(
    label: String,
    icon: ImageVector,
    selected: Boolean,
    scaleX: Float,
    scaleY: Float,
    onClick: () -> Unit,
    modifier: Modifier = Modifier
) {
    val containerShape: Shape = RoundedCornerShape(22.dp)
    Box(
        modifier = modifier
            .height(64.dp)
            .semantics {
                this.selected = selected
                contentDescription = label
                role = Role.Tab
            }
            .clip(containerShape)
            .clickable(
                interactionSource = null,
                indication = null,
                role = Role.Tab,
                onClick = onClick
            )
            .graphicsLayer {
                this.scaleX = scaleX
                this.scaleY = scaleY
            },
        contentAlignment = Alignment.Center
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
            modifier = Modifier.padding(top = 6.dp)
        ) {
            val fgColor = if (selected) Green else Color(0xFFC9D1D9)
            CompositionLocalProvider(
                LocalContentColor provides fgColor,
                LocalTextStyle provides LocalTextStyle.current.copy(color = fgColor)
            ) {
                Icon(
                    imageVector = icon,
                    contentDescription = label,
                    modifier = Modifier.size(26.dp),
                    tint = fgColor
                )
                Spacer(Modifier.height(2.dp))
                Text(
                    text = label,
                    color = fgColor,
                    fontSize = 11.sp,
                    fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal
                )
            }
        }
    }
}

@Composable
fun GlassSwitch(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier
) {
    val interactionSource = remember { androidx.compose.foundation.interaction.MutableInteractionSource() }
    val isPressed by interactionSource.collectIsPressedAsState()
    val thumbProgress = remember { Animatable(if (checked) 1f else 0f, visibilityThreshold = 0.001f) }
    val pressProgress = remember { Animatable(0f, visibilityThreshold = 0.001f) }

    LaunchedEffect(checked) {
        thumbProgress.animateTo(
            if (checked) 1f else 0f,
            spring(dampingRatio = 0.62f, stiffness = 900f, visibilityThreshold = 0.001f)
        )
    }
    LaunchedEffect(isPressed) {
        pressProgress.animateTo(if (isPressed) 1f else 0f, spring(1f, 800f, 0.001f))
    }

    val trackWidth = 52.dp
    val trackHeight = 30.dp
    val trackCornerRadius = 15.dp
    val thumbSize = 22.dp
    val thumbPadding = 4.dp

    val press = pressProgress.value
    val thumbPos = thumbProgress.value

    Box(
        modifier
            .width(trackWidth)
            .height(trackHeight)
            .clip(RoundedCornerShape(trackCornerRadius))
            .clickable(
                interactionSource = interactionSource,
                indication = null,
                role = Role.Switch,
                onClick = { onCheckedChange(!checked) }
            )
            .semantics {
                role = Role.Switch
            }
            .graphicsLayer {
                val s = lerp(1f, 0.94f, press)
                scaleX = s
                scaleY = s
            }
            .drawWithContent {
                val w = size.width
                val h = size.height

                if (thumbPos > 0.01f) {
                    drawRect(
                        brush = Brush.verticalGradient(
                            0f to Color(0xFF22C55E).copy(alpha = 0.85f * thumbPos),
                            0.5f to Color(0xFF16A34F).copy(alpha = 0.80f * thumbPos),
                            1f to Color(0xFF15803D).copy(alpha = 0.75f * thumbPos)
                        )
                    )
                }
                if (thumbPos < 0.99f) {
                    val closeAlpha = 1f - thumbPos
                    drawRect(
                        brush = Brush.verticalGradient(
                            0f to Color(0xFF30363D).copy(alpha = 0.90f * closeAlpha),
                            0.5f to Color(0xFF21262D).copy(alpha = 0.90f * closeAlpha),
                            1f to Color(0xFF161B22).copy(alpha = 0.90f * closeAlpha)
                        )
                    )
                }

                drawRect(
                    brush = Brush.verticalGradient(
                        0.00f to Color(0x80FFFFFF),
                        0.30f to Color(0x20FFFFFF),
                        1.00f to Color(0x00FFFFFF)
                    ),
                    style = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.dp.toPx())
                )

                if (thumbPos > 0f) {
                    drawRect(
                        Color(0x3322C55E).copy(alpha = thumbPos * 0.3f * (0.5f + press * 0.5f)),
                        blendMode = BlendMode.Plus
                    )
                }

                if (press > 0f) {
                    drawRect(Color.White.copy(alpha = 0.04f * press), blendMode = BlendMode.Plus)
                }

                drawContent()

                val thumbVel = thumbProgress.velocity
                val velFactor = (abs(thumbVel) * 0.12f).fastCoerceIn(0f, 0.18f)
                val thumbScaleX = 1f + velFactor + press * 0.06f
                val thumbScaleY = 1f - velFactor * 0.4f + press * 0.06f

                val thumbPx = thumbSize.toPx()
                val paddingPx = thumbPadding.toPx()
                val maxOffset = w - thumbPx - paddingPx * 2
                val thumbX = paddingPx + maxOffset * thumbPos
                val thumbCx = thumbX + thumbPx / 2f
                val thumbCy = h / 2f
                val drawW = thumbPx * thumbScaleX
                val drawH = thumbPx * thumbScaleY
                val drawX = thumbCx - drawW / 2f
                val drawY = thumbCy - drawH / 2f
                val thumbR = (minOf(drawW, drawH) / 2f)

                drawRoundRect(
                    color = Color.Black.copy(alpha = 0.15f),
                    topLeft = Offset(drawX, drawY + 1.dp.toPx()),
                    size = Size(drawW, drawH),
                    cornerRadius = androidx.compose.ui.geometry.CornerRadius(thumbR, thumbR)
                )

                drawRoundRect(
                    brush = Brush.verticalGradient(
                        0f to Color(0xFFFFFFFF),
                        0.5f to Color(0xFFF0F6FC),
                        1f to Color(0xFFD0D7DE)
                    ),
                    topLeft = Offset(drawX, drawY),
                    size = Size(drawW, drawH),
                    cornerRadius = androidx.compose.ui.geometry.CornerRadius(thumbR, thumbR)
                )

                drawRoundRect(
                    brush = Brush.verticalGradient(
                        0f to Color(0xCCFFFFFF),
                        0.4f to Color(0x00FFFFFF)
                    ),
                    topLeft = Offset(drawX, drawY),
                    size = Size(drawW, drawH * 0.5f),
                    cornerRadius = androidx.compose.ui.geometry.CornerRadius(thumbR, thumbR)
                )
            }
    )
}
