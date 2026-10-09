# PLAN_M5_EXTERNAL_CHAT.md — детальный план M5 («Хвосты M4 + внешняя чат-модель remote», Э6/W9)

> Дополняет `PLAN_SERVER_MULTIUSER.md` §2 (**W9** «Внешняя чат-модель», §7 п.3 —
> анонимизация) и §5 (**этап Э6**; хвосты M4 — закрытие находок `M4_CODE_REVIEW.md`).
> Структура — по образцу `PLAN_M4_CONCURRENCY_SLA.md`. Промт предыдущего этапа —
> `M5_HANDOFF_PROMPT.md`; код-ревью M4 — `M4_CODE_REVIEW.md` (F1–F9).
>
> **Основание:** M1–M4 закрыты на ветке `server-multiuser`. Код-ревью M4 (09.10.2026)
> показало: **приёмка SLA ≤5 мин не воспроизводима** (3 из 5 прогонов p95 ~315 с при
> пустой очереди — стопор внутри движка/слота), аутентификация применяется **после**
> очереди (DoS), дефолты server дают 4 одновременных RAG вместо 5. Рекомендация ревью
> (§6): сначала хвосты, потом новые фичи. Владелец согласовал (10.10.2026):
> **M5 = T0-хвосты M4 + внешний chat-provider `remote`**.
>
> **Статус:** запланирован 10.10.2026. Ветка — **`server-multiuser`**.
>
> **Зависимости:** M3 (rate-limit/аудит), M4 (очередь/квоты/слоты/БД — база для хвостов
> T0 и для проверки поведения при `remote`), контракт ключей M4 §3.5 (не менять конфиг
> дважды). Анонимизация (W9) — **вне M5**: без неё внешний сервис не считается
> допустимым, поэтому `remote` в M5 — опция для доверенного API; фиксируем явно в §7.

## 1. Вводные (факты из кода, а не предположения)

| Факт | Где |
|---|---|
| F1-факты: «провальные» отчёты стенда (`p5_final`, `p5_idx`, `p5_norerank`) содержат ровно один запрос ~315 с при `granted=5`, `depth=0`, `total_wait_ms=0` — очередь не ждала; таймауты клиента 600 с → это реальный отклик. При n=5 `p95 = max` | `M4_CODE_REVIEW.md` §3 F1, `tools/parity/out/m4_load_*.json` |
| Признак в логе резидента при выбросе: `[dispatcher] роль 'embedding': LOAD_ON_DEMAND` / `[busy] двигатель занят: load:embedding`; в статусе embedding/rerank `on_demand`, chat `keep` | `crates/hds-llama/src/{host.rs,config.rs}`, `llm_host status` |
| F2-факты: в `serve_request` тяжёлый запрос при `Some(q)` сперва `q.acquire(...)` (ждёт до `wait_timeout_sec`=240 с) и только потом `route(...)`, где проверка Bearer даёт `401` | `crates/hds-mcp/src/http.rs` — `serve_request` (~542), тяжёлая ветка (~810–839), `route` (~84–110) |
| F3-факты: дефолт `rate_limit.max_inflight` = 4 (лимитер M3), дефолт `llm_server.parallel` = 5 (server), очередь берёт лимит исполнения из `max_inflight` (`QueueConfig::from_config`); `hds check` печатает оба числа, рассинхрон не помечает | `crates/hds-core/src/{ratelimit.rs:45,queue.rs:101}`, `crates/hds-index/src/diag.rs` (пункт `queue`) |
| F4: `can_admit` выбирает следующего пользователя по политике, `seq` внутри пользователя не сравнивается — «FIFO внутри пользователя» в формулировках не гарантирован | `crates/hds-core/src/queue.rs` |
| F5: нет `cv.notify_all()` при успешной постановке → освободившийся слот простаивает до `WAIT_SLICE` (150 мс) | `crates/hds-core/src/queue.rs` |
| F6: `Limiter::{inflight_acquire,inflight_release,inflight_guard}`/`InflightGuard` — мёртвый путь (только юнит-тесты), рантайм — очередь/`user_inflight_*` | `crates/hds-core/src/ratelimit.rs` (~111–192) |
| F7: `queue_retry_after(Timeout)` зажат `clamp(1,60)` при `wait_timeout_sec` 240 | `crates/hds-mcp/src/http.rs` |
| F8: `DbOptions::safe_journal_mode` вычищает всё, кроме `[A-Za-z0-9]` — инъекции нет, но опечатка доезжает до `PRAGMA` и даёт невнятную ошибку | `crates/hds-core/src/db.rs` |
| RAG-чат уже ходит в OpenAI-совместимый сервер: `POST {chat.base_url}/chat/completions`, `model` — из `chat.model` (дефолт `qwen3.5-9b`), thinking off — через `chat_template_kwargs`; `Authorization` **не отправляется**; NUDGE-повтор при пустом `content` | `crates/hds-search/src/rag.rs:104-126,164-243` |
| HTTP-клиент ядра — **свой минимальный TCP без TLS** (`crates.io` на машине заказчика был недоступен в W2); `split_base` принимает только `http://` — `https` даёт ошибку «не HTTP-адрес» | `crates/hds-core/src/http.rs:1-74`, `crates/hds-index/src/embed.rs:206-211` |
| Проверено 10.10.2026: `index.crates.io` на dev-машине **доступен** → TLS-зависимость добавить можно (риск оффлайн-машины заказчика — в §7) | Cargo.toml (нет TLS-крейтов) |
| Роли резидента: chat `:8010` (retention keep), embedding/rerank `:8011/8012` (`on_demand`); реестр/диспетчер VRAM — арбитр; фасад обслуживает `/v1/chat/completions`, `/v1/embeddings`, `/v1/rerank` | `crates/hds-llama/src/{registry.rs,host.rs,facade.rs}` |


