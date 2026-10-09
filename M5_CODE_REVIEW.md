# M5_CODE_REVIEW.md — код-ревью этапа M5 «Хвосты M4 + внешняя чат-модель remote»

> Дата: 10.10.2026 · Ветка: **`server-multiuser`** · HEAD (на момент ревью): `db15bad`
> (формальное закрытие M5), дерево чистое.
> Объём: коммиты `601618e`…`ae0c664` (план, T0-хвосты F1–F9, T1–T4 remote) — код:
> `crates/hds-core/src/{db,queue,ratelimit,config,http}.rs`, `crates/hds-mcp/src/http.rs`
> + `tests/{queue_socket,quota_socket}.rs`, `crates/hds-search/src/rag.rs`,
> `crates/hds-llama/src/{config,host}.rs`, `crates/hds-index/src/diag.rs`,
> `Cargo.toml`/`crates/hds-core/Cargo.toml` (rustls), `tools/parity/load5.py`.
> Метод: чтение кода (полные диффы `3037c4f..HEAD`) + **независимые рантайм-проверки**
> (не доверяя журналу §8a): полный `cargo test --workspace` (**87 наборов / 0 failed**),
> `cargo fmt --all --check`/`clippy -D warnings` (чисто), `hds check --json` на 5 песочных
> конфигах (`HDS_CONFIG`), **пересчёт артефактов стенда** питоном
> (`tools/parity/out/m5_review_recount.py`), **повторный живой прогон стенда** —
> базовый SLA (`m4_load_m5rev_r1.json`) и remote (`m4_load_m5rev_remote.json`),
> разбор лога песочного llm-host remote-прогона (`m5_review_remote_check.txt`).

## 1. Итог

Код M5 **принят без условий**. Все девять находок ревью M4 (F1–F9) закрыты по существу:
фиксы соответствуют формулировкам ревью, покрыты тестами и подтверждаются рантаймом.
Root-cause F1 найден честно (не «перепрогонами до зелёного»): on-demand перезагрузка
`embedding` при тесной VRAM упиралась в рост fit-шага движка (~299 с), устранена
ретенцией `keep` (server) + прогревом при старте; приёмка 3/3 в SLA воспроизвелась и
в независимом прогоне. Remote-контракт §3.5 плана M4 реализован дословно (ключи,
Bearer, rustls только исходящие, chat-роль вне VRAM, очередь/квоты не тронуты).
Десктоп-профиль не изменён (боевой `hds check` ok, embedding/rerank `on_demand`).
Находок уровня F1–F3 из ревью M4 нет; есть три некритичных замечания (§3) — в хвосты.

## 2. Что проверено (рантайм и независимый пересчёт)

