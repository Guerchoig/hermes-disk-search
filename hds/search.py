"""Гибридный поиск: FTS5 (ключевые слова, BM25) + векторы (семантика), слияние RRF."""
import re
import struct
import sys

from .config import dig
from .lemmatizer import lemmatize_token

TOKEN_RE = re.compile(r"[\w]{2,}", re.UNICODE)


def _fts_tokens(q):
    """Токены запроса, лемматизированные тем же способом, что и chunks_fts."""
    return [lemmatize_token(t) for t in TOKEN_RE.findall(q)[:12]]


def _quoted(tokens):
    """Экранированные FTS-фразы («" ... «" -> '""'»)."""
    return ['"%s"' % t.replace('"', '""') for t in tokens]


def fts_query(q):
    """OR-запрос по леммам: разные словоформы находят друг друга.

    Токены запроса лемматизируются тем же способом, что и текст в chunks_fts
    (hds.lemmatizer.normalize при индексации)."""
    tokens = _fts_tokens(q)
    if not tokens:
        return None
    return " OR ".join(_quoted(tokens))


def fts_search_ids(conn, q, k, kinds=None):
    """Топ-k chunk_id по FTS: сначала AND, при пустом результате — OR.

    Последний токен ищется с префиксом («настройк*» — найдёт «настройки»).
    kinds — фильтр по files.kind, пробрасывается прямо в SQL (pushdown):
    иначе top-k кандидатов мог целиком состоять из «чужих» типов и
    отфильтрованная выдача пустела при существующих совпадениях.
    Возвращает [] при пустом запросе или ошибке."""
    tokens = _fts_tokens(q)
    if not tokens:
        return []
    quoted = _quoted(tokens)
    quoted[-1] += "*"  # префиксный матчинг последнего токена

    join, kind_sql, params_tail = "", "", []
    if kinds:
        join = ("JOIN chunks c ON c.id = cf.rowid "
                "JOIN files f ON f.id = c.file_id ")
        kind_sql = " AND f.kind IN (%s)" % ",".join("?" * len(kinds))
        params_tail = list(kinds)

    def run(expr):
        try:
            rows = conn.execute(
                "SELECT cf.rowid FROM chunks_fts cf %sWHERE chunks_fts MATCH ?%s "
                "ORDER BY bm25(chunks_fts) LIMIT ?" % (join, kind_sql),
                [expr] + params_tail + [k]).fetchall()
            return [r[0] for r in rows]
        except Exception as e:  # noqa: BLE001
            print("[fts] ошибка поиска: %s" % e, file=sys.stderr)
            return []

    ids = run(" AND ".join(quoted))
    if not ids and len(quoted) > 1:
        ids = run(" OR ".join(quoted))  # AND слишком строг — ослабляем до OR
    return ids


_SENT_END_RE = re.compile(r"[.!?…]\s|\n")


