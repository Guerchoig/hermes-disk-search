"""Диаризация pyannote community-1: exclusive + regular, блоки 15–30 мин, кэш.

Факты P0/грабли (§7.2, §7.3, §4.4):
* pyannote 4.0 + torch 2.8: перед `Pipeline.from_pretrained` обязательны
  `torch.serialization.add_safe_globals([...core.task/model...])`, иначе
  `UnpicklingError: Specifications`;
* torchcodec не грузится с FFmpeg 9 ⇒ аудио подаём словарём
  `{"waveform": tensor[1,N], "sample_rate": 16000}`, декодирование — ffmpeg CLI;
* gated-модель: нужны **принятые условия** на HF, а не только токен; при
  `local_files_only` работаем из кэша;
* 77 мин целиком дали пик VRAM 9,0 ГБ ⇒ длинные файлы режем блоками
  15–30 мин; спикеры между блоками сшиваются по перекрытию (overlap) turns.

Сшивка меток между блоками: в зоне перекрытия (по умолчанию 60 с) считаем
суммарное пересечение локальных turns с уже известными; жадно привязываем
локального спикера к каноническому с максимальным пересечением, остальные
получают новый `SPEAKER_NN`. Это детерминированно и не требует эмбеддингов
(они — P2).
"""
from __future__ import annotations

import gc
import os
import time
from typing import Dict, List, Optional, Tuple

import numpy as np

from .errors import JobError, is_oom, is_cpu_fallback
from .params import DiarizationParams, TranscribeParams
from . import cache as cache_mod

SAMPLE_RATE = 16000


def _safe_globals() -> None:
    """Разрешить unpickle классов pyannote (torch≥2.6: weights_only=True)."""
    try:
        import torch
        import pyannote.audio.core.model as _model
        import pyannote.audio.core.task as _task
    except Exception:  # noqa: BLE001 - без pyannote ошибка будет ниже, с понятным кодом
        return
    safe = [
        value
        for module in (_task, _model)
        for value in vars(module).values()
        if isinstance(value, type) and value.__module__.startswith("pyannote")
    ]
    try:
        torch.serialization.add_safe_globals(safe)
    except Exception:  # noqa: BLE001
        pass


def _torch_device(params: DiarizationParams) -> str:
    if params.device in ("cuda", "cpu"):
        return params.device
    try:
        import torch

        return "cuda" if torch.cuda.is_available() else "cpu"
    except Exception:  # noqa: BLE001
        return "cpu"


def load(params: DiarizationParams, device: Optional[str] = None):
    """Загрузить пайплайн на устройство (gated-модель → понятная ошибка)."""
    device = device or _torch_device(params)
    try:
        import torch
        from pyannote.audio import Pipeline
    except Exception as e:  # noqa: BLE001
        raise JobError(
            "model_missing",
            "pyannote.audio недоступен: %s: %s" % (type(e).__name__, e),
            "Соберите venv транскрибации (sidecar/hds_transcribe/requirements.lock).",
        )
    _safe_globals()
    from .models import ensure_diarization_model, hf_token, set_hf_offline

    if params.local_files_only:
        for model in (params.model, "pyannote/segmentation-3.0"):
            ensure_diarization_model(model, True, params.hf_token_env)
        set_hf_offline(True)
    token = hf_token(params.hf_token_env)
    kwargs = {}
    if token:
        kwargs["token"] = token
    try:
        pipeline = Pipeline.from_pretrained(params.model, **kwargs)
    except Exception as e:  # noqa: BLE001
        text = "%s: %s" % (type(e).__name__, e)
        if "403" in text or "gated" in text.lower() or "GatedRepo" in text or "Unauthorized" in text:
            raise JobError(
                "hf_token_missing",
                "нет доступа к gated-модели %s" % params.model,
                "Примите условия модели на huggingface.co и задайте HF_TOKEN; "
                "после загрузки модель работает из кэша офлайн.",
            )
        if is_oom(e):
            raise JobError("cuda_oom", "OOM при загрузке пайплайна: %s" % text)
        raise JobError("diarization_failed", "Pipeline.from_pretrained: %s" % text)
    # Батчи пайплайна ограничивают пик VRAM на длинных блоках (по умолчанию
    # pyannote берёт крупные батчи; на 77 мин это выходило за гейт 10 ГБ).
    try:
        pipeline.to(torch.device(device))
    except Exception as e:  # noqa: BLE001
        if is_oom(e):
            raise JobError("cuda_oom", "OOM при переносе пайплайна на %s: %s" % (device, e))
        raise JobError("diarization_failed", "pipeline.to(%s): %s" % (device, e))
    # Замер P1: батчи ≥ 8 у community-1 (и сегментация, и эмбеддинги) аллоцируют
    # ~9–10 ГБ транзиентно даже на 60 с; 4/4 — ~0,24 ГБ и быстрее. Ставить
    # ТОЛЬКО после `to()`: перенос устройства сбрасывает эти свойства.
    for attr, value in (("segmentation_batch_size", 4), ("embedding_batch_size", 4)):
        try:
            if hasattr(pipeline, attr):
                setattr(pipeline, attr, value)
        except Exception:  # noqa: BLE001
            pass
    return pipeline


