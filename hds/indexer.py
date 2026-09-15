"""Инкрементальный индексатор: обход дисков, извлечение, чанкинг, эмбеддинги."""
import hashlib
import json
import os
import struct
import sys
import threading
import time

from . import chunker, db as dbmod, extractors
from .config import dig, db_abs_path

MEDIA_KINDS = {"media"}
_ACTIVE_REPORTER = None   # устанавливается run_index; читается MCP index_status
_LAST_REPORTER = None     # снимок последнего завершённого прогона (для UI)


def content_hash(path, size):
    """Быстрый отпечаток: размер + первые/последние 256 КБ содержимого."""
    h = hashlib.blake2b(digest_size=16)
    h.update(str(size).encode())
    head = 256 * 1024
    try:
        with open(path, "rb") as f:
            h.update(f.read(head))
            if size > head:
                f.seek(-head, 2)
                h.update(f.read(head))
    except OSError:
        return None
    return h.hexdigest()


def iter_files(cfg, roots=None):
    roots = roots or dig(cfg, "index.roots", [])
    excl = {str(e).lower() for e in dig(cfg, "index.exclude_dirs", [])}
    prefixes = _excluded_prefixes(cfg)
    for root in roots:
        root = os.path.abspath(root)
        if _prefix_excluded(root, prefixes):
            print("[skip] корень исключён настройкой exclude_paths: %s" % root)
            continue
        if os.path.isfile(root):
            yield root
            continue
        if not os.path.isdir(root):
            print("[skip] корень не найден: %s" % root)
            continue
        print("[scan] %s" % root)
        for dirpath, dirnames, filenames in os.walk(root, onerror=lambda e: None):
            # отсекаем целые поддеревья: по имени каталога (exclude_dirs)
            # и по полному пути (exclude_paths)
            dirnames[:] = [d for d in dirnames
                           if d.lower() not in excl
                           and not _prefix_excluded(os.path.join(dirpath, d), prefixes)]
            for fn in filenames:
                fp = os.path.join(dirpath, fn)
                if _prefix_excluded(fp, prefixes):  # исключённые одиночные файлы
                    continue
                yield fp


def _norm_path(p):
    """Кроссплатформенная нормализация для сравнения путей/префиксов:
    нижний регистр и разделители -> '/'. Windows-пути ('D:\\x') корректно
    сравниваются на любой ОС (на POSIX os.sep == '/', а '\\' — обычный символ)."""
    return os.path.normcase(os.path.normpath(str(p).strip())).replace("\\", "/").lower()


def _excluded_prefixes(cfg):
    """Нормализованные префиксы из index.exclude_paths (регистр/слэши ОС)."""
    out = []
    for p in dig(cfg, "index.exclude_paths", []) or []:
        p = _norm_path(p)
        if p and p not in out:
            out.append(p)
    return out


def _prefix_excluded(path, prefixes):
    """True, если path равен исключённому префиксу или лежит под ним.
    Сравнение по границе компонента пути: 'D:\\Backup2' не совпадает с 'D:\\Backup'."""
    if not prefixes:
        return False
    np = _norm_path(path)
    return any(np == pr or np.startswith(pr + "/") for pr in prefixes)


def path_excluded(path, cfg):
    """True, если путь лежит в исключённом каталоге (exclude_dirs, по имени)
    или под исключённым префиксом пути (exclude_paths). Без учёта регистра."""
    excl = {str(e).lower() for e in dig(cfg, "index.exclude_dirs", [])}
    p = str(path).replace("\\", "/")
    if any(part.lower() in excl for part in p.split("/")):
        return True
    return _prefix_excluded(p, _excluded_prefixes(cfg))


def _limit_mb(kind, cfg):
    cap = dig(cfg, "index.max_media_mb", 1500) if kind in MEDIA_KINDS \
        else dig(cfg, "index.max_file_mb", 200)
    return int(cap) * 1024 * 1024


def _kind_of(ext):
    from .extract_av import kind_for_ext_media as av_kind
    from .extract_static import kind_for_ext_media as st_kind
    return extractors.kind_for_ext(ext) or st_kind(ext) or av_kind(ext)  # noqa: E501


