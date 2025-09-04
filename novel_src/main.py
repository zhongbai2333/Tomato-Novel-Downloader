from __future__ import annotations

import os
import re
import sys
import json
import urwid
import shutil
import pyperclip
from pathlib import Path
from typing import Callable, List, Tuple, Optional, Dict
from urllib.parse import urlparse, parse_qs
from ascii_magic import AsciiArt

# =============== 原项目内部依赖 ===============
from .base_system.context import GlobalContext, Config  # noqa: E402
from .book_parser.book_manager import BookManager  # noqa: E402
from .network_parser.network import NetworkClient  # noqa: E402
from .network_parser.downloader import ChapterDownloader  # noqa: E402
from .update import UpdateManager  # noqa: E402
from .constants import VERSION  # noqa: E402
from .old_main import main as old_main

# ------------------------------------------------------------
# 通用工具函数（与 UI 无关）
# ------------------------------------------------------------


def list_subdirs(path: Path | str) -> List[str]:
    """返回指定目录下所有一级子文件夹的名称列表（不含文件）。"""
    return [
        name for name in os.listdir(path) if os.path.isdir(os.path.join(path, name))
    ]


def load_download_status(status_path: Path) -> dict:
    """读取并返回 status_path 对应的 JSON 数据，失败时返回空 dict。"""
    try:
        with status_path.open("r", encoding="utf-8") as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def preview_ascii(image_path: Path):
    """在终端中渲染封面 ASCII 图（若失败则静默）。"""
    try:
        cols, _ = shutil.get_terminal_size(fallback=(80, 24))
        code_cols = int((cols - 8) / 2)
        print("=" * code_cols + " 封面预览 " + "=" * code_cols)
        art = AsciiArt.from_image(str(image_path))
        art.to_terminal(columns=cols)
    except Exception as e:  # noqa: BLE001
        logger = GlobalContext.get_logger()
        logger.debug(f"生成 ASCII 预览失败: {e}")


# ------------------------------------------------------------
# urwid 相关组件
# ------------------------------------------------------------

PALETTE = [
    ("reversed", "standout", ""),
    ("title", "light cyan", ""),
]


def menu_button(label: str, on_press: Callable[[urwid.Button], None]):
    """生成带反色焦点的菜单按钮。"""
    button = urwid.Button(label)
    urwid.connect_signal(button, "click", on_press)
    return urwid.AttrMap(button, None, focus_map="reversed")


class MessagePopup(urwid.WidgetWrap):
    """
    简单弹窗：显示一段文本，以下操作都会关闭弹窗并调用 on_close():
      - 按 q/Q/Enter/Esc
      - 鼠标在任何位置点击
    """

    def __init__(self, text: str, on_close: Callable[[], None], width: int = 60):
        self.on_close = on_close

        # 文本 + 提示
        txt = urwid.Text(
            text + "\n\n<按 q/Q/Enter/Esc 或 点击任意处 关闭弹窗>", align="center"
        )
        filler = urwid.Filler(txt, valign="top")
        frame = urwid.LineBox(filler)
        super().__init__(frame)

    def selectable(self) -> bool:
        # 让 urwid 把焦点给它，从而能接收 keypress
        return True

    def keypress(self, size, key):
        # q/Q/Enter/Esc 都关闭
        if key in ("q", "Q", "enter", "esc"):
            self.on_close()
            return None
        # 其余按键也不冒泡
        return None

    def mouse_event(self, size, event, button, col, row, focus):
        """
        捕获任何鼠标点击（通常 event == 'mouse press'，button >= 1）都关闭弹窗。
        返回 True 表示“该事件已被处理”，不再往下传递。
        """
        # event 格式往往是 "mouse press"、"mouse release"、"mouse drag" 等
        if event.startswith("mouse press"):
            self.on_close()
            return True
        # 其他鼠标事件也不想让往下冒泡
        return True


class PasteableEdit(urwid.Edit):
    def keypress(self, size, key):
        # 检测 Ctrl+V (urwid 中为 'ctrl v')
        if key.lower() == 'ctrl v':
            try:
                text = pyperclip.paste()
                self.insert_text(text)
            except Exception:
                pass
            return None
        return super().keypress(size, key)


class EnterEdit(PasteableEdit):
    def __init__(
        self, caption: str, edit_text: str = "", on_enter: Callable[[], None] = None
    ):
        super().__init__(caption, edit_text)
        self.on_enter = on_enter

    def keypress(self, size, key):
        # 按回车时，调用回调（如果有），不再冒泡
        if key == "enter" and callable(self.on_enter):
            self.on_enter()
            return None
        return super().keypress(size, key)


# ------------------------------------------------------------
# 各页面 widget
# ------------------------------------------------------------


class AboutPage(urwid.WidgetWrap):
    """“关于”页面，显示项目描述和作者信息。"""

    TEXT = (
        "项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader\n"
        "Fork From: https://github.com/Dlmily/Tomato-Novel-Downloader-Lite\n"
        "作者: zhongbai233 (https://github.com/zhongbai2333)\n"
        "项目早期代码: Dlmily (https://github.com/Dlmily)\n\n"
        "项目说明: 此项目基于 Dlmily 的项目 Fork 而来, 我对其进行重构+优化,\n"
        "添加更多功能, 包括: EPUB 下载支持、更好的断点传输、更好的错误管理等。\n"
        "本项目 *完全* 基于第三方 API, 未使用官方 API。\n"
        "本项目仅供 Python 网络爬虫技术、网页数据处理及相关研究的学习用途,\n"
        "请勿将其用于任何违反法律法规或侵犯他人权益的活动。"
    )

    def __init__(self, app: "TNDApp"):
        self.app = app
        txt = urwid.Text(self.TEXT)
        back_btn = menu_button("返回主菜单", lambda btn: self.app.show_main())
        piled = urwid.Pile(
            [
                urwid.Text(("title", "关于 / About")),
                urwid.Divider(),
                txt,
                urwid.Divider(),
                back_btn,
            ]
        )
        super().__init__(urwid.LineBox(urwid.Filler(piled, "top")))


