# RELEASE_NOTES v0.10.0 — общий llama-рантайм машины и смена чат-модели

## Новое

### Общий llama-рантайм: одна сборка llama.cpp и один набор GGUF на машину

llama-server и GGUF-модели больше не лежат в папке проекта — они в общем каталоге
машины, общем с `anonymizer_proxy` (переопределяется `LLAMA_RUNTIME_DIR`):

| ОС | Каталог рантайма |
|----|------------------|
| Windows | `%LOCALAPPDATA%\llama-runtime` |
| macOS | `~/Library/Application Support/llama-runtime` |
| Linux | `~/.local/share/llama-runtime` |

Раскладка: `bin\` (llama-server + DLL, **одна** сборка cuda|vulkan),
`models\{chat,embedding,rerank}\`, `projects.json` (реестр проектов),
`version.json` (вариант сборки), `models\chat\current.json` (манифест активной
чат-модели).

- **`shared:<role>`** в `llm_server.<role>.model` (дефолт `hds/config.py` и
  `config.example.yaml`): путь к GGUF берётся из манифеста рантайма — при
  обновлении моделей конфиг править не нужно. Абсолютный путь к файлу —
  escape-hatch (приоритетнее `shared:`).
- **`hds/llama_runtime.py`** — модуль рантайма (SYNC-COPY с
  `anonymizer_proxy/llama_runtime.py`, хэши совпадают): пути по ОС,
  `find_binary()`, `resolve_model()`, пресеты чат-моделей, `switch_chat_model()`,
  `register_project()`, обзор для UI. CLI:
  `python -m hds.llama_runtime list|dir|switch|download|register`.
- **Поиск бинаря** (`hds/llama_server.py`): `llm_server.bin` → общий рантайм →
  `tools\llama.cpp` → PATH; в текстах ошибок — установщик текущей ОС
  (`install_hint()`).

### Смена чат-модели одной командой — видна обоим проектам сразу

- **Веб-интерфейс**: в карточке «LLM-серверы» — виджет «Чат-модель»: селект
  скачанных файлов и пресетов («скачать»), кнопка «Применить и перезапустить»,
  поллинг прогресса. Новые API: `GET /api/chat-model`,
  `POST /api/chat-model/set` (фоновая смена: запись в манифест + перезапуск
  chat-инстанса этого проекта и llama-инстансов всех проектов из
  `projects.json`).
- **CLI**: `python -m hds.llama_runtime switch Qwen3.5-9B-Q6_K.gguf`
  (`--no-download`, `--no-restart`).
- Реестр `projects.json` — предусловие синхронной смены модели: перед первой
  сменой убедитесь, что оба проекта зарегистрированы
  (`python -m hds.llama_runtime list`).

### Установщики

- **Windows**: `installers/ensure_llama_runtime.ps1` (вызывает `setup.ps1`) —
  бинарь нужного варианта (CUDA при `nvidia-smi`, иначе Vulkan) вместе с
  CUDA-cudart, модели `chat`/`embedding`/`rerank`, `version.json`, манифест,
  регистрация проекта в `projects.json`. Идемпотентен: повторный запуск ничего не
  докачивает; уже скачанные GGUF копируются из `~/.lmstudio/models`.
- **macOS**: `installers/ensure_llama_runtime.sh` (вызывает
  `install_macos.command`) — тот же набор шагов: системный `llama-server`
  (Homebrew) **ссылкой** в `bin/` (обновление через brew подхватывается само)
  либо пре-билд с GitHub Releases со снятием карантина Gatekeeper; модели,
  `version.json` (`metal|cpu`), манифест, регистрация проекта.
- `installers/ensure_models.ps1` и `installers/ensure_models.sh` **удалены**
  (полезное — fallback-копирование из `~/.lmstudio` — перенесено в установщик
  рантайма). `models/` проекта теперь содержит только Whisper-модели.

### Исправления

- **Реранкер**: в дефолтные `llm_server.rerank.extra_args` добавлены
  `--batch-size 8192 --ubatch-size 8192` — фрагмент длиннее physical batch
  (дефолт 512) llama-server отвергал ошибкой 500 «input is too large to process»
  (проявлялось на длинных фрагментах при включённом реранке);
- **`check` (диагностика)**: проверка FTS-нормализации стала контентной
  (сравнение sample-фрагментов с `chunks_fts`), а не флаговой — свежая
  индексация с нуля больше не требует `reindex-fts`; подсказки про GGUF-модели
  указывают на установщик общего рантайма;
- **CI**: `actions/upload-artifact` и `download-artifact` переведены на v5
  (предупреждения о Node.js 20);
- **Релизный workflow**: заметки ищутся и как `RELEASE_NOTES_v<версия>.md`, и как
  `RELEASE_NOTES_<версия>.md` — раньше файл без префикса `v` молча игнорировался
  и релиз уходил с авто-заметками (так было в 0.6.0–0.9.0).

### Тесты и документация

- `tests/test_llama_runtime.py` (18 тестов): пути рантайма по ОС (win32/darwin/
  прочие), `install_hint`, бинарь и символическая ссылка, манифест и
  `resolve_model('shared:<role>')`, реестр и `switch_chat_model`, обзор для UI,
  интеграция с `hds.llama_server` (текст ошибки с установщиком текущей ОС); файл
  прогоняется и в macOS-джобе CI. Полный прогон — 209 тестов, OK.
- README: раздел «Общий llama-рантайм и смена чат-модели», правки шага установки,
  таблиц `llm_server`/`rerank` и таблицы компонентов; оговорки по macOS.
- `PLAN_LLAMA_SERVER.md` — врезка «Актуализация»; `PLAN_SHARED_LLAMA_RUNTIME.md` —
  статус ВЫПОЛНЕНО (включая macOS-шаг).

## Требования к обновлению

1. **Рантайм**: на существующей установке выполните установщик рантайма (или
   переустановку — он идемпотентен):

   ```powershell
   powershell -File installers\ensure_llama_runtime.ps1 -Models chat,embedding,rerank `
     -ProjectName "hermes-disk-search" -ProjectRoot (Get-Location) `
     -RestartArgs "-m hds.llama_server restart chat"
   ```

   macOS:

   ```bash
   bash installers/ensure_llama_runtime.sh --models chat,embedding,rerank \
     --project-name hermes-disk-search --project-root "$PWD" \
     --restart-args "-m hds.llama_server restart chat"
   ```

   Старые каталоги `models\{chat,embedding,rerank}` и `tools\llama.cpp` можно
   удалить после проверки, что роли работают из общего рантайма
   (`python -m hds.llama_server status all` → `state=llama`, `model_ready=true`).
2. **config.yaml**: замените пути моделей на `shared:chat`, `shared:embedding`,
   `shared:rerank`. Старые относительные пути (`models/chat/...`) продолжат
   работать, только если файлы остались на месте; абсолютный путь к GGUF —
   escape-hatch. Шаблон — `config.example.yaml`.
3. **Реранк**: при `rerank.enabled: true` добавьте `--batch-size 8192
   --ubatch-size 8192` в `llm_server.rerank.extra_args` (иначе 500 на длинных
   фрагментах).
4. **Перезапустите UI** (`:8765`) — старый процесс работает на старом коде и не
   знает новых эндпоинтов смены модели.
5. `index --full` **не требуется**: схема БД и API MCP не менялись.
6. **Синхронно с `anonymizer_proxy`**: общий рантайм — один на машину, поэтому
   изменения общего кода выпускаются парно (proxy — v1.17.0). Порядок при
   изменениях: сначала общие файлы (SYNC-COPY-пары), затем правки проектов, затем
   перезапуски.

## Совместимость

- API MCP (`search_local_files`, `ask_my_files`, `start_indexing`, …) не менялся.
- Схема БД не менялась — миграция и повторная индексация не нужны.
- LM Studio как HTTP-бэкенд по-прежнему работает, если он запущен (клиенты
  OpenAI-совместимые).
- Откат: `git revert` коммитов + повторный выпуск с новым PATCH; на уровне
  конфига — явные пути в `llm_server.bin` и `llm_server.<role>.model` (рантайм
  при этом можно не использовать).

