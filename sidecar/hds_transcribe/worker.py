"""Sidecar-воркер транскрибации/диаризации (P1, контракт §5.2).

Методы: `hello`, `probe`, `transcribe`, `postprocess`, `cancel`, `shutdown`.
Процесс отдельный от `hds_extract` (§5.1): 30-минутная транскрибация не должна
блокировать извлечение документов, а CUDA/DML-контекст (~0,5–1 ГБ) не нужен
процессу, который запускается ради каждого PDF.

Параметры приходят из Rust; `config.yaml` здесь не читается. Прогресс — только
в stderr (`[stage] ...`), stdout — исключительно JSON-RPC (изоляция в
`hds_sidecar.stdio`).

Запуск: python worker.py --profile cuda|cpu|dml [--idle-timeout 600]
"""
from __future__ import annotations

import copy
import gc
import importlib.metadata as md
import importlib.util
import os
import shutil
import subprocess
import sys
import threading
import time
import traceback
from dataclasses import replace

_HERE = os.path.dirname(os.path.abspath(__file__))
_SIDE = os.path.dirname(_HERE)
if _SIDE not in sys.path:
    sys.path.insert(0, _SIDE)

from hds_sidecar import progress as prog  # noqa: E402
from hds_sidecar import runtime, stdio  # noqa: E402

from hds_transcribe import align as align_mod  # noqa: E402
from hds_transcribe import asr_gigaam, asr_onnx  # noqa: E402
from hds_transcribe import cache as cache_mod  # noqa: E402
from hds_transcribe import decode, diarize_pyannote, models, quality  # noqa: E402
from hds_transcribe import glossary as glossary_mod  # noqa: E402
from hds_transcribe import llm_patch as llm_mod  # noqa: E402
from hds_transcribe import merge as merge_mod  # noqa: E402
from hds_transcribe.errors import (  # noqa: E402
    JobError,
    as_job_error,
    cancelled,
    is_cpu_fallback,
)
from hds_transcribe.params import PostprocessParams, TranscribeParams  # noqa: E402

SAMPLE_RATE = 16000
#: Версия артефактов `transcript.json` (для инвалидации кэша постобработки).
TRANSCRIPT_VERSION = 1

PROFILE = (stdio.arg("--profile") or os.environ.get("HDS_TRANSCRIBE_PROFILE") or "cuda").lower()
if PROFILE not in ("cuda", "cpu", "dml"):
    PROFILE = "cuda"

_cancel_event = threading.Event()
_active_job = {"id": None}


def _version(package: str) -> str:
    try:
        return md.version(package)
    except Exception:  # noqa: BLE001
        return ""


def _has_module(name: str) -> bool:
    try:
        return importlib.util.find_spec(name) is not None
    except Exception:  # noqa: BLE001
        return False


def _distribution_names() -> set:
    try:
        return {
            d.metadata["Name"].lower()
            for d in md.distributions()
            if d.metadata and d.metadata.get("Name")
        }
    except Exception:  # noqa: BLE001
        return set()


def _onnx_providers() -> list:
    """Доступные EP без тяжёлого импорта: по установленным пакетам."""
    if not _has_module("onnxruntime"):
        return []
    names = _distribution_names()
    providers = ["ep:cpu"]
    if "onnxruntime-directml" in names:
        providers.append("ep:dml")
    elif "onnxruntime-gpu" in names:
        providers.append("ep:cuda")
    return providers


def gigastt_path() -> str:
    """Путь к gigastt CLI (P3): только env `HDS_GIGASTT_CLI` (без путей разработчика)."""
    env = (os.environ.get("HDS_GIGASTT_CLI") or "").strip()
    return env if env and os.path.isfile(env) else ""


def _hotwords_backend(params: dict) -> str:
    """Backend hotwords из параметров `hello` или env (по умолчанию `none`)."""
    section = params.get("hotwords") if isinstance(params, dict) else None
    value = ""
    if isinstance(section, dict):
        value = str(section.get("backend") or "")
    if not value:
        value = os.environ.get("HDS_TRANSCRIBE_HOTWORDS_BACKEND") or ""
    return value.strip().lower() or "none"


def _capabilities(params: dict = None) -> list:
    caps = ["transcribe", "postprocess", "diarize", "align:native", "quality"]
    if align_mod.available():
        caps.append("align:whisperx")
    if _has_module("gigaam") and _has_module("torch"):
        caps.append("asr:torch")
    if _has_module("gigaam") and _has_module("onnxruntime"):
        caps.append("asr:onnx")
    # P3: biasing есть только у gigastt (CLI/HTTP); сообщаем возможность лишь
    # когда backend реально выбран и бинарник на месте. `none` → возможности нет.
    if _hotwords_backend(params or {}) == "gigastt" and gigastt_path():
        caps.append("hotwords:gigastt")
    caps.extend(_onnx_providers())
    return caps


def _versions() -> dict:
    return {
        "gigaam": _version("gigaam"),
        "pyannote": _version("pyannote.audio"),
        "torch": _version("torch"),
        "onnxruntime": _version("onnxruntime-directml") or _version("onnxruntime"),
        "silero": _version("silero-vad"),
        "numpy": _version("numpy"),
    }