## 2. Границы M5

**Входит:**
- **T0 — хвосты ревью M4** (`M4_CODE_REVIEW.md` F1–F9) — закрыть до новой функциональности:
  root-cause выброса 315 с (F1), auth до очереди (F2), согласование дефолтов (F3),
  полировка очереди (F4–F8), недостающие сокет-тесты (F9).
- **Внешняя чат-модель `chat.provider: remote`** (контракт `PLAN_M4_CONCURRENCY_SLA.md`
  §3.5 — не меняем его): RAG-чат ходит во внешний OpenAI-совместимый API;
  **llm-host не держит chat-роль в VRAM** (не грузит, не автостартует, не считает в
  fit-гейте); embedding/rerank остаются локальными.
- Тесты (юнит + сокет + стенд), обновление `README.md`/`README_SERVER_MULTIUSER.md`/
  `STATUS.md`/журнала §8a; `PLAN_SERVER_MULTIUSER.md` §9 — по завершении этапа.

**Не входит (фиксируем явно, чтобы не потерять):**
- **Анонимизация (W9)** — отдельный трек/этап; до неё `remote` — «доверенный API»
  (решение заказчика: внешний API допустим с будущей анонимизацией, §7 п.3 плана).
- Э5 (GPU multi-vendor/AMD+NVIDIA/Apple Silicon) — следующий этап по дорожной карте.
- Чек-лист внешнего HTTPS (M3/M4) — на машине с доменом/сертификатом.
- F9 (M3, `Host`-заголовок) и T0.9 (M2-F7/F9, вопросы заказчику) — остаются открытыми
  хвостами, в M5 не входят.

## 3. Технические решения (контракты)

### 3.1. Хвосты M4 (не менять)
- Дефолты согласуются кодом: server `rate_limit.max_inflight` ≥ `llm_server.parallel`
  (F3 — поднять дефолт до 5 и добавить warn в `hds check` при рассинхроне).
- Auth-проверка (F2): при `access.enabled` валидность Bearer проверяется **до** постановки
  в очередь (быстрый `resolve_bearer` → `401`); очередь/квоты — только для аутентифицированных.
- SLA-приёмка (F1) переформулируется: ≥3 прогона одного конфига; при n=5 `p95 = max`;
  критерий — «доля прогонов, где max ≤ 300 с»; в докладе честно фиксировать провальные.

### 3.2. Внешний чат-provider (ключи конфига — по §3.5 плана M4, дословно)
```yaml
chat:
  provider: local          # local | remote
  base_url: "https://api.example.com/v1"
  api_key: ""              # Bearer-токен внешнего API (только remote); в config.yaml (gitignored)
  model: "gpt-4o-mini"
```
- **Ключи существующие, без новых секций**: `chat.provider` — новый; `chat.base_url`/
  `chat.model` уже читаются `rag.rs` (base_url дефолт `http://127.0.0.1:8010/v1`);
  `chat.api_key` — новый.
