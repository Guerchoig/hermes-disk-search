"""Пути моделей, HF-токен и ранняя проверка gated-моделей (§7.3).

Rust — единственный источник истины по конфигу; сюда приходят абсолютные пути
(`asr.cache_dir`, `asr.onnx_dir`, `decode.ffmpeg`) и переменные окружения
поставки (`HDS_MODELS_DIR`, `HDS_GIGAAM_CACHE`, `HF_HOME`, `HF_TOKEN`).
"""
from __future__ import annotations

import os
from typing import Optional

from .errors import JobError
from .fs_compat import ensure_dir
from .params import AsrParams

GIGAAM_VARIANTS = ("e2e_rnnt", "e2e_ctc", "rnnt", "ctc", "ssl")


def _models_dir() -> str:
    """`HDS_MODELS_DIR` → `%ProgramData%\\hermes-disk-search\\models` → `models/`."""
    env = (os.environ.get("HDS_MODELS_DIR") or "").strip()
    if env:
        return env
    if os.name == "nt":
        program_data = os.environ.get("ProgramData")
        if program_data:
            return os.path.join(program_data, "hermes-disk-search", "models")
    return os.path.join(os.getcwd(), "models")


def gigaam_cache_dir(asr: AsrParams) -> str:
    """`download_root` для `gigaam.load_model`."""
    candidates = [
        asr.cache_dir,
        os.environ.get("HDS_GIGAAM_CACHE", ""),
        os.path.join(_models_dir(), "gigaam"),
    ]
    for path in candidates:
        if path and os.path.isdir(path):
            return path
    fallback = next((p for p in candidates if p), candidates[-1])
    return ensure_dir(fallback)


def onnx_dir(asr: AsrParams) -> str:
    """Каталог экспорта ONNX (`encoder/decoder/joint.onnx` + `.yaml`)."""
    candidates = [
        asr.onnx_dir,
        os.environ.get("HDS_GIGAAM_ONNX", ""),
        os.path.join(_models_dir(), "onnx", asr.variant),
        os.path.join(_models_dir(), "onnx", "e2e_rnnt_fp32"),
    ]
    for path in candidates:
        if path and os.path.isfile(os.path.join(path, "encoder.onnx")):
            return path
    return next((p for p in candidates if p), candidates[-1])


def hf_home() -> str:
    """Кэш Hugging Face: `HF_HOME` → `~/.cache/huggingface`."""
    home = (os.environ.get("HF_HOME") or "").strip()
    if home:
        return home
    return os.path.join(os.path.expanduser("~"), ".cache", "huggingface")


def hf_token(env_name: str = "HF_TOKEN") -> Optional[str]:
    """Токен: переменная из конфига → `HF_TOKEN` → `HUGGING_FACE_HUB_TOKEN` → кэш hf."""
    for name in (env_name, "HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"):
        if not name:
            continue
        value = (os.environ.get(name) or "").strip()
        if value:
            return value
    try:
        from huggingface_hub import get_token

        return get_token()
    except Exception:  # noqa: BLE001
        return None


def model_cached(model: str, hf_home_dir: str = "") -> bool:
    """Есть ли модель в HF-кэше (`models--org--name/snapshots/...`)."""
    repo = "models--" + model.replace("/", "--")
    root = os.path.join(hf_home_dir or hf_home(), "hub", repo, "snapshots")
    if not os.path.isdir(root):
        return False
    try:
        return any(os.scandir(root))
    except OSError:
        return False


def set_hf_offline(offline: bool = True) -> None:
    """Запретить сетевые обращения huggingface_hub (рантайм — только кэш).

    `pipeline.from_pretrained` в pyannote 4.0 не принимает `local_files_only`,
    поэтому офлайн-режим включаем через `HF_HUB_OFFLINE` (env читается
    `file_download` через константу модуля — правим и её).
    """
    if not offline:
        return
    os.environ["HF_HUB_OFFLINE"] = "1"
    try:
        import huggingface_hub
        import huggingface_hub.constants as constants

        constants.HF_HUB_OFFLINE = True
        huggingface_hub.HF_HUB_OFFLINE = True
    except Exception:  # noqa: BLE001
        pass


def ensure_diarization_model(model: str, local_files_only: bool, token_env: str = "HF_TOKEN") -> None:
    """Ранняя понятная ошибка вместо 403 в середине задания.

    Gated-моделям нужны **принятые условия** на HF, а не только токен (§7.3):
    без принятия `from_pretrained` отвечает 403. Если модель в кэше — работаем
    без сети (`local_files_only`).
    """
    if model_cached(model):
        return
    if not local_files_only:
        if hf_token(token_env):
            return
        raise JobError(
            "hf_token_missing",
            "модель %s не в кэше и нет HF-токена" % model,
            "Задайте HF_TOKEN и примите условия модели на huggingface.co, либо "
            "скачайте модель установщиком (installers/fetch_transcribe_models.ps1).",
        )
    raise JobError(
        "hf_token_missing",
        "модель %s не найдена в кэше HF (%s)" % (model, hf_home()),
        "Примите условия модели на huggingface.co (нужен вход в HF), задайте HF_TOKEN "
        "и выполните первичную загрузку; после этого работает офлайн.",
    )