def make_snippet(text, q, max_len=500):
    """Сниппет вокруг первого вхождения токена запроса.

    Границы окна двигаются к ближайшим концам предложений, чтобы не резать
    фразу посередине (если предложение целиком не влезает — режем как раньше)."""
    tokens = [t.lower() for t in TOKEN_RE.findall(q)][:20]
    lower = text.lower()
    pos = -1
    for t in tokens:
        p = lower.find(t)
        if p != -1 and (pos == -1 or p < pos):
            pos = p
    if pos == -1 and tokens:
        # лемма запроса может встретиться как подстрока словоформы текста
        for t in (lemmatize_token(x) for x in tokens):
            p = lower.find(t)
            if p != -1 and (pos == -1 or p < pos):
                pos = p
    if pos == -1:
        pos = 0
    start = max(0, pos - max_len // 3)
    end = min(len(text), start + max_len)
    if start > 0:
        # последний конец предложения ДО pos; окно поиска расширяем влево,
        # чтобы захватить конец предыдущего предложения рядом с границей окна
        last = None
        for m in _SENT_END_RE.finditer(text, max(0, start - 150), pos + 1):
            last = m
        if last:
            start = min(last.end(), pos)
    if end < len(text):
        # ближайший конец предложения ПОСЛЕ pos; окно расширяем вправо,
        # чтобы не обрезать предложение, чуть не влезшее в лимит
        last = None
        for m in _SENT_END_RE.finditer(text, min(pos, start),
                                       min(len(text), end + 150)):
            last = m
        if last:
            end = max(last.end(), min(pos + 1, len(text)))
    snip = text[start:end].replace("\n", " ").strip()
    return ("…" if start > 0 else "") + snip + ("…" if end < len(text) else "")


def format_location(r):
    loc = r["path"]
    if r["page"]:
        loc += " (стр. %d)" % r["page"]
    if r["t_start"] is not None:
        loc += " [%02d:%02d:%02d]" % (r["t_start"] // 3600, r["t_start"] % 3600 // 60, r["t_start"] % 60)
    return loc


def search(conn, emb, cfg, query, kinds=None, limit=8):
    """Возвращает [{path, ext, kind, page, t_start, t_end, text, snippet, score}]."""
    vec_k = int(dig(cfg, "search.vec_k", 40))
    fts_k = int(dig(cfg, "search.fts_k", 40))
    rrf_k = int(dig(cfg, "search.rrf_k", 60))
    fts_w = float(dig(cfg, "search.fts_weight", 1.0))
    vec_w = float(dig(cfg, "search.vec_weight", 1.0))
    snippet_chars = int(dig(cfg, "search.snippet_chars", 500))
    scores = {}

    # FTS-ветка: AND с префиксным хвостом, при пустом результате OR (fts_search_ids)
    for rank, cid in enumerate(fts_search_ids(conn, query, fts_k)):
        scores[cid] = scores.get(cid, 0.0) + fts_w / (rrf_k + rank)

    if emb is not None and emb.available:
        try:
            qv = emb.embed_query(query)
            blob = struct.pack("<%df" % len(qv), *qv)
            # kinds пробрасывается в SQL (проверено: sqlite-vec поддерживает
            # JOIN при KNN-запросе) — иначе top-k мог состоять из «чужих» типов
            sql = ("SELECT v.rowid, v.distance FROM chunks_vec v "
                   "JOIN chunks c ON c.id = v.rowid "
                   "JOIN files f ON f.id = c.file_id "
                   "WHERE v.embedding MATCH ? AND v.k = ?")
            params = [blob, vec_k]
            if kinds:
                sql += " AND f.kind IN (%s)" % ",".join("?" * len(kinds))
                params += list(kinds)
            rows = conn.execute(sql + " ORDER BY v.distance", params).fetchall()
            for rank, (cid, _d) in enumerate(rows):
                scores[cid] = scores.get(cid, 0.0) + vec_w / (rrf_k + rank)
        except Exception as e:  # noqa: BLE001
            print("[vec] семантический поиск недоступен (%s); работает ключевой" % e,
                  file=sys.stderr)

    # CLIP: контентный поиск по картинкам («найди изображения цветов») —
    # текстовый запрос на любом языке сравнивается с самими изображениями.
    # Результаты — по file_id (rowid = files.id); привязываем к первому чанку
    # файла (у картинок он один), дальше общий RRF-конвейер.
    if (not kinds or "image" in kinds) and dig(cfg, "index.clip", True):
        try:
            from . import clip_index

            if clip_index.available():
                qv = clip_index.embed_text(query)
                if qv:
                    blob = struct.pack("<%df" % len(qv), *qv)
                    rows = conn.execute(
                        "SELECT rowid, distance FROM images_vec WHERE embedding MATCH ? "
                        "AND k = ? ORDER BY distance",
                        (blob, vec_k),
                    ).fetchall()
                    clip_w = float(dig(cfg, "search.clip_weight", 1.0))
                    # один батч-запрос file_id -> первый чанк вместо N+1
                    fids = [r[0] for r in rows]
                    chmap = {}
                    if fids:
                        qm = ",".join("?" * len(fids))
                        for ch in conn.execute(
                                "SELECT id, file_id FROM chunks "
                                "WHERE file_id IN (%s) ORDER BY id" % qm, fids):
                            chmap.setdefault(ch[1], ch[0])
                    for rank, (fid, _d) in enumerate(rows):
                        cid = chmap.get(fid)
                        if cid is not None:
                            scores[cid] = scores.get(cid, 0.0) + clip_w / (rrf_k + rank)
        except Exception as e:  # noqa: BLE001
            print("[clip] поиск по содержанию картинок недоступен (%s)" % e,
                  file=sys.stderr)

    if not scores:
        return []
    order = sorted(scores.items(), key=lambda kv: -kv[1])
    results = []
    sql = ("SELECT c.text, c.page, c.t_start, c.t_end, f.path, f.ext, f.kind "
           "FROM chunks c JOIN files f ON f.id=c.file_id WHERE c.id=?")
    for cid, sc in order:
        row = conn.execute(sql, (cid,)).fetchone()
        if not row:
            continue
        if kinds and row["kind"] not in kinds:
            continue
        text = row["text"]
        results.append({
            "path": row["path"], "ext": row["ext"], "kind": row["kind"],
            "page": row["page"], "t_start": row["t_start"], "t_end": row["t_end"],
            "text": text, "snippet": make_snippet(text, query, snippet_chars),
            "score": round(sc, 5),
        })
        if len(results) >= limit:
            break
    return results