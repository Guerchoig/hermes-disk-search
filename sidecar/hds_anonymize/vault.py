"""База меток v2 — единственный модуль доступа к `anonymize.db` (TR-1.1).

Схема — ровно TR-1.2 `PLAN_AGENT_PRIVACY_ZONE.md`: **одна база, два уровня
меток** (D3):

* уровень 0 — `entities`/`forms`/`global_counters`: глобальные метки
  (`[PERSON_7]`), исходные значения живут **только** здесь. Их используют
  карточка «Анонимизация» и `hds anon` — поведение прежнее, суффиксов нет;
* уровень 1 — `projects`/`labels`/`counters`: метки проекта (`[PERSON_3]`),
  связанные с глобальными сущностями; номера выдаются счётчиками проекта в
  порядке первого появления и **не выводятся** из глобального номера
  (TR-1.5). В анонимную зону попадают только они;
* `aliases` — след «Объединить метки» (TR-2.6): уже выпущенные файлы с
  младшими метками по-прежнему деанонимизируются;
* `keep_terms` — «не анонимизировать» (D19). Записи `re:<шаблон>` —
  регулярные выражения (`REQ-\\d+`), остальное — точные значения.

Миграции с v1 нет (D4, TR-1.3): файл прежней схемы переименовывается в
`anonymize.db.v1-<YYYYMMDD-HHMMSS>.bak` и создаётся новая база.

Словоформы склеиваются по `norm_key` (`morph.py`): одно значение в разных
падежах — одна сущность и одна метка (D7). Формы хранятся с граммемами:
`origin='seen'` — встретились в тексте, `origin='derived'` — порождены
pymorphy2 (TR-2.2). Обратная подстановка `[T_n|падеж]` идёт по `forms`
(TR-2.4).

Rust и Node к файлу не обращаются: наружу — только RPC воркера
(`vault_snapshot`, `anonymize_text`, …), значения PII в логи не пишутся.
"""
from __future__ import annotations

import contextlib
import logging
import os
import re
import sqlite3
import time
from pathlib import Path
from typing import Iterable, Optional

from . import morph

logger = logging.getLogger("hds_anonymize.vault")

# Версия схемы (TR-1.2: meta.schema_version = 2)
SCHEMA_VERSION = 2

# Синтаксис метки (TR-1.4) — единый для Python и Rust:
#   [TYPE_n] или [TYPE_n|падеж] / [TYPE_n|падеж,мн]
LABEL_RE = re.compile(r"\[([A-Z_]+)_(\d+)(?:\|([а-яё,]+))?\]")
# Терпимый разбор (TR-2.4): [person_3], [PERSON 3], [PERSON-3]
TOLERANT_LABEL_RE = re.compile(
    r"\[\s*([A-Za-zА-Яа-яЁё_]+)[\s_-]+(\d+)\s*(?:\|\s*([а-яё,]+))?\s*\]"
)

# Префикс регулярных выражений в keep_terms (`re:REQ-\d+`), см. §3.3 карты A1
KEEP_REGEX_PREFIX = "re:"

# Стартовый список «не анонимизировать» (D19, П-14): продукты и термины 1С,
# которые NER иначе метит как ORG/LOC/INN (факт A0 §3.9), и коды требований.
DEFAULT_KEEP_TERMS = (
    # фирма и платформа
    "1С", "1С:Предприятие", "1С:ERP", "1С:ERP 2.5", "1С:Бухгалтерия",
    "1С:ЗУП", "1С:Документооборот", "1С:УТ", "1С:УНФ",
    # короткие имена продуктов (в текстах — без префикса «1С:»)
    "ЗУП", "УТ", "УТ 11", "УНФ", "Документооборот", "ДиректБанк", "ГИС МТ",
    # термины проекта
    "ТЗ", "ТЭО", "ФТТ", "HTTP-сервис", "RLS", "регламентное задание",
    "ERPLY", "СБИС",
    # коды требований — регулярка, а не значение
    r"re:REQ-\d+",
)

# Сколько предупреждений «проверить» отдавать в ответе (остальное — счётчик)
MAX_WARNINGS = 20

_BUSY_TIMEOUT_MS = 5000


class VaultError(RuntimeError):
    """Ошибка базы меток (структурируется воркером как `kind: vault`)."""


class KeepTermError(VaultError):
    """Значение входит в keep_terms: метка не выдаётся (D19)."""


