# PYTHON_SIDECAR_TRANSCRIBE_P5_HANDOFF_PROMPT.md — контекст и промт для P5/P6

> **Дата среза:** 23.10.2026 (P1–P4 закрыты и закоммичены).
> **Ветка:** `python-sidecar-transcribe`, **HEAD — коммит этого handoff**
> (проверь `git log -1`; базовый срез P1–P4 — `32b1e7c`), база `server-multiuser`
> @ `f202128` (tag `v0.21.0`). Локально **5 коммитов впереди** origin, **не запушено**.
> Рабочая копия чистая.
>
> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§8
> целиком** и отправить первым сообщением. §§0–7 — карта состояния и разрешений.
>
> **Ключевые документы:** `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` (§7 — файлы и
> установщики, §9 P5/P6, §4.7 — железо, §8 — контракты, §10 — приёмка),
> `sidecar/hds_transcribe/README.md` (§P4, «Эксплуатационные детали»),
> `PYTHON_SIDECAR_TRANSCRIBE_P4_HANDOFF_PROMPT.md` (детали P0–P4),
> `tools/parity/out/py_transcribe/P4_REPORT.md` (**gitignored**: числа, модель,
> контракт валидатора, команды).

---

## 0. Разрешения владельца (23.10.2026) — действуют в новом чате

Владелец явно разрешил (зафиксировано в этом файле и в §8):

1. **Elevated-действия.** Можно запускать команды, требующие повышения прав
   (UAC): владелец **нажимает кнопку подтверждения** при запросе системы.
   В частности разрешено:
   * `Restart-Service hermes-disk-search-llm-host` (и других служб `hds`),
     `sc.exe`, `Start-Service`/`Stop-Service`;
   * доступ/правка `C:\ProgramData\hermes-disk-search` (конфиг llm-host, модели,
     `engine`, логи), ACL — при необходимости;
   * установка/переустановка служб и задач Планировщика.
2. **Остановка и выгрузка любых процессов.** Разрешено останавливать/выгружать
   `llm_host` (роли и службу), `hds watch|mcp-http|ui`, `hds transcribe-watch`,
   `llama-server`, Python-воркеры и пробы; освобождать GPU/VRAM под замеры.
   После работ — вернуть резидент (`llm_host run` или служба/Планировщик).
3. **Перезапись рабочих данных:** `config.yaml` (gitignored), `data/`,
   `index.db`, `state_dir`, `tools/parity/out/` — можно.
4. **Сборки:** `target` — junction на `D:\hds-py-transcribe\cargo-target`;
   можно останавливать занятые `target\release\*.exe` и пересобирать релиз.
5. **Сеть/модели:** можно скачивать модели (HF и др.) в `D:\AI\Models gguf`,
   `D:\hds-py-transcribe`.

Остаётся в силе (не разрешения, а ограничения):

* не переписывать `PLAN_*.md` и не менять решения P0–P4;
* не трогать UI (`hds-ui`) и `engine: legacy` без отдельной задачи;
* артефакты замеров — только `tools/parity/out/` (gitignored);
* не откатывать чужие незакоммиченные изменения;
* **коммитить по завершении этапа** (правило изменено 23.10.2026);
  `git push` — только по явной просьбе владельца;
* если нужен HF-токен, новые эталонные тексты, RTTM/ручные таймкоды или живая
  внешняя Radeon — спросить.

---

## 1. Коммиты и состояние

```
(+ этот handoff — последний коммит, см. `git log -1`)
32b1e7c  fix(transcribe): не терять содержательные слова в LLM-патчах
5091e98  feat(transcribe): P4 LLM-постобработка (JSON-патчи через llm_host)
f1525d0  fix(transcribe): устранены дефекты код-ревью P1-P3
c0d6915  feat(transcribe): P1-P3 python-sidecar (GigaAM/pyannote, Rust-клиент,
         глоссарий)
f202128  fix(diarize): as_chunks вместо chunks_exact в parse_wav   ← v0.21.0
```

