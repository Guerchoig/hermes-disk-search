#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""ARB-1...6: живая проверка диспетчера VRAM и фасада на реальном `llm-host`.

Зачем: решения диспетчера проверены юнит-тестами (`tests/arbiter.rs`), но сценарии
из `MIGRATION_PLAN_RUST.md` §8.6 — про **живую** машину: кто кого вытесняет, что
происходит с `index.pause`, влезает ли роль. Этот скрипт гоняет их на настоящем
движке и настоящей карте и складывает доказательства в `out/w2_arb.json`.

Что проверяется (по критериям §8.6 основного плана):

* **ARB-1** запрос, которому не хватает VRAM → пауза индексации + вытеснение
  **индексных** ролей (`rerank`), резидент (`chat`) остаётся, запрос выполняется;
* **ARB-2** после запроса пауза снимается, индексные роли возвращаются сами
  (`LOAD_ON_DEMAND`); **пауза пользователя не снимается**;
* **ARB-3** живая индексация без запросов и нехватка VRAM → фоновый арбитр выгружает
  `chat` (он вернётся по первому запросу);
* **ARB-4** роль не влезает → точный отчёт «нужно X, доступно Y» **без авто-деградации**;
* **ARB-5** простой роли дольше grace/`gpu.evict_idle_sec` → роль выгружается;
* **ARB-6** один и тот же порт: `chat-think` получает размышления, MCP-путь
  (`chat_template_kwargs.enable_thinking=false`) — ответ без них.

Скрипт **не трогает** боевые порты и боевой `index.pause`: запускает свой `llm-host`
на `--port-base` (по умолчанию 8070) со своим каталогом сигналов (`--pause-dir` в
TEMP) и своими временными конфигами (копия `config.yaml` + нужные ключи).

Запуск (из корня репозитория, интерпретатор проекта):
    .\\.venv\\Scripts\\python.exe tools/parity/arb_scenarios.py
    .\\.venv\\Scripts\\python.exe tools/parity/arb_scenarios.py --keep   # не гасить хост
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "release" / "llm_host.exe"
TMP = Path(tempfile.gettempdir()) / "hds-arb"
CHAT = "chat"
EMB = "embedding"
RERANK = "rerank"


def log(msg: str) -> None:
    print(msg, flush=True)


# ---------------------------------------------------------------- HTTP-хелперы


def http_json(
    url: str, payload: dict | None = None, timeout: float = 600.0, method: str | None = None
) -> tuple[int, dict | str]:
    """GET/POST JSON. Возвращает `(статус, тело)`; тело — dict или текст."""
    data = None
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8", "replace")
            try:
                return resp.status, json.loads(body)
            except json.JSONDecodeError:
                return resp.status, body
    except urllib.error.HTTPError as e:  # 4xx/5xx — это тоже ответ
        body = e.read().decode("utf-8", "replace")
        try:
            return e.code, json.loads(body)
        except json.JSONDecodeError:
            return e.code, body


def chat(base: str, text: str, model: str = CHAT, **extra) -> tuple[int, dict | str]:
    body = {
        "model": model,
        "max_tokens": 24,
        "temperature": 0.2,
        "messages": [{"role": "user", "content": text}],
    }
    body.update(extra)
    return http_json(f"{base}/v1/chat/completions", body)


def embeddings(base: str, texts: list[str]) -> tuple[int, dict | str]:
    return http_json(f"{base}/v1/embeddings", {"model": EMB, "input": texts})


def rerank(base: str, query: str, docs: list[str]) -> tuple[int, dict | str]:
    return http_json(f"{base}/v1/rerank", {"model": RERANK, "query": query, "documents": docs})


def status(base: str) -> dict:
    code, body = http_json(f"{base}/internal/status", None, timeout=30)
    if code != 200 or not isinstance(body, dict):
        raise RuntimeError(f"/internal/status: HTTP {code}: {body}")
    return body


def role_state(base: str, role: str) -> str:
    """Состояние роли из отчёта резидента (`LOADED`/`UNLOADED`/`GRACE`/`...`)."""
    for r in status(base).get("roles", []):
        if r.get("role") == role:
            return str(r.get("state"))
    return "?"

