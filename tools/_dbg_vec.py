import json
import math
import struct
import sys

sys.path.insert(0, ".")
from hds import db as dbmod  # noqa: E402
from hds.config import db_abs_path, dig, load  # noqa: E402
from hds.embedder import make_embedder  # noqa: E402

cfg = load()
conn = dbmod.connect(db_abs_path(cfg), int(dig(cfg, "embedding.dim", 1024)))
print("chunks_vec rows:", conn.execute("SELECT COUNT(*) FROM chunks_vec").fetchone()[0])

items = [json.loads(l) for l in open("eval/golden.jsonl", encoding="utf-8") if l.strip()][:5]
emb = make_embedder(cfg)

def cos(a, b):
    dot = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(x * x for x in b))
    return dot / (na * nb) if na and nb else 0.0

for it in items:
    gold = it["chunk_id"]
    row = conn.execute("SELECT embedding FROM chunks_vec WHERE rowid=?", (gold,)).fetchone()
    if not row:
        print("qid %d: у чанка %d НЕТ вектора" % (it["qid"], gold))
        continue
    db_vec = list(struct.unpack("<%df" % (len(row[0]) // 4), row[0]))
    q_vec = emb.embed_query(it["question"])
    chunk = conn.execute("SELECT text FROM chunks WHERE id=?", (gold,)).fetchone()
    c_vec = emb.embed_query(chunk[0][:2000])
    print("qid %d (%s): dim q=%d db=%d | cos(q, gold_vec)=%.4f | cos(q, vec(q_text))=%.4f"
          % (it["qid"], it["kind"], len(q_vec), len(db_vec),
             cos(q_vec, db_vec), cos(q_vec, c_vec)))
conn.close()