В `5091e98` вошёл `PYTHON_SIDECAR_TRANSCRIBE_P4_HANDOFF_PROMPT.md`; в нём
правило §6/§8 изменено на «коммитить по завершении этапа» (push — по просьбе).

**Тесты на срезе:** Python **161** (`unittest`, было 123), `cargo test
--workspace` — все наборы ok, `clippy -D warnings` и `fmt --check` — чисто.

### Что работает end-to-end (`transcribe.engine: python`)

`transcribe-watch` / `transcribe-once` / `transcribe-check` / `hds check` →
`hds-index::transcribe::TranscribeClient` → sidecar-воркер `hds_transcribe`:
ASR GigaAM-v3 `e2e_rnnt` fp16 CUDA → Silero VAD (окна ≤25 с) → alignment
(native/whisperx/hybrid) → флаги качества → pyannote community-1 (блоки 20 мин,
батчи 4/4) → merge (max-overlap) → `<stem>.transcript.json` →
`postprocess` (глоссарий → LLM-патчи при `llm.enabled`) → `<stem>.edits.json`,
`needs_review` в транскрипте → Rust рендерит `.md`/`.orig.md`/`.speakers.json`.
Демон держит **один долгоживущий воркер**; модели грузятся по стадиям.

### 1.1 Приёмка P1–P4 (факты, не пересчитывать)

| Проверка | Факт |
|---|---|
| `meet0924` 18 мин (`src_0924.mp3`, эталон `ref_0924.txt`) | WER **17,82 %**, CER **9,52 %**, DER **13,00 %**, атрибуция по словам **90,68 %** |
| `low_confidence + failed` (гейт ≤2 %) | native **0,46 %** (11 слов), hybrid 0,71 %, whisperx 0,63 % |
| 77 мин `meet0512` | ASR ~75 с, диаризация ~151 с, пик VRAM **5 823 МБ**, 10 спикеров; hybrid +76 с (итого 313,6 с = 1,46× P1) |
| Пакет/VRAM 18 мин | блоки 286, пик VRAM **6 737–6 749 МБ**, ASR 19–27 с (RTF ~0,02–0,026) |
| hotwords | G0 biasing работает (gigastt), G1/G2 не пройдены ⇒ `backend: none` (`HOTWORDS_GATES_PASSED = false`) |
| P3-паритет (LLM off, пустой глоссарий) | WER 17,82 / CER 9,52, 2 392 слова, 286 блоков, VRAM 6 748,9 МБ, нет `edits.json`/`postprocess` |

---

## 2. P4: LLM-постобработка (закрыта)

**Модуль** `sidecar/hds_transcribe/llm_patch.py`: кандидаты только по
подозрительным фрагментам (low_confidence/failed, needs_review глоссария,
паттерны, близость к вариантам) → `POST /v1/chat/completions` (`temperature 0`) →
терпимый разбор JSON → валидатор → применение по словам + `merge.join_words`.
LLM не загружается в Python (VRAM не складывается); перед HTTP —
`gc` + `torch.cuda.empty_cache()`.

**Контракт (аддитивно к P3):** `edits.json` — `source=glossary|llm`,
`status=applied|rejected|needs_review`, `reason`, `model`, `latency_ms`,
`llm_reason`; `transcript.json.postprocess.llm` — сводка (только если LLM
включали). Порядок слоёв: **глоссарий → LLM**.

**Валидатор (гейт P4):** спикер/таймкоды/alignment не меняются; числа/даты,
отрицания, содержательные слова сохраняются; длина ≤3× и ≥½ токенов; лимит
`max_edits_per_1000`; пересечения с глоссарием запрещены; патч принимается/
отклоняется целиком.

