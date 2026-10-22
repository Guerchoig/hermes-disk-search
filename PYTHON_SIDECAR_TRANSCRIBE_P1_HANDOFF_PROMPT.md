# PYTHON_SIDECAR_TRANSCRIBE_P1_HANDOFF_PROMPT.md — контекст и промт для P1

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§9 целиком**
> (он самодостаточен) и отправить первым сообщением. §§0–8 — «карта» состояния:
> их можно читать выборочно, но §9 уже содержит главное.
>
> **Дата среза:** 22.10.2026. **Ветка:** `python-sidecar-transcribe`
> (база — `server-multiuser` @ `f202128`, tag v0.21.0). **Боевой код ещё не менялся.**
> **P0 завершён**, решения **подтверждены владельцем**; отдельно подтверждено:
> **реальных многоканальных записей нет**, DML на встроенной AMD iGPU **не считается
> ускорением**.
>
> **Ключевые файлы:** `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` (план, untracked),
> `PYTHON_SIDECAR_TRANSCRIBE_HANDOFF_PROMPT.md` (handoff P0, untracked),
> `tools/parity/out/py_transcribe/spike/REPORT.md` (отчёт P0, gitignored).

---

## 0. Что дал P0 (факты, на которые опирается P1)

Окружения собраны и проверены (обе связки ставятся вместе — риск R1 снят):

| Компонент | venv-cuda | venv-dml |
|---|---|---|
| torch | **2.8.0+cu128** (+torchaudio, torchvision 0.23.0+cu128) | **2.8.0+cpu** |
| transformers | **4.57.1** | 4.57.1 |
| pyannote.audio | **4.0.0** | 4.0.0 |
| torchcodec | 0.7.0 (не грузится с FFmpeg 9, см. §7) | 0.7.0 |
| whisperx | **3.8.7rc1** | **отсутствует** |
| onnx / onnxruntime | 1.19.0 / **1.23.2** (EP: Azure, CPU) | 1.19.0 / **onnxruntime-directml 1.24.4** (EP: **Dml**, CPU); `onnxruntime` **нет** |
| прочее | gigaam 0.2.0 (git main), silero-vad 6.2.3, psutil, jiwer | gigaam 0.2.0 (`--no-deps`), silero-vad, psutil |

Python 3.12.14 (uv). Железо: **RTX 3060 12 ГБ** (CUDA), **AMD Radeon Vega iGPU** (DX12/DML),
16 логических ядер, 64 ГБ RAM, Windows 11, ffmpeg 9.0.1 (static).

**Замеры (18 мин = `meet0924`, если не указано иное):**

| Прогон | RTF | Время | VRAM |
|---|---|---|---|
| e2e_rnnt fp16 CUDA + Silero VAD | **0,019** | 20,63 с | 6,76 ГБ |
| e2e_rnnt fp16 CUDA, 56 мин | 0,015 | 49,84 с | 7,30 ГБ |
| e2e_rnnt fp16 CUDA, 77 мин | 0,014 | 63,85 с | 5,83 ГБ |
| torch CPU fp32 (18 мин) | 0,170 | 183,15 с | RSS **15,4 ГБ** |
| ONNX CPU (18 мин) | 0,074 | 79,37 с | RSS 2,72 ГБ |
| ONNX **DML**, NVIDIA через DX12 | 0,023 | 25,05 с | — |
| ONNX **DML**, AMD iGPU (`device_id` 0/1) | **0,171–0,182** | 183–196 с | RSS 2,7 ГБ |

**Качество по реальному эталону заказчика** (18 мин, mp3 48 кГц stereo + txt, 98 реплик / 7 имён):

| Метрика | Значение | Оговорка |
|---|---|---|
| WER | **17,82 %** | 278 замен / 131 удал. / 48 вставок; 9,35 % замен — цифры/латиница (ITN) |
| CER | **9,52 %** | — |
| DER (collar 0,25) | **12,99 %** | весь DER = `miss`; false alarm и confusion = 0,0 |
| Атрибуция по словам | **90,8 %** | 4 кластера pyannote против 7 имён эталона |
| Атрибуция по репликам | **59,2 %** | страдает от искусственных концов реплик эталона |
| Диаризация 18 мин | RTF 0,030 | 32,1 с, VRAM 1,63 ГБ |
| Диаризация 77 мин | RTF 0,029 | 132,7 с, **VRAM 9,0 ГБ** (риск!), 9 спикеров |

