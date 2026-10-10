# M6_HANDOFF_PROMPT_4.md — промт для нового чата (этап Э5: T0/T1/T2/T3 закрыты, следующий шаг — T4)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, факты треков с числами, контракты, правила и грабли окружения.
>
> **Заменяет `M6_HANDOFF_PROMPT_3.md`** (там этап в точке «T0 GO + T1, следующий — T2»).
> Сейчас: **T0 закрыт (GO), T1, T2 и T3 выполнены**; код T4–T6 не начат.
> План этапа — `PLAN_M6_GPU_MULTIVENDOR.md` (**ревизия 2**, журнал §8a — по всем трекам);
> сводка — `STATUS.md` (блок M6); дорожная карта — `PLAN_SERVER_MULTIUSER.md` §2 **W4** /
> §5 **Э5**; спайки — `tools/parity/SPIKES.md` (§15 — отчёт T0).

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние на момент передачи: **HEAD `32b3de6`**, дерево чистое; боевой стенд поднят
(резидент `llm_host`, `watch`/`mcp-http`/`transcribe-watch`, `index.pause` — есть).
Этап **M6/Э5** в работе, закрыто треками (коммиты в порядке появления):

| Трек | Коммит | Итог |
|---|---|---|
| ревизия требований (владелец, 10.10.2026) | `c53e1d0` | смешанные вендоры и iGPU/UMA — **вне поддержки**; прод = N дискретных карт **одного семейства** (NVIDIA / AMD под Windows) либо один Mac/Metal; поставка отдельными сборками; чат-реплики одинаковые; индексация — ровно один экземпляр; транскрибация — экземпляр на карту |
| T0 (спайк) | `39980b7`, `d635d03` | **GO**: Vulkan-бэкенд собираем **из нашего дерева**; чужие `ggml-vulkan.dll` несовместимы (b11528 `api_version=3`; b11200 крашит процесс — апстрим сменил layout `ggml_backend_buffer_i` без бампа версии); на iGPU-свидетельстве роль идёт на Vulkan-устройство |
| T1 (конфиг-контракт) | `a919a78` | `gpu.devices: [{index, roles}]` + `llm_server.<role>.device` (приоритетнее), компат `gpu.device_index`; fail-fast валидация с числами (`LlmHostConfig.errors`); `backend=` в плане/статусе; пункт `gpu_roles` в `hds check`; образец в `config.example.yaml` |
| T2 (раскладка/реплики) | `7469d72` | роли-реплики `chat-N`/`whisper-N`; `cross_validate` (§3.5: whisper ≤1 на карту, индексация — один экземпляр, дубли портов); «роль → карта/backend» в статусе; **один фасад** + least-busy/round-robin внутри хоста; whisper по картам |
| верификация T1 (второй чат) | `0e07bf7` | независимые пробы `gpu_roles` (роль по `llm_server.<role>.device` → ok; индекс вне диапазона → warn с числами; компат → ok); 88 наборов / 0 failed |
| **T3 (бюджет по картам)** | `f14b226`, `32b3de6` | **бюджет и вытеснение в пределах карты**: `role_cards` (роль → bridge-индекс из плана), `free_for_role` (проба ↔ числа движка по карте роли), карт-скоуп вытеснения и предохранителя простоя, `cards` в `status`/`/internal/status` |

