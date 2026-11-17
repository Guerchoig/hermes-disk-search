"""Sidecar-воркер анонимизации/деанонимизации документов (P1).

Перенос наработок `C:\\Test\\anonymizer_proxy` (решение заказчика,
`PLAN_ANONYMIZER_SIDECAR.md` §2.1): движки NER без правок алгоритмов,
`settings.py` вместо `config.py`, `vault.py` — единственный модуль доступа
к базе меток v2 (SQLite WAL, два уровня меток, `keep_terms`, TR-1.1…1.7).
"""