def menu_button(label: str, callback):
    btn = urwid.Button(label)
    urwid.connect_signal(btn, "click", callback)
    return urwid.AttrMap(btn, None, focus_map="reversed")


class SavePathPage(urwid.WidgetWrap):
    """让用户输入保存路径，回车就等同点击“确定”"""

    def __init__(self, app: "TNDApp", book_id: str):
        self.app = app
        self.book_id = book_id
        default = app.config.default_save_dir
        # 输入框
        self.edit = PasteableEdit(f"保存路径 (默认: {default}): ")
        # 确定按钮
        ok_btn = menu_button("确定", lambda btn: self.on_confirm())
        pile = urwid.Pile([self.edit, urwid.Divider(), ok_btn])
        fill = urwid.Filler(pile, valign="top")
        super().__init__(urwid.LineBox(fill))

    def keypress(self, size, key):
        # 在输入框或按钮上按回车，都触发 on_confirm()
        if key == "enter":
            self.on_confirm()
            return None
        return super().keypress(size, key)

    def on_confirm(self):
        path = self.edit.edit_text.strip() or str(self.app.config.default_save_dir)
        self.app._download(self.book_id, path)


class ConfigMenu(urwid.WidgetPlaceholder):
    """配置编辑页面：
    - 布尔型选项用复选框 (CheckBox)；
    - “官方 API” 与 “helloplhm_qwq API” 互斥；
    - 当 helloplhm_qwq API 为 True 时，强制 max_workers=1，min_wait_time ≥ 1000，max_wait_time ≥ 1200。
    """

    # 配置项元数据：显示名称 -> (Config 属性名, 类型)
    OPTIONS = {
        "保存路径": ("save_path", str),
        "最大线程数": ("max_workers", int),
        "请求超时(秒)": ("request_timeout", int),
        "最大重试次数": ("max_retries", int),
        "最小等待时间(ms)": ("min_wait_time", int),
        "最大等待时间(ms)": ("max_wait_time", int),
        "小说保存格式(txt/epub)": ("novel_format", str),
        "优雅退出模式": ("graceful_exit", bool),
        "是否自动清理缓存文件": ("auto_clear_dump", bool),
        "是否使用官方API": ("use_official_api", bool),
        "是否使用 helloplhm_qwq API": ("use_helloplhm_qwq_api", bool),
        "是否以散装形式保存小说": ("bulk_files", bool),
        # —— 文本后处理（演示） ——
        "启用清洗/标点恢复": ("enable_postprocess", bool),
        "标点密度阈值(0~1)": ("pp_punct_density_threshold", float),
        "长句最小长度(字符)": ("pp_long_sentence_min_len", int),
        "逗号插入步长(演示)": ("pp_insert_chunk", int),
        # —— 本地 LLM 两级管线 ——
        "启用LLM两级管线": ("enable_llm_pipeline", bool),
        "LLM自动下载模型": ("llm_auto_model_download", bool),
        "LLM模型缓存目录": ("llm_models_dir", str),
        "HF镜像(域名或URL)": ("hf_endpoint", str),
        "检测模型Repo": ("llm_detector_repo", str),
        "检测模型文件名": ("llm_detector_filename", str),
        "精修模型Repo": ("llm_refiner_repo", str),
        "精修模型文件名": ("llm_refiner_filename", str),
        "检测转发阈值(0~1)": ("llm_detector_threshold", float),
        "B阶段比例上限(0~1)": ("llm_max_forward_ratio", float),
        "LLM上下文窗口": ("llm_ctx_window", int),
        "精修max_tokens": ("llm_max_tokens", int),
        "精修温度": ("llm_temperature", float),
    }

    def __init__(self, app: "TNDApp"):
        # 初始化时先用空白占位，接着 build 出真正的界面
        super().__init__(urwid.SolidFill())
        self.app = app
        self._build_view()

    def _build_view(self):
        """
        根据当前 config 的值重新构建整个页面。布尔项使用 CheckBox，
        其他项仍然用“点击弹编辑框”方式。
        """
        cfg = self.app.config
        body: List[urwid.Widget] = [
            urwid.Text(("title", "配置菜单 (按 q 返回)"), align="center"),
            urwid.Divider(),
        ]

        for name, (field, typ) in self.OPTIONS.items():
            cur_val = getattr(cfg, field, "N/A")

            # ----- 布尔型配置：用 CheckBox -----
            if typ is bool:
                # Create a CheckBox and capture `field` via default arg in lambda
                cb = urwid.CheckBox(label=name, state=bool(cur_val))
                urwid.connect_signal(
                    cb,
                    "change",
                    lambda checkbox, new_state, f=field: self._on_bool_toggle(
                        f, new_state
                    ),
                )
                body.append(cb)

            # ----- 其他类型：点击按钮弹出编辑框 -----
            else:
                label = f"{name}: {cur_val}"
                # 用 default arguments 把 field、typ、name “冻结”到 callback 里
                body.append(
                    menu_button(
                        label,
                        lambda btn, f=field, t=typ, n=name: self._edit(f, t, n),
                    )
                )

        body.append(urwid.Divider())
        body.append(menu_button("保存并返回主菜单", lambda btn: self._save_and_exit()))

        listbox = urwid.ListBox(urwid.SimpleFocusListWalker(body))
        self.original_widget = urwid.LineBox(listbox)

    def _on_bool_toggle(self, field: str, new_state: bool):
        """
        当某个布尔选项被勾选/取消时触发：
        - 更新对应的 config.<field> = new_state
        - 针对 use_official_api 与 use_helloplhm_qwq_api 做互斥
        - 如果启用了 helloplhm_qwq_api，则自动调整 max_workers、min_wait_time、max_wait_time
        - 保存 config 并重建整张页面
        """
        cfg = self.app.config
        setattr(cfg, field, new_state)

        # —— 互斥逻辑：使用官方 API 与 使用 helloplhm_qwq API 不能同时为 True ——
        if field == "use_official_api" and new_state:
            cfg.use_helloplhm_qwq_api = False

        if field == "use_helloplhm_qwq_api" and new_state:
            cfg.use_official_api = False

            # —— 当启用 helloplhm_qwq API 时，强制调整相关参数 ——
            changed = False
            if getattr(cfg, "max_workers", None) != 1:
                cfg.max_workers = 1
                changed = True
            if getattr(cfg, "min_wait_time", 0) < 1000:
                cfg.min_wait_time = 1000
                changed = True
            if getattr(cfg, "max_wait_time", 0) < 1200:
                cfg.max_wait_time = 1200
                changed = True

            if changed:
                self.app.show_popup(
                    "启用 helloplhm_qwq API 时已自动调整：\n"
                    "最大线程数 = 1；最小等待 ≥ 1000；最大等待 ≥ 1200。"
                )

        cfg.save()
        # 重建整个面板以刷新所有 CheckBox 和按钮文字状态
        self._build_view()

    def _edit(self, field: str, value_type: type, display_name: str):
        """
        点击非布尔项时弹出一个编辑页面，允许用户输入新值：
        保存时做类型转换与约束校验，再调用 cfg.save() 并回到完整配置页面。
        """
        cfg = self.app.config
        cur_val = getattr(cfg, field)
        caption = f"{display_name} (当前: {cur_val}) 新值: "
        edit = PasteableEdit(caption, edit_text=str(cur_val))
        save_btn = menu_button("保存", lambda btn: on_save())
        cancel_btn = menu_button("取消", lambda btn: self._build_view())

        pile = urwid.Pile([edit, urwid.Divider(), save_btn, cancel_btn])
        fill = urwid.Filler(pile, valign="top")
        self.original_widget = urwid.LineBox(fill)

        def on_save():
            raw = edit.edit_text.strip()
            try:
                # 1) 先做类型转换
                if value_type is bool:
                    new_val = raw.lower() in ("true", "1", "yes")
                else:
                    new_val = value_type(raw)

                # 2) 约束：如果当前启用了 helloplhm_qwq_api，就检查某些字段是否合法
                if getattr(cfg, "use_helloplhm_qwq_api", False):
                    if field == "max_workers" and new_val != 1:
                        raise ValueError("使用 helloplhm_qwq API 时最大线程数必须为 1")
                    if field == "min_wait_time" and new_val < 1000:
                        raise ValueError(
                            "使用 helloplhm_qwq API 时最小等待时间需 ≥ 1000ms"
                        )
                    if field == "max_wait_time" and new_val < 1200:
                        raise ValueError(
                            "使用 helloplhm_qwq API 时最大等待时间需 ≥ 1200ms"
                        )

                # 3) 互斥逻辑：如果用户在这里编辑“use_official_api”或“use_helloplhm_qwq_api”
                #    也要同步处理双方互斥，并在启用 helloplhm 时自动调整相关字段。
                if field == "use_official_api" and new_val:
                    cfg.use_helloplhm_qwq_api = False
                if field == "use_helloplhm_qwq_api" and new_val:
                    cfg.use_official_api = False
                    if cfg.max_workers != 1:
                        cfg.max_workers = 1
                    if cfg.min_wait_time < 1000:
                        cfg.min_wait_time = 1000
                    if cfg.max_wait_time < 1200:
                        cfg.max_wait_time = 1200
                    self.app.show_popup(
                        "由于启用 helloplhm_qwq API，已自动调整：\n"
                        "最大线程数 = 1；最小等待 ≥ 1000；最大等待 ≥ 1200。"
                    )

                # 4) 通过校验后，设置新值、保存并返回配置菜单
                setattr(cfg, field, new_val)
                cfg.save()
                self.app.show_popup(f"{display_name} 已保存为 {new_val}")
                self._build_view()

            except ValueError as ve:
                self.app.show_popup(str(ve))

    def _save_and_exit(self):
        """
        点击“保存并返回主菜单”时执行：config 已在每次改动时保存，直接回主菜单即可。
        """
        self.app.show_main()


