"""Прогресс и журнал sidecar-воркера — **только в stderr**.

В протокол прогресс не кладём: клиент разбирает ответ по `id`, лишняя строка
сломала бы `parse_response`. `WorkerConfig.stderr_log` пишет эти строки в журнал
задания; живой прогресс в UI — отдельная задача (§5.2).
"""
from __future__ import annotations

import contextlib
import sys
import time


def log(message: str) -> None:
    """Строка журнала с отметкой времени: `[12:34:56] ...`."""
    sys.stderr.write("[%s] %s\n" % (time.strftime("%H:%M:%S"), message))
    sys.stderr.flush()


def stage(name: str, percent=None, fields=None, **extra) -> None:
    """Стадия обработки: `[stage] asr 42% chunks=17`.

    `fields` — словарь дополнительных полей (позиционно), `extra` — именованные.
    """
    data = dict(extra)
    if isinstance(fields, dict):
        data.update(fields)
    parts = ["[stage]", str(name)]
    if percent is not None:
        parts.append("%.0f%%" % (float(percent) * 100.0 if percent <= 1.0 else percent))
    for key, value in data.items():
        if value is None:
            continue
        parts.append("%s=%s" % (key, value))
    sys.stderr.write(" ".join(parts) + "\n")
    sys.stderr.flush()


def warn(message: str) -> None:
    """Предупреждение стадии (не ошибка): `[warn] ...`."""
    sys.stderr.write("[warn] %s\n" % message)
    sys.stderr.flush()


def redirect_stdout():
    """Контекст: вывод библиотек (tqdm/принты) уходит в stderr.

    Нужен, потому что часть библиотек печатает в `sys.stdout`, который мы уже
    подменили на stderr, а часть — напрямую в fd 1 (его перенаправил `stdio.init`).
    """
    return contextlib.redirect_stdout(sys.stderr)
