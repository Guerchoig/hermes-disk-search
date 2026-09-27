"""Общие помощники для регрессионных тестов hermes-disk-search.

Изоляция: тесты используют временный config.yaml (через HDS_CONFIG) и временную
БД — реальные index.db, LM Studio и процессы не затрагиваются.
"""
import os
import sys

PROJECT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if PROJECT not in sys.path:
    sys.path.insert(0, PROJECT)

TEST_CFG = """# тестовый конфиг
index:
  roots: {roots}
  exclude_dirs: ["$RECYCLE.BIN", "System Volume Information", ".Trash", ".Trashes"]
  max_file_mb: 20
  max_media_mb: 50
  ocr: false
  transcribe: false
  whisper_model: tiny
  whisper_device: cpu
  whisper_compute: int8
  whisper_load_timeout: 30
  whisper_batch: 0
  hf_token: ""
  max_chunks: {max_chunks}
  clip: false
db_path: "{db_path}"
chunk:
  size: 300
  overlap: 50
embedding:
  base_url: "http://127.0.0.1:1/v1"
  model: "fake"
  batch_size: 4
  dim: 8
chat:
  base_url: "http://127.0.0.1:1/v1"
  model: "fake"
  temperature: 0.2
  max_context_chars: 4000
llm_server:
  host: "127.0.0.1"
  start_timeout: 1
  autostart: false
  chat:
    port: 1
    model: "models/chat/fake.gguf"
    ctx_per_slot: 512
    extra_args: ""
  embedding:
    port: 1
    model: "models/embedding/fake.gguf"
    ctx_per_slot: 512
    extra_args: ""
  rerank:
    port: 1
    model: "models/rerank/fake.gguf"
    ctx_per_slot: 512
    extra_args: ""
mcp_http:
  host: "127.0.0.1"
  port: 1
  path: "/mcp"
  autostart: false
  start_timeout: 1
search:
  vec_k: 10
  fts_k: 10
  rrf_k: 60
  snippet_chars: 200
watch:
  debounce_seconds: 1
  reconcile_on_start: false
  max_stable_wait: 5
{extra}"""


class FakeEmbedder:
    """Детерминированный эмбеддер без сети: вектор зависит от длины текста."""
    available = True
    model = "fake"

    def __init__(self, dim=8):
        self.dim = dim

    def embed(self, texts):
        return [[float(len(t) % 5 + 1)] * self.dim for t in texts]

    def embed_query(self, text):
        return self.embed([text])[0]

    def ping(self):
        return True


def write_config(tmpdir, max_chunks=2000, roots=None, extra=""):
    """Изолированный config.yaml во временном каталоге; активирует HDS_CONFIG."""
    import yaml

    path = os.path.join(tmpdir, "config.yaml")
    text = TEST_CFG.format(
        max_chunks=max_chunks,
        db_path=os.path.join(tmpdir, "index.db").replace("\\", "\\\\"),
        extra=extra,
        roots=yaml.safe_dump(
            [os.path.abspath(r) for r in (roots or [])],
            default_flow_style=True).strip(),
    )
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    os.environ["HDS_CONFIG"] = path
    return path


def write_text(tmpdir, name, text):
    os.makedirs(tmpdir, exist_ok=True)
    p = os.path.join(tmpdir, name)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8") as f:
        f.write(text)
    return p