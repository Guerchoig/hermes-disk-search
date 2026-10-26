# hermes-disk-search 0.23.1 — исправления установки стека анонимизации

**Дата:** 26.10.2026 · **База:** 0.23.0

> PATCH-релиз: исправления, найденные при развёртывании 0.23.0 на серверном стенде
> (Windows, RTX 3060). Схема базы поиска и API не менялись — переиндексация не нужна.

## Что исправлено

1. **Установщик моделей не мог скачать navec/slovnet (HTTP 404).** В 0.23.0 стояли
   ссылки GitHub Releases, которых не существует; заменены на канонические паки
   `storage.yandexcloud.net/natasha-navec|natasha-slovnet/packs/…tar`
   (те же имена, что ждёт воркер).
2. **Неполный HF-снимок GLiNER.** Загрузка 0.23.0 обрывалась на двух больших LFS-файлах
   (`onnx/model.onnx`, `pytorch_model.bin`), а GLiNER отказывается загружать неполный
   снимок (итог — `model_missing`). Теперь: до трёх попыток с докачкой `.incomplete` и
   **проверка полноты** по дереву репозитория — отсутствующие файлы перечисляются в
   предупреждениях и манифесте.
3. **Квантованный ONNX не находился.** `knowledgator/gliner-pii-large-v1.0` публикует
   `onnx/model_quint8.onnx`; движок искал только `onnx/model.onnx`/`model.onnx` и уходил
   в torch-фоллбек без весов. Добавлен кандидат `onnx/model_quint8.onnx`; проверки
   `hds anon check` и пункт `anonymize` в `hds check` принимают любой `*.onnx`.
4. **CUDA-провайдер ONNX Runtime молча работал на CPU** (`cublasLt64_12.dll is missing`).
   Профиль `cuda` теперь ставит `nvidia-cublas-cu12`, `nvidia-cuda-runtime-cu12`,
   `nvidia-cudnn-cu12` — ORT находит CUDA 12 DLL из `site-packages\nvidia` без системного
   CUDA Toolkit.
5. **`README_ANONYMIZATION.md` не попадал в архив приложения** (заметки 0.23.0 это
   утверждали). `installers/build_rust_release.ps1` копирует файл и требует его наличие;
   тело опубликованного релиза 0.23.0 поправлено.

## Обновление

* Обычное: `update.cmd` (Windows) / `update.command` (macOS). Службы и настройки не
  затрагиваются.
* Если стек анонимизации ставился из 0.23.0: пересоберите окружение
  `installers\build_anonymize_sidecar.ps1 -Profile cuda -Force` (или доставьте три
  `nvidia-*-cu12` пакета в `sidecar\venv-anonymize`) и повторите
  `installers\fetch_anonymize_models.ps1` — он докачает недостающие файлы снимка.

## Проверено

* Развёртывание на серверном стенде (Windows Server-профиль, RTX 3060): `hds check` —
  `ok: true` (критичных нет), пункт `anonymize` зелёный; `hds anon warmup` —
  GLiNER ONNX + Natasha/Slovnet; реальный NER-акт: lease `anon` (лимит ORT 10 442 МиБ),
  роли chat/embedding/rerank выгружены и вернулись, результат — метки
  `[ORG_1]`, `[PERSON_1]`, `[PHONE_1]`, `[EMAIL_1]`; после установки CUDA-пакетов
  предупреждения `cublasLt64_12.dll is missing` нет.
* `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`,
  Python-набор sidecar — зелёные.

## Ассеты

Тот же состав, что у 0.23.0 (15 файлов): `hds-0.23.1-windows-x64.zip` (+ `.sha256.txt`) —
**обязательно**; `hds-0.23.1-macos-arm64.zip`, клиентские пакеты и `client-install.*`,
рантаймы движка CUDA/Vulkan/Metal — по потребности; архивы исходников — для разработчиков.
Стек анонимизации (venv + модели) в архив не входит и ставится установщиками на машине.
