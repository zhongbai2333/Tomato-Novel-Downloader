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
from fanqie_mod import get_iid

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)
requests.packages.urllib3.disable_warnings()


def search_api(book_name: str) -> dict:
    headers = {
        "cookie": f"install_id={get_iid()}",
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