**Alignment:** WhisperX покрывает 100 % слов, но MAE к нативным — **65,4 мс** (start) /
166,9 мс (end), 1,75 % слов расходятся > 350 мс, цена +23 с и +1,95 ГБ VRAM на 18 мин
⇒ **default `native`**.

**Каналы:** все 4 реальные записи — **48 кГц, 2 канала, L≈R (r=0,9998) ⇒ dual-mono**.
`auto` обязан выбирать **downmix**. Синтетический тест «две разные дорожки»: downmix даёт
WER 44 % / 109 % к поканальному тексту ⇒ `split` нужен, но на реальных данных не проверен
(владелец подтвердил: многоканальных записей нет).

**G0 hotwords:** эффект **есть** (`gigastt` 2.22.0, CLI): «RP-система» → «ERP-система» при
`--hotwords-file` + boost 8; на верном «1С» и на отсутствующем «МЧД» текст не меняется
(ложных вставок нет). Но это отдельный INT8-рантайм, G1/G2 без RTTM/эталона не пройдены
⇒ **`hotwords.backend: none`**, глоссарий/пост-коррекция — P3/P4.

**Полный отчёт:** `tools/parity/out/py_transcribe/spike/REPORT.md` (§17 — проверка на реальном
эталоне и живой AMD). Артефакты: `spike/*.json`, скрипты `scripts/*.py`,
`setup_envs.ps1`, `run_matrix.ps1`, `run_g0.ps1`.

---

## 1. Подтверждённые решения (владелец, 22.10.2026 — не пересматривать)

| # | Что | Решение |
|---|---|---|
| 1 | ASR | **GigaAM-v3 `e2e_rnnt`**, fp16 CUDA — основной путь; ONNX (CPU/DML) — для профилей без CUDA; `rnnt`/`e2e_ctc` — только контрольные, не в боевом пути |
| 2 | Hotwords | **`hotwords.backend: none`**. G0 дал эффект, но G1/G2 не пройдены; biasing = отдельный INT8-рантайм `gigastt`, а не настройка GigaAM. Текстовая замена — post-correction (P3/P4), не biasing |
| 3 | Alignment | **`align.mode: native`** (word_timestamps GigaAM) — default; `whisperx`/`hybrid` — режим качества (P2), не в MVP |
| 4 | VAD / longform | **Silero VAD** (окна ≤ 25 с, режем только в паузах) — default; **`transcribe_longform` НЕ использовать** (см. §7.1) |
| 5 | Диаризация | **pyannote community-1** (4.0.0), CUDA/CPU; exclusive + regular; **независимый `diarization.json`** с кэшем; **нарезка длинных файлов блоками 15–30 мин** (VRAM 9 ГБ на 77 мин) |
| 6 | Merge | Python: max-overlap; `low_confidence`/`interpolated`/`failed` **не двигают спикера**; `overlap`/`ambiguous` в JSON; `.md` рендерит Rust |
| 7 | LLM-постобработка | Python `postprocess` → `llm_host`; JSON-патчи + валидатор; **`enabled: false`** по умолчанию (P4) |
| 8 | Профили | **CUDA** (NVIDIA) — основной; **CPU** — fallback и **правильный выбор для iGPU**; **DML** — только код-путь/fallback, **не ускорение** (на живой AMD iGPU медленнее CPU) |
| 9 | Каналы | **`decode.channels: auto`** (ffprobe + корреляция): ≥2 реально разных канала → поканально; дубль-стерео (r > 0,9) → downmix. Реальных `split`-файлов нет — не выдавать синтетику за приёмку |
| 10 | Упаковка | Общий пакет `sidecar/hds_sidecar` + общий CPython, но **два процесса** (`hds_extract`, `hds_transcribe`) и **venv по профилю** |
| 11 | Эталон | 18 мин с именами спикеров — **единственный** эталон; RTTM/концов реплик нет ⇒ DER завышен, MAE границ «против правды» не считается |

---