def label_of(entity_type: str, n: int, suffix: Optional[str] = None) -> str:
    """Метка по типу/номеру: `[PERSON_3]` или `[PERSON_3|дат]` (TR-1.4).

    `suffix` — **русский** падежный суффикс (`дат`, `род,мн`), не код
    pymorphy: в базах и файлах встречается только он (TR-2.3).
    """
    base = f"[{entity_type}_{n}]"
    return f"[{entity_type}_{n}|{suffix}]" if suffix else base


def parse_label(token: str) -> Optional[tuple[str, int, Optional[str]]]:
    """Разобрать метку: `"[PERSON_3|дат]"` → `("PERSON", 3, "дат")`.

    Третий элемент — русский суффикс падежа (или None). Терпимый разбор
    (TR-2.4) принимает `[person_3]`, `[PERSON 3]`, `[PERSON-3]`; тип
    приводится к верхнему регистру.
    """
    token = (token or "").strip()
    match = LABEL_RE.fullmatch(token)
    if match:
        return match.group(1), int(match.group(2)), match.group(3)
    match = TOLERANT_LABEL_RE.fullmatch(token)
    if match:
        return match.group(1).upper(), int(match.group(2)), match.group(3)
    return None


def normalize_token(token: str) -> str:
    """Привести метку к каноническому виду (`person_3` → `[PERSON_3]`)."""
    token = (token or "").strip()
    if token.startswith("[") and token.endswith("]"):
        parsed = parse_label(token)
        return label_of(*parsed) if parsed else token
    return f"[{token}]"