def _turns(output) -> Tuple[List[dict], List[dict]]:
    """Выход пайплайна → `(exclusive, regular)` в секундах."""
    exclusive = []
    regular = []
    excl_source = getattr(output, "exclusive_speaker_diarization", None)
    if excl_source is not None:
        for segment, speaker in excl_source:
            exclusive.append({"start": round(float(segment.start), 3),
                              "end": round(float(segment.end), 3),
                              "speaker": str(speaker)})
    reg_source = getattr(output, "speaker_diarization", None)
    if reg_source is not None:
        for segment, speaker in reg_source:
            regular.append({"start": round(float(segment.start), 3),
                            "end": round(float(segment.end), 3),
                            "speaker": str(speaker)})
    if not regular:
        regular = list(exclusive)
    return exclusive, regular


def _run_block(pipeline, samples: np.ndarray, params: DiarizationParams):
    """Один блок аудио → `(exclusive, regular)` в секундах блока."""
    import torch

    waveform = torch.from_numpy(np.ascontiguousarray(samples, dtype=np.float32)).unsqueeze(0)
    output = pipeline(
        {"waveform": waveform, "sample_rate": SAMPLE_RATE},
        **params.pipeline_kwargs(),
    )
    del waveform
    return _turns(output)


def plan_blocks(duration: float, block_sec: float, overlap_sec: float) -> List[Tuple[float, float]]:
    """Разбить файл на блоки с перекрытием (последний — до конца)."""
    if duration <= 0:
        return []
    if duration <= block_sec * 1.1 or block_sec <= 0:
        return [(0.0, duration)]
    overlap = min(max(0.0, overlap_sec), block_sec / 4.0)
    blocks: List[Tuple[float, float]] = []
    start = 0.0
    while start < duration:
        end = min(start + block_sec, duration)
        blocks.append((start, end))
        if end >= duration:
            break
        start = end - overlap
    return blocks


#: Порог глобальной кластеризации голосов блоков (union-find по косинусу).
EMBED_CLUSTER_MIN = 0.60
#: Сколько самых длинных turn'ов спикера идёт в центроид блока.
EMBED_MAX_TURNS = 4
#: Размер батча эмбеддингов (пик VRAM; 48 за раз давали 10,38 ГБ на 77 мин).
EMBED_BATCH = 8
#: Длина фрагмента для эмбеддинга, секунды (центр turn'а).
EMBED_CLIP_SEC = 4.0


