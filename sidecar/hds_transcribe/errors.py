"""Структурированные ошибки воркера: `{code, message, hint}` (§5.2).

Коды — часть контракта: Rust (`hds-transcribe`) превращает их в текст задания,
а `fallback_cpu` ретраит на CPU только `cuda_oom`/`dml_oom`/`onnx_error`.
"""
from __future__ import annotations

#: Ошибки, после которых имеет смысл одна попытка стадии на CPU.
CPU_FALLBACK_CODES = {"cuda_oom", "dml_oom", "onnx_error"}


class JobError(Exception):
    """Ошибка задания с машинным кодом и подсказкой человеку."""

    def __init__(self, code: str, message: str, hint: str = "") -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.hint = hint

    def to_dict(self) -> dict:
        out = {"code": self.code, "message": self.message}
        if self.hint:
            out["hint"] = self.hint
        return out

    def with_prefix(self, prefix: str) -> "JobError":
        return JobError(self.code, "%s: %s" % (prefix, self.message), self.hint)


def is_cpu_fallback(err: BaseException) -> bool:
    """Повторять ли стадию на CPU после этой ошибки."""
    return isinstance(err, JobError) and err.code in CPU_FALLBACK_CODES


def is_oom(err: BaseException) -> bool:
    """Похоже ли исключение на OOM (torch/ORT/DirectML)."""
    text = "%s: %s" % (type(err).__name__, err)
    low = text.lower()
    return "out of memory" in low or "cuda error" in low or "cublas" in low


def cancelled() -> JobError:
    """Отмена задания (stop_file/`cancel`)."""
    return JobError("cancelled", "задание отменено")


def as_job_error(err: BaseException, default: str = "internal_error", hint: str = "") -> JobError:
    """Привести исключение к `JobError` с предсказуемым кодом."""
    if isinstance(err, JobError):
        return err
    text = "%s: %s" % (type(err).__name__, err)
    if is_oom(err):
        code = "dml_oom" if "dml" in text.lower() else "cuda_oom"
        return JobError(code, text, hint)
    return JobError(default, text, hint)
