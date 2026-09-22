"""Фаза A2+A3: усечение по контексту и сверка LM Studio против сырого llama.cpp.

Запускает bundled llama-server (из LM Studio, тот же GGUF bge-m3) с явными
--ctx-size/--pooling и сравнивает:
  A2 — усечение: косинус(полный длинный текст, его префикс) при ctx=512;
  A3 — слой сервинга LM Studio: косинус(LM Studio, llama.cpp) при ctx=8192
       для pooling cls и mean (какой из них применяет LM Studio).
Только чтение: конфиг/БД не изменяются, LM Studio не перезагружается.
"""
import math
import os
import sqlite3
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import requests  # noqa: E402

from hds.config import db_abs_path, dig, load  # noqa: E402

PORT = 8899
GGUF = os.path.join(os.path.expanduser("~"), ".lmstudio", "models", "lm-kit",
                    "bge-m3-gguf", "bge-m3-Q8_0.gguf")
BACKEND = os.path.join(os.path.expanduser("~"), ".lmstudio", "extensions",
                       "backends", "llama.cpp-win-x86_64-avx2-2.24.0")


def cos(a, b):
    s = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(y * y for y in b))
    return s / (na * nb) if na and nb else 0.0


def norm(a):
    return math.sqrt(sum(x * x for x in a))


def lm_studio_vectors(cfg, texts):
    from hds.embedder import make_embedder

    emb = make_embedder(cfg)
    return emb.embed(texts)


def llama_vectors(pooling, ctx, texts):
    exe = os.path.join(BACKEND, "llama-server.exe")
    args = [exe, "-m", GGUF, "--embedding", "--pooling", pooling,
            "--ctx-size", str(ctx), "--batch-size", "8192",
            "--ubatch-size", "8192", "-ngl", "0", "--threads",
            str(max(4, (os.cpu_count() or 8) - 2)),
            "--host", "127.0.0.1", "--port", str(PORT)]
    log = open(os.path.join(os.environ.get("TEMP", "."), "llama_diag.log"), "wb")
    proc = subprocess.Popen(args, cwd=BACKEND, stdout=log, stderr=log,
                            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    try:
        ready = False
        for _ in range(240):
            if proc.poll() is not None:
                break
            try:
                h = requests.get("http://127.0.0.1:%d/health" % PORT, timeout=2)
                if h.status_code == 200 and h.json().get("status") == "ok":
                    ready = True
                    break
            except Exception:  # noqa: BLE001
                pass
            time.sleep(0.5)
        if not ready:
            time.sleep(1)
            return None, "сервер не поднялся (лог: " + os.path.join(os.environ.get("TEMP", "."), "llama_diag.log") + ")"
        out = []
        for t in texts:
            r = requests.post("http://127.0.0.1:%d/v1/embeddings" % PORT,
                              json={"input": [t]}, timeout=600)
            r.raise_for_status()
            out.append(r.json()["data"][0]["embedding"])
        return out, ""
    except Exception as e:  # noqa: BLE001
        return None, repr(e)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=20)
        except Exception:  # noqa: BLE001
            proc.kill()
        log.close()


def main():
    cfg = load()
    db = db_abs_path(cfg)
    conn = sqlite3.connect("file:%s?mode=ro" % db.replace("\\", "/"), uri=True)
    rows = [r[0] for r in conn.execute(
        "SELECT text FROM chunks WHERE length(text) BETWEEN 1000 AND 2200 "
        "ORDER BY RANDOM() LIMIT 6").fetchall()]
    long_row = conn.execute("SELECT text FROM chunks WHERE length(text) > 1500 "
                            "ORDER BY RANDOM() LIMIT 1").fetchone()
    conn.close()
    if not rows or not long_row:
        print("[A2/A3] в БД нет подходящих чанков")
        return
    print("[A2/A3] GGUF: %s (%d МБ)" % (GGUF, os.path.getsize(GGUF) // 1048576))

    # длинный текст: 12 чанков подряд (~20k символов)
    conn = sqlite3.connect("file:%s?mode=ro" % db.replace("\\", "/"), uri=True)
    many = [r[0] for r in conn.execute(
        "SELECT text FROM chunks ORDER BY RANDOM() LIMIT 12").fetchall()]
    conn.close()
    long_text = "\n".join(many)
    print("[A2/A3] эталонный длинный текст: %d символов" % len(long_text))

    print("\n[A3] Эмбеддинг 6 чанков и длинного текста через LM Studio...")
    lm = lm_studio_vectors(cfg, rows + [long_text])
    print("[A3] размерность=%d  норма(long)=%.4f" % (len(lm[0]), norm(lm[-1])))

    for pooling in ("cls", "mean"):
        print("\n[A3] llama-server --pooling %s --ctx-size 8192 ..." % pooling)
        vecs, err = llama_vectors(pooling, 8192, rows + [long_text])
        if vecs is None:
            print("[A3] ошибка: %s" % err)
            continue
        cosines = [cos(lm[i], vecs[i]) for i in range(len(vecs))]
        print("[A3] косинус LM Studio vs llama.cpp(%s): min=%.4f  mean=%.4f"
              % (pooling, min(cosines), sum(cosines) / len(cosines)))
        print("[A3] нормы llama.cpp: long=%.4f (LM Studio: %.4f)"
              % (norm(vecs[-1]), norm(lm[-1])))

    print("\n[A2] Тест усечения: llama-server --pooling cls --ctx-size 512")
    steps = list(range(1000, len(long_text), 1500)) + [len(long_text)]
    prefixes = [long_text[:n] for n in steps]
    vecs, err = llama_vectors("cls", 512, prefixes + [long_text])
    if vecs is None:
        print("[A2] ошибка: %s" % err)
        return
    full = vecs[-1]
    print("[A2] косинус(полный текст, префикс) при ctx=512:")
    for n, v in zip(steps, vecs):
        print("      префикс %6d симв. -> %.6f%s"
              % (n, cos(full, v), "   <= идентично (текст усечён)" if cos(full, v) > 0.9999 else ""))
    print("[A2] для сравнения, та же проверка при ctx=8192 (--pooling cls)")
    vecs2, err2 = llama_vectors("cls", 8192, [long_text[:2000]] + [long_text])
    if vecs2:
        print("      префикс   2000 симв. -> %.6f" % cos(vecs2[1], vecs2[0]))


if __name__ == "__main__":
    main()