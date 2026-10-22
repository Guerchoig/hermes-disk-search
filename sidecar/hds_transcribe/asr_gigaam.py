"""GigaAM-v3 (`e2e_rnnt`) на PyTorch: fp16 CUDA / fp32 CPU + Silero VAD-чанки.

Решения P0 (§0 отчёта, подтверждены владельцем):
* вариант `e2e_rnnt` — основной (пунктуация/ITN из коробки);
* `transcribe_longform` **не используем** (нужен gated `segmentation-3.0` и
  рабочий torchcodec, а он не грузится с FFmpeg 9);
* Silero VAD: окна ≤ 25 с, режем только в паузах;
* `word_timestamps=True` есть только в пакете `gigaam` из GitHub.

CPU-профиль: fp32 на 18 мин дал RSS 15,4 ГБ (§7.8), поэтому батчи небольшие,
тензоры освобождаются после каждого батча, `gc.collect()` на CPU.
"""
from __future__ import annotations

import gc
import os
from typing import Callable, List, Optional, Sequence, Tuple

import numpy as np

from .errors import JobError, is_oom
from .params import AsrParams

SAMPLE_RATE = 16000
DEFAULT_CHUNK_SEC = 25.0

StopFn = Callable[[], None]
ProgressFn = Callable[[str, float, dict], None]


def _noop_progress(name: str, percent: float, fields: dict) -> None:  # pragma: no cover
    return None


def _silero_regions(samples: np.ndarray) -> List[Tuple[float, float]]:
    """Речь по Silero VAD: `[(start, end)]` в секундах."""
    import torch
    from silero_vad import get_speech_timestamps, load_silero_vad

    vad = load_silero_vad(onnx=True)
    ts = get_speech_timestamps(
        torch.from_numpy(samples), vad, sampling_rate=SAMPLE_RATE, return_seconds=True
    )
    return [(float(r["start"]), float(r["end"])) for r in ts]


def _fixed_regions(duration: float, step: float) -> List[Tuple[float, float]]:
    out = []
    start = 0.0
    while start < duration:
        out.append((start, min(start + step, duration)))
        start += step
    return out


def speech_chunks(samples: np.ndarray, max_sec: float = DEFAULT_CHUNK_SEC,
                  vad: str = "silero") -> List[Tuple[float, float]]:
    """Окна ≤ `max_sec`, границы — только в паузах (Silero) либо фиксированные.

    Если VAD не нашёл речи (тишина/шум) — возвращаем пустой список: галлюцинаций
    на тишине не должно быть.
    """
    duration = len(samples) / float(SAMPLE_RATE)
    if duration <= 0:
        return []
    max_sec = min(30.0, max(5.0, float(max_sec)))
    regions: List[Tuple[float, float]] = []
    if vad == "silero":
        try:
            regions = _silero_regions(samples)
        except Exception as e:  # noqa: BLE001 - VAD не должен ронять задание
            raise JobError(
                "onnx_error",
                "Silero VAD не запустился: %s: %s" % (type(e).__name__, e),
                "Проверьте пакет silero-vad и onnxruntime в venv транскрибации.",
            )
    if not regions:
        if vad == "silero":
            return []
        return _fixed_regions(duration, max_sec)

    chunks: List[Tuple[float, float]] = []
    cur_start, cur_end = None, None
    for start, end in regions:
        # Режем ТОЛЬКО в паузах: регион Silero — непрерывная речь; насильно
        # делить его на 25 с нельзя (P0: это меняло текст на длинных фразах).
        # Группируем соседние регионы, пока окно ≤ max_sec; одиночный регион
        # длиннее окна оставляем целиком (речь без паузы).
        if start < 0 or end <= start:
            continue
        if cur_start is None:
            cur_start, cur_end = start, end
        elif end - cur_start <= max_sec:
            cur_end = max(cur_end, end)
        else:
            chunks.append((cur_start, cur_end))
            cur_start, cur_end = start, end
    if cur_start is not None:
        chunks.append((cur_start, cur_end))
    return chunks


def _pad_batch(chunks: Sequence[np.ndarray]):
    """Паддинг батча нулями + длины (как в P0-спайке)."""
    import torch

    lengths = [len(c) for c in chunks]
    maxlen = max(lengths) if lengths else 0
    batch = np.zeros((len(chunks), maxlen), dtype=np.float32)
    for i, chunk in enumerate(chunks):
        batch[i, : len(chunk)] = chunk
    return torch.from_numpy(batch), torch.tensor(lengths, dtype=torch.long)