def hello(params: dict) -> dict:
    """Рукопожатие: версия протокола, профиль, возможности, версии.

    Тяжёлые импорты (torch/CUDA-контекст) сюда **не** тянем: `probe` — отдельный
    метод для `transcribe-check`.
    """
    return {
        "protocol": stdio.PROTOCOL,
        "python": sys.version.split()[0],
        "pid": os.getpid(),
        "profile": PROFILE,
        "engine": "python",
        "capabilities": _capabilities(params),
        "versions": _versions(),
        "devices": {"profile": PROFILE},
    }


def _gpu_info() -> dict:
    """Вендор/имя GPU без импорта torch (P5.6: профиль должен быть объясним).

    На Windows — Win32_VideoController через PowerShell (диагностика вызывается
    один раз, 1-2 с); на других платформах — пусто.
    """
    if os.name != "nt":
        return {}
    try:
        proc = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "(Get-CimInstance Win32_VideoController).Name"],
            capture_output=True, text=True, timeout=10,
        )
        names = [line.strip() for line in (proc.stdout or "").splitlines() if line.strip()]
    except Exception:  # noqa: BLE001
        return {}
    joined = " | ".join(names)
    vendor = ""
    if "NVIDIA" in joined:
        vendor = "nvidia"
    elif any(word in joined for word in ("AMD", "Radeon")):
        vendor = "amd"
    elif "Intel" in joined:
        vendor = "intel"
    return {"vendor": vendor, "names": names}


def _ffmpeg_version(ffmpeg: str) -> str:
    if not ffmpeg:
        return ""
    try:
        proc = subprocess.run([ffmpeg, "-version"], capture_output=True, text=True, timeout=5)
        first = (proc.stdout or "").splitlines()
        return first[0].strip() if first else ""
    except Exception:  # noqa: BLE001
        return ""


def _models_info(asr) -> dict:
    """Наличие моделей на диске (без загрузки весов)."""
    cache = models.gigaam_cache_dir(asr)
    names = ["v3_%s.ckpt" % asr.variant, "%s.ckpt" % asr.variant]
    if asr.revision:
        try:
            names.insert(0, os.path.basename(models.resolve_gigaam_source(asr)))
        except Exception:  # noqa: BLE001
            pass
    gigaam = next((os.path.join(cache, n) for n in names
                   if os.path.isfile(os.path.join(cache, n))), "")
    out = {
        "gigaam": gigaam,
        "align": models.model_cached("jonatasgrosman/wav2vec2-large-xlsr-53-russian"),
        "diarization": models.model_cached("pyannote/speaker-diarization-community-1"),
        "segmentation": models.model_cached("pyannote/segmentation-3.0"),
        "onnx_dir": models.onnx_dir(asr),
        "onnx": models.has_onnx(models.onnx_dir(asr), asr.variant),
        "onnx_variant": models.resolve_onnx_variant(models.onnx_dir(asr), asr.variant),
    }
    return out


def probe(params: dict) -> dict:
    """Подробная диагностика стека (для `hds transcribe-check`)."""
    out = {"profile": PROFILE, "python": sys.version.split()[0], "versions": _versions()}
    devices = {"profile": PROFILE}
    try:
        import torch

        devices["torch"] = {
            "cuda_available": bool(torch.cuda.is_available()),
            "cuda_version": torch.version.cuda or "",
            "device_name": torch.cuda.get_device_name(0) if torch.cuda.is_available() else "",
        }
    except Exception as e:  # noqa: BLE001
        devices["torch"] = {"error": "%s: %s" % (type(e).__name__, e)}
    try:
        import onnxruntime as rt

        devices["onnxruntime"] = {
            "version": rt.__version__,
            "providers": list(rt.get_available_providers()),
        }
    except Exception as e:  # noqa: BLE001
        devices["onnxruntime"] = {"error": "%s: %s" % (type(e).__name__, e)}
    out["devices"] = devices
    out["ffmpeg"] = shutil.which("ffmpeg") or ""
    out["ffprobe"] = shutil.which("ffprobe") or ""
    out["ffmpeg_version"] = _ffmpeg_version(out["ffmpeg"])
    asr = TranscribeParams().asr
    out["gigaam_cache"] = models.gigaam_cache_dir(asr)
    out["onnx_dir"] = models.onnx_dir(asr)
    out["hf_home"] = models.hf_home()
    out["hf_token"] = bool(models.hf_token())
    try:
        out["diarization_model_cached"] = models.model_cached(
            "pyannote/speaker-diarization-community-1"
        )
    except Exception:  # noqa: BLE001
        out["diarization_model_cached"] = False
    # P5.6: вендор GPU, наличие каждой модели и ONNX-экспорта + явные замечания
    # («профиль dml, а EP DirectML нет» и т.п.) - без них профиль не объясним.
    out["gpu"] = _gpu_info()
    try:
        out["models"] = _models_info(asr)
    except Exception as e:  # noqa: BLE001
        out["models"] = {"error": "%s: %s" % (type(e).__name__, e)}
    warnings = []
    providers = (devices.get("onnxruntime") or {}).get("providers") or []
    if PROFILE == "dml" and "DmlExecutionProvider" not in providers:
        warnings.append("профиль dml, но DmlExecutionProvider недоступен "
                        "(нужен onnxruntime-directml в venv-transcribe-dml)")
    if PROFILE == "cuda":
        torch_info = devices.get("torch") or {}
        if torch_info.get("error") or not torch_info.get("cuda_available"):
            warnings.append("профиль cuda, но torch не видит CUDA "
                            "(проверьте сборку torch cu128 и драйвер)")
    models_info = out.get("models") or {}
    if models_info.get("gigaam") == "" and asr.backend == "torch":
        warnings.append("веса GigaAM не найдены в %s" % out["gigaam_cache"])
    if not out["ffmpeg"]:
        warnings.append("ffmpeg не найден в PATH (декодирование аудио не работает)")
    out["warnings"] = warnings
    return out


