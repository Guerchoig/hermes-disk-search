# PLAN_W2_LLM_HOST.md — детальный план W2 («Ядро на Rust»)

> Дополняет `MIGRATION_PLAN_RUST.md` §4 W2, §8.4–§8.7. Все технические решения из основного
> плана сохранены; здесь — детализация задач, порядок работ, тесты и приёмка.
> Основание: факты, полученные в W0 (`tools/parity/SPIKES.md` §1–§14).
> Статус: план подготовлен 29.09.2026, работы не начаты.

## 1. Что меняется в W2 по итогам W0

| Факт W0 | Следствие для W2 |
|---|---|
| **R32**: движок без явного `devices`/`--whisper-gpu-device` считает **на CPU** (`llama_params_fit_impl: no devices with dedicated memory found`), хотя CUDA-бэкенд загружен | `llm-host` **обязан** передавать `instance_params.manual_devices_csv = "CUDA0"` для chat/embedding/rerank и GPU-устройство для whisper; проверка «VRAM вырос» — часть приёмки. Без этого W2 даёт регрессию ×8–16 вместо выигрыша |
| **R29**: `memory_free` из `list_devices()` ≠ реально свободная VRAM (+10 325 МиБ при занятой VRAM, +2 906 МиБ при свободной) | бюджет VRAM вести по **NVML** (`nvml-wrapper`) как источнику истины; `memory_free` движка — только подсказка + sanity-check перед `load_instance` |
| Эмбеддинги: cos_min **0,999597** (движок на CPU vs llama-server на GPU, `pooling CLS`) | паритетный порог приёмки: cos_min ≥ 0,999 на 200 чанках, **на GPU** |
| Rerank: порядок совпадает на всём, что различимо; расхождения только внутри группы почти равных скоров (\|Δ\| ≤ 0,029 при разбросе 0,034) | критерий приёмки — не «порядок бит-в-бит», а: топ-1 совпадает, Kendall τ ≥ 0,9 на «различимых» парах, хвост (отрыв > 0,1) совпадает |
| `reasoning="off"` не даёт блоков размышлений (проверено по stderr/stdout) | фасад §8.7 реализуем; тест «`chat-think` показывает размышления, `chat` — нет» обязателен |
| Whisper: GPUs **4,3 с на 60 с** аудио против 69,9 с на CPU (×16); turbo = 1 549 МБ; ASCII-стейджинг **обязателен** (не-ASCII путь → имя файла-мозаика) | ASR-инстанс whisper с явным устройством; весь обмен с движком — через ASCII-стейджинг (каталог `data/tmp/llm-host/<uuid>/`), результат забираем по ASCII-пути и переименовываем сами |
| **R33**: при CUDA-whisper процесс Python падает на выходе (`0xC0000409`) после успешной работы | в Rust-владельце (`llm-host`) этой проблемы нет (нет CTranslate2); отдельно — починить Python-путь до W3 (или считать это закрытым фактом миграции) |
| Хэш `content_hash` 50/50 | можно использовать в W2 без переиндексации (R28 закрыт) |
| CLIP ONNX: cos = 1,0 | к W2 не относится (W3), но Dense/pooling-рецепт зафиксирован |
| Harness: 16 фикстур, 59 golden-проверок, `compare.py`, `spike6_parity.py`, `measure_run.py` | **переиспользуем как приёмку W2**: parity-скрипты переключаются с `example-cli`/llama-server на наш `llm-host` |

## 2. Границы W2

**Входит:**
* процесс `hdsw.exe llm-host` (единый владелец GPU) + диспетчер VRAM + фасад HTTP `:8010–8012`;
* маппинг конфига `llm_server.*` / `llm.*` / `gpu.*` → `instance_params`;
* сохранение файловой совместимости: `models/<role>/current.json`, `version.json`,
  скачивание GGUF, пресеты моделей, `shared:<role>`;
