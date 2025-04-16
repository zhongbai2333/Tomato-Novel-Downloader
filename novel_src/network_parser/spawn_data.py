#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import uuid
import random
import json
import time
import string
import requests
import base64

from .TTEncrypt import TT


# -------------------- 工具函数 --------------------
def generate_uuid():
    """生成 UUID4 字符串"""
    return str(uuid.uuid4())


def current_millis():
    """返回当前毫秒级时间戳"""
    return int(time.time() * 1000)


def get_custom_bt():
    """
    模拟 Android 中的 m25453o() 方法，计算设备开机时间戳：
      custom_bt = current_millis - elapsed_ms_since_boot
    首先尝试通过 /proc/uptime 获取设备已运行秒数（适用于 Linux），
    若失败则退而采用 time.monotonic() 作为备用。
    返回字符串形式的毫秒数。
    """
    try:
        with open("/proc/uptime", "r") as f:
            uptime_seconds = float(f.readline().split()[0])
    except Exception:
        uptime_seconds = time.monotonic()
    return str(current_millis() - int(uptime_seconds * 1000))


# -------------------- 各部分数据生成函数 --------------------
def generate_device_info():
    """
    随机生成设备及操作系统信息、唯一标识和安全参数。
    """
    os_name = "Android"  # 固定为 Android
    os_version = random.choice(["9", "10", "11", "12"])
    os_api = random.choice([28, 29, 30, 31])
    device_model = random.choice(["RMX1931", "MI 9", "Honor 10", "P30", "V1921A"])
    device_brand = random.choice(["realme", "Xiaomi", "Huawei", "OPPO", "vivo"])
    cpu_abi = random.choice(["armeabi-v7a", "arm64-v8a", "x86_64"])
    release_build = "".join(random.choices("0123456789abcdef", k=7))
    # 使用 UUID4 生成 cdid、req_id 和 clientudid
    cdid = generate_uuid()
    req_id = generate_uuid()
    clientudid = generate_uuid()
    # 生成 128 位随机数转换为 32 位十六进制字符串作为 sig_hash
    sig_hash = f"{random.getrandbits(128):032x}"
    # 生成 openudid：80 位随机数转换为 20 位十六进制字符串
    openudid = f"{random.getrandbits(80):020x}"

    return {
        "os": os_name,
        "os_version": os_version,
        "os_api": os_api,
        "device_model": device_model,
        "device_brand": device_brand,
        "device_manufacturer": device_brand,
        "cpu_abi": cpu_abi,
        "release_build": release_build,
        "cdid": cdid,
        "sig_hash": sig_hash,
        "openudid": openudid,
        "clientudid": clientudid,
        "req_id": req_id,
    }


def generate_optional_fields():
    """
    随机生成 ROM 信息、rom_version、_gen_time 以及 APK 首次安装时间。
    """
    rom = "".join(random.choices(string.ascii_uppercase + string.digits, k=14))
    part1 = "".join(random.choices(string.ascii_uppercase + string.digits, k=4))
    part2 = "".join(random.choices(string.digits, k=6))
    part3 = "".join(random.choices(string.digits, k=8))
    rom_version = f"coloros__{part1}.{part2}.{part3} release-keys"
    _gen_time = current_millis()
    one_day_ms = 86400000
    offset = random.randint(0, 365) * one_day_ms
    apk_first_install_time = _gen_time - offset
    return {
        "rom": rom,
        "rom_version": rom_version,
        "apk_first_install_time": apk_first_install_time,
    }, {"_gen_time": _gen_time}


def generate_ipv6_address_link_local():
    """
    生成链路本地 IPv6 地址，格式类似："FE80::XXXX:XXXX:XXXX:XXXX"，
    其中每组数字用大写十六进制表示，不强制补零。
    """
    groups = [format(random.randint(0, 0xFFFF), "X") for _ in range(4)]
    return "FE80::" + ":".join(groups)


def generate_ipv6_address_unique_local():
    """
    生成唯一本地 IPv6 地址（Unique Local Address），第一组以 FD 开头。
    """
    first_group = "FD" + format(random.randint(0, 0xFF), "02X")
    groups = [first_group] + [format(random.randint(0, 0xFFFF), "X") for _ in range(7)]
    return ":".join(groups)


def generate_ipv6_list():
    """
    生成 ipv6_list 字段，共包含 3 条 IPv6 地址记录，均为 type "client_anpi"。
    """
    return [
        {"type": "client_anpi", "value": generate_ipv6_address_link_local()},
        {"type": "client_anpi", "value": generate_ipv6_address_unique_local()},
        {"type": "client_anpi", "value": generate_ipv6_address_unique_local()},
    ]


