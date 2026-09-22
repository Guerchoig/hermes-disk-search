"""Фаза A2: что происходит с текстом длиннее контекста модели.

1) LM Studio (ctx=8192): эмбеддинг очень длинного текста и его префиксов —
   если LM Studio молча режет вход, косинус(полный, префикс) упрётся в 1.0
   на границе контекста. Считаем границу в символах и токенах bge-m3.
2) Сырой llama.cpp (--ctx-size 512): показывает явную ошибку вместо
   молчаливого усечения — фиксируем код и текст ответа.
"""
import math
import os
import sqlite3
import subprocess
import sys
import time

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
sys.stderr.reconfigure(encoding="utf-8", errors="replace")

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import requests  # noqa: E402

from hds.config import db_abs_path, load  # noqa: E402

PORT = 8898
GGUF = os.path.join(os.path.expanduser("~"), ".lmstudio", "models", "lm-kit",
                    "bge-m3-gguf", "bge-m3-Q8_0.gguf")
BACKEND = os.path.join(os.path.expanduser("~"), ".lmstudio", "extensions",
                       "backends", "llama.cpp-win-x86_64-avx2-2.24.0")


def cos(a, b):
    s = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(y * y for y in b))
    return s / (na * nb) if na and nb else 0.0


def main():
    cfg = load()
    db = db_abs_path(cfg)
    conn = sqlite3.connect("file:%s?mode=ro" % db.replace("\\", "/"), uri=True)
    many = [r[0] for r in conn.execute(
        "SELECT text FROM chunks ORDER BY RANDOM() LIMIT 30").fetchall()]
    conn.close()
    long_text = "\n".join(many)
    print("[A2] длинный текст: %d символов" % len(long_text))

    from transformers import AutoTokenizer
    tok = AutoTokenizer.from_pretrained("BAAI/bge-m3")
    print("[A2] токенов в тексте: %d" % len(tok.encode(long_text)))

    only2 = len(sys.argv) > 1 and sys.argv[1] == "llama512"

    # --- 1) LM Studio: молчаливое усечение? ---
    if not only2:
        from hds.embedder import make_embedder

        emb = make_embedder(cfg)
        steps = list(range(4000, len(long_text) + 1, 4000))
        if steps[-1] != len(long_text):
            steps.append(len(long_text))
        texts = [long_text[:n] for n in steps[:-1]] + [long_text]
        vecs = emb.embed(texts)
        full = vecs[-1]
        print("\n[A2] LM Studio (ctx=8192): косинус(полный текст, префикс)")
        sat = None
        for n, v in zip(steps, vecs):
            c = cos(full, v)
            mark = ""
            if c > 0.9999:
                mark = "  <= неотличимо от полного (вход усечён)"
                if sat is None:
                    sat = n
            print("      %6d симв. (%5d ток.) -> %.6f%s"
                  % (n, len(tok.encode(long_text[:n])), c, mark))
        if sat:
            print("[A2] LM Studio: начиная с ~%d символов (%d токенов) вектор "
                  "перестаёт отличаться -> вход молча усечён"
                  % (sat, len(tok.encode(long_text[:sat]))))
        else:
            print("[A2] LM Studio: усечения не обнаружено (векторы различаются "
                  "на всём диапазоне)")

    # --- 2) сырой llama.cpp при ctx=512: ошибка или усечение? ---
    exe = os.path.join(BACKEND, "llama-server.exe")
    log = open(os.path.join(os.environ.get("TEMP", "."), "llama_diag512.log"), "wb")
    proc = subprocess.Popen(
        [exe, "-m", GGUF, "--embedding", "--pooling", "cls", "--ctx-size", "512",
         "--batch-size", "8192", "--ubatch-size", "8192", "-ngl", "0",
         "--threads", str(max(4, (os.cpu_count() or 8) - 2)),
         "--host", "127.0.0.1", "--port", str(PORT)],
        cwd=BACKEND, stdout=log, stderr=log,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    try:
        for _ in range(240):
            if proc.poll() is not None:
                break
            try:
                h = requests.get("http://127.0.0.1:%d/health" % PORT, timeout=2)
                if h.status_code == 200 and h.json().get("status") == "ok":
                    break
            except Exception:  # noqa: BLE001
                pass
            time.sleep(0.5)
        print("\n[A2] llama.cpp --ctx-size 512: ответ на входы разной длины")
        for n in (1000, 3000, 6000):
            r = requests.post("http://127.0.0.1:%d/v1/embeddings" % PORT,
                              json={"input": [long_text[:n]]}, timeout=300)
            body = r.text[:160].replace("\n", " ")
            print("      %6d симв. (%4d ток.): HTTP %d %s"
                  % (n, len(tok.encode(long_text[:n])), r.status_code, body))
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=20)
        except Exception:  # noqa: BLE001
            proc.kill()
        log.close()


if __name__ == "__main__":
    main()