**Решение владельца C (23.10.2026):** контекст глоссария не обязателен, если
замена содержит **канон** термина и исходный фрагмент похож на **вариант**
термина (`VARIANT_FUZZY_MIN = 0.80`): `RP-система` ↔ `rp система` = 0,90 →
проходит; `слайдик` ↔ лучший вариант = 0,29 → блокируется. Путь `allow_global`
сохранён.

**Найденный дефект (исправлен `32b1e7c`):** Qwen2.5 Q5_K_M предложила
`сейчас написано RP-система` → `ERP-система` (потеря слов), валидатор пропускал
(WER 17,82 → 17,89). Добавлены `too_short` (замена короче половины токенов) и
`content_lost` (потеря содержательного слова вне термина) — патч отклоняется,
WER вернулся к 17,82.

### 2.1 Замеры P4 (meet0924, 18 окон, 2 запроса, `glossary.example.yaml`)

| Модель | предложено/принято/отклонено | LLM, с | пик VRAM | WER / CER |
|---|---|---|---|---|
| Qwen3.5-9B **Q4_K_M** | 9 / **1** / 8 | 10,6 | 6 526 МБ | 17,82 / 9,52 |
| Qwen3.5-9B Q5_K_M | 9 / 1 / 8 | 13,0 | 7 272 МБ | 17,82 / 9,52 |
| Qwen3.5-9B Q6_K (вес роли `chat`) | 9 / 1 / 8 | 13,8 | 7 984 МБ | 17,82 / 9,52 |
| Qwen2.5-7B Q4_K_M | 2 / 0 / 2 | 4,0 | 6 010 МБ | 17,82 / 9,52 |
| Qwen2.5-7B Q5_K_M | 2 / 0 / 2 | 3,7 | 6 670 МБ | 17,82 / 9,52 |