def _extract_file(conn, cfg, path, force=False, progress_cb=None):
    """Фаза 1: проверки + извлечение текста + чанки (без эмбеддингов).
    Возвращает (fid, chunks, kind) или (None, None, статус_текст-кортеж)."""
    ext = os.path.splitext(path)[1].lower()
    kind = _kind_of(ext)
    if not kind:
        return None, None, ("skipped_type", None)

    # защита в глубину: корзина/системные каталоги (важно для watcher-событий)
    if path_excluded(path, cfg):
        return None, None, ("skipped_excluded", kind)

    # служебные lock-файлы Office (~$док.xlsx): вечно меняются, openpyxl на них
    # падает BadZipFile — пропускаем как нетиповые
    if os.path.basename(path).startswith("~$"):
        return None, None, ("skipped_type", kind)

    try:
        st = os.stat(path)
    except OSError as e:
        return None, None, ("stat_error: %s" % e, kind)
    size, mtime = st.st_size, st.st_mtime
    if size > _limit_mb(kind, cfg):
        return None, None, ("skipped_big", kind)

    row = dbmod.get_file_by_path(conn, path)
    if not force and row and row["status"] == "indexed" \
            and row["size"] == size and abs((row["mtime"] or 0) - mtime) < 2:
        return None, None, ("unchanged", kind)

    # переименование/переезд: контент уже в индексе под другим путём
    chash = content_hash(path, size)
    if not force and (row is None or row["status"] != "indexed"):
        same = dbmod.get_file_by_hash(conn, chash)
        if same is not None and same["path"] != path and same["chunk_count"] > 0:
            dbmod.rename_path(conn, same["path"], path)
            return None, None, ("moved", kind)

    fid = dbmod.upsert_file(conn, path, ext, kind, size, mtime, chash)
    try:
        kind2, segments = extractors.extract(path, cfg, progress_cb=progress_cb)
        chunks = chunker.make_chunks(
            segments,
            dig(cfg, "chunk.size", 1200),
            dig(cfg, "chunk.overlap", 200),
        ) if segments else []
        max_chunks = int(dig(cfg, "index.max_chunks", 2000))
        if max_chunks > 0 and len(chunks) > max_chunks:
            chunks = chunks[:max_chunks]
            print("[warn] %s: текст обрезан до %d чанков (index.max_chunks); "
                  "увеличьте лимит в config.yaml при необходимости" % (path, max_chunks),
                  flush=True)
    except Exception as e:  # noqa: BLE001
        # битый/недописанный файл не должен ронять весь прогон индексации
        # (BadZipFile у Office-файлов, недокачанные pdf и т.п.)
        dbmod.finish_file(conn, fid, "error", str(e)[:500])
        conn.commit()
        return None, None, ("error: %s" % str(e)[:200], kind)
    return fid, chunks, (kind2 or kind)


def _commit_file(conn, emb, cfg, fid, chunks, kind, progress_cb=None):
    """Фаза 2: эмбеддинги чанков + запись в БД. Возвращает статус-строку."""
    try:
        max_chunks = int(dig(cfg, "index.max_chunks", 2000))
        bs = max(1, int(dig(cfg, "embedding.batch_size", 32)))
        total = len(chunks)
        if progress_cb and total:
            progress_cb(0.0)
        vectors = []
        for i in range(0, total, bs):
            vectors.extend(emb.embed([c["text"] for c in chunks[i:i + bs]]))
            if progress_cb and total:
                progress_cb(100.0 * min(i + bs, total) / total)
        for c, v in zip(chunks, vectors):
            c["_blob"] = struct.pack("<%df" % len(v), *v)
    except Exception as e:  # noqa: BLE001
        dbmod.finish_file(conn, fid, "error", str(e)[:500])
        conn.commit()
        return "error: %s" % str(e)[:200]

    dbmod.delete_file_data(conn, fid)
    for i, c in enumerate(chunks):
        cid = dbmod.add_chunk(conn, fid, i, c["page"], c["t_start"], c["t_end"], c["text"])
        dbmod.add_vector(conn, cid, c["_blob"])
    dbmod.finish_file(conn, fid, "indexed", chunks=len(chunks))
    conn.commit()
    return "indexed(%d чанков)" % len(chunks)


def process_file(conn, emb, cfg, path, force=False, progress_cb=None):
    """Индексирует один файл. Возвращает (статус, kind)."""
    fid, chunks, kind = _extract_file(conn, cfg, path, force=force, progress_cb=progress_cb)
    if fid is None:
        return kind  # ранний выход: это кортеж (статус, kind)
    status = _commit_file(conn, emb, cfg, fid, chunks, kind, progress_cb)
    return status, kind


