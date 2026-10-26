"""Единый модуль БД токенов (SQLite WAL) — `data/anonymize.db`.

Решения плана (§2.1), не пересматриваются:
* БД живёт в sidecar; Rust к ней не обращается;
* схема токенов — **per-type счётчики + дедуп по (значение, тип)**:
  `[PERSON_1]`, `[ORG_1]`, …; одно и то же значение разных типов получает
  разные токены (уник. индекс по (value, type));
* сессий нет (решение №1): одна общая таблица маппингов на все акты.

Схема:
    mappings(token PK, value, type, ts)
    counters(type PK, last)

Два процесса (UI + CLI) на одной БД: WAL + `busy_timeout` + выдача номера
внутри `BEGIN IMMEDIATE`. PII-значения наружу отдаёт только деанонимизация;
в логи пишутся токены/счётчики.
"""
from __future__ import annotations

import os
import re
import sqlite3
import time
from pathlib import Path
from typing import Optional

# Токен вида [PERSON_12] (как в anonymizer_proxy/replacer.py TOKEN_PATTERN)
TOKEN_PATTERN = re.compile(r"\[([A-Z_]+)_(\d+)\]")

_BUSY_TIMEOUT_MS = 5000


class MappingStore:
    """SQLite-хранилище маппингов токен ↔ значение (sync; вызовы быстрые)."""

    def __init__(self, db_path: str | os.PathLike) -> None:
        self.db_path = str(db_path)
        parent = Path(self.db_path).parent
        if str(parent):
            parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(
            self.db_path, timeout=_BUSY_TIMEOUT_MS / 1000.0, isolation_level=None
        )
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute(f"PRAGMA busy_timeout={_BUSY_TIMEOUT_MS}")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._init_schema()

    def _init_schema(self) -> None:
        cur = self._conn
        cur.execute("BEGIN IMMEDIATE")
        try:
            cur.execute(
                """
                CREATE TABLE IF NOT EXISTS mappings (
                    token TEXT PRIMARY KEY,
                    value TEXT NOT NULL,
                    type  TEXT NOT NULL,
                    ts    REAL NOT NULL
                )
                """
            )
            # Дедуп по (значение, тип): решение заказчика (§2.1)
            cur.execute(
                """
                CREATE UNIQUE INDEX IF NOT EXISTS ux_mappings_value_type
                    ON mappings(value, type)
                """
            )
            cur.execute(
                """
                CREATE TABLE IF NOT EXISTS counters (
                    type TEXT PRIMARY KEY,
                    last INTEGER NOT NULL DEFAULT 0
                )
                """
            )
            cur.execute("COMMIT")
        except BaseException:
            cur.execute("ROLLBACK")
            raise

    # ── выдача токена ───────────────────────────────────────────────────

    def get_or_create_token(self, value: str, entity_type: str) -> str:
        """Токен для (value, type): существующий или новый (per-type счётчик)."""
        conn = self._conn
        conn.execute("BEGIN IMMEDIATE")
        try:
            row = conn.execute(
                "SELECT token FROM mappings WHERE value = ? AND type = ?",
                (value, entity_type),
            ).fetchone()
            if row is not None:
                conn.execute("COMMIT")
                return row[0]

            conn.execute(
                "INSERT INTO counters(type, last) VALUES(?, 0) "
                "ON CONFLICT(type) DO NOTHING",
                (entity_type,),
            )
            conn.execute(
                "UPDATE counters SET last = last + 1 WHERE type = ?",
                (entity_type,),
            )
            last = conn.execute(
                "SELECT last FROM counters WHERE type = ?", (entity_type,)
            ).fetchone()[0]
            token = f"[{entity_type}_{last}]"
            try:
                conn.execute(
                    "INSERT INTO mappings(token, value, type, ts) VALUES(?, ?, ?, ?)",
                    (token, value, entity_type, time.time()),
                )
            except sqlite3.IntegrityError:
                # Проиграли гонку другому процессу: токен с таким номером уже
                # занят другим значением (коллизия PK) — берём следующий номер.
                row = conn.execute(
                    "SELECT token FROM mappings WHERE value = ? AND type = ?",
                    (value, entity_type),
                ).fetchone()
                if row is not None:
                    conn.execute("COMMIT")
                    return row[0]
                raise
            conn.execute("COMMIT")
            return token
        except BaseException:
            conn.execute("ROLLBACK")
            raise

    # ── чтение ──────────────────────────────────────────────────────────

    def get_value(self, token: str) -> Optional[str]:
        row = self._conn.execute(
            "SELECT value FROM mappings WHERE token = ?", (token,)
        ).fetchone()
        return row[0] if row else None

    def get_all_mappings(self) -> dict[str, str]:
        """Все маппинги: {token: value} (для полной деанонимизации)."""
        rows = self._conn.execute("SELECT token, value FROM mappings").fetchall()
        return {token: value for token, value in rows}

    def get_mappings_for_tokens(
        self, tokens: list[str]
    ) -> tuple[dict[str, str], list[str]]:
        """Подмножество {token: value} для выборочной деанонимизации
        + список токенов, которых нет в БД (unmatched)."""
        if not tokens:
            return {}, []
        placeholders = ",".join("?" for _ in tokens)
        rows = self._conn.execute(
            f"SELECT token, value FROM mappings WHERE token IN ({placeholders})",
            list(tokens),
        ).fetchall()
        found = {token: value for token, value in rows}
        unmatched = [t for t in tokens if t not in found]
        return found, unmatched

    def stats(self) -> dict:
        """Счётчики для `inspect`/диагностики (без значений)."""
        total = self._conn.execute("SELECT COUNT(*) FROM mappings").fetchone()[0]
        by_type = {
            t: n
            for t, n in self._conn.execute(
                "SELECT type, COUNT(*) FROM mappings GROUP BY type"
            ).fetchall()
        }
        return {"total": total, "by_type": by_type}

    def close(self) -> None:
        try:
            self._conn.close()
        except Exception:  # noqa: BLE001
            pass
