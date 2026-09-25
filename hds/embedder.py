"""Клиент эмбеддингов OpenAI-совместимого API (llama-server / Ollama / LM Studio).

По умолчанию — роль embedding llama-server (порт 8011, hds/llama_server.py);
любой OpenAI-совместимый сервер продолжает работать через embedding.base_url.
"""
import time

import requests


class EmbeddingError(RuntimeError):
    pass


class Embedder:
    def __init__(self, base_url, model, batch_size=32, timeout=600):
        self.base_url = base_url.rstrip("/")
        self.model = model
        self.batch = max(1, int(batch_size))
        self.timeout = timeout
        self.available = True  # выключается после первой ошибки, чтобы не тормозить поиск
        # keep-alive: переиспользование HTTP-соединения ускоряет эмбеддинги
        self._sess = requests.Session()

    def embed(self, texts):
        if not texts:
            return []
        out = []
        for i in range(0, len(texts), self.batch):
            out.extend(self._post(list(texts[i:i + self.batch])))
        return out

    def embed_query(self, text):
        return self.embed([text])[0]

    def ping(self):
        self.embed(["ping"])
        self.available = True
        return True

    def _post(self, batch):
        url = self.base_url + "/embeddings"
        last = None
        for attempt in range(4):
            try:
                r = self._sess.post(
                    url,
                    json={"model": self.model, "input": batch},
                    timeout=self.timeout,
                )
                if r.status_code == 200:
                    data = r.json().get("data", [])
                    data.sort(key=lambda d: d.get("index", 0))
                    if len(data) != len(batch):
                        raise EmbeddingError(
                            "ожидались %d векторов, пришло %d" % (len(batch), len(data))
                        )
                    return [d["embedding"] for d in data]
                last = "HTTP %s: %s" % (r.status_code, r.text[:300])
                if r.status_code in (400, 404):
                    break  # модель не установлена/не загружена — ретраи бессмысленны
            except Exception as e:  # noqa: BLE001
                last = repr(e)
            if attempt < 3:  # после последней попытки спать нечего
                time.sleep(2 * (attempt + 1))
        self.available = False
        low = (last or "").lower()
        if "exceed_context" in low or "context size" in low:
            hint = ("llama-server отвечает ЯВНОЙ ошибкой контекста: модель "
                    "загружена с ctx меньше длины входа. Запустите роль "
                    "менеджером (контекст задаётся llm_server.embedding."
                    "ctx_per_slot = 8192): python -m hds.llama_server "
                    "restart embedding.")
        elif "connection" in low or "max retries" in low or "failed to establish" in low:
            hint = ("Похоже, llama-server (роль embedding) не запущен: "
                    "python -m hds.llama_server start embedding "
                    "(или «Запустить» в группе «LLM-серверы» веб-интерфейса).")
        elif "model" in low and ("not found" in low or "no model" in low):
            hint = ("GGUF-модель не найдена на диске (общий llama-рантайм). "
                    "Скачайте её кнопкой «Скачать модель» в веб-интерфейсе "
                    "или установщиком рантайма (installers/"
                    "ensure_llama_runtime.ps1 -Models embedding — Windows, "
                    "bash installers/ensure_llama_runtime.sh --models "
                    "embedding — macOS).")
        else:
            hint = ("Проверьте состояние сервера: python -m hds.llama_server "
                    "status embedding (или карточка «Проверка компонентов» в UI).")
        raise EmbeddingError(
            "Эмбеддинги недоступны (модель '%s' на %s): %s. %s"
            % (self.model, self.base_url, last, hint)
        )


def make_embedder(cfg):
    from .config import dig

    return Embedder(
        dig(cfg, "embedding.base_url", "http://127.0.0.1:8011/v1"),
        dig(cfg, "embedding.model", "text-embedding-bge-m3"),
        dig(cfg, "embedding.batch_size", 64),
    )