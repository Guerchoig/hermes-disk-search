# hermes-disk-search 0.22.0 — транскрибация на Python-sidecar (GigaAM + pyannote)

**Дата:** 23.10.2026 · **База:** 0.21.0 (`server-multiuser`)

> Переходный релиз: **python-путь включён по умолчанию** (`transcribe.engine: python`)
> и принят на CUDA/CPU, **старая цепочка (`transcribe.engine: legacy`) сохранена**
> минимум на один минорный релиз — откат переключением одного ключа, без миграции БД
> и переиндексации.

## Что нового

* **Новый движок транскрибации/диаризации** — отдельный Python-процесс
  `sidecar/hds_transcribe` (GigaAM-v3 `e2e_rnnt` + Silero VAD + pyannote
  community-1), Rust остаётся владельцем очереди, `.md`/`.orig.md`/`.speakers.json`
  и UI-контракта. Подробности — `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md`,
  `sidecar/hds_transcribe/README.md`.
* **Профили GPU**: `cuda` (NVIDIA, torch cu128) — основной; `cpu` — fallback и
  **правильный выбор для встроенной AMD/Intel** (DML там медленнее); `dml` —
  ONNX + `onnxruntime-directml` для дискретной Radeon/Intel.
* **Глоссарий и LLM-патчи**: детерминированные замены (`data/glossary.yaml`) и
  опциональная постобработка через `llm_host` (`transcribe.llm.enabled: false`
  по умолчанию); валидатор не даёт менять спикера/таймкоды/alignment, терять
  числа, отрицания и содержательные слова.
* **Диагностика**: `hds transcribe-check` (алиас `whisper-check`) и `hds check`
  показывают профиль, вендора GPU, EP, версии, наличие моделей/ONNX, токен,
  ffmpeg, а также RTF/VRAM последних заданий.
* **Установщики**: `build_transcribe_sidecar.ps1 -Profile cuda|cpu|dml`,
  `export_gigaam_onnx.ps1`, `fetch_transcribe_models.ps1` (пины ревизий, sha256),
  идемпотентные `setup.ps1` / `install_server.ps1` / `update.ps1`;
  `build_rust_release.ps1 -TranscribeProfile` кладёт **один** venv в пакет.
* **Лицензии/атрибуция**: добавлен раздел про GigaAM/pyannote/wav2vec2/Silero/
  torch/ORT в `NOTICE.md`.

## Замеры (dev-машина: RTX 3060 12 ГБ + Ryzen 7 5700G)

| Профиль | Файл | Время | RTF | Пик VRAM |
|---|---|---|---|---|
| CUDA (torch fp16, native) | meet0924, 17,9 мин | 80,2 с | 0,0503 | 6 749 МБ |
| CPU (ONNX CPU, только ASR) | frag60, 60 с | 6,6 с | 0,0948 | — |
| CPU полный (+ pyannote CPU) | frag60, 60 с | 100,6 с | 1,56 (диаризация 79,1 с) | — |
| DML (ONNX DML, iGPU Radeon) | frag60, 60 с | 21,8 с | 0,287 | — |

CPU-профиль подходит как fallback: ASR быстрый, но диаризация pyannote на 8 ядрах
идёт примерно 1,3× реального времени (18-мин запись — порядка 25–30 мин).
DML на встроенной AMD **медленнее CPU в 3×** (P0 подтверждён) — поэтому
установщик выбирает `dml` только для дискретной карты. **Профиль `dml` на дискретной
AMD/Intel заморожен как экспериментальный**: живой приёмки на такой карте нет; на
встроенной графике он не используется. Python-DML (`onnxruntime-directml` в venv)
это не затрагивает и остаётся доступным для ручного включения.

## Конфиг: новые ключи (`config.example.yaml` и `server_config.example.yaml`)

```yaml
transcribe:
  engine: python            # по умолчанию; legacy — явный откат (старые ключи работают)
  profile: auto             # auto | cuda | cpu | dml
  models_dir: ''            # '' → HDS_MODELS_DIR → installer-путь
  asr:   { variant: e2e_rnnt, backend: auto, provider: auto, onnx_dir: '' }
  align: { mode: native }
  diarization: { enabled: true, model: pyannote/speaker-diarization-community-1 }
  glossary: { path: '' }
  llm:   { enabled: false, base_url: 'http://127.0.0.1:8010/v1', model: chat }
```

Старые ключи `auto_transcribe.*` и `index.whisper_*` **не удалены**: они нужны
для `engine: legacy` и для отката. В режиме `engine: python` whisper-роль в плане
`llm_host` больше не создаётся (P6.3); предупреждения о неизвестных ключах нет.

## Миграция

1. Обновите поставку (`update.cmd` / `installers\update.ps1`): код версии
   заменяется, `config.yaml` переносится как есть.
2. Установите python-стек (файлы поставки содержат все скрипты):

   ```powershell
   powershell -File installers\build_transcribe_sidecar.ps1 -Profile cuda -SelfTest
   powershell -File installers\fetch_transcribe_models.ps1 -ModelsDir D:\models -HfToken <token>
   powershell -File installers\export_gigaam_onnx.ps1 -ModelsDir D:\models   # только dml/cpu
   ```

   На сервере те же шаги выполнит `install_server.ps1 -Fetch` (модели — в
   `%ProgramData%\hermes-disk-search\models`); `update.ps1` делает top-up
   сам (venv при отсутствии + модели), ошибки не фатальны.
3. В `config.yaml` ничего не нужно менять: `transcribe.engine: python` — значение по
   умолчанию. `profile` и `models_dir` записывает установщик; профиль можно задать
   явно (`-TranscribeProfile`).
4. Проверка: `hds transcribe-check`, затем `hds transcribe-once <файл>`.

## Откат (§12 плана)

* **Данные:** схема БД не менялась; `.md`/`.orig.md`/`.speakers.json` совместимы —
  переиндексация и миграция не нужны.
* **Код:** явно укажите `transcribe.engine: legacy` (удалять ключ бесполезно — без него
  теперь выбирается python) и
  перезапустите роль `transcribe`. Старая цепочка (`llm_host` + whisper,
  Nemotron-3) работает как в 0.21.0. Файлы python-стека можно оставить.
* **Пакет:** default-архив 0.22.0 **не содержит** `bin\DirectML.dll` и не требует
  Nemotron-модели (P6.2): legacy-диаризация Nemotron доступна только в
  transition-архиве, собранном с `build_rust_release.ps1 -LegacyDiarizer`.
  Python-профиль DirectML (`onnxruntime-directml` в venv) это не затрагивает.
* **Модели:** старые (whisper GGML, Nemotron-3) и новые (GigaAM/pyannote/wav2vec2)
  могут лежать одновременно; удалять старые до истечения переходного релиза не нужно.

## Ограничения

* Дискретная AMD/Intel для DML **заморожена как экспериментальная**: решения владельца
  от 23.10.2026 — живой Radeon для приёмки нет, профиль документирован, но не объявлен
  приёмочным; на встроенной AMD/Intel установщик выбирает `cpu`.
* `whisperx` в lock-файле закомментирован: `align.mode: whisperx|hybrid` требуют
  ручной установки пакета (иначе warning/native), явный `whisperx` — ошибка
  `align_failed`.
* venv профиля входит в архив целиком (вариант A): обновление копирует 1,5–7,4 ГБ.
* macOS/Metal — без изменений (best-effort, без живой приёмки).
