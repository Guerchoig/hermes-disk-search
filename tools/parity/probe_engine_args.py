"""Эмпирический подбор аргументов подкоманд example-cli (спайки 5–6 W0).

Подкоманды не поддерживают --help, зато печатают, какого аргумента не хватает:
перебираем варианты и смотрим, какой проходит. Результат — в out/engine_args.txt.
"""
import os
import subprocess

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "out")
ENGINE = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "TranscribeOffline", "Engine")
CLI = os.path.join(ENGINE, "example-cli.exe")
LLAMA = os.path.join(os.environ["LOCALAPPDATA"], "llama-runtime", "models")
BGE = os.path.join(LLAMA, "embedding", "bge-m3-Q8_0.gguf")
RRK = os.path.join(LLAMA, "rerank", "bge-reranker-v2-m3-q8_0.gguf")
Q6 = os.path.join(LLAMA, "chat", "Qwen3.5-9B-Q6_K.gguf")
TURBO = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "models",
                    "openresearchtools__whisper-large-v3-turbo-GGML",
                    "whisper-large-v3-turbo-GGML.bin")
JFK = os.path.join(os.path.dirname(os.path.dirname(BASE)), "test_data", "jfk.wav")

CASES = [
    ("embed_body_json", ["bridge", "embed", "--model", BGE, "--body-json",
                         '{"input":["тест"],"model":"bge-m3","n_gpu_layers":0}']),
    ("embed_body_json_min", ["bridge", "embed", "--model", BGE, "--body-json",
                             '{"input":["тест"]}']),
    ("embed_text", ["bridge", "embed", "--model", BGE, "--text", "тест"]),
    ("rerank_body_json", ["bridge", "rerank", "--model", RRK, "--body-json",
                          '{"query":"документооборот","documents":["архив","погода"],'
                          '"n_gpu_layers":0}']),
    ("audio_body_json_cpu", ["bridge", "audio", "--audio-file", JFK, "--mode", "speech",
                             "--custom", "default", "--whisper-model", TURBO,
                             "--body-json", '{"whisper_no_gpu":true}']),
]


def main():
    os.makedirs(OUT, exist_ok=True)
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    log = []
    for name, args in CASES:
        try:
            r = subprocess.run([CLI] + args, cwd=ENGINE, capture_output=True,
                               timeout=900, env=env)
            text = ((r.stdout or b"").decode("utf-8", "replace") + "\n" +
                    (r.stderr or b"").decode("utf-8", "replace")).strip()
        except subprocess.TimeoutExpired:
            r, text = None, "TIMEOUT (>900 c)"
        rc = "timeout" if r is None else r.returncode
        head = "\n".join(text.splitlines()[:12])
        print("=== %s: rc=%s ===" % (name, rc))
        print(head[:1500])
        print()
        log.append("=== %s (rc=%s) ===\nargs: %s\n%s\n" % (name, rc, args, text[:3000]))
    with open(os.path.join(OUT, "engine_args.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(log))


if __name__ == "__main__":
    main()