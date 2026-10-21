# PLAN_DIARIZATION_NEMOTRON_FIXES.md — план исправления замечаний код-ревью

> **Основание:** `DIARIZATION_NEMOTRON_CODE_REVIEW.md` (21.10.2026, ветка `server-multiuser`,
> коммиты `968c2ab..abda621`). **Статус:** черновик к исполнению; задачи F1–F3 — обязательны
> до тега `v0.21.0`, F4–F9 — желательно в тот же релиз, F10–F16 — техдолг.
> **Связанные документы:** `PLAN_DIARIZATION_NEMOTRON.md` (решения владельца §7),
> `PLAN_AUTO_TRANSCRIBE.md` §5 (контракт `.md`/sidecar), `RELEASING.md`,
> `cline-rules/dev-machine.md` (разрешено останавливать процессы/освобождать GPU).
> **Промт для нового чата:** `DIARIZATION_NEMOTRON_FIXES_HANDOFF_PROMPT.md`.
> **Инварианты, которые нельзя нарушить:** формат `.md` (`### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`,
> `UNASSIGNED`), `.orig.md`/`.speakers.json`, ключи `POST /internal/transcribe` (только
> аддитивные добавления), legacy-путь sortformer, решения владельца §7 плана.

## 1. Цель и границы

Цель — устранить 3 блокирующих, 6 средних и 7 низких замечаний код-ревью, не меняя формат
результата и не ломая legacy-ветку. Работы ведутся в том же порядке, что и ревью: сначала
дефекты, которые теряют задания/данные или ломают обновление, затем UX/диагностика, затем
техдолг.

Границы:

* не переписывать `PLAN_*.md` с зафиксированными решениями (правки — только в этот план и в
  релизные/пользовательские документы);
* не менять `.md`-формат, sidecar и существующие ключи ответа (кроме фактического значения
  `mode`, см. F10);
* новые зависимости — только по согласованию; F7/F11 добавляют внутренние крейты без
  внешних зависимостей, кроме уже запиненного `ort = "=2.0.0-rc.15"`;
* артефакты прогонов — в `tools/parity/out/` (гитигнорится), репозиторий после работ чистый.

## 2. Сводная таблица задач

| ID | Ревью | Приоритет | Суть | Основные файлы |
|---|---|---|---|---|
| F1 | B1 | блокер | паника `split_text` на 1–2 словах и 3–4 частях | `hds-diarize/src/merge.rs` |
| F2 | B2 | блокер | клиент не должен навязывать backend; диагностика — по фактическому запросу | `hds-index/src/{transcribe,diag}.rs`, `hds-cli/src/cmd/{transcribe,whisper_check}.rs`, docs |
| F3 | B3 | блокер | `update` докачивает модель Nemotron-3; релизные заметки | `installers/update.ps1`, `installers/update.command`, docs |
| F4 | M1 | средний | предупреждение демона учитывает backend | `hds-index/src/autotranscribe.rs` |
| F5 | M2 | средний | не хешировать 400 МБ, когда backend не nemotron3; хост — лениво | `hds-index/src/diag.rs`, `hds-llama/src/host.rs` |
| F6 | M3 | средний | кэшировать и отрицательный результат проверки модели | `hds-diarize/src/sortformer.rs` |
| F7 | M4 | средний | одно ORT-окружение на процесс (`hds-ort`) | новый `crates/hds-ort`, `hds-clip`, `hds-diarize` |
| F8 | M5 | средний | guard длительности/RF64 с понятной ошибкой | `hds-diarize/src/wav.rs`, `hds-llama/src/host.rs` |
| F9 | M6 | средний | `diarization_required` доходит до хоста (или удаляется) | `hds-index/src/transcribe.rs`, `hds-llama/src/host.rs` |
| F10 | L1 | низкий | `mode` в ответе соответствует факту | `hds-llama/src/host.rs` |
| F11 | L2 | низкий | убрать дубли backend/поиска ffmpeg, мёртвый код | `hds-diarize`, `hds-llama`, `hds-index`, новый `hds-tools` |
| F12 | L3 | низкий | валидация `onset/offset` | `hds-llama/src/diar.rs`, `hds-diarize/src/sortformer.rs` |
| F13 | L4 | низкий | live-тест без dev-путей | `hds-diarize/tests/nemotron_live.rs` |
| F14 | L5 | низкий | синхронизировать комментарии про каталоги моделей | `config.example.yaml`, `hds/config.py` |
| F15 | L6 | низкий | уникальный staging-каталог whisper (pre-existing) | `hds-llama/src/whisper.rs` |
| F16 | L7 | низкий | потоковая запись/чтение WAV | `hds-diarize/src/wav.rs` |

## 3. Этап 0. Базовая линия (0.25 дня)

1. Зафиксировать текущее состояние: `git rev-parse HEAD`, `git status` (должно быть чисто).
2. Прогнать и сохранить в `tools/parity/out/fixes/baseline.txt`:
   ```powershell
   Remove-Item Env:\HDS_ROOT -ErrorAction SilentlyContinue
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
3. Снять метрики до изменений: `hds check`, `hds whisper-check --json`,
   `hds transcribe-watch --dry-run`, время старта `llm_host` и время `hds check`
   (на dev-машине, где модель Nemotron-3 установлена).
4. Для F1 отдельно зафиксировать падение: добавить тест из приложения A ревью и убедиться,
   что он падает (это доказательство дефекта; тест остаётся в кодовой базе после фикса).

**Выход этапа:** baseline-лог и воспроизведённая паника; ничего не изменено.
## 4. Этап 1. Блокеры (обязательно до тега)

### F1. Убрать панику в `split_text` (ревью B1)

**Файлы:** `crates/hds-diarize/src/merge.rs` (функция `split_text`, строки 82-110; тесты
модуля).

**Шаги:**

1. Заменить блок распределения слов (строки 99-104) на безопасный вариант:
   ```rust
   } else {
       let want = ((d / total) * n as f64).round() as usize;
       let room = n - used;              // инвариант: used <= n
       let want = if room == 0 { 0 } else { want.max(1).min(room) };
       counts.push(want);
       used += want;
   }
   ```
   `n - used` — не `saturating_sub`: инвариант должен оставаться явным; при `room == 0`
   часть получает 0 слов и её блок будет отфильтрован (`if text.is_empty() { continue }`),
   слова не теряются.
2. Проверить, что для `durations.len() > words.len()` сумма `counts` равна `n`
   (последняя часть получает `n - used`; при безопасном варианте `used <= n` всегда).
3. Добавить в `#[cfg(test)] mod tests` merge.rs:
   * `few_words_many_speakers_does_not_panic` — тест из приложения A ревью
     (2 слова, 4 части);
   * `one_word_three_parts_keeps_word_once` — 1 слово, 3 части (`split_text` напрямую:
     сумма слов = 1, ровно одна непустая строка);
   * `split_text_preserves_words` — цикл `parts in 2..=6`, `words in 1..=6`,
     `durations = vec![1.0; parts]`: нет паники, сумма слов = `words`;
   * `split_text_zero_total_duration` — `durations` из нулей не теряет текст.
