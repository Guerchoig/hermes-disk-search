# M6_CODE_REVIEW.md — код-ревью этапа M6 «GPU: раскладка ролей по картам» (Э5/W4)

> Дата: 10.10.2026 · Ветка: **`server-multiuser`** · HEAD (на момент ревью): `ffc450c`
> (T6-доклад, этап закрыт), дерево чистое.
> Объём: коммиты `b47694e`…`ffc450c` (план, T0-спайк, T1–T5, T6-доклад) — код:
> `crates/hds-llama/src/{config,device,registry,dispatch,host,status}.rs`,
> `crates/hds-llama/src/bin/llm_host_plan.rs`, `crates/hds-index/src/diag.rs`,
> `crates/hds-llama/tests/{arbiter,registry_plan}.rs`, `config.example.yaml`.
> Метод: чтение кода (полные M6-диффы и итоговое состояние) + **независимые проверки
> дерева** (не доверяя журналу §8a): `cargo test --workspace` — **88 наборов / 0 failed**,
> `cargo fmt --all --check` и `cargo clippy --workspace --all-targets -- -D warnings` —
> чисто. Живых стендов в этой сессии не гонял: мультикарточного железа нет, а
> рантайм-приёмка T5 уже зафиксирована в `PLAN_M6_GPU_MULTIVENDOR.md` §8a и `STATUS.md`
> (перепроверял её утверждения **по коду**, а не повторным прогоном).

## 1. Итог

Код M6 **принят** (findings некритичные, переносятся в хвосты). Контракты §3 плана
реализованы по существу и согласованно между `hds-llama` и `hds-index`:

* **раскладка ролей по картам** (`gpu.devices` → `llm_server.<role>.device` → компат
  `gpu.device_index`) с приоритетом ключа роли над раскладкой; «одна роль — ровно одна
  карта» и «whisper ≤1 на карту» — **fail-fast с числами** (`LlmHostConfig.errors`,
  `Host::start`/`llm_host_plan` отказываются стартовать; `registry::cross_validate`);
* **бюджет и вытеснение в пределах карты** (`role_cards`, `free_for_role`,
  `dispatch::on_card`/`cards_in_use`, карт-скоуп предохранителя простоя и ARB-3) —
  поведение на одной карте совпадает с M4/M5 (компат: пустая раскладка → прежний путь);
* **чат-реплики** `chat-N`/`whisper-N`, один фасад + least-busy → round-robin
  (`pick_replica` — чистая функция с юнитом), публикуются только базовые порты;
* **транскрибация по картам** (`Mutex<BTreeMap<bridge-индекс, Arc<WhisperCell>>>`,
  `pick_whisper_card` по свободной VRAM с числами в логе);
* **пресеты/автодетект** (`GpuPreset`+`AccelFacts`, `effective_layout`,
  `gpu.autodetect` **выключен по умолчанию**), печать раскладки с числами в логе
  старта, `llm_host_plan` и `hds check` (`gpu_roles`);
* десктоп-профиль и `gpu.device_index` не изменены, секреты не печатаются.

Незакрытых дефектов уровня «ломает прод» не найдено. Находка **F-m6-t5-1** (клиенты
всегда шлют `gpu` → host-выбор карты не срабатывает) подтверждается кодом и **остаётся
открытой** (решение — за владельцем). Есть одно расхождение диагностики (**M6-1**,
medium: `hds check` не читает `llm.<role>.device`) и ещё несколько некритичных
замечаний (§3).

## 2. Что проверено (независимо и по коду)