## 2. Что меняется против плана по итогам P0

| Пункт плана | Что в плане | Что делать в P1 (по фактам P0) |
|---|---|---|
| §9 P1 «`transcribe_longform`» | включён в MVP | **Заменить на Silero VAD-чанкинг**: longform требует gated `segmentation-3.0` **и** рабочий `torchcodec`, а он не грузится с FFmpeg 9 |
| §4.7 DML | «AMD — DML-профиль» | DML оставить как код-путь, но **профиль по умолчанию для iGPU — CPU**; в DML-сессии **пиновать `device_id`** и логировать адаптер, иначе RTF скачет в 7 раз |
| §4.4 диаризация | «подать waveform целиком» | на 77 мин VRAM 9,0 ГБ ⇒ **блоки 15–30 мин** + кэш по блокам; safe_globals обязательны |
| §4.5 каналы | «auto/downmix/split» | `split` оставить в коде, но **приёмка только на реальных разных дорожках** (их нет); для текущих файлов auto → downmix |
| §4.1 ASR | «HF-ревизии через transformers» | word_timestamps есть **только** в пакете `gigaam` из GitHub (HF remote-code их не отдаёт) ⇒ в бою — `gigaam.load_model(...)` |
| §10 приёмка | «WER не хуже текущей цепочки» | абсолютный WER = **17,8 %** на единственном эталоне; сравнивать новую реализацию **с этой же цифрой** на том же файле |

---

## 3. Объём P1 (MVP на Python + общий пакет)

Порядок работ — как в плане §9 P1, с поправками §2:

**A. Общий пакет `sidecar/hds_sidecar`** (новый, не процесс):
`__init__.py` (пустой), `stdio.py` (fd1→stderr, UTF-8 stdin, NDJSON, reply/ok/err, serve()),
`runtime.py` (reader-thread, idle-timeout, EOF, изоляция одной ошибки), `config.py`
(state_dir/config_path/load_config — yaml/hds.config лениво), `progress.py` (`[stage] …` в stderr),
`fs.py` (temp, безопасные пути, long-path).
**Перевести `sidecar/hds_extract/worker.py` на пакет без изменения поведения** — методы,
`hello.protocol=1`, idle-timeout, EOF, обработка ошибок. Проверка: golden B4 (`tools/parity`)
+ `crates/hds-extract/tests/worker_live.rs` до/после.

**B. Воркер `sidecar/hds_transcribe`** (новый процесс, venv по профилю):
`worker.py` (`serve()` + `hello/transcribe/postprocess/cancel/shutdown`), `params.py`
(датаклассы параметров — приходят из Rust, **не** из config.yaml),
`decode.py` (ffprobe каналов, ffmpeg → WAV 16 кГц mono/поканально, `channels: auto|downmix|split`),
`asr_gigaam.py` (e2e_rnnt PyTorch fp16/CPU, **Silero VAD-чанки ≤ 25 с**, батч 8,
нативные `Word(text,start,end)`), `asr_onnx.py` (те же варианты через onnxruntime EP
`cuda|dml|cpu`; **SessionOptions: `ORT_SEQUENTIAL`, `enable_mem_pattern=False`,
`intra_op_num_threads=1` на GPU**; **`device_id` пинуется**), `diarize_pyannote.py`
(community-1, **safe_globals** перед загрузкой, waveform-массив, **блоки 15–30 мин**,
exclusive + regular, `diarization.json` с кэшем), `align.py` (`native`; `whisperx` — заглушка
до P2), `merge.py` (чистые функции: max-overlap, `ambiguous`/`overlap`, low-confidence
не двигает спикера, разбиение по смене спикера/паузе > 0,8 с/границе предложения,
принудительно > 30 с или > 500 символов), `quality.py` (флаги
`aligned/interpolated/low_confidence/failed` + `alignment_quality`), `cache.py`
(transcript/diarization JSON, ключи и инвалидация), `models.py` (абсолютные пути/ревизии),
`requirements.lock` (CUDA/CPU) и `requirements-dml.lock`, `README.md`.

**C. Rust, минимальный диф:**
* `crates/hds-extract/src/worker.rs` — сделать `Worker::request(method, params, retry_dead)`
  публичным; `discover_python_with(env_var, venv_rel)`; для транскрибации **`retry_dead=false`**;
