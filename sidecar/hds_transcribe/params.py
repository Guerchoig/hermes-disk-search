"""Параметры `transcribe`/`postprocess` — датаклассы, приходят **из Rust**.

Воркер транскрибации не читает `config.yaml` (PLAN_PYTHON_SIDECAR_TRANSCRIBE.md
§5.2): `asr.*`, `align.*`, `diarization.*`, абсолютные пути к моделям и `ffmpeg`
передаются в параметрах запроса. Иначе `hds check` и боевой путь видели бы
разные значения.

Разбор терпимый: отсутствующие поля — значения по умолчанию, строки из YAML
(`"8"`, `"false"`) приводятся к нужному типу.
"""
from __future__ import annotations

from dataclasses import dataclass, field, fields
from typing import Any, Dict, List

#: Варианты GigaAM (e2e_rnnt — боевой; остальные — контрольные, P0 §5).
SUPPORTED_VARIANTS = ("e2e_rnnt", "e2e_ctc", "rnnt", "ctc")


def _as_bool(value, default: bool) -> bool:
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return bool(value)
    text = str(value).strip().lower()
    if text in ("1", "true", "yes", "on", "да"):
        return True
    if text in ("0", "false", "no", "off", "нет", ""):
        return False
    return default


def _as_int(value, default: int) -> int:
    try:
        return int(value) if value is not None and value != "" else default
    except (TypeError, ValueError):
        return default


def _as_float(value, default: float) -> float:
    try:
        return float(value) if value is not None and value != "" else default
    except (TypeError, ValueError):
        return default


def _as_str(value, default: str) -> str:
    if value is None:
        return default
    return str(value)


def _as_list(value) -> List[Any]:
    if value is None:
        return []
    return list(value) if isinstance(value, (list, tuple)) else [value]


def _section(src: Dict[str, Any], name: str) -> Dict[str, Any]:
    value = src.get(name)
    return dict(value) if isinstance(value, dict) else {}


def _from_dict(cls, data):
    """Заполнить датакласс из словаря: неизвестные ключи игнорируются."""
    if not isinstance(data, dict):
        data = {}
    known = {f.name for f in fields(cls)}
    return cls(**{k: v for k, v in data.items() if k in known})


@dataclass
class AsrParams:
    """ASR: GigaAM-v3 `e2e_rnnt` (torch fp16 на CUDA) либо ONNX (EP cuda|dml|cpu)."""

    model: str = "ai-sage/GigaAM-v3"
    variant: str = "e2e_rnnt"
    revision: str = ""
    backend: str = "torch"  # torch | onnx
    device: str = "auto"  # auto | cuda | cpu (torch)
    provider: str = "auto"  # auto | cuda | dml | cpu (ORT EP)
    device_id: int = 0  # DirectML: пиновать DXGI-адаптер
    batch_size: int = 8
    fp16: bool = True
    vad: str = "silero"  # silero | off
    longform: str = "off"  # P1: longform запрещён (torchcodec/FFmpeg 9)
    max_chunk_sec: float = 25.0
    cache_dir: str = ""  # HDS_GIGAAM_CACHE / download_root
    onnx_dir: str = ""  # каталог экспорта encoder/decoder/joint
    intra_threads: int = 0  # ONNX CPU: 0 = как в P0 (6)

    @classmethod
    def from_dict(cls, data) -> "AsrParams":
        raw = _from_dict(cls, data)
        variant = _as_str(raw.variant, "e2e_rnnt").strip().lower()
        raw.variant = variant if variant in SUPPORTED_VARIANTS else "e2e_rnnt"
        backend = _as_str(raw.backend, "torch").strip().lower()
        raw.backend = backend if backend in ("torch", "onnx") else "torch"
        raw.device = _as_str(raw.device, "auto").strip().lower() or "auto"
        raw.provider = _as_str(raw.provider, "auto").strip().lower() or "auto"
        raw.device_id = _as_int(raw.device_id, 0)
        raw.batch_size = max(1, _as_int(raw.batch_size, 8))
        raw.fp16 = _as_bool(raw.fp16, True)
        raw.vad = _as_str(raw.vad, "silero").strip().lower() or "silero"
        if raw.vad not in ("silero", "off"):
            raw.vad = "silero"
        raw.longform = _as_str(raw.longform, "off").strip().lower() or "off"
        raw.max_chunk_sec = min(30.0, max(5.0, _as_float(raw.max_chunk_sec, 25.0)))
        raw.intra_threads = _as_int(raw.intra_threads, 0)
        return raw

    def torch_device(self) -> str:
        """Фактическое устройство torch: `auto` → cuda, если доступна, иначе cpu."""
        if self.device in ("cuda", "cpu"):
            return self.device
        try:
            import torch

            return "cuda" if torch.cuda.is_available() else "cpu"
        except Exception:  # noqa: BLE001
            return "cpu"

    def ort_provider(self) -> str:
        """Имя ORT ExecutionProvider для backend=onnx."""
        if self.provider == "dml":
            return "DmlExecutionProvider"
        if self.provider == "cuda":
            return "CUDAExecutionProvider"
        if self.provider == "cpu":
            return "CPUExecutionProvider"
        try:
            import onnxruntime as rt

            available = set(rt.get_available_providers())
        except Exception:  # noqa: BLE001
            available = set()
        for name in ("CUDAExecutionProvider", "DmlExecutionProvider", "CPUExecutionProvider"):
            if name in available:
                return name
        return "CPUExecutionProvider"