* ядро индексации на Rust: `hds-index` (обход/хэш/prune), чанкер (дословный порт),
  конвейер `process_file`, watcher, sidecar-клиент + Python-воркер, `db-move`, подкоманды CLI;
* удаление менеджера llama-server (роли, PID-файлы, `ensure_llama_runtime.*`, `projects.json`).

**Не входит** (остаётся W3/W4/W5): whisper-инстанс как основной ASR-путь (в W2 — заглушка/флаг
+ перенос вызова в W3), CLIP/ONNX, упаковка/установка/CI, удаление Python-кода.

**Ключевое ограничение W2:** Python-версия остаётся источником истины по поведению —
паритет проверяется golden-файлами, а не «на глаз». Откат: БД остаётся читаемой Python-версией.

## 3. Целевая архитектура

```
                 ┌──────────────── hdsw.exe llm-host (резидентный, один владелец GPU) ──────────────┐
                 │  libloading ── multi-node-server.dll (cluster API)                              │
                 │      ├─ list_devices()          ← устройства (CUDA0/CPU)                          │
                 │      ├─ create_instance / load_instance / unload_instance / list_instances        │
                 │      ├─ embeddings / rerank / chat_complete / audio_transcriptions_raw           │
                 │      └─ retention_mode: chat=KEEP_LOADED, embedding/rerank/whisper=LOAD_ON_DEMAND │
                 │  NVML (nvml-wrapper)  ← источник истины по свободной VRAM (R29)                  │
                 │  JSON-RPC/local HTTP (loopback) ← клиенты: index/watch, mcp-http, ui              │
                 │  фасад HTTP :8010 (chat, chat-think) :8011 (embeddings) :8012 (rerank) :health   │
                 └──────────────────────────────────────────────────────────────────────────────────┘
   клиенты:  hds.exe index/watch ──┐
             hds.exe mcp-http ────┼──► llm-host (инстансы/фасад);  без llama-server-процессов
             hds.exe ui ──────────┘
   модели:   %LOCALAPPDATA%\llama-runtime\models\<role>\*.gguf + current.json  (файлы сохраняются)
   рантайм:  %APPDATA%\OpenResearchTools\TranscribeOffline\Engine\*.dll         (скачивается установщиком)
```

Ключевые решения:
1. **Один владелец GPU.** Никаких PID-файлов ролей, `gpu.state.json`, lease-таймаутов —
   состояние видно через `list_instances()` (§8.6.3 основного плана).
2. **Кросс-процессная адресация** — по факту W0 (спайк 6а) DLL грузится, но способ
   «из другого процесса» ещё не проверен: план предусматривает оба варианта
   (внутрипроцессно + локальный RPC), решение фиксируется в задаче A3.
3. **Обязательный GPU**: все инстансы в W2 создаются с явным `manual_devices_csv`.
4. **Бюджет VRAM — по NVML**, а не по `memory_free` движка (R29).

## 4. Трек A. `llm-host` + диспетчер VRAM + фасад (приоритет, 3–4 недели)

### A1. Обвязка движка (3–4 дня)

* `hds-llama` (крейт): загрузка DLL через `libloading` с `SetDllDirectoryW(<engine dir>)`
  (без этого `LoadLibraryExW failed` — проверено в спайке 6а); поиск каталога движка:
  `index.whisper_engine_dir` → `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine` →
  рядом с `hdsw.exe`.
* Тонкая обёртка над экспортами: `list_devices`, `list_instances`, `create_instance`,
  `load_instance`, `unload_instance`, `find_instance_by_name`, `remove_instance`,
  `embeddings`, `rerank`, `chat_complete`, `audio_transcriptions_raw`.
