# RELEASE_NOTES 0.9.0 — переход с LM Studio на llama-server (llama.cpp)

## Breaking change

Локальный LLM-бэкенд — теперь **llama.cpp (llama-server)** вместо LM Studio.
LM Studio как HTTP-бэкенд продолжает работать, если он уже запущен (клиенты —
generic OpenAI-совместимые), но установщики и код проекта больше его не
настраивают и не проверяют.

## Новое

- **Менеджер llama-server `hds/llama_server.py`** (порт `anonymizer_proxy/llm_server.py`):
  три роли «одна GGUF-модель — один llama-server»:
  - `chat` (порт 8010) — RAG-ответы ask_my_files; БЕЗ `--jinja`: инструменты
    вызывает агент, а не модель (дэдлоки невозможны);
  - `embedding` (порт 8011, `--embedding --pooling cls` — пулинг сверен замером);
  - `rerank` (порт 8012, `--reranking --pooling rank`).
  CLI: `python -m hds.llama_server check|start|stop|status|restart|run [role]`.
  Отвязанный запуск (переживает перезапуск UI/MCP), PID-файлы, логи
  `data/logs/llama_<role>.log`, probe с проверкой модели (/props → чужой llama
  с другой моделью не переиспользуется).
- **Очередь**: `--parallel 1` у всех инстансов — ровно один запрос в работе,
  остальные ждут в очереди llama-server.
- **Тонкое управление thinking** (главная мотивация миграции): `chat.thinking: off`
  шлёт `"chat_template_kwargs": {"enable_thinking": false}` per-request;
  `auto` — размышления разрешены, клиенту уходит только финальный текст,
  при «пустом» ответе (модель закончила внутри размышлений) — nudge-дозапрос.
- **Контексты по умолчанию**: chat 16384 (KV-кэш q8_0), embedding 8192
  (требование `EMB_CONTEXT`), rerank 8192; `--ctx-size = parallel × ctx_per_slot`.
  Явные ошибки 400 по контексту вместо тихого усечения.
- **Автозапуск**: `llm_server.autostart` — chat+embedding поднимаются в фоне при
  старте UI (`hds.ui_server.run`), MCP-сервера (`hds/mcp_server.run`), CLI
  `index`/`ask`.
- **Веб-интерфейс**: группа «Модель эмбеддингов» → «LLM-серверы»: статусы
  ролей (pid/модель/контекст/слоты), кнопки запуска/остановки, скачивание
  GGUF в `models/` проекта. Новые API: `/api/llama/start`, `/api/llama/stop`.

## Установщики

- **Windows (`setup.ps1`)**: шаг установки llama.cpp — пре-билд с GitHub Releases
  (`ggml-org/llama.cpp`: CUDA при nvidia-smi, иначе Vulkan) в `tools/llama.cpp/`.
- **macOS (`install_macos.command`)**: `brew install llama.cpp` (Metal автоматически
  на Apple Silicon).
- **Модели**: `installers/ensure_models.ps1|.sh` скачивают bge-m3 Q8_0 (~1,2 ГБ) и
  qwen3.5-9b Q6_K (~7,5 ГБ, unsloth/Qwen3.5-9B-Instruct-GGUF) в `models/{embedding,chat}/`;
  идемпотентно; уже скачанные в `~/.lmstudio` копируются оттуда. Старые
  `ensure_embedding_model.ps1|.sh` удалены (lms load больше не нужен).

## Конфигурация

- Новая секция `llm_server` (bin/host/autostart/parallel + параметры ролей);
  `embedding.base_url` → `http://127.0.0.1:8011/v1`, `chat.base_url` →
  `http://127.0.0.1:8010/v1`, `chat.model` → `qwen3.5-9b` (совпадает с `--alias`),
  `chat.thinking: off`.
- Имя модели каждому инстансу задаётся `--alias` из config.yaml — имя модели
  больше не зависит от имени GGUF-файла (источник рассинхрона устранён).

## Миграция существующих установок

- старый config.yaml с `localhost:1234` работает, пока LM Studio запущен;
  диагностика подскажет переход;
- модели можно перенести из LM Studio без повторной загрузки: инсталлятор
  копирует их из `~/.lmstudio`, либо кнопкой «Скачать модель» в UI;
- после установки llama.cpp выполните `python -m hds.llama_server start all`.

## Исправления и инфраструктура (code review)

- **CLIP-векторы картинок** создаются при любой точке входа индексации
  (watcher, `reindex_path`), а не только при полном прогоне — новые фото
  сразу участвуют в контентном поиске «найди изображения …»;
- устранены гонки переименований: hash-move срабатывает только при
  исчезнувшем исходном пути (дубликаты контента больше не «перетягивают»
  запись индекса друг у друга); перезапись файла (move на существующий
  путь) больше не падает на UNIQUE(path);
- `kinds`-фильтр поиска пробрасывается в FTS- и векторные ветки —
  выдача по типу файла больше не пустеет при существующих совпадениях;
- таймаут RAG-ответов согласован с клиентом (`chat.timeout`, по умолчанию
  240 с вместо 600+600 с «молчаливого» зависания ask_my_files);
- защита от параллельной индексации в разных процессах (guard по
  heartbeat) в `index` и MCP `start_indexing`;
- guard частично скачанной модели Whisper (обрыв сети не считается
  «модель готова»);
- **config.yaml больше не в репозитории** (личные пути пользователя):
  добавлен `config.example.yaml`, при первом запуске конфиг создаётся
  автоматически; инсталляторы устойчивы к его отсутствию;
- новый CI-workflow (тесты на push/PR, Windows + macOS); релизы при
  выпуске новых версий больше **не удаляются** (накапливаются).