class UpdateMenu(urwid.WidgetPlaceholder):
    """展示可更新小说列表，并将无更新的书籍放置到子菜单中"""

    def __init__(self, app: "TNDApp"):
        super().__init__(urwid.SolidFill())
        self.app = app
        # 分别存储有更新和无更新的书籍
        self._update_choices: List[Tuple[str, str]] = []
        self._no_update_choices: List[Tuple[str, str]] = []
        self._build()

    def _build(self):
        cfg = self.app.config
        save_dir = Path(cfg.default_save_dir)
        subdirs = list_subdirs(save_dir)

        # 清空上次的记录
        self._update_choices.clear()
        self._no_update_choices.clear()

        # 遍历所有子目录，分类
        for folder in subdirs:
            if "_" not in folder:
                continue
            book_id, book_name = folder.split("_", 1)
            try:
                chapters = self.app.network.fetch_chapter_list(book_id)
            except Exception as e:
                self.app.logger.error(f"获取章节列表失败: {e}")
                continue

            status_path = save_dir / folder / f"chapter_status_{book_id}.json"
            status = load_download_status(status_path)
            downloaded = status.get("downloaded", {})
            new_count = max(len(chapters) - len(downloaded), 0)
            desc = f"《{book_name}》({book_id}) — 新章节: {new_count}"

            if new_count > 0:
                self._update_choices.append((book_id, desc))
            else:
                self._no_update_choices.append((book_id, desc))

        # 如果两个列表都为空，则提示没有任何小说
        if not self._update_choices and not self._no_update_choices:
            self.app.show_popup("没有可更新或无更新的小说")
            return

        # 构建菜单主体
        body: List[urwid.Widget] = [
            urwid.Text(("title", "小说更新菜单 (q 返回)")),
            urwid.Divider(),
        ]

        # 有更新的书籍列表
        if self._update_choices:
            body.append(urwid.Text("有新章节的书籍："))
            for bid, desc in self._update_choices:
                body.append(menu_button(desc, lambda btn, b=bid: self._select(b)))
            body.append(urwid.Divider())

        # 无更新的书籍子菜单入口
        if self._no_update_choices:
            count = len(self._no_update_choices)
            body.append(
                menu_button(
                    f"无更新 ({count})", lambda btn: self._show_no_update_menu()
                )
            )
            body.append(urwid.Divider())

        # 返回主菜单
        body.append(menu_button("返回主菜单", lambda btn: self.app.show_main()))

        listbox = urwid.ListBox(urwid.SimpleFocusListWalker(body))
        self.original_widget = urwid.LineBox(listbox)

    def _select(self, book_id: str):
        """开始下载流程"""
        self.app.start_download_flow(book_id)

    def _show_no_update_menu(self):
        """显示无更新书籍的子菜单"""
        body: List[urwid.Widget] = [
            urwid.Text(("title", "无更新的书籍 (q 返回)")),
            urwid.Divider(),
        ]
        for bid, desc in self._no_update_choices:
            body.append(menu_button(desc, lambda btn, b=bid: self._select(b)))
        body.append(urwid.Divider())
        body.append(menu_button("返回更新菜单", lambda btn: self._build()))

        listbox = urwid.ListBox(urwid.SimpleFocusListWalker(body))
        self.original_widget = urwid.LineBox(listbox)