def run_index(conn, emb, cfg, roots=None, kinds=None, full=False,
              limit=None, single_paths=None, prune=True, confirm_delete=False,
              progress_sec=3, quiet=False):
    """Проход по корням; возвращает сводку. Выводит прогресс в stdout.
    Безопасная остановка: Ctrl+C или файл index.stop в корне проекта."""
    from .config import PROJECT_ROOT
    from .progress import PHASES, ProgressReporter

    global _ACTIVE_REPORTER
    stop_file = os.path.join(PROJECT_ROOT, "index.stop")
    pause_file = os.path.join(PROJECT_ROOT, "index.pause")
    hb_file = os.path.join(PROJECT_ROOT, "index.heartbeat.json")
    _hb_lock = threading.Lock()  # heartbeat пишут и основной цикл, и фоновый рефреш

    def _hb(extra=None):
        """Heartbeat-файл: кросс-процессный статус индексации для UI/MCP."""
        with _hb_lock:
            try:
                d = {"ts": time.time()}
                if rep is not None:
                    d.update(rep.heartbeat_data())
                if extra:
                    d.update(extra)
                with open(hb_file, "w", encoding="utf-8") as f:
                    json.dump(d, f)
            except OSError:
                pass

    def _hb_remove():
        try:
            if os.path.exists(hb_file):
                os.remove(hb_file)
        except OSError:
            pass
    t0 = time.time()
    counters = {}
    seen_roots = single_paths is None
    paths = single_paths if single_paths is not None else iter_files(cfg, roots)
    n = 0
    rep = ProgressReporter(sec=0 if quiet else progress_sec)  # счётчики всегда (heartbeat), quiet — без печати
    _ACTIVE_REPORTER = rep
    rep.start()
    # Периодический рефреш heartbeat: долгая обработка одного файла (OCR
    # скана на 10 минут, Whisper-транскрипция mp3) не должна выглядеть
    # в UI как «индексация остановилась» — ts в файле должен оставаться свежим
    _hb_stop = threading.Event()

    def _hb_loop():
        while not _hb_stop.wait(5):
            _hb()

    _hbt = threading.Thread(target=_hb_loop, daemon=True)
    _hbt.start()
    if not quiet:
        mode = "терминал (живая строка)" if rep.is_tty else "не-терминал (полные строки)"
        print("[прогресс] активен: обновление каждые %d с, режим: %s; отключить: --progress-sec 0"
              % (progress_sec, mode), flush=True)

    # Пре-подсчёт для ETA — в фоновом потоке (не блокирует основной обход)
    def _precount():
        try:
            if seen_roots:
                rep.set_total(sum(1 for _ in iter_files(cfg, roots)))
        except Exception:  # noqa: BLE001
            pass
    threading.Thread(target=_precount, daemon=True).start()

    for path in paths:
        if rep:
            rep.set_last_path(path)
        # пауза: ждём снятия index.pause (или остановки)
        if os.path.exists(pause_file):
            was_paused = False
            while os.path.exists(pause_file):
                if os.path.exists(stop_file):
                    break
                if not was_paused and rep:
                    rep.set_paused(True)
                    print("[пауза] индексация приостановлена (файл index.pause); "
                          "снимите паузу через UI или удалите файл", flush=True)
                was_paused = True
                _hb({"paused": True})
                time.sleep(1)
            if rep:
                rep.set_paused(False)
            if os.path.exists(stop_file):
                break
        if os.path.exists(stop_file):
            if rep:
                rep.note()
            print("[stop] найден index.stop — аккуратная остановка "
                  "(все обработанные файлы уже сохранены)", flush=True)
            counters["stopped"] = True
            break
        try:
            n += 1
            if rep:
                rep.seen()
            ext = os.path.splitext(path)[1].lower()
            kind = _kind_of(ext)
            if rep:
                rep.set_current(path, PHASES.get(kind, "обработка"))
            _hb({"path": path, "phase": PHASES.get(kind, "обработка")})
            progress_cb = (lambda p: rep.set_progress(p)) if rep else None
            if progress_cb and kind == "media":
                # печатать «долго»-сообщение только если файл действительно будет обрабатываться
                row0 = dbmod.get_file_by_path(conn, path)
                try:
                    st0 = os.stat(path)
                    skip = (not full and row0 and row0["status"] == "indexed"
                            and row0["size"] == st0.st_size
                            and abs((row0["mtime"] or 0) - st0.st_mtime) < 2)
                except OSError:
                    skip = True
                if not skip:
                    rep.note()
                    print("[..] %s — извлечение аудио + Whisper-транскрипция "
                          "(%.0f МБ), может занять несколько минут..."
                          % (path, st0.st_size / 1048576.0), flush=True)
            t1 = time.time()
            status, kind2 = process_file(conn, emb, cfg, path, force=full,
                                         progress_cb=progress_cb)
            dur = time.time() - t1
            _hb()
            # CLIP-вектор для картинок: контентный поиск «найди изображения …»
            if kind == "image" and dig(cfg, "index.clip", True):
                try:
                    from . import clip_index

                    fid_now = dbmod.get_file_by_path(conn, path)
                    if fid_now:
                        clip_index.store_for_file(conn, fid_now["id"], path)
                except Exception as e:  # noqa: BLE001
                    print("[clip] ошибка: %s" % e, file=sys.stderr, flush=True)
            if rep:
                chunks_n = 0
                if "(" in status:
                    try:
                        chunks_n = int(status.split("(")[1].split()[0])
                    except (ValueError, IndexError):
                        chunks_n = 0
                rep.processed(status, kind2 or kind, dur, chunks=chunks_n)
                rep.last_done = (path, status.split("(")[0], dur)
        except KeyboardInterrupt:
            if rep:
                rep.note()
            conn.rollback()  # недописанный файл откатится, останется со статусом 'new'
            print("\n[stop] Прервано (Ctrl+C). Обработанные файлы сохранены; "
                  "текущий файл будет дообработан при следующем запуске.", flush=True)
            counters["stopped"] = True
            break
        key = status.split("(")[0]
        counters[key] = counters.get(key, 0) + 1
        if rep:
            rep.note()
        if not status.startswith("unchanged") and (n % 20 == 1 or status.startswith(("indexed", "error"))):
            print("[%d] %s -> %s (%.1f с)" % (n, path, status, dur), flush=True)
        if limit and n >= limit:
            break

    if prune and seen_roots and not counters.get("stopped"):
        if rep:
            rep.note()
        _prune_deleted(conn, cfg, confirm_delete=confirm_delete)

    _remove_stop_file(stop_file)
    _hb_stop.set()      # фоновый рефреш heartbeat остановить ДО удаления файла
    _hbt.join(timeout=3)
    _hb_remove()
    if rep:
        _ACTIVE_REPORTER = None
        global _LAST_REPORTER
        _LAST_REPORTER = rep
        rep.finish(counters)
    else:
        counters["elapsed_sec"] = round(time.time() - t0, 1)
        counters["files_seen"] = n
        print("[done] %s" % counters)
    return counters


