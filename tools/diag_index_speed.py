"""Диагностика медленной индексации на второй машине (AMD/Vulkan без CUDA).

Проверяет по шагам главные причины, найденные бенчмарком на эталонной машине:
  1. LM Studio поднят и отдаёт модель эмбеддингов;
  2. не висит ли параллельно большой чат-LLM (конкуренция за GPU);
  3. реальная скорость эмбеддингов через продакшн-путь (Embedder.embed,
     batch_size из config.yaml) — главный индикатор офлоада модели на GPU;
  4. whisper.cpp (Vulkan) — установлен ли, и опционально RTF на тестовом аудио.

Запуск на диагностируемой машине (из корня проекта):
  .venv\\Scripts\\python.exe tools\\diag_index_speed.py
Опционально — бенчмарк транскрипции на аудиофайле:
  .venv\\Scripts\\python.exe tools\\diag_index_speed.py --audio test_data\\speech_2min.wav

Эталон (RTX 3060; CUDA и Vulkan-рантайм LM Studio): 33-38 мс/чанк.
CPU-офлоад bge-m3 даёт от ~10x медленнее — вердикт печатается в выводе.
"""
import argparse
import copy
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import requests


def _print(title):
    print("\n=== %s ===" % title, flush=True)


def check_server(cfg):
    _print("1. LM Studio: сервер и модель эмбеддингов")
    url = cfg.get("embedding", {}).get("base_url", "http://localhost:1234/v1")
    model = cfg.get("embedding", {}).get("model", "bge-m3")
    try:
        r = requests.get(url + "/models", timeout=5)
        ids = [m.get("id") for m in r.json().get("data", [])]
    except Exception as e:  # noqa: BLE001
        print("[!!] Сервер %s недоступен: %s" % (url, e))
        print("     Запустите LM Studio и включите сервер (Developer -> Start Server).")
        return None
    print("[ok] Сервер отвечает, моделей в каталоге: %d" % len(ids))
    if not any(model in (i or "") for i in ids):
        print("[!!] Модель '%s' не найдена среди: %s" % (model, ids))
        print("     Скачайте её в LM Studio — без неё индексация невозможна.")
        return None
    print("[ok] Модель эмбеддингов '%s' доступна" % model)
    return url


def check_loaded_models():
    _print("2. Загруженные модели (конкуренция за GPU)")
    try:
        out = subprocess.run(["lms", "ps"], capture_output=True, text=True,
                             timeout=30).stdout.strip()
    except Exception:  # noqa: BLE001
        print("[--] lms CLI не найден — проверьте вручную: lms ps")
        return
    print(out or "(ничего не загружено)")
    big = [ln for ln in out.splitlines()
           if ln.strip() and not ln.startswith(("IDENTIFIER", "---"))
           and "embedding" not in ln.lower()]
    if big:
        print("[!!] Вместе с эмбеддингами загружена LLM — на время индексации")
        print("     лучше выгрузить её: lms unload <идентификатор>")


def bench_embeddings(cfg, batches=4):
    _print("3. Скорость эмбеддингов (продакшн-путь индексатора)")
    from hds.chunker import make_chunks
    from hds.embedder import make_embedder

    ecfg = cfg.get("embedding", {})
    bs = max(1, int(ecfg.get("batch_size", 64)))
    with open("README.md", "r", encoding="utf-8") as f:
        text = f.read()
    chunks = make_chunks([{"text": text, "page": None,
                           "t_start": None, "t_end": None}] * batches,
                         cfg.get("chunk", {}).get("size", 1200),
                         cfg.get("chunk", {}).get("overlap", 200))
    total = min(len(chunks), bs * batches)
    emb = make_embedder(cfg)
    t0 = time.perf_counter()
    emb.embed(["ping"])
    print("[..] Прогрев (вкл. загрузку модели в LM Studio): %.1f с"
          % (time.perf_counter() - t0))

    t0 = time.perf_counter()
    for i in range(0, total, bs):  # ровно цикл indexer._commit_file
        emb.embed([c["text"] for c in chunks[i:i + bs]])
    wall = time.perf_counter() - t0
    per_chunk = 1000.0 * wall / total
    print("[ok] %d чанков за %.1f с -> %.1f мс/чанк (batch_size=%d)"
          % (total, wall, per_chunk, bs))
    if per_chunk < 80:
        print("[ok] Вердикт: модель на GPU, режим нормальный. Эмбеддинги — НЕ причина.")
    elif per_chunk < 250:
        print("[?] Вердикт: похоже на частичный офлоад. В LM Studio для bge-m3")
        print("     выставьте полный GPU-offload и повторите этот скрипт.")
    else:
        print("[!!] Вердикт: модель считается на CPU (~10x медленнее GPU).")
        print("     ГЛАВНЫЙ подозреваемый. В LM Studio загрузите bge-m3 с")
        print("     GPU-offload = max (рантайм: lms runtime ls / lms runtime select).")


def bench_transcribe(cfg, audio, repeats=2):
    _print("4. Транскрипция (whisper.cpp / Vulkan)")
    from hds import extract_av
    from hds import whisper_cpp as wcpp

    if not wcpp.available(cfg):
        print("[--] whisper.cpp не установлен (python -m hds.cli vulkan-setup) — пропуск.")
        return
    print("[ok] Бинарь: %s" % wcpp.find_exe(cfg))
    if not audio:
        print("[--] Аудиофайл не передан (--audio путь) — RTF не меряем.")
        return
    icfg = copy.deepcopy(cfg)
    icfg.setdefault("index", {})["whisper_device"] = "vulkan"
    for rep in range(1, repeats + 1):
        t0 = time.perf_counter()
        segs, err = extract_av._transcribe(audio, icfg, is_video=False)
        wall = time.perf_counter() - t0
        if err:
            print("[!!] Повтор %d: ошибка: %s" % (rep, err))
            return
        dur = segs[-1].get("t_end") if segs else 0.0
        print("[ok] Повтор %d: %.1f с | аудио %.0f с | RTF %.3f" %
              (rep, wall, dur, wall / dur if dur else 0.0))
    print("    Эталон этой же сборки: RTX 3060 -> RTF 0.04; AMD iGPU -> 0.12.")
    print("    RTF сильно выше 0.3 — подробный бенчмарк: tools/bench_paths.py")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--audio", default=None, help="аудиофайл для бенчмарка транскрипции")
    p.add_argument("--batches", type=int, default=4)
    args = p.parse_args()

    from hds.config import load

    cfg = load()
    if check_server(cfg) is None:
        return 1
    check_loaded_models()
    bench_embeddings(cfg, args.batches)
    bench_transcribe(cfg, args.audio)

    _print("5. Прочее (проверить глазами)")
    icfg = cfg.get("index", {})
    print("    ocr: %s (ocr_lang: %s) — Tesseract работает на CPU и часто является"
          % (icfg.get("ocr"), icfg.get("ocr_lang")))
    print("      главным тормозом PDF/картинок; сравните прогон с ocr: false")
    print("    roots: %s — на HDD/медленном SSD тормозят извлечение и content_hash"
          % icfg.get("roots"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