4. Обновить комментарий функции: «часть может получить 0 слов, если слов меньше, чем частей;
   такие блоки отбрасываются».

**Тесты/проверки:**
`cargo test -p hds-diarize` (все + новые), `cargo clippy -p hds-diarize --all-targets -- -D warnings`.

**Приёмка:** тест-репро зелёный; сумма слов на всех сценариях сохраняется; в живом прогоне
(18-мин файл) `.md` побитово не изменился (сравнить с `tools/parity/out/diar_nemotron/w4_out/*.md`).

**Риск/откат:** поведение при `words < parts` меняется только тем, что часть с 0 слов
исчезает из `.md` (раньше — паника). Откат — один хунок.
### F2. Не навязывать backend из клиента; согласовать диагностику (ревью B2)

**Требует подтверждения владельца:** меняется поведение клиента (`hds transcribe-watch`).
Рекомендуемый вариант — «нет ключа ⇒ поле не отправляется», он восстанавливает решение №3
плана и сохраняет решение №13 (продуктовый дефолт nemotron3 на стороне хоста).

**Файлы:** `crates/hds-index/src/transcribe.rs`, `crates/hds-index/src/autotranscribe.rs`
(при необходимости), `crates/hds-index/src/diag.rs`,
`crates/hds-cli/src/cmd/{transcribe,whisper_check,check}.rs`, `README.md`,
`RELEASE_NOTES_v0.21.0.md`, `config.example.yaml`, `server_config.example.yaml`,
`hds/config.py`.

**Шаги:**

1. В `DiarizationSettings` добавить признак «ключ был в конфиге»:
   ```rust
   /// `auto_transcribe.diarization_backend` был задан явно (а не взят из дефолта).
   pub backend_explicit: bool,
   ```
   `Default` → `backend_explicit: false`; в `from_auto_transcribe`:
   ```rust
   let backend_raw = dig_str(cfg, "auto_transcribe.diarization_backend");
   diarization: DiarizationSettings {
       backend_explicit: backend_raw.is_some(),
       backend: backend_raw.unwrap_or_else(|| DEFAULT_DIARIZATION_BACKEND.to_string()),
       ...
   }
   ```
2. В `TranscribeClient::transcribe_to_file` (строки 481-489) отправлять backend только при
   явном ключе:
   ```rust
   if d.enabled {
       body["diarization"] = json!(true);
       if d.backend_explicit {
           body["diarization_backend"] = json!(d.backend);
       }
       body["diarization_feed_ms"] = json!(d.feed_ms);
       if let Some(m) = &d.model { body["diarization_model"] = json!(m.to_string_lossy()); }
   }
   ```
   `transcribe()` (без диаризации) не меняется.
3. Диагностика фактического запроса:
   * в `transcribe.rs` добавить `TranscribeConfig::requested_backend(&self) -> Option<&str>`
     (возвращает `Some`, только если `backend_explicit`);
   * в `diag.rs` добавить `pub fn requested_backend_check(cfg: &Config, requested:
     Option<&str>) -> Option<Check>`: `None` → `None`; явный `nemotron3` без валидной модели
     → `fail` с `fetch_nemotron3_model.ps1`; явный `sortformer` без `.gguf` → `warn`;
     `off` → `ok`; неизвестное значение → `warn`;
   * `hds transcribe-watch --dry-run` (`cmd/transcribe.rs`) печатает **две** строки:
     хостовую (`check_diarization`) и фактическую (`requested_backend_check` или
     «backend не отправляется — хост выберет по конфигу/авто»); `fail` любой из них → код 1;
   * `hds check`/`hds whisper-check` добавляют строку «конвейер автотранскрибации
     запросит: …»; в `whisper-check --json` — аддитивный ключ
     `transcribe_requested_backend` (строка или `null`).
4. Обновить документацию под фактическое поведение:
   * `README.md:616-617` — «остаться на sortformer: `auto_transcribe.diarization_backend:
     sortformer`; ключ `diarization.backend` задаёт выбор хоста, но явный клиентский запрос
     его перекрывает»;
   * `RELEASE_NOTES_v0.21.0.md:80-82` — та же формулировка;
   * `config.example.yaml:367-368`, `server_config.example.yaml:54-56`, `hds/config.py:136-140` —
     комментарии: пусто/нет ключа ⇒ запрос не отправляется, решает хост (решение №3).
5. Обновить/добавить тесты:
   * `hds-index/src/transcribe.rs`: `auto_transcribe_defaults_to_nemotron3` дополнить
     `assert!(!tc.diarization.backend_explicit)` и проверкой `requested_backend() == None`;
     новый `explicit_sortformer_is_requested` — `backend_explicit == true`,
     `requested_backend() == Some("sortformer")`;
   * `hds-index/tests/transcribe_media.rs`: расширить `stub_server_capture` —
     `transcribe_to_file_sends_backend_only_when_explicit` (тело без ключа при дефолте;
     с ключом — `"diarization_backend": "sortformer"`);
   * `hds-index/src/diag.rs`: `requested_nemotron3_without_model_fails`,
     `requested_none_returns_none`, `requested_sortformer_without_model_warns`;
   * `hds-cli/tests/config_init.rs` — шаблоны остаются с явным nemotron3 (не ломаются).
6. Проверить на песочнице: конфиг без `auto_transcribe.diarization_backend` и с
   `diarization.backend: sortformer` → хост логирует `[diar] backend=sortformer`, в теле нет
   ключа; без секции `diarization` и без модели → хост выбирает sortformer с предупреждением
   (решение №3).

**Приёмка:** тело запроса соответствует ключам конфига; `--dry-run`/`hds check` не показывают
`ok`, если фактический запрос гарантированно упадёт; legacy-путь sortformer запускается через
`auto_transcribe.diarization_backend: sortformer` и (после F2) через
`diarization.backend: sortformer` при отсутствии клиентского ключа.

**Риск/откат:** если владелец выберет «всегда nemotron3», пункты 1-2 отменяются, остаются
3-5 (диагностика и документация); откат — вернуть безусловную отправку и флаг.
### F3. Докачка модели при обновлении + честные релизные заметки (ревью B3)

