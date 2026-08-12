#!/usr/bin/env python3
"""手动 V1 + V2 双签名
流程：
 1. 去除 APK 中任何已存在的签名
 2. zipalign -p 4
 3. apksigner 签 V2 方案
 4. 从已签好 V2 的 APK 中计算 JAR 签名（注意不修改 APK contents，以免破环 V2 签名）
    - JAR 签名规则：digest 每一个 entry，写入 META-INF/MANIFEST.MF
    - 然后写入 META-INF/CERT.SF（含每个 entry 的 digest of manifest line）
    - 最后写入 META-INF/CERT.RSA（CMS PKCS7 签名 CERT.SF）
关键：META-INF/*.MF/*.SF/*.RSA 中的文件 entry 不计入 V2 签名块覆盖的内容，
      所以 JAR 签名文件可以在 V2 签名之后「追加」，不会破环 V2。
"""
import os
import sys
import zipfile
import hashlib
import base64
import shutil
import subprocess
from pathlib import Path

SRC = Path("/workspace/project_src/TomatoWeb/app/build/outputs/apk/release/app-release.apk")
KS = Path("/workspace/project_src/TomatoWeb/林九思.jks")
KSPASS = "林九思"
ALIAS = "林九思"
JAVA_HOME = Path("/root/.local/share/mise/installs/java/17.0.2")
JAVA = JAVA_HOME / "bin/java"
JARSIGNER = JAVA_HOME / "bin/jarsigner"
KEYTOOL = JAVA_HOME / "bin/keytool"
BUILD_TOOLS = Path("/opt/android-sdk/build-tools/37.0.0")
APKSIGNER_JAR = BUILD_TOOLS / "lib/apksigner.jar"
ZIPALIGN = BUILD_TOOLS / "zipalign"

TMPDIR = Path(os.environ.get("TMPDIR", "/tmp")) / "dual_sign"
TMPDIR.mkdir(parents=True, exist_ok=True)

def run(*args, **kwargs):
    subprocess.check_call(list(args), **kwargs)

# ---- 1. 去除已有签名 ----
unsigned = TMPDIR / "unsigned.apk"
with zipfile.ZipFile(SRC, "r") as zin, zipfile.ZipFile(unsigned, "w", zipfile.ZIP_STORED) as zout:
    for info in zin.infolist():
        bname = os.path.basename(info.filename)
        parent = os.path.dirname(info.filename)
        if parent == "META-INF" and (
            bname.endswith(".MF") or bname.endswith(".SF")
            or bname.endswith(".RSA") or bname.endswith(".DSA")
            or bname.endswith(".EC") or bname.upper().startswith("SIG-")
        ):
            continue
        data = zin.read(info.filename)
        zout.writestr(info, data)
print(f"[1/5] stripped signatures → {unsigned} ({unsigned.stat().st_size//1024}KB)")

# ---- 2. zipalign ----
aligned = TMPDIR / "aligned.apk"
if aligned.exists(): aligned.unlink()
run(str(ZIPALIGN), "-f", "-p", "4", str(unsigned), str(aligned))
print(f"[2/5] zipaligned → {aligned}")

# ---- 3. apksigner 签 V2 (minSdkVersion=1 也没用；我们只要 V2) ----
v2only = TMPDIR / "v2only.apk"
if v2only.exists(): v2only.unlink()
run(str(JAVA), "-jar", str(APKSIGNER_JAR), "sign",
    "--ks", str(KS), "--ks-pass", f"pass:{KSPASS}",
    "--ks-key-alias", ALIAS, "--key-pass", f"pass:{KSPASS}",
    "--min-sdk-version", "1", "--max-sdk-version", "100",
    "--v1-signing-enabled", "false",
    "--v2-signing-enabled", "true",
    "--v3-signing-enabled", "false",
    "--out", str(v2only), str(aligned))
print(f"[3/5] apksigner v2 signed → {v2only}")

# ---- 4. jarsigner 签 V1 (JAR) 追加到 V2 上：----
# jarsigner 会对内容做 re-arrange？如果直接签 V2 签好的 APK，可能破坏 V2。
# 验证：直接尝试 jarsigner 到 v2only，然后同时检查 V1 和 V2
v1v2 = TMPDIR / "v1v2.apk"
shutil.copy2(v2only, v1v2)
run(str(JARSIGNER), "-sigalg", "SHA256withRSA", "-digestalg", "SHA-256",
    "-keystore", str(KS), "-storepass", KSPASS, "-keypass", KSPASS,
    "-signedjar", str(v1v2), str(v1v2), ALIAS,
    stdout=subprocess.DEVNULL)
print(f"[4/5] jarsigner v1 appended → {v1v2}")

# ---- 5. verify ----
print("[5/5] verify:")
run(str(JAVA), "-jar", str(APKSIGNER_JAR), "verify", "--verbose", str(v1v2))

# 替换最终产物
final = SRC
shutil.copy2(v1v2, final)
print(f"\nOK → {final} ({final.stat().st_size//1024}KB)")