def speaker_embeddings(model, samples: np.ndarray, turns, sample_rate: int = SAMPLE_RATE,
                       device=None) -> Dict[str, np.ndarray]:
    """Центроид эмбеддинга голоса на каждого спикера блока.

    Нужен для сшивки меток между блоками, когда спикер не говорил в зоне
    перекрытия (иначе каждый блок вводил бы «нового» спикера: на 77 мин
    получалось 16 вместо 9). Модель — `pipeline._embedding` (pyannote, уже
    загружена пайплайном); ошибки не фатальны — сшивка откатится к overlap.
    """
    if model is None:
        return {}
    try:
        import torch
    except Exception:  # noqa: BLE001
        return {}
    if device is None:
        # `Pipeline.to(cuda)` переносит и модель эмбеддингов: вход должен быть
        # на том же устройстве, иначе torch падает (и мы молча теряли сшивку).
        device = getattr(model, "device", None)
        if device is None:
            try:
                device = next(model.parameters()).device
            except Exception:  # noqa: BLE001
                device = None
    groups: Dict[str, List[np.ndarray]] = {}
    total = len(samples) / float(sample_rate)
    for turn in turns:
        label = turn.get("speaker")
        start = max(0.0, float(turn["start"]))
        end = min(total, float(turn["end"]))
        if not label or end - start < 0.4:
            continue
        if end - start > EMBED_CLIP_SEC:
            mid = (start + end) / 2.0
            start = max(0.0, mid - EMBED_CLIP_SEC / 2.0)
            end = start + EMBED_CLIP_SEC
        groups.setdefault(label, []).append(
            samples[int(start * sample_rate):int(end * sample_rate)]
        )
    # Берём самые длинные turn'ы спикера (до EMBED_MAX_TURNS) и считаем все
    # эмбеддинги одним батчем: короткие/обрезанные turn'ы шумят, а по одному
    # вызову на метку диаризация 77 мин замедлялась вдвое.
    items: List[tuple] = []
    for label, crops in groups.items():
        valid = [c for c in crops if len(c) >= int(0.4 * sample_rate)]
        valid.sort(key=len, reverse=True)
        for crop in valid[:EMBED_MAX_TURNS]:
            items.append((label, crop))
    if not items:
        return {}
    # Батчи по EMBED_BATCH: один большой батч на 4 блока давал пик VRAM выше
    # гейта 10 ГБ (10,38 ГБ на 77 мин), хотя на качество это не влияло.
    vectors_all: List[np.ndarray] = []
    for start in range(0, len(items), EMBED_BATCH):
        subset = items[start:start + EMBED_BATCH]
        maxlen = max(len(crop) for _, crop in subset)
        batch = np.zeros((len(subset), 1, maxlen), dtype=np.float32)
        for idx, (_, crop) in enumerate(subset):
            batch[idx, 0, : len(crop)] = crop
        try:
            tensor = torch.from_numpy(batch)
            if device is not None:
                tensor = tensor.to(device)
            with torch.no_grad():
                emb = model(tensor)
            # `PyannoteAudioPretrainedSpeakerEmbedding` возвращает numpy,
            # другие обёртки — torch.Tensor: поддерживаем оба варианта.
            if hasattr(emb, "detach"):
                emb = emb.detach().cpu().numpy()
            vectors_all.append(
                np.asarray(emb, dtype=np.float32).reshape(len(subset), -1)
            )
            del tensor, emb
        except Exception:  # noqa: BLE001
            if os.environ.get("HDS_TRANSCRIBE_DEBUG"):
                import traceback

                traceback.print_exc()
            # Тихая деградация опасна: без эмбеддингов блоки вводят «новых»
            # спикеров (на 77 мин было 16 вместо 9) — сообщаем в журнал.
            try:
                from hds_sidecar import progress as prog

                prog.warn("диаризация: эмбеддинги недоступны — сшивка только по перекрытию")
            except Exception:  # noqa: BLE001
                pass
            return {}
    arr = np.concatenate(vectors_all, axis=0)
    arr /= np.linalg.norm(arr, axis=1, keepdims=True) + 1e-9
    per_label: Dict[str, List[np.ndarray]] = {}
    for idx, (label, _) in enumerate(items):
        per_label.setdefault(label, []).append(arr[idx])
    out: Dict[str, np.ndarray] = {}
    for label, vectors in per_label.items():
        vec = np.mean(vectors, axis=0)
        out[label] = vec / (np.linalg.norm(vec) + 1e-9)
    return out


def _renumber_shared(exclusive: List[dict], regular: List[dict]) -> List[str]:
    """Единая нумерация для exclusive и regular (regular может содержать overlap)."""
    order: List[str] = []
    for turn in sorted(exclusive + regular, key=lambda t: (t["start"], t["end"])):
        if turn["speaker"] not in order:
            order.append(turn["speaker"])
    rename = {label: "SPEAKER_%02d" % idx for idx, label in enumerate(order)}
    for turn in exclusive + regular:
        turn["speaker"] = rename[turn["speaker"]]
    exclusive.sort(key=lambda t: (t["start"], t["end"]))
    regular.sort(key=lambda t: (t["start"], t["end"]))
    return sorted({t["speaker"] for t in exclusive}) or sorted(rename.values())