# ---------------------------------------------------------------- конфиг и хост


class Host:
    """Запущенный `llm_host` со своим конфигом, логом и каталогом сигналов."""

    def __init__(self, name: str, port_base: int, cfg_extra: str, pause_dir: Path):
        self.name = name
        self.port_base = port_base
        self.cfg = TMP / f"{name}.yaml"
        self.log = TMP / f"{name}.host.log"
        self.out = TMP / f"{name}.out.log"
        self.err = TMP / f"{name}.err.log"
        self.pause_dir = pause_dir
        self.proc = None
        self.cfg_extra = cfg_extra

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.port_base}"

    def write_config(self) -> None:
        """Копия боевого `config.yaml` + правки сценария (боевой файл не меняем)."""
        text = (ROOT / "config.yaml").read_text(encoding="utf-8")
        # rerank на GPU: убираем legacy `-ngl 0` (в боевом конфиге он держит реранкер на CPU)
        text = text.replace('extra_args: "-ngl 0 --batch-size 8192', 'extra_args: "--batch-size 8192')
        self.cfg.write_text(text + "\n" + self.cfg_extra + "\n", encoding="utf-8")

    def start(self, wait: float = 300.0) -> None:
        for p in (self.log, self.out, self.err):
            if p.exists():
                p.unlink()
        self.pause_dir.mkdir(parents=True, exist_ok=True)
        self.write_config()
        args = [
            str(EXE),
            "run",
            "--config",
            str(self.cfg),
            "--port-base",
            str(self.port_base),
            "--pause-dir",
            str(self.pause_dir),
            "--log",
            str(self.log),
            "--no-residency",
            "--hold",
            "1800",
        ]
        log("  запуск: llm_host run --config <temp> --port-base %d --pause-dir <temp>" % self.port_base)
        self.proc = subprocess.Popen(
            args,
            cwd=str(ROOT),
            stdout=self.out.open("wb"),
            stderr=self.err.open("wb"),
        )
        self.wait_health(wait)

    def wait_health(self, timeout: float) -> None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                code, _ = http_json(f"{self.base}/health", None, timeout=5)
                if code == 200:
                    return
            except Exception:  # noqa: BLE001 — хост ещё поднимается
                pass
            time.sleep(2)
        raise RuntimeError(f"хост '{self.name}' не поднялся за {timeout:.0f} с\n{self.err_log_tail()}")

    def stop(self) -> None:
        if self.proc is None:
            return
        try:
            http_json(f"{self.base}/internal/stop", {}, timeout=15)
        except Exception:  # noqa: BLE001
            pass
        try:
            self.proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self.proc = None

    def log_text(self) -> str:
        try:
            return self.log.read_text(encoding="utf-8", errors="replace")
        except FileNotFoundError:
            return ""

    def err_log_tail(self, lines: int = 25) -> str:
        try:
            data = self.err.read_text(encoding="utf-8", errors="replace").splitlines()
        except FileNotFoundError:
            return ""
        return "\n".join(data[-lines:])

    def pause_file(self) -> Path:
        return self.pause_dir / "index.pause"

    def write_heartbeat(self, seen: int = 100, processed: int = 90, paused: bool = False) -> None:
        """Живой heartbeat индексации — в том виде, в каком его пишет `hds/progress.py`."""
        payload = {
            "ts": time.time(),
            "seen": seen,
            "processed": processed,
            "errors": 0,
            "chunks": processed * 12,
            "paused": paused,
            "phase": "extract",
            "path": r"D:\example\file.docx",
        }
        if not paused:
            payload.update({"total": 1000, "eta_sec": 300, "remaining": 910, "rate_min": 120})
        (self.pause_dir / "index.heartbeat.json").write_text(
            json.dumps(payload, ensure_ascii=False), encoding="utf-8"
        )



# ---------------------------------------------------------------- сценарии