* **новый** `crates/hds-transcribe` — типы `TranscribeParams/Result`, `PostprocessParams/Result`,
  клиент поверх `hds_extract::Worker`, путь `sidecar/hds_transcribe/worker.py`, проверка
  `hello.capabilities`, маппинг ошибок (`model_missing`, `hf_token_missing`, `ffmpeg_missing`,
  `cuda_oom`, `dml_oom`, `onnx_error`, `diarization_failed`, `cancelled`, `bad_audio`),
  `WorkerConfig` с таймаутом ~7 ч и `stderr_log`;
* `crates/hds-index/src/transcribe.rs` — `TranscribeClient`: HTTP → `hds_transcribe::Client`;
  два вызова `transcribe` → (опц.) `postprocess` → запись `.md`; сохранить имена/атомарную
  запись/`detect_speaker_slots`; `.orig.md`, `.speakers.json`, `.bak`, `source_disposal`;
* `crates/hds-cli/src/cmd/whisper_check.rs` → `transcribe-check` (alias `whisper-check`):
  версии Python-стека, модели, HF-token, ffmpeg, профиль/EP/устройство;
* `crates/hds-index/src/diag.rs` — пункт `transcribe`;
* `Cargo.toml` — новый член `crates/hds-transcribe`.

**D. Конфиг (`config.example.yaml`, аддитивно):** `transcribe.engine: python|legacy`,
`transcribe.profile: cuda|cpu|dml`, `asr.*` (`variant`, `revision`, `backend`, `batch_size`,
`longform: off`, `vad: silero`), `align.mode`, `diarization.*` (`enabled`, `required`,
`min/max/num_speakers`, `block_minutes`), `decode.channels`, `cache.diarization`,
`hotwords.backend: none`, `llm.enabled: false`, `request_timeout`, `idle_unload_sec`,
`max_requests`, `fallback_cpu`. Старые ключи `auto_transcribe.diarization*`,
`index.whisper_*`, `diarization.*` — читать и маппить.

**E. Тесты:** protocol (общий), `merge` unit (без моделей), golden `.md`, демон e2e на 1 файле,
паритет извлечения документов (golden B4 + `worker_live`), `cargo test/clippy/fmt`.

**F. Не входит в P1:** WhisperX/hybrid (P2), глоссарий и G1–G3 (P3), LLM-патчи (P4),
установщики/упаковка (P5), удаление legacy (P6).

---

## 4. Контракты, которые нельзя ломать

* **`.md`**: `### SPEAKER_00 [00:00:01 - 00:00:11]`, литерал **`UNASSIGNED`**; нумерация с нуля;
  рендер — в Rust, Python отдаёт только структуру (слова/блоки/спикеры);
* **`.orig.md`** — итоговый текст **после** postprocess, но **до** подстановки имён;
  **`.speakers.json`** — карта плейсхолдер→имя (пустая при создании); **`.bak`** — как сейчас;
* **UI** «Транскрибация» (присвоение имён, «Перезапустить задание», папки) — без изменений;
* демон `hds transcribe-watch`: очередь, идемпотентность, `transcribe.lock`,
  `source_disposal` (delete по умолчанию, только после успешной непустой записи),
  пауза индексации, `transcribe.stop`;
* `auto_transcribe.inbox_dir|out_dir|state_dir` **не индексируются**; `out_dir` не попадает в индекс;
* `engine: legacy` работает без изменений (старый путь через `llm_host`);
* ошибки — структурированные `{code, message, hint}`; Rust превращает в текст задания.

---

## 5. Окружение, модели и воспроизведение (уже готово, переиспользовать)

