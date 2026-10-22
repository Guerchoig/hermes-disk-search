"""Реэкспорт файловых помощников общего пакета (транскрибация >1 уровня вложенности).

`hds_transcribe` живёт рядом с `hds_sidecar`, но импортировать его напрямую
можно только когда каталог `sidecar/` в `sys.path` (это делает `worker.py`).
Модули пакета не должны зависеть от порядка импорта — поэтому тонкий фасад.
"""
from __future__ import annotations

import re

from hds_sidecar.fs import (  # noqa: F401
    ensure_dir,
    long_path,
    read_json,
    safe_child,
    temp_dir,
    write_json_atomic,
    write_text_atomic,
)


def safe_name(name: str) -> str:
    """Имя файла без символов, запрещённых Windows (`\\ / : * ? " < > |`)."""
    cleaned = re.sub(r'[\\/:*?"<>|\x00-\x1f]', "_", name or "").strip(" .")
    return cleaned or "output"
