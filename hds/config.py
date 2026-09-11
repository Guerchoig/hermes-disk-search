"""Загрузка конфигурации config.yaml (кодировка UTF-8, BOM допускается)."""
import os

import yaml

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def config_path():
    return os.environ.get("HDS_CONFIG") or os.path.join(PROJECT_ROOT, "config.yaml")


def load(path=None):
    p = path or config_path()
    with open(p, "r", encoding="utf-8-sig") as f:
        return yaml.safe_load(f) or {}


def dig(cfg, dotted, default=None):
    """Достать значение по пути вида 'index.roots'."""
    cur = cfg
    for part in dotted.split("."):
        if not isinstance(cur, dict) or part not in cur:
            return default
        cur = cur[part]
    return cur


def db_abs_path(cfg):
    p = dig(cfg, "db_path", "index.db")
    if not os.path.isabs(p):
        p = os.path.join(PROJECT_ROOT, p)
    return p