def _cuda_peak_mb(reset: bool = False):
    """Пик VRAM (allocated) для отчёта; None — не CUDA/нет torch.

    Сброс делаем в начале задания: после освобождения моделей `max_memory_allocated`
    хранит пик стадии, поэтому цифра честная (P0: 6,76 ГБ на 18 мин).
    """
    try:
        import torch

        if not torch.cuda.is_available():
            return None
        if reset:
            torch.cuda.reset_peak_memory_stats()
            return None
        return round(torch.cuda.max_memory_allocated() / 1024**2, 1)
    except Exception:  # noqa: BLE001
        return None


class _VramSampler(threading.Thread):
    """Пик VRAM фоновым опросом `memory_allocated()`.

    `torch.cuda.max_memory_allocated()` ненадёжен: pyannote сбрасывает статистику
    внутри пайплайна, и пик ASR терялся (в отчёте было 0,7 ГБ вместо 6,8 ГБ).
    Опрос раз в 200 мс даёт честный максимум по всему заданию.
    """

    def __init__(self, interval: float = 0.2) -> None:
        super().__init__(daemon=True, name="hds-vram")
        self.interval = interval
        self.peak = 0
        self._stop_event = threading.Event()
        try:
            import torch

            self._torch = torch if torch.cuda.is_available() else None
        except Exception:  # noqa: BLE001
            self._torch = None

    def run(self) -> None:
        while not self._stop_event.wait(self.interval):
            if self._torch is None:
                return
            try:
                self.peak = max(self.peak, self._torch.cuda.memory_allocated())
            except Exception:  # noqa: BLE001
                return

    def stop(self) -> float:
        self._stop_event.set()
        if self.is_alive():
            self.join(timeout=2)
        return round(self.peak / 1024**2, 1) if self.peak else 0.0


def _cuda_alloc_mb():
    """Текущая живая VRAM (без пиков) — для диагностики утечек между стадиями."""
    try:
        import torch

        if not torch.cuda.is_available():
            return None
        return round(torch.cuda.memory_allocated() / 1024**2, 1)
    except Exception:  # noqa: BLE001
        return None


def _stop_checker(stop_file: str):
    """Проверка отмены: `cancel` или файл `transcribe.stop` (как сейчас)."""

    def check() -> None:
        if _cancel_event.is_set():
            raise cancelled()
        if stop_file and os.path.exists(stop_file):
            raise cancelled()

    return check


def _with_cpu(asr_params):
    """Копия ASR-параметров для повтора на CPU (`fallback_cpu`)."""
    if asr_params.backend == "onnx":
        return replace(asr_params, provider="cpu", device_id=0)
    return replace(asr_params, device="cpu", fp16=False)


def _run_asr(samples, params, stop, warnings):
    """ASR с одной попыткой на CPU после `cuda_oom`/`dml_oom`/`onnx_error`."""

    def run(asr_params):
        if asr_params.backend == "onnx":
            loaded = asr_onnx.load(asr_params)
            try:
                return asr_onnx.transcribe(
                    samples, asr_params, loaded=loaded, stop=stop, progress=prog.stage
                )
            finally:
                asr_onnx.unload(loaded[0])
        model = asr_gigaam.load_model(asr_params)
        try:
            return asr_gigaam.transcribe(
                samples, asr_params, model=model, stop=stop, progress=prog.stage
            )
        finally:
            asr_gigaam.unload(model)

    try:
        return run(params.asr)
    except JobError as e:
        if params.fallback_cpu and is_cpu_fallback(e):
            message = "ASR: %s (%s) → повтор на CPU" % (e.code, e.message)
            warnings.append(message)
            prog.warn(message)
            return run(_with_cpu(params.asr))
        raise


def _run_diarization(samples, params, key, cache_path, stop, warnings):
    """Диаризация с одной попыткой на CPU после CUDA-OOM."""
    diar = params.diarization

    def run(current):
        return diarize_pyannote.diarize(
            samples, current, cache_path=cache_path, key=key, stop=stop, progress=prog.stage
        )

    try:
        return run(diar)
    except JobError as e:
        if params.fallback_cpu and is_cpu_fallback(e) and diar.device != "cpu":
            message = "диаризация: %s (%s) → повтор на CPU" % (e.code, e.message)
            warnings.append(message)
            prog.warn(message)
            return run(replace(diar, device="cpu"))
        raise


def _words_from_segments(segments, align_params):
    """Сегменты ASR → слова с флагами и сквозным `word_id`."""
    words = []
    for segment in segments:
        words.extend(quality.normalize_words(segment, align_params.quality, len(words)))
    return words


def _align_accumulator() -> dict:
    """Накопитель `align_info` (для `decode.channels: split` — по каналам)."""
    return {"info": None, "words": 0, "shifted": 0, "shift_ms": 0.0, "shift_n": 0}