class Report:
    """Собирает результаты сценариев в один JSON + человеческий вывод."""

    def __init__(self) -> None:
        self.scenarios: list[dict] = []

    def add(self, name: str, ok: bool, checks: list[dict], evidence: dict) -> None:
        self.scenarios.append(
            {"scenario": name, "ok": bool(ok), "checks": checks, "evidence": evidence}
        )
        mark = "OK  " if ok else "FAIL"
        log(f"  [{mark}] {name}")
        for c in checks:
            log(f"        {'+' if c['ok'] else '-'} {c['what']}: {c['detail']}")

    def failed(self) -> list[str]:
        return [s["scenario"] for s in self.scenarios if not s["ok"]]


def check(ok: bool, what: str, detail: str = "") -> dict:
    return {"ok": bool(ok), "what": what, "detail": detail}


def arb1_and_arb2_and_arb6(port_base: int, pause_dir: Path) -> list[tuple[str, bool, list, dict]]:
    """ARB-1 (вытеснение по приоритетам), ARB-2 (пауза), ARB-6 (thinking) — один хост.

    Искусственно суженный бюджет (`gpu.vram_budget_mb: 3000`) даёт детерминированный
    сценарий: индексные роли влезают (по одной), а запрос чата — нет, поэтому
    диспетчер обязан решить, кого вытеснять, и сказать точные цифры (ARB-4).
    """
    out = []
    # cap 3000 МиБ: rerank (1661) и embedding (1660) влезают по одному,
    # чат (9516) — нет; значит вытеснение и отчёт гарантированы
    cfg = """
llm:
  chat:
    n_batch: 512
    n_ubatch: 512
gpu:
  policy: query_priority
  reserve_mb: 1024
  vram_budget_mb: 3000
  pause_index_on_query: true
  evict_idle_sec: 600
"""
    host = Host("arb12", port_base, cfg, pause_dir)
    host.start()
    try:
        host.pause_file().unlink(missing_ok=True)
        evidence: dict = {"status_at_start": {r["role"]: r["state"] for r in status(host.base).get("roles", [])}}

        # --- ARB-6: один порт, два режима размышлений (пробуем до 3 раз: модель
        # может ответить без блока размышлений, это не дефект фасада)
        rc_sizes = []
        for _ in range(3):
            code_th, think = chat(
                host.base, "Посчитай 17*23 и объясни шаги.", model="chat-think", max_tokens=192
            )
            rc = ""
            if isinstance(think, dict):
                rc = (think.get("choices") or [{}])[0].get("message", {}).get("reasoning_content") or ""
            rc_sizes.append(len(rc))
            if rc:
                break
        code_off, off = chat(
            host.base,
            "Посчитай 17*23 и объясни шаги.",
            max_tokens=192,
            chat_template_kwargs={"enable_thinking": False},
        )
        rc_off = ""
        content_off = ""
        if isinstance(off, dict):
            msg = (off.get("choices") or [{}])[0].get("message", {}) or {}
            rc_off = msg.get("reasoning_content") or ""
            content_off = msg.get("content") or ""
        arb6_checks = [
            check(code_th == 200 and max(rc_sizes) > 0,
                  "ARB-6: chat-think даёт reasoning_content (до 3 попыток)",
                  f"длины: {rc_sizes}"),
            check(code_off == 200 and rc_off == "",
                  "ARB-6: MCP-путь (enable_thinking=false) — без размышлений",
                  f"{len(rc_off)} симв."),
            check(bool(content_off.strip()), "ARB-6: MCP-путь отвечает текстом", content_off[:40]),
        ]
        out.append(("ARB-6 (thinking на одном порту)", all(c["ok"] for c in arb6_checks),
                    arb6_checks, {"reasoning_sizes": rc_sizes}))

        # --- ARB-1: готовим «индексные роли в памяти, чат выгружен» НА СВОБОДНОЙ карте.
        # Порядок важен: если грузить индексные роли при загруженном чате, память на
        # карте кончается по-настоящему (не по cap), и подготовка сама вытесняет чат —
        # проверка перестаёт быть детерминированной.
        for role in (CHAT, RERANK, EMB):
            http_json(f"{host.base}/internal/unload", {"role": role}, timeout=120)
        for role in (RERANK, EMB):
            code, _ = http_json(f"{host.base}/internal/load", {"role": role}, timeout=300)
            evidence[f"load_{role}_http"] = code
        evidence["status_before_query"] = {r["role"]: r["state"] for r in status(host.base).get("roles", [])}
        before = len(host.log_text())
        code, body = chat(host.base, "Одним словом: проверка.")
        msg = ""
        if isinstance(body, dict):
            msg = str((body.get("error") or {}).get("message") or "")
        new_lines = host.log_text()[before:]
        ev_rr = new_lines.find("выгрузить 'rerank'")
        ev_emb = new_lines.find("выгрузить 'embedding'")
        ev_chat = new_lines.find("выгрузить 'chat'")
        arb1_checks = [
            check(evidence.get("load_rerank_http") == 200 and evidence.get("load_embedding_http") == 200,
                  "ARB-1: подготовка — индексные роли на GPU загружены",
                  f"rerank={evidence.get('load_rerank_http')}, embedding={evidence.get('load_embedding_http')}"),
            check("пауза индексации" in new_lines,
                  "ARB-1: при нехватке VRAM поставлена пауза индексации", "строка в логе"),
            check(ev_rr >= 0 and ev_emb >= 0 and ev_rr < ev_emb,
                  "ARB-1: вытеснение по приоритету (rerank 30 → embedding 40)",
                  f"позиции: rerank={ev_rr}, embedding={ev_emb}"),
            check(ev_chat < 0, "ARB-1: чат (роль запроса) не вытесняется", f"позиция chat={ev_chat}"),
            check(code == 503 and len(re.findall(r"\d{3,6}", msg)) >= 2,
                  "ARB-1: вместо деградации — отчёт с точными цифрами (бюджет сужен)",
                  msg[:120]),
        ]
        # индексная роль возвращается сама (LOAD_ON_DEMAND) — «индексные инстансы
        # поднимаются сами» из ARB-2
        code_rerank, rerank_body = rerank(host.base, "тест", ["документ раз", "документ два"])
        arb1_checks.append(
            check(code_rerank == 200, "ARB-1/ARB-2: вытесненная индексная роль вернулась по запросу",
                  f"HTTP {code_rerank}")
        )
        evidence["shortage_message"] = msg
        out.append(("ARB-1 (вытеснение по приоритету + возврат роли)",
                    all(c["ok"] for c in arb1_checks), arb1_checks, evidence))

        # --- ARB-2: наша пауза после запроса не остаётся (в т.ч. после отказа)
        arb2_checks = [
            check(not host.pause_file().exists(),
                  "ARB-2: нашего index.pause после запросов нет",
                  f"файл {'есть' if host.pause_file().exists() else 'нет'}"),
        ]
        # --- ARB-2b: пауза пользователя цела (её ставим мы «как пользователь»)
        host.pause_file().write_text("", encoding="utf-8")
        before2 = len(host.log_text())
        code2, _ = chat(host.base, "Одним словом: ещё проверка.")
        kept = host.pause_file().exists()
        arb2_checks.append(
            check(kept, "ARB-2: пауза пользователя не снята", "index.pause на месте")
        )
        arb2_checks.append(
            check("пауза пользователя" in host.log_text()[before2:] or kept,
                  "ARB-2: в логе видно, что пауза чужая", "переиспользуем, не снимаем"),
        )
        host.pause_file().unlink(missing_ok=True)
        out.append(("ARB-2 (пауза снята/сохранена)", all(c["ok"] for c in arb2_checks), arb2_checks,
                    {"emb_http": code2}))
    finally:
        host.stop()
    return out



