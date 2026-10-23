# PYTHON_SIDECAR_TRANSCRIBE_P4_P6_CODE_REVIEW_PROMPT.md — код-ревью P4–P6

> **Дата среза:** 23.10.2026. **Ветка:** `python-sidecar-transcribe`.
> **Код-срез:** `50fb854` (python-движок по умолчанию, DML заморожен, документация P5–P6);
> **HEAD — коммит этого документа** (проверьте `git log -1`).
> **База:** `server-multiuser` @ `f202128` (tag `v0.21.0`). Локально **8 коммитов впереди**
> origin, **не запушено**, рабочая копия чистая.
>
> **Как использовать:** открыть новый чат в режиме **Plan** (ревью без правок кода),
> скопировать **§9 целиком** и отправить первым сообщением. §§0–8 — контекст,
> карта изменений, инварианты, приоритеты и команды.
>
> **Что ревьюится:** только **P4–P6**: `5091e98` (P4 LLM-постобработка),
> `32b1e7c` (фикс потери слов), `c203edd` (P5 упаковка/железо + P6 legacy/релиз),
> `50fb854` (default engine python, DML заморожен, README/README_SERVER).
> P0–P3 (`c0d6915`, `f1525d0`) — вне скоупа, их контракты считаются входными.

---

## 0. Роль и рамки ревью

Вы — независимый ревьюер кода в этом репозитории. Задача — **найти дефекты,
риски и расхождения с планом**, а не переписать код. Ничего не менять, не
коммитить, не запускать деструктивные операции; допустимы прогоны тестов и
диагностики. Результат — отчёт с приоритетами, ссылками `файл:строка`,
обоснованием и воспроизведением.

Ограничения, которые **нельзя** нарушать (это решения владельца, не обсуждаются
как «неправильные»): UI и `engine: legacy` без отдельной задачи не трогаем;
`PLAN_*.md` не переписываем; DML на дискретной Radeon/Intel **заморожен как
экспериментальный** (живой карты нет); clean-install/CI-проверка отложена до
публикации релиза на GitHub; P0–P3 не переделываем.

## 1. Состояние на срезе

| Проверка | Значение |
|---|---|
| `git status --short --branch` | чисто, `## python-sidecar-transcribe` |
| `git log -9 --oneline` | handoff этого документа → `50fb854` → `c203edd` → `0556df9` → `32b1e7c` → `5091e98` → `f1525d0` → `c0d6915` → `f202128` |
| Коммитов впереди origin | **8** |
| Python-тесты | **166 OK** (`PYTHONPATH=sidecar python -m unittest discover sidecar\tests`) |
| Rust | `cargo fmt --check` — 0; `clippy --workspace --all-targets -D warnings` — 0; lib-тесты `hds-transcribe`/`hds-index`/`hds-llama`/`hds-cli` — ok |
| `cargo test -p hds-cli` (полный, интеграционные) | на срезе прогон был запущен и **не завершился** за отведённое время (линковка/долгие тесты); перед приёмкой — перепрогнать отдельно |
| `llm_host` | `/health` ok; после работ роли `chat/embedding/rerank` загружены |
| Модели/окружения | `sidecar\venv-transcribe` (CUDA, 7,4 ГБ), `sidecar\python`, `D:\hds-py-transcribe\{venv-cuda,venv-dml,gigaam-onnx}`, HF-кэш `D:\hds-py-transcribe\p5-models\hf-home` |
| Артефакты замеров | `tools/parity/out/py_transcribe/` (`P4_REPORT.md`, `P5_REPORT.md`, `P6_REPORT.md`, `p5/`) — gitignored |

## 2. Что реализовано в P4–P6 (карта для ревью)

### P4 — LLM-постобработка (`5091e98`, фикс `32b1e7c`)

