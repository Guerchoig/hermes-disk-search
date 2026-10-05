#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""OCR (Tesseract) после установки: путь к бинарю + языковые пакеты rus/eng.

Зачем отдельный шаг: hermes-disk-search запускается из ярлыка, LaunchAgent или
MCP-сервера — в таком окружении PATH минимален, поэтому tesseract.exe из
C:\\Program Files\\Tesseract-OCR (Windows) или /opt/homebrew/bin (macOS) не
находится, и в UI появляется «Tesseract OCR не найден», хотя он установлен.
Кроме того, при ocr_lang="rus+eng" нужен языковой пакет rus, которого нет в
базовой поставке UB-Mannheim (в tessdata лежит только eng).

Что делает:
1. ищет tesseract: PATH, затем стандартные каталоги установки;
2. обеспечивает языки rus+eng в каталоге tessdata, доступном БЕЗ прав
   администратора (%LOCALAPPDATA%\\Tesseract-OCR\\tessdata) — HDS сам
   подключает его через TESSDATA_PREFIX; отсутствующий язык скачивается из
   репозитория tessdata (нужен интернет);
3. прописывает index.ocr_tesseract_cmd в config.yaml (создаётся при
   отсутствии), сохраняя остальные настройки и комментарии.

Вызывается установщиками: setup.ps1 (Windows) и
installers/install_macos.command (macOS). Отсутствие Tesseract или интернета —
НЕ ошибка установки: печатаем подсказку и возвращаем 0.

Запуск вручную:
    .venv\\Scripts\\python.exe installers\\configure_ocr.py     # Windows
    .venv/bin/python installers/configure_ocr.py               # macOS
