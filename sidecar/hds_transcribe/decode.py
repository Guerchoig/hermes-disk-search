"""Декодирование медиа: ffprobe каналов, ffmpeg → float32 16 кГц, `channels`.

Почему ffmpeg CLI, а не torchcodec (§7.1 грабель):
`torchcodec 0.7.0` не грузится с FFmpeg 9 (`libtorchcodec_core7.dll`), поэтому
аудио декодируем один раз в массив, а pyannote получает
`{"waveform": tensor, "sample_rate": 16000}` — без второго прохода и расхождения
шкал.

`decode.channels`:
* `auto` (default) — ffprobe: ≥2 каналов; проверяем корреляцию L/R на первых
  минутах; дубль-стерео (r > 0,9 в 90 % окон) → `downmix`, реально разные
  каналы → `split` (канал = спикер);
* `downmix` — всегда один канал;
* `split` — принудительно поканально.
"""
from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Sequence

import numpy as np

from .errors import JobError
from .params import DecodeParams

#: Сколько секунд начала файла анализировать для auto-решения по каналам.
PROBE_SEC = 600.0
#: Окно корреляции L/R, секунды.
CORR_WINDOW_SEC = 10.0
#: Порог «это дубль-стерео» и доля окон, которые должны его подтвердить.
DUAL_MONO_CORR = 0.9
DUAL_MONO_SHARE = 0.9


@dataclass
class AudioInfo:
    path: str
    channels: int = 1
    sample_rate: int = 0
    duration: float = 0.0
    codec: str = ""
    streams: int = 0
    layout: str = ""

    def as_dict(self) -> dict:
        return {
            "channels": self.channels,
            "sample_rate": self.sample_rate,
            "duration": round(self.duration, 3),
            "codec": self.codec,
            "audio_streams": self.streams,
            "layout": self.layout,
        }


@dataclass
class ChannelPlan:
    mode: str = "downmix"  # downmix | split
    channels: List[int] = field(default_factory=lambda: [0])
    reason: str = ""
    auto: Optional[dict] = None

    def as_dict(self) -> dict:
        out = {"mode": self.mode, "channels": list(self.channels), "reason": self.reason}
        if self.auto:
            out["auto"] = self.auto
        return out


def _binary(configured: str, name: str, hint: str) -> str:
    if configured:
        return configured
    found = shutil.which(name)
    if not found:
        raise JobError("ffmpeg_missing", "%s не найден" % name, hint)
    return found


def ffprobe_bin(params: DecodeParams) -> str:
    return _binary(
        params.ffprobe,
        "ffprobe",
        "Укажите decode.ffprobe в конфиге или установите ffmpeg (installers/setup.ps1).",
    )


def ffmpeg_bin(params: DecodeParams) -> str:
    return _binary(
        params.ffmpeg,
        "ffmpeg",
        "Укажите decode.ffmpeg в конфиге или установите ffmpeg (installers/setup.ps1).",
    )


def _run(cmd: Sequence[str], what: str) -> bytes:
    proc = subprocess.run(cmd, capture_output=True)
    if proc.returncode != 0:
        stderr = proc.stderr.decode("utf-8", "replace").strip()
        raise JobError("bad_audio", "%s: %s" % (what, stderr[-800:] or "ffmpeg error"))
    return proc.stdout


def probe(path: str, params: DecodeParams) -> AudioInfo:
    """ffprobe: число каналов, частота, длительность, кодек."""
    cmd = [
        ffprobe_bin(params),
        "-v",
        "error",
        "-show_entries",
        "stream=index,codec_type,codec_name,channels,channel_layout,sample_rate,duration",
        "-show_entries",
        "format=duration",
        "-of",
        "json",
        path,
    ]
    raw = _run(cmd, "ffprobe %s" % path)
    try:
        data = json.loads(raw.decode("utf-8", "replace"))
    except ValueError as e:
        raise JobError("bad_audio", "ffprobe вернул не JSON: %s" % e)
    audio = [s for s in data.get("streams", []) if s.get("codec_type") == "audio"]
    info = AudioInfo(path=path, streams=len(audio))
    if audio:
        stream = audio[0]
        info.channels = int(stream.get("channels") or 1)
        info.sample_rate = int(stream.get("sample_rate") or 0)
        info.codec = str(stream.get("codec_name") or "")
        info.layout = str(stream.get("channel_layout") or "")
        duration = stream.get("duration")
        if duration:
            info.duration = float(duration)
    if not info.duration:
        try:
            info.duration = float((data.get("format") or {}).get("duration") or 0.0)
        except (TypeError, ValueError):
            info.duration = 0.0
    if info.channels < 1:
        raise JobError("bad_audio", "в файле нет аудиодорожки: %s" % path)
    return info


