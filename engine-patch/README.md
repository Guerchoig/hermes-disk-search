# Патч движка openresearchtools/engine v1.15 (наш, временный)

> **Что это.** Мы несём собственный патч движка, пока владелец не починит его у себя.
> Причина — инцидент `tools/parity/W4_REPORT.md` §14: зависший вызов движка держал
> мьютекс инстанса и парализовал всё (`status`, вытеснение, роли), а `unload` не мог
> освободить VRAM. Разбор дефектов и апстрим-отчёт — `UPSTREAM_REPORT.md` (написан;
> публикация — по решению заказчика).
>
> **База:** тег **`v1.15`** (`2683eb6`) — ровно та сборка, что стоит у заказчика
> (`runtime-manifests/engine-manifest.json`). Не `main`/`v1.17`: код там тот же, но
> партией поставляется v1.15.
>
> **Файл патча:** `hds-engine-patch-v1.15.patch` (4 файла, 24 хунка, ~27 КБ).

## Что меняет патч (по классам дефектов)

| # | Дефект движка | Правка |
|---|---|---|
| P1 | `wait_for_instance_slot_locked` ждал слот **вечно** (`cv.wait` без дедлайна — в движке нет ни одного `wait_for`) | `cv.wait_until(deadline)` (600 с), таймаут виден в `instance.last_error` → попадает в `list_instances`/наш `status` |
| P2 | Загрузка модели шла **под `instance->mutex`** → на всё время загрузки (секунды) блокировались `list_instances`, `unload_instance` и все роли | Загрузка вынесена в `create_bridge_detached()` (читает только `params`), сериализована отдельным `load_mutex`, а `ensure_instance_loaded()` сам берёт/отпускает request-лок (короткие критические секции + `cv.notify_all`) |
| P3 | `set_cluster_error` (cluster-лок) вызывался под instance-локом → ABBA против `remove_instance` (cluster→instance) | Лок отпускается до `set_cluster_error` в `unload_instance`, `set_instance_retention_mode` и в guard диаризации `audio_transcriptions_raw`; в путях запросов ни загрузка, ни ошибка не под локом |
| KV | В cluster/bridge API **не было полей под тип KV** → `--cache-type-k/v` игнорировались, KV всегда f16 (512 МиБ у чата) | +2 поля (`cache_type_k/v`) в `llama_server_cluster_instance_params` и `llama_server_bridge_params`; присвоение `bridge->params.cache_type_k/v`; при квантованном V включается Flash Attention (llama.cpp требует FA для `type_v != F16`) |

**Контракт API.** Два поля добавлены **в конец** структур, поэтому по указателю
(`*_create_instance`, `*_create`) стоковая DLL просто не читает их — наша Rust-сторона
совместима с обоими вариантами. `*_default_instance_params()` возвращает структуру
**по значению**: с непатченой DLL хвост остаётся неинициализированным, поэтому в
`InstanceSpec::build` тип KV пишется **всегда явно** (0 = «не задан» → мост берёт F16).

## Как воспроизвести сборку (Windows, CUDA)

```powershell
git clone --branch v1.15 --depth 1 https://github.com/openresearchtools/engine C:\Users\Sasha\engine-1.15
# патч применяется к <repo>\bridge — это «patchable layer» движка (bridge/README.md),
# staging сам копирует его в MARKDOWN\bridge
git -C C:\Users\Sasha\engine-1.15 apply <путь>\hds-engine-patch-v1.15.patch
cd C:\Users\Sasha\engine-1.15
.\build\prepare_llama_source_from_patch.ps1 -OutDir C:\Users\Sasha\ENGINEbuilds
# vcvarsall x64 + Ninja Multi-Config (как CI движка!): с VS-генератором VS-интеграция
# CUDA не находит тулчейн — `The CUDA Toolkit directory '' does not exist`
cmd /c '"C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul && set' | ForEach-Object { if ($_ -match '^(.*?)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') } }
.\build\build_bridge.ps1 -CmakeGenerator 'Ninja Multi-Config' -Backend cuda -Config Release `
  -LlamaCppDir C:\Users\Sasha\ENGINEbuilds -WhisperCppDir C:\Users\Sasha\whisper.cpp `
  -BuildRoot C:\Users\Sasha\ENGINEbuilds -BuildDir C:\Users\Sasha\ENGINEbuilds\build-bridge-cuda `
  -EnableBackendDl $true -DisableGgmlNative $true
