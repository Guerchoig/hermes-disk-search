# PLAN_W2_LLM_HOST.md — детальный план W2 («Ядро на Rust»)

> Дополняет `MIGRATION_PLAN_RUST.md` §4 W2, §8.4–§8.7. Все технические решения из основного
> плана сохранены; здесь — детализация задач, порядок работ, тесты и приёмка.
> Основание: факты, полученные в W0 (`tools/parity/SPIKES.md` §1–§14).
> Статус: план подготовлен 29.09.2026, работы не начаты.

## 1. Что меняется в W2 по итогам W0

| Факт W0 | Следствие для W2 |
|---|---|
| **R32**: движок без явного `devices`/`--whisper-gpu-device` считает **на CPU** (`llama_params_fit_impl: no devices with dedicated memory found`), хотя CUDA-бэкенд загружен | **Подтверждено документацией движка**: `gpu` и `devices` взаимоисключающие, а «`gpu` не задан ⇒ Windows/Linux CPU-only». `llm-host` **обязан** выбирать устройство явно (`gpu=<индекс>` или `manual_devices_csv`); проверка «VRAM вырос + кратное ускорение» — часть приёмки. Без этого W2 даёт регрессию ×8–16 вместо выигрыша |
| **R29**: `memory_free` из `list_devices()` ≠ реально свободная VRAM (+10 325 МиБ при занятой VRAM, +2 906 МиБ при свободной) | бюджет VRAM вести по **NVML** (`nvml-wrapper`) как источнику истины; `memory_free` движка — только подсказка + sanity-check перед `load_instance` |
| Эмбеддинги: cos_min **0,999597** (движок на CPU vs llama-server на GPU, `pooling CLS`) | паритетный порог приёмки: cos_min ≥ 0,999 на 200 чанках, **на GPU** |
| Rerank: порядок совпадает на всём, что различимо; расхождения только внутри группы почти равных скоров (\|Δ\| ≤ 0,029 при разбросе 0,034) | критерий приёмки — не «порядок бит-в-бит», а: топ-1 совпадает, Kendall τ ≥ 0,9 на «различимых» парах, хвост (отрыв > 0,1) совпадает |
| `reasoning="off"` не даёт блоков размышлений (проверено по stderr/stdout) | фасад §8.7 реализуем; тест «`chat-think` показывает размышления, `chat` — нет» обязателен |
| Whisper: GPUs **4,3 с на 60 с** аудио против 69,9 с на CPU (×16); turbo = 1 549 МБ; ASCII-стейджинг **обязателен** (не-ASCII путь → имя файла-мозаика) | ASR-инстанс whisper с явным устройством; весь обмен с движком — через ASCII-стейджинг (каталог `data/tmp/llm-host/<uuid>/`), результат забираем по ASCII-пути и переименовываем сами |
| **R33**: при CUDA-whisper процесс Python падает на выходе (`0xC0000409`) после успешной работы | в Rust-владельце (`llm-host`) этой проблемы нет (нет CTranslate2); отдельно — починить Python-путь до W3 (или считать это закрытым фактом миграции) |
| Хэш `content_hash` 50/50 | можно использовать в W2 без переиндексации (R28 закрыт) |
| CLIP ONNX: cos = 1,0 | к W2 не относится (W3), но Dense/pooling-рецепт зафиксирован |
| Harness: 16 фикстур, 59 golden-проверок, `compare.py`, `spike6_parity.py`, `measure_run.py` | **переиспользуем как приёмку W2**: parity-скрипты переключаются с `example-cli`/llama-server на наш `llm-host` |
| **SDK движка открыт** (`github.com/openresearchtools/engine`): `bridge/llama_server_cluster.h`, `bridge/llama_server_bridge.h`, `docs/*.md` | структуры/enum'ы выписаны в §11 — риски «не собрать instance-менеджмент» и «угадывать поля» сняты; A1 стартует прямо на cluster API |

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
3. **Обязательный GPU**: все инстансы в W2 создаются с явным `manual_devices_csv`
   (числовой bridge-индекс, см. A1) и `allow_cpu = false`.
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
* **Структуры cluster API — получены** (риск W2-1 закрыт): SDK открыт в
  `github.com/openresearchtools/engine`, файл `bridge/llama_server_cluster.h`
  (и `bridge/llama_server_bridge.h`, `docs/common-runtime-and-devices.md`,
  `docs/bridge-{chat,embeddings,rerank,audio}-dll.md`). Точные поля/enum'ы — в §11
  (приложение). Поэтому A1 идёт **сразу на cluster API**, а bridge-API остаётся
  вспомогательным (одиночные запросы, `cluster_instance_name`).
