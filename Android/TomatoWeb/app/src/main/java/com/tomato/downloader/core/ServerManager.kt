package com.tomato.downloader.core

import android.content.Context
import android.os.Build
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

/**
 * 全局单例：负责复刻 installer.sh 的逻辑（获取最新版本 -> 下载二进制到应用私有目录
 * -> 赋予可执行权限），并以 --server 模式运行该二进制启动 Web UI。
 *
 * 所有状态与日志通过 StateFlow / SharedFlow 暴露给 UI。
 */
object ServerManager {

    /**
     * 端口：不在用 const val 硬编码；读取 PortStore 中用户自定义的持久化端口。
     * 兼容代码：通过 val 获取即可，其余模块直接使用 ServerManager.port 而不是 PORT 常量。
     */
    val port: Int get() = PortStore.currentPort

    private const val REPO = "zhongbai2333/Tomato-Novel-Downloader"
    private const val ACCEL_BASE = "https://dl.zhongbai233.com/release"
    private const val GITHUB_BASE =
        "https://github.com/zhongbai2333/Tomato-Novel-Downloader/releases/download"

    enum class ServerState { IDLE, INSTALLING, RUNNING, STOPPED, ERROR }

    private const val MAX_LOG_LINES = 1000

    /**
     * 日志列表：使用 StateFlow 统一管理。
     * 避免之前用 SharedFlow+replay 造成「UI 层清理后切页面再返回，旧日志又 replay 出来」的问题。
     */
    private val _logLines = MutableStateFlow<List<String>>(emptyList())
    val logLines: StateFlow<List<String>> = _logLines.asStateFlow()

    private val _state = MutableStateFlow(ServerState.IDLE)
    val state: StateFlow<ServerState> = _state.asStateFlow()

    private val _localUrl = MutableStateFlow("http://127.0.0.1:${PortStore.DEFAULT_PORT}/")
    val localUrl: StateFlow<String> = _localUrl.asStateFlow()

    private val _lanUrl = MutableStateFlow("")
    val lanUrl: StateFlow<String> = _lanUrl.asStateFlow()

    /**
     * 公网/广域网真实 IP（运营商出口 IP，如 106.226.205.206）。
     * 仅保存纯 IP，端口号由 UI 层组合。空字符串代表尚未获取或获取失败。
     */
    private val _publicIp = MutableStateFlow("")
    val publicIp: StateFlow<String> = _publicIp.asStateFlow()

    /** 公网 IP 获取状态（用于防止重复点击刷新）。 */
    private val _publicIpLoading = MutableStateFlow(false)
    val publicIpLoading: StateFlow<Boolean> = _publicIpLoading.asStateFlow()

    /**
     * 公网 IP 归属地（地理位置）。
     * 规则：
     *   - 中国（countryCode == CN）：显示「省份 城市」，例如「四川 成都」
     *   - 海外：只显示「国家名」，例如「日本」/「美国」
     *   - 未知 / 解析失败：空字符串
     */
    private val _publicGeo = MutableStateFlow("")
    val publicGeo: StateFlow<String> = _publicGeo.asStateFlow()

    /** 归属地国旗 emoji（CN 为🇨🇳，海外取国家代码对应 flag emoji），解析不出为空。 */
    private val _publicGeoFlag = MutableStateFlow("")
    val publicGeoFlag: StateFlow<String> = _publicGeoFlag.asStateFlow()

    /** 归属地请求状态，防止重复触发。 */
    private val _publicGeoLoading = MutableStateFlow(false)
    val publicGeoLoading: StateFlow<Boolean> = _publicGeoLoading.asStateFlow()

    private val _binaryPath = MutableStateFlow<String?>(null)
    val binaryPath: StateFlow<String?> = _binaryPath.asStateFlow()

    /** 探测到的可执行目录（解决 Android 10+ filesDir 的 W^X 限制）。 */
    private val _execDir = MutableStateFlow<File?>(null)
    val execDir: StateFlow<File?> = _execDir.asStateFlow()

    /** 最后一次成功安装的版本 tag（持久化到 SharedPreferences，避免杀后台重进重复下载）。 */
    private var lastInstalledTag: String? = null
    private const val PREFS_NAME = "tomato_web_prefs"
    private const val KEY_LAST_TAG = "last_installed_tag"