**Важно про диск:** на `C:` ~5,5 ГБ свободно, поэтому venv и кэши лежат на `D:\hds-py-transcribe\`,
а в `tools/parity/out/py_transcribe/` — **junctions** с теми же именами:

```
tools/parity/out/py_transcribe/venv-cuda    -> D:\hds-py-transcribe\venv-cuda
tools/parity/out/py_transcribe/venv-dml     -> D:\hds-py-transcribe\venv-dml
tools/parity/out/py_transcribe/venv-gigastt -> D:\hds-py-transcribe\venv-gigastt (только G0)
```

Переменные: `HDS_GIGAAM_CACHE=D:\hds-py-transcribe\gigaam-cache`,
`HF_HOME=D:\hds-py-transcribe\hf`, `HDS_SPIKE_WORK=D:\hds-py-transcribe\work`,
`UV_CACHE_DIR=D:\hds-py-transcribe\uv-cache`, `PYANNOTE_METRICS_ENABLED=0`.
Модели: GigaAM `v3_e2e_rnnt.ckpt` (428 МБ) + токенизатор; ONNX-экспорт
`D:\hds-py-transcribe\onnx\e2e_rnnt_fp32\` (encoder 885 МБ, decoder 4,6 МБ, joint 2,7 МБ, yaml);
pyannote community-1 + segmentation-3.0 — в `D:\hds-py-transcribe\hf`.

**Эталон и корпус:**
* `D:\diar-spike-data\meet0924.wav` (1076 с), `meet0514.wav` (3363 с), `meet0512.wav` (4620 с), `frag60.wav`;
* реальный источник: `D:\САША\ИРИС\ИИ\Тестовые записи совещаний\Новый каталог\20260924-…-mn7i8.mp3`
  (48 кГц stereo) + `20260924_Транскрипция … .txt` (98 реплик, 7 имён). Копии без пробелов:
  `D:\hds-py-transcribe\work\src_0924.mp3`, `ref_0924.txt`.

**Спайк-скрипты (образцы для P1, не переносить как есть):**
`scripts/asr_torch.py` (Silero + `_decode_chunks` + word timestamps), `scripts/asr_onnx.py`
(SessionOptions-патч + `device_id` + EP), `scripts/diar_spike.py` (safe_globals, waveform,
exclusive/regular), `scripts/eval_reference.py` (WER/CER/DER/атрибуция по эталону),
`scripts/channels_real.py`. Запуск/повтор: `setup_envs.ps1`,
`run_matrix.ps1 -Stage asr|onnx|align|diar|channels`, `run_g0.ps1`;
в `REPORT.md` §14 — полный список команд.

---

## 6. Гейт P1 (без него P1 не закрыт)

1. `hds transcribe-once <файл>` даёт `.md` **того же формата** (заголовки `### SPEAKER_NN [..]`,
   `UNASSIGNED`), рядом `.orig.md`, `.speakers.json`, `.bak`.
2. UI открывает результат, имена присваиваются; `out_dir` **не** попал в индекс.
3. Демон: файл в inbox → `.md` в out → исходник утилизирован по `source_disposal`;
   `transcribe.stop` останавливает задание; повторный запуск идемпотентен.
4. **Извлечение документов не изменилось**: golden B4 + `worker_live` до/после перевода
   `hds_extract` на `hds_sidecar`.
5. `cargo test --workspace`, `clippy -D warnings`, `fmt` — зелёные; `pytest` (Python) — зелёные.
6. `engine: legacy` работает (старый путь), `engine: python` — новый.
7. На `meet0924` (18 мин) новая реализация: **WER ≤ 17,8 %**, атрибуция по словам **≥ 90,8 %**,
   RTF CUDA **≤ 0,02**; пик VRAM ≤ 10 ГБ без LLM.
8. Диаризация: `diarization.json` создаётся и переиспользуется (кэш), блоки 15–30 мин,
   VRAM на 77 мин ≤ 10 ГБ.

---

## 7. Грабли P0, которые обязаны быть учтены в коде

1. **`torchcodec` 0.7.0 не грузится с FFmpeg 9 (static)** — `libtorchcodec_core7.dll` не
   находится. ⇒ В боевом пути **не полагаться на torchcodec**: аудио декодируем ffmpeg CLI
   в массив, pyannote получает `{"waveform": tensor, "sample_rate": 16000}`.
   `transcribe_longform` — не использовать.
2. **pyannote 4.0.0 + torch 2.8:** перед `Pipeline.from_pretrained` обязательны safe-globals:
   `torch.serialization.add_safe_globals([...классы из pyannote.audio.core.task/model...])`,
   иначе `UnpicklingError: Specifications`.