def _align_add(acc: dict, info: dict, words_count: int) -> None:
    acc["info"] = info
    acc["words"] += int(words_count or 0)
    shifted = int(info.get("words_shifted") or 0)
    acc["shifted"] += shifted
    if shifted and info.get("mean_shift_ms") is not None:
        acc["shift_ms"] += float(info["mean_shift_ms"]) * shifted
        acc["shift_n"] += shifted


def _align_info(acc: dict) -> dict:
    """Сводка alignment для `alignment_quality` (P2.2)."""
    info = dict(
        acc["info"]
        or {
            "mode_requested": "native",
            "mode_used": "native",
            "max_shift_sec": 0.0,
            "whisperx_share": 0.0,
            "words_shifted": 0,
            "mean_shift_ms": 0.0,
        }
    )
    if acc["words"]:
        info["whisperx_share"] = round(acc["shifted"] / acc["words"], 4)
    info["words_shifted"] = acc["shifted"]
    if acc["shift_n"]:
        info["mean_shift_ms"] = round(acc["shift_ms"] / acc["shift_n"], 1)
    return info


def _single_block(words) -> list:
    """Диаризация выключена: весь текст одним блоком без заголовков (§4.5 п. 7)."""
    if not words:
        return []
    starts = [w["start"] for w in words if w.get("start") is not None]
    ends = [w["end"] for w in words if w.get("end") is not None]
    return [
        {
            "speaker": None,
            "start": round(min(starts), 3) if starts else None,
            "end": round(max(ends), 3) if ends else None,
            "text": merge_mod.join_words(words),
            "words": words,
            "overlap": False,
        }
    ]


def _speaker_order(blocks) -> list:
    out = []
    for block in blocks:
        speaker = block.get("speaker")
        if speaker and speaker not in out:
            out.append(speaker)
    return out


def _params_snapshot(params) -> dict:
    """Параметры задания для `transcript.json` (без секретов)."""
    diar = params.diarization
    return {
        "language": params.language,
        "asr": {
            "variant": params.asr.variant,
            "backend": params.asr.backend,
            "device": params.asr.device,
            "provider": params.asr.provider,
            "batch_size": params.asr.batch_size,
            "fp16": params.asr.fp16,
            "vad": params.asr.vad,
            "max_chunk_sec": params.asr.max_chunk_sec,
        },
        "align": {
            "mode": params.align.mode,
            "model": params.align.model,
            "max_shift_sec": params.align.max_shift_sec,
            "quality": {
                "max_gap_sec": params.align.quality.max_gap_sec,
                "min_duration_sec": params.align.quality.min_duration_sec,
                "overlap_tolerance_sec": params.align.quality.overlap_tolerance_sec,
                "min_aligned_ratio": params.align.quality.min_aligned_ratio,
                "max_unreliable_ratio": params.align.quality.max_unreliable_ratio,
            },
        },
        "diarization": {
            "enabled": diar.enabled,
            "model": diar.model,
            "device": diar.device,
            "min_speakers": diar.min_speakers,
            "max_speakers": diar.max_speakers,
            "num_speakers": diar.num_speakers,
            "block_minutes": diar.block_minutes,
            "cache": diar.cache,
        },
        "decode": {
            "channels": params.decode.channels,
            "sample_rate": params.decode.sample_rate,
        },
    }


