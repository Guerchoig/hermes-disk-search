"""Sidecar-воркер анонимизации/деанонимизации документов (P1).

Перенос наработок `C:\\Test\\anonymizer_proxy` (решение заказчика,
`PLAN_ANONYMIZER_SIDECAR.md` §2.1): движки NER без правок алгоритмов,
`settings.py` вместо `config.py`, `mapping_store.py` — единый модуль БД
токенов (SQLite WAL, per-type счётчики + дедуп по (значение, тип)).
"""
