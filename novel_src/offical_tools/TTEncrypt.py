import base64
import hashlib
import os
import gzip
from Crypto.Cipher import AES
from Crypto.Util.Padding import pad, unpad

FIXED_STRING = base64.b64decode(
    "TdTC5rgxYgkOUrPHpnM7pByyRiuCmrWKGWs521cXdST0m69/COjWjSanLjfBqVovHwWlGJKu8pSXMrYqOKrdWA=="
)

def sha512_hash(data: bytes) -> bytes:
    """计算 SHA512 哈希值。"""
    return hashlib.sha512(data).digest()


def generate_random_bytes(size: int) -> bytes:
    """生成指定长度的随机字节。"""
    return os.urandom(size)


def gzip_compress(data: bytes) -> bytes:
    """使用 gzip 压缩数据。"""
    return gzip.compress(data)


def gzip_decompress(data: bytes) -> bytes:
    """使用 gzip 解压缩数据。"""
    return gzip.decompress(data)


def encrypt_data(data: bytes, key: bytes, iv: bytes) -> bytes:
    """使用 AES-CBC 模式加密数据。"""
    cipher = AES.new(key, AES.MODE_CBC, iv)
    padded_data = pad(data, AES.block_size)
    return cipher.encrypt(padded_data)


def decrypt_data(data: bytes, key: bytes, iv: bytes) -> bytes:
    """使用 AES-CBC 模式解密数据。"""
    cipher = AES.new(key, AES.MODE_CBC, iv)
    decrypted_data = cipher.decrypt(data)
    return unpad(decrypted_data, AES.block_size)


def encrypt(data: bytes) -> bytes:
    """
    加密函数，实现与给出的复杂代码相同的功能。

    :param data: 需要处理的原始数据。
    :return: 处理后的结果字节。
    """
    # 生成随机值
    random_value = generate_random_bytes(32)

    # 计算 SHA512 哈希值
    hash_value = sha512_hash(sha512_hash(random_value) + FIXED_STRING)

    # 提取 key 和 iv
    key = hash_value[:16]
    iv = hash_value[16:32]

    # 压缩数据并计算哈希值
    compressed_data = gzip_compress(data)
    hashed_data = sha512_hash(compressed_data) + compressed_data

    # 加密数据
    encrypted_data = encrypt_data(hashed_data, key, iv)

    # 返回结果
    return bytes([116, 99, 5, 16, 0, 0] + list(random_value) + list(encrypted_data))


def decrypt(encrypted_data: bytes) -> bytes:
    """
    解密函数，实现与加密过程相反的操作。

    :param encrypted_data: 加密后的数据。
    :return: 解密后的原始数据。
    """
    # 提取固定头、随机值和加密数据
    header = encrypted_data[:6]  # 固定头 [116, 99, 5, 16, 0, 0]
    random_value = encrypted_data[6:38]  # 随机值 (32 字节)
    encrypted_payload = encrypted_data[38:]  # 加密数据

    # 计算 SHA512 哈希值
    hash_value = sha512_hash(sha512_hash(random_value) + FIXED_STRING)

    # 提取 key 和 iv
    key = hash_value[:16]
    iv = hash_value[16:32]

    # 解密数据
    decrypted_data = decrypt_data(encrypted_payload, key, iv)

    # 分离哈希值和压缩数据
    hash_length = 64  # SHA512 哈希值的长度是 64 字节
    hashed_value = decrypted_data[:hash_length]
    compressed_data = decrypted_data[hash_length:]

    # 验证哈希值
    if sha512_hash(compressed_data) != hashed_value:
        raise ValueError("Verification fail!")

    # 解压缩数据
    original_data = gzip_decompress(compressed_data)

    return original_data

# Bridge
TTEncrypt = encrypt
class TT:
    def __init__(self):
        pass
    
    @staticmethod
    def encrypt(data: bytes) -> bytes:
        return encrypt(data)
    
    @staticmethod
    def decrypt(data: bytes) -> bytes:
        return decrypt(data)

if __name__ == "__main__":
    # 加密
    example_data = b"example data"
    encrypted_result = encrypt(example_data)
    print("加密结果:", encrypted_result)

    # 解密
    decrypted_result = decrypt(encrypted_result)
    print("解密结果:", decrypted_result.decode("utf-8"))
