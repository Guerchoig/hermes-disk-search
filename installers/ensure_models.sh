#!/bin/bash
# Скачивание GGUF-моделей llama-server в папку проекта models/:
#   embedding: bge-m3-Q8_0 (~1,2 ГБ)
#   chat:      qwen3.5-9b Q6_K (~7,5 ГБ, unsloth/Qwen3.5-9B-Instruct-GGUF)
# Идемпотентно: существующий файл не перекачивается. Если модель уже скачана
# в ~/.lmstudio (старая установка), копируется оттуда без повторной загрузки.
# Используется install_hermes_macos.sh / install_macos.command.
# Аналог для Windows: ensure_models.ps1
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

ensure_model() {   # dir file minMB url
    local dir="$1" file="$2" minMB="$3" url="$4"
    local dest="$ROOT/models/$dir/$file"
    if [ -f "$dest" ]; then
        echo "[ok] $file уже установлен: $dest"
        return 0
    fi
    mkdir -p "$(dirname "$dest")"

    # Быстрый путь: копирование из старой установки LM Studio (~/.lmstudio)
    local lms="$HOME/.lmstudio/models"
    if [ -d "$lms" ]; then
        local old
        old=$(find "$lms" -type f \( -name "$file" -o -name "*Q6_K*.gguf" \) \
              -size +${minMB}M 2>/dev/null | head -1)
        if [ -n "$old" ]; then
            echo "[..] Найдена модель из LM Studio: $old — копирую в models/..."
            cp "$old" "$dest"
            echo "[ok] Скопировано: $dest"
            return 0
        fi
    fi

    echo "[..] Скачиваю $file (разово, ~$((minMB / 1000)) ГБ)..."
    echo "     $url"
    if curl -L --fail --progress-bar -o "$dest.part" "$url" \
            && [ "$(stat -f%z "$dest.part" 2>/dev/null || stat -c%s "$dest.part" 2>/dev/null || echo 0)" -gt $((minMB * 1024 * 1024)) ]; then
        mv "$dest.part" "$dest"
        echo "[ok] Скачано: $dest"
    else
        rm -f "$dest.part"
        echo "[!!] Не удалось скачать $file. Скачайте вручную:" >&2
        echo "     $url" >&2
        echo "     и положите в $dest" >&2
        return 1
    fi
}

ensure_model embedding bge-m3-Q8_0.gguf 500 \
    "https://huggingface.co/lm-kit/bge-m3-gguf/resolve/main/bge-m3-Q8_0.gguf"
ensure_model chat qwen3.5-9b-Q6_K.gguf 4000 \
    "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q6_K.gguf"