**Решение:** `transcribe.llm.enabled: false` по умолчанию; если включать —
**Qwen3.5-9B Q4_K_M** (единственный нашёл верную `RP-система → ERP-система`);
Qwen2.5 быстрее/компактнее, но на этом промпте даёт крупные «срезы» (0 правок).
Qwen2.5-7B-Instruct Q4_K_M/Q5_K_M **докачаны** в
`D:\AI\Models gguf\Qwen2.5-7B-Instruct\` (4,68 + 5,44 ГБ).

### 2.2 Изменённые файлы P4

`crates/hds-transcribe/src/{settings,params}.rs`,
`crates/hds-index/src/{transcribe,diag}.rs`,
`crates/hds-cli/src/cmd/transcribe_check.rs`,
`sidecar/hds_transcribe/{llm_patch.py,glossary.py,worker.py,README.md}`,
`sidecar/tests/test_llm_patch.py` (38 тестов), `config.example.yaml`.


---

## 3. Окружение этой машины (dev)

* **Профиль:** CUDA (RTX 3060 12 ГБ), Python-воркер —
  `tools\parity\out\py_transcribe\venv-cuda\Scripts\python.exe`;
  `HDS_TRANSCRIBE_PYTHON` указывает на него в прогонах.
* **HF/модели:** `HF_HOME=D:\hds-py-transcribe\hf`,
  `HDS_GIGAAM_CACHE=D:\hds-py-transcribe\gigaam-cache`,
  ONNX — `C:\ProgramData\hermes-disk-search\models\onnx\e2e_rnnt`.
* **Медиа/эталон:** `D:\hds-py-transcribe\work\src_0924.mp3`,
  `ref_0924.txt`, `frag_36_60.wav`, `frag_16_26.wav`.
* **LLM-веса:** `D:\AI\Models gguf\` (Qwen3.5-9B Q4/Q5/Q6, Qwen2.5-7B Q4/Q5/Q8,
  Qwen2.5-coder); роль `chat` у службы — `Qwen3.5-9B-Q6_K.gguf`.
* **Службы:** `hermes-disk-search-llm-host`(:8010/8011/8012),
  `-mcp-http`(:8787), `-ui`(:8765), `-transcribe`; `-watch` остановлена.
* **Сборка:** `cargo` → junction `target` на `D:\hds-py-transcribe\cargo-target`;
  `target\debug\hds.exe` — CLI для e2e.
* **Временный LLM-сервер для замеров** (тот же движок, что у llm_host):
  `llama-server.exe` из `C:\ProgramData\hermes-disk-search\llama-runtime\bin`,
  `--alias chat --port 8020 -c 16384 -ngl 99 -np 1 -b 512 -ub 512
  --cache-type-k q8_0 --cache-type-v q8_0 --reasoning off --reasoning-budget 0
  --no-webui`.

### 3.1 Важно: у резидента `llm_host` сломан учёт свободной VRAM (R29)

Сейчас служба считает свободную VRAM как `min(NVML, memory_free движка)`, а
движок после выгрузки `chat` отдаёт устаревшие **~1 662 МБ** при фактических
**~11 100 МБ** → `load chat` и `/v1/chat/completions` возвращают 409/503
(«не хватает VRAM»). Роли при этом `UNLOADED`, `nvidia-smi` показывает ~1 ГБ
занятых.

**Первое действие нового чата (elevated разрешён):**

```powershell
Restart-Service hermes-disk-search-llm-host      # подтвердить UAC
Start-Sleep -Seconds 5
llm_host.exe status                              # из app\current\bin
llm_host.exe load chat
```

После перезапуска `chat` грузится (роль = Qwen3.5-9B Q6_K, нужно 9 932 МиБ).
Затем прогнать `postprocess` через **резидента** (не временный сервер) и сверить
с замерами `p4/llm_q6_c.json` (ожидается 9 предложено / 1 принято, WER 17,82).
Если служба не поднимается — `llm_host run` и/или `hds service run llm-host`.

---

## 4. Остатки P4 (не блокируют P5)

1. Проверка через резидент после рестарта (см. §3.1), обновить
   `P4_REPORT.md` строкой «через llm_host».
2. Тюнинг промпта: Qwen2.5 возвращает крупные фрагменты; Qwen3.5 часто
   предлагает `no_change`. Кандидаты/промпт тюнить на рабочем глоссарии.
3. Рабочий глоссарий `data/glossary.yaml` (gitignored) — сейчас только пример
   `sidecar/hds_transcribe/glossary.example.yaml`; контексты/варианты уточняет
   владелец.
4. UI-показ `needs_review`/LLM-правок — после P5 (word-level рендер).

---

## 5. Следующие этапы (PLAN §9; коммит по завершении каждого)

### P5. Упаковка и железо

* `installers/build_transcribe_sidecar.ps1 -Profile cuda|cpu|dml`
  (через `installers/sidecar_common.ps1`, портативный CPython из
  `sidecar/python`, venv `venv-transcribe`/`venv-transcribe-dml`);
* `installers/export_gigaam_onnx.ps1` (encoder/decoder/joint в
  `<models_dir>/gigaam-onnx/<variant>/`), `fetch_transcribe_models.ps1`
  (GigaAM-v3, wav2vec2-ru, community-1, segmentation-3.0; пины/sha256);
* `setup.ps1` (`-SkipTranscribeStack`, `-SkipTranscribeModels`),
  `install_server.ps1` (профиль по GPU, модели в `%ProgramData%`),
  `update.ps1`, `installers/build_rust_release.ps1` (в поставку — **один**
  профиль: CUDA или DML);
* `hds check`/`transcribe-check` полностью: профиль, EP, вендор GPU, VRAM/RTF,
  модели/токен, ffmpeg;
* README/NOTICE/RELEASE_NOTES (лицензии: GigaAM MIT, pyannote CC-BY-4.0
  с атрибуцией, wav2vec2 Apache-2.0, WhisperX BSD-2, torch BSD-3,
  Qwen2.5 Apache-2.0, ORT/DirectML MIT, ffmpeg LGPL/GPL);
* замеры CUDA / DML / CPU + отчёты; **DML — обязательная живая проверка на AMD**
  (на этой машине есть iGPU Ryzen 7 5700G; внешняя Radeon — спросить владельца);
* macOS/Metal — best-effort, без блокировки релиза.

**Гейт P5:** чистая установка на dev-машине: `setup.cmd` → `hds check` →
`transcribe-once` без ручных шагов; повторная установка идемпотентна.

### P6. Legacy и релиз

* `engine: legacy` сохраняет старую цепочку на один минорный релиз;
* `hds-diarize`/`parakeet-rs`/`ort`/**Rust-DirectML.dll** (legacy Nemotron) —
  вне default-сборки (feature/флаг); Python-DML (`onnxruntime-directml` в
  отдельном venv) остаётся;
* удаление `whisper`-роли из плана `llm_host` в python-режиме;
* релизные заметки, миграция конфигов, откат (§12).


---

## 6. Грабли, проверенные на практике (не повторять ошибок)

1. **pyannote:** `segmentation_batch_size`/`embedding_batch_size` = 4/4 и
   выставлять **после** `pipeline.to(device)` (перенос сбрасывает свойства);
   батчи ≥8 аллоцируют 9–10 ГБ даже на минуте. Менять алгоритм диаризации —
   только с `DIARIZATION_VERSION++` (сейчас **8**). Пик VRAM мерить своим
   `_VramSampler` (pyannote сбрасывает `max_memory_allocated`).
2. **WER/CER воспроизводятся только на `src_0924.mp3` + `ref_0924.txt`**
   (`eval_reference.py`, `duration=1076.25`). Сравнение — с базой P2/P3
   (17,82 / 9,52), а не с новыми текстами.
3. **gigastt/hotwords:** только CLI/HTTP, файл UTF-8 **без BOM**, бинарник —
   только `HDS_GIGASTT_CLI`; `hotwords.backend` остаётся `none`
   (`HOTWORDS_GATES_PASSED = false`).
4. **Один долгоживущий воркер** (`Arc<Mutex<Option<Client>>>`); не вызывать
   `shutdown` после файла. `cancel` обрабатывается в reader-потоке;
   `transcribe.stop` слушает **только демон** (`transcribe-once` — `None`).
5. **DML/ONNX без word timestamps:** слова `interpolated`, атрибуция по
   сегменту (`speaker_source="segment"`) — это не доказательство смены спикера.
6. **PowerShell:** `>`/`*>` пишут UTF-16 (читать `encoding='utf-16'`);
   многострочный `python -c` ломается — писать скрипт-файлы; `rpc_once.py`
   буферизует stderr; длинные прогоны — `Start-Process` + опрос лога;
   `Start-Process -ArgumentList` не экранирует пробелы в путях — кавычки внутри
   аргумента (`"D:\AI\Models gguf\..."`).
7. **`target` — junction** на `D:\hds-py-transcribe\cargo-target`; сборки только
   туда. Перед релизной сборкой останавливать занятые `target\release\*.exe`.
8. **Rust:** `Config = serde_yaml::Value`; неизвестные поля JSON игнорируются;
   после правок `cargo fmt` + `clippy -D warnings`; тесты не должны зависеть от
   установленных моделей машины (правило после ревью).
9. **LLM-валидатор:** не ослаблять без A/B. Действуют `no_change`, `too_long`,
   `too_short`, `content_lost`, `negation_lost`, `numbers_changed`,
   `overlaps_applied`, `metadata_change`, `no_glossary_context`, `edit_limit`;
   решение C — только канон в замене + сходство с вариантом ≥0,80.
   `edits.json`/`transcript.json.postprocess` — аддитивны к P3;
   `llm.enabled=false` + пустой глоссарий = P2/P3 байт-в-байт.
10. **Одна LLM на машине.** Перед временным `llama-server` на :8020 выгрузить
    роли резидента (`llm_host unload embedding/chat`) и остановить сервер после
    замеров; после работ вернуть резидент.
11. **Elevated-работы:** UAC подтверждает владелец. Не оставлять службы
    остановленными: после рестарта — `Get-Service`/`llm_host status`/`/health`.
12. **Не трогать** UI и `engine: legacy` без задачи; артефакты — только
    `tools/parity/out/py_transcribe/`.

---

## 7. Команды

```powershell
# тесты
python -m unittest discover sidecar\tests            # 161 OK
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# LLM-слой на готовом transcript.json (без ASR)
python tools\parity\out\py_transcribe\p4_llm_run.py `
  tools\parity\out\py_transcribe\venv-cuda\Scripts\python.exe `
  tools\parity\out\py_transcribe\p1_meet\src_0924.transcript.json `
  tools\parity\out\py_transcribe\p4\llm_run.json `
  --base-url http://127.0.0.1:8010/v1 --model chat --max-edits 10 `
  --glossary sidecar/hds_transcribe/glossary.example.yaml
