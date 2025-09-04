import re
from bs4 import BeautifulSoup
from typing import List, Optional
from .model_manager import ModelManager
from ..base_system.context import GlobalContext
from .llm_adapter import LlamaCppRunner

CN_PUNCTS = "，。！？；：、“”‘’（）—…《》【】『』「」"
HALF_PUNCTS = ",.!?;:'\"()[]<>"
ALL_PUNCTS = CN_PUNCTS + HALF_PUNCTS


def normalize_whitespace(text: str) -> str:
    # 去除全角空格、NBSP，合并多空行，修正行尾空格
    text = text.replace("\u00A0", " ").replace("\u3000", " ")
    # 去掉行尾空格
    text = re.sub(r"[ \t]+$", "", text, flags=re.MULTILINE)
    # 合并超过 2 行的空行
    text = re.sub(r"\n{3,}", "\n\n", text)
    return text


def full_half_width_unify(text: str) -> str:
    # 简单把英文标点转为中文风格（演示），保留引号配对
    repl = {
        ",": "，",
        ".": "。",
        "!": "！",
        "?": "？",
        ";": "；",
        ":": "：",
        "(": "（",
        ")": "）",
    }
    return text.translate(str.maketrans(repl))


def punctuation_density(text: str) -> float:
    if not text:
        return 0.0
    # 只统计可见汉字与数字字母
    visible = re.sub(r"\s+", "", text)
    if not visible:
        return 0.0
    count = sum(1 for ch in text if ch in ALL_PUNCTS)
    return count / max(1, len(visible))


def naive_punctuation_restore(sentence: str, insert_chunk: int = 30) -> str:
    """
    演示级“标点恢复”：
    - 若句内几乎无标点，则按固定间距插入中文逗号，末尾补句号。
    - 避免在数字/百分号/小数点后紧跟插入。
    注：仅 demo，真实项目建议替换为 ONNX 的标点恢复小模型。
    """
    s = sentence.strip()
    if not s:
        return s
    if any(p in s for p in "。！？；!?;"):
        return s

    out: List[str] = []
    last = 0
    while last < len(s):
        nxt = min(len(s), last + insert_chunk)
        seg = s[last:nxt]
        out.append(seg)
        last = nxt
        if last < len(s):
            # 不在数字/小数点/百分号后插入
            prev = s[last - 1]
            if not re.match(r"[0-9.%]", prev):
                out.append("，")
    if out and out[-1] not in ("。", "！", "？"):
        out.append("。")
    return "".join(out)


def process_plain_text(text: str, punct_density_threshold: float, long_sentence_min_len: int, insert_chunk: int) -> str:
    # 规则清洗
    text = normalize_whitespace(text)
    text = full_half_width_unify(text)

    # 对过长且低标点密度的行做粗略恢复
    lines = text.splitlines()
    fixed_lines: List[str] = []
    for line in lines:
        if len(line) >= long_sentence_min_len and punctuation_density(line) < punct_density_threshold:
            fixed_lines.append(naive_punctuation_restore(line, insert_chunk=insert_chunk))
        else:
            fixed_lines.append(line)
    return "\n".join(fixed_lines)


def process_html(html: str, punct_density_threshold: float, long_sentence_min_len: int, insert_chunk: int) -> str:
    soup = BeautifulSoup(html, "html.parser")
    # 只处理段落文本，不动 img 与特殊块
    for p in soup.find_all("p"):
        txt = p.get_text()
        new_txt = process_plain_text(txt, punct_density_threshold, long_sentence_min_len, insert_chunk)
        if new_txt != txt:
            p.string = new_txt
    return str(soup)