* **Правила выбора устройства (из документации движка, подтверждают R32):**
  1. `gpu` и `devices` **взаимоисключающие** — задавать только одно;
  2. `gpu = <индекс из list_devices>` → одно-девайсная маршрутизация; при этом
     `split_mode` по умолчанию `none`;
  3. **если `gpu` не задан, на Windows/Linux движок работает CPU-only** (macOS — первый GPU).
     Это и есть причина CPU-инференса в спайке 6в;
  4. `devices` (CSV индексов/имён) + `tensor_split` — только для мульти-девайса/сплита
     (`split_mode=layer|row`), «для влезания, а не для скорости»;
  5. `n_gpu_layers = -1` (дефолт) = полный офлоад, `kv_unified = 1`, `no_kv_offload = 0`.
* **Первый тест A1 (10 минут, снимает неопределённость из W0):** создать один инстанс
  тремя способами — без устройства, `gpu=<индекс CUDA>`, `devices="<имя CUDA>"` — и замерить
  NVML/скорость. Критерий: GPU-варианты дают рост VRAM и кратное ускорение; выбранный способ
  фиксируется в конфиге как `gpu.device_index`. (В W0 через `example-cli` сработал
  `--devices CUDA0`, а `--gpu 1` — нет; нужно понять, какой индекс ожидает наша обвязка.)
  ✅ **Выполнено 30.09.2026** (`crates/hds-llama/src/bin/a1_device_probe.rs`,
  результаты — `tools/parity/W2_REPORT.md` §1, сырой отчёт
  `tools/parity/out/w2_a1_device.json`): работает `manual_devices_csv="0"` (CUDA0,
  VRAM +636 МиБ, инференс 253 мс против 4 282 мс на CPU), имя устройства отвергается,
  `gpu.device_index = 1` → `manual_devices_csv="0"`.
* **Обязательные требования к процессу (находки A1, без них GPU-путь недостижим):**
  держать текущим каталог движка (`Engine::activate()`, движок грузит ggml-бэкенды
  относительно `.`) и добавлять в путь поиска DLL вендорские каталоги
  (`Engine\vendor\ffmpeg\bin` и пр.) через `AddDllDirectory` — иначе `LoadLibraryExW`
  падает с кодом 126, `list_devices` пуст и инференс молча уходит на CPU (R32).
* Ошибки/таймауты: коды возврата + `*_last_error`; у bridge-результатов дополнительно
  проверять `out.ok == 1` и `out.error_json` (документированное правило), запись в
  `data/logs/llm-host.log`.

### A2. Реестр инстансов и маппинг конфига (3–4 дня)

