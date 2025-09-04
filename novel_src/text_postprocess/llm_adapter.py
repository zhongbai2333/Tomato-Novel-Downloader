import json
import os
import time
from pathlib import Path
from typing import Optional, Tuple
from ..base_system.context import GlobalContext


_RUNNER_CACHE = {}


class LlamaCppRunner:
    def __init__(self, model_path: Path, n_ctx: int = 2048, n_threads: Optional[int] = None):
        try:
            import importlib
            llama_cpp = importlib.import_module("llama_cpp")
            Llama = getattr(llama_cpp, "Llama")
        except Exception as e:
            raise RuntimeError("缺少 llama-cpp-python，无法加载 GGUF 模型") from e

        self._llama_cls = Llama
        self.model_path = str(model_path)
        self.ctx = n_ctx
        # 线程数
        if n_threads is None or n_threads == 0:
            try:
                import os as _os
                self.n_threads = max(1, _os.cpu_count() or 1)
            except Exception:
                self.n_threads = 4
        else:
            self.n_threads = n_threads

        # GPU 设置
        cfg = GlobalContext.get_config()
        n_gpu_layers = 0
        if getattr(cfg, "llm_use_gpu_auto", True):
            # -1 表示尽量多，尝试启用 GPU；否则使用用户配置
            n_gpu_layers = int(getattr(cfg, "llm_n_gpu_layers", -1))
            if n_gpu_layers == 0:
                n_gpu_layers = 0
        else:
            n_gpu_layers = 0

        logger = GlobalContext.get_logger()
        logger.debug(
            f"[llama.cpp] init model={self.model_path}, n_ctx={self.ctx}, threads={self.n_threads}, n_gpu_layers={n_gpu_layers}"
        )

        # 尽量安静：设置环境变量与回调抑制底层提示
        os.environ.setdefault("LLAMA_LOG_LEVEL", "40")  # 兼容性：高等级减少噪声
        try:
            if hasattr(llama_cpp, "llama_log_set"):
                def _noop(level, text, user_data):
                    return None
                llama_cpp.llama_log_set(_noop, None)
        except Exception:
            pass

        # 初始化 Llama；尝试 GPU，失败则回退 CPU
        try:
            self._llm = self._llama_cls(
                model_path=self.model_path,
                n_ctx=self.ctx,
                n_threads=self.n_threads,
                n_gpu_layers=n_gpu_layers,
                verbose=False,
            )
        except Exception as e:
            logger.warning(f"[llama.cpp] GPU 初始化失败，将回退 CPU：{e}")
            self._llm = self._llama_cls(
                model_path=self.model_path,
                n_ctx=self.ctx,
                n_threads=self.n_threads,
                n_gpu_layers=0,
                verbose=False,
            )

    @staticmethod
    def get_or_create(model_path: Path, n_ctx: int = 2048, n_threads: Optional[int] = None):
        key = (str(model_path), n_ctx, n_threads)
        if key not in _RUNNER_CACHE:
            _RUNNER_CACHE[key] = LlamaCppRunner(model_path, n_ctx=n_ctx, n_threads=n_threads)
        return _RUNNER_CACHE[key]

    def _complete(self, prompt: str, max_tokens: int = 128, temperature: float = 0.1) -> str:
        logger = GlobalContext.get_logger()
        cfg = GlobalContext.get_config()
        t0 = time.perf_counter()
        if getattr(cfg, "llm_debug_verbose", False):
            logger.debug(f"[llama.cpp] prompt start, max_tokens={max_tokens}, temp={temperature}")
        out = self._llm(
            prompt=prompt,
            max_tokens=max_tokens,
            temperature=temperature,
            stop=["</DONE>", "\n\n"],
        )
        text = out["choices"][0]["text"].strip()
        dt = (time.perf_counter() - t0) * 1000
        if getattr(cfg, "llm_debug_verbose", False):
            logger.debug(f"[llama.cpp] prompt done, gen_len={len(text)}, took={dt:.0f}ms")
        return text

    def detect(self, text: str) -> Tuple[float, list]:
        """返回 (score, issue_types)。失败时返回 (0.0, [])."""
        prompt = (
            "你是一个中文文本质量检测器。仅检测，不修改。\n"
            "请对以下文本判断是否存在明显错误（错别字、漏标点导致的长句、语义不通顺、用词不当）。\n"
            "输出严格 JSON：{\"score\":0..1,\"issue_types\":[...]}，score 越高表示越需要修正。\n"
            "文本：\n" + text + "\n"
            "</DONE>"
        )
        try:
            resp = self._complete(prompt, max_tokens=128, temperature=0.1)
            # 提取第一个 JSON
            start = resp.find("{")
            end = resp.rfind("}")
            if start == -1 or end == -1 or end <= start:
                return 0.0, []
            obj = json.loads(resp[start : end + 1])
            score = float(obj.get("score", 0.0))
            issues = obj.get("issue_types", [])
            if not isinstance(issues, list):
                issues = []
            return max(0.0, min(1.0, score)), issues
        except Exception:
            return 0.0, []

    def refine(self, text: str, max_tokens: int = 128, temperature: float = 0.1) -> str:
        """
        最小编辑：仅修正错别字与标点/断句；不改措辞，不增删信息；保留 HTML 标签；若无法判断请原样返回。
        输出直接给出修正后的文本，不加解释。
        """
        prompt = (
            "你是一个中文文本校对器。只进行最小必要修改：修正错别字、标点和断句。\n"
            "严格要求：\n- 不改变句子风格与措辞\n- 不增删信息\n- 保留文本中的 HTML 标签结构\n- 无需解释，直接输出修正结果\n"
            "若文本无需修改，请原样输出。\n"
            "文本：\n" + text + "\n"
            "</DONE>"
        )
        try:
            resp = self._complete(prompt, max_tokens=max_tokens, temperature=temperature)
            return resp
        except Exception:
            return text
