"""Файловые помощники sidecar: temp, безопасные пути, long-path (Windows)."""
from __future__ import annotations

import contextlib
import json
import os
import shutil
import tempfile


def long_path(path: str) -> str:
    """`\\\\?\\`-форма для Windows, если путь абсолютный и длиннее 240 символов.

    Windows-API без префикса не открывает пути > 260 символов; ffmpeg и Python
    ведут себя по-разному, поэтому нормализуем сами.
    """
    if os.name != "nt":
        return path
    p = os.path.abspath(path)
    if p.startswith("\\\\?\\"):
        return p
    if len(p) < 240:
        return p
    if p.startswith("\\\\"):  # UNC: \\server\share → \\?\UNC\server\share
        return "\\\\?\\UNC\\" + p[2:]
    return "\\\\?\\" + p


def ensure_dir(path: str) -> str:
    """Создать каталог (идемпотентно) и вернуть путь как есть."""
    if path:
        os.makedirs(long_path(path), exist_ok=True)
    return path


@contextlib.contextmanager
def temp_dir(prefix: str = "hds-sidecar-", base=None):
    """Временный каталог, удаляемый на выходе (в т.ч. при исключении)."""
    parent = base or None
    if parent:
        ensure_dir(parent)
    path = tempfile.mkdtemp(prefix=prefix, dir=parent)
    try:
        yield path
    finally:
        shutil.rmtree(long_path(path), ignore_errors=True)


def safe_child(base: str, name: str) -> str:
    """Путь `base/name`, гарантированно внутри `base` (защита от `..`/абсолюта)."""
    base_abs = os.path.abspath(base)
    candidate = os.path.abspath(os.path.join(base_abs, name))
    if candidate != base_abs and not candidate.startswith(base_abs + os.sep):
        raise ValueError("путь выходит за пределы каталога: %s" % name)
    return candidate


def read_json(path: str, default=None):
    """Прочитать JSON; при отсутствии/битом файле — `default`."""
    try:
        with open(long_path(path), "r", encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def write_json_atomic(path: str, obj, indent: int = 2) -> None:
    """Атомарная запись JSON: временный файл рядом → `os.replace`."""
    directory = os.path.dirname(os.path.abspath(path))
    ensure_dir(directory)
    fd, tmp = tempfile.mkstemp(prefix=".hds-", suffix=".tmp", dir=long_path(directory))
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as f:
            json.dump(obj, f, ensure_ascii=False, indent=indent)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, long_path(path))
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(tmp)
        raise


def write_text_atomic(path: str, text: str, encoding: str = "utf-8") -> None:
    """Атомарная запись текста (та же схема, что `write_json_atomic`)."""
    directory = os.path.dirname(os.path.abspath(path))
    ensure_dir(directory)
    fd, tmp = tempfile.mkstemp(prefix=".hds-", suffix=".tmp", dir=long_path(directory))
    try:
        with os.fdopen(fd, "w", encoding=encoding, newline="\n") as f:
            f.write(text)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, long_path(path))
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(tmp)
        raise
