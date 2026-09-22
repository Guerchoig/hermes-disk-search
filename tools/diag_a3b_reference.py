"""Фаза A3b (эталон): совпадают ли векторы LM Studio с PyTorch-референсом bge-m3.

Разово скачивает BAAI/bge-m3 (~2,3 ГБ) и считает косинус LM Studio vs эталон
(CLS-пулинг + L2-нормализация) на реальных чанках. Отвечает на вопрос: не
искажает ли GGUF-конвертация (Q8_0) векторы; LM Studio при этом не трогается.
"""
import math
import os
import sqlite3
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
sys.stderr.reconfigure(encoding="utf-8", errors="replace")


def cos(a, b):
    s = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(y * y for y in b))
    return s / (na * nb) if na and nb else 0.0


def main():
    from hds.config import db_abs_path, load

    cfg = load()
    db = db_abs_path(cfg)
    conn = sqlite3.connect("file:%s?mode=ro" % db.replace("\\", "/"), uri=True)
    rows = [r[0] for r in conn.execute(
        "SELECT text FROM chunks WHERE length(text) BETWEEN 1000 AND 2200 "
        "ORDER BY RANDOM() LIMIT 6").fetchall()]
    conn.close()
    print("[A3b] чанков для сравнения: %d" % len(rows), flush=True)

    print("[A3b] загружаю эталон BAAI/bge-m3 (PyTorch, CPU)...", flush=True)
    import torch
    from transformers import AutoModel, AutoTokenizer

    tok = AutoTokenizer.from_pretrained("BAAI/bge-m3")
    model = AutoModel.from_pretrained("BAAI/bge-m3")
    model.eval()
    print("[A3b] эталон загружен", flush=True)

    def ref_embed(texts):
        out = []
        with torch.no_grad():
            for t in texts:
                enc = tok(t, return_tensors="pt", truncation=True,
                          max_length=8192)
                hidden = model(**enc).last_hidden_state
                v = torch.nn.functional.normalize(hidden[:, 0, :][0], dim=0)
                out.append(v.tolist())
        return out

    ref = ref_embed(rows)

    from hds.embedder import make_embedder

    lm = make_embedder(cfg).embed(rows)
    print("[A3b] размерность: эталон=%d  LM Studio=%d" % (len(ref[0]), len(lm[0])),
          flush=True)

    cosines = [cos(ref[i], lm[i]) for i in range(len(rows))]
    print("[A3b] косинус PyTorch(bge-m3) vs LM Studio(GGUF Q8_0):")
    for i, c in enumerate(cosines):
        print("      чанк %d (%d симв.): %.5f" % (i + 1, len(rows[i]), c),
              flush=True)
    print("[A3b] ИТОГ: min=%.5f  mean=%.5f" % (min(cosines),
                                               sum(cosines) / len(cosines)),
          flush=True)
    verdict = ("GGUF-конвертация не искажает векторы (>=0.99)"
               if min(cosines) >= 0.99 else
               "GGUF-конвертация заметно отличается (<0.99) — проверить глубже")
    print("[A3b] ВЕРДИКТ: %s" % verdict, flush=True)


if __name__ == "__main__":
    main()