    /** 从持久化恢复 lastInstalledTag（Application.onCreate 时调用）。 */
    fun init(context: Context) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        lastInstalledTag = prefs.getString(KEY_LAST_TAG, null)?.takeIf { it.isNotBlank() }
    }

    private fun persistLastTag(context: Context, tag: String?) {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE).edit()
        if (tag == null) prefs.remove(KEY_LAST_TAG) else prefs.putString(KEY_LAST_TAG, tag)
        prefs.apply()
        lastInstalledTag = tag
    }

    private var process: Process? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var runJob: Job? = null

    /** 主动关闭标志位：用于区分 stop() 主动关闭和真正的启动失败异常 */
    @Volatile
    private var isStopping = false

    /** PortStore 端口变更回调：立即更新本机/局域网 URL 显示。 */
    fun onPortChanged(newPort: Int) {
        _localUrl.value = "http://127.0.0.1:$newPort/"
        refreshLanUrl()
    }

    /** 追加一行日志；超过上限则丢弃最旧的。 */
    fun emit(line: String) {
        _logLines.value = (_logLines.value + line).let {
            if (it.size > MAX_LOG_LINES) it.drop(it.size - MAX_LOG_LINES) else it
        }
    }

    /** 真正清空日志：切页面再回来也不会出现旧日志。 */
    fun clearLogs() {
        _logLines.value = emptyList()
    }

    fun detectArch(): String {
        val abis = Build.SUPPORTED_ABIS
        return when {
            abis.any { it == "arm64-v8a" } -> "arm64"
            abis.any { it == "armeabi-v7a" || it == "armeabi" } -> "arm32"
            else -> "arm64"
        }
    }

    /**
     * 探测应用私有目录中可执行二进制的目录。
     *
     * 背景：Android 10 (API 29) 起，[Context.filesDir] 被挂载为 noexec（W^X 保护），
     * 即使 chmod +x 也无法执行下载的原生二进制。Termux 之所以能跑，是因为它的
     * /data/data/com.termux/files/home 有特殊执行权限；我们用不了 Termux 的目录
     * （SELinux 隔离），但可以尝试自己的 codeCacheDir / cacheDir / filesDir 等候选。
     *
     * 这里通过实际执行一个测试脚本（#!/system/bin/sh）来验证目录是否可执行，
     * 而不是只看文件权限位（noexec 挂载下权限位会骗人）。
     */
    suspend fun findExecDir(context: Context): File? = withContext(Dispatchers.IO) {
        // 候选目录，按可执行概率从高到低排序
        val candidates = linkedSetOf(
            context.codeCacheDir,                       // /data/data/<pkg>/code_cache —— 系统为可执行代码预留
            File(context.filesDir, "bin").apply { mkdirs() },  // filesDir 子目录，部分 ROM 可执行
            context.filesDir,                          // /data/data/<pkg>/files —— Android 10+ 多为 noexec
            context.cacheDir                           // /data/data/<pkg>/cache —— 兜底
        )

        emit("[INFO] 探测可执行目录（Android 10+ 对 filesDir 有 W^X 限制）...")
        for (dir in candidates) {
            val path = dir.absolutePath
            val test = File(dir, ".exec_probe.sh")
            try {
                test.writeText("#!/system/bin/sh\necho OK\n")
                test.setExecutable(true, false)
                val proc = Runtime.getRuntime().exec(arrayOf(test.absolutePath))
                val out = proc.inputStream.bufferedReader().readText().trim()
                val code = proc.waitFor()
                test.delete()
                if (code == 0 && out == "OK") {
                    emit("[INFO] ✓ 可执行目录：$path")
                    _execDir.value = dir
                    return@withContext dir
                } else {
                    emit("[WARN] ✗ 不可执行：$path (exit=$code, out=$out)")
                }
            } catch (e: Exception) {
                test.delete()
                emit("[WARN] ✗ 不可执行：$path (${e.message})")
            }
        }
        emit("[ERR ] 所有候选目录均不可执行。您的设备可能限制了应用运行下载的二进制。")
        _execDir.value = null
        null
    }

    /**
     * 当前应当使用的二进制文件：
     *  - 如果启用了自定义 Web 且文件已复制，返回自定义二进制
     *  - 否则返回默认从 GitHub 下载的二进制
     */
    fun binaryFile(context: Context): File {
        if (CustomWebStore.enabled.value && CustomWebStore.isCustomInstalled(context)) {
            return CustomWebStore.customBinaryFile(context)
        }
        val dir = _execDir.value ?: context.codeCacheDir
        return File(dir, "TomatoNovelDownloader-Android_${detectArch()}")
    }

    fun isInstalled(context: Context): Boolean = binaryFile(context).exists()

    /** 当前是否使用自定义 Web 二进制（用于日志和提示）。 */
    fun isUsingCustomWeb(context: Context): Boolean =
        CustomWebStore.enabled.value && CustomWebStore.isCustomInstalled(context)

    fun refreshLanUrl() {
        val ip = NetworkUtils.getLanIp()
        val p = port
        _localUrl.value = "http://127.0.0.1:$p/"
        _lanUrl.value = if (ip != null) "http://$ip:$p/" else ""
    }

    /**
     * 获取公网真实出口 IP（如 106.226.205.206）。
     * 顺序尝试多个稳定 API：ipify → icanhazip → ifconfig.me，每个超时 5s，
     * 成功后写入 _publicIp（纯 IP），并自动触发归属地解析。
     * 异步 IO 线程执行，可在 UI 层随时点击刷新。
     */
    fun refreshPublicIp() {
        if (_publicIpLoading.value) return
        scope.launch {
            _publicIpLoading.value = true
            try {
                val ip = withContext(Dispatchers.IO) { fetchPublicIp() }
                if (ip != null) {
                    _publicIp.value = ip
                    // 拿到新 IP 后自动触发归属地解析
                    refreshPublicIpInternal()
                }
            } finally {
                _publicIpLoading.value = false
            }
        }
    }

    /**
     * 解析当前公网 IP 的归属地（国家 / 省 / 市）。
     * 使用 ip-api.com/json（免费、返回中文、无需 key），每请求超时 5s。
     *  - 中国（countryCode=CN）：取 regionName（省份） + city（市）
     *  - 海外：只取 country（国家名）
     */
    fun refreshPublicGeo() {
        if (_publicGeoLoading.value) return
        if (_publicIp.value.isBlank()) return
        scope.launch { refreshPublicIpInternal() }
    }

    private suspend fun refreshPublicIpInternal() {
        if (_publicGeoLoading.value) return
        val ip = _publicIp.value.takeIf { it.isNotBlank() } ?: return
        _publicGeoLoading.value = true
        try {
            val geo = withContext(Dispatchers.IO) { fetchPublicGeo(ip) }
            if (geo != null) {
                _publicGeo.value = geo.location
                _publicGeoFlag.value = geo.flag
            }
        } finally {
            _publicGeoLoading.value = false
        }
    }

    /**
     * @param location 显示文字（中国：省 + 市；海外：国家或国家+州）
     * @param flag 国旗 emoji，空字符串表示无国旗
     * @param isCnOrRegionHint 解析器推断这是「中国大陆 / 港澳台 / 自治区直辖市」等「国内可信结果」。
     *                         百度对国内准但对海外不准，该标志为 true 时上层会优先采信 Phase A/B 的结果；
     *                         为 false 时上层会丢弃，改走 Phase C 海外专业 API 链。
     */
    private data class GeoResult(
        val location: String,
        val flag: String,
        val isCnOrRegionHint: Boolean
    )

    private fun fetchPublicGeo(ip: String): GeoResult? {
        // 多数据源分层（按精度从高到低）：
        //   Phase A（国内最高精度）：纯真 IP 库 cz88.net — 拥有市级/区级精确数据，
        //                           解决运营商省级 IP 被其他库默认回退到省会的问题
        //                           （如 39.166.8.75 江西移动 → 百度/太平洋返回南昌，纯真返回赣州）。
        //   Phase B（国内精确）：百度 opendata  → 只有当返回明确是 中国大陆省/市/自治区 或 港澳台时，才采信。
        //                        这样百度对国内/港澳台 IP 的高精度保留，同时避开「英国→意大利米兰」这种海外误判。
        //   Phase C（国内备用）：太平洋 pconline (GBK→UTF-8)，结构化返回 pro/city 字段，国内备用。
        //   Phase D（海外精确）：ipwho.is (zh-CN) + ip.sb (免费 HTTPS JSON) 链式。海外 IP 这些专业 API 远比百度准确。
        //                       直接读取 API 返回的 country_code → 国旗 emoji，不经过中文国名映射表，避免偏差。

        // —— Phase A: 纯真 IP 库 cz88.net（国内市级精度最高）——
        val cz88 = runCatching {
            val text = httpsGetText("https://www.cz88.net/api/cz88/ip/base?ip=$ip")
            parseCz88(text)
        }.getOrNull()
        if (cz88 != null) {
            return cz88
        }

        // —— Phase B: 百度 ——
        val baidu = runCatching {
            val text = httpsGetText("https://opendata.baidu.com/api.php?query=$ip&co=&resource_id=6006&oe=utf8")
            parseBaidu(text)
        }.getOrNull()
        if (baidu != null && baidu.isCnOrRegionHint) {
            return baidu
        }

        // —— Phase C: 太平洋国内备用（只有 pro/city 结构化才采信）——
        val pcon = runCatching {
            val bytes = httpsGetBytes("https://whois.pconline.com.cn/ipJson.jsp?ip=$ip&json=true")
            val text = String(bytes, charset("GBK"))
            parsePconline(text)
        }.getOrNull()
        if (pcon != null && pcon.isCnOrRegionHint) {
            return pcon
        }

        // —— Phase D: 海外 IP 专用链（按顺序，第一个成功就返回）——
        val overseasSources = listOf(
            // ipwho.is 支持中文 lang=zh-CN，返回结构规范
            {
                val text = httpsGetText("https://ipwho.is/$ip?lang=zh-CN&fields=success,country,country_code,region,city")
                parseIpwho(text)
            },
            // ip.sb 免费 HTTPS，返回带 country_code/ASN，海外常用免费源
            {
                val text = httpsGetText("https://api.ip.sb/geoip/$ip")
                parseIpSb(text)
            },
            // ipapi.co 海外兜底，有免费限额
            {
                val text = httpsGetText("https://ipapi.co/$ip/json/")
                parseIpapi(text)
            }
        )
        for (src in overseasSources) {
            runCatching { src() }.getOrNull()?.let { return it }
        }

        // 最后兜底：如果 Phase B 百度有结果（哪怕不够准），总比空着好
        if (baidu != null) return baidu
        if (pcon != null) return pcon
        return null
    }

    // —————————————— 网络请求辅助（避免重复样板）——————————————
    private fun httpsGetText(urlStr: String): String {
        val conn = URL(urlStr).openConnection() as HttpURLConnection
        conn.requestMethod = "GET"
        conn.connectTimeout = 5000
        conn.readTimeout = 5000
        conn.instanceFollowRedirects = true
        conn.setRequestProperty("User-Agent", "TomatoWeb-Android")
        return try {
            conn.inputStream.bufferedReader().use { it.readText() }
        } finally {
            conn.disconnect()
        }
    }

    private fun httpsGetBytes(urlStr: String): ByteArray {
        val conn = URL(urlStr).openConnection() as HttpURLConnection
        conn.requestMethod = "GET"
        conn.connectTimeout = 5000
        conn.readTimeout = 5000
        conn.instanceFollowRedirects = true
        conn.setRequestProperty("User-Agent", "TomatoWeb-Android")
        return try {
            conn.inputStream.readBytes()
        } finally {
            conn.disconnect()
        }
    }

    // —————————————— 纯真 IP 库 cz88.net 解析（Phase A 最高精度）——————————————
    // 响应形如：{"code":200,"success":true,"message":"操作成功",
    //   "data":{"ip":"39.166.8.75","countryCode":"CN","country":"中国",
    //   "province":"江西","city":"赣州","districts":"赣县区","isp":"中国移动",
    //   "cityCode":"360700",...}}
    // 纯真库对国内运营商 IP 的市级精度远高于百度/太平洋（它们对省级 IP 默认回退到省会）。
    private fun parseCz88(raw: String): GeoResult? {
        if (raw.isEmpty()) return null
        // cz88 的 success 是布尔值（"success":true），不是字符串，用 contains 检测
        if (!raw.contains("\"success\":true")) return null
        val countryCode = raw.extractStringField("countryCode") ?: return null
        if (countryCode.isBlank()) return null

        val country = raw.extractStringField("country") ?: ""
        val province = raw.extractStringField("province") ?: ""
        val city = raw.extractStringField("city") ?: ""

        val code = countryCode.uppercase()
        val isCn = code in setOf("CN", "HK", "MO", "TW")

        val location = when (code) {
            "CN" -> {
                val pro = normalizeCnProvince(province)
                val ci = normalizeCnCity(city, province)
                // 直辖市：省=市，city 通常为空，只显示一次
                if (isDirectMunicipality(province) && ci.isBlank()) pro
                else listOf(pro, ci).filter { it.isNotBlank() }.joinToString("") { it }
                    .ifBlank { country.ifBlank { return null } }
            }
            "HK" -> "香港特别行政区"
            "MO" -> "澳门特别行政区"
            "TW" -> "台湾省"
            // 海外：只显示国家名
            else -> country.ifBlank { province.ifBlank { return null } }
        }.trim()

        if (location.isBlank()) return null
        return GeoResult(
            location = location,
            flag = countryCodeToFlag(code),
            isCnOrRegionHint = isCn
        )
    }

    /** 判断是否为直辖市（北京/上海/天津/重庆）。 */
    private fun isDirectMunicipality(province: String): Boolean =
        province in setOf("北京", "上海", "天津", "重庆")

    /**
     * 规范化中国省名：纯真返回 "江西"，补全为 "江西省"。
     * 直辖市补 "市"，自治区补全全称。
     */
    private fun normalizeCnProvince(province: String): String {
        if (province.isBlank()) return ""
        return when (province) {
            "北京" -> "北京市"
            "上海" -> "上海市"
            "天津" -> "天津市"
            "重庆" -> "重庆市"
            "内蒙古" -> "内蒙古自治区"
            "广西" -> "广西壮族自治区"
            "西藏" -> "西藏自治区"
            "宁夏" -> "宁夏回族自治区"
            "新疆" -> "新疆维吾尔自治区"
            else -> if (province.endsWith("省") || province.endsWith("市") ||
                province.endsWith("自治区")) province else province + "省"
        }
    }

    /**
     * 规范化中国市名：纯真返回 "赣州"，补全为 "赣州市"。
     * 直辖市时 city 为空（省名已含"市"），返回空避免重复。
     */
    private fun normalizeCnCity(city: String, province: String): String {
        if (city.isBlank()) return ""
        if (isDirectMunicipality(province)) return ""
        return when {
            city.endsWith("市") || city.endsWith("自治州") ||
                city.endsWith("地区") || city.endsWith("盟") -> city
            else -> city + "市"
        }
    }

    // —————————————— 百度 opendata 解析 ——————————————
    // 响应形如：{"status":"0","data":[{"location":"江西省赣州市 电信","origip":"106.226.205.206"}]}
    //  港澳台："location":"香港特别行政区"  "location":"台湾省"  "location":"澳门特别行政区"
    //  海外：  "location":"美国"   "location":"澳大利亚"   "location":"亚太地区"
    // 这里的 data 数组元素内部是 JSON 对象，用轻量正则抽取 "location":"..."}
    private fun parseBaidu(raw: String): GeoResult? {
        if (raw.isEmpty()) return null
        val location = raw.extractStringField("location")?.takeIf { it.isNotBlank() } ?: return null

        // 剔除 ISP 后缀：电信/联通/移动/铁通/长城宽带/广电/校园网 及之前的空格
        val cleaned = location
            .replaceFirst("""\s+(电信|联通|移动|铁通|长城宽带|广电|校园网|宽带|天威视讯|教育网|科技网|和记环球电讯|电讯盈科|中华电信).*""".toRegex(), "")
            .trim()
        if (cleaned.isBlank()) return null

        // 判定是否为「国内/港澳台可信结果」——有这些关键字说明百度认得是大中华区
        val isHK = cleaned.contains("香港")
        val isMO = cleaned.contains("澳门")
        val isTW = cleaned.contains("台湾")
        val isCNMainland = cleaned.contains("省") || cleaned.contains("自治区") || cleaned.contains("特别行政区")
                || cleaned.matches(Regex("""[\u4e00-\u9fa5]{2,8}市[\u4e00-\u9fa5]{0,6}(区|县)?"""))

        val isCnHint = isHK || isMO || isTW || isCNMainland || cleaned == "中国"

        val code = when {
            isHK -> "HK"
            isMO -> "MO"
            isTW -> "TW"
            isCNMainland || cleaned == "中国" -> "CN"
            // 海外单国名："美国" "澳大利亚" "日本" "新加坡" 等
            cleaned.matches(Regex("""[\u4e00-\u9fa5]{2,12}""")) -> countryNameToCode(cleaned)
            else -> ""
        }

        return GeoResult(
            location = cleaned,
            flag = countryCodeToFlag(code),
            isCnOrRegionHint = isCnHint
        )
    }

    // —————————————— 太平洋 pconline 解析 ——————————————
    // {"ip":"...","pro":"江西省","proCode":"360000","city":"赣州市","cityCode":"360700","addr":"江西省赣州市 电信","regionNames":"","err":""}
    private fun parsePconline(raw: String): GeoResult? {
        if (raw.isEmpty()) return null
        if (raw.extractStringField("err").equals("nocity", ignoreCase = true)) {
            // nocity（香港等）但还有 addr 字段，退化为用 addr 字段解析（与百度类似）
            val addr = raw.extractStringField("addr")?.takeIf { it.isNotBlank() } ?: return null
            val cleaned = addr
                .replaceFirst("""\s+(电信|联通|移动|铁通|长城宽带|广电|校园网|宽带|天威视讯|教育网|科技网|和记环球电讯|电讯盈科|中华电信).*""".toRegex(), "")
                .trim()
            if (cleaned.isBlank()) return null
            val isHK = cleaned.contains("香港")
            val isMO = cleaned.contains("澳门")
            val isTW = cleaned.contains("台湾")
            val isCNMain = cleaned.contains("省") || cleaned.contains("自治区") || cleaned.contains("市")
            val isCn = isHK || isMO || isTW || isCNMain
            val code = when {
                isHK -> "HK"
                isMO -> "MO"
                isTW -> "TW"
                isCNMain -> "CN"
                else -> countryNameToCode(cleaned)
            }
            return GeoResult(
                location = cleaned,
                flag = countryCodeToFlag(code),
                isCnOrRegionHint = isCn
            )
        }
        val pro = raw.extractStringField("pro") ?: ""
        val city = raw.extractStringField("city") ?: ""
        if (pro.isBlank()) return null

        val (code, isCn) = when (pro) {
            "香港" -> "HK" to true
            "澳门" -> "MO" to true
            "台湾" -> "TW" to true
            else -> "CN" to true
        }

        val display = when (code) {
            "CN" -> listOf(pro, city).filter { it.isNotBlank() }.joinToString(" ").ifBlank { pro }
            else -> pro
        }
        return GeoResult(
            location = display,
            flag = countryCodeToFlag(code),
            isCnOrRegionHint = isCn
        )
    }

    // —————————————— ipwho.is 解析（Phase C 第一优先，返回中文，结构规范）——————————————
    private fun parseIpwho(raw: String): GeoResult? {
        if (raw.isEmpty()) return null
        val ok = raw.extractStringField("success")?.equals("true", ignoreCase = true)
            ?: (raw.contains("\"country\"") && !raw.contains("\"error\""))
        if (!ok) return null
        val code = raw.extractStringField("country_code") ?: ""
        val country = raw.extractStringField("country") ?: ""
        val region = raw.extractStringField("region") ?: ""
        val city = raw.extractStringField("city") ?: ""
        val isCn = code.equals("CN", ignoreCase = true)
                || code.equals("HK", ignoreCase = true)
                || code.equals("MO", ignoreCase = true)
                || code.equals("TW", ignoreCase = true)

        val location = when (code.uppercase()) {
            "CN" -> listOf(region, city).filter { it.isNotBlank() }.joinToString(" ").ifBlank { country }
            "HK", "MO", "TW" -> country.ifBlank { null } ?: return null
            // 海外：按用户需求「其他返回大概」——只显示国家名，不显示州/市（国外不需要精细）
            else -> country.ifBlank { region.ifBlank { null } ?: return null }
        }.trim()

        if (location.isBlank()) return null
        return GeoResult(
            location = location,
            flag = countryCodeToFlag(code),
            isCnOrRegionHint = isCn
        )
    }

    // —————————————— ip.sb 解析（Phase C 第二优先，免费可靠海外 HTTPS JSON）——————————————
    // 响应示例：{"country":"中国","country_code":"CN","region":"Jiangxi","city":"Ganzhou","organization":"..."}
    private fun parseIpSb(raw: String): GeoResult? {
        if (raw.isEmpty()) return null
        val code = raw.extractStringField("country_code") ?: ""
        if (code.isBlank()) return null
        val country = raw.extractStringField("country") ?: ""
        val region = raw.extractStringField("region") ?: ""
        val city = raw.extractStringField("city") ?: ""
        val isCn = setOf("CN", "HK", "MO", "TW").contains(code.uppercase())

        val location = when (code.uppercase()) {
            "CN" -> listOf(region, city).filter { it.isNotBlank() }.joinToString(" ").ifBlank { country }
            "HK" -> "香港"
            "MO" -> "澳门"
            "TW" -> "台湾"
            // 海外：大概 → 只显示国家
            else -> country.ifBlank { region.ifBlank { null } ?: return null }
        }.trim()

        return GeoResult(
            location = location,
            flag = countryCodeToFlag(code),
            isCnOrRegionHint = isCn
        )
    }

    // —————————————— ipapi.co 解析（Phase C 兜底）——————————————
    // {"country_name":"Japan","country":"JP","region":"Tokyo","city":"Tokyo","country_code":"JP",...}
    private fun parseIpapi(raw: String): GeoResult? {
        if (raw.isEmpty()) return null
        if (raw.contains("\"error\"")) return null
        val code = (raw.extractStringField("country_code") ?: raw.extractStringField("country") ?: "").uppercase()
        if (code.isBlank()) return null
        val country = raw.extractStringField("country_name") ?: ""
        val region = raw.extractStringField("region") ?: ""
        val city = raw.extractStringField("city") ?: ""
        val isCn = setOf("CN", "HK", "MO", "TW").contains(code)

        val location = when (code) {
            "CN" -> listOf(region, city).filter { it.isNotBlank() }.joinToString(" ").ifBlank { country }
            "HK" -> "香港"
            "MO" -> "澳门"
            "TW" -> "台湾"
            else -> country.ifBlank { region.ifBlank { null } ?: return null }
        }.trim()

        return GeoResult(
            location = location,
            flag = countryCodeToFlag(code),
            isCnOrRegionHint = isCn
        )
    }

    // —————————————— 字符串抽取（不引 JSON 库）——————————————
    private fun String.extractStringField(key: String): String? {
        val marker = "\"$key\":\""
        val start = this.indexOf(marker)
        if (start < 0) return null
        val valueStart = start + marker.length
        val valueEnd = this.indexOf('"', valueStart)
        if (valueEnd < 0) return null
        return this.substring(valueStart, valueEnd)
    }

    /** 常见国名（中文） → ISO 两位国家代码，用于生成国旗 emoji。找不到就返回 ""（无国旗）。 */
    private fun countryNameToCode(name: String): String = when (name.trim()) {
        "中国", "中华人民共和国", "中国大陆" -> "CN"
        "香港", "中国香港", "香港特别行政区" -> "HK"
        "澳门", "中国澳门", "澳门特别行政区" -> "MO"
        "台湾", "中国台湾", "台湾省" -> "TW"
        "美国", "美利坚合众国" -> "US"
        "日本" -> "JP"
        "韩国", "大韩民国", "南韩" -> "KR"
        "朝鲜", "北韩", "朝鲜民主主义人民共和国" -> "KP"
        "英国", "联合王国", "大不列颠及北爱尔兰联合王国" -> "GB"
        "法国", "法兰西共和国" -> "FR"
        "德国", "德意志联邦共和国" -> "DE"
        "俄罗斯", "俄罗斯联邦", "俄国" -> "RU"
        "加拿大" -> "CA"
        "澳大利亚", "澳洲" -> "AU"
        "新西兰" -> "NZ"
        "新加坡" -> "SG"
        "马来西亚", "大马" -> "MY"
        "泰国" -> "TH"
        "越南", "越南社会主义共和国" -> "VN"
        "印度尼西亚", "印尼" -> "ID"
        "菲律宾" -> "PH"
        "印度" -> "IN"
        "巴基斯坦" -> "PK"
        "孟加拉国" -> "BD"
        "缅甸" -> "MM"
        "柬埔寨" -> "KH"
        "文莱" -> "BN"
        "老挝" -> "LA"
        "尼泊尔" -> "NP"
        "斯里兰卡" -> "LK"
        "不丹" -> "BT"
        "马尔代夫" -> "MV"
        "沙特阿拉伯", "沙特" -> "SA"
        "阿联酋", "阿拉伯联合酋长国" -> "AE"
        "卡塔尔" -> "QA"
        "科威特" -> "KW"
        "土耳其" -> "TR"
        "以色列" -> "IL"
        "伊朗" -> "IR"
        "伊拉克" -> "IQ"
        "埃及" -> "EG"
        "南非" -> "ZA"
        "尼日利亚" -> "NG"
        "肯尼亚" -> "KE"
        "摩洛哥" -> "MA"
        "巴西" -> "BR"
        "阿根廷" -> "AR"
        "墨西哥" -> "MX"
        "智利" -> "CL"
        "哥伦比亚" -> "CO"
        "秘鲁" -> "PE"
        "委内瑞拉" -> "VE"
        "西班牙" -> "ES"
        "意大利" -> "IT"
        "葡萄牙" -> "PT"
        "荷兰" -> "NL"
        "比利时" -> "BE"
        "瑞士" -> "CH"
        "瑞典" -> "SE"
        "挪威" -> "NO"
        "丹麦" -> "DK"
        "芬兰" -> "FI"
        "波兰" -> "PL"
        "奥地利" -> "AT"
        "希腊" -> "GR"
        "捷克" -> "CZ"
        "匈牙利" -> "HU"
        "爱尔兰" -> "IE"
        "冰岛" -> "IS"
        else -> ""
    }

    /** 两位国家代码 → 国旗 emoji（Regional Indicator 字母，A = 0x1F1E6）。 */
    private fun countryCodeToFlag(code: String): String {
        val c = code.uppercase().take(2)
        if (c.length < 2) return ""
        val c1 = c[0]
        val c2 = c[1]
        if (c1 !in 'A'..'Z' || c2 !in 'A'..'Z') return ""
        val base = 0x1F1E6
        val r1 = base + (c1.code - 'A'.code)
        val r2 = base + (c2.code - 'A'.code)
        return String(Character.toChars(r1)) + String(Character.toChars(r2))
    }

    /** 依次请求多个公网 IP API，返回第一个有效的 IPv4 文本。 */
    private fun fetchPublicIp(): String? {
        val endpoints = listOf(
            "https://api.ipify.org?format=text",
            "https://ipv4.icanhazip.com/",
            "https://ifconfig.me/ip",
            "https://ip4.seeip.org/",
            "https://checkip.amazonaws.com/"
        )
        for (url in endpoints) {
            try {
                val conn = URL(url).openConnection() as HttpURLConnection
                conn.requestMethod = "GET"
                conn.connectTimeout = 5000
                conn.readTimeout = 5000
                conn.setRequestProperty("User-Agent", "TomatoWeb-Android")
                conn.instanceFollowRedirects = true
                conn.inputStream.bufferedReader().useLines { lines ->
                    val raw = lines.firstOrNull()?.trim() ?: return@useLines null
                    if (raw.isIpv4()) return raw
                }
            } catch (_: Exception) { /* 跳过，尝试下一个 */ }
        }
        return null
    }

    private val ipv4Regex = Regex("""^(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)$""")
    private fun String.isIpv4(): Boolean = ipv4Regex.matches(this)

    /** 复刻 installer.sh：查询版本、下载二进制到私有目录并赋予执行权限。 */
    suspend fun install(context: Context, force: Boolean = false) {
        if (_state.value == ServerState.INSTALLING) return
        _state.value = ServerState.INSTALLING
        try {
            doInstall(context, force)
            _state.value = ServerState.IDLE
            emit("[INFO] 全部完成。")
        } catch (e: Exception) {
            emit("[ERR ] 安装失败：${e.message}")
            _state.value = ServerState.ERROR
        }
    }

    private suspend fun doInstall(context: Context, force: Boolean) = withContext(Dispatchers.IO) {
        val arch = detectArch()
        emit("[INFO] 检测到设备架构：Android_$arch")

        // 先探测可执行目录（Android 10+ 的 filesDir 是 noexec）
        val execDir = findExecDir(context) ?: run {
            error("无可执行目录，无法安装。请尝试在 Termux 中运行 installer.sh")
        }

        emit("[INFO] 正在从 GitHub API 获取最新版本信息...")
        val tag = fetchLatestTag()
        val version = tag.removePrefix("v")
        emit("[INFO] 最新版本：$tag（VERSION=$version）")

        val binaryName = "TomatoNovelDownloader-Android_${arch}-v$version"
        val canonicalName = "TomatoNovelDownloader-Android_$arch"
        val dest = File(execDir, canonicalName)

        // 版本检查：已安装的版本 == 最新版本 时，跳过重复下载
        // （即使 force=true，只要版本一致就不重复下；解决用户点下载图标又点获取Web的重复下载问题）
        if (dest.exists() && lastInstalledTag == tag) {
            emit("[INFO] 当前已是最新版本（$tag），无需重复下载。")
            _binaryPath.value = dest.absolutePath
            return@withContext
        }
        // 非 force 模式且已有二进制（不知道版本号但文件存在），也跳过
        if (dest.exists() && !force && lastInstalledTag == null) {
            emit("[INFO] 已存在已安装的二进制：${dest.absolutePath}")
            _binaryPath.value = dest.absolutePath
            return@withContext
        }

        // installer.sh 提供两种下载源：加速源 与 直连 GitHub。这里依次尝试。
        val urls = listOf(
            "$ACCEL_BASE/$tag/$binaryName",
            "$GITHUB_BASE/$tag/$binaryName"
        )
        emit("[INFO] 准备下载：$binaryName")
        emit("[INFO] 安装目标目录：${execDir.absolutePath}")
        emit("[INFO] 完整文件路径：${dest.absolutePath}")

        val tmp = File(execDir, "$canonicalName.tmp")
        if (tmp.exists()) tmp.delete()

        var ok = false
        for (u in urls) {
            emit("[INFO] 下载链接：$u")
            emit("[INFO] 开始下载...")
            try {
                var lastPct = -1
                download(u, tmp) { read, total ->
                    if (total > 0) {
                        val pct = (read * 100 / total).toInt()
                        if (pct >= lastPct + 5 || pct == 100) {
                            lastPct = pct
                            emit("[INFO] 下载进度：$pct% (${fmt(read)} / ${fmt(total)})")
                        }
                    }
                }
                ok = true
                break
            } catch (e: Exception) {
                emit("[WARN] 该源下载失败：${e.message}，尝试下一个源...")
                if (tmp.exists()) tmp.delete()
            }
        }
        if (!ok) error("所有下载源均失败，请检查网络后重试")

        // 重命名为规范名（不含版本号），与程序自更新后的命名保持一致
        if (dest.exists()) dest.delete()
        if (!tmp.renameTo(dest)) error("重命名二进制失败")

        // 赋予可执行权限：使用 Java API（Android 上无法直接调用 chmod 命令）
        val execOk = dest.setExecutable(true, false)
        if (!execOk) {
            // 兜底：尝试设置可读
            val readOk = dest.setReadable(true, false)
            error("设置可执行权限失败 (exec=$execOk, read=$readOk)")
        }

        // 记录本次成功安装的版本（持久化：杀后台重进也记得，避免重复下载）
        persistLastTag(context, tag)

        emit("[INFO] 下载完成并赋予可执行权限：${dest.absolutePath}")
        emit("[INFO] 已重命名为规范名：${dest.name}")
        emit("[INFO] 已安装版本：$tag")
        _binaryPath.value = dest.absolutePath
    }

    /** 以 --server 模式启动已安装的二进制。返回 true 表示启动流程已开始。 */
    fun startServer(context: Context): Boolean {
        isStopping = false
        if (_state.value == ServerState.RUNNING) {
            emit("[WARN] 服务已在运行")
            return true
        }
        val binary = binaryFile(context)
        if (!binary.exists()) {
            emit("[ERR ] 二进制未安装，请先点击「获取 Web」")
            _state.value = ServerState.ERROR
            return false
        }
        // 确保有可执行目录（启动时再次确认，避免 install 后被清理）
        val workDir = _execDir.value ?: context.codeCacheDir
        refreshLanUrl()
        _state.value = ServerState.RUNNING

        runJob = scope.launch {
            try {
                val p = port
                emit("[INFO] 启动 Web 服务...")
                if (isUsingCustomWeb(context)) {
                    // 终端启动流程提示：包含用户选择的实际文件名
                    val fileName = CustomWebStore.name.value.ifBlank { "未知" }
                    emit("[INFO] 您现在使用的是自定义Web，Web文件名为: $fileName")
                }
                emit("[INFO] 二进制路径：${binary.absolutePath}")
                emit("[INFO] 工作目录：${workDir.absolutePath}")
                emit("[INFO] 监听地址：0.0.0.0:$p")
                emit("[INFO] 本机访问：${_localUrl.value}")
                val lan = _lanUrl.value
                if (lan.isNotEmpty()) {
                    emit("[INFO] 局域网访问：$lan")
                } else {
                    emit("[WARN] 未获取到局域网 IP，请连接 Wi-Fi")
                }

                val cmd = mutableListOf(binary.absolutePath, "--server")
                // 参考 Tomato-Novel-Downloader 文档：通过 --password 参数或 TOMATO_WEB_PASSWORD 环境变量
                // 启用密码锁。两种方式同时传入以适配不同版本。
                val pw = PasswordStore.password.value
                if (pw != null) {
                    cmd.add("--password")
                    cmd.add(pw)
                    emit("[INFO] 已启用访问密码保护")
                }
                emit("[INFO] 请将本应用小窗化悬浮于浏览器之上，否则搜索可能无响应。")

                val pb = ProcessBuilder(cmd)
                    .directory(workDir)
                    .redirectErrorStream(true)
                pb.environment()["TOMATO_WEB_ADDR"] = "0.0.0.0:$p"
                if (pw != null) pb.environment()["TOMATO_WEB_PASSWORD"] = pw

                val proc = pb.start()
                process = proc

                val reader = proc.inputStream.bufferedReader()
                var line = reader.readLine()
                while (line != null) {
                    emit(line)
                    line = reader.readLine()
                }
                val exit = proc.waitFor()
                emit("[INFO] 进程退出，代码：$exit")
            } catch (e: Exception) {
                // 主动关闭时 destroyForcibly + cancel 会触发中断异常，不要误判为启动失败
                if (!isStopping) {
                    emit("[ERR ] 启动失败：${e.message}")
                    emit("[ERR ] 若提示 Permission denied，说明该目录不可执行，请反馈日志中的路径")
                    _state.value = ServerState.ERROR
                }
            } finally {
                process = null
                isStopping = false
                if (_state.value == ServerState.RUNNING) {
                    _state.value = ServerState.STOPPED
                }
            }
        }
        return true
    }

    fun stop() {
        isStopping = true
        process?.destroyForcibly()
        process = null
        runJob?.cancel()
        runJob = null
        if (_state.value == ServerState.RUNNING || _state.value == ServerState.ERROR) {
            _state.value = ServerState.STOPPED
        }
        emit("[INFO] 已停止服务")
    }

    private fun fetchLatestTag(): String {
        val url = URL("https://api.github.com/repos/$REPO/releases/latest")
        val conn = (url.openConnection() as HttpURLConnection).apply {
            requestMethod = "GET"
            setRequestProperty("User-Agent", "TomatoWebLauncher")
            connectTimeout = 20000
            readTimeout = 20000
            instanceFollowRedirects = true
        }
        try {
            val body = conn.inputStream.bufferedReader().use { it.readText() }
            val regex = Regex("\"tag_name\"\\s*:\\s*\"([^\"]+)\"")
            return regex.find(body)?.groupValues?.get(1)
                ?: error("无法从 GitHub API 获取 tag_name")
        } finally {
            conn.disconnect()
        }
    }

    private fun download(urlStr: String, dest: File, onProgress: (Long, Long) -> Unit) {
        val conn = (URL(urlStr).openConnection() as HttpURLConnection).apply {
            instanceFollowRedirects = true
            connectTimeout = 30000
            readTimeout = 60000
        }
        try {
            val total = conn.contentLengthLong
            conn.inputStream.use { input ->
                dest.outputStream().use { output ->
                    val buf = ByteArray(8192)
                    var n: Int
                    var read = 0L
                    while (input.read(buf).also { n = it } > 0) {
                        output.write(buf, 0, n)
                        read += n
                        onProgress(read, total)
                    }
                }
            }
        } finally {
            conn.disconnect()
        }
    }

    private fun fmt(bytes: Long): String =
        String.format("%.1f MB", bytes / 1024.0 / 1024.0)
}
