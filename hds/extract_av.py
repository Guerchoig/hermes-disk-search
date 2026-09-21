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

# консольные утилиты (ffprobe/ffmpeg/curl) не должны вспыхивать окнами:
# watcher при обработке медиафайлов порождал мигающее окно на каждый файл
_NO_WINDOW = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0

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
                timeout=7200, creationflags=_NO_WINDOW,
            )
            if r.returncode != 0 or not os.path.exists(dest) or os.path.getsize(dest) == 0:
                raise RuntimeError(
                    "Не удалось скачать %s. Проверьте сеть или скачайте вручную: %s -> %s"
                    % (f, url, dest))
        print("[whisper] Модель скачана в %s" % d, flush=True)
    return d


def _add_nvidia_dll_dirs():
    """DLL cuBLAS/cuDNN из pip-пакетов nvidia-*-cu12 должны быть видны ctranslate2.
    Только Windows: на macOS CUDA нет (Apple Silicon = CPU int8 или experimental
    Metal), pip-пакетов nvidia-* нет, а os.add_dll_directory существует только
    на Windows."""
    if os.name != "nt":
        return []
    import glob

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


def _mlx_available():
    """Установлен ли mlx-whisper (Metal-ускорение транскрипции на Apple Silicon)?"""
    if sys.platform != "darwin":
        return False
    try:
        import mlx_whisper  # noqa: F401
        return True
    except Exception:  # noqa: BLE001
        return False


def _pick_device(cfg, platform=None, cpp_available=None, cuda_count=None):
    """Авто-детекция устройства транскрипции.

    faster-whisper/ctranslate2 поддерживают ТОЛЬКО cpu/cuda/auto: ни Vulkan,
    ни Metal/MPS у ctranslate2 нет (OpenNMT/CTranslate2#1562, faster-whisper#515).
    Цепочка авто-детекции:
      Windows/Linux: CUDA (faster-whisper) → Vulkan (whisper.cpp — путь для
                     AMD/Intel; бэкенд ставится `python -m hds.cli vulkan-setup`)
                     → CPU (int8);
      macOS: Metal через mlx-whisper (если установлен), иначе CPU (int8).
    Возвращает (device, compute); device ∈ {"cuda", "cpu", "vulkan", "metal-mlx"}.
    Параметры platform/cpp_available/cuda_count — для тестов (иначе определяются сами).
    """
    plat = platform or sys.platform
    device = str(dig(cfg, "index.whisper_device", "auto") or "auto").strip().lower()
    if cpp_available is None:
        from . import whisper_cpp as _wcpp
        cpp_available = lambda: _wcpp.available(cfg)  # noqa: E731
    elif not callable(cpp_available):
        _cpp_flag = bool(cpp_available)
        cpp_available = lambda: _cpp_flag  # noqa: E731

    def _cuda_n():
        if cuda_count is not None:
            return cuda_count
        try:
            import ctranslate2
            return ctranslate2.get_cuda_device_count()
        except Exception:  # noqa: BLE001
            return None

    # metal — только macOS с установленным mlx-whisper
    if device == "metal" or (device == "auto" and plat == "darwin"):
        if plat == "darwin" and _mlx_available():
            return "metal-mlx", "float16"
        if device == "metal":
            print("[whisper] metal недоступен (нужны macOS и 'pip install mlx-whisper') — авто-детекция",
                  flush=True)
        if device == "metal" and plat != "darwin":
            device = "auto"
        elif device == "auto" and plat == "darwin":
            pass  # ниже уйдёт на CPU

    # vulkan (whisper.cpp) — GPU-ускорение для AMD/Intel на Windows/Linux
    if device in ("vulkan", "amd") and plat != "darwin":
        if cpp_available():
            return "vulkan", "ggml"
        print("[whisper] whisper.cpp (Vulkan) не установлен — авто-детекция. "
              "Установите бэкенд: python -m hds.cli vulkan-setup", flush=True)
        device = "auto"

    if device == "cuda":
        if plat == "darwin":
            print("[whisper] CUDA на macOS недоступен — использую CPU (int8)", flush=True)
            return "cpu", "int8"
        if _cuda_n() == 0:
            print("[whisper] CUDA-устройства не найдены — использую CPU (int8)", flush=True)
            return "cpu", "int8"
        return "cuda", dig(cfg, "index.whisper_compute", "float16")

    if device == "auto" and plat != "darwin":
        n = _cuda_n()
        if n is None:  # ctranslate2 не смог определить — пробовать CUDA бессмысленно
            return "cpu", "int8"
        if n > 0:
            return "cuda", dig(cfg, "index.whisper_compute", "float16")
        if cpp_available():
            return "vulkan", "ggml"
        return "cpu", "int8"

    # cpu (в т.ч. auto на macOS без mlx-whisper)
    compute = dig(cfg, "index.whisper_compute", "float16")
    return "cpu", ("int8" if compute == "float16" else compute)


class _MlxSeg:
    def __init__(self, d):
        self.text = d.get("text", "")
        self.start = float(d.get("start", 0.0))
        self.end = float(d.get("end", 0.0))


class _MlxInfo:
    def __init__(self, res):
        ends = [float(s.get("end", 0.0)) for s in res.get("segments", [])]
        self.duration = max(ends) if ends else 0.0


