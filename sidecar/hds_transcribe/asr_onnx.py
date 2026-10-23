"""GigaAM ONNX через onnxruntime: EP `cuda|dml|cpu` (DML-safe SessionOptions).

Грабли P0 (§10 отчёта, §7.6):
* DirectML требует `execution_mode=ORT_SEQUENTIAL` и
  `enable_mem_pattern=False`; стоковый `gigaam.onnx_utils.load_onnx` второго не
  выставляет, поэтому подменяем `SessionOptions` до вызова загрузчика;
* `intra_op_num_threads=1` на GPU (иначе ORT душит DirectML потоками);
* `device_id` **пинуется**: без него DXGI-адаптер нестабилен, RTF отличался в
  7 раз (0,023 ↔ 0,171);
* ONNX не отдаёт word timestamps — слова интерполируются по сегментам
  (`quality.normalize_words`), флаг `interpolated` честный.
"""
from __future__ import annotations

import gc
from typing import List, Optional

import numpy as np

from .asr_gigaam import ProgressFn, StopFn, SAMPLE_RATE, _noop_progress, speech_chunks
from .errors import JobError, is_oom
from .params import AsrParams


def _patch_session_options(rt, intra_threads: int) -> None:
    """SessionOptions с опциями, обязательными для DirectML."""

    class SidecarSessionOptions(rt.SessionOptions):
        def __init__(self) -> None:
            super().__init__()
            self.execution_mode = rt.ExecutionMode.ORT_SEQUENTIAL
            self.enable_mem_pattern = False
            self.graph_optimization_level = rt.GraphOptimizationLevel.ORT_ENABLE_ALL
            self.log_severity_level = 3
            if intra_threads > 0:
                self.intra_op_num_threads = intra_threads

    rt.SessionOptions = SidecarSessionOptions  # type: ignore[assignment]


def _patch_providers(onnx_utils, provider: str, device_id: Optional[int]) -> None:
    """Закрепить DXGI-адаптер DirectML (`device_id`), не трогая другие EP."""
    if provider != "DmlExecutionProvider" or device_id is None:
        return
    original = onnx_utils._providers_list

    def patched(name):
        if name == "DmlExecutionProvider":
            return [("DmlExecutionProvider", {"device_id": int(device_id)})]
        return original(name)

    onnx_utils._providers_list = patched


def _check_provider(provider: str) -> None:
    try:
        import onnxruntime as rt
    except Exception as e:  # noqa: BLE001
        raise JobError(
            "onnx_error",
            "onnxruntime недоступен: %s: %s" % (type(e).__name__, e),
            "Соберите venv профиля (sidecar/hds_transcribe/requirements-dml.lock).",
        )
    available = list(rt.get_available_providers())
    if provider not in available:
        raise JobError(
            "onnx_error",
            "ExecutionProvider %s недоступен (есть: %s)" % (provider, ", ".join(available)),
            "Выберите asr.provider: cpu или поставьте onnxruntime-gpu/Azure EP.",
        )


def load(asr: AsrParams):
    """Создать ORT-сессии: `(sessions, cfg, provider)`. DML-safe опции."""
    from .models import onnx_dir, resolve_onnx_variant

    provider = asr.ort_provider()
    _check_provider(provider)
    try:
        import onnxruntime as rt
        from gigaam import onnx_utils
    except Exception as e:  # noqa: BLE001
        raise JobError(
            "onnx_error",
            "пакет gigaam/onnxruntime недоступен: %s: %s" % (type(e).__name__, e),
        )
    is_gpu = provider != "CPUExecutionProvider"
    intra = asr.intra_threads or (1 if is_gpu else 6)
    _patch_session_options(rt, intra)
    _patch_providers(onnx_utils, provider, asr.device_id)
    directory = onnx_dir(asr)
    # Экспорт `gigaam.to_onnx` называет файлы по cfg.model_name (`v3_e2e_rnnt`),
    # а `asr.variant` короткий (`e2e_rnnt`): без резолва load_onnx ищет
    # несуществующий `<variant>.yaml` (найдено аудитом P5.0).
    model_name = resolve_onnx_variant(directory, asr.variant)
    try:
        sessions, cfg = onnx_utils.load_onnx(directory, model_name, provider=provider)
    except FileNotFoundError as e:
        raise JobError(
            "model_missing",
            "ONNX-модель %s не найдена в %s (%s)" % (asr.variant, directory, e),
            "Экспортируйте ONNX: installers/export_gigaam_onnx.ps1 "
            "(encoder/decoder/joint + .yaml).",
        )
    except Exception as e:  # noqa: BLE001
        if is_oom(e):
            code = "dml_oom" if provider == "DmlExecutionProvider" else "cuda_oom"
            raise JobError(code, "OOM при создании ORT-сессий: %s" % e)
        raise JobError("onnx_error", "load_onnx(%s): %s" % (provider, e))
    return sessions, cfg, provider


def transcribe(samples: np.ndarray, asr: AsrParams, loaded=None, stop: Optional[StopFn] = None,
               progress: Optional[ProgressFn] = None,
               time_offset: float = 0.0) -> List[dict]:
    """Распознать массив 16 кГц mono через ONNX EP.

    Слова не возвращаются (их нет в ONNX-выходе): сегменты получают `words: []`,
    интерполяция — в `quality.normalize_words`.
    """
    stop = stop or (lambda: None)
    progress = progress or _noop_progress
    sessions, cfg, provider = loaded or load(asr)
    try:
        from gigaam import onnx_utils
    except Exception as e:  # noqa: BLE001
        raise JobError("onnx_error", "gigaam.onnx_utils недоступен: %s" % e)

    chunks_ts = speech_chunks(samples, max_sec=asr.max_chunk_sec, vad=asr.vad)
    if not chunks_ts:
        progress("asr", 1.0, {"chunks": 0, "ep": provider})
        return []

    segments: List[dict] = []
    total = len(chunks_ts)
    for start_idx in range(0, total, asr.batch_size):
        stop()
        batch_ts = chunks_ts[start_idx:start_idx + asr.batch_size]
        batch = [
            np.ascontiguousarray(samples[int(s * SAMPLE_RATE):int(e * SAMPLE_RATE)])
            for s, e in batch_ts
        ]
        try:
            texts = onnx_utils.infer_onnx(
                batch, cfg, sessions, batch_size=len(batch), progress=False
            )
        except Exception as e:  # noqa: BLE001
            if is_oom(e):
                code = "dml_oom" if provider == "DmlExecutionProvider" else "cuda_oom"
                raise JobError(code, "OOM на %s: %s" % (provider, e))
            raise JobError("onnx_error", "infer_onnx(%s): %s" % (provider, e))
        for (chunk_start, chunk_end), text in zip(batch_ts, texts):
            text = (text or "").strip()
            if not text:
                continue
            segments.append(
                {
                    "start": round(float(chunk_start) + time_offset, 3),
                    "end": round(float(chunk_end) + time_offset, 3),
                    "text": text,
                    "words": [],
                }
            )
        del batch, texts
        gc.collect()
        progress("asr", (start_idx + len(batch_ts)) / float(total),
                 {"chunks": start_idx + len(batch_ts), "of": total, "ep": provider})
    return segments


def unload(sessions) -> None:
    """Закрыть ORT-сессии (DML: без вызовов `torch.cuda.*`)."""
    del sessions
    gc.collect()