| Файл | Что сделано |
|---|---|
| `sidecar/hds_transcribe/llm_patch.py` (новый, ~1000 строк) | кандидаты только по подозрительным фрагментам → `POST /v1/chat/completions` (`temperature 0`) → терпимый разбор JSON → валидатор → применение по `word_ids` и пересборка `block.text`; `release_gpu()` перед HTTP |
| `sidecar/hds_transcribe/worker.py` | слой LLM после глоссария в `postprocess`; единый журнал `edits.json`; сводка `postprocess.llm`; предупреждения вместо падения |
| `sidecar/hds_transcribe/glossary.py` | аддитивные `word_ids`, `source=glossary`, `status` |
| `crates/hds-transcribe/src/settings.rs` | `llm_from_config()`: `enabled/base_url/model/temperature/max_tokens/timeout_sec/max_edits_per_1000`, нормализация URL, clamp'ы, notices; 6 тестов |
| `crates/hds-transcribe/src/params.rs`, `crates/hds-index/src/transcribe.rs` | `LlmParams` → `PostprocessParams`; `postprocess` вызывается только при `llm.enabled` или непустом глоссарии |
| `crates/hds-cli/src/cmd/transcribe_check.rs`, `crates/hds-index/src/diag.rs` | LLM-статус и счётчики патчей последнего `transcript.json` |
| `config.example.yaml` | полная секция `transcribe.llm.*` |
| `sidecar/tests/test_llm_patch.py` | 33+ теста на фейковом OpenAI-сервере |

**Решения P4, которые ревью должно проверить:** патч принимается/отклоняется
**целиком**; спикер/таймкоды/`alignment` не меняются; запрещены `too_long` (>3×),
`too_short`, `content_lost`, `negation_lost`, `numbers_changed`,
`overlaps_applied`, `metadata_change`, `unknown_word_ids`, `cross_block`,
`non_contiguous_word_ids`, `speaker_mismatch`, `no_change`, `forbidden`,
`no_glossary_context`, `edit_limit`; «решение C» — канон в замене + сходство
исходного фрагмента с вариантом ≥ **0,80**; лимит
`ceil(max_edits_per_1000 × слов / 1000)`, минимум 1; по умолчанию `enabled: false`.

**Уже исправленный дефект:** `32b1e7c` — валидатор пропускал удаление
содержательных слов; добавлены `too_short`/`content_lost` (тесты
`test_deleting_whole_span_is_rejected`, `test_losing_one_content_word_is_rejected`).
Проверьте, что фикс не даёт ложных отказов и не обходится иначе.

### P5 — упаковка и железо (`c203edd`)