def transcribe(raw: dict) -> dict:
    """Полный конвейер: decode → ASR → (diar) → quality → merge → transcript.json."""
    params = TranscribeParams.from_dict(raw)
    started = time.perf_counter()
    warnings = []
    if not params.path or not os.path.isfile(params.path):
        raise JobError(
            "bad_audio",
            "файл не найден: %s" % params.path,
            "Проверьте путь и права; для сетевых путей — доступность диска.",
        )
    stop = _stop_checker(params.stop_file)
    _cancel_event.clear()
    _active_job["id"] = params.job_id
    sampler = None
    try:
        stop()
        info = decode.probe(params.path, params.decode)
        decode.check_duration(info, params.max_duration_hours)
        if PROFILE == "cuda":
            _cuda_peak_mb(reset=True)
            sampler = _VramSampler()
            sampler.start()
        prog.stage("probe", 0.0, duration=round(info.duration, 1), channels=info.channels,
                   codec=info.codec)

        stem = cache_mod.stem_of(params.path)
        transcript_path = cache_mod.transcript_path(params, stem)
        key = cache_mod.diarization_key(params.path, params.diarization)
        diar_path = cache_mod.diarization_path(params, stem, key)
        plan = decode.plan_channels(params.path, info, params.decode)
        prog.stage("decode", 0.0, mode=plan.mode, channels=len(plan.channels), reason=plan.reason)

        decode_sec = 0.0
        asr_sec = 0.0
        align_sec = 0.0
        diar_sec = 0.0
        blocks = []
        diar_result = None
        align_acc = _align_accumulator()

        if plan.mode == "split":
            per_channel = []
            for channel in plan.channels:
                stop()
                t_decode = time.perf_counter()
                samples = decode.decode(params.path, params.decode, channel=channel)
                decode_sec += time.perf_counter() - t_decode
                t_asr = time.perf_counter()
                segments = _run_asr(samples, params, stop, warnings)
                asr_sec += time.perf_counter() - t_asr
                t_align = time.perf_counter()
                segments, align_warnings, channel_info = align_mod.apply(
                    segments, params.align, audio=samples, language=params.language
                )
                align_sec += time.perf_counter() - t_align
                warnings.extend(align_warnings)
                words = _words_from_segments(segments, params.align)
                _align_add(align_acc, channel_info, len(words))
                speaker = "SPEAKER_%02d" % channel
                for word in words:
                    word["speaker"] = speaker
                    word["speakers"] = [speaker]
                per_channel.extend(merge_mod.build_blocks(words))
                del samples, segments, words
            per_channel.sort(key=lambda b: (b.get("start") is None, b.get("start") or 0.0))
            blocks = per_channel
            diar_result = {
                "status": "split",
                "speakers": ["SPEAKER_%02d" % ch for ch in plan.channels],
                "channels": list(plan.channels),
                "elapsed_sec": 0.0,
                "cached": False,
            }
        else:
            t_decode = time.perf_counter()
            samples = decode.decode(params.path, params.decode)
            decode_sec = time.perf_counter() - t_decode
            t_asr = time.perf_counter()
            segments = _run_asr(samples, params, stop, warnings)
            asr_sec = time.perf_counter() - t_asr
            if os.environ.get("HDS_TRANSCRIBE_DEBUG"):
                prog.stage("mem", 0, {"after": "asr", "peak_mb": _cuda_peak_mb()})
            t_align = time.perf_counter()
            segments, align_warnings, align_info = align_mod.apply(
                segments, params.align, audio=samples, language=params.language
            )
            align_sec += time.perf_counter() - t_align
            warnings.extend(align_warnings)
            words = _words_from_segments(segments, params.align)
            _align_add(align_acc, align_info, len(words))

            if params.diarization.enabled:
                if os.environ.get("HDS_TRANSCRIBE_DEBUG"):
                    prog.stage("mem", 0, {"before": "diar", "alloc_mb": _cuda_alloc_mb()})
                diar_result = _run_diarization(samples, params, key, diar_path, stop, warnings)
                diar_sec = 0.0 if diar_result.get("cached") else float(
                    diar_result.get("elapsed_sec") or 0.0
                )
                if os.environ.get("HDS_TRANSCRIBE_DEBUG"):
                    prog.stage("mem", 0, {"after": "diar", "peak_mb": _cuda_peak_mb()})
                if diar_result.get("status") == "ok":
                    exclusive = diar_result.get("exclusive") or []
                    regular = diar_result.get("regular") or []
                    if params.diarization.use_exclusive:
                        blocks, _stats = merge_mod.merge_transcript(words, exclusive, regular)
                    else:
                        # Явный отказ от exclusive: обычная разметка (с
                        # перекрытиями) как основная.
                        blocks, _stats = merge_mod.merge_transcript(words, regular, [])
                else:
                    blocks = _single_block(words)
                    error = (diar_result.get("error") or {}).get("message", "")
                    warnings.append("диаризация не выполнена (%s): текст без меток" % error)
            else:
                blocks = _single_block(words)
                diar_result = {"status": "disabled", "speakers": [], "cached": False}
            del samples, segments

        blocks.sort(key=lambda b: (b.get("start") is None, b.get("start") or 0.0))
        all_words = []
        for block in blocks:
            for word in block.get("words") or []:
                word["word_id"] = len(all_words)
                all_words.append(word)
        if not all_words:
            raise JobError(
                "bad_audio",
                "речь не распознана (пустой текст): %s" % params.path,
                "Проверьте, что в записи есть речь и выбран верный канал (decode.channels).",
            )

        speakers = _speaker_order(blocks)
        # Разрывы считаем внутри блока-фразы (P2.1): паузы между чанками VAD и
        # смены спикеров — не признак плохого alignment.
        alignment_quality = quality.summarize(
            all_words,
            params.align.quality,
            phrases=[block.get("words") or [] for block in blocks],
        )
        # P2.2: фактический режим alignment и доля слов из whisperx.
        alignment_quality.update(_align_info(align_acc))
        elapsed = time.perf_counter() - started
        stats = {
            "audio_sec": round(info.duration, 2),
            "decode_sec": round(decode_sec, 2),
            "asr_sec": round(asr_sec, 2),
            "align_sec": round(align_sec, 2),
            "diar_sec": round(diar_sec, 2),
            "elapsed_sec": round(elapsed, 2),
            # `rtf` — как в P1 (ASR+диаризация), `rtf_total` — с alignment (P2).
            "rtf": round((asr_sec + diar_sec) / info.duration, 4) if info.duration else None,
            "rtf_total": round((asr_sec + align_sec + diar_sec) / info.duration, 4)
            if info.duration
            else None,
            "words": len(all_words),
            "blocks": len(blocks),
            "profile": PROFILE,
            "vram_peak_mb": None,
        }
        if PROFILE == "cuda":
            peak = sampler.stop() if sampler is not None else 0.0
            stats["vram_peak_mb"] = peak or _cuda_peak_mb()
        diar_summary = {
            "enabled": bool(params.diarization.enabled),
            "status": (diar_result or {}).get("status"),
            "cached": bool((diar_result or {}).get("cached")),
            "speakers": (diar_result or {}).get("speakers") or speakers,
            "model": params.diarization.model,
            "device": (diar_result or {}).get("device") or params.diarization.device,
            "block_minutes": params.diarization.block_minutes,
            "blocks": len((diar_result or {}).get("blocks") or []),
            "elapsed_sec": round(diar_sec, 2),
            "error": (diar_result or {}).get("error"),
        }
        payload = {
            "version": TRANSCRIPT_VERSION,
            "created": time.strftime("%Y-%m-%dT%H:%M:%S"),
            "media": os.path.abspath(params.path),
            "media_info": info.as_dict(),
            "decode": plan.as_dict(),
            "params": _params_snapshot(params),
            "blocks": blocks,
            "speakers": speakers,
            "stats": stats,
            "alignment_quality": alignment_quality,
            "diarization": diar_summary,
            "warnings": warnings,
        }
        cache_mod.save_json(transcript_path, payload)
        # Агрегат качества в журнал задания (`transcription.log`) — без UI:
        # видно, из чего сложились флаги и какая ветка alignment работала.
        prog.stage(
            "done",
            1.0,
            words=len(all_words),
            blocks=len(blocks),
            mode=alignment_quality.get("mode_used"),
            aligned=alignment_quality.get("aligned"),
            low_conf=alignment_quality.get("low_confidence"),
            failed=alignment_quality.get("failed"),
            unreliable=alignment_quality.get("unreliable_ratio"),
            reasons=alignment_quality.get("reasons") or None,
        )
        return {
            "job_id": params.job_id,
            "transcript_path": os.path.abspath(transcript_path),
            "diarization_path": os.path.abspath(diar_path) if params.diarization.enabled else None,
            "blocks": blocks,
            "speakers": speakers,
            "stats": stats,
            "alignment_quality": alignment_quality,
            "diarization": diar_summary,
            "warnings": warnings,
            "elapsed_ms": round(elapsed * 1000.0, 1),
        }
    finally:
        if sampler is not None and sampler.is_alive():
            sampler.stop()
        _active_job["id"] = None
        gc.collect()


