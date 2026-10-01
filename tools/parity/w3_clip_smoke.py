"""W3/§7: живая проверка `hds clip-index` (Rust) на временной БД.

setup  — создать временную БД с 2 картинками (status=indexed), печатает images_vec до.
check  — печатает images_vec после (ожидаем 2).

Запуск: .venv\\Scripts\\python.exe tools\\parity\\w3_clip_smoke.py setup|check
"""
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, ROOT)
from hds import db as dbmod  # noqa: E402

DB = os.path.join(ROOT, "tools", "parity", "out", "w3_clip.db")
FIX = os.path.join(ROOT, "tools", "parity", "fixtures")
IMAGES = ["накладная_с_exif.jpg", "цветы_без_exif.jpg"]


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    conn = dbmod.connect(DB, 1024)
    if cmd == "setup":
        if os.path.exists(DB):
            conn.execute("DELETE FROM files")
        for name in IMAGES:
            p = os.path.join(FIX, name)
            conn.execute(
                "INSERT OR REPLACE INTO files(path, ext, kind, size, mtime, status, "
                "chunk_count, indexed_at) VALUES(?,?,?,?,?,?,?,?)",
                (p, ".jpg", "image", os.path.getsize(p), 1.0, "indexed", 1, 1.0),
            )
        conn.commit()
    n = conn.execute("SELECT COUNT(*) FROM images_vec").fetchone()[0]
    rows = conn.execute("SELECT COUNT(*) FROM files WHERE kind='image'").fetchone()[0]
    print("images=%d images_vec=%d" % (rows, n))


if __name__ == "__main__":
    main()