@dataclass
class DecodeParams:
    """Декодирование: каналы и бинари ffmpeg/ffprobe (абсолютные пути от Rust)."""

    channels: str = "auto"  # auto | downmix | split
    ffmpeg: str = ""
    ffprobe: str = ""
    sample_rate: int = 16000

    @classmethod
    def from_dict(cls, data) -> "DecodeParams":
        out = _from_dict(cls, data)
        channels = _as_str(out.channels, "auto").strip().lower()
        out.channels = channels if channels in ("auto", "downmix", "split") else "auto"
        # ASR/VAD/диаризация жёстко работают на 16 кГц; другое значение молча
        # сдвигало бы все таймкоды, поэтому фиксируем частоту здесь.
        out.sample_rate = 16000
        return out


@dataclass
class AlignQuality:
    """Пороги флагов слов (`transcribe.align.quality.*`).

    `max_gap_sec` — разрыв между словами **внутри фразы** (в P2.1 сводка считает
    его по блокам, а не по всему файлу); `min_duration_sec` — минимальная
    длительность слова; `overlap_tolerance_sec` — допуск конфликта таймкодов
    соседних слов (кадр GigaAM 40 мс, решение владельца: 3 кадра = 0,12 с);
    `min_aligned_ratio` и `max_unreliable_ratio` — пороги `alignment_quality.ok`.
    """

    max_gap_sec: float = 1.5
    min_duration_sec: float = 0.02
    overlap_tolerance_sec: float = 0.12
    min_aligned_ratio: float = 0.80
    max_unreliable_ratio: float = 0.02

    @classmethod
    def from_dict(cls, data) -> "AlignQuality":
        out = _from_dict(cls, data)
        out.max_gap_sec = _as_float(out.max_gap_sec, 1.5)
        out.min_duration_sec = _as_float(out.min_duration_sec, 0.02)
        out.overlap_tolerance_sec = _as_float(out.overlap_tolerance_sec, 0.12)
        out.min_aligned_ratio = _as_float(out.min_aligned_ratio, 0.80)
        out.max_unreliable_ratio = _as_float(out.max_unreliable_ratio, 0.02)
        return out


@dataclass
class AlignParams:
    mode: str = "native"  # native | whisperx | hybrid (P1: только native)
    model: str = "jonatasgrosman/wav2vec2-large-xlsr-53-russian"
    device: str = "auto"
    max_shift_sec: float = 0.35
    quality: AlignQuality = field(default_factory=AlignQuality)

    @classmethod
    def from_dict(cls, data) -> "AlignParams":
        data = data if isinstance(data, dict) else {}
        return AlignParams(
            mode=_as_str(data.get("mode"), "native").strip().lower() or "native",
            model=_as_str(data.get("model"), "jonatasgrosman/wav2vec2-large-xlsr-53-russian"),
            device=_as_str(data.get("device"), "auto"),
            max_shift_sec=_as_float(data.get("max_shift_sec"), 0.35),
            quality=AlignQuality.from_dict(_section(data, "quality")),
        )


@dataclass
class DiarizationParams:
    """pyannote community-1: exclusive (для merge) + regular (для overlap)."""

    enabled: bool = True
    required: bool = True
    model: str = "pyannote/speaker-diarization-community-1"
    device: str = "auto"  # auto | cuda | cpu (на AMD auto → cpu)
    num_speakers: int = 0
    min_speakers: int = 1
    max_speakers: int = 20
    use_exclusive: bool = True
    block_minutes: float = 20.0  # длинные файлы — блоками 15–30 мин
    block_overlap_sec: float = 60.0
    cache: bool = True
    cache_dir: str = ""  # state_dir от Rust; пусто — рядом с transcript
    hf_token_env: str = "HF_TOKEN"
    local_files_only: bool = True

    @classmethod
    def from_dict(cls, data) -> "DiarizationParams":
        out = _from_dict(cls, data)
        out.enabled = _as_bool(out.enabled, True)
        out.required = _as_bool(out.required, True)
        out.device = _as_str(out.device, "auto").strip().lower() or "auto"
        out.num_speakers = max(0, _as_int(out.num_speakers, 0))
        out.min_speakers = max(1, _as_int(out.min_speakers, 1))
        out.max_speakers = max(out.min_speakers, _as_int(out.max_speakers, 20))
        out.use_exclusive = _as_bool(out.use_exclusive, True)
        out.block_minutes = min(30.0, max(15.0, _as_float(out.block_minutes, 20.0)))
        out.block_overlap_sec = max(0.0, _as_float(out.block_overlap_sec, 60.0))
        out.cache = _as_bool(out.cache, True)
        out.local_files_only = _as_bool(out.local_files_only, True)
        return out

    def pipeline_kwargs(self) -> Dict[str, int]:
        """Аргументы вызова пайплайна: `num_speakers` сильнее min/max."""
        if self.num_speakers > 0:
            return {"num_speakers": self.num_speakers}
        return {"min_speakers": self.min_speakers, "max_speakers": self.max_speakers}


