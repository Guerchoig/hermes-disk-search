#!/bin/bash
# Инсталлятор hermes-disk-search для macOS.
# LM Studio и Hermes НЕ устанавливает — предупреждает и даёт ссылки.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
echo "== hermes-disk-search: установка (macOS) =="

have() { command -v "$1" >/dev/null 2>&1; }

# --- 1. Homebrew ---
if ! have brew; then
    echo "[--] Homebrew не найден."
    echo "    Установите: /bin/bash -c \"\$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\""
    echo "    (требуются Command Line Tools: xcode-select --install)"
    read -p "Продолжить без brew (только venv)? [y/N] " a
    [ "$a" = "y" ] || exit 1
else
    echo "[ok] Homebrew найден"
    # --- 2. Python ---
    have python3 || { echo "[..] python3 -> brew install python"; brew install python; }
    # --- 3. ffmpeg ---
    have ffmpeg || { echo "[..] ffmpeg -> brew install ffmpeg"; brew install ffmpeg; }
    # --- 4. Tesseract OCR (опционально) ---
    read -p "Установить Tesseract OCR (текст на картинках)? [y/N] " a
    if [ "$a" = "y" ]; then
        have tesseract || brew install tesseract
        brew list tesseract-lang >/dev/null 2>&1 || brew install tesseract-lang
    fi
fi

# --- 5. LM Studio (не устанавливаем!) ---
if curl -s -m 3 http://localhost:1234/v1/models >/dev/null 2>&1; then
    echo "[ok] LM Studio запущен (localhost:1234)"
else
    echo "[!!] LM Studio не отвечает на localhost:1234"
    echo "     1) Установите: https://lmstudio.ai"
    echo "     2) Developer -> Start Server"
    echo "     3) Скачайте чат-модель и embedding text-embedding-bge-m3"
    echo "        (https://huggingface.co/lm-kit/bge-m3-gguf), загрузите модель"
fi

# --- 6. venv ---
if [ ! -x "$ROOT/.venv/bin/python" ]; then
    python3 -m venv "$ROOT/.venv"
fi
"$ROOT/.venv/bin/python" -m pip install --upgrade pip -q
"$ROOT/.venv/bin/python" -m pip install -r "$ROOT/requirements.txt" -q
"$ROOT/.venv/bin/python" -m pip install faster-whisper -q
echo "[ok] зависимости установлены"

# --- 6.1. Модель эмбеддингов bge-m3 (автоскачивание, ~1,2 ГБ, если не установлена) ---
bash "$ROOT/installers/ensure_embedding_model.sh"

# --- 6.2. Metal для транскрипции (mlx-whisper, только Apple Silicon) ---
if [ "$(uname -m)" = "arm64" ]; then
    echo "[..] Apple Silicon: подключаю Metal-ускорение транскрипции (mlx-whisper)..."
    if "$ROOT/.venv/bin/python" -m pip install mlx-whisper -q; then
        echo "[ok] mlx-whisper установлен — транскрипция через Metal"
        echo "[..] Предзагрузка Whisper-модели для Metal (mlx-community/whisper-small-mlx)..."
        "$ROOT/.venv/bin/python" -c "from huggingface_hub import snapshot_download; snapshot_download('mlx-community/whisper-small-mlx'); print('[ok] модель для Metal готова')" \
            || echo "[--] Не удалось сейчас — скачается при первой транскрипции"
    else
        echo "[--] mlx-whisper не установился — транскрипция на CPU (faster-whisper int8)"
    fi
else
    echo "[--] Процессор $(uname -m) не Apple Silicon — транскрипция на CPU (faster-whisper int8)"
fi

# --- 7. Ярлык .app и права ---
if [ -d "$ROOT/shortcuts/macos/HermesDiskSearchIndex.app" ]; then
    rm -rf "$HOME/Applications/HermesDiskSearchIndex.app" 2>/dev/null
    rm -rf "/Applications/HermesDiskSearchIndex.app" 2>/dev/null
    cp -R "$ROOT/shortcuts/macos/HermesDiskSearchIndex.app" "$HOME/Applications/" 2>/dev/null \
        || cp -R "$ROOT/shortcuts/macos/HermesDiskSearchIndex.app" /Applications/ 2>/dev/null || true
    chmod +x "$ROOT/shortcuts/macos/Индексация дисков.command" 2>/dev/null
    chmod +x "$ROOT/shortcuts/macos/HermesDiskSearchIndex.app/Contents/MacOS/run_index" 2>/dev/null
    echo "[ok] Приложение «HDS Индексация» установлено в ~/Applications"
fi

# --- 7.1. Предзагрузка модели Whisper (опционально, ~460 МБ) ---
read -p "Предзагрузить модель Whisper (small, ~460 МБ, для транскрипции)? [y/N] " a
if [ "$a" = "y" ]; then
    HF_HUB_OFFLINE=1 "$ROOT/.venv/bin/python" -c \
        "import sys; sys.path.insert(0, r'$ROOT'); from hds.config import load; from hds.extract_av import _get_whisper; _get_whisper(load()); print('[ok] модель Whisper готова')" \
        || echo "[--] Не удалось: модель скачается при первой транскрипции"
fi

# --- 8. Интеграция с Hermes Desktop (MCP-сервер + скилл) ---
# Не обязательна на этом шаге: если Hermes ещё не установлен, скрипт напечатает,
# как подключить позже, и завершится успешно.
if [ -f "$ROOT/installers/install_hermes_macos.sh" ]; then
    bash "$ROOT/installers/install_hermes_macos.sh" || true
fi

# --- 9. Диагностика ---
cd "$ROOT" && "$ROOT/.venv/bin/python" -m hds.cli check
echo ""
echo "== Готово. Первичная индексация: запустите «Индексация дисков.command» =="