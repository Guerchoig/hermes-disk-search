#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""M4 T5: load acceptance - N x ask_my_files through MCP /mcp (real RAG).

Harness (self-contained, no third-party deps):
  * builds an isolated corpus + sandbox index (`hds index` with HDS_ROOT);
  * starts a sandbox `llm_host` with the requested `parallel`/`ctx_per_slot`;
  * starts a sandbox `hds mcp-http` (server profile, queue ON, access ON,
    db_path -> sandbox index) and issues one token per user;
  * fires N concurrent REAL `ask_my_files` JSON-RPC calls through /mcp,
    records per-request latency/status/codes and polls the queue snapshot
    (depth/wait/per_user) plus the llm-host VRAM heartbeat;
  * writes `tools/parity/out/m4_load_<tag>.json`.

Battle `config.yaml`, `index.pause` and the battle index are NOT touched.
The battle resident must be stopped before the run (script aborts if port
8010 is busy); the caller restores it afterwards. The script only manages the
sandbox resident it starts itself.

Usage:
  python tools/parity/load5.py --tag p5_16384 --parallel 5 --ctx 16384
"""
import argparse
import hashlib
import json
import os
import secrets
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

REPO = Path(r"C:\Users\Sasha\hermes-disk-search")
HDS = REPO / "target" / "debug" / "hds.exe"
LLM = REPO / "target" / "debug" / "llm_host.exe"
OUTDIR = REPO / "tools" / "parity" / "out"
STATE = Path(os.environ.get("ProgramData", r"C:\ProgramData")) / "hermes-disk-search"
SANDBOX = Path(os.environ.get("TEMP", r"C:\Windows\Temp")) / "hds-m4-load"

USERS = ["anna", "boris", "vera", "grig", "dina"]


def log(msg):
    print("STEP " + msg, flush=True)


def http_get(url, timeout=5):
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return r.status


def wait_http(url, timeout, what):
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            if http_get(url, timeout=3) == 200:
                return True
        except Exception:
            pass
        time.sleep(0.5)
    raise RuntimeError("timeout waiting for %s (%s)" % (what, url))


def sha256_hex(s):
    return hashlib.sha256(s.encode("utf-8")).hexdigest()


def build_corpus(corpus):
    """Deterministic text corpus: enough distinct chunks for a real RAG prompt."""
    if corpus.exists():
        shutil.rmtree(corpus, ignore_errors=True)
    corpus.mkdir(parents=True, exist_ok=True)
    topics = [
        "учёт электроэнергии", "складской учёт", "договоры поставки",
        "бюджетирование", "кадровый учёт", "закупки и тендеры",
        "техподдержка клиентов", "логистика и доставка", "маркетинг и реклама",
        "аналитика продаж",
    ]
    for i in range(120):
        t = topics[i % len(topics)]
        lines = ["# Проект %02d: автоматизация (%s)" % (i, t), ""]
        for j in range(12):
            lines.append(
                "Раздел %d проекта %02d описывает %s: этапы внедрения, роли, "
                "интеграции с 1С и учётными системами, сроки, риски и KPI. "
                "Ответственный подрядчик ведёт реестр задач и смету." % (j, i, t)
            )
        (corpus / ("doc_%03d.txt" % i)).write_text("\n".join(lines), encoding="utf-8")
    log("corpus files=%d dir=%s" % (len(list(corpus.iterdir())), corpus))


LLM_CFG = """
profile: server
llm_server:
  bin: ""
  host: "127.0.0.1"
  autostart: true
  start_timeout: 300
  parallel: {parallel}
  chat:
    port: 8010
    model: "shared:chat"
    ctx_per_slot: {ctx}
    extra_args: "--cache-type-k q8_0 --cache-type-v q8_0 -ngl 99"
  embedding:
    port: 8011
    model: "shared:embedding"
    ctx_per_slot: 8192
    extra_args: "--batch-size 8192 --ubatch-size 512 -ngl 99"
  rerank:
    port: 8012
    model: "shared:rerank"
    ctx_per_slot: 8192
    extra_args: "-ngl 0"
gpu:
  policy: query_priority
  device_index: 1
  n_gpu_layers: -1
  reserve_mb: 1024
  evict_idle_sec: 600
  pause_index_on_query: true
  priorities: {{ chat: 100, transcribe: 60, embedding: 40, rerank: 30, whisper: 20 }}