* **Проблема, требующая решения в задаче A1a:** точные C-структуры cluster API
  (`llama_server_cluster_instance_params`, `…_device_info`, `…_instance_info`) в SDK-заголовках
  недоступны локально. Варианты: (1) получить `bridge/llama_server_cluster.h` из документации
  движка; (2) вывести структуры пробником (как спайк 6а: `dumpbin /exports` + пошаговое
  заполнение и проверка на `list_devices` — там поля уже подтверждены: `device_index, backend,
  name, type, free_mib, total_mib`); (3) временно ограничиться bridge-API
  (`llama_server_bridge_*`), который задокументирован в `transcribeoffline/src/bridge.rs`
  и **уже проверен** (audio/chat/embed/rerank + `cluster_instance_name`).
  **Решение по умолчанию:** A1 = bridge-API (работает сегодня), cluster-API (instance-менеджмент)
  вводится в A4, когда структуры будут подтверждены.
* Ошибки/таймауты: коды возврата + `*_last_error`, запись в `data/logs/llm-host.log`.

### A2. Реестр инстансов и маппинг конфига (3–4 дня)

| Наш конфиг | Поле `instance_params` | Значение по умолчанию |
|---|---|---|
| `llm.chat.model` (`shared:chat` → `models/chat/current.json`) | `model_path`, `name="chat"`, `model_kind=TEXT` | Qwen3.5-9B-Q6_K |
| `llm.chat.n_ctx` | `n_ctx` | 32768 |
| `gpu.devices` (**новое, обязательно**) | `manual_devices_csv` | `"CUDA0"` |
| `gpu.n_gpu_layers` (по умолчанию 99) | `n_gpu_layers` | 99 (все слои) |
| `llm.chat.retention` | `retention_mode` | `KEEP_LOADED` |
| `llm.embedding/rerank/whisper.retention` | `retention_mode` + `load_on_demand_grace_seconds` | `LOAD_ON_DEMAND`, grace 300 с |
| `embedding.*` | `embedding=1`, `pooling_type=CLS` | bge-m3 Q8_0 |
| `rerank.*` | `reranking=1`, `pooling_type=RANK` | bge-reranker-v2-m3 Q8_0 |
| `index.whisper_model_path` | `model_path`, `model_kind=WHISPER` | turbo GGML (W3) |
| `llm.model_policy` | логика арбитра (§A4) | `auto` |

* Конфиг читается тем же `config.yaml`; **старые ключи `llm_server.*` продолжают приниматься**
  (`llm_server.chat.model`, `port`, `ctx_per_slot`, `extra_args` — с предупреждением в
  `hds check` о нераспознанных флагах вроде `--cache-type-k`, `-fa`, `--jinja`).
* При `model_policy: auto` перед загрузкой чата арбитр сравнивает потребность «модель + KV»
  с реальной свободной VRAM и при нехватке переходит Q6_K → Q4_K_M → уменьшает `n_ctx` →
  уменьшает `n_gpu_layers` (частичный офлоад), каждый шаг — в лог и в `status`.
* Смена модели в UI переписывает `models/<role>/current.json` через существующий
  `hds-llama::runtime` (порт `hds/llama_runtime.py`: `runtime_dir`, `models_dir`,
  `current_file`, `read_current`, `set_current_chat`, `resolve_model`, `download_chat_model`,
  `chat_models_overview`, `version_file`) и перезагружает инстанс `chat`.


### A3. Кросс-процессная адресация и клиенты (3 дня)

* Проверка (по аналогии со спайком 6а): виден ли инстанс, созданный в `llm-host`, из другого
  процесса — через `cluster_instance_name` в `bridge_params`. Если нет — поднимаем
  **локальный RPC-хост** (`run_local_rpc_server(host, port, n_threads)`) и клиенты ходят через
  него; если и это недоступно — клиенты обращаются к нашему фасаду (`:8010–8012`), который
  сам маппит запрос в инстанс. Последний вариант принимается как **гарантированный fallback**.
* Клиенты в W2: `hdsw llm-host status` (инстансы, VRAM по NVML, режим), `hdsw llm-host load|unload <name>`,
  `hdsw llm-host restart`, `hdsw llm-host devices`. MCP/UI/index обращаются к чату/эмбеддингам через фасад.
