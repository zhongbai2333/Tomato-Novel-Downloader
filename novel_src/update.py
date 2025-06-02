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
    UpdateManager 用于：
    1. 查询 GitHub 仓库的最新 Release 信息。
    2. 比对本地版本和 release tag，决定是否要做“完整升级”或“热补丁”。
    3. 下载对应平台的资产文件，并应用替换（Windows 通过 .bat，Unix/macOS 直接 execv）。
    """

    GITHUB_API_TIMEOUT = 10  # 秒
    DOWNLOAD_TIMEOUT = 60  # 秒

    def __init__(self):
        self.owner = "zhongbai2333"
        self.repo = "Tomato-Novel-Downloader"
        self.local_version = f"v{VERSION}"
        # local_executable：运行时的可执行文件，路径里必须带版本号
        self.local_executable = Path(sys.argv[0]).resolve()
        self.logger = GlobalContext.get_logger()
        self.debug = GlobalContext.get_log_system().debug

    @staticmethod
    def compute_file_sha256(file_path: Path) -> str:
        """计算文件的 SHA256 哈希值"""
        h = hashlib.sha256()
        try:
            with file_path.open("rb") as f:
                for chunk in iter(lambda: f.read(8192), b""):
                    h.update(chunk)
            return h.hexdigest()
        except Exception as e:
            raise RuntimeError(f"计算文件哈希失败：{file_path}，{e}")

    @staticmethod
    def detect_platform_keyword() -> str:
        """
        返回平台和架构关键字，用于匹配 release 资产名：
        - Linux 上返回 "Linux_amd64" 或 "Linux_arm64" 等
        - Windows 上返回 "Win64"
        - macOS 上返回 "macOS_arm64"（这里假设都是 arm64，若要兼容 intel 可再做判断）
        """
        system = platform.system()
        machine = platform.machine().lower()

        # 判断架构
        if machine in ("x86_64", "amd64"):
            arch_key = "amd64"
        elif machine in ("aarch64", "arm64"):
            arch_key = "arm64"
        else:
            arch_key = machine

        if system == "Linux":
            return f"Linux_{arch_key}"
        if system == "Windows":
            return "Win64"
        if system == "Darwin":
            return "macOS_arm64"
        # 其他平台直接返回 system 名称
        return system

    def fetch_latest_release(self) -> Optional[Dict[str, Any]]:
        """调用 GitHub API，获取最新 Release 信息。出现错误时返回 None"""
        url = f"https://api.github.com/repos/{self.owner}/{self.repo}/releases/latest"
        headers = {"Accept": "application/vnd.github+json"}
        try:
            resp = requests.get(url, headers=headers, timeout=self.GITHUB_API_TIMEOUT)
            resp.raise_for_status()
            return resp.json()
        except Exception as e:
            self.logger.error(f"[UpdateManager] 获取最新 Release 时出错：{e}")
            return None

    def _find_asset_info(
        self, release_info: Dict[str, Any], plat_key: str, tag: str
    ) -> Optional[Dict[str, str]]:
        """
        从 release_info["assets"] 中查找第一个既包含 plat_key 又包含 tag 的 asset，
        返回一个 dict，包含：{"url": 下载链接, "name": 资产名称, "sha256": 哈希值(不带前缀)}，
        如果没找到则返回 None。
        """
        for asset in release_info.get("assets", []):
            name = asset.get("name", "")
            # asset["digest"] 形如 "sha256:<hex>"；也可能直接没有 digest
            digest_field = asset.get("digest", "")
            if plat_key.lower() in name.lower() and tag.lower() in name.lower():
                url = asset.get("browser_download_url")
                if not url:
                    continue
                # 提取 sha256
                sha256_val = ""
                if digest_field and digest_field.startswith("sha256:"):
                    sha256_val = digest_field.split("sha256:")[-1]
                return {"url": url, "name": name, "sha256": sha256_val}
        return None

    def download_asset(self, url: str) -> Path:
        """
        仅下载 asset，不做哈希校验。将文件写入临时目录并返回对应 Path，
        如果失败抛异常。
        """
        try:
            response = requests.get(url, stream=True, timeout=self.DOWNLOAD_TIMEOUT)
            response.raise_for_status()
        except Exception as e:
            raise RuntimeError(f"下载资产时出错：{url}，{e}")

        tmp_dir = Path(tempfile.mkdtemp(prefix="upd_"))
        fname = Path(
            url
        ).name  # 文件名，例如 "TomatoNovelDownloader-Linux_amd64-v1.5.1" 或带 ".exe"
        tmp_file = tmp_dir / fname

        try:
            with tmp_file.open("wb") as f:
                for chunk in response.iter_content(chunk_size=8192):
                    if chunk:
                        f.write(chunk)
            return tmp_file
        except Exception as e:
            raise RuntimeError(f"写入下载临时文件失败：{tmp_file}，{e}")

    def download_and_verify(self, url: str, expected_sha256: str) -> Path:
        """
        下载 asset 并校验 sha256，一旦校验不通过则删除并抛异常。
        """
        tmp_file = self.download_asset(url)
        actual_sha256 = self.compute_file_sha256(tmp_file)
        if actual_sha256.lower() != expected_sha256.lower():
            tmp_file.unlink(missing_ok=True)
            raise RuntimeError(
                f"SHA256 校验失败：下载文件 {tmp_file} 的哈希 {actual_sha256} 与期望 {expected_sha256} 不符"
            )
        return tmp_file

    def _cleanup_old_versions_unix(self, exe_dir: Path, prefix: str):
        """
        Unix/macOS 下删除 exe_dir 下所有以 prefix+"-v*" 开头且不以 .new 结尾的文件。
        prefix 示例："TomatoNovelDownloader-Linux_amd64"
        """
        for candidate in exe_dir.iterdir():
            name = candidate.name
            if name.startswith(prefix + "-v") and not name.endswith(".new"):
                try:
                    candidate.unlink()
                    self.logger.info(f"[UpdateManager] 删除旧版本：{name}")
                except Exception as e:
                    self.logger.warning(
                        f"[UpdateManager] 删除旧版本文件 {name} 时失败：{e}"
                    )

    def _create_windows_updater_bat(
        self, exe_dir: Path, orig_name: str, new_with_suffix: str
    ) -> Path:
        """
        为 Windows 生成一个 .bat 脚本，用于：
        1. 等待当前程序退出（ping 延时）。
        2. 删除 exe_dir 下所有以 old_prefix+"-v*.exe" 的旧版本。
        3. 将 "{new_with_suffix}" 重命名为正确的 final_name（去掉 ".new"）。
        4. 启动新版 exe。
        5. 删除自身 bat 脚本。
        """
        # old_prefix 例如 "TomatoNovelDownloader-Win64"
        old_prefix = orig_name.rsplit("-v", 1)[0]
        pattern = f"{old_prefix}-v*.exe"  # 用于删除通配

        exe_dir_str = str(exe_dir)
        new_full_path = exe_dir / new_with_suffix
        final_name = new_with_suffix.removesuffix(".new")
        final_full_path = exe_dir / final_name

        # 整理 bat 文件路径：存放在临时目录里
        bat_filename = f"update_{orig_name[:-4]}.bat"
        bat_path = Path(tempfile.gettempdir()) / bat_filename

        # bat 内容：
        bat_commands = [
            "@echo off",
            "echo 等待主程序退出...",
            "ping 127.0.0.1 -n 3 > nul",
            "",
            f":: 删除 exe_dir 下所有旧版本：{pattern}",
            f'for %%F in ("{exe_dir_str}\\{pattern}") do (',
            '    if exist "%%F" (',
            '        del /F /Q "%%F"',
            "    )",
            ")",
            "",
            f':: 将新版本 "{new_with_suffix}" 重命名为 "{final_name}"',
            f'if exist "{new_full_path}" (',
            f'    ren "{new_full_path}" "{final_name}"',
            ")",
            "",
            ":: 启动新版",
            f'start "" "{final_full_path}"',
            "",
            ":: 删除自身 bat 脚本",
            'del "%~f0"',
        ]
        bat_content = "\r\n".join(bat_commands)

        try:
            bat_path.write_text(bat_content, encoding="utf-8")
            return bat_path
        except Exception as e:
            raise RuntimeError(f"无法写入 Windows 更新脚本 {bat_path}：{e}")

    def apply_update(self, tmp_file: Path) -> None:
        """
        将下载后的 tmp_file（带版本号，未加 .new）移动到可执行目录并加上 .new 后缀，
        然后：
        - Windows：生成 .bat 执行脚本，由脚本完成删除旧版、重命名、启动新版、删除 bat
        - Unix/macOS：删除旧版、重命名 .new -> 正式文件、赋予执行权限、execv 启动新版
        """
        orig = self.local_executable
        system = platform.system()
        exe_dir = orig.parent
        orig_name = (
            orig.name
        )  # 包含版本号，比如 "TomatoNovelDownloader-Linux_amd64-v1.5.1" 或 “.exe”

        # 1. 将 tmp_file 移到 exe_dir，并改名为 "[资产名].new"
        new_name_base = (
            tmp_file.name
        )  # 例如 "TomatoNovelDownloader-Linux_amd64-v1.5.1" 或带 ".exe"
        new_with_suffix = new_name_base + ".new"
        new_path = exe_dir / new_with_suffix

        try:
            if tmp_file.resolve() != new_path.resolve():
                shutil.move(str(tmp_file), str(new_path))
        except Exception as e:
            self.logger.error(f"[UpdateManager] 移动下载文件到 {new_path} 时出错：{e}")
            return

        if system == "Windows":
            # Windows 平台走 .bat 脚本
            try:
                bat_path = self._create_windows_updater_bat(
                    exe_dir, orig_name, new_with_suffix
                )
                subprocess.Popen(
                    f'"{bat_path}"',
                    shell=True,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                self.logger.info(
                    "[UpdateManager] 已启动 Windows 更新脚本，主程序即将退出。"
                )
                sys.exit(0)
            except Exception as e:
                self.logger.error(
                    f"[UpdateManager] Windows 平台生成/启动更新脚本时出错：{e}"
                )
                return
        else:
            # Unix/macOS 平台：同步删除旧版本、重命名并 execv
            try:
                # 2. 提取 prefix，比如从 "TomatoNovelDownloader-Linux_amd64-v1.5.1" 中取 "TomatoNovelDownloader-Linux_amd64"
                if "-v" in orig_name:
                    prefix = orig_name.rsplit("-v", 1)[0]
                else:
                    prefix = orig_name

                # 3. 删除旧版本
                self._cleanup_old_versions_unix(exe_dir, prefix)

                # 4. 重命名 .new 文件
                final_path = exe_dir / new_name_base
                if new_path.exists():
                    try:
                        new_path.rename(final_path)
                        self.logger.info(
                            f"[UpdateManager] 将 {new_with_suffix} 重命名为 {new_name_base}"
                        )
                    except Exception as e:
                        self.logger.error(
                            f"[UpdateManager] 重命名 {new_with_suffix} 为 {new_name_base} 时出错：{e}"
                        )
                        return
                else:
                    self.logger.error(
                        f"[UpdateManager] 未找到 {new_with_suffix}，无法重命名。"
                    )
                    return

                # 5. 赋予可执行权限
                try:
                    final_path.chmod(0o755)
                except Exception:
                    pass

                # 6. execv 启动新进程并替换当前进程
                self.logger.info(f"[UpdateManager] Execv 启动新版本：{new_name_base}")
                os.execv(str(final_path), [str(final_path)] + sys.argv[1:])
            except Exception as e:
                self.logger.error(f"[UpdateManager] Unix/macOS 平台完整更新时出错：{e}")
                return

    def check_for_updates(self) -> bool:
        """
        核心入口：
        1. 获取最新 release，如果失败返回 True（表示正常继续使用）。
        2. 如果最新 tag 与本地 local_version 不同，提示完整升级；否则进入热补丁检查。
        3. 完整升级后返回 False（程序将被替换或退出），取消升级返回 False。
        4. 热补丁检查：比较本地哈希与云端 digest：
           - 相同：返回 True（无需更新）。
           - 不同：自动下载并替换，返回 False。
        """
        info = self.fetch_latest_release()
        if not info:
            return True  # 获取 release 失败，继续使用本地版本

        latest_tag = info.get("tag_name", "").strip()
        if not latest_tag:
            self.logger.error("[UpdateManager] 未获取到最新 Release 的 tag_name")
            return True

        # ------------- 1. 完整升级场景 -------------
        if latest_tag != self.local_version:
            self.logger.info(
                f"[UpdateManager] 检测到新版本：{latest_tag}，当前：{self.local_version}"
            )
            choice = input("是否下载并升级到最新版？[Y/n]: ").strip().lower()
            if choice in ("", "y", "yes"):
                plat_key = self.detect_platform_keyword()
                asset_info = self._find_asset_info(info, plat_key, latest_tag)
                if not asset_info:
                    self.logger.error(
                        "[UpdateManager] 未找到对应平台/版本的发布资产，无法升级。"
                    )
                    return False  # 取消继续执行，交由调用方决定

                url = asset_info["url"]
                name = asset_info["name"]
                sha256_val = asset_info.get("sha256", "")

                self.logger.info(f"[UpdateManager] 正在下载最新版本 ({name}) ...")
                try:
                    # 如果 asset_info["sha256"] 可用，优先校验
                    if sha256_val:
                        tmp_path = self.download_and_verify(url, sha256_val)
                    else:
                        tmp_path = self.download_asset(url)
                except Exception as e:
                    self.logger.error(f"[UpdateManager] 下载或校验失败：{e}")
                    return False

                self.logger.info("[UpdateManager] 下载完成，开始应用完整升级...")
                self.apply_update(tmp_path)
                return False
            else:
                self.logger.warning("[UpdateManager] 用户取消升级，继续使用旧版本。")
                return False

        # ------------- 2. 热补丁检查场景（版本号相同） -------------
        self.logger.info(
            f"[UpdateManager] 本地版本 ({self.local_version}) 与最新相同，检查热补丁..."
        )

        try:
            local_hash = self.compute_file_sha256(self.local_executable)
        except Exception as e:
            self.logger.error(f"[UpdateManager] 计算本地可执行哈希时出错：{e}")
            return True

        plat_key = self.detect_platform_keyword()
        asset_info = self._find_asset_info(info, plat_key, latest_tag)
        if not asset_info:
            self.logger.warning(
                "[UpdateManager] 未找到对应平台/版本的发布资产或缺少 digest，无法检查热补丁。"
            )
            return True

        asset_url = asset_info["url"]
        asset_name = asset_info["name"]
        asset_digest = asset_info.get("sha256", "")

        if not asset_digest:
            self.logger.warning(
                f"[UpdateManager] 资产 {asset_name} 未提供 sha256 字段，跳过热补丁检查。"
            )
            return True

        self.logger.info(f"[UpdateManager] 本地哈希：{local_hash}")
        self.logger.info(f"[UpdateManager] 云端哈希：{asset_digest}")

        # 对比本地哈希和云端 digest
        if local_hash.lower() != asset_digest.lower():
            if self.debug:
                choice = (
                    input("检测到热补丁更新，是否下载并应用？[Y/n]: ").strip().lower()
                )
                if choice not in ("", "y", "yes"):
                    self.logger.warning(
                        "[UpdateManager] 用户取消热补丁更新，保持当前版本。"
                    )
                    return False

            self.logger.info("[UpdateManager] 检测到热补丁更新，开始下载并应用...")
            try:
                tmp_asset = self.download_and_verify(asset_url, asset_digest)
            except Exception as e:
                self.logger.error(f"[UpdateManager] 下载或校验热补丁失败：{e}")
                return False

            self.apply_update(tmp_asset)
            return False
        else:
            self.logger.info("[UpdateManager] 本地与云端哈希一致，无需更新。")
            return True