llm:
  chat:
    n_batch: 512
    n_ubatch: 512
"""

IX_CFG = """
profile: desktop
db_path: '{db}'
index:
  roots:
    - '{corpus}'
  ocr: false
  transcribe: false
embedding:
  base_url: "http://127.0.0.1:8011/v1"
  model: "text-embedding-bge-m3"
  batch_size: 64
  dim: 1024
chat:
  base_url: "http://127.0.0.1:8010/v1"
  model: "chat"
chunk:
  size: 800
  overlap: 120
"""

MCP_CFG = """
profile: server
access:
  enabled: true
  token_ttl_days: 90
db_path: '{db}'
db:
  read_only_conns: true
  busy_timeout_ms: 5000
  journal_mode: wal
embedding:
  base_url: "http://127.0.0.1:8011/v1"
  model: "text-embedding-bge-m3"
  batch_size: 64
  dim: 1024
chat:
  base_url: "http://127.0.0.1:8010/v1"
  model: "chat"
  timeout: 600
  max_tokens: 512
  rag_max_tokens: 300
  max_context_chars: 24000
rerank:
  enabled: true
  url: "http://127.0.0.1:8012/v1"
  model: "bge-reranker-v2-m3"
  timeout: 30
  max_latency: 15
queue:
  enabled: true
  max_depth: 20
  wait_timeout_sec: 240
  policy: rr
rate_limit:
  enabled: true
  requests_per_min: 600
  burst: 60
  login_per_min: 30
  max_inflight: {parallel}
  max_inflight_per_user: {per_user}
http:
  max_body_kb: 512
  max_conns: 0
  read_timeout_sec: 60
"""


def write_text(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def make_tokens(state_dir):
    """access.yaml with one user+token per USERS entry; returns [(login, token)]."""
    now = int(time.time())
    tokens = []
    users_yaml = []
    tok_yaml = []
    for u in USERS:
        tok = secrets.token_hex(32)
        tokens.append((u, tok))
        users_yaml.append(
            "  - { name: %s, login: %s, admin: true, "
            "password_hash: \"$argon2id$v=19$m=19456,t=2,p=1$placeholder\" }" % (u, u)
        )
        tok_yaml.append(
            "  - { token_sha256: %s, user: %s, expires: %d }" % (sha256_hex(tok), u, now + 3600)
        )
    yml = "users:\n" + "\n".join(users_yaml) + "\ntokens:\n" + "\n".join(tok_yaml) + "\n"
    write_text(state_dir / "data" / "access.yaml", yml)
    return tokens


def start_llm(cfg, root):
    root.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, HDS_ROOT=str(root), HDS_CONFIG=str(cfg))
    f = open(str(root / "llm_host.log"), "wb")
    return subprocess.Popen([str(LLM), "run"], env=env, stdout=f,
                            stderr=subprocess.STDOUT, creationflags=0x08000000)


def stop_proc(p):
    if not p:
        return
    try:
        p.terminate()
        p.wait(timeout=20)
    except Exception:
        try:
            p.kill()
        except Exception:
            pass


def build_index(ix_cfg, ixroot, full=False):
    # HDS_ROOT points at a sandbox project root (junctions to .venv/sidecar/hds):
    # project_root() then has NO battle `index.pause`, and the extraction worker
    # is still found. The repo (and the battle pause) stay untouched.
    env = dict(os.environ, HDS_ROOT=str(ixroot), HDS_CONFIG=str(ix_cfg))
    log("index start (sandbox root=%s full=%s)" % (ixroot, full))
    cmd = [str(HDS), "index"] + (["--full"] if full else [])
    r = subprocess.run(cmd, env=env, capture_output=True, text=True,
                       errors="replace", timeout=7200)
    txt = (r.stdout or "") + (r.stderr or "")
    OUTDIR.mkdir(parents=True, exist_ok=True)
    (OUTDIR / "_m4_load_index.log").write_text(txt[-4000:], encoding="utf-8")
    log("index done rc=%d" % r.returncode)
    return r.returncode, txt


def make_ixroot(ixroot):
    """Sandbox project root: junctions to the repo's .venv, sidecar and hds pkg."""
    ixroot.mkdir(parents=True, exist_ok=True)
    for name in (".venv", "sidecar", "hds"):
        link = ixroot / name
        target = REPO / name
        if not link.exists():
            subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(target)],
                           capture_output=True, text=True)
    (ixroot / "data").mkdir(parents=True, exist_ok=True)
    log("ixroot ready=%s" % ixroot)


