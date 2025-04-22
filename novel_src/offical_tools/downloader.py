import base64
import gzip
import json
import time
import urllib3
from typing import Dict

import requests
from Crypto.Cipher import AES
from Crypto.Random import get_random_bytes
from Crypto.Util.Padding import pad, unpad

from ..base_system.context import GlobalContext
from .get_iid import get_iid
from .get_version_code import GetVersionCode

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
requests.packages.urllib3.disable_warnings()

# ===== 常量 =====
STATIC_KEY_PARTS = ["ac25", "c67d", "dd8f", "38c1", "b37a", "2348", "828e", "222e"]
API_BASE_URL = "https://api5-normal-sinfonlineb.fqnovel.com/reading"
AID = "1967"
_IID_EXPIRE_SEC = 12 * 60 * 60  # 12 小时


def _ensure_fresh_iid() -> None:
    cfg = GlobalContext.get_config()
    log = GlobalContext.get_logger()
    now_ms = int(time.time() * 1000)
    spawn_ms = int(getattr(cfg, "iid_spawn_time", "0") or "0")
    if not getattr(cfg, "iid", None) or (now_ms - spawn_ms) // 1000 >= _IID_EXPIRE_SEC:
        log.info("iid 已使用超过 12 小时或不存在，自动更换 …")
        cfg.iid = str(get_iid())
        cfg.iid_spawn_time = str(now_ms)
        cfg.save()


def get_static_key() -> str:
    return "".join(STATIC_KEY_PARTS)


class FqCrypto:
    def __init__(self, key_hex: str):
        self.key = bytes.fromhex(key_hex)
        if len(self.key) != 16:
            raise ValueError(f"Key length mismatch: {self.key.hex()}")
        self.mode = AES.MODE_CBC

    def encrypt(self, data: bytes, iv: bytes) -> bytes:
        return AES.new(self.key, self.mode, iv).encrypt(pad(data, AES.block_size))

    def decrypt(self, data: bytes) -> bytes:
        iv, ct = data[:16], data[16:]
        return unpad(AES.new(self.key, self.mode, iv).decrypt(ct), AES.block_size)

    def build_register_content(self, server_device_id: str, str_val: str) -> str:
        if not server_device_id.isdigit() or not str_val.isdigit():
            raise ValueError("server_device_id 和 str_val 必须为纯数字")
        raw = int(server_device_id).to_bytes(8, "little") + int(str_val).to_bytes(
            8, "little"
        )
        iv = get_random_bytes(16)
        return base64.b64encode(iv + self.encrypt(raw, iv)).decode()


class FqVariable:
    def __init__(
        self, install_id: str, server_device_id: str, aid: str, update_version_code: str
    ):
        self.install_id = install_id
        self.server_device_id = server_device_id
        self.aid = aid
        self.update_version_code = update_version_code


class FqReq:
    def __init__(self, fq_var: FqVariable, *, timeout: int = 10):
        self.var = fq_var
        self._timeout = timeout
        self.session = requests.Session()
        self._crypto = None
        self._key_version = None
        self._fetch_register_key()

    def get_contents(self, chapter_ids: str) -> Dict:
        raw = self._batch_fetch(chapter_ids)
        first = next(iter(raw.get("data", {}).values()), {})
        self._ensure_key_version(first.get("key_version"))
        return self._decrypt_contents(raw)

    def _batch_fetch(self, item_ids: str) -> Dict:
        params = {
            "item_ids": item_ids,
            "update_version_code": self.var.update_version_code,
            "aid": self.var.aid,
            "key_register_ts": "0",
        }
        headers = {"Cookie": f"install_id={self.var.install_id}"}
        r = self.session.get(
            f"{API_BASE_URL}/reader/batch_full/v",
            headers=headers,
            params=params,
            timeout=self._timeout,
            verify=False,
        )
        r.raise_for_status()
        return r.json()

    def _fetch_register_key(self) -> None:
        static_crypto = FqCrypto(get_static_key())
        payload = {
            "content": static_crypto.build_register_content(
                self.var.server_device_id, "0"
            ),
            "keyver": 1,
        }
        r = self.session.post(
            f"{API_BASE_URL}/crypt/registerkey",
            headers={
                "Cookie": f"install_id={self.var.install_id}",
                "Content-Type": "application/json",
            },
            params={"aid": self.var.aid},
            data=json.dumps(payload).encode(),
            timeout=self._timeout,
            verify=False,
        )
        r.raise_for_status()
        data = r.json()["data"]
        self._key_version = data["keyver"]
        key_hex = static_crypto.decrypt(base64.b64decode(data["key"]))
        self._crypto = FqCrypto(key_hex.hex())

    def _ensure_key_version(self, expected):
        if expected is not None and expected != self._key_version:
            self._fetch_register_key()

    def _decrypt_contents(self, res: Dict) -> Dict:
        if not self._crypto:
            raise RuntimeError("register key 尚未初始化")
        for cid, info in res.get("data", {}).items():
            enc = info.get("content")
            if not enc:
                continue
            raw = self._crypto.decrypt(base64.b64decode(enc))
            try:
                info["content"] = gzip.decompress(raw).decode()
            except:
                info["content"] = raw.decode()
        return res

    def close(self):
        self.session.close()


def search_api(book_name: str) -> dict:
    _ensure_fresh_iid()
    headers = {
        "cookie": "install_id=1229734607899353;",
    }
    api = "https://api-lf.fanqiesdk.com/api/novel/channel/homepage/search/search/v1/"
    params = {
        "offset": "0",
        "aid": "1967",
        "q": book_name,
    }

    response = requests.get(api, params=params, headers=headers, verify=False)

    response.raise_for_status()

    return response.json().get("data", {}).get("ret_data", {})


def download_chapter_official(chapter_id: str) -> Dict:
    _ensure_fresh_iid()
    cfg = GlobalContext.get_config()
    var = FqVariable(
        install_id=cfg.iid,
        server_device_id=str(int(cfg.iid) - 4096),
        aid=AID,
        update_version_code=GetVersionCode.get(),
    )
    fq = FqReq(var)
    try:
        data = fq.get_contents(chapter_id)
        return data
    finally:
        fq.close()


def spawn_iid() -> None:
    """自动生成 install_id 并验证能否正常取正文"""
    cfg = GlobalContext.get_config()
    log = GlobalContext.get_logger()

    _ensure_fresh_iid()  # ← 自动确保有可用 iid
    log.info("当前 iid=%s", cfg.iid)

    fq_var = FqVariable(
        install_id=cfg.iid,
        server_device_id=str(int(cfg.iid) - 4096),
        aid=AID,
        update_version_code=GetVersionCode.get(),
    )
    fq_req = FqReq(fq_var)
    try:
        # 随便请求一章测试
        for attempt in range(1, 6):
            try:
                fq_req.get_contents("7310102404588896783")
                log.info(f"验证成功（第 {attempt} 次）")
                break
            except Exception as e:
                log.warning(f"验证失败（第 {attempt} 次）：{e}")
                time.sleep(0.3)
        else:
            raise RuntimeError("连续验证失败，请检查网络或算法")
    finally:
        fq_req.close()
    log.info("iid 获取并验证成功！")
