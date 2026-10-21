# DIARIZATION_NEMOTRON_FIXES_HANDOFF_PROMPT.md — промт для нового чата

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать всё, что ниже
> разделителя `---`, и отправить одним сообщением. Файлы
> `DIARIZATION_NEMOTRON_CODE_REVIEW.md` и `PLAN_DIARIZATION_NEMOTRON_FIXES.md` должны
> лежать в корне репозитория (они уже созданы).
> **Статус:** промт составлен 21.10.2026 по итогам код-ревью; решения Q1/Q2/Q6 зафиксированы
> как рекомендованные — если владелец в новом чате не скажет иначе, выполнять их.

---

Ты — Cline, продолжаешь работу в репозитории `c:\Users\Sasha\hermes-disk-search`
(Windows 11, ветка `server-multiuser`, HEAD `abda621`). Режим — **Act**: можно править код,
запускать сборки/тесты и процессы по правилам `cline-rules/dev-machine.md`.

## Задача

Выполнить исправления по код-ревью доработки «Диаризация Nemotron-3» (W0–W9). Ревью нашло
3 блокирующих, 6 средних и 7 низких замечаний. Источники истины (прочитать **целиком** до
любых правок):

1. `DIARIZATION_NEMOTRON_CODE_REVIEW.md` — находки B1–B3, M1–M6, L1–L7 с файлами/строками,
   последствиями и рекомендациями.
2. `PLAN_DIARIZATION_NEMOTRON_FIXES.md` — задачи F1–F16: шаги, фрагменты кода, тесты,
   критерии приёмки, порядок этапов, среда (§11), Definition of Done (§12).
3. `PLAN_DIARIZATION_NEMOTRON.md` §3/§7 — контракты и решения владельца (не переписывать).
4. `PLAN_AUTO_TRANSCRIBE.md` §5 — формат `.md`, `.orig.md`, `.speakers.json` (не менять).
5. `cline-rules/dev-machine.md` — что разрешено делать с процессами/GPU/конфигом.

Номера строк в ревью/плане могут сместиться после первых правок — ориентируйся на имена
функций и поиск (`rg`), а не только на номера.

## Решения, которые считаются согласованными

Если пользователь в этом чате явно не возразит, реализуй рекомендованные варианты:

| Вопрос | Решение |
|---|---|
| Q1 (F2) | Клиент **не отправляет** `diarization_backend`, если `auto_transcribe.diarization_backend` не задан; хост применяет правило №3 (конфиг → авто). |
| Q2 (F9) | `diarization_required` — **аддитивный** ключ тела `/internal/transcribe`, тело сильнее `diarization.required` хоста (для nemotron3; legacy sortformer по-прежнему жёстко падает). |
| Q3 (F7/F11) | F7 (`hds-ort`) — в этот релиз; F11 (дедупликация backend/ffmpeg) — отдельной задачей после тега. |
| Q4 (F8) | Предел длительности записи — **6 ч**, понятная ошибка до ASR; RF64 — отдельная задача. |
| Q5 (F16) | Потоковая запись/чтение WAV — после тега, если W7-числа устраивают. |
| Q6 (F10) | В nemotron-ветке ответ содержит фактический `mode: "transcript"` (диаризация всегда даёт `.md`). |

Если владелец выбрал альтернативу — выполни её и зафиксируй отступление в отчёте.
## Порядок работ (не переставлять блокеры)

