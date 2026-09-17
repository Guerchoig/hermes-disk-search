"""Бэкенд whisper.cpp (внешний CLI) для транскрипции — путь GPU-ускорения
для AMD/Intel видеокарт на Windows (Vulkan), где faster-whisper/ctranslate2
не работает (ctranslate2 поддерживает только cpu/cuda/auto).

Бэкенд независим от источника бинарника: в папке (index.whisper_cpp_dir,
по умолчанию models/whisper-cpp) ищется whisper-cli.exe / main.exe / whisper.exe
(рекурсивно), рядом — GGML-веса ggml-<имя>.bin. Установкой занимается
`python -m hds.cli vulkan-setup` (вызывается из setup.ps1); можно и вручную
положить любую сборку whisper.cpp в эту папку.

Запуск: whisper-cli -m model.bin -f audio.wav -oj -of <prefix> -np -l auto
JSON-результат: {"transcription": [{"offsets": {"from": ms, "to": ms}, "text": ...}]}
"""
import glob
import json
import os
import shutil
import subprocess
import sys
import tempfile

_GGML_URL = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{name}.bin"
# Источники бинарников (по убыванию доверия): официальные релизы whisper.cpp
# (появление vulkan-ассета — вопрос времени) и AMD-проект lemonade-sdk.
_GH_RELEASES = [
    "https://api.github.com/repos/ggml-org/whisper.cpp/releases?per_page=20",
    "https://api.github.com/repos/lemonade-sdk/whisper.cpp-amd/releases?per_page=10",
]
# Сборка сообщества — используется автоматически, если официальная Vulkan-
# сборка whisper.cpp для Windows ещё не опубликована.
_UNOFFICIAL_VULKAN_URL = ("https://github.com/jerryshell/whisper.cpp-windows-vulkan-bin/"
                          "releases/download/v1.0.0/whisper.cpp-windows-vulkan.zip")
_EXE_NAMES = ("whisper-cli.exe", "main.exe", "whisper.exe")


def _no_window():
    return subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0


def backend_dir(cfg):
    from .config import PROJECT_ROOT, dig

    d = dig(cfg, "index.whisper_cpp_dir", "") or os.path.join(
        PROJECT_ROOT, "models", "whisper-cpp")
    return d


def model_name(cfg):
    from .config import dig
    return dig(cfg, "index.whisper_model", "small") or "small"


def model_path(cfg):
    return os.path.join(backend_dir(cfg), "ggml-%s.bin" % model_name(cfg))


def find_exe(cfg):
    """Рекурсивный поиск CLI whisper.cpp (whisper-cli/main/whisper.exe)."""
    d = backend_dir(cfg)
    if not os.path.isdir(d):
        return None
    for exe in _EXE_NAMES:
        hits = glob.glob(os.path.join(d, "**", exe), recursive=True)
        if hits:
            return hits[0]
    return None


def available(cfg):
    """Бэкенд готов (бинарник + веса на месте)?"""
    exe = find_exe(cfg)
    return bool(exe) and os.path.exists(model_path(cfg))


def gpu_vendor_present():
    """Есть ли GPU с Vulkan-драйвером (AMD/NVIDIA/Intel) на Windows?"""
    if os.name != "nt":
        return False, ""
    try:
        out = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "(Get-CimInstance Win32_VideoController | "
             "Where-Object {$_.Name} | Select-Object -First 3).Name -join '; '"],
            capture_output=True, text=True, timeout=30,
            creationflags=_no_window()).stdout.strip()
    except Exception:  # noqa: BLE001
        return False, ""
    if any(v in out for v in ("AMD", "Radeon", "NVIDIA", "GeForce", "Intel")):
        return True, out
    return False, out


