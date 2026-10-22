"""Пути и конфиг, общие для воркеров (извлечение; транскрибация конфиг не читает).

Транскрибация получает все параметры из Rust (`transcribe.params`), поэтому
`load_config` ей не нужен: второй парсер конфига означал бы, что `hds check` и
боевой путь видят разное.

`yaml`/`hds.config` импортируются лениво — на уровне модуля пакет тянет только
stdlib (правило границы §5.1).
"""
from __future__ import annotations

import os
import sys

#: Имя системного каталога состояния (M2 §3.2): там server-поставка держит `config.yaml`.
STATE_DIR_NAME = "hermes-disk-search"

_cache = {}


def state_dir():
    """Системный каталог состояния: Windows — `%ProgramData%\\hermes-disk-search`,
    macOS — `/Library/Application Support/hermes-disk-search`; иначе None."""
    if os.name == "nt":
        pd = os.environ.get("ProgramData")
        return os.path.join(pd, STATE_DIR_NAME) if pd else None
    if sys.platform == "darwin":
        return os.path.join("/Library", "Application Support", STATE_DIR_NAME)
    return None


def config_path(root):
    """Путь `config.yaml` — тем же порядком, что `hds_core::config::config_path`.

    `HDS_CONFIG` → `<root>/config.yaml` (desktop-раскладка) → системный каталог
    состояния (server-раскладка: `app\\<версия>` только для чтения) →
    `<root>/config.yaml` (для внятного сообщения об ошибке).

    РЕГРЕССИЯ (20.10.2026, релиз 0.20.5): раньше читался **только**
    `<root>/config.yaml`, а у служб `HDS_CONFIG` указывал на ProgramData —
    воркер падал с `FileNotFoundError` на первом же `extract`.
    """
    env = (os.environ.get("HDS_CONFIG") or "").strip()
    if env:
        return env
    root_cfg = os.path.join(root, "config.yaml")
    if os.path.exists(root_cfg):
        return root_cfg
    state = state_dir()
    if state:
        sys_cfg = os.path.join(state, "config.yaml")
        if os.path.exists(sys_cfg):
            return sys_cfg
    return root_cfg


def load_config(path=None, root=None, loader=None):
    """Прочитать конфиг: `hds.config.load` (источник истины), иначе `yaml`.

    `loader` — явный загрузчик (тесты/особые раскладки). Результат кэшируется по
    пути: воркер читает конфиг один раз, как и раньше.
    """
    root = root or os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    path = path or config_path(root)
    key = os.path.abspath(path)
    if key in _cache:
        return _cache[key]
    if loader is not None:
        cfg = loader(path)
    else:
        try:
            from hds.config import load as hds_load  # лениво: может отсутствовать в поставке

            cfg = hds_load(path)
        except ImportError:
            import yaml

            with open(path, "r", encoding="utf-8") as f:
                cfg = yaml.safe_load(f) or {}
    if not isinstance(cfg, dict):
        cfg = {}
    _cache[key] = cfg
    return cfg


def reset_cache() -> None:
    """Сбросить кэш конфига (тесты/`reload`)."""
    _cache.clear()


def dig(cfg, dotted, default=None):
    """Значение по точечному ключу (`index.transcribe`) или `default`."""
    cur = cfg
    for part in str(dotted).split("."):
        if not isinstance(cur, dict) or part not in cur:
            return default
        cur = cur[part]
    return cur if cur is not None else default