class InputPage(urwid.WidgetPlaceholder):
    """首页输入：<BookID/链接/书名>，或选择其它操作。"""

    def __init__(self, app: "TNDApp"):
        super().__init__(urwid.SolidFill())
        self.app = app
        self._build()

    def _build(self):
        banner = f"欢迎使用番茄小说下载器 v{VERSION}\n项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader"
        banner = urwid.Text(("title", banner))
        prompt = urwid.Text(("title", "输入小说ID/链接/书名，或选择其它操作"))
        self.edit = PasteableEdit("输入: ")
        ok_button = menu_button("确定", lambda btn: self.on_submit())
        cfg_button = menu_button("配置", lambda btn: self.app.show_config())
        up_button = menu_button("更新", lambda btn: self.app.show_update())
        about_button = menu_button("关于", lambda btn: self.app.show_about())
        quit_button = menu_button("退出", lambda btn: raise_exit())

        pile = urwid.Pile(
            [
                banner,
                urwid.Divider(),
                prompt,
                urwid.Divider(),
                self.edit,
                urwid.Divider(),
                ok_button,
                cfg_button,
                up_button,
                about_button,
                quit_button,
            ]
        )
        fill = urwid.Filler(pile, valign="top")
        self.original_widget = urwid.LineBox(fill)

    def on_submit(self):
        text = self.edit.edit_text.strip()
        if text == "":
            return
        self.app.handle_user_input(text)

    def keypress(self, size, key):
        """
        当焦点在 Edit 且按下 Enter 时，直接触发 on_submit()。
        其他情况则按默认逻辑继续分发。
        """
        if key == "enter":
            # 取出 LineBox 中的 Filler，再从 Filler.body 拿到 Pile
            linebox = self.original_widget
            fill = linebox.original_widget  # 这是我们当初传进去的 Filler(pile)
            if isinstance(fill, urwid.Filler):
                pile = fill.body  # pile 是 urwid.Pile
                focus_widget = pile.get_focus()  # 仅返回当前焦点 Widget
                if focus_widget is self.edit:
                    self.on_submit()
                    return None  # 吃掉这个按键，不再往下分发

        # 其余按键交给上层或下层继续处理
        return super().keypress(size, key)


