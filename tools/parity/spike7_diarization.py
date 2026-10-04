"""Спайк 7 (PLAN_AUTO_TRANSCRIBE, задача T0.1): офлайн-диаризация движка.

Цель — зафиксировать ТОЧНЫЙ формат вывода движка в режиме `mode: transcript`
(транскрибация whisper + диаризация sortformer одним вызовом bridge-audio) до
начала реализации фичи «автотранскрибация» (PLAN_AUTO_TRANSCRIBE §5.3–5.4):

  * как помечаются реплики (`SPEAKER_00`), формат заголовка и таймкода;
  * спецметка неприсвоенной реплики (`UNASSIGNED`) — как выглядит;
  * расширение выходного файла (`md`) и правило его имени (`<stem>.md`);
  * поля JSON-ответа (`diarization`, `output`, `stats`, `speaker_spans`, `timings_sec`).

**Почему не `example-cli.exe`.** Спайки 5–6 гоняли движок стоковым `example-cli.exe`
из каталога движка. После внедрения нашего патча движка (02.10.2026) этот CLI
несовместим с патченными DLL (ABI структур SDK) и падает `0xC0000005`; со стоковыми
`*.orig` — `0xC06D007E`. Поэтому спайк идёт через **нашу** пробу
`crates\\hds-llama\\src\\bin\\diar_probe.rs` (публичный bridge-API — тот же путь, что у
`llm-host`). Отчёт: `T0_1_DIARIZATION_FORMAT.md`.

Результаты: `out/spike7_diarization.json`, сырые логи `out/spike7_*.log`/`.md`.
"""
import json
import os
import shutil
import subprocess
import wave

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
PROBE = os.path.join(ROOT, "target", "release", "diar_probe.exe")
DIAR_TMP = os.path.join(os.environ["LOCALAPPDATA"], "Temp", "hds_diar", "result.md")
JFK = os.path.join(ROOT, "test_data", "jfk.wav")
SPEECH_2MIN = os.path.join(ROOT, "test_data", "speech_2min.wav")
RU_60S = os.path.join(BASE, "fixtures", "русская_речь_60сек.wav")


def make_two_speaker_fixture(dst):
    """Склеить два разных голоса (EN + RU) в одну дорожку — для проверки SPEAKER_01."""
    if os.path.exists(dst):
        return dst
    out = wave.open(dst, "wb")
    out.setnchannels(1)
    out.setsampwidth(2)
    out.setframerate(16000)
    for p in (JFK, RU_60S):
        w = wave.open(p)
        out.writeframes(w.readframes(w.getnframes()))
        w.close()
    out.close()
    return dst


def run(label, audio, extra_env=None):
    """Один прогон `diar_probe`; возвращает сводку (тайминги, спикеры, файлы)."""
    log = os.path.join(OUT, "spike7_%s.log" % label)
    env = dict(os.environ)
    env.update(extra_env or {})
    with open(log, "w", encoding="utf-8", errors="replace") as f:
        r = subprocess.run([PROBE, audio], stdout=f, stderr=subprocess.STDOUT,
                           timeout=3600, env=env)
    text = open(log, encoding="utf-8", errors="replace").read()
    summary = {"rc": r.returncode, "log": os.path.basename(log)}
    for line in text.splitlines():
        if '"timings_sec"' in line:
            try:
                j = json.loads(line)
                summary["timings_sec"] = j.get("timings_sec")
                summary["stats"] = j.get("stats")
                summary["diarization"] = j.get("diarization")
                summary["output"] = j.get("output")
                summary["speakers"] = sorted({s["speaker"] for s in j.get("speaker_spans", [])})
            except json.JSONDecodeError:
                pass
        if line.startswith("--- dump ---"):
            summary["dump"] = line.split("---", 2)[-1].strip()
    if os.path.exists(DIAR_TMP):
        dst = os.path.join(OUT, "spike7_%s.md" % label)
        shutil.copy2(DIAR_TMP, dst)
        summary["md"] = os.path.basename(dst)
        md = open(dst, encoding="utf-8", errors="replace").read()
        summary["md_speakers"] = sorted({t for t in md.split()
                                         if t == "UNASSIGNED" or t.startswith("SPEAKER_")})
    print("=== %s: rc=%s ===" % (label, r.returncode))
    print("  timings: %s" % summary.get("timings_sec"))
    print("  speakers(json): %s  speakers(md): %s"
          % (summary.get("speakers"), summary.get("md_speakers")))
    return summary


def main():
    if not os.path.exists(PROBE):
        raise SystemExit("нет %s — соберите: cargo build -p hds-llama --release "
                         "--bin diar_probe" % PROBE)
    os.makedirs(OUT, exist_ok=True)
    two = make_two_speaker_fixture(os.path.join(OUT, "spike7_two_speakers.wav"))

    res = {
        "probe": PROBE,
        "note": "сторонний CLI example-cli.exe несовместим с патченными DLL; "
                "спайк идёт через нашу пробу diar_probe",
        "jfk": run("jfk", JFK),
        "two_speakers": run("two", two),
        "ru_60s": run("ru60", RU_60S),
        "speech_2min": run("speech2min", SPEECH_2MIN),
    }
    with open(os.path.join(OUT, "spike7_diarization.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike7_diarization.json")


if __name__ == "__main__":
    main()
