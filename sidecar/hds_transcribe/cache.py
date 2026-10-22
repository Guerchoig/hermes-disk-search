"""Кэш артефактов: `transcript.json` и `diarization.json` (§4.4, §5.2).

Диаризация — **независимый артефакт**: при смене ASR/alignment/глоссария её
не пересчитываем. Ключ включает содержимое-отпечаток файла, модель и параметры
числа спикеров; изменённый ключ = другой артефакт.

Отпечаток файла: размер + mtime + первые/последние 1 МБ (быстро даже на
многогигабайтном видео; полный sha256 длинной записи стоил бы секунд на каждый
запуск и не даёт большей защиты от подмены).
"""
from __future__ import annotations

import hashlib
import os
from typing import Optional

from .fs_compat import ensure_dir, read_json, safe_name, write_json_atomic
from .params import DiarizationParams, TranscribeParams

CHUNK = 1024 * 1024
#: Версия алгоритма диаризации: меняется — кэш недействителен (иначе после
#: правок сшивки блоков переиспользовался бы старый `diarization.json`).
DIARIZATION_VERSION = 8


def file_fingerprint(path: str) -> str:
    """sha256(размер + mtime + первые/последние 1 МБ)."""
    digest = hashlib.sha256()
    stat = os.stat(path)
    digest.update(str(stat.st_size).encode("ascii"))
    digest.update(b"|")
    digest.update(str(stat.st_mtime_ns).encode("ascii"))
    with open(path, "rb") as f:
        digest.update(f.read(CHUNK))
        if stat.st_size > 2 * CHUNK:
            f.seek(-CHUNK, os.SEEK_END)
            digest.update(f.read(CHUNK))
    return digest.hexdigest()


def diarization_key(path: str, params: DiarizationParams, fingerprint: Optional[str] = None) -> str:
    """Ключ кэша: файл + модель + число спикеров + блоки."""
    digest = hashlib.sha256()
    digest.update((fingerprint or file_fingerprint(path)).encode("ascii"))
    for part in (
        params.model,
        str(params.min_speakers),
        str(params.max_speakers),
        str(params.num_speakers),
        str(params.block_minutes),
        str(params.block_overlap_sec),
        "exclusive" if params.use_exclusive else "regular",
        "v%d" % DIARIZATION_VERSION,
    ):
        digest.update(b"|")
        digest.update(part.encode("utf-8"))
    return digest.hexdigest()


def state_root(params: TranscribeParams) -> str:
    """Каталог служебных артефактов: `state_dir` от Rust, иначе `data/auto-transcribe`."""
    root = (params.state_dir or "").strip()
    if not root:
        root = os.environ.get("HDS_TRANSCRIBE_STATE") or os.path.join("data", "auto-transcribe")
    ensure_dir(root)
    return root


def stem_of(path: str) -> str:
    return safe_name(os.path.splitext(os.path.basename(path))[0] or "transcript")


def transcript_path(params: TranscribeParams, stem: str) -> str:
    directory = state_root(params)

    def same_media(path: str) -> bool:
        data = read_json(path) or {}
        media = data.get("media")
        return not media or os.path.abspath(media) == os.path.abspath(params.path)

    base = os.path.join(directory, "%s.transcript.json" % stem)
    if not os.path.exists(base) or same_media(base):
        return base
    digest = file_fingerprint(params.path)[:8]
    return os.path.join(directory, "%s-%s.transcript.json" % (stem, digest))


def diarization_path(params: TranscribeParams, stem: str, key: Optional[str] = None) -> str:
    directory = params.diarization.cache_dir or state_root(params)
    ensure_dir(directory)
    base = os.path.join(directory, "%s.diarization.json" % stem)
    if not os.path.exists(base) or key is None:
        return base
    data = read_json(base) or {}
    if data.get("key") in (None, key):
        return base
    return os.path.join(directory, "%s-%s.diarization.json" % (stem, key[:8]))


def load_json(path: str) -> Optional[dict]:
    return read_json(path)


def save_json(path: str, payload: dict) -> str:
    write_json_atomic(path, payload)
    return path