def edits_path(transcript_path: str) -> str:
    """`<state_dir>/<stem>.edits.json` рядом с `<stem>.transcript.json` (P3.2)."""
    stem = os.path.basename(transcript_path)
    suffix = ".transcript.json"
    stem = stem[: -len(suffix)] if stem.endswith(suffix) else os.path.splitext(stem)[0]
    return os.path.join(os.path.dirname(os.path.abspath(transcript_path)),
                        "%s.edits.json" % stem)


def _load_glossary(params) -> tuple:
    """Файл `glossary.yaml` + явные `terms` из конфига → `(глоссарий, warnings)`.

    Пустой путь и отсутствующий файл дают **пустой** глоссарий: поведение P2
    сохраняется байт-в-байт (гейт P3). Явный файл с ошибкой YAML — `bad_request`.
    """
    gl = glossary_mod.Glossary.load(params.glossary_path)
    warnings = list(gl.warnings)
    if params.terms:
        extra = glossary_mod.Glossary.from_dict({"terms": params.terms}, source="params")
        known = {glossary_mod.normalize(t.canonical) for t in gl.terms}
        for term in extra.terms:
            if glossary_mod.normalize(term.canonical) in known:
                continue
            known.add(glossary_mod.normalize(term.canonical))
            gl.terms.append(term)
        warnings.extend(extra.warnings)
    return gl, warnings


def _hotwords_info(params, gl) -> dict:
    """Сводка hotwords: фразы, лимиты 64×64, что реально применено.

    `postprocess` **не** делает biasing: это стадия ASR (gigastt). Здесь только
    валидация лимитов до похода в CLI/HTTP и честная диагностика — иначе
    «текстовая замена» выдавалась бы за hotword-эффект (§4.2).
    """
    backend = (params.hotwords_backend or "none").strip().lower()
    phrases = [str(p).strip() for p in params.hotwords_phrases if str(p).strip()]
    if not phrases:
        phrases = gl.hotword_phrases(
            limit=params.hotwords_max_phrases,
            max_chars=params.hotwords_max_phrase_chars,
        )
    used = bool(params.hotwords_enabled) and backend not in ("", "none")
    if used:
        phrases = glossary_mod.validate_hotwords(
            phrases,
            max_phrases=params.hotwords_max_phrases,
            max_phrase_chars=params.hotwords_max_phrase_chars,
        )
    return {
        "enabled": bool(params.hotwords_enabled),
        "backend": backend,
        "phrases": len(phrases),
        "boost": params.hotwords_boost,
        "available": bool(gigastt_path()),
        "applied_in_postprocess": False,
        "note": (
            "biasing выполняется стадией ASR (gigastt), текстовые замены — глоссарием"
            if backend not in ("", "none") else
            "hotwords.backend=none (ворота G1/G2): biasing выключен"
        ),
    }


def _annotate_review(blocks, review_edits) -> None:
    """Разметка `needs_review` в словах блоков (для UI/диагностики, P3).

    Снимаем прежние пометки (иначе после правки глоссария остался бы мусор) и
    ставим `needs_review: true` на слова, попавшие в правку «на проверку».
    Текст слов **не** меняется: это только метка.
    """
    for block in blocks or []:
        for word in block.get("words") or []:
            word.pop("needs_review", None)
    for edit in review_edits:
        block_index = edit.get("block_index")
        span = edit.get("span") or []
        if not isinstance(block_index, int) or len(span) != 2:
            continue
        if not 0 <= block_index < len(blocks or []):
            continue
        words = (blocks[block_index] or {}).get("words") or []
        start, end = span
        if 0 <= start < len(words):
            words[start]["needs_review"] = True


