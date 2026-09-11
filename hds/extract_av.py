"""Аудио/видео: ffprobe-метаданные + транскрипция faster-whisper с таймкодами."""
import os
import shutil
import subprocess
import tempfile

from .config import dig
from .extractors import seg

AUDIO_EXTS = {".mp3", ".wav", ".m4a", ".flac", ".ogg", ".wma", ".aac", ".opus"}
VIDEO_EXTS = {".mp4", ".avi", ".mkv", ".mov", ".wmv", ".flv", ".webm", ".mpg", ".mpeg", ".3gp", ".mts"}

_whisper_model = None


def kind_for_ext_media(ext):
    if ext in AUDIO_EXTS or ext in VIDEO_EXTS:
        return "media"
    return None


def _ffprobe(path):
    ff = shutil.which("ffprobe")
    if not ff:
        return ""
    try:
        return subprocess.run(
            [ff, "-v", "error", "-show_entries", "format=duration:stream=codec_name,width,height",
             "-of", "default=noprint_wrappers=1", path],
            capture_output=True, text=True, timeout=30,
        ).stdout.strip()
    except Exception:  # noqa: BLE001
        return ""


def _get_whisper(cfg):
    global _whisper_model
    if _whisper_model is None:
        from faster_whisper import WhisperModel

        _whisper_model = WhisperModel(
            dig(cfg, "index.whisper_model", "small"),
            device=dig(cfg, "index.whisper_device", "cuda"),
            compute_type=dig(cfg, "index.whisper_compute", "float16"),
        )
    return _whisper_model


def _transcribe(path, cfg, is_video):
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        return None, "ffmpeg не найден в PATH"
    tmp = None
    try:
        if is_video:
            tmp = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
            tmp.close()
            subprocess.run(
                [ffmpeg, "-y", "-i", path, "-vn", "-ac", "1", "-ar", "16000", tmp.name],
                capture_output=True, timeout=3600,
            )
            src = tmp.name
        else:
            src = path
        model = _get_whisper(cfg)
        result, _info = model.transcribe(src, vad_filter=True, language=None)
        segs = []
        for s in result:
            t = (s.text or "").strip()
            if t:
                segs.append(seg(t, t_start=s.start, t_end=s.end))
        return segs, None
    except Exception as e:  # noqa: BLE001
        return None, repr(e)
    finally:
        if tmp and os.path.exists(tmp.name):
            try:
                os.unlink(tmp.name)
            except OSError:
                pass


def extract_media(path, cfg):
    ext = os.path.splitext(path)[1].lower()
    is_video = ext in VIDEO_EXTS
    segs = [seg("Медиафайл: %s\n%s" % (os.path.basename(path), _ffprobe(path) or "метаданные недоступны"))]
    if not dig(cfg, "index.transcribe", True):
        return segs
    try:
        _get_whisper(cfg)
    except Exception as e:  # noqa: BLE001
        segs.append(seg("Транскрипция недоступна (faster-whisper не установлен или CUDA не готов): %s" % e))
        return segs
    tsegs, err = _transcribe(path, cfg, is_video)
    if err:
        segs.append(seg("Транскрипция не удалась: %s" % err))
    elif tsegs:
        segs.extend(tsegs)
    return segs


def extract_dispatch_media(path, cfg, kind):
    return kind, extract_media(path, cfg)