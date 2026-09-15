# Релиз v0.4.0 — hermes-disk-search

## Новое

- **Поддержка macOS (Apple Silicon M1-M4)**:
  - Whisper-транскрипция без CUDA: устройство выбирается автоматически — на macOS
    `cuda` заменяется на CPU (`int8`), без ошибок «CUDA not available» и ложных
    fallback-таймаутов; экспериментальный Metal включается явно
    (`index.whisper_device: metal`, ctranslate2 ≥ 4.5);
  - автопоиск JDK для MS Project (.mpp) на macOS: Homebrew openjdk и
    `/Library/Java/JavaVirtualMachines`;
  - tessdata от Homebrew-Tesseract (`/opt/homebrew/share/tessdata`) подхватывается
    автоматически;
  - nvidia-* пакеты на macOS не ставятся (и не нужны).
- **Установщик Hermes для macOS**: `installers/install_hermes_macos.sh` — аналог
  `install_hermes.ps1`: регистрирует MCP-сервер в config.yaml Hermes, ставит скилл,
  выставляет `tools.tool_search.enabled: off`; идемпотентен, с YAML-валидацией;
  вызывается автоматически из `install_macos.command`.
- **Архивы релиза вместо отдельных иконок**: на странице релизов теперь два полных
  архива — `hermes-disk-search-<версия>-windows.zip` и
  `hermes-disk-search-<версия>-macos.zip` (исходники + иконки `assets/` + инсталляторы +
  ярлыки) и `HermesDiskSearchIndex.app.zip`. Отдельно иконки больше не выкладываются.
- **CI на две платформы**: job `test-macos` (macos-latest, Apple Silicon) — регрессионные
  тесты без CUDA блокируют релиз при провале; mac-архив собирается на macOS-раннере
  (права на исполнение `.command`/`run_index` сохраняются, `git archive`).

## Исправления

- **install_macos.command не падает на шаге копирования .app** (был `rm -rf "$APP"`
  с несуществующей переменной при `set -u`); добавлена опциональная предзагрузка
  модели Whisper и автоматическая интеграция с Hermes.
- Все mac-скрипты (`.command`, `.sh`, `run_index`) переведены на LF — bash на macOS
  не исполняет CRLF-файлы.
- `float16` на CPU заменяется на `int8` (ctranslate2 float16 на CPU не поддерживает).

## Требования к обновлению

- Миграция БД/индекса не требуется; переиндексация не нужна.
- На macOS: установщик `installers/install_macos.command` ставит всё сам; для .mpp
  нужна Java (`brew install openjdk`) — найдётся автоматически.
- Для транскрипции на macOS рекомендуется CPU (`int8`) — включается автоматически.