def _spans_from_edits(edits):
    """`(applied_spans, review_spans)` из журнала правок (глоссарий и LLM).

    `applied_spans: {block_index: [(start, end)]}` — с ними LLM-патчам
    пересекаться запрещено. `review_spans: {(block_index, pos)}` — спорные случаи,
    которые тоже показываем модели как кандидаты.
    """
    applied = {}
    review = set()
    for edit in edits or []:
        block_index = edit.get("block_index")
        span = edit.get("span") or []
        if not isinstance(block_index, int) or len(span) != 2:
            continue
        start, end = span
        if not isinstance(start, int) or not isinstance(end, int) or end <= start:
            continue
        if edit.get("needs_review"):
            review.add((block_index, start))
        else:
            applied.setdefault(block_index, []).append((start, end))
    return applied, review


def _run_llm_layer(blocks, params, glossary, glossary_edits, warnings) -> dict:
    """P4.1–P4.4: слой LLM после глоссария (пустой, если `llm.enabled=false`)."""
    applied_spans, review_spans = _spans_from_edits(glossary_edits)
    if not params.llm_enabled:
        return {"edits": [], "stats": {"edits": 0, "applied": 0, "rejected": 0,
                                       "needs_review": 0, "reasons": {}},
                "summary": {"enabled": False, "model": params.llm_model,
                            "host": params.llm_base_url}, "warnings": []}
    # P4.4: перед вызовом llm_host отдать VRAM (модели уже выгружены, но кэш
    # аллокатора torch держит блоки; на DML ORT-сессии закрыты `asr_onnx.unload`).
    prog.stage("postprocess", 0.0, llm=params.llm_model, host=params.llm_base_url)
    result = llm_mod.run(blocks, params, glossary, applied_spans, review_spans)
    for warning in result.get("warnings") or []:
        warnings.append(warning)
        prog.warn(warning)
    summary = result.get("summary") or {}
    prog.stage(
        "postprocess",
        1.0,
        windows=summary.get("windows"),
        applied=summary.get("applied"),
        rejected=summary.get("rejected"),
        latency_ms=summary.get("latency_ms"),
    )
    return result


def postprocess(raw: dict) -> dict:
    """Постобработка (P3.2 + P4): глоссарий → LLM-патчи, `edits.json`, без ASR.

    Читает `transcript.json` (его пишет `transcribe`), меняет **только**
    `words[].text` и пересобирает `block.text` через `merge.join_words`;
    таймкоды, спикеры, `alignment` и `word_id` не трогает. Повторный вызов
    идемпотентен и не запускает ASR — смысл отдельного метода (§5.2).

    Слои: сначала детерминированный глоссарий (`glossary.apply`), затем — при
    `llm.enabled: true` — LLM-патчи по подозрительным фрагментам
    (`llm_patch.run`, инференс в `llm_host`). LLM не может переписать то, что
    уже исправил глоссарий: пересечение по словам → отказ валидатора.

    `transcript.json` **дополняется** разметкой `needs_review` (решение
    владельца 22.10.2026): слова-кандидаты на ручную проверку получают
    `needs_review: true`, а сводка — секцию `postprocess` (`needs_review`,
    статистика, путь к `edits.json`, `llm` при включённой LLM). Текст ASR в
    файле не переписывается: применённые замены живут в `.md`/`edits.json`.
    """
    started = time.perf_counter()
    params = PostprocessParams.from_dict(raw)
    path = params.transcript_path
    if not path or not os.path.isfile(path):
        raise JobError(
            "bad_request",
            "нет transcript.json: %s" % path,
            "Сначала вызовите transcribe; путь берётся из его результата.",
        )
    data = cache_mod.load_json(path)
    if not isinstance(data, dict):
        raise JobError("bad_request", "transcript.json не читается: %s" % path)
    glossary, warnings = _load_glossary(params)
    hotwords = _hotwords_info(params, glossary)
    blocks = data.get("blocks") or []
    # Копия «как от ASR» — в transcript.json текст остаётся исходным, туда пишутся
    # только пометки needs_review (решение владельца 22.10.2026).
    asr_blocks = copy.deepcopy(blocks)
    glossary_edits, edit_stats = glossary.apply(blocks)
    # P4.1–P4.4: второй слой — LLM-патчи по подозрительным фрагментам. Порядок
    # «глоссарий → LLM» подтверждён (§4.6): LLM видит уже исправленный текст и не
    # может переписать то, что решил детерминированный слой.
    llm_result = _run_llm_layer(blocks, params, glossary, glossary_edits, warnings)
    llm_edits = llm_result.get("edits") or []
    llm_summary = llm_result.get("summary") or {"enabled": bool(params.llm_enabled)}
    edits = list(glossary_edits) + list(llm_edits)
    journal = bool(glossary.terms) or bool(params.llm_enabled)
    out_path = None
    if journal:
        out_path = edits_path(path)
        edit_stats = dict(edit_stats)
        edit_stats["sources"] = {
            "glossary": len(glossary_edits),
            "llm": len(llm_edits),
        }
        payload = {
            "version": 1,
            "job_id": params.job_id or data.get("job_id") or "",
            "transcript": os.path.abspath(path),
            "glossary": glossary.stats(),
            "hotwords": hotwords,
            "stats": edit_stats,
            "edits": edits,
        }
        if params.llm_enabled:
            # Сводка LLM: модель/хост, окна, запросы, применено/отклонено, latency.
            payload["llm"] = llm_summary
        cache_mod.save_json(out_path, payload)
        # Решение владельца: needs_review дублируем в transcript.json (для UI).
        # Текст ASR не переписываем — только метки на словах + сводка.
        review_edits = [e for e in edits if e.get("needs_review")]
        _annotate_review(asr_blocks, review_edits)
        data["blocks"] = asr_blocks
        data["postprocess"] = {
            "version": 1,
            "glossary": glossary.stats(),
            "hotwords": hotwords,
            "stats": edit_stats,
            "edits_path": out_path,
            "needs_review": [
                {k: edit.get(k) for k in
                 ("block_index", "word_id", "word_ids", "before", "after", "term",
                  "reason", "source", "start", "end")}
                for edit in review_edits
            ],
        }
        if params.llm_enabled:
            # Аддитивно к P3: сводка LLM в transcript.json (только если LLM
            # включали — при `llm.enabled=false` файл байт-в-байт как в P3).
            data["postprocess"]["llm"] = llm_summary
        cache_mod.save_json(path, data)
        if edit_stats.get("applied") or edit_stats.get("needs_review"):
            warnings.append(
                "постобработка: применено %d (%d LLM), needs_review %d, отклонено LLM %d "
                "(edits: %s)"
                % (edit_stats["applied"] + (llm_result.get("stats") or {}).get("applied", 0),
                   (llm_result.get("stats") or {}).get("applied", 0),
                   edit_stats["needs_review"] + (llm_result.get("stats") or {}).get("needs_review", 0),
                   (llm_result.get("stats") or {}).get("rejected", 0),
                   out_path)
            )
    elif "postprocess" in data:
        # Глоссарий убрали и LLM выключена — снимаем прежнюю разметку (как в P2).
        _annotate_review(blocks, [])
        data.pop("postprocess", None)
        cache_mod.save_json(path, data)
    text = "\n\n".join(b.get("text", "") for b in blocks if b.get("text"))
    return {
        "job_id": params.job_id,
        "text": text,
        "blocks": blocks,
        "edits": edits,
        "edits_path": out_path,
        "glossary": glossary.stats(),
        "hotwords": hotwords,
        "llm": llm_summary,
        "warnings": warnings,
        "elapsed_ms": round((time.perf_counter() - started) * 1000.0, 1),
    }