```

Требуется: CMake 4.3+ (в VS 18 идёт в комплекте), MSVC C++ (VS 18 Community), CUDA Toolkit
13.4 (`nvcc`), `whisper.cpp` рядом (для аудио-моста). Полная поставка (как ассет) —
через `build_full_stack_cuda.ps1` с `-EnableCpuAllVariants`/`-StageCudaRuntime`, как в CI.

## Результаты живой проверки (02.10.2026, RTX 3060 12 ГБ)

Собран нашим патчем и проверен на **копии** рантайма (`C:\Users\Sasha\engine-patched`),
боевой каталог движка не тронут; резидент запущен с `--engine-dir <копия>`.

| Что | Было (сток v1.15) | Стало (патч) |
|---|---|---|
| Размер DLL | `llama-server-bridge.dll` 5,16 МБ, `multi-node-server.dll` 0,28 МБ | 5,18 / 0,28 МБ (сборка воспроизводит апстрим) |
| Экспорты `multi-node-server.dll` | 39 | 39 (**ABI не менялся**) |
| KV чата (n_ctx 16384, 8/32 слоёв) | f16 = 512 МиБ | **q8_0 = 272 МиБ** |
| VRAM с загруженным чатом | 7905 МиБ | **7704 МиБ** (−201 МиБ; разница с −240 — Flash Attention) |
| compute-буфер embedding (legacy `--ubatch-size 8192`) | ~1,4 ГиБ (бюджет этого не видел) | **90 МиБ** при `n_ubatch 512` (конфиг исправлен) |
| Чат | — | `POST /v1/chat/completions` → `"content":"ok"`, 2 токена ✔ |
| Embeddings | — | `POST /v1/embeddings` → вектор 1024 ✔ |

Наш бюджет теперь печатает честную оценку (лог резидента):
`роль chat: модель 7112 МиБ, KV q8_0 272 МиБ (KV-слоёв 8 из 32), compute-буфер 480 МиБ (n_ubatch 512), n_batch 512, нужно 8220 МиБ`.

### Грабля сборки (стоила времени)

`build_bridge.ps1` **копирует исходники моста с сохранением mtime** (`bridge/README.md` —
«patchable layer»), поэтому после правки патча `<BuildDir>` может решить, что объекты
свежее источников, и **не пересобрать** (`ninja: no work to do`, в DLL остаётся старый код).
Проверять: маркерная строка в DLL (`slot wait timeout`) или
`Get-ChildItem <BuildDir> -Recurse -Filter '*cluster*.obj'` по времени. Лечение: удалить
объекты моста (`*cluster*.obj`, `*bridge*.obj`) перед сборкой — их проще, чем mtime.

## Установлено на этой машине (02.10.2026)

Патч внедрён в **боевой** каталог движка (`%APPDATA%\OpenResearchTools\TranscribeOffline\Engine`):
наши `llama-server-bridge.dll`, `multi-node-server.dll`, `llama-server-audio.dll`, `llama.dll`,
`mtmd.dll`, `ggml*.dll` скопированы поверх, штатные — сохранены рядом как `*.orig` (9 файлов).
Штатный резидент (`target\release\llm_host.exe run`, без `--engine-dir`) их использует —
лог подтверждает: `движок: …\TranscribeOffline\Engine`, `роль chat: KV q8_0 272 МиБ`.
**Откат одной командой:** `installers\fetch_engine_runtime.ps1 -RollbackEnginePatch` (вернёт `*.orig`) —
или перезалить стоковый рантайм по `runtime-manifests/engine-manifest.json`; наша Rust-сторона
совместима со стоком.

**Итерация 2 (02.10.2026).** В боевой каталог перезалиты `llama-server-bridge.dll` (`79d6ec73…`) и
`multi-node-server.dll` (`7593b176…`) из пересборки с закрытым хвостом P3 (остальные DLL не менялись).
Резидент останавливался на подмену и поднят заново; чат проверен. Патч **опубликован** — тег
`engine-patch-v1` (10 ассетов), поэтому `fetch_engine_runtime.ps1 -PatchEngine` работает и на других
машинах.

## Итерация 3 (04.10.2026): мост с поддержкой FFmpeg — тег `engine-patch-v2`

Инцидент: автотранскрибация `.mp4` падала `501 not_implemented` — «bridge was built
without FFmpeg support (LLAMA_SERVER_BRIDGE_ENABLE_FFMPEG=OFF)». Мост в теге
`engine-patch-v1` был собран без флага `-EnableFfmpeg`, декодирование контейнеров
(mp4/mkv/…) не вкомпилировано; `.wav` работал, потому что декодируется нативно.

* Пересобран мост по рецепту выше **с `-EnableFfmpeg`** (ffmpeg 8.1 LGPL-shared
  из `build/download-ffmpeg-lgpl-win-x64.ps1` → `ENGINEbuilds\runtime-deps\ffmpeg`;
  soname `avcodec-62` совпадает с вендорным `Engine\vendor\ffmpeg\bin`).
* Изменились только `llama-server-bridge.dll` (5 439 488 байт, `8c2ec8f4…`) и
  `multi-node-server.dll` (перелинковка, `1cbbcadd…`); остальные 8 DLL побайтово
  совпали с v1.
* Опубликовано тегом **`engine-patch-v2`** (`runtime-manifests/engine-patch.json`
  обновлён: тег/URL/sha256); `fetch_engine_runtime.ps1 -PatchEngine` на свежих
  установках ставит именно его. На машинах с v1 — повторно запустить `-PatchEngine`
  и перезапустить `llm_host`.
* Проверено на боевой машине: `.mp4` (~1 ч, 309 МБ) прошёл полный конвейер
  декодирование → whisper (CUDA0) → диаризация sortformer → `.md` со спикерами.

## Побочный эффект: стоковый `example-cli.exe` несовместим с патчеными DLL (T0.1, 04.10.2026)

Патч меняет **раскладки SDK-структур** (`llama_server_bridge_params` и cluster-структуры —
добавлены поля `cache_type_k/v` и др.). Штатный `example-cli.exe` из каталога движка собран
против **стоковых** заголовков, поэтому с нашими DLL аудио-путь падает:

| Рантайм в каталоге | `example-cli.exe bridge audio …` |
|---|---|
| наши патченые DLL | `0xC0000005` (access violation; в журнале Windows — сбой в `ucrtbase.dll`) |
| стоковые `*.orig` (копия каталога) | `0xC06D007E` — у копии не оказалось `vendor\ffmpeg\*` |
| **наши бинарники** (`hds`, `llm_host`, `diar_probe`/`audio_probe`) | ✅ работают (собраны против патченых заголовков) |

Практический вывод: **спайки и проверки аудио/диаризации гонять нашими пробами**, а не
`example-cli`: `cargo run -p hds-llama --release --bin diar_probe -- <audio>` (или `audio_probe`).
Спайки 5–6 (`spike5_whisper.py`, `spike6_*`) делались **до** внедрения патча — поэтому у них
`example-cli.exe` работал; после `-RollbackEnginePatch` CLI снова заработает.

Мелочь для копий рантайма: `vendor\` в боевом каталоге **штатный** — копируя движок
(`tools\parity\out\engine-*`), копировать дерево целиком, иначе ffmpeg-DLL не найдутся
(`robocopy <Engine> <dst> /E /XF *.orig` — с `/E`; без него подкаталоги пропускаются).

## Как проверить (на копии рантайма! боевой не трогаем)

1. Скопировать каталог движка (`%APPDATA%\OpenResearchTools\TranscribeOffline\Engine`) в
   `tools\parity\out\engine-patched` и положить туда собранные
   `llama-server-bridge.dll` + `multi-node-server.dll` (+ `llama-server-audio.dll`).
2. `HDS_ENGINE_DIR=<копия>` (или `--engine-dir`) → прогоны: chat, embeddings, rerank,
   `whisper-check` + `/internal/transcribe`, CLIP, `tools\parity\arb_scenarios.py`.
3. KV: `cargo run -p hds-llama --release --bin kv_probe -- --role chat --ngl 99 --n-ctx 16384`
   — ожидаем KV ≈ 256 МиБ вместо 512 (наш бюджет это уже учитывает, `KvBits::Q8_0`).
4. Репро исходного дефекта: `load_instance` для embedding (с `n_batch/n_ubatch` 8192) при
   загруженном чате — до патча залипание, после ожидаем ошибку/таймаут вместо паралича.

## Откат

* Вернуть стоковые DLL (из манифеста `engine-manifest.json`) — наша Rust-сторона
  совместима со стоком (см. «Контракт API»).
* Либо `git -C C:\Users\Sasha\engine-1.15 checkout -- bridge` и пересобрать.
* Никаких изменений в конфиге заказчика патч не требует: `--cache-type-k/v` заработают
  только с патченой DLL, а без неё просто не дадут эффекта (предупреждения больше нет).

## Хвосты путей запросов — уточнено и закрыто (итерация 2, 02.10.2026)

**Уточнение (перепроверено по коду, а не по памяти).** В **хвостах** пяти путей запросов
(`chat_complete`, `vlm_complete`, `embeddings`, `rerank`, `audio_transcriptions_raw`)
`set_cluster_error` вызывается **вне** instance-лока: `finish_request_locked(*instance);
lock.unlock();` стоят строкой выше — и это было так уже в стоке v1.15 (сверено с
`git show <tag>v1.15:bridge/llama_server_cluster.cpp`). Прежняя формулировка «в хвостах пяти
путей `set_cluster_error` под instance-локом» относилась к тому, что эти вызовы происходят
после снятия лока; ошибка была в прочтении, а не в коде.

**Единственное реальное место** P3-класса, где `set_cluster_error` исполнялся под
`instance->mutex`, — guard `enable_diarization` в `audio_transcriptions_raw` (ветка, где аудио
идёт **не** нативным бэкендом): лок берётся в начале функции и в этой ветке не снимался.
Итерация 2 патча отпускает лок до `set_cluster_error`. **ABBA-класс P3 закрыт полностью** —
под instance-локом не остаётся ни одного вызова `set_cluster_error`.