**Файлы:** `installers/update.ps1`, `installers/update.command`, `update.cmd` (проброс
аргументов уже есть), `RELEASE_NOTES_v0.21.0.md`, `README.md` (раздел «Обновление»),
`crates/hds-cli/tests/install_server_parity.rs`.

**Шаги:**

1. `installers/update.ps1`: новый параметр `[switch]$NoModels`; после `Switch-Current`
   (когда новый `installers/` уже активен) и при `-not $WhatIf`:
   ```powershell
   if (-not $NoModels) {
       $fetch = Join-Path $cur "installers\fetch_nemotron3_model.ps1"
       if (Test-Path $fetch) {
           $modelsDir = if ($env:HDS_MODELS_DIR) { $env:HDS_MODELS_DIR }
                        elseif ($Profile -eq "server") { Join-Path $env:ProgramData "hermes-disk-search\models" }
                        else { Join-Path $env:APPDATA "OpenResearchTools\models" }
           try {
               & powershell -NoProfile -ExecutionPolicy Bypass -File $fetch -ModelsDir $modelsDir
               if ($LASTEXITCODE -ne 0) { Write-Host "[!!] Nemotron-3 model not fetched (exit $LASTEXITCODE)" -ForegroundColor Yellow }
               else { Write-Host "[ok] Nemotron-3 diarization model ready" }
           } catch { Write-Host "[!!] Nemotron-3 model fetch failed: $_" -ForegroundColor Yellow }
       } else { Write-Host "[..] fetch_nemotron3_model.ps1 not found in $cur - skipped" }
   }
   ```
   Ошибка загрузки **не** должна валить обновление (модель можно докачать вручную); итог
   печатается в сводке.
2. `installers/update.command` (macOS): после переключения версии — если есть `pwsh`,
   вызвать тот же скрипт с `-ModelsDir "$MODELS_DIR"` (по образцу
   `install_server.command:326-345`); иначе напечатать ручную команду.
3. `RELEASE_NOTES_v0.21.0.md:74` — заменить на фактическое: «`update.cmd`/
   `installers/update.command` докачивают модель Nemotron-3 (идемпотентно; `-NoModels` —
   пропустить), `install-server.cmd -Fetch` — тоже; вручную — `fetch_nemotron3_model.ps1`».
4. `README.md` («Обновление и удаление») — добавить пункт: обновление докачивает модель;
   при `-NoModels`/ошибке сети автотранскрибация не запустится до ручной докачки (`hds check`
   покажет fail и команду).
5. Тесты `install_server_parity.rs`:
   * `update_ps1_fetches_nemotron3_model` — в `installers/update.ps1` есть
     `fetch_nemotron3_model.ps1`, `-ModelsDir`, `-NoModels` и вызов не фатален
     (`try/catch`/`$LASTEXITCODE` не `throw`);
   * `update_command_fetches_nemotron3_model` — в `installers/update.command` есть
     `fetch_nemotron3_model.ps1` (или явная ручная команда);
   * сохранить существующие проверки `setup.ps1`/`install_server.ps1`.
6. Проверка на dev-машине: `installers/update.ps1 -Root <копия установки> -WhatIf` не качает;
   реальный прогон на песочнице (или `-NoModels`) завершает обновление; затем ручной
   `fetch_nemotron3_model.ps1 -ModelsDir <tmp>` пропускает уже скачанный файл (exit 0).

**Приёмка:** обновление с чистой машины (без модели) оставляет `hds check` зелёным по
диаризации; тест-паритет зелёный; релизные заметки и README совпадают с кодом.

**Риск/откат:** докачка идёт после переключения версии; при сбое сети обновление всё равно
успешно, модель — вручную. Откат — убрать блок и параметр `-NoModels`.

**Выход этапа 1:** F1–F3 закрыты, тесты/линтеры зелёные, на песочнице проверены:
репро-паника исчезла, backend выбирается по правилу №3, обновление без модели докачивает её.
## 5. Этап 2. Средние замечания (желательно в тот же релиз)

### F4. Предупреждение демона учитывает backend (ревью M1)

**Файлы:** `crates/hds-index/src/autotranscribe.rs` (`warnings`, строки 417-424; вызов —
строка 719).

**Шаги:**

1. Сделать `warnings` зависимым от backend:
   ```rust
   pub fn warnings(&self, cfg: &Config) -> Vec<String> {
       let mut w = Vec::new();
       let d = &self.transcribe.diarization;
       if d.enabled {
           match d.backend.trim().to_ascii_lowercase().as_str() {
               "sortformer" => {
                   if d.model.is_none() {
                       w.push("diarization_backend=sortformer, но diarization_model не задан: \
                               без .gguf задание упадёт (fetch_diarization_model.ps1)".into());
                   }
               }
               "nemotron3" => {
                   if nemotron_model_for(cfg).is_none() {
                       w.push("diarization_backend=nemotron3, модель не найдена: \
                               fetch_nemotron3_model.ps1".into());
                   }
                   if hds_core::tools::find_ffmpeg(cfg).is_none() {
                       w.push("diarization_backend=nemotron3, ffmpeg не найден: \
                               декодировать WAV нечем".into());
                   }
               }
               "off" => {}
               other => w.push(format!("diarization_backend '{other}' неизвестен")),
           }
       }
       ...
   }
   ```
2. Обновить вызов `cfg.warnings()` → `cfg.warnings(&cfg)` в `run_transcribe_watch`
   (строка 719); поправить тесты, вызывающие `warnings()`.
3. Тесты: `warnings_are_backend_aware` (nemotron3 + фейковая модель/ffmpeg → без
   sortformer-текста; sortformer без модели → предупреждение; off → пусто). В тесте
   использовать `HDS_MODELS_DIR` на временный каталог с фиктивным
   `nemotron3/nemotron3_diar_v3.onnx` (проверяется только наличие, файл не хешируется).

**Приёмка:** старт `hds transcribe-watch` с дефолтным `nemotron3` не печатает предупреждение
про sortformer; при отсутствии модели/ffmpeg — печатает правильную команду.

### F5. Не хешировать модель, когда backend не nemotron3; хост — лениво (ревью M2)

**Файлы:** `crates/hds-index/src/diag.rs` (`DiarSummary`, `diarization_summary`,
`diarization_check`), `crates/hds-cli/src/cmd/whisper_check.rs`,
`crates/hds-llama/src/host.rs` (`Host::start`, `resolved_diar_backend`).

**Шаги:**