**Этап 0 — базовая линия (до правок):**
1. `git status --short` (ожидается чисто, кроме untracked-документов ревью/плана/промта).
2. `Remove-Item Env:\HDS_ROOT -ErrorAction SilentlyContinue`.
3. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` — сохранить вывод в `tools/parity/out/fixes/baseline.txt`.
4. Добавить тест-репро из приложения A ревью и убедиться, что он **падает** (B1).

**Этап 1 — блокеры (обязательно):**
* **F1** — фикс `split_text` в `crates/hds-diarize/src/merge.rs` + 4 теста (план F1);
  живой прогон 18 мин: `.md` не изменился.
* **F2** — `backend_explicit` в `hds-index::transcribe::DiarizationSettings`; отправка
  `diarization_backend` только при явном ключе; диагностика фактического запроса
  (`requested_backend_check`, `--dry-run`, `hds check`/`whisper-check --json`); правки
  README/RELEASE_NOTES/конфигов; тесты (план F2).
* **F3** — докачка Nemotron-3 в `installers/update.ps1` (параметр `-NoModels`, ошибка не
  валит обновление) и `installers/update.command`; тесты `install_server_parity`; правка
  релизных заметок/README.
* После этапа: полный `fmt`/`clippy`/`test` + живые сценарии nemotron3/sortformer/off/без модели.

**Этап 2 — средние:**
* **F4** — предупреждения `auto_transcribe` по backend (`warnings(&Config)`), тест.
* **F5** — `NemotronCheck` в `diag`, хеш только для nemotron3/auto, перенос
  `resolved_diar_backend()` из `Host::start` на первое задание; замер `hds check` при `off`.
* **F6** — кэш отрицательного результата `verify_against` + тест.
* **F7** — новый `crates/hds-ort` (единый `OnceLock<Environment>`), `hds-clip::env` и
  `hds-diarize::ort_env` — обёртки; тест «оба API в одном процессе»; `cargo tree -i ort`.
* **F8** — `encode_wav_checked`/RF64-ошибка/guard 6 ч в `transcribe_nemotron3`; тесты.
* **F9** — `diarization_required` в теле → `transcribe_nemotron3`; тесты.
* После этапа: полный прогон + живые сценарии §7.3 плана.

**Этап 3 — низкие/техдолг:** F10, F12–F15 (и F11/F16, если владелец разрешил в этом чате).
Для каждой — тесты из плана. F15 (pre-existing гонка staging) и F16 (память) можно отложить
с записью причины.

## Ограничения (жёсткие)

* Не переписывать `PLAN_DIARIZATION_NEMOTRON.md` и решения владельца §7; правки — только в
  код, тесты и пользовательские/релизные документы.
* Не менять формат `.md` (`### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`, `UNASSIGNED`), `.orig.md`,
  `.speakers.json`; новые ключи API — только аддитивные (кроме фактического `mode` по F10).
* Legacy-путь sortformer должен остаться рабочим без изменений поведения.
* Не коммитить, не ставить теги и не пушить без отдельной команды владельца.
* Не трогать чужие незакоммиченные изменения; не делать `git clean`/`git reset --hard`.
* Артефакты прогонов — только в `tools/parity/out/fixes/` (каталог гитигнорится). В рабочем
  дереве не должно остаться `*.onnx`, `*.gguf`, логов, временных WAV.
* Процессы разрешено останавливать/запускать (`hds`, `llm_host`, `hds watch`, `hds mcp-http`,
  `hds ui`, пробы), GPU можно освобождать; после работ вернуть `llm_host` (задача Планировщика
  или `llm_host run`). `cargo build --release` падает, если `target\release\*.exe` заняты.
* Установщик/обновление проверять на **копии** установки (`-WhatIf`, `-NoModels`), не на
  боевой.

## Среда (проверено 21.10.2026)

* Rust `1.97.1`, MSRV воркспейса `1.92`; ветка `server-multiuser`, тег `v0.21.0` **не
  поставлен** — правки входят в 0.21.0.
* Модель: `D:\diar-spike-models\nemotron3\nemotron3_diar_v3.onnx` (400 506 656 байт). В
  `%APPDATA%`/`%ProgramData%` её нет. Для живой проверки:
  `$env:HDS_MODELS_DIR = 'D:\diar-spike-models'` (или явно
  `diarization.nemotron3.model: 'D:/diar-spike-models/nemotron3_diar_v3.onnx'`).
* WAV: `D:\diar-spike-data\frag60.wav`, `meet0512.wav` (56 мин), `meet0514.wav` (77 мин),
  `meet0924.wav` (18 мин).
