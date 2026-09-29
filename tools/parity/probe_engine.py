"""Спайк-разведка движка openresearchtools/engine через официальный example-cli (W0 §4 п.8).

Печатает справку подкоманд и список устройств (cluster API изнутри движка), вывод
дублируется в tools/parity/out/engine_probe_*.txt для журнала SPIKES.md.
"""
import os
import subprocess
import sys

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "out")
ENGINE = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "TranscribeOffline",
                      "Engine")
CLI = os.path.join(ENGINE, "example-cli.exe")


def run(args, timeout=180):
    """Запуск example-cli из каталога движка (нужен search path для зависимых DLL)."""
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    r = subprocess.run([CLI] + args, cwd=ENGINE, capture_output=True, timeout=timeout)
    out = (r.stdout or b"").decode("utf-8", "replace")
    err = (r.stderr or b"").decode("utf-8", "replace")
    return r.returncode, out, err


def dump(name, args, timeout=180):
    print("=== %s ===" % " ".join(args))
    try:
        rc, out, err = run(args, timeout)
    except subprocess.TimeoutExpired:
        print("TIMEOUT")
        return
    print("rc=%s" % rc)
    text = out.strip() or err.strip()
    print(text[:4000])
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, "engine_probe_%s.txt" % name), "w",
              encoding="utf-8") as f:
        f.write("args: %s\nrc=%s\n--- stdout ---\n%s\n--- stderr ---\n%s"
                % (" ".join(args), rc, out, err))
    print()


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    dump("help", ["help"])
    dump("list_devices", ["list-devices"], timeout=300)
    for sub in ("audio", "embed", "rerank", "chat"):
        dump("help_" + sub, ["bridge", sub, "--help"])
