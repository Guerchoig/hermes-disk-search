# M6_HANDOFF_PROMPT.md — промт для нового чата (старт этапа Э5: GPU per-role / multi-vendor)

> ⚠️ **Устарел (10.10.2026).** Требования Э5 пересмотрены владельцем: смешанные вендоры
> (AMD+NVIDIA на одной машине) и iGPU/UMA исключены из области поддержки, сценарий
> «CUDA+Vulkan в одном `llm-host`» **снят**, план переписан (**ревизия 2**, коммит `c53e1d0`).
> Актуальный промт — **`M6_HANDOFF_PROMPT_2.md`**; план — `PLAN_M6_GPU_MULTIVENDOR.md`.


> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, правила и грабли.
> Предыдущий промт — `M5_HANDOFF_PROMPT_2.md` (M5 закрыт); код-ревью M5 — `M5_CODE_REVIEW.md`.
> Детальный план этапа — `PLAN_M6_GPU_MULTIVENDOR.md` (§1 вводные-факты, §2 границы,
> §3 контракты, §4 треки T0–T6, §5 приёмка, §7 риски, §8a журнал).
> Дорожная карта — `PLAN_SERVER_MULTIUSER.md` (§2 **W4**, §5 **Э5**, §6 риски, §8.1
> эскиз `gpu.devices`, §9 статусы); сводка — `STATUS.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние: **M1–M5 закрыты**; M5 («Хвосты M4 + внешний chat `remote`») прошёл **код-ревью
без условий** (`M5_CODE_REVIEW.md`). Владелец выбрал следующий этап — **Э5 «GPU:
per-role/instance, AMD+NVIDIA (+спайк), Apple Silicon, VRAM по картам, пресеты» (W4)**.
План написан, код не начат: **первый шаг — трек T0 (спайк), без него остальные треки не
начинаем**. Станция: Windows, RTX 3060 12 ГБ (+ AMD-карта, движку как ускоритель не видна),
Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1. Ядро на Rust (`crates/`), Python — sidecar.

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -8`
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`   # чисто
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`:
   либо остановить `hds watch` / `hds mcp-http` / `hds transcribe-watch`, либо
   `cargo test --workspace --exclude hds-cli`; после работ сервисы **вернуть**.
   Ожидание: 87 наборов тестов, 0 failed.
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`
5. Прочитать: `PLAN_M6_GPU_MULTIVENDOR.md` (целиком, короткий) → `STATUS.md` (блок M5 +
   код-ревью M5) → `M5_CODE_REVIEW.md` → `PLAN_SERVER_MULTIUSER.md` §2 W4/§5 Э5/§6/§8.1 →
   `engine-patch/README.md` (сборка движка: `-Backend cuda`) →
   `crates/hds-llama/src/{device.rs,registry.rs}` (выбор устройства и раскладка ролей).
6. Проверить факт боевого состояния (не менять!):
   `& .\target\release\llm_host.exe status` — desktop-профиль: `parallel 1`, chat LOADED
   `retention=keep` n_ctx=65536, embedding/rerank `on_demand`; **важно для T0**: в списке
   устройств видны только `index=0 backend=CUDA` (RTX 3060) и `index=1 backend=CPU` —
   **Vulkan/AMD среди ускорителей нет**;
   `Test-Path .\index.pause` (**боевая пауза — не снимать**, сейчас есть);
   `& .\target\debug\hds.exe check --json` — пункты `chat_provider`, `queue` = ok.

## Ключевые факты для T0 (из кода, проверены 10.10.2026)
* Устройство сейчас — **один глобальный** `gpu.device_index` → числовой
  `manual_devices_csv` (`device.rs::selection_from_config_index`), и он применяется ко
  **всем** ролям (`registry.rs::plan_role` ~144). Per-role выбора нет.
* Пробы VRAM уже **по-картам и вендор-нейтральные**: `bind_device_probes` (`host.rs` ~790)
  даёт `BTreeMap<bridge_device_index, Arc<dyn VramProbe>>` (NVML → все NVIDIA, затем DXGI
  `open_all()`); сопоставление — по имени/описанию устройства движка.
* Но бюджет/вытеснение считаются по **одному** целевому устройству
  (`target_bridge` из `gpu.device_index`, `target_free`) — это и есть основная работа T3.
* Патченая сборка движка — **`-Backend cuda`** (engine-patch/README.md), поэтому T0 должен
  ответить: видит ли движок Vulkan вообще, и переживает ли один процесс CUDA+Vulkan
  (риск §6 дорожной карты). Сборка Vulkan — в **копию** рантайма
  (`tools\parity\out\engine-*`), боевой каталог не трогать.
* Готовые инструменты T0: `llm-host devices`, `crates/hds-llama/src/bin/{a1_device_probe,
  llm_host_plan,vram_budget}.rs`, `tools/parity/spike6_{gpu,gpu_dia,vram,parity}.py`.
* Контракт раскладки (эскиз §8.1): `gpu.devices: [{ index, roles }]`;
  компат-ключ `gpu.device_index` обязан работать и дальше (десктоп не ломаем).

## Правила
* Сначала план по файлам (план уже есть — согласуйте уточнения), при согласовании — код.
  После каждого шага — тесты + коммит.
* Не ломать десктоп-профиль; не трогать чужие незакоммиченные изменения и боевые
  `PLAN_*.md` (обновлять `STATUS.md`, `README*.md`, журнал `PLAN_M6…` §8a; статус в
  `PLAN_SERVER_MULTIUSER.md` §9 — по завершении этапа).
* Обязательно проверять в рантайме (карты/бэкенды/VRAM/слоты — на живом сценарии), а не
  только юнитами. GPU-пробы — по `cline-rules/dev-machine.md`: останавливать и
  **возвращать** резидент, освобождать занятые `.exe`, артефакты — в `tools/parity/out/`.
* Держать хвосты: незакрытое переносить явно, а не «терять» (Apple Silicon без железа —
  заведомый хвост; замечания M5-1…M5-3 из `M5_CODE_REVIEW.md` §3 низкие).

## Что осталось от прошлых этапов (не потерять)
* **Движок: рост fit-шага** от числа загрузок в процессе (0,57 → 4,2 → 27,3 → **299 с**) —
  фикс M5 убрал перезагрузки из боевого пути, но ARB-3/`model_policy: auto`/смена модели
  снова могут дать минутные паузы → кандидат `engine-patch` P5.
* **Анонимизация (W9)** — без неё `chat.provider: remote` только для доверенных API.
* **`remote` на реальном доверенном API**; **чек-лист внешнего HTTPS** (домен/сертификат);

## Грабли окружения (важно! — проверено сессиями M4/M5)
* **`run_commands`: каждый элемент массива = ОТДЕЛЬНАЯ PowerShell-сессия** — переменные не живут
  между элементами; многошаговая проба — одним элементом через `;` или `.ps1`-файлом.
* **PowerShell иногда «съедает» первый токен** команды (видели `сgit` вместо `git`) — повторять
  команду; для диагностики проще начинать с `Write-Output 'x';` или питона.
* **Пробы с кодом — файлом** (`editor`, ASCII-only для `.ps1`; для python — `.py` в
  `tools/parity/out/`, он гитигнорится), запуск
  `powershell -NoProfile -ExecutionPolicy Bypass -File <файл>` / `python <файл>`.
* **PowerShell портит кириллицу программ (cp866-мусор)** — числа читаемы, но логи/файлы с
  кириллицей фильтровать **питоном** в файл в `tools/parity/out/` и читать read-инструментом
  (не `Select-String`).
* **`HDS_ROOT` переопределяет `project_root()`** и ломает поиск воркера (`.venv`/`sidecar`) →
  для MCP/RAG/индексации **не ставить**. Изолированный корень — junction'ы
  `.venv`/`sidecar`/`hds` + свой `config.yaml` (`tools/parity/load5.py::make_ixroot`);
  `index.pause`/`index.stop`/`index.heartbeat.json` читаются из `project_root()` — песочнице
  нужен СВОЙ корень, иначе «зависнет» на боевой паузе.
* **`HDS_CONFIG`** — сильнейший путь конфига (удобно для проб `hds check` на песочных конфигах),
  но **сессионный**: снимать (`Remove-Item Env:HDS_CONFIG,Env:HDS_ROOT`) в той же сессии.
* **Живые сервисы держат `target\debug\hds.exe`** → `cargo test --workspace` падает сборкой;
  либо `--exclude hds-cli`, либо остановить сервисы и вернуть. При работающем стенде
  (`load5.py`) НЕ пересобирать `target\debug\{hds,llm_host}.exe` и НЕ запускать
  `cargo test -p hds-llama` (собирает бины) — только `--lib`/`cargo check`.
* **Порты 8010–8012 — резидент**; песочницы: 8797 (mcp), 8799 (fake remote);
  `load5.py` требует свободный 8010 (сначала `llm_host stop`).
* **`Stop-Process -Force` = TerminateProcess** → код очистки не выполняется.
* **git — всегда `--no-pager`**; `config.yaml` в `.gitignore`; `tools/parity/out/` гитигнорится
  целиком; **править `load5.py` можно ТОЛЬКО когда ни один прогон не запущен**.
* **`hds check --json`** → `{"checks":[{id,status,title,msg,fix}],"ok":bool}` — фильтровать
  `Where-Object { $_.id -eq '<id>' }`; id текущих пунктов: `profile`,`db`,`queue`,
  `chat_provider` (планируемый новый — `gpu_roles`).
* **После server-профильных проб чистить `%ProgramData%\hermes-disk-search`**
  (`access.yaml` с тестовыми токенами, снапшот очереди, `data/logs/audit.jsonl`) —
  `load5.py` это делает сам, ручные пробы — вручную.
* **Боевой каталог движка — не трогать**: сборки/копии только в `tools\parity\out\engine-*`,
  резидент с `--engine-dir <копия>`; откат патча — `installers\fetch_engine_runtime.ps1
  -RollbackEnginePatch`.
* **Урок сессий M5/M6:** в начале работы в дереве могут быть «неожиданные» незакоммиченные
  изменения — это работа предыдущей сессии того же этапа. Не откатывать: сверить дифф и
  доращивать. Один активный чат на этап.

### Остановить / вернуть боевые сервисы (проверено)
```powershell
# остановить (освободить target\debug\hds.exe для сборки и 8010-8012 для песочниц)
Get-CimInstance Win32_Process -Filter "Name='hds.exe'" | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
& .\target\release\llm_host.exe stop
# вернуть (порядок: резидент → watch → mcp-http → transcribe-watch)
Start-Process -FilePath '.\target\release\llm_host.exe' -ArgumentList 'run' -WindowStyle Hidden
Start-Sleep 20
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'watch' -WindowStyle Hidden
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'mcp','--http','--host','127.0.0.1','--port','8787','--path','/mcp' -WindowStyle Hidden
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'transcribe-watch' -WindowStyle Hidden
# проверить: status → parallel 1; Test-Path .\index.pause → True; http://127.0.0.1:8787/health → 200
```

### Стенд (по нужде; требует свободных 8010–8012)
```powershell
python tools\parity\load5.py --tag p5_16384        --parallel 5 --ctx 16384    # базовый SLA
python tools\parity\load5.py --tag remote1         --parallel 5 --ctx 16384 --chat-remote fake --remote-latency 3
# для Э5: раскладка по картам — сначала через песочный llm-конфиг load5.py (profile server)
```
* Пишет `tools/parity/out/m4_load_<tag>.json`; сам удаляет системный state-каталог, если создал.
* Прогон ~1–6 мин → фоном: `Start-Process python -ArgumentList ... -RedirectStandardOutput <лог>`
  и поллить лог (`STEP WROTE ...`, `statuses=...`).

## Правила (повтор, коротко)
* Сначала план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
* Не ломать десктоп-профиль; не переписывать боевые `PLAN_*.md`.
* Рантайм-проверка обязательна (карты/бэкенды/VRAM/слоты/очередь — на живом сценарии).
* Хвосты переносить явно (раздел «Что осталось»), а не «терять».
