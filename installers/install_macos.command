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

# --- 7. Ярлык .app и права ---
if [ -d "$ROOT/shortcuts/macos/HermesDiskSearchIndex.app" ]; then
    APP_DST="/Applications/HermesDiskSearchIndex.app"
    rm -rf "$APP" 2>/dev/null || rm -rf "$HOME/Applications/HermesDiskSearchIndex.app"
    cp -R "$ROOT/shortcuts/macos/HermesDiskSearchIndex.app" "$HOME/Applications/" 2>/dev/null \
        || cp -R "$ROOT/shortcuts/macos/HermesDiskSearchIndex.app" /Applications/ 2>/dev/null || true
    chmod +x "$ROOT/shortcuts/macos/Индексация дисков.command" 2>/dev/null
    chmod +x "$ROOT/shortcuts/macos/HermesDiskSearchIndex.app/Contents/MacOS/run_index" 2>/dev/null
    echo "[ok] Приложение «HDS Индексация» установлено в ~/Applications"
fi

# --- 8. Диагностика ---
cd "$ROOT" && "$ROOT/.venv/bin/python" -m hds.cli check
echo ""
echo "== Готово. Первичная индексация: запустите «Индексация дисков.command» =="