def cluster_embeddings(vectors: Dict[tuple, np.ndarray], first_seen: Dict[tuple, float],
                       threshold: float = EMBED_CLUSTER_MIN) -> Dict[tuple, str]:
    """Глобальная кластеризация голосов блоков (union-find по косинусу).

    Локальная кластеризация pyannote в каждом блоке даёт несогласованные метки
    (на 77 мин выходило 16 «спикеров» вместо 9). Здесь центроиды голосов всех
    блоков сливаются, если косинус ≥ `threshold`; нумерация — по первому
    появлению, чтобы результат был стабилен между запусками.
    """
    keys = list(vectors)
    parent = {key: key for key in keys}

    def find(key):
        while parent[key] != key:
            parent[key] = parent[parent[key]]
            key = parent[key]
        return key

    def union(a, b):
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[rb] = ra

    pairs = []
    for i, a in enumerate(keys):
        for b in keys[i + 1:]:
            pairs.append((float(np.dot(vectors[a], vectors[b])), a, b))
    pairs.sort(key=lambda item: -item[0])
    for score, a, b in pairs:
        if score >= threshold:
            union(a, b)

    groups: Dict[tuple, List[tuple]] = {}
    for key in keys:
        groups.setdefault(find(key), []).append(key)
    ordered = sorted(groups.values(), key=lambda g: min(first_seen.get(k, 1e9) for k in g))
    mapping: Dict[tuple, str] = {}
    for idx, group in enumerate(ordered):
        name = "SPEAKER_%02d" % idx
        for key in group:
            mapping[key] = name
    return mapping


def _diarize_impl(samples: np.ndarray, params: DiarizationParams, device: Optional[str],
                  stop, progress) -> dict:
    duration = len(samples) / float(SAMPLE_RATE)
    block_sec = params.block_minutes * 60.0
    blocks = plan_blocks(duration, block_sec, params.block_overlap_sec)
    overlap = 0.0 if len(blocks) <= 1 else min(params.block_overlap_sec, block_sec / 4.0)

    pipeline = None
    collected: List[dict] = []
    vectors: Dict[tuple, np.ndarray] = {}
    first_seen: Dict[tuple, float] = {}
    pipeline = load(params, device=device)
    embedding_model = getattr(pipeline, "_embedding", None)
    t0 = time.perf_counter()
    try:
        debug_mem = bool(os.environ.get("HDS_TRANSCRIBE_DEBUG"))
        for index, (start, end) in enumerate(blocks):
            stop()
            if debug_mem:
                try:
                    import torch

                    if torch.cuda.is_available():
                        torch.cuda.empty_cache()
                        torch.cuda.reset_peak_memory_stats()
                except Exception:  # noqa: BLE001
                    pass
            slice_ = samples[int(start * SAMPLE_RATE):int(end * SAMPLE_RATE)]
            local_excl, local_reg = _run_block(pipeline, slice_, params)
            # Эмбеддинги — по времени БЛОКА (до сдвига в абсолютную шкалу):
            # `slice_` начинается с нуля, иначе для блоков 2+ окна уходят за
            # пределы массива и центроиды терялись.
            local_emb = speaker_embeddings(embedding_model, slice_, local_excl)
            for turn in local_excl + local_reg:
                turn["start"] = round(turn["start"] + start, 3)
                turn["end"] = round(turn["end"] + start, 3)
            for label, vec in local_emb.items():
                vectors[(index, label)] = vec
            keep_from = start + (overlap if index > 0 else 0.0)
            for kind, turns in (("exclusive", local_excl), ("regular", local_reg)):
                for turn in turns:
                    collected.append(
                        {
                            "block": index,
                            "kind": kind,
                            "label": turn["speaker"],
                            "start": turn["start"],
                            "end": turn["end"],
                            "keep_from": keep_from,
                            "block_end": end,
                        }
                    )
                    first_seen.setdefault((index, turn["speaker"]), turn["start"])
            if debug_mem:
                try:
                    import torch

                    if torch.cuda.is_available():
                        progress("mem", 0,
                                 {"block": index, "peak_mb":
                                  round(torch.cuda.max_memory_allocated() / 1024**2, 1)})
                except Exception:  # noqa: BLE001
                    pass
            progress("diar", (index + 1) / float(len(blocks)),
                     {"block": index + 1, "of": len(blocks), "device": device or params.device})
            del slice_, local_excl, local_reg
    finally:
        pipeline = None
        gc.collect()
        try:
            import torch

            if torch.cuda.is_available():
                torch.cuda.empty_cache()
        except Exception:  # noqa: BLE001
            pass

    # Глобальная сшивка голосов; метки без эмбеддинга получают свои id по времени.
    label_map = cluster_embeddings(vectors, first_seen)
    next_id = max([int(n.split("_")[-1]) for n in label_map.values()] or [-1]) + 1
    for key in sorted(first_seen, key=lambda k: first_seen[k]):
        if key not in label_map:
            label_map[key] = "SPEAKER_%02d" % next_id
            next_id += 1

    exclusive_all: List[dict] = []
    regular_all: List[dict] = []
    for item in collected:
        speaker = label_map.get((item["block"], item["label"]), "UNASSIGNED")
        turn = {
            "start": max(item["start"], item["keep_from"]),
            "end": min(item["end"], item["block_end"]),
            "speaker": speaker,
        }
        if turn["end"] - turn["start"] <= 0.05:
            continue
        turn["start"] = round(turn["start"], 3)
        turn["end"] = round(turn["end"], 3)
        (exclusive_all if item["kind"] == "exclusive" else regular_all).append(turn)

    speakers = _renumber_shared(exclusive_all, regular_all)
    elapsed = time.perf_counter() - t0
    debug_embeddings = None
    if os.environ.get("HDS_TRANSCRIBE_DEBUG"):
        debug_embeddings = {
            "%d:%s" % key: [round(float(x), 4) for x in vec]
            for key, vec in vectors.items()
        }
    result = {
        "status": "ok",
        "exclusive": exclusive_all,
        "regular": regular_all,
        "speakers": speakers,
        "audio_sec": round(duration, 2),
        "elapsed_sec": round(elapsed, 2),
        "rtf": round(elapsed / duration, 4) if duration else None,
        "blocks": [{"start": round(s, 3), "end": round(e, 3)} for s, e in blocks],
        "device": device or params.device,
        "model": params.model,
        "block_minutes": params.block_minutes,
        "block_overlap_sec": overlap,
    }
    if debug_embeddings is not None:
        result["embeddings"] = debug_embeddings
    return result