class Vault:
    """SQLite-хранилище меток v2 (sync; вызовы короткие).

    Два процесса (UI + CLI) на одной базе: WAL + `busy_timeout`, выдача
    номеров внутри `BEGIN IMMEDIATE`.
    """

    def __init__(
        self,
        db_path: str | os.PathLike,
        *,
        keep_terms: Optional[Iterable[str]] = None,
        seed_default_keep: bool = True,
    ) -> None:
        self.db_path = str(db_path)
        parent = Path(self.db_path).parent
        if str(parent):
            parent.mkdir(parents=True, exist_ok=True)
        # Открыть базу прежней схемы нельзя (D4): файл уводится в бэкап
        self.backup_path: Optional[str] = self._migrate_if_needed(Path(self.db_path))
        self._conn = sqlite3.connect(
            self.db_path, timeout=_BUSY_TIMEOUT_MS / 1000.0, isolation_level=None
        )
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute(f"PRAGMA busy_timeout={_BUSY_TIMEOUT_MS}")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._keep_cache: Optional[tuple[frozenset[str], tuple[re.Pattern, ...]]] = None
        self._init_schema()
        # keep_terms: сначала файл/аргумент, затем стартовый список D19
        if keep_terms:
            self.seed_keep_terms(keep_terms)
        if seed_default_keep:
            self.seed_keep_terms(DEFAULT_KEEP_TERMS)

    # ── миграция v1 → v2 (TR-1.3, D4) ───────────────────────────────────

    @staticmethod
    def _migrate_if_needed(path: Path) -> Optional[str]:
        """Переименовать базу прежней схемы и вернуть путь бэкапа.

        Метки **не переносятся** (D4): файл уводится в
        `anonymize.db.v1-<YYYYMMDD-HHMMSS>.bak`, новая база создаётся с нуля.
        """
        if not path.is_file() or path.stat().st_size == 0:
            return None
        version: Optional[str] = None
        try:
            conn = sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True)
            try:
                row = conn.execute(
                    "SELECT value FROM meta WHERE key = 'schema_version'"
                ).fetchone()
                version = row[0] if row else None
            finally:
                conn.close()
        except sqlite3.Error:
            version = None  # нет таблицы meta — схема v1 или не база меток
        if version is not None and str(version) == str(SCHEMA_VERSION):
            return None
        stamp = time.strftime("%Y%m%d-%H%M%S")
        backup = path.with_name(f"{path.name}.v1-{stamp}.bak")
        try:
            os.replace(path, backup)
        except OSError as exc:
            raise VaultError(
                f"не удалось переименовать базу прежней схемы v1 ({path.name}): {exc}. "
                "Закройте воркер/карточку анонимизации и повторите"
            ) from exc
        for suffix in ("-wal", "-shm"):
            try:
                os.remove(f"{path}{suffix}")
            except FileNotFoundError:
                pass
        logger.warning(
            "База меток прежней схемы (v%s) переименована: %s; метки не переносятся (D4)",
            version or "1",
            backup.name,
        )
        return str(backup)

    def rev(self) -> int:
        """Текущая ревизия базы (растёт при любой записи, TR-1.2/TR-1.6)."""
        row = self._conn.execute(
            "SELECT value FROM meta WHERE key = 'rev'"
        ).fetchone()
        return int(row[0]) if row else 0

    def close(self) -> None:
        try:
            self._conn.close()
        except Exception:  # noqa: BLE001
            pass


    # ── схема (TR-1.2) ──────────────────────────────────────────────────

    def _init_schema(self) -> None:
        conn = self._conn
        conn.execute("BEGIN IMMEDIATE")
        try:
            conn.execute(
                "CREATE TABLE IF NOT EXISTS meta ("
                " key TEXT PRIMARY KEY, value TEXT NOT NULL)"
            )
            # Уровень 0: глобальные метки = сущности (исходные значения — здесь)
            conn.execute(
                "CREATE TABLE IF NOT EXISTS entities ("
                " id INTEGER PRIMARY KEY AUTOINCREMENT,"
                " type TEXT NOT NULL,"
                " n INTEGER NOT NULL,"
                " norm_key TEXT NOT NULL,"
                " ts REAL NOT NULL,"
                " UNIQUE(type, n),"
                " UNIQUE(type, norm_key))"
            )
            conn.execute(
                "CREATE TABLE IF NOT EXISTS forms ("
                " id INTEGER PRIMARY KEY AUTOINCREMENT,"
                " entity_id INTEGER NOT NULL REFERENCES entities(id),"
                " value TEXT NOT NULL,"
                " gram TEXT NULL,"
                " origin TEXT NOT NULL CHECK(origin IN ('seen','derived')),"
                " UNIQUE(entity_id, value))"
            )
            conn.execute(
                "CREATE TABLE IF NOT EXISTS global_counters ("
                " type TEXT PRIMARY KEY, last INTEGER NOT NULL DEFAULT 0)"
            )
            # Уровень 1: метки проектов, связанные с глобальными
            conn.execute(
                "CREATE TABLE IF NOT EXISTS projects ("
                " id TEXT PRIMARY KEY, ts REAL NOT NULL)"
            )
            conn.execute(
                "CREATE TABLE IF NOT EXISTS labels ("
                " project_id TEXT NOT NULL,"
                " entity_id INTEGER NOT NULL REFERENCES entities(id),"
                " type TEXT NOT NULL,"
                " n INTEGER NOT NULL,"
                " ts REAL NOT NULL,"
                " PRIMARY KEY(project_id, entity_id),"
                " UNIQUE(project_id, type, n))"
            )
            conn.execute(
                "CREATE TABLE IF NOT EXISTS counters ("
                " project_id TEXT NOT NULL,"
                " type TEXT NOT NULL,"
                " last INTEGER NOT NULL DEFAULT 0,"
                " PRIMARY KEY(project_id, type))"
            )
            conn.execute(
                "CREATE TABLE IF NOT EXISTS aliases ("
                " project_id TEXT NULL, type TEXT NOT NULL, n INTEGER NOT NULL,"
                " entity_id INTEGER NOT NULL)"
            )
            conn.execute(
                "CREATE TABLE IF NOT EXISTS keep_terms (value TEXT PRIMARY KEY)"
            )
            conn.execute(
                "INSERT INTO meta(key, value) VALUES('schema_version', ?)"
                " ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (str(SCHEMA_VERSION),),
            )
            conn.execute(
                "INSERT INTO meta(key, value) VALUES('rev', '0')"
                " ON CONFLICT(key) DO NOTHING"
            )
            conn.execute("COMMIT")
        except BaseException:
            conn.execute("ROLLBACK")
            raise

    # ── транзакции ──────────────────────────────────────────────────────

    @contextlib.contextmanager
    def _write(self):
        """Запись в `BEGIN IMMEDIATE`; любая запись растит `meta.rev`."""
        conn = self._conn
        conn.execute("BEGIN IMMEDIATE")
        try:
            yield conn
            conn.execute(
                "UPDATE meta SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)"
                " WHERE key = 'rev'"
            )
            conn.execute("COMMIT")
        except BaseException:
            conn.execute("ROLLBACK")
            raise
        self._keep_cache = None


    # ── сущности и формы (уровень 0) ────────────────────────────────────

    def _add_form(
        self,
        conn,
        entity_id: int,
        value: str,
        gram: Optional[str],
        origin: str,
    ) -> None:
        """Добавить форму сущности (дедуп по значению — UNIQUE(entity_id,value)).

        Встреченная в тексте форма **повышает** происхождение до `seen`, даже
        если раньше была порождённой: иначе обратная подстановка считала бы
        точную форму «проверить» (TR-2.4). Порождённая форма `seen` не
        понижает.
        """
        if origin == "seen":
            conn.execute(
                "INSERT INTO forms(entity_id, value, gram, origin) VALUES(?, ?, ?, 'seen')"
                " ON CONFLICT(entity_id, value) DO UPDATE SET origin = 'seen',"
                " gram = COALESCE(forms.gram, excluded.gram)",
                (entity_id, value, gram),
            )
            return
        conn.execute(
            "INSERT INTO forms(entity_id, value, gram, origin) VALUES(?, ?, ?, 'derived')"
            " ON CONFLICT(entity_id, value) DO NOTHING",
            (entity_id, value, gram),
        )

    def _ensure_entity(self, conn, value: str, entity_type: str) -> tuple[int, int]:
        """Сущность (id, n) для значения; создаётся с формами при первом появлении.

        Склейка словоформ — по `norm_key` (TR-2.1): «Иванову И. И.» и
        «Иванов И. И.» — одна сущность и одна метка (D7).
        """
        value = value.strip()
        key = morph.norm_key(value, entity_type) or value.lower()
        row = conn.execute(
            "SELECT id, n FROM entities WHERE type = ? AND norm_key = ?",
            (entity_type, key),
        ).fetchone()
        if row is not None:
            self._add_form(conn, row[0], value, morph.gram_of(value, entity_type), "seen")
            return int(row[0]), int(row[1])

        conn.execute(
            "INSERT INTO global_counters(type, last) VALUES(?, 0)"
            " ON CONFLICT(type) DO NOTHING",
            (entity_type,),
        )
        conn.execute(
            "UPDATE global_counters SET last = last + 1 WHERE type = ?",
            (entity_type,),
        )
        n = int(
            conn.execute(
                "SELECT last FROM global_counters WHERE type = ?", (entity_type,)
            ).fetchone()[0]
        )
        try:
            cursor = conn.execute(
                "INSERT INTO entities(type, n, norm_key, ts) VALUES(?, ?, ?, ?)",
                (entity_type, n, key, time.time()),
            )
        except sqlite3.IntegrityError:
            # Проиграли гонку другому процессу: сущность уже создана
            row = conn.execute(
                "SELECT id, n FROM entities WHERE type = ? AND norm_key = ?",
                (entity_type, key),
            ).fetchone()
            if row is None:
                raise
            self._add_form(conn, row[0], value, morph.gram_of(value, entity_type), "seen")
            return int(row[0]), int(row[1])
        entity_id = int(cursor.lastrowid)
        self._add_form(conn, entity_id, value, morph.gram_of(value, entity_type), "seen")
        # Порождённые падежи (TR-2.2): нужны фильтру исходящего и подстановке
        for form, gram in morph.generate_forms(value, entity_type):
            if form == value:
                continue
            self._add_form(conn, entity_id, form, gram, "derived")
        return entity_id, n

    def _entity_by_type_n(self, entity_type: str, n: int):
        return self._conn.execute(
            "SELECT id, type, n FROM entities WHERE type = ? AND n = ?",
            (entity_type, n),
        ).fetchone()

    def _nominal_value(self, entity_id: int, entity_type: str) -> Optional[str]:
        """Именительная форма сущности (seen → derived → любая seen)."""
        rows = self._conn.execute(
            "SELECT value, gram, origin FROM forms WHERE entity_id = ? ORDER BY id",
            (entity_id,),
        ).fetchall()
        for value, gram, origin in rows:
            if origin == "seen" and (gram is None or gram.startswith("nomn")):
                return value
        for value, gram, origin in rows:
            if gram is not None and gram.startswith("nomn"):
                return value
        for value, gram, origin in rows:
            if origin == "seen":
                return value
        return rows[0][0] if rows else None

    def _resolve_form(
        self,
        entity_id: int,
        entity_type: str,
        gram: Optional[str],
        warnings: Optional[list[str]] = None,
    ) -> Optional[str]:
        """Форма значения по граммеме метки (TR-2.4).

        Порядок: `seen` с этой граммемой → `derived` с этой граммемой
        (пометка «проверить») → именительная форма с пометкой «падеж не
        согласован».
        """
        if not gram:
            return self._nominal_value(entity_id, entity_type)
        row = self._conn.execute(
            "SELECT value FROM forms WHERE entity_id = ? AND gram = ? AND origin = 'seen'",
            (entity_id, gram),
        ).fetchone()
        if row is not None:
            return row[0]
        row = self._conn.execute(
            "SELECT value FROM forms WHERE entity_id = ? AND gram = ? AND origin = 'derived'",
            (entity_id, gram),
        ).fetchone()
        if row is not None:
            if warnings is not None:
                warnings.append("проверить: падеж формы взят из порождённых")
            return row[0]
        case, plural = morph.parse_gram(gram)
        base = self._nominal_value(entity_id, entity_type)
        if base and case:
            generated = morph.inflect_value(base, entity_type, case, plural)
            if generated:
                if warnings is not None:
                    warnings.append("проверить: падеж формы сгенерирован")
                return generated
        if warnings is not None:
            warnings.append("падеж не согласован: подставлена именительная форма")
        return base


    # ── keep_terms (D19) ───────────────────────────────────────────────

    def seed_keep_terms(self, values: Iterable[str]) -> int:
        """Добавить значения/шаблоны в keep_terms; вернуть число добавленных."""
        added = 0
        with self._write() as conn:
            for value in values:
                value = (value or "").strip()
                if not value:
                    continue
                cursor = conn.execute(
                    "INSERT INTO keep_terms(value) VALUES(?) ON CONFLICT(value) DO NOTHING",
                    (value,),
                )
                if cursor.rowcount and cursor.rowcount > 0:
                    added += 1
        return added

    def add_keep_term(self, value: str) -> bool:
        """«Не скрывать»: значение уходит в keep_terms (TR-2.6, панель меток)."""
        return self.seed_keep_terms([value]) > 0

    def keep_list(self) -> list[str]:
        return [
            row[0]
            for row in self._conn.execute("SELECT value FROM keep_terms ORDER BY value")
        ]

    def _keep_matchers(self) -> tuple[frozenset[str], tuple[re.Pattern, ...]]:
        """Точные значения и шаблоны keep_terms (+ компоненты составных терминов).

        NER режет составные названия: «1С:ERP» он метит как `ERP`,
        «1С:ЗУП» — как `ЗУП`. Поэтому компоненты составных терминов
        (`ERP`, `ЗУП`, `Документооборот`) считаются «своими» и не получают
        меток — иначе агент теряет предметную область (D19).
        """
        if self._keep_cache is None:
            exact: set[str] = set()
            patterns: list[re.Pattern] = []
            for entry in self.keep_list():
                if entry.startswith(KEEP_REGEX_PREFIX):
                    try:
                        patterns.append(
                            re.compile(entry[len(KEEP_REGEX_PREFIX):], re.IGNORECASE)
                        )
                    except re.error as exc:
                        logger.warning("keep_terms: битый шаблон %r (%s)", entry, exc)
                    continue
                exact.add(entry.casefold())
                for part in re.split(r"[^\w]+", entry):
                    if len(part) >= 3:
                        exact.add(part.casefold())
            self._keep_cache = (frozenset(exact), tuple(patterns))
        return self._keep_cache

    def is_kept(self, value: str) -> bool:
        """Входит ли значение в «не анонимизировать» (точное или `re:`-совпадение)."""
        value = (value or "").strip()
        if not value:
            return False
        exact, patterns = self._keep_matchers()
        if value.casefold() in exact:
            return True
        return any(pattern.fullmatch(value) for pattern in patterns)

    # ── уровень 0: глобальные метки (карточка и `hds anon`) ────────────

    def global_label(
        self, value: str, entity_type: str, *, respect_keep: bool = True
    ) -> Optional[str]:
        """Глобальная метка значения `[PERSON_7]` — без суффиксов (TR-2.3).

        Возвращает None, если значение входит в keep_terms (D19).
        """
        value = (value or "").strip()
        if not value:
            return None
        if respect_keep and self.is_kept(value):
            return None
        with self._write() as conn:
            _entity_id, n = self._ensure_entity(conn, value, entity_type)
        return label_of(entity_type, n)

    def get_or_create_token(self, value: str, entity_type: str) -> str:
        """Синоним `global_label` для прежних вызовов (совместимость P1)."""
        token = self.global_label(value, entity_type, respect_keep=False)
        assert token is not None
        return token

    def get_value(self, token: str) -> Optional[str]:
        """Значение глобальной метки (с учётом суффикса падежа)."""
        parsed = parse_label(token)
        if parsed is None:
            return None
        entity_type, n, suffix = parsed
        row = self._entity_by_type_n(entity_type, n)
        if row is None:
            return None
        gram = morph.gram_from_suffix(suffix)
        return self._resolve_form(int(row[0]), entity_type, gram)

    def get_all_mappings(self) -> dict[str, str]:
        """Все глобальные метки: `{token: value}` (полная деанонимизация)."""
        out: dict[str, str] = {}
        for entity_id, entity_type, n in self._conn.execute(
            "SELECT id, type, n FROM entities ORDER BY type, n"
        ).fetchall():
            value = self._nominal_value(int(entity_id), entity_type)
            if value is not None:
                out[label_of(entity_type, int(n))] = value
        return out

    def get_mappings_for_tokens(
        self, tokens: list[str]
    ) -> tuple[dict[str, str], list[str]]:
        """Подмножество `{token: value}` + список ненайденных токенов."""
        found: dict[str, str] = {}
        unmatched: list[str] = []
        for token in tokens:
            value = self.get_value(token)
            if value is None:
                unmatched.append(token)
            else:
                found[normalize_token(token)] = value
        return found, unmatched

    def stats(self) -> dict:
        """Счётчики для `inspect`/диагностики (без значений)."""
        by_type = {
            row[0]: row[1]
            for row in self._conn.execute(
                "SELECT type, COUNT(*) FROM entities GROUP BY type"
            ).fetchall()
        }
        return {
            "total": self._conn.execute("SELECT COUNT(*) FROM entities").fetchone()[0],
            "by_type": by_type,
            "projects": self._conn.execute(
                "SELECT COUNT(*) FROM projects"
            ).fetchone()[0],
            "keep_terms": self._conn.execute(
                "SELECT COUNT(*) FROM keep_terms"
            ).fetchone()[0],
            "rev": self.rev(),
            "schema_version": SCHEMA_VERSION,
        }


    # ── уровень 1: метки проекта (TR-1.5, TR-2.3) ──────────────────────

    def ensure_project(self, project_id: str) -> None:
        """Зарегистрировать проект (имя проекта в базе не хранится, TR-1.2)."""
        project_id = (project_id or "").strip()
        if not project_id:
            raise VaultError("не задан project_id")
        with self._write() as conn:
            conn.execute(
                "INSERT INTO projects(id, ts) VALUES(?, ?) ON CONFLICT(id) DO NOTHING",
                (project_id, time.time()),
            )

    def _ensure_label(
        self, conn, project_id: str, entity_id: int, entity_type: str
    ) -> int:
        """Номер метки проекта для сущности (счётчик проекта, TR-1.5)."""
        row = conn.execute(
            "SELECT n FROM labels WHERE project_id = ? AND entity_id = ?",
            (project_id, entity_id),
        ).fetchone()
        if row is not None:
            return int(row[0])
        conn.execute(
            "INSERT INTO counters(project_id, type, last) VALUES(?, ?, 0)"
            " ON CONFLICT(project_id, type) DO NOTHING",
            (project_id, entity_type),
        )
        conn.execute(
            "UPDATE counters SET last = last + 1 WHERE project_id = ? AND type = ?",
            (project_id, entity_type),
        )
        n = int(
            conn.execute(
                "SELECT last FROM counters WHERE project_id = ? AND type = ?",
                (project_id, entity_type),
            ).fetchone()[0]
        )
        conn.execute(
            "INSERT INTO labels(project_id, entity_id, type, n, ts) VALUES(?, ?, ?, ?, ?)",
            (project_id, entity_id, entity_type, n, time.time()),
        )
        return n

    def project_label_ex(
        self,
        project_id: str,
        value: str,
        entity_type: str,
        *,
        suffix: bool = True,
        respect_keep: bool = True,
    ) -> tuple[Optional[str], bool]:
        """Метка проекта: `(метка, падеж однозначен)`.

        `suffix=True` — с падежным суффиксом (TR-2.3); в именах файлов и
        папок суффикса нет всегда (`suffix=False`). None — значение в
        keep_terms (D19). Второй элемент — «падеж определён однозначно»:
        False означает, что подстановку надо помечать «проверить» (TR-2.4).
        """
        project_id = (project_id or "").strip()
        if not project_id:
            raise VaultError("не задан project_id")
        value = (value or "").strip()
        if not value:
            return None, False
        if respect_keep and self.is_kept(value):
            return None, False

        gram = morph.gram_of(value, entity_type) if suffix else None
        with self._write() as conn:
            conn.execute(
                "INSERT INTO projects(id, ts) VALUES(?, ?) ON CONFLICT(id) DO NOTHING",
                (project_id, time.time()),
            )
            entity_id, _global_n = self._ensure_entity(conn, value, entity_type)
            n = self._ensure_label(conn, project_id, entity_id, entity_type)
        case, plural = morph.parse_gram(gram)
        return label_of(entity_type, n, morph.suffix_for(case, plural)), (gram is not None)

    def project_label(
        self,
        project_id: str,
        value: str,
        entity_type: str,
        *,
        suffix: bool = True,
        respect_keep: bool = True,
    ) -> Optional[str]:
        """Метка проекта (без флага «падеж однозначен»)."""
        label, _confident = self.project_label_ex(
            project_id, value, entity_type, suffix=suffix, respect_keep=respect_keep
        )
        return label


    def _project_entity(
        self, project_id: str, entity_type: str, n: int
    ) -> Optional[int]:
        """Сущность метки проекта с учётом псевдонимов (TR-2.6)."""
        row = self._conn.execute(
            "SELECT entity_id FROM labels WHERE project_id = ? AND type = ? AND n = ?",
            (project_id, entity_type, n),
        ).fetchone()
        if row is not None:
            return int(row[0])
        row = self._conn.execute(
            "SELECT entity_id FROM aliases WHERE project_id = ? AND type = ? AND n = ?",
            (project_id, entity_type, n),
        ).fetchone()
        return int(row[0]) if row is not None else None

    def project_value(
        self, project_id: str, token: str, warnings: Optional[list[str]] = None
    ) -> Optional[str]:
        """Значение метки проекта (с падежным суффиксом, TR-2.4)."""
        parsed = parse_label(token)
        if parsed is None:
            return None
        entity_type, n, suffix = parsed
        entity_id = self._project_entity(project_id, entity_type, n)
        if entity_id is None:
            return None
        gram = morph.gram_from_suffix(suffix)
        if suffix and gram is None and warnings is not None:
            warnings.append("проверить: неизвестный падеж в метке — подставлена именительная форма")
        return self._resolve_form(entity_id, entity_type, gram, warnings)

    def mappings_for_text(
        self, project_id: str, text: str, tokens: Optional[list[str]] = None
    ) -> tuple[dict[str, str], list[str], list[str]]:
        """Подстановка меток проекта в тексте: `{метка: значение}`.

        Возвращает `(mappings, unmatched, warnings)`: `unmatched` — метки,
        которых нет в проекте (файл чужой или выпущен другой базой);
        `warnings` — пометки «проверить» (TR-2.4).
        """
        allowed: Optional[set[str]] = None
        if tokens is not None:
            allowed = set()
            for token in tokens:
                parsed = parse_label(token)
                if parsed is not None:
                    allowed.add(label_of(parsed[0], parsed[1]))
        mappings: dict[str, str] = {}
        unmatched: list[str] = []
        warnings: list[str] = []
        for match in LABEL_RE.finditer(text or ""):
            token = match.group(0)
            parsed = parse_label(token)
            if parsed is None:
                continue
            base = label_of(parsed[0], parsed[1])
            if allowed is not None and base not in allowed:
                continue
            value = self.project_value(project_id, token, warnings)
            if value is None:
                if token not in unmatched:
                    unmatched.append(token)
                continue
            mappings[token] = value
        return mappings, unmatched, warnings

    def project_mappings(self, project_id: str) -> dict[str, str]:
        """Метки проекта `{метка: именительное значение}` (панель/диагностика)."""
        out: dict[str, str] = {}
        rows = self._conn.execute(
            "SELECT entity_id, type, n FROM labels WHERE project_id = ? ORDER BY type, n",
            (project_id,),
        ).fetchall()
        for entity_id, entity_type, n in rows:
            value = self._nominal_value(int(entity_id), entity_type)
            if value is not None:
                out[label_of(entity_type, int(n))] = value
        return out


    # ── снимок для Rust (TR-1.6) ────────────────────────────────────────

    def snapshot(self, project_id: str) -> dict:
        """Снимок меток проекта: `{rev, labels, others, keep}` (TR-1.6).

        `labels` — метки проекта с формами (значения отдаются Rust-фильтру,
        он держит их только в памяти); `others` — формы всех прочих
        сущностей базы; `keep` — список D19.
        """
        project_id = (project_id or "").strip()
        labels: list[dict] = []
        project_entities: set[int] = set()
        for entity_id, entity_type, n in self._conn.execute(
            "SELECT entity_id, type, n FROM labels WHERE project_id = ? ORDER BY type, n",
            (project_id,),
        ).fetchall():
            entity_id = int(entity_id)
            project_entities.add(entity_id)
            forms = [
                {"value": value, "gram": gram, "origin": origin}
                for value, gram, origin in self._conn.execute(
                    "SELECT value, gram, origin FROM forms WHERE entity_id = ? ORDER BY id",
                    (entity_id,),
                ).fetchall()
            ]
            labels.append(
                {
                    "label": label_of(entity_type, int(n)),
                    "type": entity_type,
                    "forms": forms,
                }
            )
        others: list[str] = []
        for (entity_id,) in self._conn.execute("SELECT id FROM entities").fetchall():
            if int(entity_id) in project_entities:
                continue
            for (value,) in self._conn.execute(
                "SELECT value FROM forms WHERE entity_id = ? ORDER BY id", (entity_id,)
            ):
                others.append(value)
        return {
            "rev": self.rev(),
            "labels": labels,
            "others": others,
            "keep": self.keep_list(),
        }

    # ── «Объединить метки» и удаление проекта (TR-2.6, TR-1.7) ─────────

    def merge_entities(self, project_id: str, keep: str, drop: Iterable[str]) -> dict:
        """Склеить метки проекта: формы младших переносятся в старшую (TR-2.6).

        Младшие номера остаются псевдонимами (`aliases`) — уже выпущенные
        файлы с `[PERSON_5]` продолжают деанонимизироваться.
        """
        project_id = (project_id or "").strip()
        keep_parsed = parse_label(keep)
        if keep_parsed is None:
            raise VaultError(f"не разобрана метка: {keep!r}")
        keep_type, keep_n, _gram = keep_parsed
        keep_entity = self._project_entity(project_id, keep_type, keep_n)
        if keep_entity is None:
            raise VaultError(f"метка {keep} не принадлежит проекту {project_id}")

        merged: list[str] = []
        moved = 0
        with self._write() as conn:
            conn.execute(
                "INSERT INTO projects(id, ts) VALUES(?, ?) ON CONFLICT(id) DO NOTHING",
                (project_id, time.time()),
            )
            for token in drop:
                parsed = parse_label(token)
                if parsed is None:
                    continue
                entity_type, n, _ = parsed
                if entity_type != keep_type:
                    raise VaultError(
                        f"нельзя объединить метки разных типов: {keep} и {token}"
                    )
                if (entity_type, n) == (keep_type, keep_n):
                    continue
                entity_id = self._project_entity(project_id, entity_type, n)
                if entity_id is None or entity_id == keep_entity:
                    continue
                cursor = conn.execute(
                    "INSERT INTO forms(entity_id, value, gram, origin)"
                    " SELECT ?, value, gram, origin FROM forms WHERE entity_id = ?"
                    " ON CONFLICT(entity_id, value) DO NOTHING",
                    (keep_entity, entity_id),
                )
                if cursor.rowcount and cursor.rowcount > 0:
                    moved += cursor.rowcount
                conn.execute(
                    "DELETE FROM labels WHERE project_id = ? AND entity_id = ?",
                    (project_id, entity_id),
                )
                conn.execute(
                    "DELETE FROM aliases WHERE project_id = ? AND type = ? AND n = ?",
                    (project_id, entity_type, n),
                )
                conn.execute(
                    "INSERT INTO aliases(project_id, type, n, entity_id) VALUES(?, ?, ?, ?)",
                    (project_id, entity_type, n, keep_entity),
                )
                merged.append(label_of(entity_type, n))
        return {
            "keep": label_of(keep_type, keep_n),
            "dropped": merged,
            "forms_moved": moved,
        }

    def drop_project(self, project_id: str) -> dict:
        """Удалить проект и его метки; глобальные сущности остаются (TR-1.7)."""
        project_id = (project_id or "").strip()
        with self._write() as conn:
            labels = conn.execute(
                "DELETE FROM labels WHERE project_id = ?", (project_id,)
            ).rowcount
            counters = conn.execute(
                "DELETE FROM counters WHERE project_id = ?", (project_id,)
            ).rowcount
            aliases = conn.execute(
                "DELETE FROM aliases WHERE project_id = ?", (project_id,)
            ).rowcount
            conn.execute("DELETE FROM projects WHERE id = ?", (project_id,))
        return {
            "project_id": project_id,
            "labels": labels or 0,
            "counters": counters or 0,
            "aliases": aliases or 0,
        }
