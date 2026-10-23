# hermes-disk-search 0.22.0 — транскрибация на Python-sidecar (GigaAM + pyannote)

**Дата:** 23.10.2026 · **База:** 0.21.0 (`server-multiuser`)

> Переходный релиз: **python-путь включён по умолчанию** (`transcribe.engine: python`)
> и принят на CUDA/CPU, **старая цепочка (`transcribe.engine: legacy`) сохранена**
> минимум на один минорный релиз — откат переключением одного ключа, без миграции БД
> и переиндексации.

## Что скачивать

| Файл | Нужен? | Что это |
|---|---|---|
| `hds-0.22.0-windows-x64.zip` (+ `.sha256.txt`) | **Да, обязательно** (Windows) | полная поставка (258,8 МБ): `bin\` (`hds`, `hds_mcp`, `llm_host`), установщик `setup.cmd`, обёртки `install-server.cmd` и `update.cmd`, манифесты, собранный sidecar **с обоими воркерами** (`hds_extract`, `hds_transcribe`, `hds_sidecar`, портативный CPython), `config.example.yaml`, `server_config.example.yaml`, `README.md` и `README_SERVER_MULTIUSER.md`, `client\` (пакеты + скрипты). `DirectML.dll` в default-пакете нет (P6.2) |
| `client-install.ps1` / `client-install.sh` | Для установки клиента | скрипт-установщик (токен аргументом): скачает пакет с сервера, распакует и подключит Cline |
| `hds-client-0.22.0-windows-x64.zip` (+ `.sha256.txt`) | Для клиента (если не через скрипт) | тонкий клиент: `bin\hds.exe`, мастер `setup-client.*`, правило/скилл, `version.txt` (12,8 МБ) |
| `hds-client-0.22.0-macos-arm64.zip` (+ `.sha256.txt`) | Только для macOS | то же для Apple Silicon (13,4 МБ) |
| `hds-engine-runtime-windows-x64-cuda.zip` | Только для **офлайн-установки** (NVIDIA) | рантайм движка CUDA (593,8 МБ); обычный установщик качает его сам по манифесту |
| `hds-engine-runtime-windows-x64-vulkan.zip` | Только для **офлайн-установки** (AMD/Intel) | рантайм движка Vulkan (89,9 МБ) |
| `hds-0.22.0-macos-arm64.zip` (+ `.sha256.txt`) | Только для macOS (best-effort) | полная поставка для Apple Silicon (296,1 МБ) |
| `hds-engine-runtime-macos-arm64-metal.zip` | Только для macOS офлайн | рантайм движка Metal (54,7 МБ) |
| `hermes-disk-search-v0.22.0-windows.zip` / `-macos.zip` | Нет | архивы исходников (для разработчиков) |

**В архив не входят** (установщик готовит на машине): модели поиска (LLM/CLIP/whisper),
модели расшифровки (GigaAM-v3 ~1 ГБ, wav2vec2-ru ~1,2 ГБ, pyannote community-1 +
segmentation-3.0 ~1,2 ГБ) и **venv профиля расшифровки** (1,5–7,4 ГБ) — после установки
его собирает `installers\build_transcribe_sidecar.ps1`. `ffmpeg` для расшифровки
установщик ставит сам.

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
  числа, отрицания и содержательные слова, править слова **вне отправленных
  окон** (`outside_window`) и менять порядок слов фрагмента (`word_order`).
  Обоснование замены берётся только из глоссария (контекст, `allow_global`,
  `priority: high` термина); флаг `allow_global` в ответе модели не учитывается.
* **Диагностика**: `hds transcribe-check` (алиас `whisper-check`) и `hds check`
  показывают профиль, вендора GPU, EP, версии, наличие моделей/ONNX, токен,
  ffmpeg, а также RTF/VRAM последних заданий.
* **Проверка без собранного профиля**: если venv расшифровки ещё не собран
  (свежая установка или обновление до 0.22.0), `transcribe-check` сообщает об
  этом обычной ошибкой запуска воркера: рукопожатие больше не уходит в
  бесконечный перезапуск (stack overflow движка клиента исправлен до тега).
* **Установщики**: `build_transcribe_sidecar.ps1 -Profile cuda|cpu|dml`,
  `export_gigaam_onnx.ps1`, `fetch_transcribe_models.ps1` (пины ревизий, sha256),
  идемпотентные `setup.ps1` / `install_server.ps1` / `update.ps1`;
  релизный архив **venv не содержит** — профиль (ровно один) собирается на
  машине; `build_rust_release.ps1 -TranscribeProfile` нужен только для
  локальной сборки пакета с уже готовым venv.
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
* `export_gigaam_onnx.ps1` ставит **только** вариант `rnnt` (`e2e_rnnt`): GigaAM
  экспортирует `ctc` одним `<model>.onnx`, и этот формат установщиком пока не
  проверяется.
* venv профиля (1,5–7,4 ГБ) **в архив не входит**: пакет компактный, но стек
  расшифровки после установки/обновления собирается на машине (шаг 2 «Миграции»);
  `update.ps1` делает это сам, если venv ещё нет.
* macOS/Metal — без изменений (best-effort, без живой приёмки).