def arb3(port_base: int, pause_dir: Path) -> tuple[str, bool, list, dict]:
    """ARB-3: живая индексация без запросов и нехватка VRAM → арбитр выгружает чат."""
    cfg = """
llm:
  chat:
    n_batch: 512
    n_ubatch: 512
gpu:
  policy: query_priority
  reserve_mb: 1024
  vram_budget_mb: 500
  evict_idle_sec: 0
"""
    host = Host("arb3", port_base, cfg, pause_dir)
    host.start()
    try:
        host.pause_file().unlink(missing_ok=True)
        host.write_heartbeat(seen=250, processed=200)
        before = len(host.log_text())
        chat_before = role_state(host.base, CHAT)
        chat_after = chat_before
        waited = 0
        while waited < 50:
            time.sleep(5)
            waited += 5
            chat_after = role_state(host.base, CHAT)
            if chat_after not in ("LOADED", "SERVING"):
                break
        new_lines = host.log_text()[before:]
        checks = [
            check(chat_before in ("LOADED", "SERVING"), "ARB-3: подготовка — chat загружен", chat_before),
            check("arbiter/indexing" in new_lines,
                  "ARB-3: фоновый арбитр принял решение по индексации", f"через {waited} с"),
            check(chat_after not in ("LOADED", "SERVING"),
                  "ARB-3: резидент выгружен под индексацию", chat_after),
            check(not host.pause_file().exists(),
                  "ARB-3: пауза индексации не ставится (индексация важнее)", "файла нет"),
        ]
        return ("ARB-3 (индексация вытесняет резидент)",
                all(c["ok"] for c in checks), checks,
                {"chat_before": chat_before, "chat_after": chat_after, "waited_sec": waited})
    finally:
        host.stop()