1. В `DiarSummary` заменить `nemotron: Option<Result<(), String>>` на явное состояние:
   ```rust
   pub enum NemotronCheck {
       Missing,                 // файла нет
       NotChecked,              // backend не nemotron3 — не проверяли
       Ok,
       Failed(String),
   }
   ```
   `nemotron_ok()` → `matches!(self.nemotron, NemotronCheck::Ok)`.
   Обновить `nemotron3_check`, `sortformer_legacy_check`, `whisper_check::print_diarization`
   и JSON (`nemotron_ok`, новый аддитивный `nemotron_checked`).
2. В `diarization_summary` перенести `verify_model` **после** вычисления `backend`:
   ```rust
   let backend = parsed.unwrap_or(auto); // auto сам требует проверки модели
   let nemotron = match (nemotron_model.as_ref(), backend) {
       (None, _) => NemotronCheck::Missing,
       (Some(p), DiarBackend::Nemotron3) => match hds_diarize::verify_model(p) { ... },
       (Some(_), _) => NemotronCheck::NotChecked,
   };
   ```
   Для `auto` проверка остаётся (она и определяет выбор) — это документируется.
3. `host.rs`: убрать `backend.resolved_diar_backend();` из `Host::start`; решение
   по-прежнему кэшируется `OnceLock` и принимается в `pick_diar_backend` при первом запросе.
   Лог `[diar] backend=...` перенести в `resolved_diar_backend`; на старте — дешёвая строка
   «решение о backend будет принято при первом задании» (+ `is_file` модели, без хеша).
4. Тесты:
   * `diag`: `off_and_sortformer_do_not_hash_model` — временный файл размером не 400 МБ,
     `backend: off` → `NemotronCheck::NotChecked`;
     `nemotron3_with_wrong_size_reports_failed`;
   * `whisper_check` JSON-тест — `nemotron_checked: false` для `off`;
   * `host`: функциональный тест `pick_diar_backend` с `DiarSettings { backend:
     Some(Sortformer) }` и несуществующей моделью — не падает и не читает файл.
5. Замер: `Measure-Command { hds check }` до/после на конфиге с `backend: off` (ожидание —
   без чтения 400 МБ).

**Приёмка:** `hds check` с `off`/`sortformer` не хеширует модель; старт `llm_host` без
автотранскрибации не читает 400 МБ; для `nemotron3`/auto проверка и диагностика сохранены.
### F6. Кэшировать отрицательный результат проверки модели (ревью M3)

**Файлы:** `crates/hds-diarize/src/sortformer.rs` (`Verified`, `verify_model`,
`sha256_file`), тесты модуля.

**Шаги:**

1. Заменить тип кэша:
   ```rust
   type Verified = Mutex<HashMap<(PathBuf, u64, u64), std::result::Result<(), String>>>;
   ```
2. Вынести проверку в `fn verify_against(path, expected_size, expected_sha) -> Result<()>`
   (публичная `verify_model` вызывает её с `NEMOTRON_SIZE`/`NEMOTRON_SHA256`); в
   `verify_against`:
   * ключ `(path, meta.len(), mtime)`;
   * попадание → `Ok(())` или `Err(DiarizeError::Format(cached))`;
   * после хеширования записать в кэш и успех, и ошибку.
3. Тесты:
   * `negative_result_is_cached`: временный файл 3 байта, `verify_against(path, 3,
     "00…00")` → `Err` (sha); удалить файл; повторный вызов → **та же** ошибка sha (не Io);
     перезаписать файл (другая mtime) → `Err(Io)` (ключ изменился);
   * существующие `verify_model_rejects_wrong_size` и `sha256_of_known_bytes` не меняются.
4. Опционально вместе с F5: в `run_nemotron` логировать только первое падение загрузки
   модели, чтобы не спамить одинаковой ошибкой каждое задание.

**Приёмка:** при битой модели второе задание не тратит время на хеш; ошибка та же и понятная.

### F7. Одно ORT-окружение на процесс (ревью M4)

**Файлы:** новый `crates/hds-ort/{Cargo.toml,src/lib.rs}`; `Cargo.toml` (workspace
members); `crates/hds-clip/src/{env,lib}.rs`; `crates/hds-diarize/src/{ort_env,sortformer}.rs`;
новый тест `crates/hds-cli/tests/ort_env_singleton.rs`.

**Шаги:**

1. Новый крейт:
   ```toml
   [package]
   name = "hds-ort"
   version.workspace = true
   edition.workspace = true
   rust-version.workspace = true

   [dependencies]
   ort = "=2.0.0-rc.15"
   ```
   ```rust
   use std::sync::OnceLock;
   use ort::environment::Environment;

   static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();

   pub fn environment() -> std::result::Result<&'static Environment, String> {
       match ENV.get_or_init(|| ort::init().with_name("hds").build().map_err(|e| format!("ONNX Runtime environment: {e}"))) {
           Ok(env) => Ok(env),
           Err(e) => Err(e.clone()),
       }
   }

   pub fn clone_environment() -> std::result::Result<Environment, String> {
       environment().map(|e| e.clone())
   }
   ```
2. `hds-clip/src/env.rs` и `hds-diarize/src/ort_env.rs` становятся тонкими обёртками над
   `hds_ort` (сохранить публичные имена `environment()`, `ort_env()`,
   `ort_env_clone()` — внешний код не меняется). Убрать локальные `static ENV`.
3. `parakeet-rs` уже требует `ort = "=2.0.0-rc.15"` — тип `Environment` тот же; проверить
   `cargo tree -i ort` (одна версия).
4. Интеграционный тест:
   ```rust
   #[test]
   fn one_environment_per_process() {
       let clip = hds_clip::env::environment();
       let diar = hds_diarize::ort_env();
       assert!(clip.is_ok() && diar.is_ok(), "оба API должны работать в одном процессе");
   }
   ```
   (если типы сравнимы — дополнительно `std::ptr::eq`; иначе достаточно отсутствия
   `only one environment is allowed per process`).
5. Обновить doc-комментарии: «единственное окружение процесса — `hds_ort`; не создавать
   второе».

**Приёмка:** `hds` может создать и CLIP-сессию, и `Diarizer` в одном процессе без ошибки
ORT; `cargo tree -i ort` — одна версия; тест зелёный; паритет CLIP (опционально
`cargo test -p hds-clip --test clip_parity -- --ignored`) не изменился.

**Риск:** feature-унификация `directml` остаётся общей (как и сейчас); `hds-ort` не должен
включать лишние фичи.
### F8. Guard длительности и RF64 с понятной ошибкой (ревью M5)

**Файлы:** `crates/hds-diarize/src/wav.rs`, `crates/hds-llama/src/host.rs`
(`transcribe_nemotron3`), README (ограничения).

**Шаги:**