| Наш конфиг | Поле `instance_params` | Значение по умолчанию |
|---|---|---|
| `llm.chat.model` (`shared:chat` → `models/chat/current.json`) | `model_path`, `name="chat"`, `model_kind=TEXT` | Qwen3.5-9B-Q6_K |
| `llm.chat.n_ctx` | `n_ctx` | 32768 |
| `gpu.device_index` (**обязательно**, `0` = CPU, `1` = первый GPU) | `manual_devices_csv` (**числовой** bridge-индекс; имя не принимается — A1) | определяется `list_devices()`; `0` → CSV с индексом CPU-устройства (не «пусто»: без выбора сборка ушла на GPU — A1); `allow_cpu = false`, чтобы откат на CPU был ошибкой, а не тишиной |
| `gpu.n_gpu_layers` | `n_gpu_layers` | `-1` = полностью на GPU (дефолт движка) |
| `llm.chat.retention` | `retention_mode` | `KEEP_LOADED` (1) |
| `llm.embedding/rerank/whisper.retention` | `retention_mode` + `load_on_demand_grace_seconds` | `LOAD_ON_DEMAND` (2), grace 300 с |
| `embedding.*` | `embedding=1`, `model_kind=EMBEDDINGS`, `pooling_type=CLS` | bge-m3 Q8_0 |
| `rerank.*` | `reranking=1`, `model_kind=RERANK`, `pooling_type=RANK` | bge-reranker-v2-m3 Q8_0 |
| `index.whisper_model_path` | `model_path`, `model_kind=WHISPER` | turbo GGML (W3) |
| `llm.chat.n_ctx` / `n_batch` / `n_parallel` | одноимённые поля | 32768 / 2048 / 1 (дефолты движка) |
| `llm.model_policy` | **отчёт о нехватке** (§A4), без авто-деградации | `fixed` (решение заказчика) |

* Конфиг читается тем же `config.yaml`; **старые ключи `llm_server.*` продолжают приниматься**
  (`llm_server.chat.model`, `port`, `ctx_per_slot`, `extra_args` — с предупреждением в
  `hds check` о нераспознанных флагах вроде `--cache-type-k`, `-fa`, `--jinja`).
* **При нехватке VRAM — не деградировать молча (решение заказчика 29.09.2026)**: арбитр
  сравнивает потребность «модель + KV» с реальной свободной VRAM и, если не влезает,
  **сообщает** (лог + UI «GPU и модели» + `hdsw check`) и ждёт, пока пользователь сменит
  модель в UI (`models/chat/current.json` → перезагрузка инстанса). Никаких автоматических
  переходов Q6_K → Q4_K_M, уменьшения `n_ctx` и частичного офлоада — только явное согласие
  пользователя (`llm.model_policy: fixed` по умолчанию).
* `anonymizer_proxy` из бюджета **не вычитаем**: по решению заказчика ждём его переезда на
  движок; до этого допускаем, что часть VRAM может быть занята им (учёт — по NVML, без магии).
* Смена модели в UI переписывает `models/<role>/current.json` через существующий
  `hds-llama::runtime` (порт `hds/llama_runtime.py`: `runtime_dir`, `models_dir`,
  `current_file`, `read_current`, `set_current_chat`, `resolve_model`, `download_chat_model`,
  `chat_models_overview`, `version_file`) и перезагружает инстанс `chat`.
  ⚠️ **Обязательное требование (SPIKES §14.7):** `shared:<role>` разрешается в **общем рантайме**
  (`%LOCALAPPDATA%\llama-runtime` / `~/Library/Application Support/llama-runtime`), а не в
  `<project>/models/<role>` — текущий Python-менеджер именно здесь и падает («GGUF-модель роли
  'chat' не найдена» при живом файле). В W2 добавить: `hdsw check` печатает «искали здесь / нашли
  это» по каждой роли и негативный тест «файла нет → понятное сообщение с двумя путями».


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
* источник истины — **NVML** (`nvml-wrapper`) на Windows/Linux с NVIDIA: `memory.used/free/total`;
  на macOS — **`VramProbe::Metal`** (`recommendedMaxWorkingSetSize` как total,
  `currentAllocatedSize` как своя занятость, дельта `ioreg` как чужая) — **реализуется как задел,
  но не исполняется** до появления Apple Silicon машины (§10.0 основного плана, R34);
  в DoD W2 входит только NVML + фолбэк;
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

