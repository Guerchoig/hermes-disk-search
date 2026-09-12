"""Живой прогресс индексации: периодический статус + текущий файл.

- В интерактивном терминале: одна перерисовываемая строка (\\r).
- В файл/не-tty: полные строки раз в N секунд (чтобы логи читались).
"""
import collections
import os
import sys
import threading
import time


def _fmt_dur(sec):
    sec = int(sec)
    return "%02d:%02d:%02d" % (sec // 3600, sec % 3600 // 60, sec % 60)


def _human(n):
    return "%d %03d" % (n // 1000, n % 1000) if n >= 1000 else str(n)


class ProgressReporter:
    def __init__(self, sec=3, stream=None):
        self.sec = max(0, int(sec or 0))
        self.stream = stream or sys.stdout
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._th = None
        self._last_len = 0
        self.t0 = time.time()
        self.seen_count = 0
        self.processed_count = 0
        self.errors = 0
        self.chunks = 0
        self.by_kind = {}
        self.current = None      # (path, phase)
        self.current_since = None
        self.progress = None     # % обработки текущего файла (медиа)
        self.paused = False
        self.last_done = None    # (path, status, dur)
        self._last_path = None
        self.events = collections.deque(maxlen=50)  # последние обработанные файлы

    # --- управление ---
    @property
    def is_tty(self):
        try:
            return hasattr(self.stream, "isatty") and self.stream.isatty()
        except Exception:  # noqa: BLE001
            return False

    def start(self):
        if self.sec > 0:
            self._th = threading.Thread(target=self._loop, daemon=True)
            self._th.start()

    def stop(self):
        self._stop.set()
        if self._th:
            self._th.join(timeout=2)
            self._th = None
        self.note()

    # --- события из indexer ---
    def seen(self):
        with self._lock:
            self.seen_count += 1

    def processed(self, status, kind, dur, chunks=0):
        with self._lock:
            key = str(status).split("(")[0]
            if key.startswith("error"):
                self.errors += 1
            if key in ("indexed", "moved", "unchanged", "skipped_big", "skipped_type"):
                self.processed_count += 1
            self.chunks += chunks
            if kind:
                self.by_kind[kind] = self.by_kind.get(kind, 0) + 1
            self.current = None
            self.current_since = None
            self.progress = None
            self.events.appendleft({
                "path": self._last_path, "status": key, "kind": kind,
                "dur": round(dur, 2), "chunks": chunks, "ts": time.time(),
            })

    def set_last_path(self, path):
        with self._lock:
            self._last_path = path

    def set_paused(self, paused):
        """Пауза индексации (файл-сигнал index.pause)."""
        with self._lock:
            self.paused = bool(paused)

    def heartbeat_data(self):
        """Снимок счётчиков для кросс-процессного heartbeat-файла."""
        with self._lock:
            d = {"seen": self.seen_count, "processed": self.processed_count,
                 "errors": self.errors, "chunks": self.chunks,
                 "paused": self.paused,
                 "elapsed": round(time.time() - self.t0, 1),
                 "rate_min": round(self.seen_count / max(0.001, time.time() - self.t0) * 60)}
            if self.current:
                d["path"], d["phase"] = self.current
                d["progress"] = self.progress
            return d

    def set_current(self, path, phase):
        with self._lock:
            self.current = (path, phase)
            self.current_since = time.time()
            self.progress = None

    def set_progress(self, pct):
        """Прогресс внутри текущего файла, % (для Whisper-транскрипции)."""
        with self._lock:
            if self.current:
                self.progress = max(0.0, min(100.0, float(pct)))

    def note(self):
        """Очистить живую строку перед печатью обычной строки лога."""
        if self.is_tty and self._last_len:
            self.stream.write("\r" + " " * self._last_len + "\r")
            self.stream.flush()
            self._last_len = 0

    # --- отрисовка ---
    def _status_line(self, final=False):
        with self._lock:
            rate = self.seen_count / max(0.001, time.time() - self.t0) * 60.0
            line = "⏱ %s | просмотрено %s | обработано %d | ошибок %d | %.0f файлов/мин" % (
                _fmt_dur(time.time() - self.t0), _human(self.seen_count),
                self.processed_count, self.errors, rate)
            if self.paused:
                line = "⏸ ПАУЗА | " + line
            if self.current:
                path, phase = self.current
                age = _fmt_dur(time.time() - (self.current_since or time.time()))
                tail = " | ▶ %s [%s, идёт %s]" % (os.path.basename(path), phase, age)
                if self.progress is not None:
                    tail = " | ▶ %s [%s — %.0f%%, идёт %s]" % (
                        os.path.basename(path), phase, self.progress, age)
            elif self.last_done and not final:
                path, status, dur = self.last_done
                tail = " | ✓ %s (%s, %.1f с)" % (os.path.basename(path), status, dur)
            else:
                tail = ""
            return line + tail

    def _render(self, final=False):
        line = self._status_line(final=final)
        if self.is_tty and not final:
            width = 120
            out = line[:width].ljust(max(self._last_len, len(line[:width])))
            self.stream.write("\r" + out)
            self.stream.flush()
            self._last_len = len(out)
        else:
            self.note()
            self.stream.write(line + "\n")
            self.stream.flush()

    def _loop(self):
        while not self._stop.wait(self.sec or 3):
            try:
                self._render()
            except Exception:  # noqa: BLE001
                pass

    def finish(self, counters=None):
        self.final_elapsed = round(time.time() - self.t0, 1)
        self.stop()
        self._render(final=True)
        if counters is not None:
            self.stream.write("[done] %s\n" % counters)
            self.stream.flush()


# Фазы по типам файлов — для показа «чем занят индексатор»
PHASES = {
    "media": "видео/аудио → ffmpeg + Whisper",
    "pdf": "PDF → текст/OCR",
    "image": "картинка → OCR",
    "mpp": "MS Project → разбор задач",
    "docx": "Word-документ",
    "xlsx": "Excel-таблица",
    "pptx": "PowerPoint-презентация",
    "text": "текст/код",
}