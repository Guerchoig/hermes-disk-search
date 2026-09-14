"""CLIP-индекс изображений: поиск фото по содержанию («найди изображения цветов»).

Image-энкодер — CLIP ViT-B-32, text-энкодер — multilingual-v1 (понимает русский).
Связка sentence-transformers; векторы нормализованы, dim=512, хранятся в
images_vec (rowid = files.id). Всегда безопасен: любая ошибка -> available()=False,
индексация и поиск продолжают работать без CLIP."""
import os
import sys

CLIP_DIM = 512
_img_model = None
_txt_model = None
_ok = None  # None = ещё не проверяли


def enabled(cfg):
    """Выключено явно в конфиге? (index.clip: false)"""
    from .config import dig

    return bool(dig(cfg, "index.clip", True))


def available():
    """Ленивая загрузка моделей; True, если CLIP готов к работе."""
    global _img_model, _txt_model, _ok
    if _ok is not None:
        return _ok
    try:
        from sentence_transformers import SentenceTransformer

        _img_model = SentenceTransformer("clip-ViT-B-32")
        _txt_model = SentenceTransformer(
            "sentence-transformers/clip-ViT-B-32-multilingual-v1")
        _ok = True
    except Exception as e:  # noqa: BLE001
        print("[clip] недоступен (%s) — поиск картинок по содержанию выключен" % e,
              file=sys.stderr, flush=True)
        _ok = False
    return _ok


def embed_images(paths):
    """Векторы [dim] для файлов-картинок (нормализованные)."""
    if not available():
        return []
    from PIL import Image

    imgs = []
    for p in paths:
        try:
            imgs.append(Image.open(p).convert("RGB"))
        except Exception:  # noqa: BLE001
            imgs.append(Image.new("RGB", (64, 64), "black"))
    vecs = _img_model.encode(imgs, normalize_embeddings=True, show_progress_bar=False)
    return [v.tolist() for v in vecs]


def embed_text(query):
    """Вектор текстового запроса на любом языке (ru/en/...)."""
    if not available():
        return None
    v = _txt_model.encode([query], normalize_embeddings=True)[0]
    return v.tolist()


def store_for_file(conn, file_id, path):
    """Посчитать и сохранить CLIP-вектор картинки. Ошибки глотаются:
    картинка без CLIP-вектора просто не участвует в контентном поиске."""
    if not os.path.exists(path) or not available():
        return
    try:
        import struct

        v = embed_images([path])[0]
        add_image_vector(conn, file_id, struct.pack("<%df" % len(v), *v))
    except Exception as e:  # noqa: BLE001
        import sys

        print("[clip] %s: %s" % (os.path.basename(path), e), file=sys.stderr, flush=True)


def add_image_vector(conn, file_id, blob):
    # vec0-таблицы не поддерживают INSERT OR REPLACE — удаляем и вставляем;
    # конфликт (например, вектор уже добавил параллельный watcher) пропускаем
    conn.execute("DELETE FROM images_vec WHERE rowid=?", (file_id,))
    try:
        conn.execute(
            "INSERT INTO images_vec(rowid, embedding) VALUES(?,?)",
            (file_id, blob),
        )
    except Exception:  # noqa: BLE001
        pass
