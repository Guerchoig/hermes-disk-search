#!/bin/bash
# Инсталлятор hermes-disk-search для macOS.
# LM Studio и Hermes НЕ устанавливает — предупреждает и даёт ссылки.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
echo "== hermes-disk-search: установка (macOS) =="

# --- 0. Снятие карантина Gatekeeper (com.apple.quarantine) ---
# Файлы, распакованные из скачанного браузером архива, получают карантинную
# метку; без её снятия macOS блокирует неподписанные бинарники и .command
# («повреждён» / «не удаётся открыть»). Скрипт запускается из Терминала —
# это разрешено, поэтому снять карантин можно уже здесь.
if command -v xattr >/dev/null 2>&1; then
    echo "[..] Снятие карантина Gatekeeper с файлов проекта..."
    xattr -dr com.apple.quarantine "$ROOT" 2>/dev/null || true
fi

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

# --- 5. llama.cpp (llama-server) — локальный LLM-бэкенд ---
# Ставим через Homebrew (как раньше); в общий llama-рантайм машины бинарь
# подключит шаг 6.1 ссылкой — оттуда его берут все проекты (hds.llama_server
# через hds/llama_runtime.py). Без brew бинарь можно и не ставить здесь:
# шаг 6.1 скачает готовую сборку llama.cpp с GitHub Releases.
if have llama-server; then
    echo "[ok] llama-server найден в PATH: $(command -v llama-server)"
elif [ -x "/opt/homebrew/bin/llama-server" ] || [ -x "/usr/local/bin/llama-server" ]; then
    echo "[ok] llama-server установлен через Homebrew"
elif have brew; then
    echo "[..] Устанавливаю llama.cpp (Metal включён автоматически для Apple Silicon)..."
    brew install llama.cpp
    have llama-server || { echo "[--] llama-server не установился — скачайте с https://github.com/ggml-org/llama.cpp/releases"; }
else
    echo "[--] Homebrew не найден — llama.cpp не установлен."
    echo "    Установите brew (https://brew.sh/) и выполните: brew install llama.cpp"
    echo "    или скачайте бинарь с https://github.com/ggml-org/llama.cpp/releases"
    echo "    и укажите путь в config.yaml (llm_server.bin)"
fi
have llama-server && llama-server --version 2>/dev/null | head -1

# --- 6. venv ---
if [ ! -x "$ROOT/.venv/bin/python" ]; then
    python3 -m venv "$ROOT/.venv"
fi
"$ROOT/.venv/bin/python" -m pip install --upgrade pip -q
"$ROOT/.venv/bin/python" -m pip install -r "$ROOT/requirements.txt" -q
"$ROOT/.venv/bin/python" -m pip install faster-whisper -q
echo "[ok] зависимости установлены"

# --- 6.1. Общий llama-рантайм машины (llama-server + GGUF-модели) ---
# Единый с anonymizer_proxy каталог: бинарь llama.cpp (Homebrew или пре-билд
# с GitHub Releases) и модели chat/embedding/rerank лежат в
# ~/Library/Application Support/llama-runtime (переопределяется
# LLAMA_RUNTIME_DIR). Модели в папку проекта больше не скачиваются.
# Идемпотентно: повторный запуск (в т.ч. установщиком второго проекта) ничего
# не докачивает; проект регистрируется в projects.json — смена общей
# чат-модели перезапускает его llama-инстансы.
bash "$ROOT/installers/ensure_llama_runtime.sh" \
    --models chat,embedding,rerank \
    --project-name hermes-disk-search \
    --project-root "$ROOT" \
    --restart-args "-m hds.llama_server restart chat" \
    || echo "[--] Общий llama-рантайм не готов — поиск по ключевым словам работает и без него"

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
    # снять карантин и подписать копию приложения ad-hoc на целевой машине:
    # bundle в mac-архиве собирается из git-коммита и не подписан, а кодовые
    # подписи нельзя закоммитить — поэтому инсталлятор подписывает копию сам
    # (codesign входит в macOS, ad-hoc бесплатна)
    xattr -dr com.apple.quarantine "$HOME/Applications/HermesDiskSearchIndex.app" 2>/dev/null || true
    codesign --force --deep --sign - "$HOME/Applications/HermesDiskSearchIndex.app" 2>/dev/null || true
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

# --- 8.1. Интеграция с Cline Desktop (MCP-сервер + скилл) ---
if [ -f "$ROOT/installers/install_cline_macos.sh" ]; then
    bash "$ROOT/installers/install_cline_macos.sh" || true
fi

# --- 9. Диагностика ---
cd "$ROOT" && "$ROOT/.venv/bin/python" -m hds.cli check
echo ""
echo "== Готово. Первичная индексация: запустите «Индексация дисков.command» =="