class SearchMenu(urwid.WidgetPlaceholder):
    """
    搜索结果列表页面，展示一组 {title, book_id, author}。
    用户选中后，直接调用 app.start_download_flow(book_id)。
    """

    def __init__(self, app: "TNDApp", results: List[Dict[str, str]]):
        super().__init__(urwid.SolidFill())
        self.app = app
        self.results = results
        self._build()

    def _build(self):
        # 标题栏
        body: List[urwid.Widget] = [
            urwid.Text(("title", "搜索结果 (q 返回)")),
            urwid.Divider(),
        ]

        # 如果没有结果
        if not self.results:
            body.append(urwid.Text("未找到结果"))
        else:
            # 遍历搜索结果，每条显示“书名 | ID | 作者”
            for res in self.results:
                label = f"书名: {res['title']}  |  ID: {res['book_id']}  |  作者: {res['author']}"
                body.append(
                    menu_button(label, lambda btn, b=res["book_id"]: self._select(b))
                )

        body.append(urwid.Divider())
        body.append(menu_button("返回主菜单", lambda btn: self.app.show_main()))

        listbox = urwid.ListBox(urwid.SimpleFocusListWalker(body))
        self.original_widget = urwid.LineBox(listbox)

    def _select(self, book_id: str):
        # 用户选中后，直接进入下载流程
        self.app.start_download_flow(book_id)


class PreDownloadPage(urwid.WidgetWrap):
    """
    如果检测到已有下载记录，询问“继续下载剩余章节”还是“重新下载全部章节”。
    “继续下载”会使用原有缓存并只下载新章节，最终输出包含旧章节 + 新章节；
    “重新下载”会清空缓存，然后下载全部章节。
    如果没有下载记录，则跳转到 RangeSelectPage。
    """

    def __init__(self, app: "TNDApp", manager: BookManager, chapter_list: list[dict]):
        self.app = app
        self.manager = manager
        self.chapter_list = chapter_list

        widget = self.build_widget()
        super().__init__(widget)

    def build_widget(self):
        downloaded_failed = len(
            [v for v in self.manager.downloaded.values() if v[1] == "Error"]
        )
        downloaded_count = len(self.manager.downloaded) - downloaded_failed
        total = len(self.chapter_list)

        # 如果没有任何下载记录，直接进入 RangeSelectPage
        if downloaded_count == 0:
            # 发出 no_history 信号后，TNDApp 会切到 RangeSelectPage
            urwid.emit_signal(self, "no_history")
            return urwid.SolidFill()

        # 构建提示 UI
        header = urwid.Text(("title", "检测到已有下载记录"), align="center")
        info = urwid.Text(
            f"共 {total} 章，下载失败 {downloaded_failed} 章，已下载 {downloaded_count} 章"
        )
        cont_btn = menu_button(
            "继续下载剩余章节", lambda btn: self._continue_download()
        )
        redo_btn = menu_button("重新下载全部章节", lambda btn: self._redo_download())
        cancel_btn = menu_button("取消，返回主菜单", lambda btn: self.app.show_main())

        pile = urwid.Pile(
            [
                header,
                urwid.Divider(),
                info,
                urwid.Divider(),
                cont_btn,
                redo_btn,
                urwid.Divider(),
                cancel_btn,
            ]
        )
        fill = urwid.Filler(pile, valign="middle")
        return urwid.LineBox(fill)

    def _continue_download(self):
        """
        继续下载：直接下载剩余章节，不进入范围选择。
        """
        downloader = ChapterDownloader(self.manager.book_id, self.app.network)
        self.app.run_terminal_download(self.manager, downloader, self.chapter_list)

    def _redo_download(self):
        """
        重新下载：清空历史记录后进入 RangeSelectPage，让用户选择下载范围。
        """
        # 清空已有缓存记录
        self.manager.downloaded.clear()
        # reset_history=True 表示重新下载全部章节，可在 RangeSelectPage 里选择具体区间
        self.app.show_range_page(self.manager, self.chapter_list, reset_history=True)


class RangeSelectPage(urwid.WidgetWrap):
    """
    让用户选择“全部下载”还是“指定区间下载”。
    如果用户在输入框中按回车：
      · 起始+结束都填了 → 自动执行区间下载；
      · 否则 → 自动执行全部下载。
    """

    def __init__(
        self,
        app: "TNDApp",
        manager: BookManager,
        chapter_list: list[dict],
        reset_history: bool,
    ):
        self.app = app
        self.manager = manager

        # 计算待下载列表
        if reset_history:
            self.pending = chapter_list.copy()
        else:
            self.pending = [
                ch
                for ch in chapter_list
                if not (
                    ch["id"] in manager.downloaded
                    and manager.downloaded[ch["id"]][1] != "Error"
                )
            ]

        # 记录总数与首章号
        self.total = len(chapter_list)
        self.first_idx = int(chapter_list[0]["index"]) + 1 if chapter_list else 1

        # 构造界面
        super().__init__(self._build_widget())

    def _build_widget(self):
        if not self.pending:
            # 没有待下载章节
            self.app.show_popup("没有章节需要下载，操作结束")
            return urwid.SolidFill()

        header = urwid.Text(("title", "请选择下载方式"), align="center")
        info = urwid.Text(
            f"可下载章节共 {len(self.pending)} 章（从 {self.first_idx} 到 {self.total}）"
        )

        # 使用 EnterEdit，让它回车时跑 self._on_enter()
        self.lo_edit = EnterEdit("起始章节号 (如 1): ", on_enter=self._on_enter)
        self.hi_edit = EnterEdit("结束章节号 (如 50): ", on_enter=self._on_enter)

        # 仍保留按钮，鼠标/焦点切换后也可点击
        all_btn = menu_button("全部下载", lambda btn: self._on_all())
        range_btn = menu_button("按区间下载", lambda btn: self._on_range())
        cancel_btn = menu_button("取消，返回主菜单", lambda btn: self.app.show_main())

        pile = urwid.Pile(
            [
                header,
                urwid.Divider(),
                info,
                urwid.Divider(),
                self.lo_edit,
                self.hi_edit,
                urwid.Divider(),
                all_btn,
                range_btn,
                urwid.Divider(),
                cancel_btn,
            ]
        )
        fill = urwid.Filler(pile, valign="middle")
        return urwid.LineBox(fill)

    def _on_enter(self):
        """
        回车统一入口：如果两个框都填了，就按区间下载，
        否则一律全部下载。
        """
        lo = self.lo_edit.edit_text.strip()
        hi = self.hi_edit.edit_text.strip()
        if lo and hi:
            self._on_range()
        else:
            self._on_all()

    def _on_all(self):
        downloader = ChapterDownloader(self.manager.book_id, self.app.network)
        self.app.run_terminal_download(self.manager, downloader, self.pending)

    def _on_range(self):
        lo_text = self.lo_edit.edit_text.strip()
        hi_text = self.hi_edit.edit_text.strip()
        try:
            lo, hi = int(lo_text), int(hi_text)
        except ValueError:
            self.app.show_popup(
                f"请输入有效数字，范围应在 {self.first_idx} 到 {self.total} 之间"
            )
            return

        if not (self.first_idx <= lo <= hi <= self.total):
            self.app.show_popup(
                f"范围不合法，应在 {self.first_idx} 到 {self.total} 之间，且 起始 ≤ 结束"
            )
            return

        filtered = [ch for ch in self.pending if lo - 1 <= int(ch["index"]) <= hi - 1]
        if not filtered:
            self.app.show_popup("未找到符合该范围的章节")
            return

        downloader = ChapterDownloader(self.manager.book_id, self.app.network)
        self.app.run_terminal_download(self.manager, downloader, filtered)