| Проверка | Как проверено | Результат |
|---|---|---|
| Полный набор тестов | `cargo test --workspace` (боевые `hds.exe` остановлены, затем возвращены) | **87 наборов / 0 failed** ✅ |
| Формат/линт | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` | чисто ✅ |
| F1 (доки vs факт) | пересчёт `m4_load_f1_r1..r3` и `m4_load_f1_fix_r1..r3` питоном | до фикса max 315,6/315,7/46,5 с (2 из 3 за порогом); после — 34,1/34,0/33,9 с, `sla_ok=true` во всех ✅ |
| F1 (воспроизводимость) | свежий прогон `load5.py --tag m5rev_r1 --parallel 5 --ctx 16384` | 200×5, **p95 33,8 с** ✅ |
| F2/F9 (сокет-покрытие) | код `quota_socket.rs` (`unauthorized_heavy_rejected_before_queue`, `aborted_client_releases_queue_slot`) + тесты зелёные | auth **до** `queue.acquire`, аудит `unauthorized` сохранён; обрыв клиента освобождает слот ✅ |
| F3 (дефолты/warn) | пробы `hds check --json`: server-дефолты и явный рассинхрон (parallel 5 / inflight 3) | server-дефолт `max_inflight=5`; warn с числами «max_inflight (3) >= llm_server.parallel (5)…»; согласованные дефолты не помечены ✅ |
| F4/F5 (FIFO/notify) | код `queue.rs` (`ahead_of_me` по минимальному `seq`, `notify_all` при постановке) + тест `fifo_within_user_by_seq` | соответствует ревью ✅ |
| F6 (мёртвый путь) | grep `inflight_acquire|InflightGuard|inflight_release` по `crates/` | в коде не встречается (только `user_inflight_*` и документы) ✅ |
| F7 (`Retry-After`) | юнит `queue_retry_after_follows_wait_timeout` (240→120, 300→150, 4→floor 5) | соответствует ✅ |
| F8 (allowlist) | юнит `journal_mode_allowlist_falls_back_to_wal` + живое открытие с `wal2` | фолбэк `wal`, громкое сообщение ✅ |
| T1–T3 remote (конфиг) | пробы `chat_provider`: bare remote → warn (2 подсказки), полный remote → ok | секрет `sk-…` **не печатается** ни в одном поле; напоминание про W9 в msg ✅ |
| T2 (TLS/Bearer) | код `hds-core/src/http.rs` (`parse_url` с IPv6/дефолт-портами, `tls_stream` — rustls webpki-roots, `ServerName::try_from`), `rag.rs` (Bearer при непустом ключе ≠ `-`), юнит `request_url_posts_json_and_parses_reply` | соответствует контракту; TLS только исходящий ✅ |
| T4 remote (воспроизводимость) | свежий прогон `load5.py --tag m5rev_remote --chat-remote fake --remote-latency 3` | 200×5, p95 7,3 с; `remote={requests:5, auth_ok:5}`; **VRAM 1102→1121 МиБ**; в логе песочного llm-host chat-роль не грузится, прогрев embedding/rerank виден; ответы с источниками ✅ |
| Десктоп не сломан | боевой `hds check --json`, `llm_host status` | `chat_provider`/`queue` ok; chat LOADED `keep`, embedding/rerank `on_demand`; `parallel 1` ✅ |
| Чистота после проб | `%ProgramData%\hermes-disk-search` вычищен руками, песочницы — сам стенд | мусора не осталось ✅ |

## 3. Замечания (некритичные; в хвосты, не блокируют)

1. **M5-1 (низкая, косметика).** В warn-сообщении `chat_provider` для remote без
   `base_url` печатается пустая скобка: «чат ВНЕШНИЙ (): …» — лучше «URL не задан».
2. **M5-2 (низкая, функциональная граница).** TLS-клиент — только системные корни
   webpki: API с собственным/корпоративным CA (self-signed/приватный CA) не заработает;
   клиентских сертификатов нет. Для «доверенных API» из контракта достаточно, но при
   появлении реального API стоит зафиксировать в README и учесть в W9/Э6.
3. **M5-3 (низкая, риск).** rustls в `Cargo.lock`/релизных бинарниках: для сборки
   оффлайн-машины заказчика нужен `cargo vendor` (Э8/Э9; риск уже есть в §7 плана M5 —
   продублировать в README сборки).

Отдельно фиксируем (не находка M5, системный хвост): рост fit-шага движка от числа
загрузок в процессе (0,57 → 4,2 → 27,3 → 299 с) никуда не делся — фикс F1 просто убрал
перезагрузки из боевого пути. Любая будущая выгрузка/загрузка (ARB-3 при индексации,
`llm.model_policy: auto`, смена модели) может снова дать минутные паузы → кандидат
`engine-patch` P5.

## 4. Сверка приёмки §5 плана M5 с фактом

| Пункт | Факт |
|---|---|
| test/fmt/clippy зелёные | 87/0; чисто ✅ |
| F1: root-cause + ≥3 прогона max ≤ 300 с | 3/3 (34,1/34,0/33,9) + независимый 33,8 с ✅ |
| F2: мгновенный `401` до очереди | сокет-тест + код `serve_request` ✅ |
| F3: дефолты согласованы, warn о рассинхроне | пробы `hds check` ✅ |
| F4–F8: юнит/сокет-тесты; `wal2` → понятная ошибка | юниты зелёные ✅ |
| Remote: живой прогон при выгруженной chat-роли, ответ с источниками, очередь/квоты прежние, десктоп не изменён | 2 прогона (`m4_load_m5_remote_r1`, `m4_load_m5rev_remote`): 200×5, Bearer 5/5, VRAM ~1,1 ГиБ, источники в ответах ✅ |
| Рантайм-проверки (не только юниты) | `hds check` (`chat_provider`), `llm status`, сокет-тесты на реальном HTTP ✅ |

## 5. Статус хвостов после M5

| Хвост | Статус |
|---|---|
| Рост fit-шага движка (P5) | ⏸ открыт — кандидат в `engine-patch` (см. §3) |
| Анонимизация W9 | ⏸ открыт — без неё `remote` только для доверенных API |
| `remote` на реальном доверенном API | ⏸ по желанию владельца (ключ в `config.yaml`, не коммитить) |
| Чек-лист внешнего HTTPS (M3/M4) | ⏸ открыт — машина с доменом/сертификатом |
| F9 (M3, `Host` не читается) | ⏸ открыт — формулировка `PLAN_M3` §3.1 (PLAN не переписываем) |
| T0.9 (M2-F7 пароль в argv, M2-F9 транскрибация в server) | ⏸ вопросы заказчику |
| Замечания M5-1/M5-2/M5-3 | ⏸ новые (§3) — низкие, в следующие этапы |

## 6. Рекомендации для следующего этапа

1. Выбор этапа — за владельцем: **Э5** (GPU per-role/multi-vendor) по дорожной карте;
   альтернативы — **W9** (анонимизация; снимает ограничение «доверенных API») или
   **P5** (патч движка; убирает системный риск fit-шага).
2. Мелочь M5-1 (косметика msg) — унести в ближайший кодовый этап; M5-2/M5-3 — в
   контекст W9/Э8–Э9.
3. При Э5: с remote-чатом VRAM freed ~7,8 ГиБ — раскладка embedding/rerank на AMD
   (Vulkan) реальнее; спайк CUDA+Vulkan в одном `llm-host` остаётся открытым риском
   (§6 дорожной карты).

## 7. Как воспроизведены проверки

```powershell
# остановить боевые сервисы (держат target\debug\hds.exe), после работ — вернуть
Get-CimInstance Win32_Process -Filter "Name='hds.exe'" | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
& .\target\release\llm_host.exe stop
cargo test --workspace                    # 87 наборов / 0 failed
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
# песочные пробы hds check (HDS_CONFIG):   tools/parity/out/m5_review_probes.py
# независимый пересчёт отчётов стенда:     tools/parity/out/m5_review_recount.py
# повторные прогоны стенда (сначала llm_host stop, потом вернуть):
python tools\parity\load5.py --tag m5rev_r1     --parallel 5 --ctx 16384
python tools\parity\load5.py --tag m5rev_remote --parallel 5 --ctx 16384 --chat-remote fake --remote-latency 3
# разбор лога песочного llm-host remote-прогона: tools/parity/out/m5_review_remote_check.py
```

> Артефакты ревью — `tools/parity/out/m5_review_*.{py,json,txt}` и
> `m4_load_m5rev_*.json` (гитигнор); старые отчёты `m4_load_*.json` не перезаписывались.
