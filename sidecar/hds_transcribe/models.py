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


def _onnx_names(variant: str) -> tuple:
    """Возможные имена экспортированных графов (P0-экспорт добавляет `v3_`).

    `gigaam.to_onnx` пишет файлы по `cfg.model_name` (`v3_e2e_rnnt_encoder.onnx`,
    `v3_e2e_rnnt.yaml`), а `asr.variant` — короткое имя (`e2e_rnnt`). Раньше
    проверялся только `encoder.onnx`, которого экспорт не создаёт: DML/CPU-путь
    после установки не находил модель. Поддерживаем оба варианта и старый
    `onnx/<variant>` из спайка.
    """
    return (
        "%s_encoder.onnx" % variant,
        "v3_%s_encoder.onnx" % variant,
        "encoder.onnx",
        "%s.onnx" % variant,
        "v3_%s.onnx" % variant,
    )


def has_onnx(directory: str, variant: str) -> bool:
    """Есть ли в каталоге граф(ы) для варианта (RNNT — encoder+.., CTC — один)."""
    if not directory:
        return False
    return any(os.path.isfile(os.path.join(directory, name)) for name in _onnx_names(variant))


def resolve_onnx_variant(directory: str, variant: str) -> str:
    """Имя для `onnx_utils.load_onnx`: `e2e_rnnt` или `v3_e2e_rnnt`.

    Загрузчик GigaAM строит пути как `<dir>/<name>.yaml` и `<dir>/<name>_encoder.onnx`,
    поэтому имя обязано совпадать с тем, что реально лежит в каталоге.
    """
    for name in ("v3_%s" % variant, variant):
        if os.path.isfile(os.path.join(directory, "%s.yaml" % name)):
            return name
    return variant


def onnx_dir(asr: AsrParams) -> str:
    """Каталог ONNX-экспорта: `<models>/gigaam-onnx/<variant>` (P5) или старый `onnx/`."""
    default_dir = os.path.join(_models_dir(), "gigaam-onnx", asr.variant)
    candidates = [
        asr.onnx_dir,
        os.environ.get("HDS_GIGAAM_ONNX", ""),
        default_dir,
        os.path.join(_models_dir(), "gigaam-onnx", "%s_fp32" % asr.variant),
        os.path.join(_models_dir(), "onnx", asr.variant),
        os.path.join(_models_dir(), "onnx", "%s_fp32" % asr.variant),
    ]
    for path in candidates:
        if has_onnx(path, asr.variant):
            return path
    return next((p for p in candidates if p), default_dir)


def hf_home() -> str:
    """Кэш Hugging Face: `HF_HOME` → `<models_dir>/hf-home` → `~/.cache/huggingface`.

    Второй вариант — требование §7: на сервере служба не видит пользовательский
    кэш, поэтому модели живут рядом с поставкой (`HDS_MODELS_DIR`).
    """
    home = (os.environ.get("HF_HOME") or "").strip()
    if home:
        return home
    models = (os.environ.get("HDS_MODELS_DIR") or "").strip()
    if models:
        return os.path.join(models, "hf-home")
    return os.path.join(os.path.expanduser("~"), ".cache", "huggingface")


def resolve_gigaam_source(asr: AsrParams) -> str:
    """Источник весов GigaAM: вариант или закреплённый локальный snapshot.

    `gigaam.load_model` принимает либо имя варианта, либо путь к `.ckpt`.
    Поэтому `transcribe.asr.revision` — это **локальный** `.ckpt`/каталог с
    `<variant>.ckpt` (фиксация весов для воспроизводимости, R14). HF-revision
    здесь не применим: выбранный рантайм скачивает веса с CDN Sber, а не из HF.
    """
    revision = (asr.revision or "").strip()
    if not revision:
        return asr.variant
    path = os.path.expanduser(revision)
    if os.path.isfile(path):
        return path
    if os.path.isdir(path):
        for name in ("%s.ckpt" % asr.variant, "v3_%s.ckpt" % asr.variant):
            candidate = os.path.join(path, name)
            if os.path.isfile(candidate):
                return candidate
        raise JobError(
            "model_missing",
            "в %s нет %s.ckpt или v3_%s.ckpt" % (path, asr.variant, asr.variant),
            "Укажите файл .ckpt в transcribe.asr.revision или оставьте ключ пустым.",
        )
    raise JobError(
        "bad_request",
        "transcribe.asr.revision=%r не является локальным .ckpt/каталогом" % revision,
        "Выбранный рантайм GigaAM закрепляется локальным файлом весов: "
        "укажите путь к .ckpt или каталогу со снапшотом.",
    )


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