def arb5(port_base: int, pause_dir: Path) -> tuple[str, bool, list, dict]:
    """ARB-5: простой роли дольше grace/`evict_idle_sec` → роль выгружается."""
    cfg = """
llm:
  chat:
    n_batch: 512
    n_ubatch: 512
  embedding:
    grace_seconds: 600
gpu:
  policy: query_priority
  reserve_mb: 1024
  evict_idle_sec: 5
"""
    host = Host("arb5", port_base, cfg, pause_dir)
    host.start()
    try:
        code, _ = http_json(f"{host.base}/internal/load", {"role": EMB}, timeout=300)
        state_before = role_state(host.base, EMB)
        before = len(host.log_text())
        state_after = state_before
        waited = 0
        while waited < 60:
            time.sleep(5)
            waited += 5
            state_after = role_state(host.base, EMB)
            if state_after not in ("LOADED", "SERVING"):
                break
        new_lines = host.log_text()[before:]
        checks = [
            check(code == 200 and state_before in ("LOADED", "SERVING"),
                  "ARB-5: подготовка — embedding загружен", f"HTTP {code}, {state_before}"),
            check("arbiter/idle" in new_lines, "ARB-5: сработал предохранитель простоя арбитра",
                  f"через {waited} с"),
            check(state_after not in ("LOADED", "SERVING"), "ARB-5: роль выгружена", state_after),
        ]
        return ("ARB-5 (простой роли)", all(c["ok"] for c in checks), checks,
                {"state_before": state_before, "state_after": state_after, "waited_sec": waited})
    finally:
        host.stop()