def generate_full_request_body():
    """
    组合生成完整的请求体数据，包括基本信息、设备信息、版本信息、可选字段和 IPv6 列表。
    """
    # 固定部分
    raw_data = {
        "magic_tag": "ss_app_log",
        "header": {
            "display_name": "番茄免费小说",
            "aid": 1967,
            "channel": "43536163a",
            "package": "com.dragon.read",
            "sdk_version": "3.7.0-rc.25-fanqie-xiaoshuo",
            "sdk_target_version": 29,
            "git_hash": "711d1a7",
            "density_dpi": 240,
            "display_density": "hdpi",
            "resolution": "720x1280",
            "language": "zh",
            "timezone": 8,
            "access": "wifi",
            "not_request_sender": 0,
            "carrier": "CHINA MOBILE",
            "mcc_mnc": "46000",
            "region": "CN",
            "tz_name": "Asia/Shanghai",
            "tz_offset": 28800,
            "sim_region": "cn",
            "sim_serial_number": [],
            "oaid_may_support": False,
            "device_platform": "android",
            "custom": {"host_bit": 32, "dragon_device_type": "pad"},
            "pre_installed_channel": "",
            "is_system_app": 0,
            "sdk_flavor": "china",
            "guest_mode": 0,
        },
    }
    device_info = generate_device_info()
    optional_fields, _gen_time = generate_optional_fields()
    ipv6_list = generate_ipv6_list()
    app_version = "6.6.7.32"
    app_code = app_version.replace(".", "")
    version_data = {
        "app_version_minor": app_version,
        "app_version": app_version,
        "update_version_code": app_code,
        "manifest_version_code": app_code,
        "version_code": app_code,
    }

    raw_data["header"].update(device_info)
    raw_data["header"].update(version_data)
    raw_data["header"].update(optional_fields)
    raw_data["header"]["ipv6_list"] = ipv6_list

    # 合并 _gen_time 到顶层
    raw_data.update(_gen_time)
    return raw_data


# -------------------- tt_info 生成及请求相关 --------------------
# 定义目标 tt_info 字典中需要的 key 及默认样板（对于 normal_session_id 和 cold_start_session_id 始终使用新生成的 uuid）
TARGET_KEYS = [
    "timezone",
    "carrier",
    "mcc_mnc",
    "sim_region",
    "custom_bt",
    "app_version_minor",
    "ac",
    "channel",
    "aid",
    "app_name",
    "version_code",
    "version_name",
    "device_platform",
    "os",
    "ssmix",
    "device_type",
    "device_brand",
    "language",
    "os_api",
    "os_version",
    "openudid",
    "manifest_version_code",
    "resolution",
    "dpi",
    "update_version_code",
    "_rticket",
    "normal_session_cnt_in_day",
    "gender",
    "cold_start_session_cnt_in_day",
    "host_abi",
    "dragon_device_type",
    "sys_mini_window",
    "pv_player",
    "app_mini_window",
    "normal_session_id",
    "compliance_status",
    "har_status",
    "cold_start_session_id",
    "cold_start_session_cnt_in_life",
    "charging",
    "normal_session_cnt_in_life",
    "is_power_save_mode",
    "app_dark_mode",
    "screen_brightness",
    "battery_pct",
    "down_speed",
    "sys_dark_mode",
    "need_personal_recommend",
    "player_so_load",
    "font_scale",
    "is_android_pad_screen",
    "network_type",
    "rom_version",
    "current_volume",
    "cdid",
    "req_id",
]

DEFAULT_TEMPLATE = {
    "timezone": "8.0",
    "carrier": "CHINA MOBILE",
    "mcc_mnc": "46000",
    "sim_region": "cn",
    "app_version_minor": "6.6.7.32",
    "ac": "wifi",
    "channel": "43536163a",
    "aid": "1967",
    "app_name": "novelapp",
    "version_code": "66732",
    "version_name": "6.6.7.32",
    "device_platform": "android",
    "os": "android",
    "ssmix": "a",
    "normal_session_cnt_in_day": "0",
    "gender": "2",
    "cold_start_session_cnt_in_day": "1",
    "host_abi": "armeabi-v7a",
    "dragon_device_type": "pad",
    "sys_mini_window": "0",
    "pv_player": "66732",
    "app_mini_window": "0",
    "compliance_status": "0",
    "har_status": "0",
    "cold_start_session_cnt_in_life": "1",
    "charging": "0",
    "normal_session_cnt_in_life": "1",
    "is_power_save_mode": "0",
    "app_dark_mode": "0",
    "screen_brightness": "1",
    "battery_pct": "0",
    "down_speed": "1048576",
    "sys_dark_mode": "0",
    "need_personal_recommend": "1",
    "player_so_load": "1",
    "font_scale": "100",
    "is_android_pad_screen": "1",
    "network_type": "4",
    "resolution": "1280*720",
    "dpi": "240",
    "update_version_code": "66732",
}