1. `wav.rs`:
   * `pub const MAX_RIFF_SAMPLES: usize = (u32::MAX as usize - 36) / 2;`
   * `pub fn encode_wav_checked(samples: &[i16], rate: u32) -> Result<Vec<u8>>` — при
     `samples.len() > MAX_RIFF_SAMPLES` вернуть `DiarizeError::Format("WAV > 4 ГиБ: RIFF
     не поддерживается; запись слишком длинная (≈6.7 ч при 16 кГц mono)")`;
   * `write_wav` использует `encode_wav_checked` (сигнатура и тесты сохраняются, ошибка —
     вместо битого файла);
   * `parse_wav`: если `buf[0..4] == b"RF64"` — понятная ошибка
     `Format("WAV/RF64 не поддерживается (запись > 4 ГиБ)")`.
2. `transcribe_nemotron3`: после `read_wav(&full)` и до ASR проверить длительность:
   ```rust
   const MAX_DIAR_SEC: f64 = 6.0 * 3600.0;
   if wav.duration_sec() > MAX_DIAR_SEC {
       return Err(EngineError::Other(format!(
           "запись {:.1} ч превышает предел диаризации {MAX_DIAR_SEC/3600.0:.1} ч (RIFF/u32)",
           wav.duration_sec() / 3600.0
       )));
   }
   ```
   Проверка дешёвая (длина сэмплов), выполняется до ASR — не тратим часы.
3. Тесты:
   * `encode_wav_checked_rejects_overflow` — проверить арифметику через маленький лимит
     (`#[cfg(test)] fn encode_wav_with_limit(samples, rate, max)`); публичная функция
     вызывает её с `MAX_RIFF_SAMPLES`;
   * `parse_wav_rejects_rf64_with_hint` — собрать 44-байтный заголовок с `RF64`;
   * существующий `tolerates_data_size_beyond_eof` не меняется.
4. README (раздел про диаризацию) — одна строка: «запись длиннее ≈6.7 ч не поддерживается
   (RIFF/u32); при необходимости — отдельная задача RF64».

**Приёмка:** длинная запись даёт понятную ошибку до обработки; `< 6 ч` работает как раньше;
`write_wav` не создаёт битый файл.

### F9. `diarization_required` доходит до хоста (ревью M6)

**Рекомендуемый вариант A:** аддитивный ключ тела `diarization_required`
(`true|false`) — тело сильнее конфига, как и `diarization_backend`.

**Файлы:** `crates/hds-index/src/transcribe.rs` (`transcribe_to_file`, строки 481-489),
`crates/hds-llama/src/host.rs` (`internal_transcribe`, `transcribe_nemotron3`),
`crates/hds-cli/src/cmd/transcribe.rs` (dry-run), документация конфига.

**Шаги:**

1. Клиент: `if d.backend_explicit || ...` — проще отправлять `diarization_required` всегда
   при `d.enabled`:
   ```rust
   body["diarization_required"] = json!(d.required);
   ```
2. Хост: в `internal_transcribe` вычислить
   ```rust
   let diar_required = body
       .get("diarization_required")
       .and_then(|v| v.as_bool())
       .unwrap_or(self.diar.required);
   ```
   и передать в `transcribe_nemotron3` (параметр или поле запроса) вместо чтения
   `self.diar.required` на строке 2712. Для legacy sortformer оставить жёсткий отказ движка
   (деградации там нет) и добавить в лог пояснение, если пришло `false`.
3. Тесты:
   * `hds-index`: `transcribe_to_file_sends_diarization_required` (тело содержит
     `false` при `required: false`);
   * `hds-llama`: unit на хелпер `pick_required(body, configured)` — тело `false` сильнее
     конфига `true`; отсутствие ключа → конфиг;
   * `hds transcribe-watch --dry-run` — печатает, что именно уйдёт (`required` источника).
4. Документация: `config.example.yaml`/`README` — «`auto_transcribe.diarization_required`
   уходит в теле и перекрывает `diarization.required` хоста (для `nemotron3`; legacy
   sortformer при сбое всё равно падает)».

**Приёмка:** `diarization_required: false` в клиентском конфиге даёт `.md` без меток вместо
отказа (nemotron3); при `true` поведение прежнее; legacy-путь не изменился.

**Альтернатива B (если владелец против расширения тела):** удалить
`auto_transcribe.diarization_required` из конфига/документации/dry-run и оставить только
`diarization.required`; в релизных заметках пояснить.

**Выход этапа 2:** F4–F9 закрыты; `hds check`/`whisper-check`/dry-run согласованы с
фактическим запросом; модель не хешируется без необходимости; ORT-синглтон; длинные записи
и `diarization_required` обработаны предсказуемо.
## 6. Этап 3. Низкие замечания и техдолг (отдельная задача, не блокирует релиз)

### F10. `mode` в ответе соответствует факту (ревью L1)

**Файлы:** `crates/hds-llama/src/host.rs` (`transcribe_nemotron3`), README.

**Шаги:**

1. В nemotron-ветке возвращать фактический режим:
   ```rust
   let mode_out = "transcript"; // диаризация всегда даёт .md с метками
   ...
   "mode": mode_out,
   ```
   Если запрошенный `mode` отличался (например, `speech`), один раз записать в лог
   `[diar] mode 'speech' → 'transcript' (диаризация всегда .md)`.
2. Обновить `docs` (`PLAN_DIARIZATION_NEMOTRON.md` не переписывать; README/`RELEASE_NOTES`):
   «при `diarization_backend: nemotron3` результат — всегда `.md` со спикерами; в ответе
   `mode: "transcript"`».
3. Тест: helper `fn effective_mode(requested: &str) -> &'static str` (возвращает
   `"transcript"` для nemotron3) + unit-тест; существующие потребители `mode` не меняются.

**Приёмка:** `segments`, `out_ext` и `mode` согласованы; клиент не путается.

### F11. Убрать дубли backend/ffmpeg и мёртвый код (ревью L2)

**Файлы:** `crates/hds-diarize/src/lib.rs` (+ новый `backend.rs`), `crates/hds-llama/src/
diar.rs`, `crates/hds-index/src/diag.rs`, новый `crates/hds-tools/**`, `Cargo.toml`.

**Шаги (можно частями):**

1. Backend-правило — один источник в `hds-diarize::backend`:
   ```rust
   pub enum Backend { Nemotron3, Sortformer, Off }
   impl Backend { pub fn parse(s: &str) -> Option<Self>; pub fn as_str(&self) -> &'static str; }
   pub fn resolve(requested: Option<Backend>, configured: Option<Backend>, nemotron_ok: bool) -> Backend;
   ```
   `hds-llama::diar::Backend` и `hds-index::diag::DiarBackend` становятся `pub use`
   (имена сохранить для совместимости). Тесты перенести/оставить, логика — в одном месте.