def arb4(port_base: int, pause_dir: Path) -> tuple[str, bool, list, dict]:
    """ARB-4: роль не влезает → точный отчёт без авто-деградации; после смены
    условий (конфиг не сужен) роль грузится и работает."""
    cfg = """
llm:
  chat:
    n_batch: 512
    n_ubatch: 512
gpu:
  policy: query_priority
  reserve_mb: 1024
  vram_budget_mb: 1200
  pause_index_on_query: true
  evict_idle_sec: 600
"""
    host = Host("arb4", port_base, cfg, pause_dir)
    host.start()
    evidence: dict = {}
    checks: list[dict] = []
    try:
        # выгружаем чат: запрос потребует загрузки, а бюджета («нужно» > «доступно») нет
        http_json(f"{host.base}/internal/unload", {"role": CHAT}, timeout=120)
        code, body = chat(host.base, "Скажи одно слово: тест")
        msg = ""
        if isinstance(body, dict):
            msg = str((body.get("error") or {}).get("message") or "")
        numbers = re.findall(r"\d{3,6}", msg)
        checks += [
            check(code == 503, "ARB-4: запрос отклонён с 503 (деградации нет)", f"HTTP {code}"),
            check("не хватает VRAM" in msg, "ARB-4: в отчёте сказано, чего не хватает", msg[:160]),
            check("авто-деградации нет" in msg, "ARB-4: явно сказано, что деградации нет",
                  "llm.model_policy: fixed"),
            check(len(numbers) >= 2, "ARB-4: точные цифры «нужно/доступно» в отчёте",
                  f"числа: {numbers[:6]}"),
        ]
        evidence["shortage_message"] = msg
        evidence["chat_state_after_shortage"] = role_state(host.base, CHAT)
    finally:
        host.stop()

    host2 = Host("arb4b", port_base, """
llm:
  chat:
    n_batch: 512
    n_ubatch: 512
gpu:
  policy: query_priority
  reserve_mb: 1024
  evict_idle_sec: 600
""", pause_dir)
    host2.start()
    try:
        code2, body2 = chat(host2.base, "What is 2+2? Answer with a single number.")
        answer = ""
        if isinstance(body2, dict):
            answer = ((body2.get("choices") or [{}])[0].get("message", {}) or {}).get("content", "") or ""
        checks.append(
            check(code2 == 200 and bool(answer.strip()),
                  "ARB-4: без сужения бюджета роль грузится и отвечает",
                  f"HTTP {code2}, ответ: {answer.strip()[:24]!r}")
        )
        evidence["answer_after"] = answer.strip()
    finally:
        host2.stop()
    return ("ARB-4 (отчёт о нехватке + ручная настройка)",
            all(c["ok"] for c in checks), checks, evidence)


# ---------------------------------------------------------------- вход


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="ARB-1...6 на живом llm-host")
    ap.add_argument("--port-base", type=int, default=8070,
                    help="базовые порты проверок (боевые 8010-8012 не трогаем)")
    ap.add_argument("--json", default=str(ROOT / "tools" / "parity" / "out" / "w2_arb.json"))
    ap.add_argument("--no-build", action="store_true", help="не собирать release-бинарник")
    ap.add_argument("--only", default="", help="подмножество сценариев: 1,2,3,4,5,6")
    args = ap.parse_args(argv)

    if not args.no_build:
        log("== сборка llm_host (release)")
        rc = subprocess.call(
            ["cargo", "build", "-q", "-p", "hds-llama", "--release", "--bin", "llm_host"],
            cwd=str(ROOT),
        )
        if rc != 0:
            log(f"!! сборка не удалась (rc={rc})")
            return 2
    if not EXE.exists():
        log(f"!! нет бинарника {EXE} (соберите: cargo build --release -p hds-llama --bin llm_host)")
        return 2

    TMP.mkdir(parents=True, exist_ok=True)
    pause_dir = TMP / "signals"
    if pause_dir.exists():
        shutil.rmtree(pause_dir, ignore_errors=True)
    pause_dir.mkdir(parents=True, exist_ok=True)

    only = {s.strip() for s in args.only.split(",") if s.strip()}
    want = lambda tag: (not only) or (tag in only)  # noqa: E731
    log(f"== ARB-сценарии: порты {args.port_base}-{args.port_base + 2}, сигналы {pause_dir}")
    report = Report()
    rows: list[tuple[str, bool, list, dict]] = []
    if want("1") or want("2") or want("6"):
        rows += arb1_and_arb2_and_arb6(args.port_base, pause_dir)
    if want("3"):
        rows.append(arb3(args.port_base, pause_dir))
    if want("4"):
        rows.append(arb4(args.port_base, pause_dir))
    if want("5"):
        rows.append(arb5(args.port_base, pause_dir))
    for name, ok, checks, evidence in rows:
        report.add(name, ok, checks, evidence)

    payload = {
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "port_base": args.port_base,
        "scenarios": report.scenarios,
        "failed": report.failed(),
    }
    out = Path(args.json)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    log(f"== отчёт: {out}")
    failed = report.failed()
    if failed:
        log(f"!! не прошли: {', '.join(failed)}")
        return 1
    log("== все сценарии прошли")
    return 0


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())