class TNDApp:
    """终端 UI 主应用封装。"""

    def __init__(self):
        # ================= 原上下文 =================
        self.logger = GlobalContext.get_logger()
        self.config: Config = GlobalContext.get_config()
        self.network = NetworkClient()
        self.update_mgr = UpdateManager()
        # 初始化时可检查更新（可注释掉）
        if not GlobalContext.get_log_system().debug:
            self.update_mgr.check_for_updates()

        # ================= urwid 相关 ================
        self._o_count = 0
        self._should_restart = False
        self.main_placeholder = urwid.WidgetPlaceholder(urwid.SolidFill())
        self.loop = urwid.MainLoop(
            self.main_placeholder,
            palette=PALETTE,
            unhandled_input=self._global_key_handler,
        )
        self.show_main()

    # --------------------------------------------------------
    # 页面切换方法
    # --------------------------------------------------------
    def show_main(self):
        self.main_placeholder.original_widget = InputPage(self)

    def show_config(self):
        self.main_placeholder.original_widget = ConfigMenu(self)

    def show_update(self):
        self.main_placeholder.original_widget = UpdateMenu(self)

    def show_about(self):
        self.main_placeholder.original_widget = AboutPage(self)

    def show_popup(self, text: str):
        """
        在当前界面叠加一个 MessagePopup 弹窗，按 q/Q/Enter/Esc 时仅关闭弹窗并恢复到弹前界面。
        """

        # 1. 先把“弹窗之前”的界面保存起来
        old_widget = self.main_placeholder.original_widget

        # 2. 定义关闭回调：把界面恢复到 old_widget，并强制重绘
        def on_close():
            self.main_placeholder.original_widget = old_widget
            self.loop.draw_screen()

        # 3. 创建一个新的 MessagePopup，把 on_close() 传进去
        popup = MessagePopup(text, on_close=on_close, width=60)

        # 4. 叠加：让 popup 直接处于最顶层
        overlay = urwid.Overlay(
            popup,  # 顶层 widget，一定是 MessagePopup 实例本身
            old_widget,  # 底层原界面
            align="center",
            width=("relative", 60),
            valign="middle",
            height=("relative", 40),
        )

        # 5. 直接把 overlay 放到 main_placeholder 上，让它立即生效
        self.main_placeholder.original_widget = overlay
        self.loop.draw_screen()

    def show_cover_preview(self, book_name: str):
        """
        暂时退出 Urwid 界面，打印整屏 ASCII 封面，用户按任意键后恢复到原来的 UI。
        """
        # 1. 找到封面图片路径
        cover_path = Path(self.config.get_status_folder_path) / f"{book_name}.jpg"
        if not cover_path.exists():
            # 如果没有封面文件，跳出一个弹窗提示
            self.show_popup("未找到封面图片，无法预览")
            return

        # 2. 先把 Urwid 界面画面清掉，恢复到普通终端模式
        self.loop.screen.clear()
        # 这一行会立刻刷新屏幕，让 Urwid 的内容消失
        self.loop.screen.set_terminal_properties(colors=256)

        # 3. 调用 preview_ascii，打印封面 ASCII 到屏幕
        preview_ascii(cover_path)

        # 4. 等待用户按任意键（因为我们现在已经不在 Urwid 的 MainLoop 里）
        try:
            self._pause_mouse_tracking()
            input("\n\n<按回车键返回>")
            self._resume_mouse_tracking()
        except Exception:
            pass

        # 5. 用户按回车后，重新绘制原先的 TUI 界面
        # 先清屏，然后告诉 Urwid 画一次当前画面
        self.loop.screen.clear()
        self.loop.draw_screen()

    def show_pre_download_page(self, manager: BookManager, chapter_list: list[dict]):
        """
        直接在 _download 中被调用，
        如果 manager.downloaded 非空，则显示 PreDownloadPage，
        否则直接跳到 RangeSelectPage。
        """
        downloaded_failed = len(
            [v for v in manager.downloaded.values() if v[1] == "Error"]
        )
        downloaded_count = len(manager.downloaded) - downloaded_failed
        if downloaded_count == 0:
            # 无历史记录，直接跳到 RangeSelectPage (reset_history=False)
            self.show_range_page(manager, chapter_list, reset_history=False)
        else:
            # 有历史记录，显示 PreDownloadPage
            self.main_placeholder.original_widget = PreDownloadPage(
                self, manager, chapter_list
            )

    def show_range_page(
        self, manager: BookManager, chapter_list: list[dict], reset_history: bool
    ):
        """
        由 PreDownloadPage 调用，也可直接调用：
        如果用户在 PreDownloadPage 里选了“继续”或“重置”，就会来到这里。
        """
        self.main_placeholder.original_widget = RangeSelectPage(
            self, manager, chapter_list, reset_history
        )

    # --------------------------------------------------------
    # 全局按键处理（如 q 返回）
    # --------------------------------------------------------
    def _global_key_handler(self, key: str):
        # 跟踪连续三次小写 o
        if key == "o":
            self._o_count += 1
            if self._o_count >= 3:
                # 1) 发出提示音
                try:
                    if sys.platform.startswith("win"):
                        import winsound
                        # 1000Hz，持续200ms
                        winsound.Beep(1000, 200)
                    else:
                        # ASCII BEL
                        sys.stdout.write("\a")
                        sys.stdout.flush()
                except Exception:
                    pass
                # 连续 3 次 o，打标记并退出主循环
                self.config.old_cli = True
                self.config.save()
                self._should_restart = True
                raise urwid.ExitMainLoop()
                return
        else:
            self._o_count = 0

        # 原有的 q/Q 逻辑
        if key in ("q", "Q"):
            current = self.main_placeholder.original_widget
            if isinstance(current, InputPage):
                raise urwid.ExitMainLoop()
            self.show_main()

    def _pause_mouse_tracking(self):
        """
        暂停鼠标捕获，让 input() 只接收键盘，不会把鼠标左右键当输入字符。
        """
        try:
            # Urwid 的 screen 对象提供 set_mouse_tracking(False)
            self.loop.screen.set_mouse_tracking(False)
        except Exception:
            pass

    def _resume_mouse_tracking(self):
        """
        恢复鼠标捕获，让 Urwid 继续处理鼠标事件。
        """
        try:
            self.loop.screen.set_mouse_tracking(True)
        except Exception:
            pass

    # --------------------------------------------------------
    # 解析用户输入并启动下载流程
    # --------------------------------------------------------
    def handle_user_input(self, text: str):
        if text == "ooo":
            # 1) 发出提示音
            try:
                if sys.platform.startswith("win"):
                    import winsound
                    # 1000Hz，持续200ms
                    winsound.Beep(1000, 200)
                else:
                    # ASCII BEL
                    sys.stdout.write("\a")
                    sys.stdout.flush()
            except Exception:
                pass
            self.config.old_cli = True
            self.config.save()
            self._should_restart = True
            raise urwid.ExitMainLoop()
            return

        book_id: Optional[str] = None

        # ① 尝试从 URL 中提取 book_id
        urls = re.findall(r"(https?://[^\s]+)", text)
        if urls:
            url_str = urls[0]
            parsed = urlparse(url_str)
            m = re.search(r"/page/(\d+)", parsed.path)
            if m:
                book_id = m.group(1)
            else:
                qs = parse_qs(parsed.query)
                bid_list = qs.get("book_id") or qs.get("bookId")
                if bid_list:
                    book_id = bid_list[0]
            if not book_id:
                self.show_popup("无法从链接中解析 book_id")
                return

        # ② 如果全数字，则直接当作 book_id
        elif text.isdigit():
            book_id = text

        # ③ 作为书名搜索
        else:
            try:
                # 调用改后的 search_book，得到一个列表
                results = self.network.search_book(text)
            except Exception:
                results = []

            # 如果列表为空
            if not results:
                self.show_popup("未搜索到对应书籍 / API 异常")
                return

            # 让用户在新页面里选择一本
            self.main_placeholder.original_widget = SearchMenu(self, results)
            return

        # 如果前面已经拿到 book_id，直接启动下载
        self.start_download_flow(book_id)

    # --------------------------------------------------------
    # 下载流程入口：先选择保存路径
    # --------------------------------------------------------
    def start_download_flow(self, book_id: str):
        """用 SavePathPage 代替原来的内联 Edit+按钮"""
        self.main_placeholder.original_widget = SavePathPage(self, book_id)

    # 修改 _download，让它先进入“检查历史/选择范围”流程
    def _download(self, book_id: str, save_path: str):
        logger = self.logger
        cfg = self.config
        network = self.network

        # 获取书籍信息（保持不变）
        book_info = network.get_book_info(book_id)
        if book_info[0] is None:
            self.show_popup("获取书籍信息失败")
            return
        book_name, author, description, tags, chapter_count = book_info

        # 显示书籍信息 + 按钮 (不变，只改变开始下载的回调)
        info_lines = [
            f"书名: {book_name}",
            f"作者: {author}",
            f"是否完结: {tags[0]} | 共 {chapter_count} 章",
            f"标签: {' | '.join(tags[1:])}",
            f"简介: {description[:50]}…",
            "\n请选择：",
        ]
        txt = urwid.Text("\n".join(info_lines))
        preview_btn = menu_button(
            "预览封面", lambda btn: self.show_cover_preview(book_name)
        )
        ok_btn = menu_button("开始下载", lambda btn: on_confirm())
        cancel_btn = menu_button("返回主菜单", lambda btn: self.show_main())

        pile = urwid.Pile(
            [txt, urwid.Divider(), ok_btn, urwid.Divider(), preview_btn, cancel_btn]
        )
        self.main_placeholder.original_widget = urwid.LineBox(urwid.Filler(pile, "top"))

        def on_confirm():
            manager = BookManager(
                save_path, book_id, book_name, author, tags, description
            )
            downloader = ChapterDownloader(book_id, network)
            chapter_list = network.fetch_chapter_list(book_id)
            # 进入“检查历史/选择范围”的流程
            self.show_pre_download_page(manager, chapter_list)

    def run_terminal_download(
        self, manager: BookManager, downloader: ChapterDownloader, chapters: list[dict]
    ):
        """
        切换到“终端模式”运行 download_book，等下载结束后再恢复 UI。
        """
        # 1. 用空白覆盖 Urwid 并刷新（保持 UI 仍在后台，只是马上清屏）
        try:
            self.main_placeholder.original_widget = urwid.SolidFill()
            self.loop.draw_screen()
        except:
            pass

        # 2. 让 tqdm 读到最新终端宽度
        cols, _ = shutil.get_terminal_size(fallback=(80, 24))
        os.environ["COLUMNS"] = str(cols)

        # 3. 第一次下载
        try:
            result = downloader.download_book(manager, manager.book_name, chapters)
        except Exception as e:
            # 异常时也先暂停鼠标捕获，再做 input
            self._pause_mouse_tracking()
            print(f"\n下载异常中止: {e}\n")
            choice = input("按回车返回 TUI …")
            # input 结束后再重新开启鼠标（如果需要的话）
            self._resume_mouse_tracking()
            # 清屏并把 Urwid 主界面重新绘制
            os.system("cls" if os.name == "nt" else "clear")
            self.show_main()
            return

        # 4. 如果有失败章节，询问是否重试
        if result.get("failed", 0) > 0:
            failed_ids = [
                cid
                for cid, status in manager.downloaded.items()
                if status[1] == "Error"
            ]

            # 暂停鼠标捕获，让 input() 只接收键盘
            self._pause_mouse_tracking()
            print(f"\n共有 {len(failed_ids)} 章下载失败。")
            retry_input = input("是否重试下载失败章节？(Y/n): ").strip().lower()
            # 恢复鼠标捕获，以便后续仍能点击返回 TUI
            self._resume_mouse_tracking()

            if retry_input in ("", "y", "yes"):
                failed_chapters = [ch for ch in chapters if ch["id"] in failed_ids]
                if failed_chapters:
                    # 再次清屏，以便日志整洁
                    os.system("cls" if os.name == "nt" else "clear")
                    try:
                        result = downloader.download_book(
                            manager, manager.book_name, failed_chapters
                        )
                    except Exception as e2:
                        # 再次出错时，把鼠标再暂停输入
                        self._pause_mouse_tracking()
                        print(f"\n重试时出现异常：{e2}\n")
                        input("按回车继续 …")
                        self._resume_mouse_tracking()
                    else:
                        print(
                            f"\n重试完成！本次重试成功 {result['success']} 章，取消 {result['canceled']} 章，仍失败 {result['failed']} 章。\n"
                        )
                        input("按回车继续 …")

        # 5. 最终提示并等回车
        # 同样暂停鼠标捕获，再让用户回车，然后恢复鼠标
        self._pause_mouse_tracking()
        print(
            f"\n最终结果：成功 {result['success']} 章，取消 {result['canceled']} 章，失败 {result['failed']} 章。\n"
        )
        input("按回车返回 TUI …")
        self._resume_mouse_tracking()

        # 6. 清屏并恢复Urwid主界面
        os.system("cls" if os.name == "nt" else "clear")
        self.show_main()

    # --------------------------------------------------------
    # 启动应用
    # --------------------------------------------------------
    def run(self):
        # 欢迎 Banner（打印到日志中，同时可见于终端）
        banner = (
            f"欢迎使用番茄小说下载器 v{VERSION}\n"
            "项目地址: https://github.com/zhongbai2333/Tomato-Novel-Downloader\n"
            "(q 退出, Enter 选择)\n"
        )
        self.logger.info(banner)
        self.loop.run()


def raise_exit():
    """用于按钮回调中直接退出程序。"""
    raise urwid.ExitMainLoop()


def main():
    """应用入口。"""
    if not GlobalContext.get_config().old_cli:
        app = TNDApp()
        try:
            app.run()
        except urwid.ExitMainLoop:
            # 捕获 Urwid 的退出信号，不在这里输出任何内容
            pass
        finally:
            # 程序无论是正常退出还是异常触发，都主动清屏一次
            # 用 Linux/Unix 下的 clear 命令；如果你未来要兼容 Windows，可用 "cls"
            os.system("clear" if os.name != "nt" else "cls")
        
        if getattr(app, "_should_restart", False):
            os.execv(sys.executable, [sys.executable] + sys.argv)
    else:
        old_main()

    # 退出前可以再输出一句“再见”，这时已经是清空后的屏幕
    print("再见！")
