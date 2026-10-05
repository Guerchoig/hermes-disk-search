# hermes-disk-search
import os

__version__ = "0.15.1"

# Файлы-входы метки сборки: правка любого из них (git pull, распаковка архива
# релиза) должна приводить к перезапуску общего MCP-сервера — иначе на порту
# (:8787) продолжит работать СТАРЫЙ код. См. hds.mcp_http.restart_if_stale.
_BUILD_INPUTS = ("requirements.txt",)


def build_stamp():
    """Метка сборки кода: максимальный mtime исходников (hds/*.py + requirements.txt).

    Момент времени (Unix, float), а не хеш: сравнение «код новее процесса»
    сводится к os.path.getmtime и не требует читать содержимое файлов.
    0.0 — метка неизвестна (исходники недоступны).
    """
    pkg = os.path.dirname(os.path.abspath(__file__))
    root = os.path.dirname(pkg)
    paths = [os.path.join(root, name) for name in _BUILD_INPUTS]
    try:
        paths += [os.path.join(pkg, n) for n in os.listdir(pkg) if n.endswith(".py")]
    except OSError:
        pass
    stamp = 0.0
    for path in paths:
        try:
            stamp = max(stamp, os.path.getmtime(path))
        except OSError:
            continue
    return stamp
