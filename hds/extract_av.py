"""Аудио/видео: ffprobe-метаданные + транскрипция faster-whisper с таймкодами."""
import os
import shutil
import subprocess
import sys
import tempfile
import threading

from .config import dig
from .extractors import seg

AUDIO_EXTS = {".mp3", ".wav", ".m4a", ".flac", ".ogg", ".wma", ".aac", ".opus"}
VIDEO_EXTS = {".mp4", ".avi", ".mkv", ".mov", ".wmv", ".flv", ".webm", ".mpg", ".mpeg", ".3gp", ".mts"}

_whisper_model = None

# Файлы репозитория Systran/faster-whisper-<size>, нужные для локальной работы
_WHISPER_FILES = ["model.bin", "config.json", "tokenizer.json", "vocabulary.txt"]
_HF_URL = "https://huggingface.co/Systran/faster-whisper-{name}/resolve/main/{fname}"


def _model_dir(cfg):
    from .config import PROJECT_ROOT

    name = dig(cfg, "index.whisper_model", "small")
    d = dig(cfg, "index.whisper_dir", "")
    if not d:
        d = os.path.join(PROJECT_ROOT, "models", "whisper-" + name)
    os.makedirs(d, exist_ok=True)
    return d, name


def ensure_whisper_model(cfg):
    """Гарантирует локальную копию модели Whisper (через curl, без сетевых
    зависимостей huggingface_hub). Возвращает путь к папке модели."""
    d, name = _model_dir(cfg)
    missing = [f for f in _WHISPER_FILES if not os.path.exists(os.path.join(d, f))]
    if missing:
        curl = shutil.which("curl")
        for f in missing:
            url = _HF_URL.format(name=name, fname=f)
            dest = os.path.join(d, f)
            print("[whisper] Загрузка %s (~%s) с HuggingFace через curl..."
                  % (f, "460 МБ" if f == "model.bin" else "небольшой файл"), flush=True)
            r = subprocess.run(
                [curl, "-L", "--fail", "--retry", "3",
                 "--connect-timeout", "15", "--max-time", "7200",
                 "--progress-bar", "-o", dest, url],
                timeout=7200,
            )
            if r.returncode != 0 or not os.path.exists(dest) or os.path.getsize(dest) == 0:
                raise RuntimeError(
                    "Не удалось скачать %s. Проверьте сеть или скачайте вручную: %s -> %s"
                    % (f, url, dest))
        print("[whisper] Модель скачана в %s" % d, flush=True)
    return d


def _add_nvidia_dll_dirs():
    """DLL cuBLAS/cuDNN из pip-пакетов nvidia-*-cu12 должны быть видны ctranslate2."""
    import glob
    import sys

    candidates = [os.path.join(sys.prefix, "Lib", "site-packages"),
                  os.path.join(sys.prefix, "lib", "site-packages")]
    dirs = []
    for sp in candidates:
        dirs.extend(glob.glob(os.path.join(sp, "nvidia", "*", "bin")))
    for d in dirs:
        if os.path.isdir(d):
            try:
                os.add_dll_directory(d)
            except OSError:
                pass
            os.environ["PATH"] = d + os.pathsep + os.environ.get("PATH", "")
    return dirs


def _get_whisper(cfg):
    global _whisper_model
    if _whisper_model is None:
        os.environ.setdefault("HF_HUB_OFFLINE", "1")
        os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
        _add_nvidia_dll_dirs()
        token = dig(cfg, "index.hf_token", "")
        if token:
            os.environ.setdefault("HF_TOKEN", token)
        path = ensure_whisper_model(cfg)
        device = dig(cfg, "index.whisper_device", "cuda")
        compute = dig(cfg, "index.whisper_compute", "float16")
        timeout = int(dig(cfg, "index.whisper_load_timeout", 180))
        print("[whisper] Загрузка модели из %s (%s/%s) ..." % (path, device, compute),
              flush=True)

        result = {}

        def _load():
            try:
                from faster_whisper import WhisperModel

                result["m"] = WhisperModel(path, device=device, compute_type=compute)
            except Exception as e:  # noqa: BLE001
                result["err"] = repr(e)

        th = threading.Thread(target=_load, daemon=True)
        th.start()
        th.join(timeout)
        if "m" in result:
            _whisper_model = result["m"]
            print("[whisper] Модель готова", flush=True)
            return _whisper_model
        if device == "cpu":
            raise RuntimeError(
                "Не удалось загрузить Whisper за %d с (%s). Проверьте установку "
                "faster-whisper." % (timeout, result.get("err", "таймаут загрузки")))
        print("[whisper] Загрузка на %s не завершилась за %d с (%s) — "
              "переключаюсь на CPU (работает медленнее, но надёжно)"
              % (timeout, result.get("err", "таймаут")), flush=True)
        from faster_whisper import WhisperModel

        _whisper_model = WhisperModel(path, device="cpu", compute_type="int8")
        print("[whisper] Модель готова (CPU)", flush=True)
        return _whisper_model
    return _whisper_model


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
        batch = int(dig(cfg, "index.whisper_batch", 8))
        if batch > 1:
            try:
                from faster_whisper import BatchedInferencePipeline

                pipe = BatchedInferencePipeline(model=model)
                result, _info = pipe.transcribe(src, language=None, batch_size=batch)
            except Exception as e:  # noqa: BLE001
                print("[whisper] Батчевый режим недоступен (%s) — обычный" % e,
                      file=sys.stderr, flush=True)
                result, _info = model.transcribe(src, vad_filter=True, language=None)
        else:
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