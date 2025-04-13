import base64
import json
import gzip
import urllib3
import requests
from Crypto.Cipher import AES
from Crypto.Util.Padding import pad, unpad
from Crypto.Random import get_random_bytes
from typing import Tuple

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
requests.packages.urllib3.disable_warnings()

# 固定密钥分段，组合后形成16字节密钥（AES-128使用）
STATIC_KEY_PARTS = ["ac25", "c67d", "dd8f", "38c1", "b37a", "2348", "828e", "222e"]

# 基础 API URL（根据需求可以进一步封装）
API_BASE_URL = "https://api5-normal-sinfonlineb.fqnovel.com/reading"

INSTALL_ID = "4343550757889306"
AID = "1967"
UPDATE_VERSION_CODE = "66732"


def get_static_key():
    """
    组合固定的密钥分段，返回一个16字节的十六进制字符串
    """
    return "".join(STATIC_KEY_PARTS)


class FqCrypto:
    """
    封装 AES 加解密操作，使用AES-CBC模式和PKCS7填充
    """

    def __init__(self, key_hex: str):
        self.key = bytes.fromhex(key_hex)
        if len(self.key) != 16:
            raise ValueError(f"Key length mismatch! key: {self.key.hex()}")
        self.cipher_mode = AES.MODE_CBC

    def encrypt(self, data: bytes, iv: bytes) -> bytes:
        cipher = AES.new(self.key, self.cipher_mode, iv)
        ct_bytes = cipher.encrypt(pad(data, AES.block_size))
        return ct_bytes

    def decrypt(self, data: bytes) -> bytes:
        iv = data[:16]
        ct = data[16:]
        cipher = AES.new(self.key, self.cipher_mode, iv)
        pt = unpad(cipher.decrypt(ct), AES.block_size)
        return pt

    def new_register_key_content(self, server_device_id: str, str_val: str) -> str:
        """
        根据 server_device_id 和 str_val 生成加密后的注册 key 内容。
        要求这两个参数必须为纯数字字符串，转换为8字节小端字节后拼接为16字节原始数据，
        然后使用随机生成的IV和AES-CBC进行加密，最后将IV和密文拼接后Base64编码返回。
        """
        if not str_val.isdigit() or not server_device_id.isdigit():
            raise ValueError(
                f"Parse failed\nserver_device_id: {server_device_id}\nstr_val:{str_val}"
            )
        # 将两个数字各转换为8字节小端字节后拼接
        combined_bytes = int(server_device_id).to_bytes(8, byteorder="little") + int(
            str_val
        ).to_bytes(8, byteorder="little")
        iv = get_random_bytes(16)
        enc_data = self.encrypt(combined_bytes, iv)
        result = iv + enc_data
        return base64.b64encode(result).decode("utf-8")


class FqVariable:
    """
    存储设备注册相关的变量
    """

    def __init__(
        self, install_id: str, server_device_id: str, aid: str, update_version_code: str
    ):
        self.install_id = install_id
        self.server_device_id = server_device_id
        self.aid = aid
        self.update_version_code = update_version_code


class FqReq:
    """
    用于发起网络请求，获取和解密 APK 返回的关键信息
    """

    def __init__(self, fq_var: FqVariable):
        self.var = fq_var
        self.session = requests.Session()

    def batch_get(self, item_ids: str, download: bool = False) -> dict:
        """
        发送批量获取请求，返回JSON数据
        """
        headers = {"Cookie": f"install_id={self.var.install_id}"}
        url = f"{API_BASE_URL}/reader/batch_full/v"
        params = {
            "item_ids": item_ids,
            "req_type": "0" if download else "1",
            "aid": self.var.aid,
            "update_version_code": self.var.update_version_code,
        }
        response = self.session.get(url, headers=headers, params=params, verify=False)
        response.raise_for_status()
        return response.json()

    def get_register_key(self) -> str:
        """
        发送注册 key 请求，并解密返回的数据，返回解密后的 key（十六进制字符串）
        """
        headers = {
            "Cookie": f"install_id={self.var.install_id}",
            "Content-Type": "application/json",
        }
        url = f"{API_BASE_URL}/crypt/registerkey"
        params = {"aid": self.var.aid}
        crypto = FqCrypto(get_static_key())
        payload = json.dumps(
            {
                "content": crypto.new_register_key_content(
                    self.var.server_device_id, "0"
                ),
                "keyver": 1,
            }
        ).encode("utf-8")
        response = self.session.post(
            url, headers=headers, params=params, data=payload, verify=False
        )
        response.raise_for_status()
        ret = response.json()
        key_str = ret["data"]["key"]
        decrypted_key = crypto.decrypt(base64.b64decode(key_str))
        return decrypted_key.hex()

    def get_decrypt_contents(self, res_arr: dict) -> dict:
        """
        根据批量获取返回的数据，利用注册 key 解密其中每项数据
        """
        key = self.get_register_key()
        crypto = FqCrypto(key)
        for item_id, content in res_arr.get("data", {}).items():
            encrypted_content = content.get("content")
            if encrypted_content:
                byte_content = crypto.decrypt(base64.b64decode(encrypted_content))
                # 对解密后的数据进行 gzip 解压（如果数据经过压缩）
                try:
                    decompressed = gzip.decompress(byte_content).decode("utf-8")
                except Exception as e:
                    decompressed = byte_content.decode("utf-8")  # 如果未压缩则直接解码
                res_arr["data"][item_id]["content"] = decompressed
        return res_arr

    def close(self):
        """
        显式关闭网络会话，释放资源
        """
        self.session.close()

    def __del__(self):
        try:
            self.session.close()
        except Exception:
            pass

def download_chapter_official(chapter: str) -> dict:
    install_id = INSTALL_ID
    server_device_id = str(int(install_id) - 4096)
    aid = AID
    update_version_code = UPDATE_VERSION_CODE

    # 初始化变量和请求类
    fq_var = FqVariable(install_id, server_device_id, aid, update_version_code)
    fq_req = FqReq(fq_var)

    try:
        response_data = fq_req.batch_get(chapter)

        decrypted_data = fq_req.get_decrypt_contents(response_data)
        decrypted_data["data"] = decrypted_data["data"][chapter]
        return decrypted_data
    except Exception as ex:
        raise("发生异常:", ex)
    finally:
        fq_req.close()
