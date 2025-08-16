import os
import sys
import tempfile
import hashlib
import requests
import platform
import shutil
import subprocess
from pathlib import Path
from typing import Dict, Any, Optional
from tqdm import tqdm

from .base_system.context import GlobalContext
from .constants import VERSION


class UpdateManager(object):
    def __init__(self):
        self.logger = GlobalContext.get_logger()

    @staticmethod
    def _compute_file_sha256(file_path: Path) -> str:
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
    def _get_self_hash() -> str:
        """获取当前脚本的 SHA256 哈希值"""
        local_executable = Path(sys.argv[0]).resolve()
        return UpdateManager._compute_file_sha256(local_executable)

    @staticmethod
    def _detect_platform_keyword() -> str:
        """
        返回平台和架构关键字，用于匹配 release 资产名：
        - Linux:  Linux_amd64 / Linux_arm64
        - Windows: Win64
        - macOS:  macOS_arm64 / macOS_amd64 (新增 Intel 支持)
          （如果需要将来支持 universal，可扩展为 macOS_universal）
        其他平台直接返回 system 名称
        """
        system = platform.system()
        machine = platform.machine().lower()

        # 归一化架构
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
            # 区分 Apple Silicon 与 Intel
            if arch_key == "amd64":
                return "macOS_amd64"
            if arch_key == "arm64":
                return "macOS_arm64"
            # 兜底：如果出现其它（很少见），保留架构名
            return f"macOS_{arch_key}"
        return system

    def _fetch_latest_release(self) -> Optional[Dict[str, Any]]:
        """调用 GitHub API，获取最新 Release 信息。出现错误时返回 None"""
        url = "https://api.github.com/repos/zhongbai2333/Tomato-Novel-Downloader/releases/latest"
        headers = {"Accept": "application/vnd.github+json"}
        try:
            resp = requests.get(url, headers=headers, timeout=10)
            resp.raise_for_status()
            return resp.json()
        except Exception as e:
            self.logger.error(f"[UpdateManager] 获取最新 Release 时出错：{e}")
            return None

    def _get_latest_release(self) -> Dict[str, str]:
        """获取最新 Release 的当前平台匹配资产信息。匹配不到返回空 dict"""
        latest_release = self._fetch_latest_release()
        if not latest_release:
            return {}

        platform_key = self._detect_platform_keyword()

        for asset in latest_release.get("assets", []):
            name = asset.get("name", "")
            if platform_key in name:
                original_url = asset.get("browser_download_url")
                # 通过笒鬼鬼 API 获取加速下载地址
                api_url = (
                    "https://api.cenguigui.cn/api/github/"
                    "?type=json"
                    f"&url={original_url}"
                )
                download_url = original_url  # 默认使用原始地址
                try:
                    resp = requests.get(api_url, timeout=10)
                    resp.raise_for_status()
                    json_data = resp.json()
                    download_url = json_data.get("data", {}).get(
                        "downUrl", original_url
                    )
                except Exception as e:
                    self.logger.warning(
                        f"[UpdateManager] 获取加速下载地址失败，使用原始地址：{e}"
                    )

                # SHA256: 目前 Release 资产没有标准 digest 字段时可能为空
                sha256_val = asset.get("digest", "")
                if sha256_val:
                    sha256_val = sha256_val.split(":")[-1]
                else:
                    # 若没有 digest，可考虑在将来提供一个 <asset>.sha256 文件，
                    # 或者跳过严格校验（这里保持兼容，返回空字符串）
                    sha256_val = ""

                return {
                    "name": latest_release.get("name", ""),
                    "tag_name": latest_release.get("tag_name", ""),
                    "browser_download_url": download_url,
                    "size": asset.get("size", 0),
                    "sha256": sha256_val,
                }
        return {}

    def _download_asset(self, tmp_dir: Path, size: str | int, url: str) -> Path:
        """
        下载 asset，并显示 tqdm 进度条。返回下载到本地的临时文件路径。
        如果下载过程中出错，会抛出 RuntimeError。
        """
        try:
            response = requests.get(url, stream=True, timeout=60)
            response.raise_for_status()
        except Exception as e:
            raise RuntimeError(f"[UpdateManager] 下载资产时出错：{url}，{e}")

        total_size_header = response.headers.get("Content-Length")
        if total_size_header is not None:
            try:
                total_size = int(total_size_header)
            except ValueError:
                total_size = (
                    int(size)
                    if isinstance(size, (int, str)) and str(size).isdigit()
                    else 0
                )
        else:
            total_size = (
                int(size) if isinstance(size, (int, str)) and str(size).isdigit() else 0
            )

        fname = Path(url).name
        tmp_file = tmp_dir / fname

        try:
            chunk_size = 8192
            with tmp_file.open("wb") as f:
                with tqdm(
                    total=total_size if total_size > 0 else None,
                    unit="B",
                    unit_scale=True,
                    desc=f"Downloading {fname}",
                ) as pbar:
                    for chunk in response.iter_content(chunk_size=chunk_size):
                        if chunk:
                            f.write(chunk)
                            if total_size > 0:
                                pbar.update(len(chunk))
            return tmp_file
        except Exception as e:
            try:
                tmp_file.unlink()
            except Exception:
                pass
            raise RuntimeError(f"[UpdateManager] 写入下载临时文件失败：{tmp_file}，{e}")

    def _download_and_verify(
        self, tmp_dir: Path, size: str | int, url: str, expected_sha256: str
    ) -> Path:
        """
        下载 asset 并校验 sha256：
        - 如果 expected_sha256 为空则跳过校验（兼容没有提供 digest 的情况）
        - 校验失败抛出异常
        """
        tmp_file = self._download_asset(tmp_dir, size, url)
        if expected_sha256:
            actual_sha256 = self._compute_file_sha256(tmp_file)
            if actual_sha256.lower() != expected_sha256.lower():
                tmp_file.unlink(missing_ok=True)
                raise RuntimeError(
                    f"SHA256 校验失败：下载文件 {tmp_file} 的哈希 {actual_sha256} 与期望 {expected_sha256} 不符"
                )
        else:
            # 没有 expected_sha256 时仅提示
            pass
        return tmp_file

    def _unix_apply(self, tmp_file: Path) -> Path:
        """
        将下载的临时文件移动到当前脚本所在目录，并重命名为原脚本名。
        对于 Unix 系统，确保新文件具有可执行权限。
        """
        local_executable = Path(sys.argv[0]).resolve()
        if local_executable.exists():
            local_executable.unlink(missing_ok=True)
        shutil.move(str(tmp_file), str(local_executable.parent))
        new_executable = local_executable.parent / tmp_file.name
        new_executable.chmod(0o755)
        self.logger.info(
            f"[UpdateManager] 更新成功，已将 {tmp_file} 移动到 {local_executable}"
        )
        return new_executable

    def _windows_apply(self, tmp_file: Path) -> None:
        """
        Windows 系统下的更新逻辑
        """
        local_executable = Path(sys.argv[0]).resolve()
        new_executable = local_executable.parent / tmp_file.name
        shutil.move(str(tmp_file), str(new_executable) + ".new")

        lines = [
            "@echo off",
            "echo Waiting...",
            "timeout /t 3 /nobreak",
            "",
            f'cd /d "{str(local_executable.parent)}"',
            "",
            f'for %%F in ("{str(local_executable.name)}") do (',
            '    if exist "%%~fF" (',
            '        del /F /Q "%%~fF"',
            "    )",
            ")",
            "",
            f'if exist "{str(new_executable.name) + ".new"}" (',
            f'    ren "{str(new_executable.name) + ".new"}" "{str(new_executable.name)}"',
            ")",
            "",
            "set PYINSTALLER_RESET_ENVIRONMENT=1",
            f'start "" "{str(new_executable.name)}"',
            "",
            'del "%~f0"',
        ]
        bat_content = "\r\n".join(lines)

        try:
            bat_path = Path(tempfile.gettempdir()) / "update_script.bat"
            bat_path.write_text(bat_content, encoding="utf-8")
        except Exception as e:
            raise RuntimeError(f"无法写入 Windows 更新脚本 {bat_path}：{e}")
        try:
            subprocess.Popen(
                f'"{bat_path}"',
                shell=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except Exception as e:
            raise RuntimeError(f"无法执行 Windows 更新脚本 {bat_path}：{e}")

    def _apply_update(self, tmp_file: Path) -> None | Path:
        """
        将下载的临时文件移动到当前脚本所在目录，并重命名为原脚本名。
        """
        if platform.system() == "Windows":
            self._windows_apply(tmp_file)
        else:
            return self._unix_apply(tmp_file)

    def _start_update(self, latest_release: dict) -> None:
        """开始更新程序"""
        with tempfile.TemporaryDirectory() as tmp_dir:
            self.logger.info(
                f"[UpdateManager] 开始下载最新版本：{latest_release['name']}"
            )
            tmp_file = self._download_and_verify(
                Path(tmp_dir),
                latest_release["size"],
                latest_release["browser_download_url"],
                latest_release["sha256"],
            )
            self.logger.info(
                f"[UpdateManager] 下载完成，开始应用更新：{latest_release['name']}"
            )
            new_executable = self._apply_update(tmp_file)
        if platform.system() == "Windows":
            self.logger.info("[UpdateManager] 请稍等，更新完成后将自动重启程序。")
            sys.exit(0)
        else:
            self.logger.info("[UpdateManager] 更新完成，正在重启程序...")
            env = os.environ.copy()
            env["PYINSTALLER_RESET_ENVIRONMENT"] = "1"  # 关键！
            os.execv(new_executable, [str(new_executable)] + sys.argv[1:])

    def check_for_updates(self, auto_ture: bool = False) -> None:
        """检查是否有可用更新"""
        self.logger.info("[UpdateManager] 正在检查更新...")
        current_version = f"v{VERSION}"
        latest_release = self._get_latest_release()
        if not latest_release:
            self.logger.info(
                "[UpdateManager] 无法获取最新版本信息，可能网络异常或 GitHub API 出错。"
            )
            return
        if (
            latest_release.get("tag_name")
            and latest_release["tag_name"] != current_version
        ):
            self.logger.info(
                f"[UpdateManager] 检测到新版本：{latest_release['tag_name']}，当前：{current_version}"
            )
            choice = input("是否下载并升级到最新版？[Y/n]: ").strip().lower()
            if choice in ("", "y", "yes") or auto_ture:
                try:
                    self._start_update(latest_release)
                except Exception as e:
                    self.logger.error(f"[UpdateManager] 更新失败：{e}")
                    return
            else:
                self.logger.warning("[UpdateManager] 用户取消升级，继续使用旧版本。")
                return
        else:
            self.logger.info(
                f"[UpdateManager] 本地版本 ({current_version}) 与最新相同，检查热补丁..."
            )
            # 如果没有提供 sha256（为空），跳过热补丁校验
            if latest_release.get("sha256"):
                try:
                    if self._get_self_hash() != latest_release["sha256"].lower():
                        self.logger.info(
                            "[UpdateManager] 检测到热补丁更新，正在应用..."
                        )
                        try:
                            self._start_update(latest_release)
                        except Exception as e:
                            self.logger.error(f"[UpdateManager] 热补丁更新失败：{e}")
                except Exception as e:
                    self.logger.warning(f"[UpdateManager] 热补丁校验过程出现问题：{e}")