class _MlxWhisper:
    """Адаптер mlx-whisper (Apple Metal) под интерфейс faster_whisper.WhisperModel,
    который ожидает _transcribe(). Веса — в формате MLX (mlx-community на HF)."""

    is_mlx = True
    batchable = False  # BatchedInferencePipeline работает только с faster-whisper

    def __init__(self, repo):
        self.repo = repo

    def transcribe(self, path, vad_filter=True, language=None):
        import mlx_whisper
        res = mlx_whisper.transcribe(path, path_or_hf_repo=self.repo, language=language)
        return iter(_MlxSeg(s) for s in res.get("segments", [])), _MlxInfo(res)


def _get_mlx_model(cfg):
    """Metal-бэкенд: скачивает MLX-веса в HF-кэш (онлайн или из кэша).
    Возвращает _MlxWhisper или None — тогда вызывающий код уходит на CPU."""
    name = dig(cfg, "index.whisper_model", "small")
    repo = dig(cfg, "index.whisper_mlx_repo", "mlx-community/whisper-%s-mlx" % name)
    try:
        from huggingface_hub import snapshot_download
    except Exception as e:  # noqa: BLE001
        print("[whisper] Metal-бэкенд недоступен (%s) — использую CPU (int8)" % e, flush=True)
        return None
    print("[whisper] Metal (mlx-whisper): модель %s..." % repo, flush=True)
    local = None
    try:
        local = snapshot_download(repo)
    except Exception:  # noqa: BLE001
        try:
            local = snapshot_download(repo, local_files_only=True)
        except Exception as e:  # noqa: BLE001
            print("[whisper] Metal-модель недоступна (%s) — использую CPU (int8)" % e, flush=True)
            return None
    print("[whisper] Metal: модель готова (%s)" % local, flush=True)
    return _MlxWhisper(local)


def _get_whisper(cfg):
    global _whisper_model
    if _whisper_model is None:
        os.environ.setdefault("HF_HUB_OFFLINE", "1")
        os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
        _add_nvidia_dll_dirs()
        token = dig(cfg, "index.hf_token", "")
        if token:
            os.environ.setdefault("HF_TOKEN", token)
        device, compute = _pick_device(cfg)
        if device == "metal-mlx":
            # mlx-whisper скачивает веса сам (HF-кэш) — офлайн-режим ему мешает
            os.environ.pop("HF_HUB_OFFLINE", None)
            _whisper_model = _get_mlx_model(cfg)
            if _whisper_model is not None:
                print("[whisper] Модель готова (Metal / mlx-whisper)", flush=True)
                return _whisper_model
            device, compute = "cpu", "int8"  # mlx не скачался/не установлен
        if device == "vulkan":
            from . import whisper_cpp
            _whisper_model = whisper_cpp.make_adapter(cfg)
            if _whisper_model is not None:
                print("[whisper] Модель готова (Vulkan / whisper.cpp)", flush=True)
                return _whisper_model
            device, compute = "cpu", "int8"  # бэкенд не установлен/битый
        path = ensure_whisper_model(cfg)
        if device == "cpu" and compute == "float16":
            compute = "int8"  # float16 на CPU ctranslate2 не поддерживает
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
            capture_output=True, text=True, timeout=30, creationflags=_NO_WINDOW,
        ).stdout.strip()
    except Exception:  # noqa: BLE001
        return ""


def _transcribe(path, cfg, is_video, progress_cb=None):
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
                capture_output=True, timeout=3600, creationflags=_NO_WINDOW,
            )
            src = tmp.name
        else:
            src = path
        model = _get_whisper(cfg)
        batch = int(dig(cfg, "index.whisper_batch", 8))
        lang = dig(cfg, "index.whisper_language", "") or None
        if batch > 1 and getattr(model, "batchable", True):
            try:
                from faster_whisper import BatchedInferencePipeline

                pipe = BatchedInferencePipeline(model=model)
                result, info = pipe.transcribe(src, language=lang, batch_size=batch)
            except Exception as e:  # noqa: BLE001
                print("[whisper] Батчевый режим недоступен (%s) — обычный" % e,
                      file=sys.stderr, flush=True)
                result, info = model.transcribe(src, vad_filter=True, language=lang)
        else:
            result, info = model.transcribe(src, vad_filter=True, language=lang)
        duration = getattr(info, "duration", 0.0) or 0.0
        segs = []
        for s in result:
            t = (s.text or "").strip()
            if progress_cb and duration > 0:
                try:
                    progress_cb(min(100.0, 100.0 * float(s.end) / duration))
                except Exception:  # noqa: BLE001
                    pass
            if t:
                segs.append(seg(t, t_start=s.start, t_end=s.end))
        if progress_cb:
            progress_cb(100.0)
        return segs, None
    except Exception as e:  # noqa: BLE001
        return None, repr(e)
    finally:
        if tmp and os.path.exists(tmp.name):
            try:
                os.unlink(tmp.name)
            except OSError:
                pass


def extract_media(path, cfg, progress_cb=None):
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
    tsegs, err = _transcribe(path, cfg, is_video, progress_cb=progress_cb)
    if err:
        segs.append(seg("Транскрипция не удалась: %s" % err))
    elif tsegs:
        segs.extend(tsegs)
    return segs


def extract_dispatch_media(path, cfg, kind, progress_cb=None):
    return kind, extract_media(path, cfg, progress_cb=progress_cb)