* Артефакты: `data/llm-host.pid` (атомарное создание; эвристика «PID принадлежит llm-host»),
  `data/logs/llm-host.log`.
* Автозапуск: как у watcher/MCP (`install_autostart.ps1` → задача `HermesDiskSearchLlmHost`,
  на macOS — LaunchAgent). В W2 достаточно ручного запуска + `llm_server.autostart`
  для совместимости.

### A4. Диспетчер VRAM (4–5 дней) — ядро ценности W2

Бюджет:
* источник истины — **NVML** (`nvml-wrapper`): `memory.used/free/total`;
* `gpu.reserve_mb` (по умолчанию 1024) не выедаем;
* `memory_free` движка — только sanity-check: если NVML free < потребности, а движок
  сообщает «свободно», доверяем NVML (R29).

Потребность модели оцениваем как `файл_GGUF + KV(n_ctx, n_parallel, тип KV)`; для KV
используем формулу по слоям/головам модели из метаданных GGUF (или калибровочную таблицу,
измеренную в W0: Q6_K 9B при `n_ctx=32768 --cache-type-k/v q8_0` занял ≈7,5 ГБ).

Правила (соответствуют §8.6.2 основного плана, но с явными шагами):

| Событие | Действия (в порядке) |
|---|---|
| Запрос (`ask`/`search`/внешний агент), свободной VRAM < потребности | 1) `index.pause` (существующий механизм); 2) `unload_instance`: whisper → rerank → embedding (по `gpu.priorities`); 3) пере-проверка NVML; 4) при нехватке — `model_policy.auto` шаги (Q4_K_M → n_ctx ↓ → n_gpu_layers ↓); 5) `load_instance(chat)`; 6) ответ; 7) снять `index.pause` |
| Запрос завершён | `index.pause` снимется; индексные инстансы вернутся сами (`LOAD_ON_DEMAND`) |
| Индексация без запросов, VRAM не хватает | разрешено выгрузить `chat` (единственный владелец GPU) |
| Простой роли > `load_on_demand_grace_seconds` | движок сам переводит в `GRACE` → `UNLOADED`; внешний предохранитель `gpu.evict_idle_sec` |
| `load_instance` → `FAILED` | не ретраить бесконечно: деградация (квант/ctx/ngl/CPU) + сообщение в `hds check` и в UI |
| Смена `current.json` чат-модели | `unload_instance(chat)` → `remove_instance` → `create_instance` с новым путём → `load_instance` |

Наблюдаемость: `hdsw llm-host status --json` отдаёт по каждому инстансу `name/state/retention/
active_request_count/queued_request_count/last_error/занимаемая VRAM (NVML − baseline)`;
эти поля использует UI (карточка «GPU и модели»).

### A5. Фасад HTTP `:8010–8012` (3 дня)

| Порт | Эндпоинты | Примечание |
|---|---|---|
| 8010 | `/v1/chat/completions`, `/v1/models`, `/health`, `/props` | алиасы `chat` (thinking off) и `chat-think` (thinking on) |
| 8011 | `/v1/embeddings`, `/v1/models` | `X-Embed-Pooling: CLS` фиксировано |
| 8012 | `/v1/rerank`, `/v1/models` | `RANK` |
| — | (опционально, W3) `/v1/audio/transcriptions` | |

* Маппинг thinking: `reasoning=on|off|auto`, `reasoning_budget` (0 = off, −1 = без лимита),
  `reasoning_format`; тела запросов с `chat_template_kwargs.enable_thinking` и `think`
  **прозрачно** переводятся в эти поля; `llm.facade.thinking_default: off`.
* `/props` и `/health` отдаём сами (совместимость с внешними проверками и с прежним
  `hds/llama_server.probe`/`props_context` — формат ответа сохраняем).
* Режимы `llm_server.mode: embedded|facade|off` (§8.7): `off` — HDS работает без LLM
  (только FTS), это же — аварийный режим при недоступном движке.
* Таймауты и размеры ответов — как у текущего llama-server (240 с на чат, стриминг не
  требуется в W2 — проверяется по факту использования клиентами).