* Маппинг thinking (по `docs/bridge-chat-dll.md` — семантика подтверждена): `reasoning`
  (`on|off|auto`), при `off` движок **форсирует `reasoning_budget=0`**; при `on`/`auto` без
  бюджета движок ставит `reasoning_budget=-1`; если `reasoning` задан без формата, движок
  отправляет `reasoning_format="deepseek"`, поэтому для видимых размышлений указываем явно
  `reasoning_format="none"`; при незаданном `reasoning` флаги не отправляются вовсе.
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
| 5 | ARB-сценарии (см. §7) + отчёт о нехватке VRAM; наблюдаемость в UI | B5 watcher; B7 db-move/подкоманды |
| 6 | доводка: деградация/логи/`hdsw check`; удаление llama-server (пункт 9 W2) | паритет пилота (10 000 файлов), замеры, доводка |
| 7 (резерв) | регресс-прогон harness, обновление `STATUS.md`, подготовка к W3 | — |

Параллельность: треки почти независимы (пересечение — вызовы эмбеддингов/чата, то есть
фасад A5 и sidecar B6, которые идут в одну неделю у разных исполнителей/сессий).


## 7. Критерии приёмки (измеримые, переиспользуют harness W0)

**Трек A (llm-host).** Все критерии ниже — **windows-x64 и проверяются в W2**; mac-варианты
помечены отдельно и помечены «не проверено» (§10.0 основного плана).
1. **A-1 Устройство (Windows)**: `hdsw llm-host status --json` показывает `devices: ["CUDA0"]` и
   `n_gpu_layers: 99`; во время работы инстанса NVML подтверждает рост занятой VRAM
   (chat +4,9 ГБ, whisper +2,0 ГБ — эталоны W0, SPIKES §14.3). Отсутствие роста = провал (R32).
   *A-1-mac (не проверяется):* то же через `VramProbe::Metal` — рост `currentAllocatedSize`;
   входит в `tools/parity/MAC_CHECKLIST.md`.
2. **A-2 Дев-скрипт паритета (Windows)**: `tools/parity/spike6_parity.py` переключается на
   фасад/инстансы → embeddings `cos_min ≥ 0,999` на 200 чанках (эталон W0: 0,999597); rerank:
   топ-1 совпадает, Kendall τ ≥ 0,9 на парах с разницей скоров > 0,05.
   *A-2-mac (не проверяется):* паритет на tiny-модели вместо bge-m3 — в mac-чек-листе.
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
7. **Отсутствие молчаливой деградации**: при нехватке VRAM чат-модель **не** деградирует
   (квант/ctx/ngl не меняются автоматически) — вместо этого появляется сообщение
   (лог + UI + `hdsw check`) с точными цифрами «нужно/доступно», а смена модели — за
   пользователем в UI. Проверяется искусственным сужением бюджета (`gpu.vram_budget_mb`).

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
| W2-7 | **macOS: нет NVML, бюджет VRAM — только через Metal API**, чужая GPU-память видна лишь системно (`ioreg`) | высокая / среднее | реализовать `VramProbe::Metal` за trait-абстракцией; в DoD W2 входят только NVML и фолбэк `list_devices`; критерий для mac — «рост `currentAllocatedSize` + ускорение против CPU» — **в постпроектном чек-листе (§10.0 основного плана)** |
| W2-1 | ~~Структуры cluster API недоступны~~ | **закрыт (29.09.2026)**: SDK движка открыт (`github.com/openresearchtools/engine`, `bridge/llama_server_cluster.h`), структуры и enum'ы выписаны в §11 | — |
| W2-2 | Кросс-процессная адресация не работает без локального RPC | средняя / среднее | вариант «клиенты → наш фасад `:8010–8012`» как гарантированный путь; RPC — оптимизация |
| W2-3 | Оверкоммит VRAM (WDDM): движок «влезает», но работает в разы медленнее (наблюдалось в W0) | средняя / высокое | жёсткий контроль NVML + `gpu.reserve_mb`; запрет частичного офлоада без явного согласия (`n_gpu_layers` не уменьшаем молча) |
| W2-4 | Порты `:8010–8012` заняты старым llama-server при переключении | высокая / низкое | `hdsw llm-host` при старте проверяет занятость и сообщает роль-владельца; режим `off` для аварийного отката |
| W2-5 | Регресс ASR: whisper-инстанс требует ASCII-стейджинга и своих параметров | средняя / среднее | ASCII-каталог `data/tmp/llm-host/<uuid>/` + перенос результата; тест с русским путём (эталон W0) |
| W2-6 | Индексация «встаёт» из-за чужих heartbeat (R30) | средняя / среднее | различать паузную/зависшую сессию; `hdsw llm-host status` показывает владельца heartbeat; тест «паузная сессия не блокирует новый прогон» |