| Проверка | Как проверено | Результат |
|---|---|---|
| Полный набор тестов | `cargo test --workspace --no-fail-fast` (независимо) | **88 наборов / 0 failed** ✅ (совпадает с журналом §8a и верификацией T1) |
| Формат/линт | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` | чисто ✅ |
| Раскладка `gpu.devices` | код `config.rs::build` (парсинг+warn по номеру записи), `registry::plan_role` (приоритет ключ→devices→компат, двойное назначение → `Err` с числами); тест `layout_assigns_roles_to_cards_and_reports_backend` | соответствует §3.1/§3.5 ✅ |
| Пресет/автодетект | код `GpuPreset::{parse,autodetect,layout,required_vendor}`, `effective_layout` (порядок: devices → preset → autodetect → компат), `preset_note`; тесты `gpu_preset_layout_and_override`, `gpu_autodetect_off_by_default` | соответствует §3.4 ✅ |
| Карт-скоуп бюджета/вытеснения | код `dispatch::{on_card,cards_in_use}`, `host::{free_for_role,uses_for_role}`, `uses()` (проставляет `u.card` из `role_cards`), `arbiter_tick` (idle и ARB-3 — по карте); тесты `eviction_and_budget_are_scoped_to_card`, `cards_in_use_lists_distinct_sorted_cards`, `idle_eviction_on_one_card_does_not_touch_other_card` | соответствует §3.3 ✅ |
| Чат-реплики / один фасад | код `host::{chat_roles,chat_loads,pick_chat_role}`; юнит `pick_replica` (least-busy→RR) | соответствует §3.6 ✅ |
| Whisper по картам | код `host::{whisper_cards,pick_whisper_card}` (числа в логе), клетки по bridge-индексу; `cross_validate` — ≤1 whisper на карту | соответствует §3.5/§3.6 ✅ |
| Fail-fast контракта | код `host.rs:2695` (`contract_note()` → `Err`), `llm_host_plan.rs:123` | отказ старта с текстом ✅ |
| Статус по картам | код `host::cards_lines`, поле `cards` в `/internal/status` (~2050), `cards_line` в `LocalStatus` | соответствует §3.3 ✅ |
| `hds check` `gpu_roles` | код `diag::{check_gpu_roles,check_gpu_roles_live,preset_hint_text}`; статус пункта — `ok/warn`, требует живой `/internal/devices` | частично — см. **M6-1** ⚠️ |
| Хвост T5 (клиенты) | код `transcribe.rs:392/439` (`"gpu": self.cfg.gpu` всегда) + `host.rs:2244-2248` | **F-m6-t5-1 подтверждён** ⚠️ |

## 3. Находки

**M6-1 (medium) — `hds check` `gpu_roles` игнорирует `llm.<role>.device`.**
`crates/hds-index/src/diag.rs::role_device_from_cfg` (~1097) читает **только**
`llm_server.<role>.device`, тогда как рантайм (`crates/hds-llama/src/config.rs::build_role`,
~981) даёт приоритет `llm.<role>.device` (и явно документирует это в §3.1 плана).
Следствия: если оператор задал карту роли новым ключом (`llm.chat.device: 2`), пункт
`gpu_roles` не покажет роль/карту/VRAM, не поймает её двойное назначение с `gpu.devices`
и в части случаев напечатает «раскладка ролей по картам не задана». Это расхождение
двух крейтов (в `hds-index` логика продублирована намеренно — нет зависимости от
`hds-llama`) и частичный пробел приёмки §5 («`gpu_roles` с картами/ролями/числами»).
Тестами `llm.<role>.device` в `diag.rs` не покрыт. Рекомендация: читать
`llm.<role>.device` первым, затем `llm_server.<role>.device` — зеркально `config.rs`.

**M6-2 (medium, унаследовано: F-m6-t5-1) — штатные клиенты всегда шлют `gpu`, host-выбор
карты не срабатывает.** `crates/hds-index/src/transcribe.rs` (строки 392 и 439) всегда
кладёт `"gpu": self.cfg.gpu` в тело `/internal/transcribe`; `host.rs:2244-2248` вызывает
`pick_whisper_card` только при отсутствии `gpu`. Итог: на машине с N картами все файлы
`hds transcribe`/`auto_transcribe` пойдут на одну карту (bridge-индекс из
`gpu.device_index`/`whisper_gpu`), а экземпляры `whisper-N` на других картах простаивают.
Находка честно зафиксирована этапом (журнал T5), **не исправлена** — ждёт решения
владельца (менять семантику `-1 = как gpu.device_index` нельзя без согласования).
Рекомендация: не заполнять `gpu`, когда `whisper_gpu` не задан явно.

**M6-3 (low) — пресет принимается на «неизвестных» вендорах.** `GpuPreset::layout`
(~265) проверяет вендор как `!vendors.is_empty() && !vendors.contains(&want)`. Если
описания всех карт не распознаны (`vendor_of` → `unknown`, `unique_vendors` пуст),
проверка пропускается — пресет `nvidia-n`/`amd-n` применится «вслепую» и разложит роли
по числу ускорителей. Для прод-профиля это маловероятно, но лучше warn с числами.

**M6-4 (low, косметика) — `CUDA ,` в `gpu_roles`.** `diag.rs::check_gpu_roles_live`
(~1377) печатает `format!("{line} ({backend} {desc}, свободно …")`; при пустом
`description` устройства получается «`CUDA , свободно …`» (пробел перед запятой).
Отмечено независимым верификатором T1, но в коде осталось. Рекомендация: склеивать
`backend`/`desc` через non-empty join.

**M6-5 (low) — предохранитель простоя whisper глобален, не по картам.**
`host.rs::whisper_idle_evict` (~1717) ведёт один таймер простоя по базовой роли
`whisper` (`idle_map().get("whisper")`) и при срабатывании **очищает транскрибаторы всех
карт** (`map.clear()`), в отличие от остального T3 (везде карт-скоуп). На N картах нельзя
освободить VRAM простаивающей карты, пока другая занята. Отклонение от «per-card»
намерения — задокументировать или сделать по картам.

**M6-6 (low) — `gpu_roles` проверяет только базовые роли.** `check_gpu_roles` идёт по
списку `["chat","embedding","rerank","whisper"]`; карта реплик `chat-2`/`whisper-2`
(их `device`/записи `gpu.devices`) в `hds check` не показывается и не валидируется —
только в рантайме (`cross_validate`). Небольшой пробел относительно формулировки
«карты/роли/числа» в §5.

## 4. Сверка приёмки §5 плана с фактом

| Пункт | Факт | Статус |
|---|---|---|
| T0: Vulkan-бэкенд из нашего дерева, роль на Vulkan-устройстве, фолбэк — с числами | Спайк GO (`d635d03`), отчёт `SPIKES.md` §15, свидетельство на AMD iGPU (частичное) | ✅ (по журналу; сборку не перепроверял) |
| Роли реально на назначенных картах; backend в `status`; недоступная карта → ошибка с числами | `registry::plan_role` → `bridge_device_index`/`device_backend`, `summary`; `selection_for_role` fail-fast с ролью и числами | ✅ |
| Реплики: несколько чат-инстансов, каждый своим портом; индексация — один экземпляр; whisper — на карту | `chat-N`/`whisper-N`, `check_unique_ports`, `cross_validate`, `single_instance_violation` | ✅ |
| Бюджет/вытеснение по картам; на одной карте числа = M4/M5 | `on_card`/`role_cards`/`free_for_role`; юниты карт-скоупа; живой регресс одной карты — в T3/T5 журнала | ✅ (код+юниты; живой регресс — по журналу) |
| Компат `gpu.device_index`, десктоп не изменён; `gpu_roles` с числами | пустая раскладка → прежний путь; `role_lines`/backend/free по живым устройствам | ⚠️ кроме `llm.<role>.device` (**M6-1**) |
| `cargo test` зелёный; fmt/clippy чисто | **88/0**, fmt/clippy чисто (независимо) | ✅ |
| Рантайм-проверки (транскрибация приоритетно): живой файл, латентность/карта; `status`, `hds check` | T5: 6 прогонов (5 ok), R1 10,48 с рядом с чат-нагрузкой, пик ~11028 МиБ, паралича резидента нет; R3 host-выбор сработал | ✅ (по журналу T5 + `m6_t5_report.json`; не перепрогонял) |

## 5. Статус хвостов после M6

| Хвост | Статус |
|---|---|
| **F-m6-t5-1** (клиенты всегда шлют `gpu`) — **M6-2** | ⏸ открыт — ждёт решения владельца |
| **M6-1**: `gpu_roles` не читает `llm.<role>.device` | ⏸ новый — некритично, но подрывает часть `hds check` |
| Мультикарточность живьём (2+ карты одного семейства) | ⏸ открыт — нет железа (юниты + хвост) |
| Дискретная AMD / Vulkan-путь целиком | ⏸ частично — только iGPU-свидетельство |
| Apple Silicon (Metal) | ⏸ без живой проверки — машина с Mac |
| M6-3/M6-4/M6-5/M6-6 | ⏸ новые (§3) — низкие, в следующие этапы |
| Хвосты M5 (P5/рост fit-шага, анонимизация W9, HTTPS-чек-лист, M5-1…M5-3) | ⏸ без изменений |

## 6. Рекомендации для следующего этапа

1. Починить **M6-1** (проброс `llm.<role>.device` в `diag::role_device_from_cfg`) —
   дёшево и восстанавливает согласованность диагностики с рантаймом; добавить юнит.
2. Решить судьбу **F-m6-t5-1/M6-2**: либо клиенты перестают слать `gpu` без явного
   `whisper_gpu` (тогда host-выбор карт заработает), либо зафиксировать ограничение
   «транскрибация — одна карта» в доке явно.
3. Косметику **M6-4** и `unknown`-вендор **M6-3** — в ближайший кодовый этап;
   **M6-5/M6-6** — в контекст эксплуатационной доводки мультикарточности.
4. Перед Э11 (нагрузочная приёмка на 1/2 картах) — прогнать мультикарточный сценарий
   на реальном железе: без него карт-скоуп бюджета и раскладка покрыты только юнитами.

## 7. Как воспроизведены проверки

```powershell
# независимая проверка дерева (эта сессия)
cargo test --workspace --no-fail-fast        # 88 наборов / 0 failed
cargo fmt --all --check                       # чисто
cargo clippy --workspace --all-targets -- -D warnings   # чисто
```

> Живые рантайм-проверки этапа (спайк T0, стенд T5, `hds check --json`, `llm_host_plan`)
> в этой сессии **не повторялись**: мультикарточного железа нет, а результаты и артефакты
> зафиксированы в `PLAN_M6_GPU_MULTIVENDOR.md` §8a, `STATUS.md` и `tools/parity/out/m6_*`.
> Их утверждения сверены с кодом (см. §2), расхождений, кроме M6-1/F-m6-t5-1, не найдено.