class TextPostProcessor:
    @staticmethod
    def process_content(
        content: str,
        fmt: str = "epub",
        punct_density_threshold: float = 0.01,
        long_sentence_min_len: int = 60,
        insert_chunk: int = 30,
        chapter_label: Optional[str] = None,
    ) -> str:
        if not content:
            return content
        cfg = GlobalContext.get_config()
        logger = GlobalContext.get_logger()

        # 1) 规则 + 演示标点恢复
        processed = process_html(content, punct_density_threshold, long_sentence_min_len, insert_chunk) if fmt == "epub" else process_plain_text(content, punct_density_threshold, long_sentence_min_len, insert_chunk)

    # 2) 可选：两级 LLM（A 整章检测 + B 分段精修）
        if not getattr(cfg, "enable_llm_pipeline", False):
            return processed

        # 准备 llama.cpp 运行器与模型路径
        models_root = ModelManager.get_models_dir(
            default_base=cfg.status_folder_path(book_name="_models", book_id="_sys"),
            override=getattr(cfg, "llm_models_dir", "")
        )
        det_path = ModelManager.ensure_local_file(
            models_dir=models_root,
            repo_id=getattr(cfg, "llm_detector_repo", ""),
            filename=getattr(cfg, "llm_detector_filename", ""),
            auto_download=getattr(cfg, "llm_auto_model_download", True),
            logger=logger,
        )
        ref_path = ModelManager.ensure_local_file(
            models_dir=models_root,
            repo_id=getattr(cfg, "llm_refiner_repo", ""),
            filename=getattr(cfg, "llm_refiner_filename", ""),
            auto_download=getattr(cfg, "llm_auto_model_download", True),
            logger=logger,
        )
        if not det_path or not ref_path:
            return processed

        try:
            det_runner = LlamaCppRunner.get_or_create(det_path, n_ctx=getattr(cfg, "llm_ctx_window", 2048))
            ref_runner = LlamaCppRunner.get_or_create(ref_path, n_ctx=getattr(cfg, "llm_ctx_window", 2048))
        except Exception as e:
            logger.warning(f"加载本地 LLM 失败：{e}")
            return processed

        threshold = float(getattr(cfg, "llm_detector_threshold", 0.55))
        max_ratio = float(getattr(cfg, "llm_max_forward_ratio", 0.2))

        # 先整章检测：若不达阈值则跳过整章，避免逐段检测开销
        whole_text = BeautifulSoup(processed, "html.parser").get_text("\n") if fmt == "epub" else processed
        try:
            det_score, det_issues = det_runner.detect(whole_text.strip()[:8000])  # 截断到 8k 字符
        except Exception:
            det_score = 0.0
        if det_score < threshold:
            return processed

        # 针对 HTML/txt 的段落切分
        if fmt == "epub":
            soup = BeautifulSoup(processed, "html.parser")
            ps = soup.find_all("p")
            n = len(ps)
            if n == 0:
                return processed
            # 分段候选：仍按“长且低标点密度”筛选，但不逐段二次检测；直接进入 B 精修候选
            MAX_SCAN = 24
            suspicious = []
            for idx, p in enumerate(ps):
                txt = p.get_text().strip()
                if not txt:
                    continue
                if len(txt) < long_sentence_min_len or punctuation_density(txt) >= punct_density_threshold:
                    continue
                suspicious.append((idx, 1.0))
                if len(suspicious) >= MAX_SCAN:
                    break
            # 控制比例
            suspicious.sort(key=lambda x: x[1], reverse=True)
            limit = max(0, int(n * max_ratio))
            suspicious = suspicious[:limit] if limit > 0 else []
            # B 精修
            changed = 0
            for idx, _ in suspicious:
                p = ps[idx]
                original = p.get_text()
                refined = ref_runner.refine(
                    original,
                    max_tokens=min(48, int(getattr(cfg, "llm_max_tokens", 128) or 48)),
                    temperature=min(0.2, float(getattr(cfg, "llm_temperature", 0.1) or 0.1)),
                )
                # 安全网：长度差异过大时回退
                if len(refined) < 0.4 * len(original) or len(refined) > 2.0 * len(original):
                    continue
                if refined != original:
                    p.string = refined
                    changed += 1
            if changed > 0:
                logger.info(f"LLM精修：章节[{chapter_label or 'unknown'}] 修改段落 {changed}/{n}（整章触发，score={det_score:.2f}）")
            return str(soup)
        else:
            lines = processed.splitlines()
            n = len(lines)
            if n == 0:
                return processed
            # 分行候选：按“长且低标点密度”筛选，不再逐行检测
            MAX_SCAN = 36
            suspicious = []
            for i, line in enumerate(lines):
                text = line.strip()
                if not text:
                    continue
                if len(text) < long_sentence_min_len or punctuation_density(text) >= punct_density_threshold:
                    continue
                suspicious.append((i, 1.0))
                if len(suspicious) >= MAX_SCAN:
                    break
            suspicious.sort(key=lambda x: x[1], reverse=True)
            limit = max(0, int(n * max_ratio))
            suspicious = suspicious[:limit] if limit > 0 else []
            changed = 0
            for i, _ in suspicious:
                original = lines[i]
                refined = ref_runner.refine(
                    original,
                    max_tokens=min(48, int(getattr(cfg, "llm_max_tokens", 128) or 48)),
                    temperature=min(0.2, float(getattr(cfg, "llm_temperature", 0.1) or 0.1)),
                )
                if len(refined) < 0.4 * len(original) or len(refined) > 2.0 * len(original):
                    continue
                if refined != original:
                    lines[i] = refined
                    changed += 1
            if changed > 0:
                logger.info(f"LLM精修：章节[{chapter_label or 'unknown'}] 修改行数 {changed}/{n}（整章触发，score={det_score:.2f}）")
            return "\n".join(lines)
