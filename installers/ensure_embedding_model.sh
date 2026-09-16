#!/bin/bash
# Скачивание embedding-модели bge-m3 (GGUF, ~1,2 ГБ) в папку моделей LM Studio,
# если файл ещё не на месте, и попытка загрузить её через lms (best effort).
# Используется install_macos.command. Аналог для Windows: ensure_embedding_model.ps1
set -u

GGUF="$HOME/.lmstudio/models/lm-kit/bge-m3-gguf/bge-m3-Q8_0.gguf"

if [ -f "$GGUF" ]; then
    echo "[ok] Модель эмбеддингов bge-m3 уже установлена: $GGUF"
else
    echo "[..] Модель эмбеддингов bge-m3 не найдена — скачиваю (~1,2 ГБ, разово)..."
    echo "     https://huggingface.co/lm-kit/bge-m3-gguf"
    mkdir -p "$(dirname "$GGUF")"
    if curl -L --fail --progress-bar -o "$GGUF.part" \
         "https://huggingface.co/lm-kit/bge-m3-gguf/resolve/main/bge-m3-Q8_0.gguf" \
         && [ -s "$GGUF.part" ]; then
        mv "$GGUF.part" "$GGUF"
        echo "[ok] Модель bge-m3 скачана: $GGUF"
    else
        rm -f "$GGUF.part"
        echo "[--] Не удалось скачать модель. Скачайте bge-m3-Q8_0.gguf вручную с"
        echo "     https://huggingface.co/lm-kit/bge-m3-gguf"
        echo "     и положите в $GGUF"
        echo "     (можно позже из веб-интерфейса: кнопка «Скачать модель»)."
    fi
fi

# Попытка загрузить модель в LM Studio через lms CLI (best effort)
if command -v lms >/dev/null 2>&1; then
    echo "[..] Загрузка модели в LM Studio (lms load text-embedding-bge-m3)..."
    if lms load text-embedding-bge-m3 -y >/dev/null 2>&1; then
        echo "[ok] Модель загружена в LM Studio"
    else
        echo "[--] Автозагрузка не удалась — загрузите модель в LM Studio:"
        echo "     Developer -> Select a model to load -> text-embedding-bge-m3"
    fi
fi

# Итоговая проверка сервера
if curl -s -m 3 http://localhost:1234/v1/models >/dev/null 2>&1; then
    echo "[ok] LM Studio отвечает на localhost:1234"
else
    echo "[--] LM Studio не запущен: установите (https://lmstudio.ai), запустите"
    echo "     сервер (Developer -> Start Server) и загрузите модель"
fi