| Файл | Что сделано |
|---|---|
| `installers/sidecar_common.ps1` (новый) | портативный CPython, venv, `uv pip`→`pip`, `Test-SidecarImports`, `Copy-SidecarTree`, `Get-GpuVendor`/`Get-TranscribeProfile` (iGPU AMD/Intel → `cpu`), правка одной строки YAML `Set-ConfigPath(Nested)`, ASCII-only |
| `installers/build_transcribe_sidecar.ps1` (новый) | `-Profile cuda\|cpu\|dml`; torch сначала из индекса `cu128`/`cpu`, затем lock; DML — `gigaam --no-deps`; маркер `.hds-transcribe-profile` (profile + sha256 lock), `-Force`, `-SelfTest` |
| `installers/export_gigaam_onnx.ps1` (новый) | `<ModelsDir>\gigaam-onnx\<variant>\`, `export.json` (dtype/версии/sha256), `-CheckOnly`, `-Verify` (ORT CPU-сессии) |
| `installers/fetch_transcribe_models.ps1` (новый) | GigaAM-v3 + 3 HF-репозитория с **пинами ревизий**, `HF_HOME=<models>\hf-home`, `-HfToken`, `-Offline`, `-Force`, манифест `transcribe-models.json` |
| `sidecar/hds_transcribe/models.py`, `asr_onnx.py` | каталог `gigaam-onnx/<variant>` (+старый `onnx/*`), `has_onnx`, `resolve_onnx_variant` (`e2e_rnnt` ↔ `v3_e2e_rnnt`) — фикс несовпадения имён экспорта |
| `sidecar/hds_transcribe/worker.py` | `probe`: вендор GPU (WMI), версия ffmpeg, наличие GigaAM/HF/ONNX, `warnings` |
| `crates/hds-cli/src/cmd/transcribe_check.rs`, `crates/hds-index/src/diag.rs` | профиль/вендор/модели/ONNX/предупреждения; `TranscribeSummary.models`; ONNX требуется только при `backend: onnx`; тест |
| `setup.ps1`, `installers/install_server.ps1`, `installers/update.ps1` | `-TranscribeProfile`, `-HfToken`, `-SkipTranscribeStack`, `-SkipTranscribeModels`; шаги 7a/3b, правка конфига (`profile`, `models_dir`, `engine`, `asr.backend/provider`), non-fatal, `-NoModels`/`-WhatIf` |
| `installers/build_rust_release.ps1` | `-TranscribeProfile none\|cuda\|cpu\|dml` (ровно один venv), `-LegacyDiarizer` (P6.2), исключение venv из проверки «моделей в архиве» |
| `server_config.example.yaml` | секция `transcribe.*` (в серверном шаблоне её не было) |
| `.gitignore`, README sidecar | venv-каталоги; профили/модели/ONNX |

**Замеры на срезе** (dev: RTX 3060 12 ГБ + Ryzen 7 5700G, 64 ГБ):
CUDA meet0924 17,9 мин — 80,2 с, RTF 0,0503, пик 6 749 МБ;
DML iGPU frag60 — 21,8 с, RTF 0,287 (в 3× медленнее CPU);
CPU ASR-only frag60 — 6,6 с, RTF 0,0948;
CPU полный (ASR+pyannote) frag60 — 100,6 с, RTF 1,56 (диаризация 79,1 с);
полный 18-мин CPU-прогон остановлен после ~28 мин (CPU — fallback).

### P6 — legacy и релиз (`c203edd`, `50fb854`)

| Файл | Что сделано |
|---|---|
| `installers/build_rust_release.ps1` | `-LegacyDiarizer`: по умолчанию `DirectML.dll` не копируется и не требуется; переходный архив — с флагом (P6.2 **пакетный** уровень) |
| `crates/hds-llama/src/config.rs` | при `transcribe.engine: python` whisper-роль (базовая и реплики) не попадает в план `llm_host`; тест `python_engine_drops_whisper_role`; `legacy` — как раньше (P6.3) |
| `crates/hds-transcribe/src/settings.rs` | `transcribe.engine` по умолчанию **`python`**; `legacy` — явный откат; тест `engine_defaults_to_python_and_legacy_is_explicit` |
| `RELEASE_NOTES_v0.22.0.md` | новое, замеры, ключи, миграция, откат §12, ограничения (DML заморожен, CPU, whisperx, размер venv) |
| `NOTICE.md` | лицензии GigaAM/pyannote/wav2vec2/Silero/torch/ORT, gated-модели, офлайн, отсутствие `Rust-DirectML.dll` в default-пакете |
| `README.md`, `README_SERVER_MULTIUSER.md` | python-стек, профили, флаги установки, модели/HF-токен, `transcribe-check`, откат, DML заморожен, диск 20–30 ГБ |

**Не сделано осознанно:** compile-time вынос `hds-diarize`/`parakeet-rs`/`ort` в
Cargo-feature (P6.2 на уровне сборки) — записано в `P6_REPORT.md`; clean-install и
CI — после публикации релиза; DML на дискретной карте — заморожен.

## 3. Инварианты, которые ревью обязано проверить

1. **P4 не меняет акустику:** ни один принятый патч не меняет `speaker`,
   `start`, `end`, `alignment`, `word_id`; текст `.orig.md` — после postprocess,
   исходный ASR — в `transcript.json`; `edits.json` — аддитивный (старые поля P3
   сохранены).
2. **P4 не теряет и не добавляет смысл:** числа/даты/отрицания, содержательные
   слова вне термина, границы слов; ложный отказ лучше ложной правки.
3. **P4 деградирует, а не падает:** недоступный `llm_host`, таймаут, HTTP 4xx/5xx,
   мусор вместо JSON, огромный ответ — warning, `.md` пишется, задание успешно.
4. **P5 идемпотентен:** повторный `build_transcribe_sidecar` ничего не
   переустанавливает; повторный `setup/install_server/update` не ломает конфиг и
   не перекачивает модели; `-Force` — единственный способ пересборки.
5. **P5 не разрушает существующее:** извлечение документов (`hds_extract`) и
   `engine: legacy` не меняются; `sidecar\python` после `build_transcribe_sidecar`
   содержит зависимости извлечения; `setup` не ставит `engine: python`, если venv
   не собрался.
6. **ONNX-путь корректен:** `e2e_rnnt` (encoder/decoder/joint + yaml) и `ctc`
   (один граф); имена `v3_*` резолвятся; DirectML SessionOptions
   (`SEQUENTIAL`, `enable_mem_pattern=False`); CPU/DML не вызывают `torch.cuda.*`.
7. **P6.3:** при `engine: python` в плане `llm_host` нет роли `whisper` (включая
   реплики из `llm.whisper*`/`gpu.devices[].roles`), `status` это подтверждает;
   при `legacy` роль создаётся.
8. **P6 default:** конфиг **без** `transcribe` даёт `engine=python`; `legacy` —
   только явный ключ; откат не требует миграции; при отсутствии venv ошибка
   понятная и задание не «висит».
9. **Релиз:** по умолчанию в архиве нет `bin\DirectML.dll` и моделей;
   `-TranscribeProfile` кладёт ровно один venv; `-LegacyDiarizer` — переходный
   вариант; `*.onnx` из venv не считаются «моделями проекта».
10. **Документация = код:** README/README_SERVER/RELEASE_NOTES/NOTICE не обещают
    того, чего нет (профили, флаги, DML «экспериментальный», размеры, команды).

## 4. Приоритеты ревью (что искать в первую очередь)

### P0 — безопасность и целостность данных

* **Обход валидатора P4.** Unicode-нормализация (ё/е, й, латиница/кириллица,
  невидимые символы), пунктуация внутри/вне `word_ids`, регистр, дефисы,
  составные числа (`1 000,50`), отрицания не из списка («не», «нет», «без»,
  «кроме»), замена числа словом и наоборот, `content_lost`/`too_short` на
  легитимных сокращениях (`эрпи система` → `ERP-система`), дубли/перестановки
  `word_ids`, патч через границу блока/спикера, пустая замена, замена на пробел.
* **Prompt injection.** Текст транскрипта попадает в промпт: инструкции внутри
  записи не должны менять поведение; модель обязана вернуть только JSON;
  валидатор не доверяет `reason`; ответ не может задать `word_ids` чужих блоков.
* **HTTP-клиент P4.** Схема URL (только http/https), отсутствие авто-редиректов
  на чужие хосты, лимит размера ответа, таймаут, `temperature=0`, отсутствие
  утечки API-ключа/промпта в журнал, поведение при 401/429/500, ограниченные
  повторы (если есть).
* **PowerShell-установщики.** `-Force` удаляет venv (`Remove-Item -Recurse`) без
  проверки, что воркер не запущен; `Set-ConfigPath` правит YAML построчно —
  проверьте кавычки с апострофами, `\n` vs `\r\n`, BOM, многострочные значения,
  чужие секции; токен HF в командной строке дочернего процесса виден в списке
  процессов; небезопасная склейка путей/команд; запуск без администратора; пути
  с пробелами.
* **Данные моделей.** `fetch_transcribe_models.ps1 -Offline` при отсутствующей
  модели: проверьте код возврата и сообщение (`ok=false` может не давать
  non-zero exit); частичный провал не должен затирать валидный манифест;
  `HF_HOME`/`HF_HUB_OFFLINE` не должны протекать в другие шаги установщика.

### P1 — корректность поведения и совместимость

* **P4:** кандидаты (не flood), лимит правок `ceil`, порядок применения патчей,
  пересборка `block.text` без потери пунктуации, идемпотентность повторного
  `postprocess`, `needs_review` не меняет текст, `edits.json` пишется атомарно,
  `postprocess` не запускает ASR и не грузит модели заново.
* **P5:** `Get-TranscribeProfile` без WMI/с двумя картами; `Ensure-PortablePython`
  для repo- и dist-дерева; `Copy-SidecarTree` не теряет `hds_sidecar`; порядок
  «build_sidecar → venv»; `requirements*.lock` (torch из индекса, `uv`/`pip`
  fallback); `models.onnx_dir` fallback; `has_onnx` для CTC; `transcribe-check`
  не падает без venv/ffmpeg/GPU.
* **P6:** default python для **существующих** установок без venv (сервер без
  `-Fetch`, шаблон уже с `engine: python` — упадут ли задания?); фильтрация
  whisper в `gpu.devices`/`role_cards`; старый `llm.whisper` в конфигах;
  `build_rust_release` без DirectML для legacy-диаризации — понятное ли
  предупреждение.
* **Конфиг:** приоритет `transcribe.models_dir` → `HDS_MODELS_DIR` → ProgramData
  согласован между Rust (`settings.rs`, `diag.rs`) и Python (`models.py`);
  `asr.backend/provider`, записанные установщиком, не конфликтуют с `auto`.
* **Документация:** README/README_SERVER обещают `-TranscribeProfile`, `-HfToken`,
  `transcribe-check`, 20–30 ГБ, откат — сверьте с фактическими параметрами.

### P2 — тесты, стиль, поддерживаемость

* Тесты P4 покрывают отказы/приёмы патчей, а не только happy path; нет тестов,
  зависящих от сети/моделей; фейковый HTTP-сервер корректно закрывается.
* `cargo clippy -D warnings`, `fmt`; PowerShell — `Parser::ParseFile` (PS 5.1),
  ASCII-only; нет мёртвого кода и дублей (`fetch_diarization_model` в README
  раньше был продублирован — проверьте, не осталось ли копий).
* `NOTICE.md` соответствует фактическим моделям/пакетам; `RELEASE_NOTES_v0.22.0.md`
  не противоречит `settings.rs` и README.

## 5. Как проверять (команды)

Все артефакты — только в `tools/parity/out/py_transcribe/` (gitignored). Не менять
`config.yaml`, `data/`, `index.db`; для проверок использовать `HDS_CONFIG` с
временным конфигом.

```powershell
# базовые проверки (обязательны)
$env:PYTHONPATH='sidecar'; python -m unittest discover sidecar\tests
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# отдельно интеграционные hds-cli (на срезе не завершились — прогоните с запасом)
$env:CARGO_BUILD_JOBS='2'; cargo test -p hds-cli

# PowerShell 5.1: синтаксис изменённых сценариев
$files = @('setup.ps1','installers\sidecar_common.ps1',
  'installers\build_transcribe_sidecar.ps1','installers\export_gigaam_onnx.ps1',
  'installers\fetch_transcribe_models.ps1','installers\install_server.ps1',
  'installers\update.ps1','installers\build_rust_release.ps1')
foreach ($f in $files) {
  $errs = $null
  [System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path $f),[ref]$null,[ref]$errs) | Out-Null
  if ($errs) { "$f : $($errs.Message)" } else { "$f : OK" }
}

# P4 e2e через резидента (llm_host уже загружен; не временный llama-server)
& tools\parity\out\py_transcribe\venv-cuda\Scripts\python.exe `
  tools\parity\out\py_transcribe\p4_llm_run.py `
  tools\parity\out\py_transcribe\venv-cuda\Scripts\python.exe `
  tools\parity\out\py_transcribe\p5\verify\src_0924.transcript.json `
  tools\parity\out\py_transcribe\p5\verify\llm_review.json `
  --base-url http://127.0.0.1:8010/v1 --model chat --max-edits 10 `
  --glossary sidecar\hds_transcribe\glossary.example.yaml
& tools\parity\out\py_transcribe\venv-cuda\Scripts\python.exe `
  tools\parity\out\py_transcribe\p4_eval.py `
  tools\parity\out\py_transcribe\p5\verify\llm_review.json `
  tools\parity\out\py_transcribe\p5\verify\llm_review_eval
# ожидание: 18 окон, 9 предложено, 1 принято, 8 отклонено, WER 17,82 / CER 9,52
```

`llm_host`: `llm_host.exe status` / `GET /health`; если снова R29 (свободно
1655 МиБ при свободных NVML) — elevated `Restart-Service hermes-disk-search-llm-host`.
Перед CUDA-замерами роли можно выгрузить (`llm_host unload chat|embedding|rerank`),
после — вернуть (`llm_host load …`). Модели **не скачивать заново**; для `-Offline`
используется `D:\hds-py-transcribe\p5-models` (junctions на существующие кэши).

## 6. Рамки и запреты ревью

* **Только чтение кода и диагностика.** Не править файлы, не коммитить, не
  менять `PLAN_*.md`, `PLAN_AUTO_TRANSCRIBE.md`, UI и legacy без отдельной задачи.
* Не удалять `data/`, `index.db`, `config.yaml`, модели, venv; не запускать
  `setup/install_server/update` «на боевом» конфиге — только `-WhatIf`/временный
  `HDS_CONFIG`/отдельная папка.
* `-Force` установщиков не запускать без явной необходимости (удаляет venv).
* Если нужен elevated-рестарт службы — можно, но после вернуть роли и указать это
  в отчёте.
* Не выдавать «не проверил» за «багов нет»: явно перечисляйте, что не проверяли
  (clean-install/CI, дискретная Radeon, macOS, полный 18-мин CPU).

## 7. Формат отчёта

1. **Сводка:** 3–7 строк — общий вердикт и самые опасные находки.
2. **Таблица находок** (по убыванию важности):

   | # | Важность | Файл:строка | Что не так | Почему важно | Как воспроизвести | Ожидаемое поведение |
   |---|---|---|---|---|---|---|
   | 1 | blocker/high/medium/low/nit | `sidecar/...py:123` | … | … | команда/сценарий | … |

3. **Проверенные утверждения:** что реально прогнали (тесты, P4 e2e, PS-парсер,
   `transcribe-check`) и с каким результатом.
4. **Не проверено:** список с причиной (нет железа/времени/данных).
5. **Вопросы к владельцу** (если решение продуктовое, а не дефект).

Важность: **blocker** — потеря данных/обход гейта/падение задания; **high** —
неверное поведение на типовом сценарии; **medium** — совместимость/диагностика;
**low** — UX/документация; **nit** — стиль.

## 8. Решения, которые НЕ являются багом (не переоткрывать)

| Решение | Почему так |
|---|---|
| LLM-постобработка выключена по умолчанию | Требование владельца (P4_REPORT §8.4); включается ключом/для отдельных файлов |
| `.orig.md` — после postprocess, исходный ASR только в `transcript.json` | Иначе кнопка «присвоить имена» откатила бы LLM-правки (PLAN §8) |
| DML на дискретной AMD/Intel заморожен, не приёмочный | Нет живой Radeon/Intel (решение 23.10.2026); на iGPU установщик выбирает `cpu` |
| P6.2 не вынесен в Cargo-feature | Отдельная задача; сейчас исключён только `DirectML.dll` на уровне пакета |
| Clean-install и GitHub Actions не проверены | Отложено до публикации релиза (решение 23.10.2026) |
| CPU-профиль медленный (диаризация ~1,3× RT) | Осознанный fallback; CUDA — боевой профиль |
| `whisperx` не установлен в lock (закомментирован) | Опция качества; `hybrid` деградирует с warning, явный `whisperx` — `align_failed` |
| Модели и venv не в репозитории | `.gitignore`; venv в `app\<ver>` — цена варианта A |
| В качестве LLM по умолчанию `chat` (Qwen3.5-9B Q6_K) | Рекомендация Q4_K_M — в отчёте; конфиг машины владельца не меняем |
| `hds whisper-check` оставлен алиасом | Контракт CLI (PLAN §13 п.10) |

## 9. Промт для нового чата (скопировать целиком)

```text
Ты — независимый ревьюер кода в проекте hermes-disk-search
(C:\Users\Sasha\hermes-disk-search), ветка python-sidecar-transcribe.
Режим — Plan: только чтение кода, тесты и диагностика, БЕЗ правок, коммитов и
деструктивных операций. Задача — код-ревью этапов P4–P6.

СОСТОЯНИЕ (проверь сам, не пересчитывай):
- Код-срез 50fb854, база f202128 (v0.21.0), HEAD — коммит этого handoff,
  8 коммитов впереди origin, рабочая копия чистая.
- В скоупе: 5091e98 (P4 LLM-постобработка), 32b1e7c (фикс content_lost),
  c203edd (P5 упаковка/железо + P6 legacy/релиз), 50fb854 (engine python по
  умолчанию, DML заморожен, README/README_SERVER).
- P0–P3 вне скоупа; их контракты — входные данные.
- Прочитай PYTHON_SIDECAR_TRANSCRIBE_P4_P6_CODE_REVIEW_PROMPT.md (этот контекст):
  §§2–8 — карта изменений, инварианты, приоритеты, команды, известные решения.
- Отчёты: tools/parity/out/py_transcribe/{P4_REPORT.md,P5_REPORT.md,P6_REPORT.md}
  (gitignored).

ПЕРВЫЕ ДЕЙСТВИЯ:
1) git status --short --branch; git --no-pager log -9 --oneline --decorate;
   git rev-list --count origin/server-multiuser..HEAD
2) прочитай документ ревью и отчёты P4/P5/P6; составь список файлов к чтению.
3) прогони базовые проверки:
   $env:PYTHONPATH='sidecar'; python -m unittest discover sidecar\tests
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all -- --check
   (полный `cargo test -p hds-cli` на срезе не завершился — прогони отдельно
   с CARGO_BUILD_JOBS=2 и зафиксируй результат).
4) P4 e2e через резидента llm_host (:8010, model=chat, max_edits=10,
   glossary.example.yaml, tools/parity/out/py_transcribe/p5/verify/src_0924.transcript.json):
   ожидание 18 окон / 9 предложено / 1 принято / 8 отклонено, WER 17,82 / CER 9,52.
   Если llm_host отдаёт 409 «свободно 1655 МиБ» (R29) — elevated
   Restart-Service hermes-disk-search-llm-host, затем верни роли.

ЧТО ЧИТАТЬ (минимум):
- P4: sidecar/hds_transcribe/{llm_patch.py,worker.py,glossary.py},
  sidecar/tests/test_llm_patch.py, crates/hds-transcribe/src/{settings.rs,params.rs},
  crates/hds-index/src/transcribe.rs (postprocess), transcribe_check.rs, diag.rs.
- P5: installers/{sidecar_common.ps1,build_transcribe_sidecar.ps1,
  export_gigaam_onnx.ps1,fetch_transcribe_models.ps1}, setup.ps1,
  installers/{install_server.ps1,update.ps1,build_rust_release.ps1},
  sidecar/hds_transcribe/{models.py,asr_onnx.py,worker.py::probe},
  crates/hds-cli/src/cmd/transcribe_check.rs, crates/hds-index/src/diag.rs,
  server_config.example.yaml, config.example.yaml.
- P6: crates/hds-llama/src/config.rs, crates/hds-transcribe/src/settings.rs,
  build_rust_release.ps1, RELEASE_NOTES_v0.22.0.md, NOTICE.md,
  README.md, README_SERVER_MULTIUSER.md.

ПРИОРИТЕТЫ (подробно — §4 документа):
P0: обход валидатора P4 (unicode/пунктуация/регистр/числа/отрицания/word_ids/
    границы блоков), prompt injection, HTTP-клиент (схема/редиректы/лимит/
    таймаут/утечки), PowerShell (удаление venv при -Force, правка YAML, токен
    в cmdline, пробелы/кавычки), fetch -Offline (exit code/частичный провал/
    утечка HF_HOME).
P1: P4 кандидаты/лимит/порядок/идемпотентность/атомарность edits.json;
    P5 профиль по GPU, Ensure-PortablePython для repo/dist, ONNX-резолв
    (v3_*, ctc), transcribe-check без venv/GPU; P6 default python для старых
    установок без venv, фильтрация whisper (gpu.devices/role_cards), legacy
    без DirectML; согласованность models_dir Rust↔Python; README vs код.
P2: тесты (не только happy path), clippy/fmt, PS Parser, дубли, лицензии/NOTICE.

ОБЯЗАТЕЛЬНО ПРОВЕРЬ ИНВАРИАНТЫ (§3): P4 не меняет speaker/start/end/alignment/
word_id и не теряет смысл; деградация без llm_host не валит задание; P5
идемпотентен и не ломает извлечение/legacy; ONNX и DML-safe options; P6.3
whisper не в плане при engine=python; конфиг без transcribe → python; релиз без
DirectML.dll по умолчанию и ровно один venv при -TranscribeProfile.

НЕ ЯВЛЯЕТСЯ БАГОМ (§8): LLM off по умолчанию, .orig после postprocess, DML
заморожен, P6.2 без Cargo-feature, clean-install/CI отложены, медленный CPU,
whisperx опционален, модели/venv не в репозитории, chat=Qwen3.5 Q6.

ЗАПРЕТЫ: не править файлы, не коммитить, не менять PLAN_*.md/UI/legacy, не
удалять data//index.db/config.yaml/модели/venv, не запускать -Force и
setup/install_server/update на боевом конфиге (только -WhatIf/временный
HDS_CONFIG/отдельная папка). Можно запускать тесты, transcribe-check и P4 e2e;
артефакты — только tools/parity/out/py_transcribe/.

ОТЧЁТ (§7): сводка 3–7 строк; таблица находок (важность blocker/high/medium/low/
nit, файл:строка, что не так, почему важно, как воспроизвести, ожидаемое
поведение); список реально прогнанных проверок с результатами; список «не
проверено» с причинами; вопросы к владельцу. Не выдавай «не проверял» за «багов
нет».

Начни с подтверждения состояния git и списка прочитанных файлов, затем
приступай к ревью по приоритетам P0→P2.
```