python tools\parity\out\py_transcribe\p4_eval.py `
  tools\parity\out\py_transcribe\p4\llm_run.json `
  tools\parity\out\py_transcribe\p4\llm_run_eval

# e2e через Rust (60-секундный фрагмент; HDS_CONFIG — p4\hds_config_p4.yaml)
$env:HDS_CONFIG='...\p4\hds_config_p4.yaml'
$env:HDS_TRANSCRIBE_PYTHON='...\venv-cuda\Scripts\python.exe'
$env:HF_HOME='D:\hds-py-transcribe\hf'
target\debug\hds.exe transcribe-once D:\hds-py-transcribe\work\frag_36_60.wav
target\debug\hds.exe transcribe-check
target\debug\hds.exe check

# llm_host (после elevated-рестарта)
& 'C:\Program Files\hermes-disk-search\app\current\bin\llm_host.exe' status
& '...\llm_host.exe' load chat
& '...\llm_host.exe' unload chat
Restart-Service hermes-disk-search-llm-host   # UAC

# GPU
nvidia-smi --query-gpu=memory.used,memory.free --format=csv,noheader
```


---

## 8. ПРОМТ для нового чата (копировать целиком)

```text
Ты — Cline в режиме Act. Продолжаешь проект hermes-disk-search
(C:\Users\Sasha\hermes-disk-search), ветка python-sidecar-transcribe.
HEAD — коммит этого handoff (проверь `git log -1`): P4 (LLM-постобработка)
закоммичена (5091e98), дефект «удаление содержательных слов» исправлен
(32b1e7c); база — server-multiuser @ f202128 (tag v0.21.0). Ветка на 5
коммитов впереди origin, НЕ запушена, рабочая копия чистая. Твоя задача —
P5: упаковка и железо (PLAN §7, §9 P5), затем P6: legacy и релиз
(PLAN §9 P6). P0–P4 не переделывать.