@dataclass
class TranscribeParams:
    job_id: str = ""
    path: str = ""
    language: str = "ru"
    align: AlignParams = field(default_factory=AlignParams)
    decode: DecodeParams = field(default_factory=DecodeParams)
    asr: AsrParams = field(default_factory=AsrParams)
    diarization: DiarizationParams = field(default_factory=DiarizationParams)
    state_dir: str = ""
    stop_file: str = ""
    max_duration_hours: float = 6.0
    fallback_cpu: bool = True

    @classmethod
    def from_dict(cls, data) -> "TranscribeParams":
        data = data if isinstance(data, dict) else {}
        diar = DiarizationParams.from_dict(data.get("diarization"))
        cache = _section(data, "cache")
        if "diarization" in cache:
            diar.cache = _as_bool(cache.get("diarization"), diar.cache)
        return TranscribeParams(
            job_id=_as_str(data.get("job_id"), ""),
            path=_as_str(data.get("path"), ""),
            language=_as_str(data.get("language"), "ru").strip(),
            align=AlignParams.from_dict(data.get("align")),
            decode=DecodeParams.from_dict(data.get("decode")),
            asr=AsrParams.from_dict(data.get("asr")),
            diarization=diar,
            state_dir=_as_str(data.get("state_dir"), ""),
            stop_file=_as_str(data.get("stop_file"), ""),
            max_duration_hours=_as_float(data.get("max_duration_hours"), 6.0),
            fallback_cpu=_as_bool(data.get("fallback_cpu"), True),
        )


@dataclass
class PostprocessParams:
    """Постобработка: глоссарий (P3) и hotwords-настройки (лимиты), LLM — P4."""

    job_id: str = ""
    transcript_path: str = ""
    glossary_path: str = ""
    terms: List[Any] = field(default_factory=list)
    #: `transcribe.hotwords.*`: в postprocess только валидация лимитов 64×64 и
    #: диагностика; сам biasing — на стадии ASR (gigastt), а не замена текста.
    hotwords_enabled: bool = False
    hotwords_backend: str = "none"
    hotwords_phrases: List[str] = field(default_factory=list)
    hotwords_max_phrases: int = 64
    hotwords_max_phrase_chars: int = 64
    hotwords_boost: float = 5.0
    hotwords_url: str = ""
    llm_enabled: bool = False
    llm_base_url: str = "http://127.0.0.1:8010/v1"
    llm_model: str = "chat"
    llm_temperature: float = 0.0
    llm_max_tokens: int = 2048
    llm_timeout_sec: float = 300.0
    llm_max_edits_per_1000: float = 10.0

    @classmethod
    def from_dict(cls, data) -> "PostprocessParams":
        data = data if isinstance(data, dict) else {}
        glossary = _section(data, "glossary")
        llm = _section(data, "llm")
        hotwords = _section(data, "hotwords")
        return PostprocessParams(
            job_id=_as_str(data.get("job_id"), ""),
            transcript_path=_as_str(data.get("transcript_path"), ""),
            glossary_path=_as_str(glossary.get("path"), ""),
            terms=_as_list(glossary.get("terms")),
            hotwords_enabled=_as_bool(hotwords.get("enabled"), False),
            hotwords_backend=_as_str(hotwords.get("backend"), "none").strip().lower(),
            hotwords_phrases=[str(x) for x in _as_list(hotwords.get("phrases"))],
            hotwords_max_phrases=_as_int(hotwords.get("max_phrases"), 64),
            hotwords_max_phrase_chars=_as_int(hotwords.get("max_phrase_chars"), 64),
            hotwords_boost=_as_float(hotwords.get("boost"), 5.0),
            hotwords_url=_as_str(hotwords.get("url"), ""),
            llm_enabled=_as_bool(llm.get("enabled"), False),
            llm_base_url=_as_str(llm.get("base_url"), "http://127.0.0.1:8010/v1"),
            llm_model=_as_str(llm.get("model"), "chat"),
            llm_temperature=_as_float(llm.get("temperature"), 0.0),
            llm_max_tokens=_as_int(llm.get("max_tokens"), 2048),
            llm_timeout_sec=_as_float(llm.get("timeout_sec"), 300.0),
            llm_max_edits_per_1000=_as_float(llm.get("max_edits_per_1000"), 10.0),
        )
