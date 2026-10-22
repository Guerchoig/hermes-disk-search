"""pytest/unittest: делаем импортируемыми `hds_sidecar` и `hds_transcribe`.

Тесты запускаются из корня репозитория:
    python -m pytest sidecar\\tests
    python -m unittest discover sidecar\\tests
"""
from __future__ import annotations

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_SIDECAR = os.path.dirname(_HERE)
if _SIDECAR not in sys.path:
    sys.path.insert(0, _SIDECAR)
