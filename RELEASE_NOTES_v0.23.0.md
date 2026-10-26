# hermes-disk-search 0.23.0 — анонимизация документов

**Дата:** 26.10.2026 · **База:** 0.22.0 (`anonymizer-sidecar`)

> Новый функциональный блок: **анонимизация и деанонимизация документов** — Python-sidecar
> `sidecar/hds_anonymize` (движки перенесены из `anonymizer_proxy` без правок алгоритмов),
> Rust-клиент `crates/hds-anonymize`, CLI `hds anon` и карточка **«Анонимизация»** в `hds ui`.
> Схема базы поиска не менялась — переиндексация не нужна. Стек анонимизации
> (окружение + модели) **опциональный** и ставится отдельно: в архив он не входит.

## Что скачивать

| Файл | Нужен? | Что это |
|---|---|---|
| `hds-0.23.0-windows-x64.zip` (+ `.sha256.txt`) | **Да, обязательно** (Windows) | полная поставка: `bin\` (`hds`, `hds_mcp`, `llm_host`), установщик `setup.cmd`, обёртки `install-server.cmd` и `update.cmd`, манифесты, собранный sidecar **с тремя воркерами** (`hds_extract`, `hds_transcribe`, `hds_anonymize`, `hds_sidecar`, портативный CPython), `config.example.yaml` (с секцией `anonymize`), `server_config.example.yaml`, `README.md`, `README_ANONYMIZATION.md`, `README_SERVER_MULTIUSER.md`, `client\` (пакеты + скрипты) |
| `client-install.ps1` / `client-install.sh` | Для установки клиента | скрипт-установщик (токен аргументом): скачает пакет с сервера, распакует и подключит Cline |
| `hds-client-0.23.0-windows-x64.zip` (+ `.sha256.txt`) | Для клиента (если не через скрипт) | тонкий клиент: `bin\hds.exe`, мастер `setup-client.*`, правило/скилл, `version.txt` |
| `hds-client-0.23.0-macos-arm64.zip` (+ `.sha256.txt`) | Только для macOS | то же для Apple Silicon |
| `hds-engine-runtime-windows-x64-cuda.zip` | Только для **офлайн-установки** (NVIDIA) | рантайм движка CUDA; обычный установщик качает его сам по манифесту |
| `hds-engine-runtime-windows-x64-vulkan.zip` | Только для **офлайн-установки** (AMD/Intel) | рантайм движка Vulkan |
| `hds-0.23.0-macos-arm64.zip` (+ `.sha256.txt`) | Только для macOS (best-effort) | полная поставка для Apple Silicon |
| `hds-engine-runtime-macos-arm64-metal.zip` | Только для macOS офлайн | рантайм движка Metal |
| `hermes-disk-search-v0.23.0-windows.zip` / `-macos.zip` | Нет | архивы исходников (для разработчиков) |

**В архив не входят** (установщик готовит на машине): модели поиска, модели расшифровки и
**venv профиля расшифровки**, а также **стек анонимизации** — окружение `sidecar\venv-anonymize`
и модели GLiNER PII / navec / slovnet. Порядок установки — ниже.

## Что нового

* **Карточка «Анонимизация»** (`hds ui`): поле пути и серверный обзор папки (верхний уровень,
  форматы берутся из `hello` воркера), выходной каталог с сохранением в `config.yaml`, список
  найденных файлов, четыре кнопки — **«Скрыть всё»**, **«Скрыть указанное»**, **«Показать всё»**,
  **«Показать указанное»** — пофайловый прогресс и отчёт (вход → выход, метки `token/type/count`,
  счётчики найденных/ненайденных, предупреждения). Одна активная пачка: вторая отклоняется.
* **Серверные маршруты** (админ-доступ; в server-профиле POST пишется в аудит как `admin_op`):
  `GET /api/anonymize/ls`, `GET /api/anonymize/status`, `POST /api/anonymize/run`,
  `POST /api/config/anonymize-dirs`.
* **CLI** `hds anon`: `anonymize`, `values`, `deanon`, `deanon-values`, `inspect`, `warmup`,
  `check`; `--out-dir` перекрывает `anonymize.output_dir`.
* **Метки и база:** токены per-type (`[PERSON_1]`, `[ORG_1]`, …) с дедупликацией по
  (значение, тип); база — SQLite WAL `data/anonymize.db`, её ведёт только воркер.
  Выходные имена: `<имя>.anonymized.<ext>` / `<имя>.deanon.<ext>` (суффикс `.anonymized` у
  входа отбрасывается).
* **Приватность:** значения PII не возвращаются ни в UI, ни в API, ни в журналы — только метки,
  типы, число замен и длины списков; `inspect` показывает метки без значений.
* **Видеопамять:** при `anonymize.gpu: auto` акт берёт аренду у `llm_host`
  (`/internal/anon/enter|exit`, роль `anon`): низкоприоритетные роли выгружаются, занятый сеанс
  чата ожидается `lease_timeout_sec`; при недоступном хосте — работа на процессоре с
  предупреждением в отчёте, при `503 gpu_busy` — повтор позже.
* **Установщики:** `installers/build_anonymize_sidecar.ps1` (профили `cpu`/`cuda`, ровно один
  `onnxruntime` в venv, идемпотентно по маркеру) и `installers/fetch_anonymize_models.ps1`
  (HF-кэш GLiNER PII + navec/slovnet, манифест с ревизиями и sha256; **HF-токен не нужен**).
* **Диагностика:** `hds anon check` и пункт `anonymize` в `hds check` / `/api/diagnostics`
  (интерпретатор, веса, база, адрес аренды; недоступный `llm_host` — `warn`, fallback CPU).
* **Индексация:** `anonymize.output_dir` исключается из индексации неявно (как каталоги
  автотранскрибации): анонимизированные копии в базу поиска не попадают.
* **Конфиг:** в `config.example.yaml` добавлена секция `anonymize` (§2.6 плана); существующие
  установки могут её не иметь — карточка допишет при первом сохранении.

## Установка стека анонимизации (необязательно)

Полный режим («Скрыть всё») требует окружения и моделей; «Скрыть указанное» и «Показать»
работают без них. Окружение ~3–6 ГБ, модели ~1 ГБ; интернет нужен только на установку.

```powershell
# окружение: cpu (процессор) или cuda (видеокарта NVIDIA)
powershell -File installers\build_anonymize_sidecar.ps1 -Profile cpu -SelfTest
# модели: GLiNER PII в HF-кэш + navec + slovnet (+ манифест с ревизиями и sha256)
powershell -File installers\fetch_anonymize_models.ps1 -ModelsDir D:\models
# проверка готовности
hds anon check
```

Токен Hugging Face не требуется (модель публичная). `setup.ps1` этот стек **пока не запускает** —
это отдельный шаг; до установки пункт `anonymize` в `hds check` показывает `warn` (не блокирует
остальные проверки и работу программы).

## Миграция

* **База поиска не меняется** — схема прежняя, переиндексация не нужна.
* Секция `anonymize` в `config.yaml` необязательна: значения по умолчанию
  (`output_dir: data/anonymized`, `db_path: data/anonymize.db`, `gpu: auto`) действуют и без неё.
* База меток создаётся при первом акте. Файлы, анонимизированные прежней отдельной программой
  `anonymizer_proxy`, **не деанонимизируются**: схема меток другая (per-type + дедупликация),
  база не переносится — это зафиксированное решение.
* Обновление обычное: `update.cmd` (Windows) / `update.command` (macOS).

## Откат

* Стек анонимизации не затрагивает остальные функции: достаточно не устанавливать его
  (или удалить `sidecar\venv-anonymize` и модели `<models>\anonymize`). Пункт `hds check`
  вернётся к `warn`.
* `anonymize.gpu: off` — акты всегда на процессоре (без аренды видеопамяти).
* Полный откат версии — `update.cmd -RollbackTo 0.22.0` (или `git revert` + повторный выпуск
  с новым PATCH); данные поиска не мигрировались.

## Ограничения (важно)

* **Паритет NER** с исходным проектом (`anonymizer_proxy`) проверяется отдельным набором
  golden-сущностей (порог 0.3). В этом выпуске живьём проверены карточка и API, обзор папки,
  «скрыть указанное» и «показать» (round-trip), отчёт без значений; полный NER требует
  установленных моделей и проверяется на машине с весами — это следующий шаг.
* **Сессий нет:** программа хранит только входной и выходной файлы; «Показать» работает, пока
  жива база меток.
* **DOCX/XLSX** после «скрыть → показать» совпадают по тексту и структуре, но не байт-в-байт
  (маркер и очистка метаданных); TXT/MD — байт-в-байт, XML — текст и структура тегов.
* Файлы больше `anonymize.max_file_mb` пропускаются с предупреждением; занятый Word/LibreOffice
  файл даёт `file_busy` с подсказкой.
* Значения PII в карточке, API и журналах не отображаются — это сделано намеренно; для разведки
  файла используйте `hds anon inspect` (метки и их известность базе).

## Проверено

* `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` — зелёные (110 наборов, 0 failed).
* Python-набор sidecar: `python -m unittest discover -s sidecar/tests -t sidecar -p
  'test_anonymize_*.py'` — **28/28 OK** (модели NER не нужны: сценарии идут через список значений).
* Живой smoke UI (Windows, `hds ui`): обзор папки → «Скрыть указанное» на TXT → отчёт с меткой
  `[PERSON_1] type=PERSON count=1`, `matched_count=1`; в JSON статуса нет исходных значений;
  деанонимизация вернула исходный текст (`restored: ["[PERSON_1]"]`). Fallback на процессор
  при недоступном `llm_host` подтверждён строкой в отчёте.
* CI: `rust.yml` дополнен веткой `anonymizer-sidecar` и зависимостями набора анонимизации
  (`python-docx`, `openpyxl`, `lxml`); `release.yml` проверяет наличие `sidecar\hds_anonymize`
  в архиве, отсутствие `venv-anonymize` и работоспособность `hds anon --help`.

## Лицензии

Стек анонимизации: Natasha/navec/slovnet, pymorphy2, python-docx, openpyxl, setuptools — MIT;
lxml — BSD; numpy — BSD-3; onnxruntime — MIT; PyTorch — BSD-3-Clause; transformers,
huggingface_hub — Apache-2.0; GLiNER и модель `knowledgator/gliner-pii-large-v1.0` — лицензии по
PyPI и карточке модели (модели в архив не входят). Подробности — `NOTICE.md`.