Станция: Windows, RTX 3060 12 ГБ (единственный дискретный GPU), AMD Radeon Graphics
(**iGPU, UMA** — в прод-область не входит), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1.
Ядро на Rust (`crates/`), Python — только sidecar.

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -12`  # HEAD — этот промт
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`     # чисто
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`:
   либо остановить `hds watch`/`hds mcp-http`/`hds transcribe-watch`, либо
   `cargo test --workspace --exclude hds-cli`; после работ сервисы **вернуть**.
   Ожидание: **88 наборов, 0 failed**.
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
5. Прочитать: `PLAN_M6_GPU_MULTIVENDOR.md` целиком (ревизия 2; §3.4 — пресеты для T4,
   §4 — треки, §5 — приёмка, §8a — журнал) → `STATUS.md` (блок M6) → `tools/parity/SPIKES.md` §15 (T0).
6. Проверить боевое состояние (не менять!): `& .\target\release\llm_host.exe status`
   (desktop-профиль: `parallel 1`, chat LOADED `retention=keep`, роли печатают
   `card=0 backend=CUDA`, ниже — строки «карта 0 (CUDA, NVIDIA GeForce RTX 3060): свободно … МиБ,
   роли: chat, embedding, rerank»); `Test-Path .\index.pause` (**True — не снимать**);
   `& .\target\debug\hds.exe check --json` — пункты `gpu_roles`, `chat_provider`, `queue` = ok.

## Факты по трекам (проверено кодом и рантаймом)

### T3 — бюджет и вытеснение по картам (сделано 10.10.2026, `f14b226`; журнал `32b3de6`)
* `crates/hds-llama/src/dispatch.rs`: `InstanceUse.card: Option<i32>` (+ билдер `on_card(i32)`),
  публичные хелперы **`on_card(&[InstanceUse], Option<i32>)`** (скоуп карты: `Some(c)` — только
  инстансы этой карты, `None` — все, поведение до M6) и **`cards_in_use(&[InstanceUse])`**
  (отсортированные карты с инстансами).
* `crates/hds-llama/src/host.rs`: поле `ClusterBackend.role_cards: BTreeMap<String, i32>`
  (из поднятого плана: `planned[].bridge_device_index`); строка старта «бюджет и вытеснение
  по картам ролей: chat → карта 0, …»; методы `free_for_role(role)` (проба по bridge-индексу
  ↔ числа движка; роль без раскладки → прежний `target_free_mib`), `uses_for_role(role, uses)`;
  `prepare_demand` строит план по карте роли (+ note «карта роли 'chat' — 0: бюджет и вытеснение
  в её пределах (M6 §3.3)»), сообщение об отказе берёт числа её карты; `arbiter_tick` —
  предохранитель простоя **по каждой карте** отдельно и индексация на карте роли `embedding`;
  публичная **`cards_lines(devices, role_cards, free_fn)`** → строки «карта N (backend, устройство):
  свободно X МиБ, роли: …» в `llm_host status` (`LocalStatus.cards_line`) и в `/internal/status`
  (поле `cards` + в `lines`).
* Тесты: `tests/arbiter.rs` +3 (`eviction_and_budget_are_scoped_to_card`,
  `cards_in_use_lists_distinct_sorted_cards`, `idle_eviction_on_one_card_does_not_touch_other_card`),
  `src/host.rs` +1 (`cards_lines_show_per_card_roles_and_free`).
* Рантайм (пересобранный release, боевой desktop-конфиг): `/internal/status` →
  `cards: ["карта 0 (CUDA, NVIDIA GeForce RTX 3060): свободно 3269 МиБ, роли: chat, embedding, rerank"]`;
  `POST :8010/v1/chat/completions` → 200; бюджет не поехал (chat «нужно 9036 МиБ»).
  Артефакты: `tools/parity/out/m6_t3_runtime.json`, скрипт `tools/parity/out/m6_t3_runtime.py`
  (переиспользуемый: лог карт + `cards` + живой запрос).
* **Ограничение (хвост):** двухкарточный сценарий живьём не проверяется — железа нет
  (мультикарточность принимается юнитами + хвостом «машина с N картами»). На боевом конфиге
  диспетчер выключен, поэтому note «карта роли» в логе не появится — он проверен юнитами.

### Карта кода (что где трогать в T4–T6)
* Парсинг/валидация раскладки: `crates/hds-llama/src/config.rs` — `pub fn build` (~409),
  `LlmHostConfig.errors` (~334, fail-fast §3.5), тест `gpu_devices_layout_parsed_with_warnings` (~1287);
  helpers `role_base`/`replica_index`/`is_replica` (~46–74), `replicas_of` (~346).
* Раскладка в план ролей: `crates/hds-llama/src/registry.rs` — `PlannedInstance{bridge_device_index,
  device_backend}`, `plan`, `cross_validate`, `summary()` (печатает `card=`/`backend=`).
* `hds check`: `crates/hds-index/src/diag.rs` — пункт **`gpu_roles`** (`check_gpu_roles` ~1148,
  `check_gpu_roles_live` ~1189, тесты статусов ~1520); рядом `check_chat_provider` ~1007,
  `check_gpu_observability` ~620, `check_queue` ~872; список пунктов — `run_checks` ~1352.
* Статус ролей: `crates/hds-llama/src/status.rs` (строка с `card=`/`backend=` ~70, JSON ~380).
* Образец конфига: `config.example.yaml` — блок `gpu:` ~159–205 (раскладка, реплики,
  транскрибация по картам уже прокомментированы в T1/T2; T4 добавляет пресеты).
* Стенд/пробы: `tools/parity/load5.py` (SLA-стенд, требует свободных 8010–8012),
  `tools/parity/out/m6_*` (артефакты T0–T3, гитигнор), `crates/hds-llama/src/bin/{llm_host_plan,
  a7_multiinstance_probe,vram_budget}.rs`.

## Что делать дальше (T4 → T5 → T6, план §4)

### T4. Пресеты и автодетект — 1 дн (следующий шаг)
- Пресеты **вендор-гомогенные** (§3.4): `NVIDIA: одна карта`; `NVIDIA: N карт`; `AMD: N карт`;
  `Apple Silicon`. Смысл: чат-реплика на каждую карту (или `n_parallel`/вторая реплика на одной
  карте), `embedding`/`rerank` — одним экземпляром на выделенной карте, `whisper` — на каждой.
- Предлагаемый контракт (уточнить и зафиксировать в §3.4/`config.example.yaml`): ключ
  `gpu.preset: none|nvidia-one|nvidia-n|amd-n|apple` (+ ручная раскладка `gpu.devices` сильнее)
  и `gpu.autodetect: false` по умолчанию. **Любая** раскладка печатается с числами и требует
  явного включения; секретов не печатаем.
- `hds check` → подсказка в пункте `gpu_roles` (что предложить на этой машине: сколько
  ускорителей/бэкендов нашлось, какой пресет уместен, что смешанные вендоры/UMA не
  поддерживаются — warn уже есть).
- Тесты: юнит на резолв пресета (раскладка из пресета + override `gpu.devices`), на
  `autodetect: false` (ничего не меняется), на warn/подсказку в `diag`.
- Валидация — через `LlmHostConfig.errors` (fail-fast с числами, как T1/T2).

### T5. Рантайм-приёмка — 1–1,5 дн (приоритет владельца — **транскрибация**)
- Транскрибация: живой файл (`.wav`/`.mp4`) через `hds transcribe` / `auto_transcribe` /
  `POST :8010/internal/transcribe`; в отчёте — карта, латентность, пик VRAM, отсутствие
  паралича резидента; **≥3 прогона**, в т.ч. при загруженном чате.
- Раскладка (если применяется): свидетельство **по каждой карте** — `llm_host status`/`devices`,
  `cards` в `/internal/status`, VRAM/латентности; реплики чата отвечают одновременно.
- Десктоп-профиль: `hds check` ok, поведение не изменено, `index.pause` не снимается.
- Отчёты — `tools/parity/out/m6_*_*.json` (гитигнор).

### T6. Доклад — 0,5 дн
- `STATUS.md` (блок M6), `README.md` («Несколько карт и вендоров», пресеты, честные
  ограничения: Apple Silicon без живой проверки; мультикарточность — юниты + хвост),
  `README_SERVER_MULTIUSER.md` (сценарий «N карт одного семейства»), `config.example.yaml`,
  журнал §8a, `PLAN_SERVER_MULTIUSER.md` §9 — по завершении этапа; при необходимости —
  заметка в `engine-patch/README.md` (сборка под Vulkan + Vulkan SDK).

## Требования (ревизия 2 — решения владельца 10.10.2026, без изменений)
1. Прод — несколько **дискретных** GPU **одного семейства**: NVIDIA под Windows **или** AMD
   под Windows; либо **один Mac с Metal**. Смешанные вендоры и iGPU/UMA — **не поддерживаются**
   (пункт `gpu_roles` уже warn'ит).
2. Поставка — **отдельными сборками**: NVIDIA / AMD / Metal.
3. Несколько карт — обслуживать разных пользователей одновременно; **чат-реплики одинаковые**
   (одна модель/контекст; разные порты/слоты), допустимы на одной карте и на разных.
4. **Индексация (`embedding`+`rerank`) — ровно один экземпляр на машину** (`n_parallel = 1`).
5. **Транскрибация (`whisper`) — экземпляр на карту**; один файл — одна карта (split отвергнут);
   карту под файл выбирает llm-host по свободной VRAM (числа в логе).
6. **Один фасад** (`chat` → 8010) + внутренняя маршрутизация least-busy → round-robin; порты
   реплик — внутренние/справочные (авторизация/аудит/лимиты/очередь — в одной точке).
7. Индексация — «лёгкая добавка» рядом с чат-репликой; при нехватке VRAM — пауза (`index.pause`),
   чат-реплики не вытесняются.
8. Железа для нескольких карт нет → мультикарточность принимается юнитами + **явным хвостом**;
   дискретной AMD нет (iGPU-свидетельство из T0); Apple Silicon — без живой проверки.
9. Стенд: приоритет владельца — **транскрибация** (приёмка T5 вокруг неё).

## Правила
* Сначала план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
* Не ломать десктоп-профиль; не трогать чужие незакоммиченные изменения и боевые `PLAN_*.md`
  (обновлять `STATUS.md`, `README*.md`, журнал `PLAN_M6` §8a; статус в
  `PLAN_SERVER_MULTIUSER.md` §9 — по завершении этапа).
* Рантайм-проверка обязательна (карты/бэкенды/VRAM/слоты/транскрибация — на живом сценарии).
* GPU-пробы — по `cline-rules/dev-machine.md`: останавливать и **возвращать** резидент,
  освобождать занятые `.exe`, артефакты — в `tools/parity/out/`.
* Хвосты переносить явно (раздел ниже), а не «терять».
* Один активный чат на этап (в M6 уже была коллизия двух чатов — сверяйте `git log` перед работой).

## Грабли окружения (важно! — проверено сессиями M5/M6)
* **PowerShell часто «съедает» первый токен команды** (`git` → `сgit`, `python` → `сpython`,
  `Write-Output` → `сWrite-Output`). Команда при этом НЕ выполняется или выполняется частично
  (видели: запуск резидента «пропал» — на деле команда не выполнилась). Лечение: начинать
  элемент с безобидного `Write-Output 'x';` или повторять команду; для диагностики — питон.
* **`run_commands`: каждый элемент массива = отдельная PowerShell-сессия** — переменные между
  элементами не живут; многошаговая проба — одним элементом через `;` или `.ps1`-файлом
  (ASCII-only; для python — `.py` в `tools/parity/out/`, он гитигнорится).
* **`read_files` по диапазону строк иногда отдаёт «outdated»** для недавно изменённых файлов —
  обход: питоном выгрузить нужные строки в `tools/parity/out/*.txt` и прочитать файл целиком
  (так разбирался T3: `m6_t3_notes.txt`, `m6_t3_code.txt`, `m6_t3_host_regions.txt`).
* **PowerShell портит кириллицу программ (cp866-мусор)** — числа читаемы, логи фильтровать
  питоном в файл и читать read-инструментом, не `Select-String`.
* **Резидент держит `target\release\llm_host.exe`** → пересборка release требует
  `llm_host stop` (или `stop --force` по pid-файлу); **живые `hds.exe` держат
  `target\debug\hds.exe`** → `cargo test --workspace` нужно либо с `--exclude hds-cli`,
  либо с остановкой `watch`/`mcp-http`/`transcribe-watch` (и вернуть их после).
  `cargo test -p hds-llama` собирает и бины — не запускать при работающем стенде `load5.py`.
* **`llm_host devices` и `llm_host status`** после T2 ходят в резидент по HTTP
  (`/internal/devices`) — если резидент не поднят/завис, будет warn («резидент не отвечает»),
  а не список устройств.
* **`HDS_CONFIG`** — сильнейший путь конфига (пробы `hds check` на песочных конфигах), но
  сессионный (`Remove-Item Env:HDS_CONFIG,Env:HDS_ROOT`); **`HDS_ROOT`** ломает
  `project_root()`/воркер — для MCP/RAG/индексации не ставить.
* **`index.pause` в репо-корне — боевая пауза владельца: не снимать.** Песочницам нужен свой
  корень (`load5.py::make_ixroot`); `Stop-Process -Force` = TerminateProcess (уборка не идёт).
* **Порты:** 8010–8012 — резидент; песочницы: 8797 (mcp), 8799 (fake remote); `load5.py`
  требует свободный 8010.
* **git — всегда `--no-pager`**; `config.yaml` и `tools/parity/out/` — в `.gitignore`;
  править `load5.py` можно только когда ни один прогон не запущен.
* **После server-профильных проб чистить `%ProgramData%\hermes-disk-search`**
  (`access.yaml`, снапшот очереди, `data/logs/audit.jsonl`) — `load5.py` делает это сам.

### Остановить / вернуть боевые сервисы (проверено)
```powershell
# остановить (освободить target\debug\hds.exe для сборки и 8010-8012 для песочниц)
Get-CimInstance Win32_Process -Filter "Name='hds.exe'" | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
& .\target\release\llm_host.exe stop
# вернуть (порядок: резидент → watch → mcp-http → transcribe-watch)
Start-Process -FilePath '.\target\release\llm_host.exe' -ArgumentList 'run' -WindowStyle Hidden
Start-Sleep 25
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'watch' -WindowStyle Hidden
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'mcp','--http','--host','127.0.0.1','--port','8787','--path','/mcp' -WindowStyle Hidden
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'transcribe-watch' -WindowStyle Hidden
# проверить: status → card=0 backend=CUDA, chat LOADED; index.pause=True; :8010 и :8787 → 200
```

### Стенд и полезные пробы
```powershell
python tools\parity\load5.py --tag p5_16384 --parallel 5 --ctx 16384            # базовый SLA (8010 свободен!)
python tools\parity\out\m6_t3_runtime.py                                        # карт-строки + живой запрос (T3)
python tools\parity\out\m6_t1_verify.py                                         # пробы пункта gpu_roles (T1)
```
* Прогон стенда 1–6 мин → фоном (`Start-Process python ... -RedirectStandardOutput <лог>`) и поллить;
  пишет `tools/parity/out/m4_load_<tag>.json`, сам чистит системный state-каталог.

## Хвосты (не потерять)
* **M6-хвосты:** мультикарточность живьём не проверена (нет железа) — юниты + хвост
  «машина с N картами»; чат-реплики на двух картах — без живой проверки; дискретной AMD нет —
  AMD-путь частично (iGPU-свидетельство T0); Apple Silicon — подготовка без живой проверки;
  смешанные вендоры и UMA — вне области поддержки (warn).
* **Движок: рост fit-шага** от числа загрузок в процессе (0,57 → 4,2 → 27,3 → **299 с**) —
  фикс M5 убрал перезагрузки из боевого пути, но ARB-3/`model_policy: auto`/смена модели
  снова могут дать минутные паузы → кандидат `engine-patch` P5.
* **Анонимизация (W9)** — без неё `chat.provider: remote` только для доверенных API;
  **`remote` на реальном API**; **чек-лист внешнего HTTPS** (домен/сертификат).
* **F9 (M3, `Host` не читается)**, **T0.9** (вопросы заказчику: пароль в argv, транскрибация
  в server), **M5-1…M5-3** (`M5_CODE_REVIEW.md` §3: косметика msg `chat_provider`; TLS только
  webpki-корни; rustls и оффлайн-сборка `cargo vendor`).

## Текущее состояние на момент передачи
HEAD `32b3de6`; дерево чистое; боевой резидент поднят (chat LOADED, `card=0 backend=CUDA`,
embedding/rerank `on_demand`), `index.pause` — есть; сервисы `watch`/`mcp-http`(:8787)/
`transcribe-watch` работают; `hds check` → ok. Песочная Vulkan-копия движка —
`tools/parity/out/engine-vulkan` (наша `ggml-vulkan.dll`); боевой каталог движка без Vulkan.
Ближайший шаг — **T4** (пресеты и автодетект), затем T5 (приёмка, приоритет — транскрибация)
и T6 (доклад).