## 9. Решения заказчика (29.09.2026) и что осталось выяснить

| Вопрос | Решение |
|---|---|
| SDK движка | **Доступен**: `github.com/openresearchtools/engine` (открытый репозиторий). Структуры cluster/bridge API и правила устройств выписаны в §11; W2-1 закрыт |
| `anonymizer_proxy` (чужая VRAM) | **Ждём его переезда на движок**; в бюджет не вычитаем, ориентируемся на NVML |
| Нехватка VRAM под чат-модель | **Никакой авто-деградации**: сообщение о нехватке → пользователь сам меняет модель в UI. `llm.model_policy: fixed` |
| Судьба llama-server | **Удаляем в конце W2** (пункт 9 задач W2), оставляем `llm_server.mode: off` как аварийный режим |
| `gpu.evict_idle_sec` | **600 с — устраивает** (и приоритеты `chat:100, embedding:40, rerank:30, whisper:20` принимаются) |

Осталось выяснить по ходу работ (не блокирует старт):
1. ~~**Какой индекс устройства ожидает обвязка**~~ — ✅ **выяснено 30.09.2026 (A1)**:
   cluster API принимает **числовой** `manual_devices_csv` = bridge-индекс устройства
   (`"0"` → `CUDA0`, `"1"` → `CPU`); имя (`"CUDA0"`) отвергается с
   `manual device selection is no longer available`; «без устройства» на проверенной
   сборке ушло на GPU, а не на CPU, как обещает документация. Детали,
   цифры VRAM/скорости и требования (cwd движка + вендорские каталоги DLL) —
   `tools/parity/W2_REPORT.md` §1.
2. **Кросс-процессная адресация** — тест A3; при неудаче гарантированный путь «клиенты → фасад».

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
* **macOS в DoD W2 не входит** (§10.0 основного плана): `VramProbe::Metal`, имена `lib*.dylib`
  и mac-ветки установщика реализуются «с заделкой», но критерии по ним помечены «не проверено»
  и выполняются в постпроектной фазе по `tools/parity/MAC_CHECKLIST.md` (риск R34/W2-7).


## 11. Приложение. Точные контракты движка (из открытого SDK)

Источники (`github.com/openresearchtools/engine`, ветка `main`):
`bridge/llama_server_cluster.h`, `bridge/llama_server_bridge.h`,
`docs/common-runtime-and-devices.md`, `docs/bridge-chat-dll.md`,
`docs/bridge-embeddings-dll.md`, `docs/bridge-rerank-dll.md`, `docs/bridge-audio-dll.md`.

### 11.1. Перечисления cluster API

```c
enum llama_server_cluster_instance_retention_mode {
  LLAMA_SERVER_CLUSTER_INSTANCE_KEEP_LOADED    = 1,
  LLAMA_SERVER_CLUSTER_INSTANCE_LOAD_ON_DEMAND = 2,
};
enum llama_server_cluster_instance_state {
  UNLOADED = 0, LOADING = 1, LOADED = 2, SERVING = 3, GRACE = 4, FAILED = 5,
};
enum llama_server_cluster_instance_model_kind {
  TEXT = 0, VISION = 1, EMBEDDINGS = 2, RERANK = 3, WHISPER = 4,
  REALTIME_AUDIO = 5, DIARIZATION = 6,
};
```