2. `hds_llama::diar::pick_backend` — либо использовать в
   `ClusterBackend::pick_diar_backend`, либо удалить (тест `body_backend_overrides_config`
   оставить на общей функции).
3. Поиск ffmpeg — новый `hds-tools` (без внешних зависимостей): `which`, `standard_paths`,
   `find_ffmpeg(cfg_path: Option<&Path>) -> Option<PathBuf>`. `hds-core::tools` реэкспортирует
   (`pub use hds_tools::{...}`) — Python-паритет не меняется; `hds-llama::diar` и
   `diar_wav_probe` используют общий крейт. Тесты поиска — один набор.
4. `render_srt`: оставить публичным (формат зафиксирован планом), добавить doc-комментарий
   «не используется продуктом; задел для `out_format: srt`» — либо удалить вместе с тестом,
   если владелец считает API лишним. Решение зафиксировать в этом плане.

**Приёмка:** `rg 'DiarBackend|fn which|fn find_ffmpeg'` показывает один источник; тесты
зелёные; публичные имена сохранены.

### F12. Валидация `onset/offset` (ревью L3)

**Файлы:** `crates/hds-llama/src/diar.rs` (строки 209-214),
`crates/hds-diarize/src/sortformer.rs` (`NemotronConfig::validate`).

**Шаги:**

1. `from_yaml`: если значение вне `0.0..=1.0` — предупреждение
   `"diarization.nemotron3.onset {v} вне 0..=1 — оставляю {default}"` и дефолт.
2. `NemotronConfig::validate`: ошибка `Format("onset/offset должен быть 0..=1")`.
3. Тесты: расширить `config_validation` (onset=1.5 → Err) и
   `invalid_values_are_warned_not_fatal` (2 предупреждения → 3).

**Приёмка:** некорректные пороги не доходят до модели; диагностика их показывает.

### F13. Live-тест без dev-путей (ревью L4)

**Файлы:** `crates/hds-diarize/tests/nemotron_live.rs`.

**Шаги:**

1. Убрать `D:\...` дефолты; читать только `HDS_DIAR_MODEL` и `HDS_TEST_DIAR_WAV`.
2. Если переменные **не заданы** — `eprintln!("[skip] ...")` и `return`; если заданы, но
   файла нет — `panic!` с путём (явная ошибка конфигурации теста).
3. Обновить шапку теста (команда запуска с переменными).

**Приёмка:** на чистой машине тест честно пропускается; с заданными путями — проверяет
`frag60` (2 спикера, 5 сегментов).
### F14. Синхронизировать комментарии про каталоги моделей (ревью L5)

**Файлы:** `config.example.yaml:367`, `hds/config.py:137` (и аналогичные места в
`server_config.example.yaml`, если есть).

**Шаги:** заменить «`''` → автопоиск `%APPDATA%\OpenResearchTools\models\*sortformer*`» на
«`''` → автопоиск: `HDS_MODELS_DIR` → `%ProgramData%\hermes-disk-search\models` →
`%APPDATA%\OpenResearchTools\models` (каталог `*sortformer*`)». Для nemotron3 уже написано
корректно — сверить формулировки.

**Приёмка:** комментарии совпадают с `hds_diarize::models_roots`; `rg 'OpenResearchTools'`
не показывает устаревших обещаний.

### F15. Уникальный staging-каталог whisper (ревью L6, pre-existing)

**Файлы:** `crates/hds-llama/src/whisper.rs` (`stage_ascii`, `transcribe_file`), тест там же.

**Шаги:**

1. В `transcribe_file` вместо общего `temp_dir()/hds_whisper` создавать уникальный каталог на
   запрос через уже существующий `crate::diar::TempDir::new("hds_whisper")`; `Drop` удалит его
   после чтения текста/сегментов.
2. `build_metadata` получает этот же каталог как `out_dir` — менять не нужно.
3. Тест: два последовательных `TempDir` дают разные пути и удаляются (уже есть
   `temp_dir_is_unique_and_removed` в `diar.rs`; добавить проверку, что `stage_ascii` пишет
   в переданный каталог и не переиспользует чужой).
4. Живая проверка (по возможности): два файла параллельно на двух картах — выходы не
   перепутаны (на dev-машине с одной картой — последовательно, но с разными каталогами).

**Приёмка:** параллельные задания не перезаписывают `input.*`/выход; после задания каталог
удалён.

**Примечание:** дефект не внесён этой доработкой; исправление можно вынести в отдельную
задачу, если не хотим расширять релиз.

### F16. Потоковая запись/чтение WAV (ревью L7, по желанию)

**Файлы:** `crates/hds-diarize/src/wav.rs`.

**Шаги:**

1. `write_wav`: писать заголовок и данные через `BufWriter`, кусками по 64 КиБ (не собирать
   весь файл в `Vec<u8>`); `encode_wav` оставить для тестов/проб.
2. `read_wav`: читать заголовок, затем `data` сразу в `Vec<i16>` (chunked `read_exact`),
   без промежуточного `fs::read` всего файла; общий разбор чанков вынести в функцию,
   возвращающую `(fmt, data_offset, data_len)`.
3. Замер: RSS на 77-мин прогоне (как W7) — ожидание −150…−300 МБ пика; метрики записать в
   `tools/parity/out/fixes/report.md`.
4. Тесты: существующие roundtrip/edge-кейсы остаются; добавить `write_then_read_large`
   на 10 мин синтетики (быстрый, ~19 МБ).

**Приёмка:** результаты `.md`/turns не меняются; пик RSS ниже; тесты зелёные.

**Примечание:** не блокирует, выполнять после тега, если W7-числа устраивают владельца.

## 7. Общая тестовая матрица

### 7.1. Обязательные команды после каждого этапа