def load_model(asr: AsrParams, device: Optional[str] = None):
    """Загрузить `gigaam.load_model` (fp16 только на CUDA)."""
    device = device or asr.torch_device()
    if asr.longform not in ("off", "vad"):
        # Защита от «включим longform на глаз»: P1 его не поддерживает.
        raise JobError(
            "model_missing",
            "asr.longform=%s не поддерживается в P1" % asr.longform,
            "Используйте asr.longform: off (Silero VAD-чанки).",
        )
    try:
        import gigaam
    except Exception as e:  # noqa: BLE001
        raise JobError(
            "model_missing",
            "пакет gigaam недоступен: %s: %s" % (type(e).__name__, e),
            "Соберите venv транскрибации (sidecar/hds_transcribe/requirements.lock).",
        )
    from .models import gigaam_cache_dir, resolve_gigaam_source

    cache = gigaam_cache_dir(asr)
    source = resolve_gigaam_source(asr)
    fp16 = bool(asr.fp16 and device == "cuda")
    try:
        return gigaam.load_model(
            source,
            fp16_encoder=fp16,
            device=device,
            download_root=cache,
        )
    except FileNotFoundError as e:
        raise JobError(
            "model_missing",
            "модель %s не найдена (%s)" % (asr.variant, e),
            "Скачайте модель: installers/fetch_transcribe_models.ps1 или HDS_GIGAAM_CACHE.",
        )
    except Exception as e:  # noqa: BLE001
        if is_oom(e):
            raise JobError("cuda_oom", "не хватило VRAM для ASR: %s" % e)
        raise JobError("model_missing", "загрузка %s: %s" % (asr.variant, e))


def _decode_batch(model, batch: Sequence[np.ndarray], word_timestamps: bool = True):
    """Forward+decode одного батча: `[(text, words)]`."""
    import torch

    wav, lens = _pad_batch(batch)
    device = next(model.parameters()).device
    dtype = next(model.parameters()).dtype
    wav = wav.to(device).to(dtype)
    lens = lens.to(device)
    enc, enc_len = model.forward(wav, lens)
    decoded = model._decode(enc, enc_len, lens, word_timestamps)
    del wav, lens, enc, enc_len
    return decoded


def transcribe(samples: np.ndarray, asr: AsrParams, model=None, stop: Optional[StopFn] = None,
               progress: Optional[ProgressFn] = None,
               time_offset: float = 0.0) -> List[dict]:
    """Распознать массив 16 кГц mono → сегменты с нативными `Word`.

    Возвращает `[{start, end, text, words:[{text,start,end}]}]` — слова уже в
    абсолютной шкале времени файла (`time_offset` — смещение канала/фрагмента).
    """
    import torch

    stop = stop or (lambda: None)
    progress = progress or _noop_progress
    model = model or load_model(asr)
    device = next(model.parameters()).device
    if device.type == "cpu":
        torch.set_num_threads(min(8, os.cpu_count() or 8))

    chunks = speech_chunks(samples, max_sec=asr.max_chunk_sec, vad=asr.vad)
    if not chunks:
        progress("asr", 1.0, {"chunks": 0})
        return []

    segments: List[dict] = []
    total = len(chunks)
    for start_idx in range(0, total, asr.batch_size):
        stop()
        batch_ts = chunks[start_idx:start_idx + asr.batch_size]
        batch = [
            np.ascontiguousarray(
                samples[max(0, int(s * SAMPLE_RATE)):int(e * SAMPLE_RATE)]
            )
            for s, e in batch_ts
        ]
        decoded = _decode_batch(model, batch, word_timestamps=True)
        for (chunk_start, chunk_end), (text, words) in zip(batch_ts, decoded):
            text = (text or "").strip()
            word_list = []
            for word in words or []:
                w_text = (getattr(word, "text", "") or "").strip()
                w_start = getattr(word, "start", None)
                w_end = getattr(word, "end", None)
                if not w_text:
                    continue
                word_list.append(
                    {
                        "text": w_text,
                        "start": None if w_start is None else round(float(w_start) + chunk_start + time_offset, 3),
                        "end": None if w_end is None else round(float(w_end) + chunk_start + time_offset, 3),
                    }
                )
            if not text and not word_list:
                continue
            segments.append(
                {
                    "start": round(float(chunk_start) + time_offset, 3),
                    "end": round(float(chunk_end) + time_offset, 3),
                    "text": text,
                    "words": word_list,
                }
            )
        del batch, decoded
        if device.type == "cpu":
            gc.collect()
        progress("asr", (start_idx + len(batch_ts)) / float(total),
                 {"chunks": start_idx + len(batch_ts), "of": total})
    return segments


def unload(model) -> None:
    """Освободить модель и кэш VRAM (по профилю — §5.3 п. 4)."""
    try:
        import torch

        cuda = torch.cuda.is_available()
    except Exception:  # noqa: BLE001
        cuda = False
    del model
    gc.collect()
    if cuda:
        try:
            import torch

            torch.cuda.empty_cache()
        except Exception:  # noqa: BLE001
            pass
