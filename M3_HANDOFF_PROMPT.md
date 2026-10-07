# M3_HANDOFF_PROMPT.md — промт для нового чата (исполнение M3 «Сетевой периметр»)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где лежит детальный план, контракты, правила и грабли.
> Детали задач — `PLAN_M3_NETWORK_PERIMETER.md`; контекст проекта — `PLAN_SERVER_MULTIUSER.md`;
> код-ревью предыдущего этапа — `M2_CODE_REVIEW.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`** (в ней уже
сделаны M1 и M2 — `feat(access)` … `docs(server): M2 complete`). Станция: Windows,
RTX 3060 12 ГБ (+ AMD-карта), rustc/cargo 1.97.1. Ядро на **Rust** (`crates/`), Python —
только `sidecar`. Тесты: `cargo test --workspace` (сейчас зелёные).

## Что делаем (M3, сетевая часть этапа Э2)
Реализуем milestone **M3 «Сетевой периметр»**: **TLS/reverse-proxy** (шаблоны Caddy/nginx
+ учёт заголовков прокси), **аудит** событий доступа (JSONL в каталоге состояния),
**rate-limit** по токену/по IP на MCP и UI, **сетевой hardening** (лимит тела, предел
соединений), и — первым делом — **быстрые фиксы дефектов M2** из код-ревью (трек T0).
Детальный план — **`PLAN_M3_NETWORK_PERIMETER.md`** (читать целиком: §3 контракты, §4 треки
T0–T5, §6 порядок, §5 приёмка).

## Точка входа (10 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -12`
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`   # должно быть чисто
3. Прочитать **`PLAN_M3_NETWORK_PERIMETER.md`** (полностью) → `M2_CODE_REVIEW.md` →
   `PLAN_SERVER_MULTIUSER.md` (§2 W2, §5 Э2, §8.4) → `README_SERVER_MULTIUSER.md`.
4. Прочитать `tools/parity/HANDOFF_PROMPT.md` (грабли окружения — §«Грабли»).
5. `cargo build --workspace` и `cargo test --workspace` — убедиться, что база зелёная.

## Дефекты M2, которые надо закрыть первыми (T0) — из `M2_CODE_REVIEW.md` §5
1. **F1 — `DisplayName` службы Windows:** `crates/hds-cli/src/cmd/service.rs` (~строка 239)
   строка `"DisplayName= \"Hermes Disk Search: <role>\""` содержит **литерал** `<role>`;
   нужно `format!("DisplayName= \"Hermes Disk Search: {role}\"")`. Тест проверял только binPath.
2. **F4 — `run_ui.ps1` жёстко фиксирует порт 8765** и передаёт `--port 8765`, отменяя
   чтение `ui.port` из конфига (T3). Взять host/port из конфига (или не передавать флаг).
3. **F3 — `HDS_ROOT` не учитывается в `llm-host`:** `hds-llama::host::{resident_data_root,
   HostConfig::default}` читают только `profile: server`; зеркально добавить приоритет `HDS_ROOT`
   (как в `hds-core::config::state_dir`).

## Ключевые решения (заказчик, 07.10.2026; полнее — `PLAN_SERVER_MULTIUSER.md`)
- Периметр — **reverse-proxy с TLS**; наружу только `/mcp` и `/ui`; фасад `:8010–8012` — localhost.
- **UI — только админ**; пользователи ходят через Cline/Hermes (MCP); аутентификация — локальные токены.
- Индексацией занимается **только админ**; отправка во внешний API допустима (анонимизация — позже).
- Десктоп-профиль (`profile: desktop`, дефолт) **не ломаем**: новые фичи по умолчанию выключены.

## Контракты (кратко; детали — план §3)
- Прокси-образцы `installers/proxy/{Caddyfile,nginx.conf}`; `trusted_proxy` в конфиге
  решает, верить ли `X-Forwarded-For/Proto`; без него заголовки игнорируются.
- Аудит: `hds-core::audit`, JSONL `state_dir/data/logs/audit.jsonl`, ротация по размеру;
  конфиг `audit: {enabled,path?,max_mb?}`; server — вкл., desktop — выкл.
- Rate-limit: token-bucket, ключ токен/IP, `429`+`Retry-After`; `/auth/login` жёстче;
  конфиг `rate_limit: {...}`; server — разумные дефолты, desktop — выкл.
- Hardening: лимит тела (`http.max_body_kb`→`413`), предел соединений (`http.max_conns`),
  единый JSON ошибок `{error,hint}`.

## Порядок работы (план §6)
T0 (фиксы) → T1 прокси/TLS → T2 аудит → T3 rate-limit → T4 hardening → T5 доклад.
По одному коммиту на шаг (`fix(service): …`, `fix(ui): …`, `fix(paths): …`,
`feat(proxy): …`, `feat(audit): …`, `feat(ratelimit): …`, `feat(http): …`, `docs(server): …`).
После каждого шага — тесты.

## Грабли окружения (кратко; полнее — tools/parity/HANDOFF_PROMPT.md)
- `git` — **всегда** `--no-pager`. PowerShell портит первый токен команды; кириллица в
  `Select-String` **не ищется** — читать файл инструментом.
- `config.yaml` — **в `.gitignore`**; правки боевого конфига/индекса — только по решению.
- Запущенные `llm_host.exe`/`hds mcp-http`/`hds watch` держат `target\release\*.exe` →
  сборка падает «отказано в доступе»; на dev-машине их можно остановить
  (`cline-rules/dev-machine.md`), кроме чужих процессов.
- `HDS_CONFIG`/`HDS_ROOT` персистентны в сессии PowerShell — изоляция тестов через них.
- Полный SCM-цикл служб — только из-под администратора (на этой машине шелл без elevated).

## Правила
- Сначала предложить план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
- Не ломать десктоп-профиль; не трогать чужие незакоммиченные изменения и боевые `PLAN_*.md`.
- Обновлять `PLAN_M3_NETWORK_PERIMETER.md`, `STATUS.md`, `README*.md`.
- Обязательно проверять изменения **в рантайме** (особенно сеть/заголовки/лимиты), а не только юнит-тестами.