РАЗРЕШЕНИЯ ВЛАДЕЛЬЦА (23.10.2026, действуют):
- elevated-действия разрешены: запускай команды, требующие повышения прав;
  владелец нажимает кнопку подтверждения UAC. Разрешено Restart-Service /
  sc.exe / Stop-Service / Start-Service для служб hermes-disk-search-*,
  доступ и правка C:\ProgramData\hermes-disk-search (llm-host config, модели,
  logs), установка служб/задач Планировщика.
- разрешено останавливать и выгружать ЛЮБЫЕ процессы, если это нужно для
  прогонов/замеров: llm_host (роли и службу), hds watch|mcp-http|ui|
  transcribe-watch, llama-server, Python-воркеры и пробы; освобождать GPU/VRAM.
  После работ возвращать резидент (llm_host run / служба / Планировщик) и не
  оставлять службы остановленными.
- можно перезаписывать config.yaml (gitignored), data/, index.db, state_dir,
  tools/parity/out/; пересобирать релизные бинарники (target — junction на
  D:\hds-py-transcribe\cargo-target), останавливая занятые exe.
- можно скачивать модели в D:\AI\Models gguf и D:\hds-py-transcribe.
Остаётся в силе: не переписывать PLAN_*.md и решения P0–P4; не трогать UI
(hds-ui) и engine: legacy без отдельной задачи; артефакты — только
tools/parity/out/; не откатывать чужие незакоммиченные изменения; коммитить
по завершении этапа (push — только по явной просьбе).