def diarize(samples: np.ndarray, params: DiarizationParams, cache_path: Optional[str] = None,
            key: Optional[str] = None, stop=None, progress=None,
            device: Optional[str] = None) -> dict:
    """Диаризация с кэшем `diarization.json` (независимый артефакт, §4.4).

    Кэш переиспользуется только при совпадении ключа и `status=ok`. Сбой при
    `required: true` поднимает `JobError` (задание падает), при `false` —
    возвращается артефакт со `status=failed` и пустыми turns.
    """
    stop = stop or (lambda: None)
    progress = progress or (lambda *a, **k: None)
    if cache_path and params.cache and key:
        cached = cache_mod.load_json(cache_path)
        if cached and cached.get("key") == key and cached.get("status") == "ok":
            cached["cached"] = True
            progress("diar", 1.0, {"cached": True})
            return cached

    payload: dict = {"key": key, "model": params.model, "params": {
        "min_speakers": params.min_speakers,
        "max_speakers": params.max_speakers,
        "num_speakers": params.num_speakers,
        "block_minutes": params.block_minutes,
    }}
    try:
        result = _diarize_impl(samples, params, device, stop, progress)
        payload.update(result)
    except JobError as e:
        payload.update({"status": "failed", "error": e.to_dict()})
        if cache_path and params.cache:
            cache_mod.save_json(cache_path, payload)
        if params.required:
            raise
        payload.update({"exclusive": [], "regular": [], "speakers": []})
        return payload
    except Exception as e:  # noqa: BLE001
        err = JobError("diarization_failed", "%s: %s" % (type(e).__name__, e))
        payload.update({"status": "failed", "error": err.to_dict()})
        if cache_path and params.cache:
            cache_mod.save_json(cache_path, payload)
        if params.required:
            raise err
        payload.update({"exclusive": [], "regular": [], "speakers": []})
        return payload

    if cache_path and params.cache:
        cache_mod.save_json(cache_path, payload)
        payload["cache_path"] = cache_path
    payload["cached"] = False
    return payload