### A6. Значения по умолчанию и совместимость (2 дня)

* `hdsw llm-host status` печатает те же смысловые строки, что `python -m hds.llama_server status`
  (роль → состояние/порт/модель/контекст), чтобы UI не переписывать целиком.
* Все предупреждения старых ключей — в `hdsw check`.
* Порты/алиасы сохраняются: внешние потребители (`anonymizer_proxy`, скрипты) продолжают
  работать без изменений — это проверяется тестом «старый клиент против нового владельца».


## 5. Трек B. Ядро индексации на Rust (параллельно треку A, 4–5 недель)

Задачи — из §4 W2 основного плана (пункты 1–6, 10) с уточнениями по фактам W0:

| # | Задача | Уточнения из W0 | Оценка |
|---|---|---|---|
| B1 | `hds-index`: обход `walkdir`, `exclude_dirs`/`exclude_paths` (границы компонентов, нормализация Win/POSIX), лимиты размеров, `~$`-файлы, дедупликация по хэшу | хэш уже проверен (50/50, R28) | 4 дня |
| B2 | `content_hash` (Blake2b-16: `str(size)` + head/tail 256 КБ) | готовый Rust-код есть в `tools/parity/spikes/src/lib.rs` — переносится дословно | 1 день |
| B3 | Чанкер — дословный порт `hds/chunker.py` | golden-файлы содержат чанки всех 16 фикстур → паритет «побайтово» | 4 дня |
| B4 | Конвейер `process_file` (фазы, атомарный коммит на файл, `clip_for_embedding`, `max_chunks`, прогресс-репортёр с теми же полями и heartbeat) | **R30**: различать «живой прогон» и «паузную/зависшую сессию» (по времени последнего прогресса), не полагаться только на свежесть файла; **вывод прогресса не должен зависеть от читателя stdout** (в W0 харнесс встал на переполнении pipe) | 5 дней |
| B5 | Watcher на `notify`: debounce, очередь, `watch.lock`, reconcile, rename/удаление/корзина | Ловушка W0: роли — пары launcher→worker; lock должен корректно освобождаться при убийстве дерева | 5 дней |
| B6 | Sidecar-клиент + Python-воркер `hds-extract` | Контракт подтверждён спайком 3: старт 0,22 с, RSS 34,5→72,7 МБ, извлечение 87–456 мс, выход по EOF 0,08 с; **библиотеки печатают в stdout — уводить в stderr**; мерить память по дереву процессов | 5 дней |
| B7 | `db-move`, подкоманды `check/reindex/reindex-fts/forget/stop/clip-index/status` | схему БД и PRAGMA сохраняем 1:1; подключение sqlite-vec — через `sqlite3_auto_extension` (спайк 1) | 4 дня |

**Тесты трека B** (используют уже готовый harness):
1. `compare.py` против `tools/parity/golden` — сегменты/чанки/FTS/хэши строго, поиск с допуском.
2. Индексация пилота (10 000 файлов) — совпадение числа чанков, текстов, `chunk_count`,
   `indexed_at`; БД остаётся читаемой Python-версией.
3. Watcher-сценарии (создание/изменение/rename/удаление/корзина/массовая запись).
4. `spike1_db`-тест на реальной БД + `hash_parity` — как регресс в CI.
5. Замеры памяти (`measure_run.py`) после переезда: сравнить с W0-цифрами
   (индексация 500 файлов: было 48,2 с / ws 122 МБ / commit 597 МБ).

## 6. Порядок работ и график (5–7 недель)