def post_ask(port, token, question, limit):
    body = json.dumps({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "ask_my_files", "arguments": {"question": question, "limit": limit}},
    }).encode("utf-8")
    req = urllib.request.Request(
        "http://127.0.0.1:%d/mcp" % port, data=body, method="POST",
        headers={"Content-Type": "application/json", "Authorization": "Bearer " + token},
    )
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=600) as r:
            return {"status": r.status, "ms": int((time.time() - t0) * 1000),
                    "retry_after": r.headers.get("Retry-After"),
                    "queue_depth": r.headers.get("X-Queue-Depth"),
                    "body": r.read().decode("utf-8", "replace")[:400]}
    except urllib.error.HTTPError as e:
        return {"status": e.code, "ms": int((time.time() - t0) * 1000),
                "retry_after": e.headers.get("Retry-After"),
                "queue_depth": e.headers.get("X-Queue-Depth"),
                "body": e.read().decode("utf-8", "replace")[:400]}


def read_snapshot():
    try:
        return json.loads((STATE / "data" / "mcp-queue.json").read_text(encoding="utf-8"))
    except Exception:
        return None


def read_heartbeat(root):
    try:
        return json.loads((root / "data" / "llm-host.heartbeat.json").read_text(encoding="utf-8"))
    except Exception:
        return None


def percentile(vals, p):
    if not vals:
        return 0
    s = sorted(vals)
    return s[int(round((len(s) - 1) * p))]


