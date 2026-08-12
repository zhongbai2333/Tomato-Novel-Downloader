package com.tomato.downloader.core

import java.net.Inet4Address
import java.net.NetworkInterface

object NetworkUtils {

    /** 获取设备局域网 IPv4 地址（排除回环），未连接时返回 null。 */
    fun getLanIp(): String? {
        return try {
            val ifaces = NetworkInterface.getNetworkInterfaces()
            for (iface in ifaces) {
                if (!iface.isUp || iface.isLoopback) continue
                val addrs = iface.inetAddresses
                while (addrs.hasMoreElements()) {
                    val addr = addrs.nextElement()
                    if (addr is Inet4Address && !addr.isLoopbackAddress) {
                        return addr.hostAddress
                    }
                }
            }
            null
        } catch (e: Exception) {
            null
        }
    }
}