def generate_tt_info_from_dict(input_data):
    """
    根据输入 dict 生成 tt_info 字典。
    对于 header 中提供的值直接覆盖默认样板，以避免样板默认值的干扰；
    特殊字段如 normal_session_id、cold_start_session_id 均自动生成；
    _rticket 和 custom_bt 使用当前值生成。
    """
    header = input_data.get("header", {})
    output = DEFAULT_TEMPLATE.copy()

    # 覆盖 header 中存在的键（仅处理 TARGET_KEYS 中的字段，且保证转换为字符串）
    for key in TARGET_KEYS:
        if key in header:
            # 针对 timezone 数字转换为 "数字.0"
            if key == "timezone" and isinstance(header[key], (int, float)):
                output[key] = f"{header[key]}.0"
            else:
                output[key] = str(header[key])

    # 特殊生成项：normal_session_id 与 cold_start_session_id 使用新 uuid
    output["normal_session_id"] = generate_uuid()
    output["cold_start_session_id"] = generate_uuid()

    # _rticket 使用当前毫秒级时间戳生成
    output["_rticket"] = str(current_millis())
    # custom_bt 使用 get_custom_bt()
    output["custom_bt"] = get_custom_bt()

    # 如果 header 中包含 custom 字段里的 dragon_device_type，则使用之
    custom = header.get("custom", {})
    if "dragon_device_type" in custom:
        output["dragon_device_type"] = str(custom["dragon_device_type"])

    # 对于 rom_version，若 header 提供则处理：去掉 "coloros__" 前缀后使用
    if "rom_version" in header:
        rom = str(header["rom_version"])
        if rom.startswith("coloros__"):
            rom = rom.replace("coloros__", "", 1)
        output["rom_version"] = rom

    return output


def build_query_string(params):
    """
    根据字典 params 生成 URL 查询字符串
    """
    return "&".join(f"{key}={value}" for key, value in params.items())


def activate_install(install_id, tt_info):
    """
    使用激活 install_id 的 GET 请求进行激活测试。
    """
    params = {"aid": "1967", "tt_info": tt_info}
    url = "https://log.snssdk.com/service/2/app_alert_check/"
    headers = {
        "Accept-Encoding": "gzip",
        "X-SS-REQ-TICKET": str(current_millis()),
        "x-vc-bdturing-sdk-version": "3.7.2.cn",
        "sdk-version": "2",
        "passport-sdk-version": "50564",
        "User-Agent": (
            "com.dragon.read/66732 (Linux; U; Android 9; zh_CN; RMX1931; "
            "Build/PQ3B.190801.04011825;tt-ok/3.12.13.4-tiktok)"
        ),
        "Host": "ichannel.snssdk.com",
        "Connection": "Keep-Alive",
        "Cookie": f"install_id={install_id}; store-region=cn-zj; store-region-src=did",
    }
    try:
        response = requests.get(url, params=params, headers=headers)
        return response.status_code
    except Exception as e:
        print(f"activate_install error: {e}")
        return None


# -------------------- 主流程 --------------------
def main():
    while True:
        # 生成完整请求体数据
        data = generate_full_request_body()
        # 根据生成的数据构建 tt_info（使用优化后函数）
        tt_info_dict = generate_tt_info_from_dict(data)
        tt_info_str = build_query_string(tt_info_dict)

        # 同时生成 JSON 数据（本次请求体完整数据）
        json_data = json.dumps(data, ensure_ascii=False)  # 去除 indent 可减小数据量

        # 使用 TTEncrypt 模块进行加密
        try:
            tt = TT()
            encrypted_json = tt.encrypt(json_data)
            encrypted_tt_info = tt.encrypt(tt_info_str)
            # 使用 URL-safe 的 base64 编码
            encoded_tt_info = base64.b64encode(encrypted_tt_info, altchars=b"-_")
        except Exception as e:
            print(f"Encryption error: {e}")
            continue

        # 设置请求参数和请求头
        url = "https://log.snssdk.com/service/2/device_register/?tt_data=a"
        headers = {
            "accept-encoding": "gzip",
            "log-encode-type": "gzip",
            "x-ss-req-ticket": str(current_millis()),
            "x-vc-bdturing-sdk-version": "3.7.2.cn",
            "sdk-version": "2",
            "passport-sdk-version": "50564",
            "user-agent": (
                "com.dragon.read/66732 (Linux; U; Android 9; zh_CN; RMX1931; "
                "Build/PQ3B.190801.04011825;tt-ok/3.12.13.4-tiktok)"
            ),
            "x-neptune": "-8|50:51:59:20:21:30:40:47:49:39:22:29",
            "content-type": "application/octet-stream;tt-data=a",
        }

        try:
            response = requests.post(url, headers=headers, data=encrypted_json)
            res_data = response.json()
        except Exception as e:
            print(f"POST request error: {e}")
            time.sleep(1)
            continue

        # 检查返回数据，并计算激活条件：
        try:
            if (
                res_data.get("install_id")
                and int(res_data.get("install_id")) - int(res_data.get("device_id"))
                == 4096
            ):
                code = activate_install(res_data.get("install_id_str"), encoded_tt_info)
                if code == 200:
                    print("Activation success:", res_data.get("install_id"))
                    return res_data.get("install_id")
        except Exception as e:
            print(f"Response processing error: {e}")

        # 休眠 1 秒后重试
        time.sleep(1)