ПЕРВЫЕ ДЕЙСТВИЯ (не начинай с кода):
1) git status --short --branch; git --no-pager log -4 --oneline --decorate;
   git rev-list --count origin/server-multiuser..HEAD
2) прочитай PYTHON_SIDECAR_TRANSCRIBE_P5_HANDOFF_PROMPT.md (этот контекст),
   PLAN_PYTHON_SIDECAR_TRANSCRIBE.md §7, §9 P5/P6, §4.7, §8, §10, §12;
   sidecar/hds_transcribe/README.md; P4_REPORT.md (gitignored) §2–§3, §8
3) прогони тесты: python -m unittest discover sidecar\tests (ожидается 161),
   cargo test --workspace, clippy -D warnings, fmt --check
4) проверь llm_host: GET /health, /v1/models; зафиксируй VRAM. Сейчас у
   резидента сломан учёт свободной VRAM (R29: min(NVML, memory_free движка),
   движок отдаёт ~1662 МиБ при 11 100 свободных) — chat не грузится. Это
   ЛЕЧИТСЯ elevated-рестартом (разрешено):
       Restart-Service hermes-disk-search-llm-host
       llm_host.exe load chat     # C:\Program Files\hermes-disk-search\app\current\bin
   После рестарта прогони postprocess через резидента (не временный сервер)
   на tools\parity\out\py_transcribe\p1_meet\src_0924.transcript.json с
   llm.enabled=true, base_url=http://127.0.0.1:8010/v1, model=chat,
   max_edits_per_1000=10, glossary=sidecar/hds_transcribe/glossary.example.yaml;
   сверь с p4/llm_q6_c.json (ожидается 9 предложено / 1 принято, WER 17,82).
5) только после этого — код P5.


СОСТОЯНИЕ (факты, не пересчитывать):
- P1–P4 работают e2e (python-sidecar: GigaAM-v3 + pyannote community-1 +
  глоссарий + LLM-патчи через llm_host); LLM off по умолчанию.
- meet0924: WER 17,82 / CER 9,52 / DER 13,00 / атрибуция 90,68 %,
  low_confidence 11 (0,46 %), блоки 286, пик VRAM 6 737–6 749 МБ.
- P4 (18 окон, 2 запроса): Qwen3.5-9B Q4_K_M 9/1/8, 10,6 с, 6 526 МБ (лучший);
  Q5 9/1/8, 13,0 с, 7 272 МБ; Q6 9/1/8, 13,8 с, 7 984 МБ;
  Qwen2.5-7B Q4 2/0/2, 4,0 с, 6 010 МБ; Q5 2/0/2, 3,7 с, 6 670 МБ.
  Валидатор: спикер/таймкоды/alignment не меняются; too_long/too_short/
  content_lost/negation_lost/numbers_changed/overlaps_applied/metadata_change/
  no_glossary_context/edit_limit; решение C — канон в замене + сходство
  фрагмента с вариантом ≥0,80.
- Python 161 тест; cargo workspace/clippy/fmt зелёные.

ЗАДАЧИ P5 (в этом порядке):
P5.0 Аудит текущих установщиков (build_sidecar.ps1, setup.ps1,
     install_server.ps1, update.ps1, build_rust_release.ps1) и sidecar/python;
     зафиксировать, что переиспользуется, а что новое.