def start_fake_openai(port, latency):
    """M5 T4: built-in fake OpenAI-compatible chat server (provider: remote).

    Answers POST /v1/chat/completions after `latency` seconds with a short
    OpenAI-shaped reply. Counts requests and whether Authorization arrived.
    """
    state = {"requests": 0, "auth_ok": 0, "last_status": None}

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            n = int(self.headers.get("Content-Length", 0) or 0)
            self.rfile.read(n)
            state["requests"] += 1
            auth = self.headers.get("Authorization", "")
            if auth.startswith("Bearer sk-"):
                state["auth_ok"] += 1
            state["last_status"] = 200
            time.sleep(latency)
            content = (
                "Проекты автоматизации описаны в doc_005 и doc_006: этапы внедрения, "
                "роли, интеграции с 1С и учётными системами, риски и KPI. [1] [2]"
            )
            reply = json.dumps(
                {"choices": [{"message": {"content": content}}]}
            ).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(reply)))
            self.end_headers()
            self.wfile.write(reply)

        def log_message(self, *a):  # silence per-request stderr noise
            pass

    srv = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    srv.daemon_threads = True
    th = threading.Thread(target=srv.serve_forever, daemon=True)
    th.start()
    log("fake openai up port=%d latency=%.1fs" % (port, latency))
    return srv, state


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", required=True)
    ap.add_argument("--parallel", type=int, required=True)
    ap.add_argument("--ctx", type=int, default=16384)
    ap.add_argument("--clients", type=int, default=5)
    ap.add_argument("--per-user", type=int, default=None)
    ap.add_argument("--mcp-port", type=int, default=8797)
    ap.add_argument("--limit", type=int, default=8)
    ap.add_argument("--question", default="какие проекты автоматизации есть и что в них делается")
    ap.add_argument("--index-during", action="store_true",
                    help="start `hds index --full` in the sandbox concurrently with the load")
    ap.add_argument("--no-rerank", action="store_true",
                    help="disable the reranker in the MCP config (isolate role-load thrash)")
    ap.add_argument("--chat-remote", default=None, metavar="fake|URL",
                    help="point chat at an EXTERNAL provider (chat.provider: remote); "
                         "'fake' starts a built-in fake OpenAI server on --remote-port")
    ap.add_argument("--remote-port", type=int, default=8799)
    ap.add_argument("--remote-latency", type=float, default=2.0,
                    help="fake remote reply latency, seconds")
    args = ap.parse_args()

    per_user = args.per_user if args.per_user is not None else max(2, args.clients)
    OUTDIR.mkdir(parents=True, exist_ok=True)
    sand = SANDBOX / args.tag
    ixroot = sand / "ixroot"       # sandbox project root (HDS_ROOT)
    # The resident and the indexer share ONE project root, so that
    # `gpu.pause_index_on_query` (file index.pause) is visible to the indexer —
    # exactly like a real deployment. The repo/battle pause is never touched.
    root = ixroot
    corpus = sand / "corpus"
    db = sand / "index.db"
    if sand.exists():
        shutil.rmtree(sand, ignore_errors=True)
    sand.mkdir(parents=True, exist_ok=True)
    log("sandbox=%s" % sand)

    # A running battle resident would hold 8010-8012.
    try:
        http_get("http://127.0.0.1:8010/health", timeout=2)
        raise RuntimeError("port 8010 busy - stop the battle resident first (llm_host stop)")
    except urllib.error.URLError:
        pass

    result = {"tag": args.tag, "parallel": args.parallel, "ctx_per_slot": args.ctx,
              "clients": args.clients, "max_inflight_per_user": per_user,
              "question": args.question, "sandbox": str(sand)}

    # M5 T4: external chat provider (chat.provider: remote). 'fake' -> built-in
    # fake OpenAI server; any other value -> used as chat.base_url as-is.
    fake = None
    fake_state = None
    remote_url = None
    if args.chat_remote:
        if args.chat_remote == "fake":
            fake, fake_state = start_fake_openai(args.remote_port, args.remote_latency)
            remote_url = "http://127.0.0.1:%d/v1" % args.remote_port
        else:
            remote_url = args.chat_remote
        result["chat_remote"] = remote_url

    build_corpus(corpus)
    make_ixroot(ixroot)

    llm_cfg = sand / "llm_config.yaml"
    llm_text = LLM_CFG.format(parallel=args.parallel, ctx=args.ctx)
    if remote_url:
        # llm-host must NOT keep the chat role in VRAM (M5 §3.2).
        llm_text += "\nchat:\n  provider: remote\n"
    write_text(llm_cfg, llm_text)
    ix_cfg = sand / "index_config.yaml"
    write_text(ix_cfg, IX_CFG.format(db=db, corpus=corpus))
    # the sidecar worker loads <root>/config.yaml (root = sandbox project root)
    write_text(ixroot / "config.yaml", IX_CFG.format(db=db, corpus=corpus))
    mcp_cfg = sand / "mcp_config.yaml"
    mcp_text = MCP_CFG.format(db=db, parallel=args.parallel, per_user=per_user)
    if args.no_rerank:
        mcp_text = mcp_text.replace("rerank:\n  enabled: true", "rerank:\n  enabled: false")
    if remote_url:
        mcp_text = mcp_text.replace(
            'chat:\n  base_url: "http://127.0.0.1:8010/v1"\n  model: "chat"',
            'chat:\n  provider: remote\n  base_url: "%s"\n  api_key: "sk-fake-remote"\n  model: "fake-model"' % remote_url,
        )
    write_text(mcp_cfg, mcp_text)

    snap_before = (STATE / "data" / "mcp-queue.json").exists()
    acc_existed = (STATE / "data" / "access.yaml").exists()
    hb_before = (STATE / "data" / "index.heartbeat.json").exists()
    state_existed = STATE.exists()

    llm = None
    mcp = None
    try:
        llm = start_llm(llm_cfg, root)
        if remote_url:
            # chat role is NOT planned (remote): wait for the embedding facade.
            wait_http("http://127.0.0.1:8011/health", 420, "sandbox llm_host embedding")
            log("llm_host up (chat remote -> %s)" % remote_url)
        else:
            wait_http("http://127.0.0.1:8010/health", 420, "sandbox llm_host chat")
            props = json.loads(urllib.request.urlopen("http://127.0.0.1:8010/props", timeout=5).read())
            result["total_slots"] = props.get("total_slots")
            result["n_ctx_props"] = (props.get("default_generation_settings") or {}).get("n_ctx")
            log("llm_host up slots=%s n_ctx=%s" % (result["total_slots"], result["n_ctx_props"]))

        rc, _ = build_index(ix_cfg, ixroot)
        if rc != 0:
            raise RuntimeError("hds index failed rc=%d" % rc)

        tokens = make_tokens(STATE)
        log("tokens issued for %s" % ", ".join(u for u, _ in tokens))

        env = dict(os.environ, HDS_CONFIG=str(mcp_cfg))
        env.pop("HDS_ROOT", None)
        mcp = subprocess.Popen(
            [str(HDS), "mcp", "--http", "--host", "127.0.0.1", "--port", str(args.mcp_port),
             "--path", "/mcp"],
            env=env, creationflags=0x08000000,
            stdout=open(str(sand / "mcp.log"), "wb"), stderr=subprocess.STDOUT)
        wait_http("http://127.0.0.1:%d/health" % args.mcp_port, 90, "sandbox mcp")
        log("mcp up port=%d" % args.mcp_port)

        polls = []
        stop_flag = {"stop": False}

        def poller():
            while not stop_flag["stop"]:
                s = read_snapshot()
                if s:
                    polls.append({"t": round(time.time(), 1), "depth": s.get("depth"),
                                  "executing": s.get("executing"),
                                  "granted": s.get("granted"),
                                  "rejected_user": s.get("rejected_user"),
                                  "rejected_full": s.get("rejected_full"),
                                  "timeouts": s.get("timeouts"),
                                  "wait_p95_ms": s.get("wait_p95_ms"),
                                  "per_user": s.get("per_user")})
                time.sleep(0.5)

        th = threading.Thread(target=poller, daemon=True)
        th.start()

        hb0 = read_heartbeat(root)
        result["vram_before"] = (hb0 or {}).get("vram_ours_mib")
        idx_res = {}

        def idx_job():
            idx_res["rc"], idx_res["out"] = build_index(ix_cfg, ixroot, full=True)

        idx_thread = None
        if args.index_during:
            idx_thread = threading.Thread(target=idx_job, daemon=True)
            idx_thread.start()
            log("index --full started concurrently with the load")

        wall0 = time.time()
        with ThreadPoolExecutor(max_workers=args.clients) as ex:
            futs = [ex.submit(post_ask, args.mcp_port, tok, args.question, args.limit)
                    for _, tok in tokens[:args.clients]]
            responses = [f.result() for f in futs]
        result["wall_sec"] = round(time.time() - wall0, 1)
        if idx_thread is not None:
            idx_thread.join(timeout=1800)
            out = idx_res.get("out") or ""
            result["index_during"] = {"rc": idx_res.get("rc"),
                                      "locked": "database is locked" in out.lower(),
                                      "tail": out[-600:]}
        stop_flag["stop"] = True
        th.join(timeout=3)
        time.sleep(3)
        hb1 = read_heartbeat(root)
        result["vram_after"] = (hb1 or {}).get("vram_ours_mib")
        result["snap_after"] = read_snapshot()
        result["polls"] = polls
        result["responses"] = responses
        if fake_state is not None:
            result["remote"] = dict(fake_state)
        oks = [r["ms"] for r in responses if r["status"] == 200]
        result["latencies_ms"] = oks
        result["statuses"] = [r["status"] for r in responses]
        result["p50_ms"] = percentile(oks, 0.50)
        result["p95_ms"] = percentile(oks, 0.95)
        result["sla_ok"] = bool(oks) and result["p95_ms"] <= 300000
    finally:
        stop_proc(mcp)
        if llm is not None:
            try:
                env = dict(os.environ, HDS_ROOT=str(root), HDS_CONFIG=str(llm_cfg))
                subprocess.run([str(LLM), "stop"], env=env, timeout=30, capture_output=True)
            except Exception:
                pass
            stop_proc(llm)
        if fake is not None:
            fake.shutdown()
        if not snap_before:
            try:
                (STATE / "data" / "mcp-queue.json").unlink()
            except Exception:
                pass
        if not acc_existed:
            try:
                (STATE / "data" / "access.yaml").unlink()
            except Exception:
                pass
        if not hb_before:
            try:
                (STATE / "data" / "index.heartbeat.json").unlink()
            except Exception:
                pass
        if not state_existed:
            # the whole system state dir was created by this probe - drop it
            # (its data/logs/audit.jsonl would carry the test users otherwise)
            shutil.rmtree(STATE, ignore_errors=True)

    out = OUTDIR / ("m4_load_%s.json" % args.tag)
    out.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    log("WROTE %s" % out)
    log("statuses=%s p50=%sms p95=%sms wall=%ss" % (
        result.get("statuses"), result.get("p50_ms"), result.get("p95_ms"), result.get("wall_sec")))


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print("FATAL: %s" % e, file=sys.stderr)
        sys.exit(1)
