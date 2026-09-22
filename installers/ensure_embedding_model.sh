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
# --context-length обязателен: при меньшем контексте LM Studio МОЛЧА обрезает
# вход длиннее контекста (вектор совпадает с вектором только первых токенов,
# без ошибки в ответе) — длинные фрагменты индексируются неполно.
if command -v lms >/dev/null 2>&1; then
    echo "[..] Загрузка модели в LM Studio (lms load text-embedding-bge-m3 --context-length 8192)..."
    if lms load text-embedding-bge-m3 --context-length 8192 -y >/dev/null 2>&1; then
        echo "[ok] Модель загружена в LM Studio (контекст 8192)"
    else
        echo "[--] Автозагрузка не удалась — загрузите модель в LM Studio:"
        echo "     Developer -> Select a model to load -> text-embedding-bge-m3"
    fi
    CTX=$(lms ps --json 2>/dev/null | grep -o '"contextLength":[0-9]*' | head -1 | cut -d: -f2)
    if [ -n "$CTX" ] && [ "$CTX" -lt 8192 ]; then
        echo "[!!] Фактический контекст модели: $CTX (должно быть 8192) — длинные"
        echo "     фрагменты будут обрезаны. Повторите вручную:"
        echo "     lms unload text-embedding-bge-m3; lms load text-embedding-bge-m3 --context-length 8192 -y"
    fi
fi

# Итоговая проверка: сервер отвечает И модель реально загружена
if curl -s -m 3 http://localhost:1234/v1/models 2>/dev/null | grep -q text-embedding-bge-m3; then
    echo "[ok] Эмбеддинги готовы: модель загружена в LM Studio"
elif curl -s -m 3 http://localhost:1234/v1/models >/dev/null 2>&1; then
    echo "[--] Файл модели на месте, но сервер её не загрузил."
    echo "     В LM Studio: Developer -> Select a model to load -> text-embedding-bge-m3,"
    echo "     или позже нажмите «Загрузить в LM Studio» в веб-интерфейсе."
else
    echo "[--] LM Studio не запущен. Установите (https://lmstudio.ai), запустите сервер"
    echo "     (Developer -> Start Server) и загрузите модель (Developer -> Load)."
    echo "     Файл модели уже скачан — в списке моделей LM Studio она появится."
fi