| Неделя | Трек A (llm-host) | Трек B (ядро) |
|---|---|---|
| 1 | A1 обвязка DLL + `list_devices`; A1a решение по структурам (bridge vs cluster) | B1 обход/хэш/лимиты |
| 2 | A2 реестр инстансов + маппинг конфига; `hdsw llm-host devices/status` | B2 хэш-паритет; старт B3 чанкер |
| 3 | A4 базовый арбитр (NVML-бюджет, unload по приоритету); A3 адресация клиентов | B3 чанкер + паритет-тест; B4 старт конвейера |
| 4 | A5 фасад `:8010–8012` + thinking-маппинг; A6 совместимость | B4 конвейер + прогресс/heartbeat; B6 sidecar |
| 5 | ARB-сценарии (см. §7) + `model_policy: auto`; наблюдаемость в UI | B5 watcher; B7 db-move/подкоманды |
| 6 | доводка: деградация/логи/`hdsw check`; удаление llama-server (пункт 9 W2) | паритет пилота (10 000 файлов), замеры, доводка |
| 7 (резерв) | регресс-прогон harness, обновление `STATUS.md`, подготовка к W3 | — |

Параллельность: треки почти независимы (пересечение — вызовы эмбеддингов/чата, то есть
фасад A5 и sidecar B6, которые идут в одну неделю у разных исполнителей/сессий).


## 7. Критерии приёмки (измеримые, переиспользуют harness W0)

**Трек A (llm-host).**
1. **A-1 Устройство**: `hdsw llm-host status --json` показывает `devices: ["CUDA0"]` и
   `n_gpu_layers: 99`; во время работы инстанса NVML подтверждает рост занятой VRAM
   (chat +4,9 ГБ, whisper +2,0 ГБ — эталоны W0, SPIKES §14.3). Отсутствие роста = провал (R32).
2. **A-2 Дев-скрипт паритета**: `tools/parity/spike6_parity.py` переключается на фасад/инстансы
   → embeddings `cos_min ≥ 0,999` на 200 чанках (эталон W0: 0,999597); rerank: топ-1 совпадает,
   Kendall τ ≥ 0,9 на парах с разницей скоров > 0,05.
3. **A-3 Thinking**: `model: "chat"` → ответ без блоков размышлений; `model: "chat-think"` →
   размышления присутствуют; проверка одновременно двумя клиентами (как ARB-6).
4. **A-4 Совместимость фасада**: `curl :8010/v1/models`, `/health`, `/props` — формат как у
   llama-server; существующий клиент (`anonymizer_proxy`, скрипты пользователя) работает без правок.
5. **A-5 ARB-1…ARB-6** (сценарии §4 W2 основного плана) — автоматизировать скриптом
   `tools/parity/arb_scenarios.py` с фиксацией таймингов и VRAM; критерий — все шесть проходят,
   а пауза/возобновление индексации не теряет файлов (проверка по `seen/processed`).
6. **A-6 Бюджет**: при двух загруженных инстансах (chat + embedding) и работающей индексации
   NVML free ≥ `gpu.reserve_mb`; при запросе индексация реально встаёт на паузу
   (`index.pause` появляется, heartbeat помечается paused).
7. **A-7 Деградация**: искусственно уменьшенный бюджет (`gpu.vram_budget_mb`) приводит к
   переходу Q6_K → Q4_K_M и к записи в лог/`hdsw check` — без падения.

**Трек B (ядро).**
8. **B-1 Паритет harness**: `compare.py` — 100 % совпадение по сегментам/чанкам/FTS/хэшам
   на 16 фикстурах, поиск — состав топ-20 совпадает.
9. **B-2 Пилот**: 10 000 файлов — совпадение числа чанков/текстов/`chunk_count`/`indexed_at`
   с Python-версией; время не хуже +10 %; БД читается Python-версией.
10. **B-3 Watcher**: 6 сценариев без дублей/пропусков; `watch.lock` освобождается при
    принудительном убийстве дерева процессов.
11. **B-4 Память**: индексация 500 файлов — не хуже эталона W0 (ws 122 МБ / commit 597 МБ,
    допуск +20 %); простой с watcher — не хуже 46/518 МБ.

## 8. Риски W2 (сверх R29/R32/R33 основного плана)