- **Семантика `provider`**:
  - `local` (дефолт, desktop и server) — как сейчас: chat-роль в llm-host, base_url — фасад.
  - `remote` — RAG-чат идёт в `chat.base_url` напрямую (внешний OpenAI-совместимый API);
    llm-host: chat-роль **исключается** из плана реестра/автостарта и из fit-гейта VRAM
    (`llm_server.parallel`/`ctx_per_slot` к чату не применяются); embedding/rerank — как было.
  - Слоты/очередь/квоты MCP **не меняются**: очередь (`queue.*`) и квоты (`rate_limit.*`)
    остаются в `hds mcp-http` — латентность remote не отменяет fair-share.
- **Аутентификация remote:** `Authorization: Bearer <chat.api_key>` на каждый запрос
  (если `api_key` непуст и отличен от `-`); TLS — см. §3.3.
- **`hds check`:** новый пункт `chat_provider` — `remote` без `base_url` → warn; без
  `api_key` → warn («внешний API без ключа»); `local` — десктоп/фасад, поведение прежнее.
- **Анонимизация:** в M5 **не реализуется**; README фиксирует: без анонимизации текст
  вопроса и фрагменты локальных файлов уходят внешнему провайдеру как есть — включать
  `remote` только для доверенных API (W9 — следующий шаг).