### 11.2. Структуры (поля, нужные нам)

```c
struct llama_server_cluster_device_info {
  int32_t bridge_device_index; int32_t type;
  uint64_t memory_free; uint64_t memory_total;   // источник НЕ надёжен (R29) — сверять с NVML
  const char* backend; const char* name; const char* description;  // backend="CUDA", name="CUDA0"
};
struct llama_server_cluster_execution_group_info {
  const char* id; const char* label; const char* backend_summary; const char* devices_csv;
  int32_t device_count; int32_t uses_local_split;
  uint64_t memory_free; uint64_t memory_total;
};
struct llama_server_cluster_instance_params {
  const char* name; const char* model_path; const char* mmproj_path;
  const char* diarization_model_path; const char* execution_group_id;
  const char* rpc_servers;          // "host:port,host:port"
  const char* manual_devices_csv;   // упорядоченные bridge-индексы устройств инстанса
  const char* manual_tensor_split;  // веса, согласованные с manual_devices_csv
  int32_t retention_mode; int32_t load_on_demand_grace_seconds;
  int32_t embedding; int32_t reranking; int32_t model_kind;
  int32_t allow_cpu; int32_t allow_integrated_gpu;
  int32_t n_ctx; int32_t n_batch; int32_t n_ubatch; int32_t n_parallel;
  int32_t n_threads; int32_t n_threads_batch; int32_t n_gpu_layers;
};
struct llama_server_cluster_instance_info {
  int64_t instance_id; const char* name; const char* model_path; const char* mmproj_path;
  const char* diarization_model_path; const char* execution_group_id; const char* rpc_servers;
  int32_t retention_mode; int32_t load_on_demand_grace_seconds; int32_t model_kind;
  int32_t state; int32_t active_request_count; int32_t queued_request_count; int32_t n_parallel;
  int64_t grace_deadline_unix_ms; const char* last_error;
};
```

Вызовы по инстансу принимают `instance_id`: `chat_complete`, `vlm_complete`, `embeddings`,
`rerank`, `audio_transcriptions_raw`. Есть `*_default_*()` хелперы и `*_free_*()` для
результатов — **всегда** инициализировать структуры хелперами (правило SDK).

### 11.3. Правила выбора устройства (из документации движка)

* `gpu` и `devices` **взаимоисключающие** — не задавать оба;
* `gpu >= 0` → одно-девайсная маршрутизация по индексу из `list_devices()`; `split_mode`
  по умолчанию `none`;
* **`gpu` не задан ⇒ Windows/Linux работает CPU-only** (macOS — первый GPU). ← причина
  CPU-инференса в спайке 6в и корень R32;
* мульти-девайс/сплит (`devices` + `tensor_split`, `split_mode = layer|row`) — «для влезания,
  а не для скорости»;
* дефолты bridge-параметров: `n_ctx=32768`, `n_batch=2048`, `n_ubatch=2048`, `n_parallel=1`,
  `n_threads=8`, `n_gpu_layers=-1` (полный офлоад), `kv_unified=1`, `no_kv_offload=0`,
  `split_mode=0`, `gpu=-1`, `main_gpu=-1`;
* для VLM: `mmproj_use_gpu=-1` = auto, при выбранном GPU mmproj следует за ним.

### 11.4. Thinking (чат)

* `reasoning` ∈ {`on`, `off`, `auto`}; `off` → движок форсирует `reasoning_budget = 0`;
* `on`/`auto` без бюджета → `reasoning_budget = -1`;
* `reasoning` задан, а `reasoning_format` нет ⇒ движок ставит `deepseek`; для **видимых**
  размышлений указываем `reasoning_format = "none"`;
* `reasoning` не задан ⇒ никакие флаги не отправляются; `auto` отдаёт решение шаблону модели.

### 11.5. Что проверить в A1/A2 (следствия для реализации)