* `ffmpeg` есть в `PATH` (winget Gyan.FFmpeg 9.0.1).
* `HDS_ROOT` в среде ассистента указывает на репозиторий и валит 2 теста `hds-llama`
  (`host_resident`) — перед тестами очищать переменную (см. этап 0).
* `target\release\*.exe` устарели (14.10.2026); `DirectML.dll` на месте. Перед релизной
  сборкой остановить `hds`/`llm_host` (сейчас в системе могут быть запущены 4 × `hds` и
  `llm_host`).
* Песочницы: `tools/parity/out/diar_nemotron/{w4_host.yaml,w5_check.yaml,
  w5_check_missing.yaml,w7_host.yaml}`. Артефакты — `tools/parity/out/fixes/`.
## Проверки (минимум)

**Автотесты:**
```powershell
Remove-Item Env:\HDS_ROOT -ErrorAction SilentlyContinue
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Плюс точечные тесты из §7.2 плана (F1: `merge::`, F2: `transcribe::`/`transcribe_media`,
F3: `install_server_parity`, F5/F4: `diag::`/`autotranscribe::`, F7: новый
`ort_env_singleton`, F8: `wav::`, и т.д.).

**Живые проверки (18-мин файл, песочница `w7_host.yaml`, порт-база 8110):**
1. `diarization.backend: nemotron3` + `HDS_MODELS_DIR=D:\diar-spike-models` — `.md` с
   `SPEAKER_*`, 5 спикеров, абсолютные таймкоды, `stats.timings_sec` на месте.
2. `diarization.backend: sortformer`, **без** `auto_transcribe.diarization_backend` — legacy
   `.md` (4 спикера + `UNASSIGNED`), формат не изменился (после F2).
3. `diarization.backend: off` — плоский текст, `diarization.enabled: false`.
4. Без секции `diarization` и без модели (временный пустой `HDS_MODELS_DIR`) — хост выбирает
   sortformer с предупреждением (решение №3), задание не падает.

**Диагностика:** `hds check`, `hds whisper-check --json`, `hds transcribe-watch --dry-run`
должны показывать согласованный backend; при `off` модель не должна хешироваться (проверить
по времени/логу). `hds check` и живые прогоны запускать с тем же `HDS_MODELS_DIR`.

**Обновление:** копия установки + `installers/update.ps1 -Root <копия> -WhatIf`; затем
реальный прогон с `-NoModels` и без — в сводке должно быть «модель докачана/пропущена»,
`hds check` — без fail по диаризации.

## Формат отчёта в конце

1. **Статус задач:** таблица F1–F16 — `сделано` / `отложено (причина)`; для сделанных —
   файлы и ключевые изменения.
2. **Проверки:** команды и результаты (`fmt`/`clippy`/`test`, число тестов; точечные тесты).
3. **Живые проверки:** четыре сценария + числа (спикеры, время, RSS если меряли).
4. **Документация:** что обновлено и где.
5. **Отклонения от плана:** если владелец выбрал альтернативу Q1/Q2 или что-то отложено.
6. **Состояние дерева:** `git status --short`, `git diff --stat`; подтверждение, что
   артефакты только в `tools/parity/out/fixes/`, коммитов/тегов нет.

## Первое действие

1. Прочитай `DIARIZATION_NEMOTRON_CODE_REVIEW.md` и `PLAN_DIARIZATION_NEMOTRON_FIXES.md`
   (полностью), затем `PLAN_DIARIZATION_NEMOTRON.md` §3/§7 и `PLAN_AUTO_TRANSCRIBE.md` §5.
2. Составь краткий todo по F1–F16 (этапы 1–3) и покажи его пользователю **до** правок.
3. Если пользователь не уточнил Q1/Q2/Q6 — действуй по таблице решений выше.
4. Начинай с этапа 0 и F1; не переходи к F2, пока тест-репро F1 не станет зелёным.
5. Критерий завершения — §12 плана (`Definition of Done`). Не коммить и не ставь тег без
   отдельной команды.


