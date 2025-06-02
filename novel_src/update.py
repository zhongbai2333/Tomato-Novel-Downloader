import sys
import os
import shutil
import tempfile
import hashlib
import platform
import subprocess
import requests
from pathlib import Path
from typing import Optional, Dict, Any

from .constants import VERSION
from .base_system.context import GlobalContext


class UpdateManager:
    """
    …（其余注释同前面示例，不再重复）…
    """

    def __init__(self):
        self.owner = "zhongbai2333"
        self.repo = "Tomato-Novel-Downloader"
        self.local_version = f"v{VERSION}"
        # local_executable 必须包含版本号，例如:
        # "/usr/local/bin/TomatoNovelDownloader-Linux_amd64-v1.5.1"
        # "C:\\Program Files\\TomatoNovelDownloader\\TomatoNovelDownloader-Win64-v1.5.1.exe"
        self.local_executable = Path(sys.argv[0]).resolve()
        self.logger = GlobalContext.get_logger()
        self.debug = GlobalContext.get_log_system().debug

    @staticmethod
    def compute_file_sha256(file_path: Path) -> str:
        h = hashlib.sha256()
        with file_path.open("rb") as f:
            for chunk in iter(lambda: f.read(8192), b""):
                h.update(chunk)
        return h.hexdigest()

    @staticmethod
    def detect_platform_keyword() -> str:
        system = platform.system()
        machine = platform.machine()
        arch = machine.lower()
        if arch in ("x86_64", "amd64"):
            arch_key = "amd64"
        elif arch in ("aarch64", "arm64"):
            arch_key = "arm64"
        else:
            arch_key = arch

        if system == "Linux":
            return f"Linux_{arch_key}"
        elif system == "Windows":
            return "Win64"
        elif system == "Darwin":
            return "macOS_arm64"
        else:
            return system

    @staticmethod
    def download_asset(url: str) -> Path:
        response = requests.get(url, stream=True, timeout=60)
        response.raise_for_status()
        tmp_dir = Path(tempfile.mkdtemp())
        fname = Path(
            url
        ).name  # 例如 "TomatoNovelDownloader-Linux_amd64-v1.5.1" 或加扩展名 ".exe"
        tmp_file = tmp_dir / fname
        with tmp_file.open("wb") as f:
            for chunk in response.iter_content(chunk_size=8192):
                if chunk:
                    f.write(chunk)
        return tmp_file

    def apply_update(self, tmp_file: Path) -> None:
        """
        1. 将下载后的 tmp_file（带版本号、但尚未加 .new）搬到可执行目录并加 .new 后缀
           （保证不会跟旧版本冲突）；
        2. 在同目录下先删除所有旧版本（不论 v1.5.0 还是 v1.5.1，都删掉）；
        3. 再把 .new 文件重命名/移动到“去掉 .new”的正式名称（带版本号）；
        4. 启动改好名后的可执行；删除 bat（Windows）或替换进程（Unix/macOS）。
        """
        orig = self.local_executable  # 运行时带版本号的路径
        system = platform.system()
        exe_dir = orig.parent
        orig_name = (
            orig.name
        )  # 例： "TomatoNovelDownloader-Linux_amd64-v1.5.1" 或带 ".exe"

        # 1. 先把 tmp_file 移到 exe_dir，重命名为 "[原始 tmp 文件名].new"
        #    例如 tmp_file.name == "TomatoNovelDownloader-Linux_amd64-v1.5.1"（或 ".exe"）
        new_name_base = (
            tmp_file.name
        )  # 例如 "TomatoNovelDownloader-Linux_amd64-v1.5.1" 或带 ".exe"
        new_with_suffix = new_name_base + ".new"
        new_path = exe_dir / new_with_suffix

        try:
            if tmp_file.resolve() != new_path.resolve():
                shutil.move(str(tmp_file), str(new_path))
        except Exception as e:
            self.logger.error(f"移动下载文件到 {new_path} 时出错：{e}")
            return

        if system == "Windows":
            # Windows 平台：生成 .bat 去等进程退出后删旧、启动新
            try:
                bat_path = self._create_windows_updater_bat(
                    exe_dir, orig_name, new_with_suffix
                )
                # 用 shell=True 直接执行 .bat
                subprocess.Popen(
                    f'"{bat_path}"',
                    shell=True,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                self.logger.info("已启动 Windows 更新脚本，主程序即将退出以完成更新。")
                sys.exit(0)
            except Exception as e:
                self.logger.error(f"Windows 平台生成或启动更新脚本时出错：{e}")
                return

        else:
            # Linux / macOS 平台：先删除所有旧版本，再把 .new 重命名，最后 execv
            try:
                # 2. 删除“同平台所有旧版本”：
                #    先从 orig_name（如 "TomatoNovelDownloader-Linux_amd64-v1.5.1"）中
                #    提取出“平台+程序名称”前缀，例如切分到 "-v" 之前
                #    prefix = "TomatoNovelDownloader-Linux_amd64"
                if "-v" in orig_name:
                    prefix = orig_name.rsplit("-v", 1)[0]
                else:
                    # 万一没有 -v，就全部名字当前缀
                    prefix = orig_name

                # 在目录 exe_dir 下寻找所有符合 "{prefix}-v*" 的文件，把它们统统删除
                for candidate in exe_dir.iterdir():
                    name = candidate.name
                    if name.startswith(prefix + "-v") and not name.endswith(".new"):
                        # 只删不带 .new 的旧版本（如果是 .new，就留到下一步重命名）
                        try:
                            # Windows 下如果正运行的话会失败，但我们在 Unix/macOS 分支
                            # 所以直接 unlink 就行
                            candidate.unlink()
                            self.logger.info(f"已删除旧版本：{candidate.name}")
                        except Exception:
                            pass

                # 3. 把 new_path（带 .new 后缀）改成“去掉 .new”（也就是保留版本号）
                final_name = new_name_base  # 本来 new_name_base 就是带版本号的正确名字
                final_path = exe_dir / final_name

                if new_path.exists():
                    try:
                        new_path.rename(final_path)
                        self.logger.info(f"将 {new_with_suffix} 重命名为 {final_name}")
                    except Exception as e:
                        self.logger.error(
                            f"重命名 {new_with_suffix} 为 {final_name} 时出错：{e}"
                        )
                        return
                else:
                    self.logger.error(f"未找到 {new_with_suffix}，无法重命名。")
                    return

                # 4. 确保新文件有可执行权限
                try:
                    final_path.chmod(0o755)
                except Exception:
                    pass

                # 5. 用 execv 启动新版本并替换当前进程
                self.logger.info(f"即将 execv 新版本：{final_name}")
                os.execv(str(final_path), [str(final_path)] + sys.argv[1:])

            except Exception as e:
                self.logger.error(f"Unix/macOS 平台完整更新时出错：{e}")
                return

    def _create_windows_updater_bat(
        self, exe_dir: Path, orig_name: str, new_with_suffix: str
    ) -> Path:
        """
        exe_dir: 可执行所在目录
        orig_name: 旧版本的文件名（带版本号），例如 "TomatoNovelDownloader-Win64-v1.5.0.exe"
        new_with_suffix: 新版本下载后带 .new 的名字，例如
                         "TomatoNovelDownloader-Win64-v1.5.1.exe.new"

        .bat 脚本做的事：
        1. 等待主程序退出（ping 延时）
        2. 删除 exe_dir 下所有符合 "TomatoNovelDownloader-Win64-v*-*.exe" 的旧版本
        3. 把 new_with_suffix 重命名成正确的 final_name（即去掉 .new）
        4. 启动 final_name（新版 exe）
        5. 删除自身 .bat
        """
        # 先准备好各种路径字符串
        bat_filename = f"update_{orig_name[:-4]}.bat"
        bat_path = Path(tempfile.gettempdir()) / bat_filename

        old_prefix = orig_name.rsplit("-v", 1)[0]  # 例如 "TomatoNovelDownloader-Win64"
        # 生成匹配模式，比如 "TomatoNovelDownloader-Win64-v*.exe"
        pattern_prefix = old_prefix + "-v"

        orig_str_pattern = str(
            exe_dir / (pattern_prefix + "*")
        )  # for logging/reference
        new_full_str = str(exe_dir / new_with_suffix)  # 带 .new
        final_name = new_with_suffix[: -len(".new")]  # 去掉 .new
        final_full_str = str(exe_dir / final_name)  # 去掉 .new 后的路径

        # bat 内容：先 ping 等待，再删除旧 exe，再重命名新版，启动新版，最后自删
        bat_content = f"""@echo off
echo 等待主程序退出...
ping 127.0.0.1 -n 3 > nul

:: 删除可执行目录下所有 "{pattern_prefix}*.exe" 旧版本
for %%F in ("{exe_dir}\\{pattern_prefix}*.exe") do (
    if exist "%%F" (
        del /F /Q "%%F"
    )
)

:: 将新版本 "{new_with_suffix}" 改名为 "{final_name}"
if exist "{new_full_str}" (
    ren "{new_full_str}" "{final_name}"
)

:: 启动新版
start "" "{final_full_str}"

:: 删除这个批处理脚本自身
del "%~f0"
"""
        debug_bat_content = f"""@echo off
rem ------------------- 调试日志开始 -------------------
echo [BAT 调试] 执行时间: %date% %time% >> "%TEMP%\bat_debug.txt"
echo [BAT 调试] exe_dir = "%~dp0" >> "%TEMP%\bat_debug.txt"
echo [BAT 调试] new_with_suffix = "%~dp0\%~n0.new%" >> "%TEMP%\bat_debug.txt"
echo [BAT 调试] final_name = "%~dp0\%~n0%" >> "%TEMP%\bat_debug.txt"
rem ----------------------------------------------------

echo 等待主程序退出...
rem 用 timeout/循环检测，让主程序一定退出才继续
:waitloop
tasklist /FI "IMAGENAME eq TomatoNovelDownloader-Win64-*.exe" | findstr /I "TomatoNovelDownloader-Win64" > nul
if "%ERRORLEVEL%"=="0" (
  timeout /T 1 /NOBREAK > nul
  goto waitloop
)

rem 删除旧版本
for %%F in ("%~dp0\TomatoNovelDownloader-Win64-v*.exe") do (
    if exist "%%F" (
        echo [BAT 调试] 删除旧文件：%%F >> "%TEMP%\bat_debug.txt"
        del /F /Q "%%F"
    )
)

rem 重命名 .new 文件到正式 exe（只保留文件名部分，路径保持 %~dp0）
if exist "%~dp0\%~n0.new%" (
    echo [BAT 调试] 重命名 "%~dp0\%~n0.new%" -> "%~dp0\%~n0%" >> "%TEMP%\bat_debug.txt"
    ren "%~dp0\%~n0.new%" "%~n0%"
) else (
    echo [BAT 调试] 找不到 "%~dp0\%~n0.new%"，无法重命名! >> "%TEMP%\bat_debug.txt"
)

rem 切换到 exe 所在目录，确保后续启动的工作目录正确
pushd "%~dp0"

rem 启动新版可执行
echo [BAT 调试] 开始启动 "%~dp0\%~n0%" >> "%TEMP%\bat_debug.txt"
start "" "%~dp0\%~n0%"

rem 等待几秒，确认启动有没有抛错（可选）
timeout /T 2 /NOBREAK > nul

popd

rem 删除自己
echo [BAT 调试] 删除自身："%~f0" >> "%TEMP%\bat_debug.txt"
del "%~f0"
"""

        try:
            with open(bat_path, "w", encoding="utf-8") as f:
                if self.debug:
                    f.write(debug_bat_content)
                else:
                    f.write(bat_content)
        except Exception as e:
            self.logger.error(f"无法生成 Windows 更新脚本 {bat_path}：{e}")
            raise

        return bat_path

    def check_for_updates(self) -> bool:
        """
        …（前面版本判断的逻辑保持不变）…
        """
        info = self.fetch_latest_release()
        if info is None:
            return True

        latest_tag = info.get("tag_name", "").strip()
        if not latest_tag:
            self.logger.error("Error: 未获取到最新 Release 的 tag_name")
            return True

        # 1. 版本号不同 —— 完整升级逻辑（保持原样）
        if latest_tag != self.local_version:
            self.logger.info(
                f"检测到新版本：{latest_tag}，当前版本：{self.local_version}"
            )
            choice = input("是否下载并升级到最新版？[Y/n]: ").strip().lower()
            if choice in ("", "y", "yes"):
                plat_key = self.detect_platform_keyword()
                asset_url = None
                asset_name = None
                asset_digest = None  # 用于存储云端提供的 sha256 值
                for asset in info.get("assets", []):
                    name = asset.get("name", "")
                    if (
                        plat_key.lower() in name.lower()
                        and latest_tag.lower() in name.lower()
                    ):
                        asset_url = asset.get("browser_download_url")
                        asset_name = name
                        # 直接从 asset 字段里取 digest
                        digest_field = asset.get("digest", "")  # 形如 "sha256:<hex>"
                        if digest_field.startswith("sha256:"):
                            asset_digest = digest_field.split("sha256:")[-1]
                        break

                if not asset_url:
                    self.logger.error(
                        "Error: 未找到对应平台/版本的发布资产，无法升级。"
                    )
                    return True

                self.logger.info(f"正在下载最新版本 ({asset_name}) ...")
                tmp_path = self.download_asset(asset_url)
                self.logger.info(f"下载完成，文件路径：{tmp_path}")

                self.logger.info("正在应用完整升级...")
                self.apply_update(tmp_path)
                return False
            else:
                self.logger.warning("已取消升级，继续使用旧版本。")
                return False

        # 2. 版本号相同：检查热补丁 —— 使用 digest 字段直接比较，无需先下载
        self.logger.info(
            f"当前版本 ({self.local_version}) 与最新版本一致，使用云端 digest 检查是否有热补丁..."
        )
        # 2.1 先计算本地可执行的 SHA256
        try:
            local_hash = self.compute_file_sha256(self.local_executable)
        except Exception as e:
            self.logger.error(f"计算本地可执行文件哈希时出错：{e}")
            return True

        plat_key = self.detect_platform_keyword()
        asset_url = None
        asset_name = None
        asset_digest = None  # 云端 sha256（不含前缀）
        for asset in info.get("assets", []):
            name = asset.get("name", "")
            if plat_key.lower() in name.lower() and latest_tag.lower() in name.lower():
                asset_url = asset.get("browser_download_url")
                asset_name = name
                # 读取 digest 字段
                digest_field = asset.get("digest", "")
                if digest_field.startswith("sha256:"):
                    asset_digest = digest_field.split("sha256:")[-1]
                break

        if not asset_url or not asset_digest:
            self.logger.warning(
                "Warning: 未找到对应平台/版本的发布资产或缺少 digest，无法检查热补丁。"
            )
            return True

        self.logger.info(f"本地哈希:  {local_hash}")
        self.logger.info(f"云端哈希: {asset_digest}")

        # 2.2 直接比较本地 hash 和 云端 digest
        if local_hash != asset_digest:
            if self.debug:
                choice = input("是否下载并升级到最新版？[Y/n]: ").strip().lower()
                if choice in ("", "y", "yes"):
                    self.logger.info("检测到热补丁更新，开始下载并应用...")
                    tmp_asset = self.download_asset(asset_url)
                    self.apply_update(tmp_asset)
                    return False
            else:
                self.logger.info("检测到热补丁更新，开始下载并应用...")
                tmp_asset = self.download_asset(asset_url)
                self.apply_update(tmp_asset)
                return False
        else:
            self.logger.info("本地已与云端哈希一致，无需更新。")

        return True

    def fetch_latest_release(self) -> Optional[Dict[str, Any]]:
        url = f"https://api.github.com/repos/{self.owner}/{self.repo}/releases/latest"
        headers = {"Accept": "application/vnd.github+json"}
        try:
            resp = requests.get(url, headers=headers, timeout=10)
            resp.raise_for_status()
            return resp.json()
        except Exception as e:
            self.logger.error(f"Error: 获取最新 Release 时出错：{e}")
            return None