"""
from __future__ import annotations

import argparse
import os
import re
import shutil
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

IS_MAC = sys.platform == "darwin"
IS_WIN = sys.platform == "win32"
LANGS = ("rus", "eng")
TESSDATA_URL = os.environ.get(
    "TESSDATA_BASE_URL",
    "https://raw.githubusercontent.com/tesseract-ocr/tessdata/main")
TIMEOUT = 120


# ==================== Поиск tesseract ====================

def _win_candidates() -> list[Path]:
    pf = os.environ.get("ProgramFiles", r"C:\Program Files")
    pf86 = os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")
    la = os.environ.get("LOCALAPPDATA", "")
    out = [Path(pf) / "Tesseract-OCR" / "tesseract.exe",
           Path(pf86) / "Tesseract-OCR" / "tesseract.exe"]
    if la:
        out += [Path(la) / "Programs" / "Tesseract-OCR" / "tesseract.exe",
                Path(la) / "Tesseract-OCR" / "tesseract.exe"]
    return out


def find_tesseract(explicit: str = "") -> str:
    """Путь к tesseract(.exe): явный > PATH > стандартные каталоги; "" — нет."""
    if explicit:
        p = Path(explicit)
        return str(p) if p.is_file() else ""
    found = shutil.which("tesseract")
    if found:
        return found
    if IS_WIN:
        for cand in _win_candidates():
            if cand.is_file():
                return str(cand)
    elif IS_MAC:
        for cand in (Path("/opt/homebrew/bin/tesseract"),
                     Path("/usr/local/bin/tesseract")):
            if cand.is_file():
                return str(cand)
    else:
        for cand in (Path("/usr/bin/tesseract"),
                     Path("/usr/local/bin/tesseract")):
            if cand.is_file():
                return str(cand)
    return ""


def in_path(tess: str) -> bool:
    """True, если tesseract находится через PATH (путь в config не нужен)."""
    found = shutil.which("tesseract")
    if not found:
        return False
    if not tess:
        return True
    try:
        return Path(found).samefile(Path(tess))
    except OSError:
        return False


def system_tessdata_dirs(tess: str) -> list[Path]:
    """Каталоги tessdata, которые видит системная установка Tesseract."""
    dirs: list[Path] = []
    if tess:
        dirs.append(Path(tess).parent / "tessdata")
    if IS_MAC:
        dirs += [Path("/opt/homebrew/share/tessdata"),
                 Path("/usr/local/share/tessdata")]
    elif not IS_WIN:
        dirs += list(Path("/usr/share").glob("tesseract-ocr*/tessdata"))
        dirs.append(Path("/usr/share/tessdata"))
    return [d for d in dirs if d.is_dir()]


def user_tessdata_dir() -> Path | None:
    """Каталог tessdata без прав администратора (Windows: %LOCALAPPDATA%)."""
    if IS_WIN:
        la = os.environ.get("LOCALAPPDATA", "")
        if la:
            return Path(la) / "Tesseract-OCR" / "tessdata"
    return None

# ==================== Языковые пакеты ====================

def _have_lang(directory: Path, lang: str) -> bool:
    return (directory / f"{lang}.traineddata").is_file()


def _download(lang: str, dest_dir: Path) -> bool:
    """Скачать <lang>.traineddata в dest_dir (True — успех)."""
    url = f"{TESSDATA_URL}/{lang}.traineddata"
    tmp = dest_dir / f"{lang}.traineddata.part"
    try:
        print(f"    [..] скачиваю языковой пакет {lang}: {url}")
        with urllib.request.urlopen(url, timeout=TIMEOUT) as resp, \
                open(tmp, "wb") as out:
            shutil.copyfileobj(resp, out)
        if tmp.stat().st_size < 100_000:            # явно не traineddata
            raise OSError("файл подозрительно мал")
        os.replace(tmp, dest_dir / f"{lang}.traineddata")
        return True
    except Exception as exc:  # noqa: BLE001
        print(f"    [!!] не удалось скачать {lang}: {exc}")
        tmp.unlink(missing_ok=True)
        return False


def ensure_langs(target: Path, sources: list[Path]) -> list[str]:
    """Обеспечить rus+eng в target (копией из sources или скачиванием).

    Возвращает список языков, которые обеспечить не удалось.
    """
    target.mkdir(parents=True, exist_ok=True)
    missing: list[str] = []
    for lang in LANGS:
        if _have_lang(target, lang):
            continue
        src = next((d / f"{lang}.traineddata" for d in sources
                    if _have_lang(d, lang)), None)
        if src is not None:
            print(f"    [..] {lang}: копирую из {src.parent}")
            shutil.copy2(src, target / src.name)
            continue
        if not _download(lang, target):
            missing.append(lang)
    return missing


# ==================== Правка config.yaml ====================

def _patch_config_text(text: str, value: str) -> tuple[str, str]:
    """Вставить/обновить index.ocr_tesseract_cmd, сохранив комментарии.

    Возвращает (новый текст, описание действия).
    """
    quoted = "'" + value.replace("'", "''") + "'"
    rx = re.compile(r"(?m)^([ \t]*)ocr_tesseract_cmd\s*:.*$")
    m = rx.search(text)
    if m:
        # значение сравниваем без учёта хвостового комментария
        cur = re.search(r":[ \t]*([^#]*?)[ \t]*(?:#.*)?$", m.group(0))
        cur_val = (cur.group(1).strip().strip("'\"") if cur else "")
        if cur_val == value:
            return text, "уже задан"
        line = f"{m.group(1)}ocr_tesseract_cmd: {quoted}"
        return text[:m.start()] + line + text[m.end():], "обновлён"
    for anchor in ("ocr_lang", "ocr"):
        rx2 = re.compile(r"(?m)^([ \t]*)" + anchor + r"\s*:.*$")
        m2 = rx2.search(text)
        if m2:
            note = ("   # задано установщиком: tesseract не в PATH"
                    " (запуск из ярлыка/агента)")
            ins = f"\n{m2.group(1)}ocr_tesseract_cmd: {quoted}{note}"
            return text[:m2.end()] + ins + text[m2.end():], "добавлен"
    return text, "пропущено (в config.yaml нет блока index:)"

# ==================== Запись config.yaml ====================

def _replace_file_atomic(tmp: Path, cfg: Path) -> None:
    """os.replace с ретраями: Windows — целевой файл может держать живой процесс."""
    import time

    last: Exception | None = None
    for attempt in range(6):
        try:
            os.replace(tmp, cfg)
            return
        except PermissionError as e:
            last = e
            time.sleep(0.5 * (attempt + 1))
    if last is not None:
        raise last


def write_config(path: Path | None, value: str) -> str:
    """Прописать путь к tesseract в config.yaml (создать при отсутствии).

    Два режима:
    * полный (репозиторий/.venv): `hds.config` — ensure_config + атомарная
      замена с ретраями;
    * фолбэк (релизный архив: пакета `hds` и PyYAML может не быть):
      config.yaml создаётся из config.example.yaml, правка — текстом
      ([`_patch_config_text`]), запись с сохранением BOM и os.replace.
    """
    cfg: Path
    if path:
        cfg = Path(path)
    else:
        try:
            from hds.config import config_path, ensure_config

            cfg = Path(config_path())
            if ensure_config():
                print(f"    [..] создан config.yaml по умолчанию: {cfg}")
        except ImportError:
            cfg = ROOT / "config.yaml"
            if not cfg.exists():
                example = ROOT / "config.example.yaml"
                if not example.exists():
                    return "пропущено (config.yaml не найден)"
                shutil.copyfile(example, cfg)
                print(f"    [..] создан config.yaml из config.example.yaml: {cfg}")

    raw = cfg.read_bytes()
    bom = raw.startswith(b"\xef\xbb\xbf")
    new_text, action = _patch_config_text(raw.decode("utf-8-sig"), value)
    if action.startswith("пропущено"):
        return action
    try:
        import yaml

        yaml.safe_load(new_text)      # не пишем заведомо битый YAML (если есть PyYAML)
    except ImportError:
        pass                          # фолбэк-режим: правка текстом, YAML тривиален
    tmp = cfg.with_suffix(cfg.suffix + ".tmp")
    tmp.write_bytes((b"\xef\xbb\xbf" if bom else b"") + new_text.encode("utf-8"))
    _replace_file_atomic(tmp, cfg)
    return f"{action}: {value}"


# ==================== CLI ====================

def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        prog="configure_ocr",
        description="Путь к Tesseract и языки rus/eng для hermes-disk-search")
    parser.add_argument("--tesseract", default="",
                        help="явный путь к tesseract(.exe)")
    parser.add_argument("--config", default="",
                        help="путь к config.yaml (по умолчанию — проекта)")
    parser.add_argument("--tessdata", default="",
                        help="каталог tessdata назначения")
    parser.add_argument("--no-download", action="store_true",
                        help="не скачивать языковые пакеты")
    args = parser.parse_args(argv)

    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    tess = find_tesseract(args.tesseract)
    if not tess:
        print("    [--] Tesseract не найден — OCR картинок работать не будет.")
        print("    Windows: winget install -e --id UB-Mannheim.TesseractOCR")
        print("    macOS:   brew install tesseract tesseract-lang")
        print("    (затем повторите: python installers/configure_ocr.py)")
        return 0
    print(f"    [ok] Tesseract: {tess}")

    sources = system_tessdata_dirs(tess)
    if args.tessdata:
        target: Path | None = Path(args.tessdata)
    else:
        target = user_tessdata_dir() or (sources[0] if sources else None)
    if target is None:
        print("    [!!] каталог tessdata не найден — языки не проверены")
        missing = list(LANGS)
    elif args.no_download:
        missing = [lang for lang in LANGS if not _have_lang(target, lang)]
    else:
        print(f"    [..] языки {', '.join(LANGS)} -> {target}")
        missing = ensure_langs(target, sources)
    if missing:
        print(f"    [!!] нет языковых пакетов: {', '.join(missing)} — скачайте "
              "*.traineddata с https://github.com/tesseract-ocr/tessdata")
    else:
        print("    [ok] языковые пакеты rus+eng на месте")

    if in_path(tess):
        print("    [ok] tesseract есть в PATH — ocr_tesseract_cmd не нужен")
    else:
        cfg_path = Path(args.config) if args.config else None
        print(f"    [..] config.yaml: {write_config(cfg_path, tess)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