| # | Риск | Вероятность/влияние | Мера |
|---|---|---|---|
| W2-1 | **Структуры cluster API (`instance_params` и др.) недоступны** → instance-менеджмент не собрать | средняя / высокое | fallback на bridge-API (`cluster_instance_name` + `llama_server_bridge_*`), который уже проверен; cluster-API — инкрементально, по мере подтверждения полей |
| W2-2 | Кросс-процессная адресация не работает без локального RPC | средняя / среднее | вариант «клиенты → наш фасад `:8010–8012`» как гарантированный путь; RPC — оптимизация |
| W2-3 | Оверкоммит VRAM (WDDM): движок «влезает», но работает в разы медленнее (наблюдалось в W0) | средняя / высокое | жёсткий контроль NVML + `gpu.reserve_mb`; запрет частичного офлоада без явного согласия (`n_gpu_layers` не уменьшаем молча) |
| W2-4 | Порты `:8010–8012` заняты старым llama-server при переключении | высокая / низкое | `hdsw llm-host` при старте проверяет занятость и сообщает роль-владельца; режим `off` для аварийного отката |
| W2-5 | Регресс ASR: whisper-инстанс требует ASCII-стейджинга и своих параметров | средняя / среднее | ASCII-каталог `data/tmp/llm-host/<uuid>/` + перенос результата; тест с русским путём (эталон W0) |
| W2-6 | Индексация «встаёт» из-за чужих heartbeat (R30) | средняя / среднее | различать паузную/зависшую сессию; `hdsw llm-host status` показывает владельца heartbeat; тест «паузная сессия не блокирует новый прогон» |

## 9. Открытые вопросы (требуют решения до/во время W2)

1. **SDK движка**: есть ли у заказчика доступ к `bridge/llama_server_cluster.h` (или иной
   документации с точными структурами)? Если нет — идём по W2-1 (bridge-API + reverse).
2. **Нужен ли локальный RPC** для кросс-процессной адресации — решается тестом в A3;
   заранее согласие на fallback через фасад.
3. **`anonymizer_proxy`**: план §8.6 исключает его из арбитра (заказчик переводит на движок
   отдельно). До этого момента он продолжает потреблять VRAM вне нашего учёта — нужно
   согласовать: (а) добавить его в `gpu.external_vram_mb` как фиксированный вычет, или
   (б) дождаться его переезда.
4. **Политика кванта**: `llm.model_policy: auto` по умолчанию — согласовано? Если нет,
   ставим `fixed` и не деградируем молча.
5. **Приоритеты** `gpu.priorities { chat:100, embedding:40, rerank:30, whisper:20 }` — принимаются?
6. **Что делать с llama-server как fallback**: оставляем код до W5 (откат) или удаляем сразу
   в W2 (пункт 9)? Рекомендация: удалить в конце W2, сохранив `llm_server.mode: off`.
7. **Часы простоя**: `gpu.evict_idle_sec: 600` — устраивает? От этого зависит, как быстро
   освобождается VRAM под индексацию.

## 10. Что считается «сделано» (Definition of Done для W2)

* `hdsw.exe llm-host` владеет GPU: все четыре роли поднимаются с явным `CUDA0`, VRAM
  подтверждена NVML, llama-server-процессов и их PID-файлов нет;
* `hds.exe index/watch` индексируют пилот с паритетом по golden и по пилоту 10 000 файлов;
* фасад `:8010–8012` обслуживает `chat`/`chat-think`/`embeddings`/`rerank` с сохранением
  формата (внешние клиенты не меняются);
* ARB-1…ARB-6 проходят автоматизированно, VRAM-бюджет соблюдается;
* замеры памяти/времени не хуже эталонов W0 (допуски из §7);
* `tools/parity/SPIKES.md`-подобный отчёт по W2 (например, `tools/parity/W2_REPORT.md`)
  с цифрами по каждому критерию приёмки, обновлённый `STATUS.md` и запись об удалении
  llama-server;
* риски R29/R32/R33 либо закрыты, либо имеют зафиксированное решение и место в бэклоге.