def _remove_stop_file(stop_file):
    try:
        os.unlink(stop_file)
    except OSError:
        pass


def _prune_deleted(conn, cfg, confirm_delete=False):
    total = conn.execute("SELECT COUNT(*) FROM files").fetchone()[0]
    if not total:
        return
    missing = []
    for fid, path in dbmod.all_files(conn):
        if not os.path.exists(path):
            missing.append((fid, path))
    if not missing:
        return
    if len(missing) > 0.2 * total and not confirm_delete:
        print("[prune] Пропало %d из %d файлов (>%d%%). Похоже, диск был отключён. "
              "Удаление заблокировано; перезапустите с --confirm-delete, если это ожидаемо."
              % (len(missing), total, 20))
        return
    for fid, path in missing:
        dbmod.delete_file_data(conn, fid)
        conn.execute("DELETE FROM files WHERE id=?", (fid,))
    conn.commit()
    print("[prune] Удалено из индекса: %d" % len(missing))


def reindex_path(conn, emb, cfg, path, force=True):
    p = os.path.abspath(path)
    if os.path.isfile(p):
        return [process_file(conn, emb, cfg, p, force=force)]
    res = []
    excl = {str(e).lower() for e in dig(cfg, "index.exclude_dirs", [])}
    for dirpath, dirnames, filenames in os.walk(p, onerror=lambda e: None):
        dirnames[:] = [d for d in dirnames if d.lower() not in excl]
        for fn in filenames:
            res.append(process_file(conn, emb, cfg, os.path.join(dirpath, fn), force=force))
    return res