3. **Gated-модели:** `pyannote/speaker-diarization-community-1` и `segmentation-3.0` требуют
   **принятых условий** на HF (одного токена мало; до принятия — 403). Проверять на старте и
   отдавать `hf_token_missing` **до** задания; в рантайме — `local_files_only`.
4. **HF-кэш на Windows без Developer Mode:** symlink падает (`WinError 1314`) на «грязном»
   каталоге модели ⇒ либо `local_dir`, либо чистый кэш; ошибка должна быть понятной.
5. **`onnxruntime` и `onnxruntime-directml` в одном venv — конфликт** (оба дают модуль
   `onnxruntime`). DML-venv — отдельный; `gigaam` ставить `--no-deps` (иначе тянет
   `onnxruntime==1.23.*`).
6. **DML:** `execution_mode=ORT_SEQUENTIAL`, `enable_mem_pattern=False`,
   `intra_op_num_threads=1`; **`device_id` пинуется** (без него DXGI-адаптер нестабилен,
   RTF отличался в 7 раз: 0,023 ↔ 0,171). Логировать фактический EP/адаптер.
7. **iGPU ≠ ускорение:** на встроенной AMD DML медленнее CPU в 2,2–2,5× ⇒ профиль `cpu`
   для iGPU; DML — fallback и только при явном выборе/дискретной Radeon.
8. **Память CPU-профиля:** torch CPU fp32 на 18 мин дал **RSS 15,4 ГБ** ⇒ освобождать чанки,
   не держать весь файл и все окна одновременно; ограничить батч.
9. **`gigaam` из GitHub**, а не HF remote-code: только там `word_timestamps=True` и
   `to_onnx(dtype=…)`.
10. **PowerShell/кириллица (для тестов и установщика):** `Set-Content -Encoding UTF8` пишет
    BOM (gigastt отбрасывал такие hotword-файлы); `python -c` с кириллицей — мусор; пути
    с пробелами рвут `Start-Process` (копировать в путь без пробелов). В прод-код это не
    тащить, но в тестах учитывать.
11. **`onnxruntime` (PyPI, Windows) без CUDA EP** — «torch vs ONNX на CUDA» требует
    `onnxruntime-gpu` + DLL; для CUDA-профиля ONNX не нужен.

---

## 8. Первые действия нового чата (чек-лист)

```powershell
cd C:\Users\Sasha\hermes-disk-search
git status --short --branch          # ожидаем: только ?? план/handoff/P1-промт
git log -1 --oneline --decorate      # f202128
Get-Item .\PYTHON_SIDECAR_TRANSCRIBE_P1_HANDOFF_PROMPT.md, .\PLAN_PYTHON_SIDECAR_TRANSCRIBE.md
Get-Item .\tools\parity\out\py_transcribe\spike\REPORT.md
```

Затем: прочитать этот файл (§0–§7), `REPORT.md` §0/§10/§17, план §5.1–5.4 (протокол),
§9 P1, §16 (DoD); после этого начинать P1 с пункта **A** (§3): `sidecar/hds_sidecar` +
перевод `hds_extract` **без изменения поведения**.

**Правила:** не трогать чужие незакоммиченные правки; не переписывать боевые `PLAN_*.md`;
не коммитить без явной просьбы; артефакты замеров и временные venv — только в
`tools/parity/out/py_transcribe/` (gitignored).

---

## 9. Промт для нового чата (копировать целиком)

