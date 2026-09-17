"""Клиент эмбеддингов OpenAI-совместимого API (LM Studio / Ollama)."""
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
        # keep-alive: каждое НОВОЕ соединение с LM Studio стоит ~2 с (замер),
        # переиспользование соединения ускоряет эмбеддинги в разы
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
            time.sleep(2 * (attempt + 1))
        self.available = False
        low = (last or "").lower()
        if "no models loaded" in low or "400" in low:
            hint = ("Модель скачана, но не загружена в LM Studio: веб-интерфейс → "
                    "карточка «Модель эмбеддингов» → «Загрузить в LM Studio», "
                    "или в LM Studio: Developer → Select a model to load → %s."
                    % self.model)
        elif "connection" in low or "max retries" in low or "failed to establish" in low:
            hint = ("Похоже, LM Studio не запущен: запустите LM Studio и включите "
                    "сервер (Developer → Start Server).")
        else:
            hint = ("Проверьте состояние модели в веб-интерфейсе: карточка "
                    "«Модель эмбеддингов».")
        raise EmbeddingError(
            "Эмбеддинги недоступны (модель '%s' на %s): %s. %s"
            % (self.model, self.base_url, last, hint)
        )


def make_embedder(cfg):
    from .config import dig

    return Embedder(
        dig(cfg, "embedding.base_url", "http://localhost:1234/v1"),
        dig(cfg, "embedding.model", "bge-m3"),
        dig(cfg, "embedding.batch_size", 64),
    )