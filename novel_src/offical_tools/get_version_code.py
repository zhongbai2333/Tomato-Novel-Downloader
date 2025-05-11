import re
import requests
import urllib3
from urllib.parse import urlparse

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
requests.packages.urllib3.disable_warnings()

SHORT_URL = "https://fqnovel.ugurl.cn/ncX3"
TIMEOUT = 10  # 秒

code = None

class GetVersionCode(object):
    @staticmethod
    def _extract_filename_from_disposition(header: str) -> str | None:
        """
        解析 Content-Disposition，兼容 RFC2231 filename*=utf-8''xxx.apk 的写法
        """
        if not header:
            return None

        # filename*=utf-8''fqnovel_7.55.0_75500.apk
        m = re.search(r"filename\*\s*=\s*[^'']+''([^;]+)", header, flags=re.I)
        if m:
            return requests.utils.unquote(m.group(1))

        # filename="fqnovel_7.55.0_75500.apk"
        m = re.search(r'filename\s*=\s*"([^"]+)"', header, flags=re.I)
        if m:
            return m.group(1)

        # filename=fqnovel_7.55.0_75500.apk
        m = re.search(r"filename\s*=\s*([^;]+)", header, flags=re.I)
        if m:
            return m.group(1).strip().strip('"')

        return None

    @staticmethod
    def _get_apk_filename(short_url: str = SHORT_URL) -> str:
        """
        主函数：返回 APK 名，若失败抛出异常
        """
        # 用 HEAD 就足够拿到 header；如果服务端不支持 HEAD 可改用 GET，并加上 stream=True
        resp = requests.get(short_url, stream=True, allow_redirects=True, timeout=TIMEOUT, verify=False)
        resp.close()  # 立刻关闭，不真正下载

        # 尝试从 Content-Disposition 提取
        cdisp = resp.headers.get("Content-Disposition", "")
        filename = GetVersionCode._extract_filename_from_disposition(cdisp)

        if not filename:
            # 没有 Content-Disposition，就取最终 URL 的 path
            path = urlparse(resp.url).path
            filename = path.split("/")[-1]

        if not filename.lower().endswith(".apk"):
            raise RuntimeError(f"无法确定 APK 文件名，得到的是: {filename!r}")

        return filename

    @staticmethod
    def get() -> str:
        """获取VersionCode"""
        global code
        try:
            if code is None:
                apk_name = GetVersionCode._get_apk_filename()
                code = re.search(r"_v\d+_(\d+)_", apk_name).group(1)
            if code:
                return code
            return None
        except Exception as exc:
            return None