### 3.3. TLS для внешнего API
- Наш HTTP-клиент — TCP без TLS; внешний API обычно `https`. Контракт: добавить в
  `hds-core::http` поддержку `https` (rustls; crates.io на dev-машине доступен,


## 4. Треки

### T0. Хвосты ревью M4 (`M4_CODE_REVIEW.md` F1–F9) — 2–3 дн
- [ ] **T0.1 (F1, приёмка):** ≥3 прогона `load5.py` одного конфига (`p5_16384`) +
      таймлайн (`polls` отчёта, лог резидента `[dispatcher]`/`[busy]`, снапшоты VRAM)
      → root-cause выброса ~315 с. Кандидаты: on-demand загрузка/перепланирование
      `embedding` при 5 занятых слотах; ретенция; прогрев; арбитр. Починка
      (`llm_server.embedding.retention: keep` / прогрев перед приёмом тяжёлых) —
      отдельным коммитом, затем повторный стенд ≥3 прогонов. SLA считать
      **подтверждённым** только если во всех прогонах max ≤ 300 с.
- [ ] **T0.2 (F2, DoS):** при `access.enabled` проверять Bearer **до** `queue.acquire`
      (`resolve_bearer` → `401` сразу; недействительный токен в очередь не ставится).
      Сокет-тест: неавторизованный тяжёлый запрос при занятых слотах → мгновенный `401`,
      `depth` не растёт; валидный — ждёт как раньше.
- [ ] **T0.3 (F3):** дефолт `rate_limit.max_inflight` server ≥ `llm_server.parallel`;
      `hds check` (пункт `queue`) помечает рассинхрон `parallel` ↔ `max_inflight`
      (warn с числами). Десктоп-профиль не меняется.
- [ ] **T0.4 (F4):** строгое «FIFO внутри пользователя»: сравнение `seq` при допуске
      (`can_admit`/пробуждение выбирают минимальный `seq` этого пользователя);
      формулировки README — точные.
- [ ] **T0.5 (F5):** `cv.notify_all()` после успешной постановки, если есть свободный
      слот (тест: слот снимается с ожидания без тайм-среза 150 мс).
- [ ] **T0.6 (F6):** мёртвый путь `Limiter::inflight_*`/`InflightGuard` удалить либо
      пометить как тест-API (выбрать по числу вызовов; регрессии M3 не сломать).
- [ ] **T0.7 (F7):** `Retry-After` таймаута очереди — от `wait_timeout_sec`
      (например `clamp(wait_timeout/2, 5, wait_timeout)`), а не жёстко 60.
- [ ] **T0.8 (F8):** allowlist `journal_mode` `{delete,truncate,persist,memory,wal,off}`
      + понятное сообщение об ошибке опечатки.
- [ ] **T0.9 (F9):** сокет-тесты: `QueueReject::Aborted` (обрыв клиента в ожидании →
      место освобождается, соединение молча закрыто) и сценарий F2.

### T1. Конфиг-контракт `chat.provider` + валидация — 0,5–1 дн
- [ ] `chat.provider` (дефолт `local`), `chat.api_key`; чтение в `hds-core::config`.
- [ ] Валидация + пункт `chat_provider` в `hds check`/`/api/diagnostics`.


### T3. llm-host без chat-роли при `remote` — 1 дн
- [ ] Реестр/автостарт/диспетчер: chat-роль с `provider: remote` не планится, не
      грузится, в `status` — `REMOTE`; fit-гейт и метрики VRAM — без chat.
- [ ] `hds status`/`llm_host status`/UI: понятное отображение.
- [ ] Тесты: реестр-план при remote (chat исключён, embedding/rerank на месте),
      десктоп `provider: local` — поведение прежнее.

### T4. Стенд remote + SLA-приёмка — 1 дн
- [ ] `load5.py`: флаг `--chat-remote <url>` (фейковый OpenAI-сервер с управляемой
      латентностью или реальный доверенный API по желанию владельца) — прогон
      5 × `ask_my_files` при выгруженной chat-роли: VRAM падает (~4 ГиБ свободно),
      RAG работает, `429/503`-поведение очереди не меняется.
- [ ] Отчёт `tools/parity/out/m5_*.json` (гитигнор) + числа в README.

### T5. Доклад — 0,5 дн
- [ ] `STATUS.md` (M5 целиком), `README.md` (§«Слоты и SLA» + provider remote),
      `README_SERVER_MULTIUSER.md` (сценарий remote + предупреждение об анонимизации),
      журнал §8a; `PLAN_SERVER_MULTIUSER.md` §9 — по завершении этапа;
      `M4_CODE_REVIEW.md` — отметки о закрытии F1–F9 (история сохраняется).

## 5. Приёмка
- [ ] `cargo test --workspace` зелёный; `cargo fmt --all --check`, `cargo clippy
      --workspace --all-targets -- -D warnings` — чисто.
- [ ] F1: ≥3 прогона `load5.py` одного конфига — max ≤ 300 с во всех; либо явный
      root-cause + починка + повторная приёмка (числа в §8a).
- [ ] F2: сокет-тест — неавторизованный тяжёлый запрос при занятых слотах → мгновенный
      `401`, место в очереди не занимает.
- [ ] F3: дефолты согласованы; `hds check` помечает рассинхрон.
- [ ] F4–F8: юнит/сокет-тесты; `journal_mode: wal2` в конфиге → понятная ошибка.
- [ ] Remote: живой прогон `ask_my_files` с фейковым/реальным remote-API при
      выгруженной chat-роли (`/props` чата недоступен или VRAM без chat), ответ с
      источниками; очередь/квоты работают как в M4; десктоп `local` не изменён.
- [ ] Рантайм-проверки, а не только юниты: `hds check` (новый пункт), `llm_host status`
      (`REMOTE`), сокет-тесты на реальном HTTP.

## 7. Риски
| Риск | Митигация |
|---|---|
| TLS-зависимость (rustls) на оффлайн-машине заказчика | Cargo.lock фиксирует версии; сборка оффлайн — `cargo vendor` в Э8/Э9; TLS только в клиенте RAG-чата |
| Внешний API видит вопрос + фрагменты локальных файлов | предупреждение в README/`hds check`; анонимизация — W9 (следующий шаг), до неё remote — только доверенный API |
| remote без ограничений частоты/стоимости со стороны провайдера | очередь/квоты MCP остаются; предложенные дефолты квот не меняются |
| Выброс 315 с может не воспроизвестись в 3 прогонах (флаки) | приёмка «доля прогонов с max ≤ 300 с»; при невоспроизводимости — фиксировать статистику и условия, не закрывать «молча» |
| Починка F1 (retention keep embedding) сжирает VRAM при 5 слотах чата | fit-гейт с числами; при нехватке — явная ошибка, не деградация (решение заказчика) |
| `chat.api_key` в `config.yaml` (плоский файл) | config.yaml в `.gitignore`; в README — права на файл; `api_key` не печатать в диагностике |

## 8a. Журнал исполнения (заполняется по ходу этапа)

| Трек | Коммит | Что сделано (факт) |
|---|---|---|
| план | `601618e` | План этапа M5: хвосты M4 (F1–F9) + внешний chat-provider; согласовано с владельцем 10.10.2026 (вариант «T0 + remote»). |
| T0.2 (F2) | `c2dc2ea` | Аутентификация ДО очереди: в `serve_request` при `access.enabled` неавторизованный тяжёлый запрос получает `401` до `queue.acquire` (аудит `unauthorized` сохранён). Сокет-тесты: `unauthorized_heavy_rejected_before_queue` (занятый слот → мгновенный 401, depth не растёт, валидный пользователь ждёт как раньше) и `aborted_client_releases_queue_slot` (T0.9/F9: обрыв клиента в ожидании → `aborted`, место освобождается). |
| T0.3 (F3) | `47b79c1` | Дефолт `rate_limit.max_inflight` по профилю: server **5** (= `llm_server.parallel`), desktop 4; `hds check` (пункт `queue`) помечает рассинхрон `parallel` ↔ `max_inflight` warn'ом с числами; юнит-тесты `hds-core::ratelimit`, `diag::queue_check_warns_on_inflight_parallel_mismatch`; `config.example.yaml`. |
| T0.4–T0.8 (F4–F8) | `6e9e1cf` | F4: строгий FIFO внутри пользователя — слот берёт минимальный `seq` (`ahead_of_me` в `acquire`), тест `fifo_within_user_by_seq`; F5: `cv.notify_all()` после постановки (свободный слот не простаивает 150 мс); F7: `Retry-After` таймаута = `wait_timeout/2` (min 5) вместо cap 60, тесты в новом `mod tests` `hds-mcp::http`; F8: allowlist `journal_mode` `{delete,truncate,persist,memory,wal,off}` + фолбэк `wal` с громким сообщением, тест `journal_mode_allowlist_falls_back_to_wal`. |
| T0.6 (F6) | `ee58b19`+`6fa51d6` | Мёртвый путь `Limiter::inflight_*`/`InflightGuard` удалён; тесты переведены на рантайм-путь `user_inflight_*`. |
| T0.1 (F1) | `204a88b` | **Root-cause найден и устранён.** База (3 прогона до фикса, `m4_load_f1_r1..r3`): 315,6 / 315,7 / 46,5 с — выброс вероятностный 2 из 3. По логам резидента: on-demand **перезагрузка `embedding`** при тесной VRAM; fit-шаг движка `llama_params_fit_impl` («свободно ~1900 МиБ») завершается **~299 с** (рост ×~8 с каждой загрузкой в процессе: 0,57 → 4,2 → 27,3 → 299; знак «−» — баг печати движка); unlucky-запрос ждал перезагрузку целиком (315 ≈ 299+16), очередь `granted=5/total_wait_ms=0`. Фикс: в server-профиле embedding/rerank по умолчанию `retention: keep` (явный ключ сильнее; desktop — `load_on_demand` как раньше) + прогрев embedding/rerank при старте llm-host (`host.rs`, после загрузки чата; ошибка прогрева — НЕ молчаливая). Стенд: `load5.py` в песочном llm-конфиге теперь `profile: server`. Приёмка: **3/3 прогона max ≤ 300 с — 34,1 / 34,0 / 33,9 с** (`m4_load_f1_fix_r1..r3.json`); прогрев виден в логе (fit 0,91/0,09 с), в ходе прогонов загрузок нет. Хвост: рост fit-шага от числа загрузок — кандидат в `engine-patch` (P5). |
| T1/T2/T3 (задел remote) | `204a88b` | Конфиг-контракт по §3.5 плана M4: `chat.provider` (`local|remote`, дефолт `local`), `chat.api_key` (`hds_core::config::{chat_provider,chat_api_key}`); RAG (`hds-search/src/rag.rs`) ходит в `chat.base_url` через `hds-core::http::request_url` — **TLS-клиент rustls** (корни webpki; только исходящие, периметр = reverse-proxy) + `Authorization: Bearer` при непустом ключе; `parse_url` (дефолт-порты 80/443, IPv6) с юнит-тестами, тест `request_url` на живом фейк-сервере OpenAI-формата. llm-host: при `chat.provider: remote` chat-роль **не планится** (`config.rs::build` — предупреждение в `warnings`), fit-гейт/VRAM без chat; `hds check` — новый пункт `chat_provider` (remote без `base_url`/`api_key` → warn, секрет не печатается, напоминание про анонимизацию W9). `cargo test --workspace` (369+ новых), fmt/clippy `-D warnings` — зелёные. |
| T4 (стенд remote) | `ae0c664` | `load5.py`: флаг `--chat-remote fake|<url>` (+`--remote-port`, `--remote-latency`) и встроенный фейковый OpenAI-сервер (`/v1/chat/completions`, счётчики запросов/авторизации); при remote llm-конфиг получает `chat.provider: remote` (chat-роль не планится), ожидание готовности — по embedding-фасаду. Приёмка `m4_load_m5_remote_r1.json`: 5 × `ask_my_files` → **200×5, p95 6,8 с** (fake latency 3 с), remote-счётчик `requests=5, auth_ok=5` (Bearer доходит), **VRAM 1102 → 1121 МиБ** (было 8916 → 9643 — chat-роль освободила ~7,8 ГиБ), ответы содержат текст remote + источники поиска. |

## 9. Ссылки
- `PLAN_SERVER_MULTIUSER.md` — §2 W9, §5 Э6, §7 (анонимизация), §9.
- `M4_CODE_REVIEW.md` — находки F1–F9 (источник трека T0).
- `PLAN_M4_CONCURRENCY_SLA.md` — §3.5 (контракт ключей remote — не менять), §8a.
- `tools/parity/load5.py` — стенд; `tools/parity/out/` — артефакты (гитигнор).
- `M5_HANDOFF_PROMPT.md` — промт предыдущего этапа.

- [ ] `config.example.yaml`: ключи с пояснениями (анонимизация — W9, предупреждение).

### T2. RAG ходит в remote — 1–1,5 дн
- [ ] `hds-core::http` + `embed::split_base`: поддержка `https` (rustls, только клиент).
- [ ] `rag.rs`: при `provider: remote` (или непустом `api_key`) — заголовок
      `Authorization: Bearer`; ошибки внешнего API — читаемое сообщение (как сейчас);
      NUDGE-логика сохраняется; `chat.timeout` — тот же потолок.
- [ ] Тесты: юнит `rag` (payload/заголовок/ошибка), сокет-тест с фейковым
      OpenAI-сервером (`POST /v1/chat/completions` → фиксированный JSON) на живом
      `hds mcp-http`: `ask_my_files` возвращает ответ, chat-роль фасада не поднималась.

  проверено 10.10.2026). TLS-клиент — только исходящие запросы RAG-чата
  (входящие серверы остаются без TLS — периметр = reverse-proxy, M3).
