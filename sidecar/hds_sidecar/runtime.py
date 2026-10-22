"""Цикл обслуживания sidecar-воркера: читатель в потоке, idle-timeout, EOF.

`select()` по pipe на Windows не работает, поэтому stdin читает отдельный
поток, а главный цикл ждёт строку из очереди с таймаутом простоя.

Изоляция ошибок: исключение в обработчике **одного** запроса превращается в
структурную ошибку `-32603`, процесс продолжает жить. Иначе клиент пишет в
закрытый канал и каждый следующий файл получает «закрытие канала» (регрессия
0.20.5: 43 537 ошибок в БД за прогон).
"""
from __future__ import annotations

import json
import queue
import threading

from . import stdio

#: Коды ошибок JSON-RPC (как у воркера извлечения).
CODE_BAD_JSON = -32700
CODE_INTERNAL = -32603
CODE_UNKNOWN_METHOD = -32601


def serve(handle, idle_timeout: float = 60.0, control=None) -> int:
    """Обслуживать запросы до EOF/`shutdown`/простоя.

    `handle(req)` разбирает запрос и отвечает через :mod:`stdio`; возвращает
    истинное значение, если процесс нужно завершить (`shutdown`). Исключение в
    обработчике не завершает цикл — клиент получает `-32603`.

    `control(req)` вызывается **в потоке-читателе** до постановки запроса в
    очередь. Это единственный способ обработать `cancel` во время долгого
    `transcribe`: главный поток занят, последовательный `handle` до очереди не
    дойдёт. Ответ на `cancel` всё равно отправит `handle`, когда освободится.
    """
    lines: "queue.Queue" = queue.Queue()

    def reader() -> None:
        while True:
            raw = stdio.readline()
            if not raw:
                break
            line = raw.decode("utf-8", "replace")
            if control is not None:
                try:
                    early = json.loads(line)
                except Exception:  # noqa: BLE001 - битый JSON обработает главный цикл
                    early = None
                if isinstance(early, dict):
                    try:
                        control(early)
                    except Exception as e:  # noqa: BLE001 - отмена не должна убивать читатель
                        stdio.err(None, CODE_INTERNAL, "control error: %s" % e)
            lines.put(line)
        lines.put(None)  # EOF

    threading.Thread(target=reader, daemon=True, name="hds-sidecar-reader").start()

    timeout = idle_timeout if idle_timeout and idle_timeout > 0 else None
    while True:
        try:
            line = lines.get(timeout=timeout)
        except queue.Empty:
            break  # простой → штатный выход (0 процессов/0 МБ в ожидании)
        if line is None:
            break  # EOF на stdin — основной путь остановки (родитель владеет процессом)
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as e:  # noqa: BLE001
            stdio.err(None, CODE_BAD_JSON, "bad json: %s" % e)
            continue
        if not isinstance(req, dict):
            stdio.err(None, CODE_BAD_JSON, "запрос должен быть JSON-объектом")
            continue
        stop = False
        try:
            stop = bool(handle(req))
        except SystemExit:
            raise
        except BaseException as e:  # noqa: BLE001 - процесс не должен умирать от одного запроса
            stdio.err(
                req.get("id"),
                CODE_INTERNAL,
                "internal error: %s: %s" % (type(e).__name__, e),
            )
        if stop:
            break
    return 0