def _gh_assets(url):
    """Список (name, url) ассетов GitHub-релизов; [] при любой ошибке."""
    try:
        import requests as rq
        r = rq.get(url, timeout=30, headers={"Accept": "application/vnd.github+json"})
        if r.status_code != 200:
            return []
        out = []
        for rel in r.json():
            for a in rel.get("assets", []):
                out.append((a.get("name", ""), a.get("browser_download_url", "")))
        return out
    except Exception:  # noqa: BLE001
        return []


def _vulkan_zip_url(assets=None):
    """URL Windows x64 Vulkan-сборки whisper.cpp: официальный релиз, если
    опубликован, иначе — доступная сборка сообщества (fallback автоматический).
    assets — готовый список (name, url) для тестов; иначе запрашивается GitHub."""
    if assets is None:
        assets = []
        for api in _GH_RELEASES:
            assets.extend(_gh_assets(api))
    for name, url in assets:
        n = name.lower()
        if "vulkan" in n and n.endswith(".zip") and ("x64" in n or "win64" in n):
            return url
    return _UNOFFICIAL_VULKAN_URL

def _unblock_tree(d):
    """Windows: снять Zone.Identifier («скачано из интернета») у распакованных
    файлов — аналогично Unblock-File, иначе ОС/политики могут блокировать запуск
    скачанных бинарников (адаптировано из практики transcribeoffline)."""
    if os.name != "nt":
        return
    for root, _dirs, files in os.walk(d):
        for f in files:
            try:
                os.remove(os.path.join(root, f) + ":Zone.Identifier")
            except OSError:
                pass


def download_backend(cfg, log=None):
    """Скачивает и разворачивает бэкенд транскрипции: Vulkan-сборка whisper.cpp
    + GGML-веса модели. Источник — официальный релиз whisper.cpp; если тот ещё
    не публикует Vulkan-сборку, автоматически используется доступная сборка
    сообщества. Возвращает (ok, msg)."""
    log = log or (lambda s: print(s, flush=True))
    if os.name != "nt":
        return False, "Бэкенд whisper.cpp (Vulkan) поддерживается только на Windows"
    d = backend_dir(cfg)
    os.makedirs(d, exist_ok=True)
    if available(cfg):
        return True, "whisper.cpp уже установлен: %s" % find_exe(cfg)

    zip_url = _vulkan_zip_url()
    if not zip_url:  # страховка: fallback-URL константен, ветка недостижима
        return False, ("Не удалось определить источник Vulkan-сборки whisper.cpp. "
                       "Варианты: положите сборку (whisper-cli.exe) в %s, соберите "
                       "сами (cmake -B build -DGGML_VULKAN=ON) или транскрипция "
                       "продолжит работать на CPU." % d)

    curl = shutil.which("curl")
    if not curl:
        return False, "curl не найден — скачайте %s вручную и распакуйте в %s" % (zip_url, d)
    zpath = os.path.join(d, "whisper-cpp.zip")
    ok_dl = False
    for attempt in (1, 2):  # transient-сбои сети — обычное дело для CI/домашних сетей
        log("[vulkan] Скачивание рантайма whisper.cpp (Vulkan)%s..." %
            (" (повтор)" if attempt > 1 else ""))
        r = subprocess.run([curl, "-L", "--fail", "--retry", "3", "--connect-timeout", "15",
                            "--max-time", "3600", "--progress-bar", "-o", zpath, zip_url],
                           timeout=3600, creationflags=_no_window())
        if r.returncode == 0 and os.path.exists(zpath):
            ok_dl = True
            break
    if not ok_dl:
        return False, "Не удалось скачать Vulkan-рантайм (%s). Проверьте сеть и повторите" % zip_url
    import zipfile
    with zipfile.ZipFile(zpath) as z:
        z.extractall(d)
    os.remove(zpath)
    _unblock_tree(d)  # снять MotW, если архив скачан браузером и получил метку
    if not find_exe(cfg):
        return False, "В архиве не найден whisper-cli.exe/main.exe (распаковано в %s)" % d
    return _download_ggml_model(cfg, log)