def decode(path: str, params: DecodeParams, channel: Optional[int] = None,
           seconds: Optional[float] = None) -> np.ndarray:
    """ffmpeg → float32 mono 16 кГц.

    `channel` — взять только этот канал (`pan=mono|c0=cN`), иначе downmix
    (`-ac 1`, среднее каналов — штатное поведение ffmpeg).
    """
    cmd = [ffmpeg_bin(params), "-nostdin", "-threads", "0", "-i", path]
    if channel is not None:
        cmd += ["-af", "pan=mono|c0=c%d" % int(channel)]
    if seconds:
        cmd += ["-t", "%.3f" % float(seconds)]
    cmd += [
        "-f",
        "s16le",
        "-acodec",
        "pcm_s16le",
        "-ar",
        str(int(params.sample_rate)),
        "-ac",
        "1",
        "-",
    ]
    raw = _run(cmd, "ffmpeg %s" % path)
    if not raw:
        raise JobError("bad_audio", "ffmpeg вернул пустой поток: %s" % path)
    return np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0


def window_correlations(left: np.ndarray, right: np.ndarray, sample_rate: int,
                        window_sec: float = CORR_WINDOW_SEC) -> List[float]:
    """Корреляция L/R по окнам (тишина/константа пропускается)."""
    n = min(len(left), len(right))
    step = max(1, int(window_sec * sample_rate))
    corrs: List[float] = []
    for start in range(0, max(0, n - step + 1), step):
        a = left[start:start + step]
        b = right[start:start + step]
        if float(a.std()) < 1e-5 or float(b.std()) < 1e-5:
            continue
        value = float(np.corrcoef(a, b)[0, 1])
        if np.isfinite(value):
            corrs.append(value)
    return corrs


def auto_channel_decision(path: str, info: AudioInfo, params: DecodeParams) -> dict:
    """`auto`: отличаются ли каналы (две дорожки) или это дубль-стерео."""
    left = decode(path, params, channel=0, seconds=PROBE_SEC)
    right = decode(path, params, channel=1, seconds=PROBE_SEC)
    corrs = window_correlations(left, right, int(params.sample_rate))
    median = float(np.median(corrs)) if corrs else 1.0
    share = (sum(1 for c in corrs if c > DUAL_MONO_CORR) / len(corrs)) if corrs else 1.0
    same = bool(np.array_equal(left, right))
    dual_mono = same or (bool(corrs) and median > DUAL_MONO_CORR and share >= DUAL_MONO_SHARE)
    del left, right
    return {
        "windows": len(corrs),
        "corr_median": round(median, 4),
        "share_above_threshold": round(share, 4),
        "identical": same,
        "dual_mono": bool(dual_mono),
    }


def plan_channels(path: str, info: AudioInfo, params: DecodeParams) -> ChannelPlan:
    """Выбрать `downmix`/`split` по режиму и (для auto) корреляции каналов."""
    mode = params.channels
    if mode == "split" and info.channels >= 2:
        return ChannelPlan("split", list(range(info.channels)), "decode.channels=split")
    if mode == "downmix" or info.channels < 2:
        reason = "decode.channels=downmix" if mode == "downmix" else "один канал"
        return ChannelPlan("downmix", [0], reason)
    try:
        auto = auto_channel_decision(path, info, params)
    except JobError as e:
        return ChannelPlan("downmix", [0], "auto: %s" % e.message)
    if auto["dual_mono"]:
        return ChannelPlan("downmix", [0], "auto: дубль-стерео (r>%.1f)" % DUAL_MONO_CORR, auto)
    return ChannelPlan("split", list(range(info.channels)), "auto: каналы различаются", auto)


def check_duration(info: AudioInfo, max_duration_hours: float) -> None:
    """Предел длительности (по умолчанию 6 ч, как `MAX_DIAR_SEC`)."""
    if not info.duration:
        return
    limit = max_duration_hours * 3600.0
    if limit > 0 and info.duration > limit:
        raise JobError(
            "bad_audio",
            "файл %.1f ч длиннее предела %.1f ч" % (info.duration / 3600.0, max_duration_hours),
            "Увеличьте transcribe.max_duration_hours, если запись действительно такая длинная.",
        )