P5.1 installers/sidecar_common.ps1 (если его нет) +
     installers/build_transcribe_sidecar.ps1 -Profile cuda|cpu|dml:
     портативный CPython, venv-transcribe / venv-transcribe-dml,
     requirements.lock / requirements-dml.lock, индексы pytorch cu128/cpu.
P5.2 installers/export_gigaam_onnx.ps1 (encoder/decoder/joint в
     <models_dir>/gigaam-onnx/<variant>/, идемпотентно, штамп ревизии).
P5.3 installers/fetch_transcribe_models.ps1 (GigaAM-v3 e2e_rnnt, wav2vec2-ru,
     community-1, segmentation-3.0; -ModelsDir/-HfToken; пины/sha256).
P5.4 setup.ps1 (-SkipTranscribeStack/-SkipTranscribeModels),
     install_server.ps1 (профиль по GPU; модели в %ProgramData%),
     update.ps1; идемпотентность.
P5.5 installers/build_rust_release.ps1: один профиль venv в поставку
     (CUDA или DML), sidecar/hds_sidecar + sidecar/hds_transcribe.
P5.6 hds check / transcribe-check: профиль, EP, вендор GPU, версии, модели,
     токен, ffmpeg, фактический VRAM/RTF (в python-режиме — новый стек).
P5.7 Замеры CUDA (3060), DML (AMD; на этой машине iGPU Ryzen 7 5700G —
     уточни у владельца, нужна ли внешняя Radeon), CPU; отчёты
     tools/parity/out/py_transcribe/P5_REPORT.md.
P5.8 Чистая установка на dev-машине: setup.cmd → hds check → transcribe-once
     без ручных шагов; повторная установка идемпотентна.
ГЕЙТ P5: чистая установка и идемпотентность; DML проверен живьём;
legacy не сломан; .md/.orig.md/.speakers.json/UI не тронуты.

ЗАДАЧИ P6 (после P5):
P6.1 engine: legacy сохраняет старую цепочку на один минорный релиз;
P6.2 hds-diarize/parakeet-rs/ort/Rust-DirectML.dll — вне default-сборки
     (feature/флаг); Python-DML (onnxruntime-directml в venv) остаётся;
P6.3 убрать whisper-роль из плана llm_host в python-режиме;
P6.4 RELEASE_NOTES, миграция конфигов, откат (§12), NOTICE/лицензии.

ГРАБЛИ: pyannote батчи 4/4 после to(device), DIARIZATION_VERSION=8,
_VramSampler; WER только src_0924+ref_0924; hotwords none, gigastt только
HDS_GIGASTT_CLI, UTF-8 без BOM; один долгоживущий воркер, cancel в
reader-потоке, stop_file только демону; DML — speaker_source=segment;
PowerShell: >/*> пишут UTF-16, многострочный python -c ломается, длинные
прогоны — Start-Process + лог, пробелы в путях Start-Process — кавычки;
target — junction на D:; HDS_TRANSCRIBE_PYTHON → venv-cuda;
HF_HOME/HDS_GIGAAM_CACHE на D:; cargo fmt/clippy; тесты не зависят от
установленных моделей; одна LLM на машине (перед временным llama-server на
:8020 выгрузить роли резидента, после — остановить и вернуть резидент).

ОГРАНИЧЕНИЯ: коммитить по завершении этапа (push — только по явной просьбе);
не переписывать PLAN_*.md; не менять решения P0–P4; не трогать UI и legacy
без задачи; артефакты — tools/parity/out/; если нужен HF-токен, новые
эталонные тексты, RTTM/ручные таймкоды, внешняя Radeon или решение по
контракту (формат патчей, модель LLM) — спроси.

Начни с первых действий, кратко изложи план P5 (P5.0–P5.8) и приступай к P5.0
(аудит установщиков), не меняя поведение существующих скриптов.
```