def _download_ggml_model(cfg, log):
    """GGML-веса Whisper (отдельный формат, не совместим с faster-whisper)."""
    curl = shutil.which("curl")
    dest = model_path(cfg)
    if os.path.exists(dest) and os.path.getsize(dest) > 10 * 1024 * 1024:
        return True, "whisper.cpp готов: %s + %s" % (find_exe(cfg), dest)
    name = model_name(cfg)
    url = _GGML_URL.format(name=name)
    log("[vulkan] Скачивание GGML-модели whisper-%s (%s)..." % (name, url))
    r = subprocess.run([curl, "-L", "--fail", "--retry", "3", "--connect-timeout", "15",
                        "--max-time", "7200", "--progress-bar", "-o", dest, url],
                       timeout=7200, creationflags=_no_window())
    if r.returncode != 0 or not os.path.exists(dest) or os.path.getsize(dest) == 0:
        try:
            os.remove(dest)
        except OSError:
            pass
        return False, "Не удалось скачать %s" % url
    return True, "whisper.cpp готов: %s + %s" % (find_exe(cfg), dest)


class _CppSeg:
    def __init__(self, start, end, text):
        self.start = start
        self.end = end
        self.text = text


class _CppInfo:
    def __init__(self, duration):
        self.duration = duration


def _load_transcription(jpath):
    """Парсинг JSON-результата whisper-cli (-oj): сегменты с секундами."""
    with open(jpath, "r", encoding="utf-8") as f:
        data = json.load(f)
    items = data.get("transcription", [])
    segs = [_CppSeg(i["offsets"]["from"] / 1000.0,
                    i["offsets"]["to"] / 1000.0,
                    i.get("text", "").strip())
            for i in items if i.get("offsets")]
    return segs, _CppInfo(segs[-1].end if segs else 0.0)


class WhisperCppModel:
    """Адаптер whisper.cpp под интерфейс, ожидаемый extract_av._transcribe().
    transcribe() возвращает (итератор сегментов, info.duration) как faster-whisper."""

    batchable = False  # BatchedInferencePipeline работает только с faster-whisper

    def __init__(self, exe, model, cfg=None):
        self.exe = exe
        self.model = model
        self.cfg = cfg or {}

    def transcribe(self, path, vad_filter=True, language=None):
        # whisper.cpp принимает только WAV 16 кГц mono — конвертируем ffmpeg'ом
        ffmpeg = shutil.which("ffmpeg")
        if not ffmpeg:
            raise RuntimeError("ffmpeg не найден в PATH (нужен для whisper.cpp)")
        tmp = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
        tmp.close()
        try:
            subprocess.run(
                [ffmpeg, "-y", "-i", path, "-vn", "-ac", "1", "-ar", "16000", tmp.name],
                capture_output=True, timeout=3600, creationflags=_no_window())
            base = tmp.name[:-4]
            cmd = [self.exe, "-m", self.model, "-f", tmp.name,
                   "-oj", "-of", base, "-np", "-l", (language or "auto")]
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=7200,
                               creationflags=_no_window())
            jpath = base + ".json"
            if r.returncode != 0 or not os.path.exists(jpath):
                err = (r.stderr or r.stdout or "")[-400:]
                raise RuntimeError("whisper.cpp завершился с ошибкой: %s" % err)
            with open(jpath, "r", encoding="utf-8") as f:
                if not os.path.getsize(jpath):
                    raise RuntimeError("whisper.cpp вернул пустой JSON")
            segs, info = _load_transcription(jpath)
            return iter(segs), info
        finally:
            for p in (tmp.name, tmp.name[:-4] + ".json"):
                try:
                    os.remove(p)
                except OSError:
                    pass


def make_adapter(cfg):
    """Готовый адаптер или None (бэкенд не установлен)."""
    exe = find_exe(cfg)
    model = model_path(cfg)
    if not exe or not os.path.exists(model):
        return None
    return WhisperCppModel(exe, model, cfg)