```powershell
Remove-Item Env:\HDS_ROOT -ErrorAction SilentlyContinue   # см. W3/W7: иначе 2 теста host_resident падают
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### 7.2. Точечные тесты по задачам

| Задача | Команда/тест |
|---|---|
| F1 | `cargo test -p hds-diarize merge::` (новые тесты `split_text_*`, репро) |
| F2 | `cargo test -p hds-index transcribe::` и `--test transcribe_media`; `hds transcribe-watch --dry-run` |
| F3 | `cargo test -p hds-cli --test install_server_parity`; `update.ps1 -WhatIf` |
| F4/F5 | `cargo test -p hds-index diag:: autotranscribe::`; `hds check`/`whisper-check --json` |

## 11. Среда и предпосылки (проверено 21.10.2026)

Факты, на которые можно опираться в новом чате (перед работами перепроверить командами из §7):

| Что | Значение |
|---|---|
| Репозиторий | `c:\Users\Sasha\hermes-disk-search`, ветка `server-multiuser`, HEAD `abda621` |
| Теги | `v0.21.0` отсутствует (последние в списке — до `v0.9.0`); правки можно включить в 0.21.0 |
| Rust | `rustc 1.97.1`, MSRV воркспейса `1.92` (из-за `ort 2.0.0-rc.15`/`parakeet-rs 0.4.2`) |
| ОС | Windows 11; macOS-машины нет (CoreML проверяется только сборкой/гардами) |
| Модель Nemotron-3 | `D:\diar-spike-models\nemotron3\nemotron3_diar_v3.onnx` (400 506 656 байт), `sha256` валиден. В `%APPDATA%\OpenResearchTools\models` и `%ProgramData%\hermes-disk-search\models` файла **нет** |
| Как включить модель | `HDS_MODELS_DIR=D:\diar-spike-models` (канонический `<root>\nemotron3\...`) — этого достаточно для `llm_host` и `hds check` |
| Тестовые WAV | `D:\diar-spike-data\frag60.wav`, `meet0512.wav` (56 мин), `meet0514.wav` (77 мин), `meet0924.wav`/`meet0924_mp3.wav` (18 мин) |
| ffmpeg | `C:\Users\Sasha\AppData\Local\Microsoft\WinGet\Packages\Gyan.FFmpeg_...\bin\ffmpeg.exe` (также виден в `PATH`) |
| Release-бинари | `target\release\*.exe` устарели (14.10.2026), `DirectML.dll` на месте; перед сборкой пакета остановить `hds`/`llm_host` |
| Запущенные процессы | на момент ревью: 4 × `hds.exe` и `llm_host.exe`; PID меняется — искать через `Get-Process hds,llm_host` |
| Песочницы | `tools/parity/out/diar_nemotron/{w4_host.yaml,w5_check.yaml,w5_check_missing.yaml,w7_host.yaml}` (гитигнорится) |
| Артефакты правок | складывать в `tools/parity/out/fixes/` (каталог гитигнорится, в репозиторий не попадает) |

**Важные грабли окружения:**

* `HDS_ROOT` в среде ассистента указывает на репозиторий и валит 2 теста `hds-llama`
  (`host_resident`). Перед `cargo test` — `Remove-Item Env:\HDS_ROOT -ErrorAction SilentlyContinue`
  (зафиксировано в W3/W7).
* `cargo build --release` падает с «отказано в доступе», если `target\release\*.exe` заняты
  службой/задачей. По `cline-rules/dev-machine.md` процессы разрешено остановить и вернуть
  после сборки (`llm_host run`/задача Планировщика).
* Модель лежит **вне** стандартных каталогов: для живой проверки задавать
  `HDS_MODELS_DIR=D:\diar-spike-models` (или `diarization.nemotron3.model` явно), иначе
  получите «модель не найдена» и ложный вывод о регрессии.
* `D:\diar-spike-data` вне репозитория — пути к WAV не коммитить, только env/артефакты.

## 12. Definition of Done (общий критерий приёмки правок)

Работы считаются завершёнными, когда:

1. **F1–F9 закрыты** (или явно отложены владельцем с записью причины в этом плане); F10–F16 —
   закрыты либо отложены с обоснованием.
2. `Remove-Item Env:\HDS_ROOT; cargo fmt --all -- --check; cargo clippy --workspace
   --all-targets -- -D warnings; cargo test --workspace` — зелёные (0 падений).
3. Живая песочница (18-мин файл) подтверждает четыре сценария §7.3: nemotron3 / sortformer /
   off / без модели без секции `diarization` (решение №3).
4. `.md`, `UNASSIGNED`, `.orig.md`, `.speakers.json` — формат не изменился (diff с W4-артефактом
   на nemotron3-сценарии; отличия только там, где F1 меняет поведение при нехватке слов).
5. Документация согласована с кодом: `RELEASE_NOTES_v0.21.0.md`, `README.md`,
   `README_SERVER_MULTIUSER.md`, `config.example.yaml`, `server_config.example.yaml`,
   `hds/config.py`; `hds check`/`whisper-check`/`transcribe-watch --dry-run` показывают
   фактический backend.
6. Установщик модели докачивается при обновлении (`update.ps1`/`update.command`) и вручную;
   тест `install_server_parity` зелёный.
7. Рабочее дерево содержит только осмысленные изменения по задачам (никаких `*.onnx`/`*.gguf`,
   логов и временных файлов; артефакты — в `tools/parity/out/fixes/`).
8. Итоговый отчёт в новом чате: таблица F1–F16 (сделано/отложено + почему), команды и
   результаты прогонов, оставшиеся риски. Коммит/тег — **только по отдельной команде владельца**.

| F6 | `cargo test -p hds-diarize sortformer::` |
| F7 | новый `cargo test -p hds-cli --test ort_env_singleton`; `cargo tree -i ort` |
| F8 | `cargo test -p hds-diarize wav::` |
| F9 | `cargo test -p hds-index transcribe::`, `cargo test -p hds-llama diar::` |
| F10 | `cargo test -p hds-llama host::` (helper `effective_mode`) |
| F11 | `cargo test --workspace`, `cargo tree` (нет новых внешних зависимостей) |
| F12 | `cargo test -p hds-llama diar::`, `cargo test -p hds-diarize sortformer::` |
| F13 | `cargo test -p hds-diarize --test nemotron_live -- --ignored` с env |
| F15/F16 | `cargo test -p hds-llama whisper::`, `cargo test -p hds-diarize wav::`, RSS-замер |

### 7.3. Живые проверки (dev-машина, правила `dev-machine.md`)

1. Остановить занятые `target\release\*.exe` (резидент/задачи), собрать, вернуть резидент
   (`llm_host run`/задача Планировщика).
2. Песочница `tools/parity/out/diar_nemotron/w7_host.yaml` (порт-база 8110):
   * `diarization.backend: nemotron3` — 18-мин файл: `.md` с `SPEAKER_*`, 5 спикеров,
     `segments[]` в абсолютной шкале, `stats.timings_sec` присутствуют;
   * `diarization.backend: sortformer` + без `auto_transcribe.diarization_backend` — legacy
     `.md` (4 спикера + `UNASSIGNED`), формат не изменился (после F2);
   * `diarization.backend: off` — плоский текст, `diarization.enabled: false`;
   * без модели (временный `HDS_MODELS_DIR` без `nemotron3`) и без секции `diarization` —
     хост выбирает sortformer с предупреждением (решение №3), задание не падает.
3. `hds check`, `hds whisper-check --json`, `hds transcribe-watch --dry-run` — три источника
   показывают согласованный backend; при `off` модель не хешируется (время/лог).
4. Обновление: копия установки + `update.ps1 -Root <копия> -WhatIf`; затем реальный прогон
   с `-NoModels` и без — сводка и `hds check`.

### 7.4. Регрессия, которую нельзя допустить

* `.md` формат, `UNASSIGNED`, порядок реплик, `.orig.md`, `.speakers.json` — без изменений
  (проверить на 18-мин файле diff с W4-артефактом; изменений быть не должно, кроме F1-кейсов).
* Legacy sortformer: `mode: transcript`, `diarization_model`, `feed_ms`, ответ `out_ext`,
  `.md` — как до доработки.
* Python-паритет индексации (`pipeline_parity`, `walk_parity`) — не затронут (F11 только
  реэкспорт `find_ffmpeg`).
* `hds config init` — шаблоны остаются валидными (`config_init` тесты).
* Пакет: `DirectML.dll`, отсутствие весов, fetch-скрипты в архиве
  (`install_server_parity`).
## 8. Документация и релиз

1. **Версия.** Тег `v0.21.0` ещё не создан (W9: «тег ждёт решения по RSS»), поэтому F1–F9
   входят в `0.21.0`; если тег уже поставлен к моменту работ — выпуск `0.21.1` по
   `RELEASING.md`.
2. **Обновить:**
   * `RELEASE_NOTES_v0.21.0.md`: B3 (докачка при обновлении), B2 (что именно переключает
     sortformer), при необходимости F8 (предел длины), F9 (ключ тела);
   * `README.md`: глава «Диаризация» (fallback sortformer) и «Обновление» (модель качается);
   * `README_SERVER_MULTIUSER.md`: update/install-server — модель качается, `-NoModels`;
   * `config.example.yaml`, `server_config.example.yaml`, `hds/config.py`: комментарии
     backend/required/каталоги моделей (F2, F9, F14);
   * `STATUS.md`: строка о закрытии код-ревью (после приёмки).
3. **Не менять:** `PLAN_DIARIZATION_NEMOTRON.md` (решения владельца §7) — только ссылка на
   этот план и ревью в шапке (если владелец разрешит).
4. **Пакет:** пересобрать `installers/build_rust_release.ps1` (DirectML.dll, отсутствие
   весов) — проверить `install_server_parity`; для F7 убедиться, что новый крейт не добавил
   DLL/весов в архив.

## 9. Риски и открытые вопросы

| № | Вопрос | Рекомендация | Кто решает |
|---|---|---|---|
| Q1 | F2: отправлять ли `diarization_backend`, если ключ не задан? | Не отправлять — восстанавливает решение №3 и сохраняет №13 | владелец |
| Q2 | F9: расширять ли тело `diarization_required`? | Да, аддитивно (тело → конфиг), как у backend | владелец |
| Q3 | F11/F7: делать ли рефакторинг в этом релизе? | F7 — да (защита от будущего сбоя), F11 — частями после тега | владелец/агент |
| Q4 | F8: предел длины | 6 ч (запас до 6.7 ч RIFF) | владелец |
| Q5 | F16: потоковая память | после тега, если W7-числа устраивают | владелец |
| Q6 | F10: значение `mode` в ответе | `"transcript"` (факт) | владелец |

Общие риски:

* **F2 меняет поведение клиента** — единственная задача, где нужна явная санкция владельца;
  без неё можно сделать только диагностику/документацию (п.3-5 F2).
* **F7 трогает оба ORT-крейта** — обязательный прогон паритета CLIP и живой диаризации;
  при сомнениях откатить `hds-ort` и оставить комментарии (дефект латентный).
* **F3 трогает установщик** — проверять на копии установки (`-WhatIf`/`-NoModels`), не на
  боевой; после реального прогона — `hds check` и один 18-мин файл.
* **F1/F6/F8** меняют крейт `hds-diarize` — обязательный живой прогон 18 мин, чтобы
  убедиться, что `.md` не изменился.

## 10. Чек-лист выполнения

### Этап 0
- [ ] baseline-лог (`fmt`/`clippy`/`test`) в `tools/parity/out/fixes/`
- [ ] паника B1 воспроизведена тестом

### Этап 1 (блокеры)
- [ ] F1: фикс `split_text` + 4 теста, репро зелёный
- [ ] F1: живой прогон 18 мин — `.md` не изменился
- [ ] F2: `backend_explicit`, отправка только при ключе
- [ ] F2: диагностика фактического запроса (`--dry-run`, `hds check`, `whisper-check --json`)
- [ ] F2: README/RELEASE_NOTES/config-комментарии согласованы
- [ ] F2: песочница — sortformer через `diarization.backend` без клиентского ключа
- [ ] F3: докачка модели в `update.ps1`/`update.command`, тесты паритета
- [ ] F3: релизные заметки/README соответствуют коду
- [ ] Этап 1: `fmt`/`clippy -D warnings`/`test --workspace` зелёные

### Этап 2 (средние)
- [ ] F4: предупреждения демона по backend + тест
- [ ] F5: `NemotronCheck`, хеш только для nemotron3/auto, ленивый резолв в хосте + тесты
- [ ] F5: замер `hds check` при `off` (без чтения 400 МБ)
- [ ] F6: кэш отрицательного результата + тест
- [ ] F7: `hds-ort`, оба крейта на нём, интеграционный тест, `cargo tree -i ort`
- [ ] F8: guard длины/RF64, тесты, README-ограничение
- [ ] F9: `diarization_required` в теле → хост, тесты, документация
- [ ] Этап 2: полный прогон + живая песочница (nemotron3/sortformer/off/без модели)

### Этап 3 (техдолг)
- [ ] F10: фактический `mode` + тест
- [ ] F11: единый backend/ffmpeg, мёртвый код, `hds-tools`
- [ ] F12: валидация `onset/offset` + тесты
- [ ] F13: live-тест только через env
- [ ] F14: комментарии про каталоги моделей
- [ ] F15: уникальный staging-каталог whisper
- [ ] F16: потоковая запись/чтение WAV + RSS-замер

### Релиз
- [ ] `RELEASE_NOTES`/`README`/`README_SERVER`/`STATUS` обновлены
- [ ] `installers/build_rust_release.ps1` — пакет без весов, `DirectML.dll` на месте
- [ ] `hds check` на чистой установке без модели после `update` — модель докачана
- [ ] тег `v0.21.0` (или `0.21.1`) после зелёной приёмки по `RELEASING.md`