```text
Ты — Cline в режиме Act. Продолжаешь проект hermes-disk-search
(C:\Users\Sasha\hermes-disk-search), ветка python-sidecar-transcribe
(база server-multiuser @ f202128, tag v0.21.0). P0 (спайк) завершён,
решения подтверждены владельцем, боевой код ещё не менялся.
Твоя задача — P1: MVP транскрибации/диаризации на Python-sidecar.

ПЕРВЫЕ ДЕЙСТВИЯ (не начинай с кода):
1) git status --short --branch и git log -1 --oneline --decorate
2) прочитай PYTHON_SIDECAR_TRANSCRIBE_P1_HANDOFF_PROMPT.md целиком
3) прочитай PLAN_PYTHON_SIDECAR_TRANSCRIBE.md: §5.1–5.4 (sidecar и протокол), §6 (конфиг),
   §8 (контракты), §9 «P1. MVP», §16 (Definition of Done)
4) прочитай tools/parity/out/py_transcribe/spike/REPORT.md: §0, §10, §17
5) прочитай PLAN_AUTO_TRANSCRIBE.md §5, sidecar/README.md,
   crates/hds-extract/src/worker.rs и protocol.rs
Только после этого пиши код.

ПОДТВЕРЖДЁННЫЕ РЕШЕНИЯ (не пересматривать):
- ASR: GigaAM-v3 e2e_rnnt, PyTorch fp16 на CUDA — основной; ONNX (CPU/DML) — для профилей
  без CUDA. rnnt/e2e_ctc — только контрольные, не в боевом пути.
- Alignment: native word_timestamps GigaAM (default). WhisperX/hybrid — P2, не в MVP.
- VAD: Silero (окна <=25 с, режем только в паузах). transcribe_longform НЕ использовать.
- Диаризация: pyannote community-1 4.0.0, exclusive + regular, независимый diarization.json
  с кэшем; длинные файлы — блоками 15–30 мин (на 77 мин VRAM 9 ГБ).
- Merge: max-overlap; low_confidence/interpolated/failed не двигают спикера;
  overlap/ambiguous в JSON; .md рендерит Rust.
- Hotwords: backend none (G0 дал эффект, но G1/G2 не пройдены). LLM-постобработка: enabled false.
- Профили: CUDA — основной; CPU — fallback и правильный выбор для iGPU;
  DML — код-путь/fallback, НЕ ускорение (на встроенной AMD медленнее CPU в 2,2–2,5x).
- Каналы: decode.channels auto (ffprobe + корреляция); дубль-стерео (r>0,9) -> downmix,
  реально разные каналы -> поканально. Реальных split-файлов у владельца нет.
- Упаковка: общий пакет sidecar/hds_sidecar + общий CPython, но ДВА процесса
  (hds_extract, hds_transcribe) и venv по профилю.

ФАКТЫ P0 (для сверки, не пересчитывать):
- 18 мин: RTF 0,019 (20,63 с), VRAM 6,76 ГБ; 56 мин: 0,015; 77 мин: 0,014.
- Эталон заказчика (18 мин, 98 реплик, 7 имён): WER 17,82 %, CER 9,52 %,
  DER 12,99 % (collar 0,25; весь DER = miss), атрибуция по словам 90,8 %.
- ONNX CPU 0,074; ONNX DML на AMD iGPU 0,171–0,182; DML-текст = torch fp32 побитово.
- Окружения и модели уже готовы: tools/parity/out/py_transcribe/{venv-cuda,venv-dml}
  (junctions на D:\hds-py-transcribe), кэши HDS_GIGAAM_CACHE/HF_HOME на D:.

ЧТО ДЕЛАТЬ (P1), в этом порядке:
A. Создать sidecar/hds_sidecar: __init__.py (пустой), stdio.py (fd1->stderr, UTF-8 stdin,
   NDJSON, reply/ok/err, serve()), runtime.py (reader-thread, idle-timeout, EOF, изоляция
   ошибки одного запроса), config.py (state_dir/config_path/load_config, yaml/hds.config
   лениво), progress.py ([stage] ... в stderr), fs.py (temp, безопасные пути, long-path).
   Перевести sidecar/hds_extract/worker.py на пакет БЕЗ изменения поведения; проверить
   golden B4 + crates/hds-extract/tests/worker_live.rs до/после.
B. Создать sidecar/hds_transcribe: worker.py (hello/transcribe/postprocess/cancel/shutdown),
   params.py (датаклассы; параметры приходят из Rust, НЕ из config.yaml), decode.py
   (ffprobe каналов, ffmpeg -> 16 кГц mono/поканально, channels auto|downmix|split),
   asr_gigaam.py (e2e_rnnt fp16/CPU, Silero VAD-чанки <=25 с, батч 8, нативные Word),
   asr_onnx.py (EP cuda|dml|cpu; SessionOptions ORT_SEQUENTIAL + enable_mem_pattern=False,
   intra_op=1 на GPU, device_id пинуется), diarize_pyannote.py (safe_globals перед загрузкой,
   waveform-массив, блоки 15–30 мин, exclusive+regular, diarization.json с кэшем),
   align.py (native; whisperx — заглушка), merge.py (чистые функции, без моделей),
   quality.py (флаги aligned/interpolated/low_confidence/failed + alignment_quality),
   cache.py, models.py, requirements.lock, requirements-dml.lock, README.md.
C. Rust: сделать Worker::request публичным + discover_python_with(env_var, venv_rel) в
   hds-extract (для транскрибации retry_dead=false); новый крейт crates/hds-transcribe
   (типы + клиент + маппинг ошибок model_missing/hf_token_missing/ffmpeg_missing/cuda_oom/
   dml_oom/onnx_error/diarization_failed/cancelled/bad_audio); переключить
   crates/hds-index/src/transcribe.rs (HTTP -> sidecar, два вызова transcribe -> postprocess,
   .md/.orig.md/.speakers.json/.bak/source_disposal); whisper_check -> transcribe-check
   (alias whisper-check); diag.rs; Cargo.toml.
D. Конфиг аддитивно: transcribe.engine python|legacy, transcribe.profile cuda|cpu|dml,
   asr.* (variant, revision, backend, batch_size, longform: off, vad: silero), align.mode,
   diarization.* (+block_minutes), decode.channels, cache.diarization, hotwords.backend none,
   llm.enabled false, request_timeout, idle_unload_sec, max_requests, fallback_cpu;
   старые ключи auto_transcribe.diarization*/index.whisper_*/diarization.* читать и маппить.
E. Тесты: protocol (общий), merge unit, golden .md, демон e2e на 1 файле, паритет извлечения
   (golden B4 + worker_live), cargo test/clippy/fmt + pytest.

КОНТРАКТЫ (не ломать): .md «### SPEAKER_NN [HH:MM:SS - HH:MM:SS]» и литерал UNASSIGNED;
.orig.md (после postprocess, до имён), .speakers.json, .bak, UI «Транскрибация», демон
hds transcribe-watch (очередь, идемпотентность, transcribe.lock, source_disposal,
transcribe.stop), out_dir не индексируется, engine: legacy работает.

ГЕЙТ P1: transcribe-once даёт .md того же формата (+ .orig.md/.speakers.json/.bak); UI
открывает и присваивает имена; демон обрабатывает файл и утилизирует исходник; извлечение
документов не изменилось; cargo test/clippy/fmt и pytest зелёные; на meet0924 WER <= 17,8 %,
атрибуция >= 90,8 %, RTF CUDA <= 0,02, VRAM <= 10 ГБ; диаризация кэшируется и режется блоками.

ГРАБЛИ (обязательно учесть):
- torchcodec 0.7.0 не работает с FFmpeg 9 => не полагаться на него: аудио через ffmpeg CLI
  в массив, pyannote получает {"waveform": tensor, "sample_rate": 16000}.
- pyannote 4.0.0 + torch 2.8: torch.serialization.add_safe_globals([...pyannote.audio.core.
  task/model...]) ДО Pipeline.from_pretrained.
- gated HF-модели: нужны принятые условия, а не только токен; проверка на старте,
  понятная ошибка hf_token_missing, в рантайме local_files_only.
- onnxruntime и onnxruntime-directml в одном venv конфликтуют; gigaam ставить --no-deps
  в DML-venv.
- DML: SEQUENTIAL + enable_mem_pattern=False + intra_op=1 + фиксированный device_id,
  логировать фактический EP/адаптер.
- CPU-профиль: освобождать чанки (на 18 мин fp32 дал RSS 15,4 ГБ).
- word_timestamps есть только в пакете gigaam из GitHub, не в HF remote-code.

ОГРАНИЧЕНИЯ: не трогать чужие незакоммиченные изменения; не переписывать боевые PLAN_*.md;
не коммитить без явной просьбы; временные артефакты — только в
tools/parity/out/py_transcribe/ (gitignored); если нужен HF-токен, эталонный корпус,
живая дискретная Radeon или решение по контракту — спроси, не выдумывай.

Начни с первых действий, затем кратко изложи план P1 по пунктам A–E и приступай к A.
```