1. **Pooling**: в `instance_params` поля `pooling_type` нет (оно есть только в bridge-API).
   Ожидаем, что кластер выводит pooling из `model_kind` (`EMBEDDINGS` → CLS, `RERANK` → RANK).
   **Обязательная проверка**: паритет эмбеддингов `cos_min ≥ 0,999` через **кластерный**
   инстанс (в W0 паритет снимался через bridge/CLI). При расхождении — создавать
   embedding/rerank через bridge-API с явным `pooling_type` и адресовать именем инстанса.
2. **`manual_devices_csv` vs `gpu`**: определить рабочий вариант первым тестом A1 и
   зафиксировать в конфиге; помнить, что `device_info.bridge_device_index` может отличаться
   от порядка перечисления cluster-устройств.
3. **Аудио (для W3)**: путь «файл → транскрипт» — сессия (`audio_session_create` →
   `start_transcription` в offline-режиме с параметрами Whisper → `push_encoded` → `flush` →
   чтение `TRANSCRIPTION_RESULT_JSON`/событий); устройство задаётся через
   `realtime_params.backend_name` (например, `"Vulkan0"`). В W2 достаточно создать инстанс
   `WHISPER` и убедиться, что он попадает на GPU.
4. **Ошибки**: `rc == 0` означает лишь успешный путь вызова — у bridge-результатов проверять
   `out.ok == 1` и `out.error_json`; у кластера — `last_error` в `instance_info`.


### 11.6. macOS-отличия (справочник для постпроектной фазы, §10.0 основного плана)

| Аспект | Windows (проверяется) | macOS arm64 (не проверяется до появления Mac) |
|---|---|---|
| Бэкенд движка | `cuda` или `vulkan` (выбор в установщике) | **только `metal`** (сборка `engine-macos-arm64-metal`), Intel Mac не поддерживается |
| Файлы рантайма | `multi-node-server.dll`, `llama-server-bridge.dll`, `llama-server-audio.dll` | `libmulti-node-server.dylib`, `libllama-server-bridge.dylib`, `libllama-server-audio.dylib` — держать имена в одной константе, а не в строках по коду |
| Загрузка библиотеки | `SetDllDirectoryW(<engine dir>)` обязателен, иначе `LoadLibraryExW failed` | зависимости разрешаются через `@loader_path/@rpath` (в CI движка ctypes-тест проходит без `DYLD_LIBRARY_PATH`), `libloading::Library::new(<abs path>)` достаточен |
| Дефолт устройства | **`gpu` не задан ⇒ CPU-only** (R32; это ловили в спайке 6в) | **`gpu` не задан ⇒ первый доступный GPU**; устройство всё равно задаём явно для предсказуемости |
| Бюджет VRAM | NVML (`memory.used/free/total`) — источник истины | **NVML нет**; бюджет = `MTLDevice.recommendedMaxWorkingSetSize` (~65–75 % RAM, поднимается `sysctl iogpu.wired_limit_mb`), своя занятость = `currentAllocatedSize`, чужая — по дельте `ioreg` (`IOAccelerator`) |
| Память в целом | дискретная VRAM | **унифицированная память**: превышение бюджета ⇒ свопинг и резкое падение скорости (аналог WDDM-оверкоммита из W0, но по другому лимиту) |
| ASR/VLM | CUDA/Vulkan | Metal |
| Установка | `setup.ps1`, Планировщик задач | `install_macos.command`, LaunchAgent, снятие `com.apple.quarantine`, ad-hoc подпись; **прогон только в постпроектной фазе** |

Источник фактов: открытый SDK движка (`github.com/openresearchtools/engine`) —
`docs/common-runtime-and-devices.md` (правила устройств), `docs/manual.md` («Metal — обычный
путь на macOS»), `.github/workflows/macos-arm64.yml` (единственный backend `metal`, smoke-тест
`list-devices` + ctypes-загрузка dylib), релизные ассеты `…-macos-arm64-metal.zip`/`…dmg`.

