"""Бенчмарк индексации по продакшн-пути проекта (никаких самодельных пайплайнов).

Два подкоманды:

  embed      — фаза 2 индексатора: chunker.make_chunks() + Embedder.embed()
               батчами embedding.batch_size против LM Studio (как в indexer.
               _commit_file). Устройство/рантайм определяет сам LM Studio
               (переключается `lms runtime select`).

  transcribe — extract_av._transcribe() с форсированным index.whisper_device
               (cuda = faster-whisper + BatchedInferencePipeline + VAD, как в
               проде; vulkan = whisper.cpp через whisper_cpp.make_adapter,
               ровно тот вызов, что делает индексатор на AMD-машине).

Примеры:
  python tools/bench_paths.py embed --batches 4
  python tools/bench_paths.py transcribe cuda   test_data\\speech_2min.wav
  python tools/bench_paths.py transcribe vulkan test_data\\speech_2min.wav
"""
import argparse
import copy
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def cmd_embed(args):
    from hds.chunker import make_chunks
    from hds.config import dig, load
    from hds.embedder import make_embedder

    cfg = load()
    bs = int(dig(cfg, "embedding.batch_size", 64))

    # Реальный текст проекта -> сегменты -> чанки (та же функция, что в индексере)
    with open("README.md", "r", encoding="utf-8") as f:
        text = f.read()
    segments = [{"text": text, "page": None, "t_start": None, "t_end": None}] * args.files
    chunks = make_chunks(segments, dig(cfg, "chunk.size", 1200),
                         dig(cfg, "chunk.overlap", 200))
    total = min(len(chunks), bs * args.batches)
    texts = [c["text"] for c in chunks[:total]]
    nchars = sum(len(t) for t in texts)
    print("[bench] LM Studio: %s | модель %s | batch_size=%d" %
          (dig(cfg, "embedding.base_url", ""), dig(cfg, "embedding.model", ""), bs))
    print("[bench] чанков: %d (%d симв., ~%d KB)" % (total, nchars, nchars // 1024))

    emb = make_embedder(cfg)
    t0 = time.perf_counter()
    emb.embed(["ping"])  # прогрев: JIT-загрузка модели в LM Studio + keep-alive
    print("[bench] прогрев (1 запрос, вкл. загрузку модели): %.2f с" %
          (time.perf_counter() - t0))

    # Ровно цикл indexer._commit_file
    times = []
    t0 = time.perf_counter()
    for i in range(0, total, bs):
        b = time.perf_counter()
        vecs = emb.embed([c for c in texts[i:i + bs]])
        times.append(time.perf_counter() - b)
        assert len(vecs) == len(texts[i:i + bs])
        assert len(vecs[0]) == int(dig(cfg, "embedding.dim", 1024))
    wall = time.perf_counter() - t0
    print("[bench] итого: %.2f с | %.1f мс/чанк | %.0f тыс.симв/с" %
          (wall, 1000.0 * wall / total, nchars / wall / 1000.0))
    for i, t in enumerate(times):
        print("[bench]   батч %2d: %6.2f с (%d чанков)" % (i + 1, t, min(bs, total - i * bs)))
    return 0


def cmd_transcribe(args):
    from hds.config import dig, load

    cfg = copy.deepcopy(load())
    cfg.setdefault("index", {})
    cfg["index"]["whisper_device"] = args.device
    if args.batch is not None:
        cfg["index"]["whisper_batch"] = args.batch

    from hds import extract_av

    print("[bench] device=%s | whisper_model=%s | whisper_batch=%s | файл=%s" % (
        args.device, dig(cfg, "index.whisper_model", "small"),
        dig(cfg, "index.whisper_batch", "?"), args.audio))
    durs = []
    for rep in range(1, args.repeats + 1):
        t0 = time.perf_counter()
        segs, err = extract_av._transcribe(args.audio, cfg, is_video=False)
        wall = time.perf_counter() - t0
        if err:
            print("[bench] повтор %d: ОШИБКА %.2f с: %s" % (rep, wall, err))
            return 1
        dur = segs[-1].get("t_end") if segs else 0.0
        durs.append((wall, dur))
        print("[bench] повтор %d: %.2f с | аудио %.1f с | RTF %.3f | сегментов %d" %
              (rep, wall, dur, wall / dur if dur else 0.0, len(segs)))
    warm = durs[1:]
    if warm:
        rtf = sum(w / d for w, d in warm) / len(warm)
        print("[bench] среднее по повторам 2..%d: RTF %.3f (1-й повтор вкл. загрузку модели)"
              % (len(durs), rtf))
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    pe = sub.add_parser("embed")
    pe.add_argument("--batches", type=int, default=4)
    pe.add_argument("--files", type=int, default=6,
                    help="сколько раз повторить README как 'файл' (для объёма чанков)")
    pe.set_defaults(fn=cmd_embed)

    pt = sub.add_parser("transcribe")
    pt.add_argument("device", choices=["cuda", "vulkan"])
    pt.add_argument("audio")
    pt.add_argument("--repeats", type=int, default=3)
    pt.add_argument("--batch", type=int, default=None,
                    help="переопределить index.whisper_batch (0 = без батчей)")
    pt.set_defaults(fn=cmd_transcribe)

    args = p.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