- `split_base` расширяется: `https://` → соединение через TLS, дефолт-порт 443.
- Риск оффлайн-машины заказчика: зависимость попадает в `Cargo.lock`/релизные бинарники;
  для сборки из исходников оффлайн — `cargo vendor` в Э8/Э9 (пометка в README сборки).

### 3.4. llm-host при `remote` (раскладка VRAM)
- Реестр/диспетчер: роль `chat` с `provider: remote` — `state=REMOTE`, не грузится,
  не эвиктируется, в `status` печатается как «внешняя (`chat.base_url`)».
- Fit-гейт/статус VRAM: свободная VRAM считается **без** chat-модели — это и есть
  «рычаг SLA» (освобождается ~8–9 ГиБ на RTX 3060 12 ГиБ).
- Политика `llm.model_policy: fixed` не трогается; конфиг chat-роли (`llm_server.chat.*`)
  при `remote` игнорируется с warn-пунктом в `hds check`.

| `llm_server.parallel` доходит до движка (`registry::plan_role`: у чата `parallel`, у индексных ролей 1); шлюз `RwLock`; fit-гейт VRAM с числами | `crates/hds-llama/src/{registry.rs,host.rs,gate.rs}` |
| Стенд T5 умеет песочные конфиги через `HDS_CONFIG` (подмена `chat.base_url`/`provider` возможна), сам чистит state-каталог; требует свободные 8010–8012 | `tools/parity/load5.py` |
| Слоты и VRAM (боевой замер): chat Qwen3.5-9B Q6_K «модель+KV» ~8–9 ГиБ на 12 ГиБ — при `remote` эта VRAM освобождается | `llm status`, `tools/parity/out/m4_slots_*.json` |