#: Числовые коды JSON-RPC для машинных кодов ошибок (Rust читает `kind`).
RPC_BY_KIND = {
    "bad_request": -32602,
    "cancelled": -32010,
    "model_missing": -32011,
    "hf_token_missing": -32012,
    "ffmpeg_missing": -32013,
    "cuda_oom": -32014,
    "dml_oom": -32015,
    "onnx_error": -32016,
    "diarization_failed": -32017,
    "bad_audio": -32018,
    "asr_failed": -32019,
    "align_failed": -32020,
}


def _control(req: dict) -> None:
    """Управляющие запросы в потоке-читателе (отмена во время `transcribe`).

    Главный цикл занят долгой стадией, поэтому `cancel` из очереди дошёл бы
    только после её завершения. Здесь только ставим флаг; ответ отправит
    `_handle`, когда освободится.
    """
    if req.get("method") != "cancel":
        return
    job_id = (req.get("params") or {}).get("job_id")
    if job_id is not None and _active_job.get("id") == job_id:
        _cancel_event.set()


def _handle(req: dict) -> bool:
    mid = req.get("id")
    method = req.get("method")
    params = req.get("params") or {}
    try:
        if method == "hello":
            stdio.ok(mid, hello(params))
            return False
        if method == "probe":
            stdio.ok(mid, probe(params))
            return False
        if method == "transcribe":
            stdio.ok(mid, transcribe(params))
            return False
        if method == "postprocess":
            stdio.ok(mid, postprocess(params))
            return False
        if method == "cancel":
            job_id = params.get("job_id")
            active = _active_job.get("id") == job_id and job_id is not None
            if active:
                _cancel_event.set()
            stdio.ok(mid, {"cancelled": bool(active), "job_id": job_id})
            return False
        if method == "shutdown":
            stdio.ok(mid, {"bye": True})
            return True
        stdio.err(mid, runtime.CODE_UNKNOWN_METHOD, "unknown method: %s" % method)
        return False
    except JobError as e:
        prog.warn("%s: %s" % (e.code, e.message))
        stdio.err(mid, RPC_BY_KIND.get(e.code, -32000), e.message, e.hint, kind=e.code)
        return False
    except BaseException as e:  # noqa: BLE001 - один сбой не должен убивать процесс
        traceback.print_exc(file=sys.stderr)
        err = as_job_error(e)
        stdio.err(mid, RPC_BY_KIND.get(err.code, -32603), err.message, err.hint, kind=err.code)
        return False


def main() -> int:
    stdio.init()
    idle = 600.0
    raw_idle = stdio.arg("--idle-timeout") or os.environ.get("HDS_TRANSCRIBE_IDLE_TIMEOUT")
    if raw_idle:
        try:
            idle = float(raw_idle)
        except ValueError:
            pass
    prog.log("hds_transcribe: profile=%s python=%s" % (PROFILE, sys.version.split()[0]))
    return runtime.serve(_handle, idle, control=_control)


if __name__ == "__main__":
    sys.exit(main())
