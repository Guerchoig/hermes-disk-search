# README_SERVER_MULTIUSER.md — серверная многопользовательская подсистема (M1–M9)

> Практическое руководство по **всей серверной подсистеме** `hermes-disk-search`:
> проекты и авторизация (**M1**), серверный профиль и службы (**M2**), сетевой периметр —
> TLS/аудит/rate-limit (**M3**), конкурентность под SLA (**M4**), внешняя чат-модель (**M5**),
> несколько GPU (**M6**), управление службами/токенами из UI (**M7**), серверный пакет
> установки (**M8**), клиентский пакет (**M9**). Ветка **`server-multiuser`**.
> **Все этапы выполнены и покрыты тестами** (код-ревью M2–M7; карта — §0.1).
>
> Планы и журналы этапов — `PLAN_M1_ACCESS_PROJECTS.md` … `PLAN_M9_CLIENT_PACKAGE.md`;
> дорожная карта — `PLAN_SERVER_MULTIUSER.md` (§2 workstreams, §5 этапы, §9 статусы).
>
> **Осталось (вне M1–M9):** эксплуатация — healthz/бэкап/диагностика (Э10/W8); нагрузочная
> приёмка на 1/2 картах и Apple Silicon (Э11); анонимизация внешнего API (W9); живая проверка
> HTTPS через TLS-прокси на машине с сертификатом. Подробнее — §12.

## 0. Что появилось (кратко)

* **Проект** — единица разграничения доступа. Каждый проиндексированный файл принадлежит
  одному проекту (по корню индексации с наибольшим совпадающим префиксом).
* **Пользователи** — логин + пароль (Argon2id), выдаёт админ. Хранилище — `data/access.yaml`
  (образец: `data/access.example.yaml`; сам файл в `data/` — не коммитится).
* **Токены** — opaque 256 бит; хранятся хэшем sha256; TTL 90 дней (`access.token_ttl_days`).
  Выдаются **админом** (`hds token new` / кнопка «Выдать токен» в UI) или **самим пользователем**
  через логин (`POST /auth/login` / `hds login`) — см. §3.
* **Гейт** — `access.enabled: true` в `config.yaml`. **Выключен (по умолчанию) = десктоп**:
  прежнее поведение без токенов и без фильтра. Ничего не ломается для одиночной машины.
* **Фильтрация** — в server-режиме поиск/RAG отдают только файлы проектов токена
  (все ветки: ключевой поиск, семантика, CLIP, RAG-цитаты). Админ видит всё.
* **Admin-гейтинг** — индексацией (`start_indexing`/`stop_indexing`/`reindex_path`)
  занимается только админ; пользователи — никогда.
* ⚠️ **Переиндексация обязательна**: схема БД изменилась (`files.project`); старая база
  дропается при первом подключении (сообщение в stderr) — запустите `hds index` заново.

## 0.1. Карта этапов (что где)

| Этап | Что сделано | Раздел | План |
|---|---|---|---|
| **M1** | Проекты (roots→`files.project`), Argon2id-пароли, авто-токены, Bearer, фильтрация выдачи, admin-гейтинг индексации | §0–§8, §10 | `PLAN_M1_ACCESS_PROJECTS.md` |
| **M2** | Профиль `desktop\|server`, каталог состояния (ProgramData / Application Support), службы Windows/macOS (`hds service`), headless-UI, установщики | §2a | `PLAN_M2_SERVER_PROFILE.md` |
| **M3** | TLS reverse-proxy (образцы), аудит JSONL, rate-limit, hardening (`413`/`503`, fail-closed UI) | §11 | `PLAN_M3_NETWORK_PERIMETER.md` |
| **M4** | БД под конкурентность (WAL + read-only), слоты чата + fit-гейт VRAM, очередь тяжёлых RAG, квоты по пользователю | §11 | `PLAN_M4_CONCURRENCY_SLA.md` |
| **M5** | Внешняя чат-модель `chat.provider: remote` (RAG без локальной chat-роли в VRAM) | §8a | `PLAN_M5_EXTERNAL_CHAT.md` |
| **M6** | Несколько GPU одного семейства: `gpu.preset`/`gpu.devices`, чат-реплики, whisper по картам | §8b | `PLAN_M6_GPU_MULTIVENDOR.md` |
| **M7** | Управление службами и выдача токенов из UI (`GET/POST /api/services`, `hds token new`) | §8c | `PLAN_M7_UI_CONTROL.md` |
| **M8** | Серверный пакет установки (unattended `install_server.*`, `update.ps1` + откат) | §8d | `PLAN_M8_INSTALL_PACKAGES.md` |
| **M9** | Клиентский пакет (тонкий Cline-клиент: `cline-sync --server-url/--token`, мастеры, тонкая сборка) | §8e | `PLAN_M9_CLIENT_PACKAGE.md` |

## 1. Роли

| Роль | Что может |
|---|---|
| **Админ** (`admin: true` в `data/access.yaml`) | всё: поиск без фильтра, индексация, управление пользователями/токенами, UI |
| **Пользователь** | поиск/RAG только по своим проектам (`projects:` в `data/access.yaml`); никаких управляющих инструментов; **свой UI** (`/ui/user/`): поиск, установка клиента, транскрипты/спикеры |
| **Десктоп** (нет `access.enabled`) | одиночная машина, как раньше: без логинов, всё видно |

Пустой список `projects` у обычного пользователя = **не видит ничего** (защита «по умолчанию
запрещено»). У админа пустой список означает «видит всё».

### 1a. Два гейта UI и свой кабинет пользователя (R4 `PLAN_TRANSCRIBE_SERVICE.md`)

| Что | Кто пускается | Ответ без токена | Ответ не-админу |
|---|---|---|---|
| Админская страница `/` (и `/ui/`) | только **админ**-токен | `401` | `403` |
| Админские API (`/api/config`, `/api/services`, `/api/access/*`, `/api/index/*`, `/api/transcribe/daemon`, `/api/ask`) | админ | `401` | `403` |
| Кабинет `/ui/user/` и «безопасные» API (`/api/user/*`, `/api/search`, `/api/transcribe/{list,file,speakers,apply}`) | **любой валидный** токен (или вход по логину/паролю) | `401` | — (разрешено) |

* Вход в кабинет: `POST /api/auth/login {"login","password"}` → `{"token","expires","user","projects","admin"}`
  (`401` неверные данные, `400` битый JSON, `429` при антибрутфоре `login_per_min` по IP).
  Тело/ответ/коды — те же, что у `hds login` и у MCP `POST /auth/login` (§10).
* Страница читает `?token=...` (как админская) и хранит токен в localStorage браузера.
* Не-админ в кабинете может **и править `.md`, и присваивать имена спикерам** (решение
  владельца §8 №5): копия `.bak` и sidecar — в каталоге сервисных файлов; имена с путём
  (`..`, абсолютные, `:`) отклоняются (path-safety внутри `out_dir`).
* Токен в кабинете не даёт ничего админского: `403` возвращается **до** выполнения операции,
  отказ пишется в аудит (`forbidden`), как и `401` (`unauthorized`).


## 2. Сценарий A — первичное включение server-режима (админ, один раз)

```powershell
# 0) Переиндексация после обновления (схема БД изменилась — согласовано в M1):
#    при первом подключении старая index.db дропается автоматически (см. stderr),
#    запускаем полный прогон:
hds index --full

# 1) Включаем доступ и объявляем корни-проекты в config.yaml:
#      profile: server
#      access:
#        enabled: true
#        token_ttl_days: 90
#      index:
#        roots:
#          - "E:\Archive"                                   # проект "Archive" (младшая папка)
#          - { path: "D:\Work\Sales", project: "Sales" }    # явное имя
#          - { path: "F:\Reports",    project: "Sales" }    # тот же проект, другой корень
#          - "D:\"                                          # весь диск → проект "D:"
#    Корни правятся и из UI («Настройки → Сохранить корни»).
#    Проект по умолчанию = имя младшей папки; для корня диска — метка с двоеточием (D:\ → D:).

# 2) Создаём первого админа:
hds user add --login admin --password "<надёжный пароль>" --admin

# 3) Проверяем себя:
hds user list
hds login --login admin --password "<пароль>" --url http://127.0.0.1:8787
#    → {"token":"<64 hex>","expires":<unix>,"user":"admin","projects":[]}

# 4) Проверяем гейт (токен обязателен):
#    без токена:  curl http://127.0.0.1:8787/mcp -X POST ... → 401
#    /health остаётся публичным — это маркер «наш» инстанс для менеджера.
```

После шага 1–2 любой запрос к MCP/UI без валидного токена получает `401`; админская страница
UI открывается только с админ-токеном (не-админу — `403`), а **кабинет** `/ui/user/` доступен
любому валидному токену — включая вход по логину/паролю (`POST /api/auth/login`), см. §1a и §6.

## 2a. Серверный профиль, каталог состояния и службы (M2)

* **Профиль** — ключ `profile: desktop | server` в `config.yaml` (по умолчанию `desktop`).
  Профиль **сам авторизацию не включает** (её включает только `access.enabled`); `server`
  задаёт серверные дефолты (`llm_server.parallel: 5`, `db.*`, очередь/квоты/аудит — включены) и
  предупреждения `hds check` (server без `access.enabled`/`ui.*` → `warn` с подсказками).
* **Каталог состояния (server):** `%ProgramData%\hermes-disk-search` (Windows) /
  `/Library/Application Support/hermes-disk-search` (macOS). Там `config.yaml` (фолбэк, если
  конфига нет в корне установки), `index.db`, `data/` (`access.yaml`, PID `mcp-http`/`llm-host`,
  логи, heartbeat, `mcp-queue.json`). Переопределение — `HDS_ROOT` (сильнее всего). Desktop —
  прежнее поведение (всё рядом с программой).
* **Bind веб-интерфейса:** секция `ui: { host, port }` (по умолчанию `127.0.0.1:8765`); флаги
  `hds ui --host/--port` переопределяют. В server-профиле держите `127.0.0.1`, наружу — только
  через reverse-proxy с TLS (§11).
* **Службы** (старт при загрузке, без интерактивного входа):
  `hds service run|install|uninstall|start|stop|status <role>`, роли
  `watch | mcp-http | ui | llm-host`. Windows — SCM (`hermes-disk-search-<role>`, автостарт +
  авторестарт при падении; `run` под SCM обслуживает крейт `windows-service`), фолбэк —
  AtStartup-задача SYSTEM; macOS — LaunchDaemon (`com.hermes-disk-search.<role>`, `RunAtLoad` +
  `KeepAlive`). Управляющие подкоманды требуют прав администратора. **Десктопный** автозапуск
  (Планировщик `AtLogOn`) не меняется.
* **Headless-UI (server):** `run_ui.ps1` в server-профиле не открывает браузер и не создаёт ярлыки;
  вход — строго по админ-токену (§6).
* **Установка** — одной командой: `install-server.cmd` (Windows, из корня распакованного архива) /
  `installers/install_server.ps1` / `installers/install_server.command` (macOS) — см. §8d.
  **Обновление** — `update.cmd` (Windows) / `installers/update.command` (macOS); они сами находят
  корень установки и версию, ручные `-Root/-Version` не нужны. Управление службами из UI — §8c.

## 3. Сценарий B — подключение нового пользователя (админ → пользователь)

У пользователя **две независимые возможности** получить токен:

| Способ | Кто и где | Когда применять |
|---|---|---|
| **Токен от администратора** (основной) | админ на сервере: `hds token new --user alice` (или кнопка «Выдать токен» в UI) | всегда; **обязателен** для удалённого клиента вне доверенной сети — пароль не нужен |
| **Самостоятельный логин** (доп.) | пользователь: `hds login --login alice --password … --url http://<сервер>:8787` | доверенная **локальная** сеть, где `/auth/login` доступен по http |

**Админ:**

```powershell
# 1) создаёт учётку (пароль нужен только для самостоятельного логина, см. ниже):
hds user add --login alice --password "<начальный пароль>" --projects "Sales,Docs"

# 2) выписывает токен и передаёт его пользователю по защищённому каналу:
hds token new --user alice
# → plain-токен (показывается ОДИН раз) + срок; в data/access.yaml — только token_sha256

# (позже — изменить доступ / пароль:
hds user grant  --login alice --projects "Sales,Docs,Ops"   # добавить Ops
hds user revoke --login alice --projects "Docs"             # убрать Docs
hds user passwd --login alice --password "<новый>"          # смена пароля + отзыв ВСЕХ токенов alice
)
```

**Пользователь:**

* **Вне доверенной сети (штатный путь):** получает токен от админа и **вставляет его в клиент**
  (см. §8e) — как заголовок `Authorization: Bearer <токен>` в настройках MCP. Логин/пароль
  не вводит.
* **В доверенной сети (self-service):** может получить/обновить токен сам:

```powershell
hds login --login alice --password "<пароль>" --url http://<сервер>:8787
# → {"token":"<64 hex>","expires":1798761600,"user":"alice","projects":["Sales","Docs"]}
```

⚠️ **Ограничения самостоятельного логина (важно):**

* `hds login` ходит по **plain HTTP** (в CLI нет TLS) — используйте `http://…:8787` в доверенной
  сети; **через `https`-прокси команда не сработает**;
* образцы reverse-proxy (`installers/proxy/`) публикуют наружу **только `/mcp` и `/ui`** —
  `POST /auth/login` наружу **не выставлен** (добавьте маршрут явно, если он нужен);
* эндпоинт `/auth/login` активен только при `access.enabled: true` (в desktop-режиме — `404`);
* лимит по IP (`rate_limit.login_per_min`, антибрутфорс) + аудит `login_ok`/`login_fail`.

Токен показывается **один раз** (в `data/access.yaml` лежит только `token_sha256`); TTL — 90 дней
(`access.token_ttl_days`). Если потерян/истёк — админ выписывает новый (`hds token new`), либо
пользователь логинится заново (в доверенной сети).

## 4. Сценарий C — настройка клиентов (Cline / Hermes)

В настройках MCP-сервера клиента (`streamableHttp`) — URL и заголовок:

```json
{
  "mcpServers": {
    "disk-search": {
      "type": "streamableHttp",
      "url": "https://hds.example.com/mcp",
      "headers": { "Authorization": "Bearer <токен пользователя>" },
      "autoApprove": ["search_local_files", "ask_my_files", "index_status"],
      "timeout": 300
    }
  }
}
```

* Токен для `headers` выдаёт **админ** (§3, `hds token new`); логин/пароль клиенту **не** нужны.
* В `autoApprove` для обычного пользователя **не** давать управляющие инструменты —
  они всё равно вернут ошибку «требуются права администратора» (гейтинг на сервере).
* Проверка токена — на каждом вызове: истёкший/отозванный токен → `401` сразу.

## 5. Сценарий D — повседневная работа пользователя

Пользователь работает в Cline/Hermes как раньше — инструментами MCP:

* `search_local_files(query, kinds, limit)` — «найди на этом компе…»: сервер сам
  подставляет фильтр проектов токена (`f.project IN (...)`) во все ветки поиска —
  файлы чужих проектов **не видны ни в каком виде** (пути, сниппеты, цитаты).
* `ask_my_files(question, limit)` — RAG-ответ: фрагменты и цитаты только из его проектов.
* `index_status` — доступен всем (состояние индекса без содержимого файлов).
* `start_indexing` / `stop_indexing` / `reindex_path` — вернут
  «требуются права администратора» (в server-режиме).

Поведение при пустой выдаче не отличается от обычного «ничего не найдено» — утечки
через сообщения об ошибках нет.

## 6. Сценарий E — администрирование через UI

UI открывается только с админ-токеном:

```
http://127.0.0.1:8765/?token=<админ-токен>
```

Токен из `?token=` запоминается в браузере (localStorage) и автоматически подставляется
во все запросы страницы. Без токена — `401` на странице и на каждом API-вызове; валидный
**не-админский** токен на админском API/странице получает `403` (§1a).

### Кабинет пользователя `/ui/user/`

Отдельная страница для не-админов — по ссылке `https://<сервер>/ui/user/` (за прокси) или
`http://127.0.0.1:8765/ui/user/` (локально). Три блока:

1. **Поиск по файлам** — выдача фильтруется проектами токена (то же, что MCP-инструмент,
   но в браузере);
2. **Установка клиента (Cline)** — ссылка на публичный скрипт `<сервер>/mcp/client-install.ps1|sh`,
   готовые команды с токеном пользователя, адрес MCP из конфига и ссылка на `/health`
   (сверка версии сервера/клиента);
3. **Транскрипты и спикеры** — список выходов `out_dir`, превью и присвоение имён спикерам
   (замена в заголовках реплик, `.bak` в каталоге сервисных файлов).

Вход — логин/пароль (`POST /api/auth/login`), либо админ уже выдал токен: тогда страница
открывается как `/ui/user/?token=<токен>`. В десктоп-режиме (`access.enabled: false`) кабинет
открыт локально без входа.

Карточка **«Доступ (пользователи и проекты)»**:

* список пользователей (логин, админ/не админ, проекты) + список «корень → проект»
  из `index.roots` конфига;
* создание пользователя (логин, пароль, проекты через запятую, флаг «админ»);
* удаление пользователя (его токены отзываются автоматически);
* через API (для скриптов, тем же админ-токеном):
  `GET /api/access/users`, `GET /api/access/projects`,
  `POST /api/access/user/add|del|passwd|grant`, `POST /api/access/token/revoke`.

В карточке «Доступ» кнопка **«Выдать токен»** (логин + TTL) выпускает новый токен —
plain-значение показывается **один раз**. Карточка **«Службы (server-профиль)»** показывает
состояние OS-служб и кнопки `install/uninstall/start/stop/restart` (детали — §8c). Установка
серверного пакета одной командой — §8d; клиентского (тонкий Cline) — §8e.

Остальные карточки UI (индексация, llm-host, конфиг, транскрибация) — админские операции,
в server-режиме они защищены тем же гейтом.

## 7. Сценарий F — ротация и отзыв доступа

| Действие | Команда (админ) | Эффект |
|---|---|---|
| Посмотреть токены | `hds token list [--user alice]` | префикс хэша, владелец, срок, «ПРОСРОЧЕН» |
| Отозвать один токен | `hds token revoke --sha <префикс из list>` | этот токен → 401 сразу |
| Отозвать все токены юзера | `hds token revoke --user alice` | alice перелогинивается |
| Сменить пароль | `hds user passwd --login alice --password <новый>` | пароль + **все токены alice** отзываются |
| Убрать проект | `hds user revoke --login alice --projects Docs` | файлы Docs исчезают из выдачи немедленно |
| Отключить учётку | — | отдельной команды нет (флаг `disabled` есть в схеме `access.yaml`); используйте `hds user del --login alice` |
| Удалить пользователя | `hds user del --login alice` | учётка + токены удаляются |

Истёкший по TTL токен перестаёт действовать сам (проверка `expires` при каждом вызове);
`hds login` выдаёт новый.

## 8. Портрет отказов

| Ситуация | Что видит клиент |
|---|---|
| Нет/неверный/истёкший/отозванный токен на MCP | HTTP `401` (`{"error":"unauthorized","hint":"POST /auth/login"}`) |
| Неверный логин/пароль на `/auth/login` | HTTP `401` «неверный логин или пароль» |
| Не-админ вызывает управляющий инструмент | isError-контент «требуются права администратора» |
| Пользователь без проектов ищет что-то | обычное «ничего не найдено» (пустой список = ничего не видно) |
| UI без админ-токена | страница и API — `401` с подсказкой про `?token=` |
| Превышен лимит частоты | HTTP `429` + `Retry-After` + `X-RateLimit-Limit`/`-Remaining`/`-Reset` (ключ — пользователь, иначе IP) |
| Очередь тяжёлых RAG полна (`queue.max_depth`) | HTTP `429` + `Retry-After: 5` + `X-Queue-Depth`; hint — про `queue.max_depth` |
| Превышен пер-пользовательский предел RAG (`rate_limit.max_inflight_per_user`) | HTTP `429` + `Retry-After` + `X-Queue-Depth: 0`; hint — про `rate_limit.max_inflight_per_user` |
| Не дождался слота за `queue.wait_timeout_sec` | HTTP `503` + `Retry-After` (половина `wait_timeout_sec`, мин 5); hint — про `queue.wait_timeout_sec` |
| Десктоп (`access.enabled` нет) | ничего из вышеописанного не применяется |

Дополнительно (M5): неавторизованный тяжёлый запрос при занятых слотах получает
`401` **мгновенно** — очередь не занимает (закрыт вектор DoS из ревью M4 F2).

## 8a. Внешний chat-provider `remote` (M5)

RAG-ответ можно отдать внешнему OpenAI-совместимому API — тогда `llm-host` не
держит chat-модель в VRAM (освобождается ~8 ГиБ на RTX 3060 — «рычаг SLA»):

```yaml
chat:
  provider: remote          # local (дефолт) | remote
  base_url: "https://api.example.com/v1"
  api_key: "sk-..."         # Bearer-ключ; "-" = без ключа; в git не входит
  model: "gpt-4o-mini"
```

* Поиск/эмбеддинги/реранкер и фильтрация по проектам остаются локальными —
  наружу уходит только собранный промпт (вопрос + фрагменты).
* ⚠ **Анонимизация (W9) ещё не реализована**: текст вопроса и фрагменты
  локальных файлов уходят провайдеру как есть — включайте `remote` только для
  доверенных API.
* Проверка: `hds check` (пункт `chat_provider`), `llm-host status` (chat-роль
  не поднимается; предупреждение в warnings).

## 8b. Несколько карт одного семейства (M6)

Прод-сервер может иметь **несколько дискретных GPU одного семейства** (NVIDIA под
Windows или AMD под Windows/Vulkan; на macOS — один Metal). Смешанные вендоры и
iGPU/UMA **не поддерживаются**. Раскладка — пресетом или явно:

```yaml
profile: server
gpu:
  preset: nvidia-n              # nvidia-one | nvidia-n | amd-n | apple (| none)
  # devices:                    # явная раскладка сильнее пресета
  #   - { index: 1, roles: [chat, whisper] }
  #   - { index: 2, roles: [chat-2, whisper-2] }
llm_server:
  parallel: 5
  chat:   { port: 8010, model: "shared:chat" }
  chat-2: { port: 8030, model: "shared:chat" }   # реплика — та же модель/контекст
  embedding: { port: 8011, model: "shared:embedding" }   # один экземпляр на машину
  rerank:    { port: 8012, model: "shared:rerank" }      # один экземпляр (на CPU — ок)
llm:
  whisper:   { model: "…whisper-large-v3-turbo-GGML.bin" }   # экземпляр на карту
  whisper-2: { model: "…whisper-large-v3-turbo-GGML.bin" }
```

Что это даёт и как проверять:

* **Пользователи** ходят в один фасад `chat` → `:8010`; хост раскидывает запросы по
  репликам (least-busy → round-robin). Порты реплик (`8010+N`) наружу не публикуются.
* **Индексация** — ровно один экземпляр (`embedding`+`rerank`); реплика `embedding-2`
  → **ошибка конфига** (fail-fast с числами).
* **Транскрибация** — экземпляр на карту, один файл за раз; число карт и роли видны в
  логе старта (`транскрибация по картам: …`) и в `cards`.
* Проверки: `llm_host status` (строки `cards`: карта / backend / свободно / роли),
  `hds check` (пункт `gpu_roles` — карты, роли, уместный пресет, warn на смешанные
  вендоры/UMA), `llm_host_plan` (сухой прогон раскладки с числами).
* ⚠ **Ограничение:** host-выбор карты под файл (`pick_whisper_card`) срабатывает
  только когда клиент **не** шлёт поле `gpu`; штатные `hds whisper-check`/
  `auto_transcribe` его шлют (см. `PLAN_M6_GPU_MULTIVENDOR.md` §8a, T5 / F-m6-t5-1).
  На одной карте разницы нет; на N картах — учитывать.

## 8c. Управление службами и токенами из UI (M7)

Карточки Rust-UI в server-профиле (доступ — с админ-токеном, см. §6).

**«Службы (server-профиль)»** — состояние 4 OS-служб (`watch | mcp-http | ui | llm-host`)
и кнопки `install | uninstall | start | stop | restart`:

* API: `GET /api/services` → `{profile, services:[{role,kind,state,autostart,detail}], hint}`
  (`kind = service|absent`, `state = running|stopped|absent|unknown`);
  `POST /api/services {role, action}` → `{ok, msg, state, hint?}`.
* Требует прав администратора (или UI, запущенного как служба `LocalSystem`). Нет прав →
  `{ok:false, msg:"…Access denied…", hint:"запустите UI от администратора или из службы LocalSystem"}`.
* Служба не установлена (`sc` код 1060) → `{ok:false, msg, hint:"…сначала install", state}`.
* `restart` = stop + start; для роли `ui` (UI перезапускает сам себя) ответ отдаётся сразу,
  перезапуск выполняется отложенным helper'ом — страница не виснет.
* **desktop-профиль:** `services: []` + `hint`, кнопок нет, действия отклоняются.

**«Выдать токен»** (карточка «Доступ») — plain-токен показывается **один раз**:

* CLI: `hds token new --user <login> [--ttl-days N]` → печатает plain-токен и срок;
  для несуществующего/отключённого логина — отказ (exit 1);
* UI: `POST /api/access/token/new {user, ttl_days?}` → `{token, expires, user}`;
* в `data/access.yaml` пишется только `token_sha256`; в аудит — событие `admin_op` **без** токена;
* отзыв — `hds token revoke …` / `POST /api/access/token/revoke` (§7).

**Проверено на живом стенде M7** (`PLAN_M7_UI_CONTROL.md` §4, T5): полный SCM-цикл по всем
4 ролям **из-под администратора** (install→start→status→stop→uninstall) со сверкой
`/api/services` ↔ `sc query`; выдача/отзыв токена живым запросом, `401` без токена, аудит без
plain-токена. Практические находки: `sc.exe create` принимает `binPath` только в «классическом»
виде (`binPath= "\"<exe>\" service run <role>"` — в коде через `raw_arg`), а `sc qc` локализован
(метка `Тип_запуска`) — состояние/автозапуск читаются по значениям, а не по меткам.

## 8d. Установка серверного пакета (M8)

Серверный пакет ставится **одной командой** (формат поставки — zip + скрипты, MSI/`.pkg` нет).
Установщики **unattended** (параметры вместо приглашений); без новых параметров — прежнее
поведение M2. Требуется администратор (Windows/SCM) или `sudo` (macOS/LaunchDaemon).

**Windows** (из-под администратора):

```powershell
# Обёртка из корня распакованного архива (снимает MotW и зовёт install_server.ps1):
install-server.cmd -WhatIf

# План + тело конфига — без изменений и без админа:
installers\install_server.ps1 -WhatIf -StateDir C:\ProgramData\hermes-disk-search `
  -McpPort 8787 -UiPort 8765 -Parallel 5 -GpuDevice 1 -Roots "D:\;E:\Archive" `
  -EnableAccess -AdminLogin admin -AdminPassword <secret>

# Реальная установка (службы SCM, автозагрузка; фолбэк — AtStartup-задача SYSTEM):
installers\install_server.ps1 -StateDir C:\ProgramData\hermes-disk-search -Parallel 5 `
  -Roots "D:\;E:\Archive" -EnableAccess -AdminLogin admin -AdminPassword <secret> -Fetch
```

**macOS** (под `sudo`; LaunchDaemon `com.hermes-disk-search.<role>`):

```bash
sudo bash installers/install_server.command \
  --statedir "/Library/Application Support/hermes-disk-search" \
  --parallel 5 --roots "D:\;E:\Archive" --enable-access --admin-login admin --admin-password <secret> --fetch
```

* **Паритет параметров** (Win ↔ macOS): `-StateDir`↔`--statedir`, `-McpHost/-McpPort`↔`--mcp-host/--mcp-port`,
  `-UiHost/-UiPort`, `-Parallel`, `-GpuDevice/-GpuPreset`, `-Roots`, `-EnableAccess`,
  `-AdminLogin/-AdminPassword`, `-Fetch/-NoFetch`, `-WhatIf`↔`--whatif`.
* **`-WhatIf`/`--whatif`** — печатает план и тело `config.yaml` **без изменений и без админа**;
  **`-Roles ""`/`--roles ""`** — config-only (без служб и без админа).
* Генерируется server-`config.yaml` (`profile: server`, `access.enabled`, `llm_server.parallel`,
  `gpu.device_index`/`gpu.preset`, `index.roots`, `mcp_http`/`ui`); существующий конфиг
  **не затирается** (идемпотентно). Первый админ — `hds user add --admin` (пароль только
  хэшем argon2id, plain не в логах/аудите); токен — `hds token new --user admin`.
* `-Fetch` вызывает `fetch_engine_runtime.ps1`/`fetch_llm_models.ps1`/`fetch_whisper_model.ps1`/
  `fetch_clip_models.ps1` (они unattended, без `Read-Host`; на macOS — через `pwsh`).
* **Обновление/откат** (`installers\update.ps1`): `-Profile server -From <stage> -Version <ver>`
  останавливает службы (`ui→mcp-http→watch→transcribe→llm-host`, ожидание `STOPPED`), подменяет
  `app\current`, стартует (`llm-host→transcribe→watch→mcp-http→ui`) и проверяет `/health`;
  `-RollbackTo <ver>` возвращает прошлую версию; `-ListVersions` — список. **desktop**-профиль —
  прежний путь (задачи Планировщика + резидент).
* **Обёртки релизного архива** (0.20.1) — то же без ручных `-Root/-From/-Version` и без борьбы с
  execution policy: `install-server.cmd` (первичная установка), `update.cmd` (обновление) на
  Windows, `installers/update.command` на macOS (нужен `pwsh`). Они снимают MotW/карантин, берут
  корень установки из `bin\hds.exe service root` (читается из `binPath` SCM-службы или из плейста
  LaunchDaemon) и версию из `version.txt`; политика выполнения применяется только к `.ps1`, поэтому
  `.cmd`-обёртка запускается всегда. Явные `-Root`/`-WhatIf`/`-Force`/`-RollbackTo` пробрасываются
  в `update.ps1` как есть.
* **TLS reverse-proxy** — вручную: образцы `installers/proxy/{Caddyfile,nginx.conf}` (наружу
  только `/mcp` и `/ui`), затем `trusted_proxy: true`. Автоустановка Caddy/nginx не входит.
* **Проверено на живом стенде (10.10.2026):** `install_server.ps1` ставит 4 службы SCM
  (`kind=service`, автозапуск); `GET /api/services` (админ-Bearer) совпадает с `sc query`;
  MCP-гейт — без токена `401`, с токеном `200`; `update.ps1 -Profile server` + `-RollbackTo`
  без «отказано в доступе» (health `200`). **Среда:** `watch` требует `index.roots` **и**
  sidecar-рантайм (`.venv\Scripts\python.exe`/`sidecar/python`), `llm-host` — моделей/движка;
  без них эти роли не удерживаются (на реальной поставке их даёт `-Fetch` + корни). Службы
  ставятся с **автозапуском** и не стартуют немедленно — старт при загрузке или
  `hds service start <role>`.
* Приёмка M8 (песочницы + UAC-хелпер `m8_t5_scm.ps1`) — `PLAN_M8_INSTALL_PACKAGES.md` §4/§5.

## 8e. Установка клиентского пакета (M9)

**Тонкий клиент** подключает Cline (Desktop/CLI) к **удалённому** серверу disk-search и
ничего тяжёлого не ставит (ни движка, ни моделей, ни индекса, ни sidecar) — вся работа
на сервере. Формат поставки — zip + скрипты; права администратора **не** нужны.

**Быстрый путь (рекомендуемый): три шага.** Админ выдаёт токен → пользователь скачивает
**скрипт-установщик из общедоступного места** (токен для скачивания не нужен) → запускает его
с токеном; скрипт сам скачает клиентский пакет под платформу клиента, распакует, снимет
блокировку/карантин и подключит Cline.

```powershell
# 1) на сервере (админ): токен для клиента
hds token new --user laptop --ttl-days 90          # plain-токен показывается один раз

# 2) на клиенте (права админа не нужны): скрипт с сервера или ассетом релиза
curl.exe -f https://host/mcp/client-install.ps1 -o client-install.ps1

# 3) запуск с токеном
powershell -ExecutionPolicy Bypass -File .\client-install.ps1 -Token <ТОКЕН> -ServerUrl https://host
# macOS:  curl -f https://host/mcp/client-install.sh -o client-install.sh
#         bash client-install.sh --token <ТОКЕН> --server-url https://host
```

Ниже — тот же путь по шагам (видно, что именно происходит и как сделать вручную).

**Шаг 0 (на клиентской машине): получить клиентский пакет.** Два канала:

* **с сервера** (своим токеном — гейт как у `/mcp`, путь внутри опубликованного
  `/mcp`-префикса, работает и через TLS-прокси):
  ```powershell
  curl.exe -f -H "Authorization: Bearer <ТОКЕН>" -o hds-client.zip https://host/mcp/client-package.zip
  Expand-Archive .\hds-client.zip -DestinationPath .\hds-client -Force
  Get-ChildItem .\hds-client -Recurse | Unblock-File
  ```
  ```bash
  curl -f -H "Authorization: Bearer <ТОКЕН>" -o hds-client.zip https://host/mcp/client-package.zip && \
    unzip -q hds-client.zip -d hds-client && xattr -dr com.apple.quarantine hds-client
  ```
* **ассетом релиза** — `hds-client-<версия>-<платформа>.zip` (тот же файл, что вложен в
  серверную поставку в `client/`);
* (dev) собрать самому из репозитория: `installers/build_client_release.ps1 -Version <версия>`
  / `build_client_release_macos.sh <версия>`.

**Шаг 1 (на сервере): выписать токен** для клиента:

```powershell
bin\hds.exe token new --user <логин>        # plain-токен показывается ОДИН раз
```

**Шаг 2 (на клиентской машине): подключить Cline.**

Windows:
```powershell
.\setup-client.ps1 -ServerUrl https://host/ -Token <bearer>
# или: .\setup-client.cmd -ServerUrl https://host/ -Token <bearer>
```
macOS:
```bash
bash install_client.command --server-url https://host/ --token <bearer>
```

Мастер: проверяет `GET <ServerUrl>/health` (200) → `hds cline-sync --server-url --token` →
просит перезапустить Cline. Флаг `-DryRun`/`--dry-run` показывает план без записи;
`-SkipHealth`/`--skip-health` пропускает проверку соединения.

* **Что делает пользователь с токеном (вне доверенной сети):**
  1. вставляет токен в клиент — мастером (Шаг 2 выше) или вручную в конфиг Cline
     (`headers: { "Authorization": "Bearer <токен>" }`);
  2. указывает адрес сервера по **`https://`** — токен идёт по TLS (по `http://` он открытым текстом);
  3. перезапускает Cline → в списке MCP-серверов появляется `disk-search`;
  4. проверяет связь: `GET https://host/health` → `200` (сам токен сервер проверяет на **каждом**
     вызове `/mcp`: истёк/отозван → `401`);
  5. логин/пароль **не** вводит и `hds login` не запускает (почему — ниже).
* **Почему токен, а не логин:** вне доверенной сети самостоятельный `hds login` неприменим —
  CLI ходит по plain HTTP, а `/auth/login` через прокси по умолчанию не публикуется (детали — §3).
  Поэтому админ выписывает токен на сервере (`hds token new`), а пользователь вставляет его в клиент.
* **Секретность:** токен — как пароль: не пересылайте его по открытым каналам, не публикуйте в
  тикетах/чатах. При утере или истечении TTL (90 дней) админ выписывает новый (`hds token new`);
  старый можно отозвать (`hds token revoke …`, §7).
* **Что пишется:** MCP-сервер `disk-search` в `~/.cline/data/settings/cline_mcp_settings.json`
  и `~/.cline/mcp.json` — с **внешним** URL (`<ServerUrl><mcp-path>`) и заголовком
  `Authorization: Bearer <токен>`; правило `~/.cline/rules/disk-search.md` и скилл
  `~/.cline/skills/disk-search/SKILL.md` (из клиентского пакета). Окна моделей `models.json`
  в клиентском режиме **не** меняются (локального `llm_server` нет). Идемпотентно.
* **Безопасность:** токен передаётся в Cline как заголовок `Authorization`; в логи/аудит он не
  пишется; мастер передаёт его в `hds` через переменную `HDS_CLINE_TOKEN` (не в командной
  строке). Без TLS (`http://`) токен идёт открытым текстом — используйте `https://`.
* **Скрипты-установщики:** `client-install.ps1` / `client-install.sh` — один файл, принимает
  токен аргументом. Сервер отдаёт их **публично** (без токена) по
  `<mcp-path>/client-install.ps1|sh` (они же — ассеты релиза): так установка сводится к
  «скачать скрипт → получить токен → запустить скрипт с токеном».
* **Платформа:** в поставке лежат **оба** пакета (`windows-x64` и `macos-arm64`), сервер
  выбирает нужный по `?platform=…`, иначе по `User-Agent`, иначе по своей платформе
  (`hds_core::client::pick_package`); в ответе — заголовки `X-Client-Package`/`X-Client-Platform`.
* **Откуда берётся пакет:** релизный ассет `hds-client-<версия>-<платформа>.zip` — и он же
  вложен в серверную поставку (`client/`), откуда сервер отдаёт его по
  `<mcp-path>/client-package.zip` (Bearer-гейт как у `/mcp`). Тонкую сборку делает
  `installers/build_client_release.ps1` (Windows) / `build_client_release_macos.sh` (macOS):
  в пакет входят только `bin\hds(.exe)`, правило/скилл, мастер, `README`, `version.txt`,
  `sha256.txt`; сборщик **отвергает** «тяжёлые» части (`hds_mcp`/`llm_host`/`sidecar`/`runtime-manifests`).
* **Версии (W7):** пакет несёт `version.txt`, сервер публикует `version` в `/health`; мастер
  сверяет их и предупреждает при расхождении (правило/скилл могли отстать).
* **Проверка:** `GET https://host/health` → 200 (`version` — версия сервера); в Cline виден
  сервер `disk-search`; после перезапуска — вопрос по файлам работает через внешний сервер.

## 9. Десктоп-режим (по умолчанию)

Если в `config.yaml` нет `access.enabled: true` (или он `false`):

* MCP/UI работают без токена — прежнее поведение одиночной машины;
* фильтр проектов не применяется (виден весь индекс);
* управляющие инструменты и карточки UI работают как раньше;
* `POST /auth/login` отвечает `404` (эндпоинт не активен);
* `data/access.yaml` может существовать и использоваться CLI-командами — но гейт выключен.

## 10. Шпаргалка

> Полный справочник команд (все флаги CLI, утилиты и скрипты, где бинарник/конфиг/
> состояние) — `README.md` §«Справочник команд: сервер, клиент, утилиты, установщики»;
> общая карта документов проекта — `README.md` §«Карта документов». Ниже — минимум
> по серверным сценариям.

```powershell
# ── админ ──────────────────────────────────────────────
hds user add --login alice --password "..." --projects "Sales,Docs"   # создать
hds user list                                                          # список
hds user grant --login alice --projects "Ops"                          # +проект
hds user revoke --login alice --projects "Docs"                        # −проект
hds user passwd --login alice --password "..."                         # смена пароля (+отзыв токенов)
hds user del --login alice                                             # удалить (+отзыв токенов)
hds token list --user alice                                            # токены
hds token revoke --sha <префикс> | --user alice                        # отзыв

# ── службы (server-профиль, админ) ─────────────────────
hds service install|start|stop|status|uninstall <role>                 # role: watch|mcp-http|ui|llm-host|transcribe
hds service root                                                      # корень установки служб (binPath/плейст)
# из корня распакованного архива: install-server.cmd [-WhatIf]        # первичная установка сервера
#                                 update.cmd [-WhatIf|-RollbackTo <ver>] # обновление (macOS: installers/update.command)

# ── пользователь ───────────────────────────────────────
hds login --login alice --password "..." --url http://127.0.0.1:8787   # → токен (JSON)
# UI /ui/user/                                                         # кабинет: поиск, клиент, спикеры

# ── клиент (Cline, тонкий) ─────────────────────────────
hds cline-sync --server-url https://host/ --token <bearer>             # записать настройки Cline

# ── HTTP ───────────────────────────────────────────────
# POST /auth/login  {"login":"...","password":"..."}  → 200 {token,expires,user,projects} | 401
# POST /api/auth/login  — то же для кабинета UI (тот же ответ + admin)
# POST <mcp-path>   JSON-RPC,  header Authorization: Bearer <token>    → 401 без токена
# GET  /health      — публичный (маркер инстанса, поле version)
# UI                — ?token=<админ-токен> (админская), /ui/user/ — любой валидный токен
# <mcp-path>/client-install.ps1|sh — публично (скрипт без токена), пакет — по Bearer
```

## 11. Сетевой периметр и конкурентность (M3–M4): TLS, аудит, rate-limit, SLA

Наружу публикуются **только** `/mcp`, `/ui` и публичная проба `/health` через reverse-proxy
с TLS (Caddy или nginx, образцы — `installers/proxy/{Caddyfile,nginx.conf}`).
Приложение и фасад остаются на localhost:

| Сервис | Адрес | Наружу? |
|---|---|---|
| MCP (`/mcp`, `<mcp-path>/client-install.ps1\|sh` — публично) | `127.0.0.1:8787` | через прокси (TLS) |
| Проба `/health` (имя приложения + `version`) | `127.0.0.1:8787` | через прокси (TLS), без токена |
| UI (`/ui/` — админская, `/ui/user/` — кабинет) | `127.0.0.1:8765` | через прокси (TLS) |
| Фасад `:8010–8012` (`/v1/*`) | localhost | **не публикуется** |

Прокси передаёт путь как есть (`/ui/...`), а UI сам понимает и префикс `/ui`, и корень —
поэтому работает и прямой доступ к `127.0.0.1:8765`, и публикация через прокси.

```powershell
# Caddy: caddy run --config installers\proxy\Caddyfile (домен/сертификат — в файле)
# nginx: копия installers\proxy\nginx.conf в conf.d, затем nginx -t && nginx -s reload
```

После публикации прокси включите в `config.yaml`:

```yaml
trusted_proxy: true   # доверять X-Forwarded-For/Proto от прокси
```

* Пока `trusted_proxy: false` (дефолт, в т.ч. десктоп), заголовки прокси
  **игнорируются**: сидящий напрямую клиент не может подделать свой IP.
* При `trusted_proxy: true` реальный клиент — **последняя (правая) запись**
  `X-Forwarded-For` (её дописал наш прокси, клиенту не подделать); аудит и
  rate-limit ключуются по нему. Схема (`https`) — из `X-Forwarded-Proto`.
* Диагностика: `GET /health` MCP-сервера возвращает `client_ip` — так видно,
  как сервер «видит» клиента (напрямую — его IP; за прокси — IP прокси, а с
  `trusted_proxy: true` — реальный клиент из XFF).
* TLS терминируется на прокси; сертификаты настраивает оператор
  (Let's Encrypt или свои) — приложение остаётся на plain HTTP в localhost.

### Аудит (M3 T2)

Append-only JSONL: `<state_dir>/data/logs/audit.jsonl`, ротация по размеру
(`audit.max_mb`, дефолт 50 → `audit.jsonl.1`). Включение — `audit.enabled`;
без ключа: server — **включён**, desktop — выключен (не шумим локально).

```json
{"ts":1791406900,"event":"login_ok","user":"alice","ip":"203.0.113.7","method":"POST","path":"/auth/login","status":200,"ms":395}
{"ts":1791406900,"event":"unauthorized","ip":"203.0.113.7","method":"POST","path":"/mcp","status":401,"ms":0}
{"ts":1791406900,"event":"tool_call","user":"alice","ip":"203.0.113.7","tool":"search_local_files","projects":["Sales"],"status":200,"ms":120}
```

События: `login_ok`/`login_fail` (MCP), `unauthorized` (MCP и UI-гейт),
`tool_call` (кто/какой инструмент/проекты токена), `admin_op` (POST-операции
UI в server-режиме), `token_revoke` (`hds token revoke`). **Не логируются**
тела запросов и токены. Состояние аудита — `hds check` и карточка
«Диагностика» в UI (`audit`): вкл/выкл, путь, размер.

### Rate-limit (M3 T3)

Token-bucket без внешних крейтов, в памяти процесса (mcp-http и ui лимитируются
независимо — общий лимит, если нужен, ставится на прокси). Ключ — **пользователь**
(`user:<login>` по Bearer), иначе реальный IP (за доверенным прокси — из XFF);
`rate_limit.per_token: true` возвращает прежний пер-токенный ключ (M4 T4 — дефект
M3 F10.3: два токена одного пользователя удваивали квоту). `POST /auth/login`
— отдельный жёсткий лимит по IP (антибрутфорс); `ask_my_files` — лимит
параллелизма (`max_inflight` глобально + `max_inflight_per_user` на пользователя).

```yaml
rate_limit:
  enabled: true            # без ключа: server — true, desktop — false
  requests_per_min: 120
  burst: 30
  login_per_min: 10
  max_inflight: 4          # ИСПОЛНЕНИЕ (глобально — слотов очереди)
  max_inflight_per_user: 2 # на пользователя (3-й одновременный RAG -> 429 + hint)
  requests_per_min_per_user: 120  # агрегат частоты на пользователя (при per_token: true)
  per_token: false         # true = ключ лимитов — токен (поведение M3)
```

Превышение → `429 Too Many Requests` + `Retry-After: <сек>` +
`X-RateLimit-Limit: <rpm>`.

### Hardening (M3 T4)

* `http.max_body_kb` (дефолт 512; 0 — без лимита): тело больше лимита →
  `413` без чтения тела.
* `http.max_conns` (дефолт 32; 0 — без предела): сверх предела → короткий
  `503` (+ `Retry-After`) без спавна потоков.
* Ошибки — единый JSON `{error, hint}`, без внутренних путей/версий.
* `ui_auth_gate` при нечитаемом `config.yaml` в server-поставке **fail-closed**
  (401), а не молча открытый UI (дефект F2 из ревью M2).

### БД под конкурентность (M4 T1)

Один писатель (конвейер индексации/watcher) и много читателей (MCP/UI/CLI-поиск) на общей
SQLite-базе — без «database is locked»:

```yaml
db:
  busy_timeout_ms: 5000    # server-дефолт; desktop — 30000 (прежнее поведение)
  journal_mode: wal        # WAL
  read_only_conns: true    # server: читатели — read-only соединения (desktop: false)
```

* `open_rw` — единственный писатель (DDL + WAL + `synchronous` + FK + `busy_timeout`);
  `open_ro` — читатели (read-only, без DDL; отсутствующая база → явная ошибка).
* ⚠ WAL + read-only: читателям нужен доступ к файлу `-shm` (иначе `immutable=1`).
* **Бэкап:** копирование файла при WAL небезопасно — только SQLite `.backup` / `VACUUM INTO`
  (штатный бэкап — этап Э10). `hds check` показывает фактические PRAGMA.
* Стресс «5 читателей + писатель» без `SQLITE_BUSY` — `crates/hds-core/tests/db_concurrency.rs`.

### Слоты чата и контекст (M4 T2)

* `llm_server.parallel` — сколько запросов чата исполняются **одновременно** (server-дефолт **5**,
  desktop — 1); контекст слота — `llm_server.chat.ctx_per_slot` (для RAG подобран под SLA).
* **Fit-гейт VRAM** на старте чата: не влезает — **явная ошибка** с числами «нужно/свободно/резерв»
  и подсказкой снизить `parallel`/`ctx_per_slot`; у индексных ролей — громкое предупреждение.
* `hds check` (пункт `chat`) показывает фактические слоты из `/props` рядом с настроенным
  `parallel` (расхождение → `warn`).

### Очередь тяжёлых RAG и SLA (M4 T3)

`ask_my_files` — тяжёлый инструмент (поиск + 27k-токенный промпт + генерация). До M4
при насыщении `rate_limit.max_inflight` он сразу получал `429`. Теперь (server-профиль)
запрос **ждёт слот** в очереди MCP-процесса:

* **fair-share:** round-robin между пользователями (ключ — `user:<login>` из Bearer,
  иначе `ip:<addr>`, без авторизации — `local`), FIFO внутри пользователя;
  политика `queue.policy: rr | fifo`;
* **лимиты:** `rate_limit.max_inflight` — сколько исполняется **одновременно**,
  `queue.max_depth` — сколько **ждёт**, `queue.wait_timeout_sec` — потолок ожидания;
* **коды:** очередь полна → `429` + `Retry-After` + `X-Queue-Depth`; не дождался →
  `503` + `Retry-After`; обрыв клиента в ожидании освобождает место;
* **индексация уступает:** пауза `index.pause` держится на время запроса
  (`gpu.pause_index_on_query`), запрос не ждёт индексного писателя БД (WAL, M4 T1);
* **метрики:** глубина, ожидание p50/p95, отказы/таймауты, «кто кого ждёт» — в
  `hds check` и `/api/diagnostics` (пункт `queue`), источник — снапшот
  `<state_dir>/data/mcp-queue.json`, который пишет MCP-процесс раз в 2 с (нет/устарел →
  предупреждение: MCP не запущен).

```yaml
queue:
  enabled: true            # без ключа: server — true, desktop — false (поведение M3)
  max_depth: 20            # сколько запросов ЖДУТ
  wait_timeout_sec: 240    # < 300 (таймаут MCP-клиента), иначе 503 придёт позже обрыва
  policy: rr               # rr | fifo
```

Ожидание в очереди пишется в аудит событием `queue_wait` (`ms` — сколько ждали).
`wait_timeout_sec` держите меньше клиентского таймаута (300 с) — `hds check`
предупредит, если значение больше. Ограничение: очередь живёт в процессе
`hds mcp-http`; админский `ask` веб-UI лимитируется своими лимитами M3 (общая очередь
на два процесса — вне M4).

### Квоты и приоритеты по пользователю (M4 T4)

* **Ключ лимитов — пользователь:** `user:<login>` по валидному Bearer, иначе
  `ip:<addr>`. Два токена одного пользователя делят одну квоту (регресс M3 F10.3);
  `rate_limit.per_token: true` возвращает пер-токенный ключ, но частота всё равно
  дополнительно ограничивается агрегатом на пользователя
  (`requests_per_min_per_user`).
* **Пер-пользовательский предел исполнения:** `rate_limit.max_inflight_per_user`
  (дефолт 2) — 3-й одновременный `ask_my_files` одного пользователя получает
  `429` + hint про `rate_limit.max_inflight_per_user` (в очереди — сразу, не ждёт).
  Предел учитывается, только если он строже глобального `max_inflight`; без очереди
  (desktop) эффективный предел — `min(max_inflight_per_user, max_inflight)`.
* **Инвариант «индексация уступает»:** тяжёлый RAG не ждёт индексного писателя БД
  (WAL + `db.read_only_conns`, M4 T1), а индекс встаёт на паузу на время запроса
  (`gpu.pause_index_on_query` → файл `index.pause`).
* **Проверки:** юнит-тесты `hds-core::{ratelimit,queue}` и сокет-тесты
  `crates/hds-mcp/tests/quota_socket.rs` (пер-пользовательский `429` доходит до
  клиента; два токена одного пользователя не удваивают квоту).
* **Нагрузочная приёмка (M4 T5):** стенд `tools/parity/load5.py` — 5
  одновременных РЕАЛЬНЫХ `ask_my_files` через `/mcp` (5 пользователей, песочный
  индекс и резидент). `parallel 5 / ctx 16384` → все `200`, p95 **49,6 с**
  (SLA ≤5 мин выполнен); `parallel 3` → p95 48,9 с; `parallel 2` → p95 58,1 с
  (очередь ждёт, отказов/таймаутов 0). При индексации параллельно `200`×5, но
  p95 315 с — «индексация уступает» (25 с → 333 с) и «database is locked» нет.
  Остаточный риск: одиночный выброс ~305–315 с при on-demand загрузке
  `embedding` на 12 ГБ (см. `README.md`, §«Слоты и SLA»). Отчёты —
  `tools/parity/out/m4_load_*.json`.

## 12. Текущее состояние и что осталось

**Реализовано и покрыто тестами (M1–M9)** — карта §0.1: профили `desktop`/`server`; проекты,
пароли (Argon2id) и токены (Bearer); фильтрация выдачи по проектам во всех ветках поиска;
admin-гейтинг; TLS reverse-proxy (образцы) + аудит + rate-limit + hardening; БД под
конкурентность + слоты/fit-гейт + очередь тяжёлых RAG + квоты под SLA; внешняя чат-модель;
несколько GPU одного семейства; управление службами/токенами из UI; серверный и клиентский
пакеты установки. Код-ревью пройдены для M2–M7 (`M*_CODE_REVIEW.md`).

**Известные ограничения (осознанные):**

* **`disabled`-флаг учётки** есть в схеме `access.yaml`, но отдельной команды/переключателя нет —
  для отключения используйте `hds user del` (отзывает токены).
* **Фасад `:8010–8012`** (chat/embedding/rerank) без авторизации — **не публиковать наружу**;
  наружу — только `/mcp` и `/ui` через TLS-прокси (§11).
* **Анонимизация (W9) не реализована:** при `chat.provider: remote` вопрос и фрагменты уходят
  провайдеру как есть (§8a).
* **Остаточный выброс латентности** ~305–315 с при 5 занятых слотах и on-demand загрузке
  `embedding` на 12 ГБ (детали — `README.md` §«Слоты и SLA», §11).
* **Не проверено на живом железе:** несколько дискретных карт и Apple Silicon; host-выбор карты
  под файл при явном `gpu` от клиента (F-m6-t5-1, §8b).

**Осталось (следующие этапы, вне M1–M9):**

* **Э10/W8 «Эксплуатация»:** агрегированный `/healthz` (llm-host + БД + watcher + диски),
  бэкап `index.db` (`.backup`/`VACUUM INTO`) и восстановление, расширенная админ-диагностика
  `hds check`, обновление с малым простоем (план — `PLAN_M10_OPS.md`).
* **Э11:** нагрузочная приёмка на 1/2 картах и Apple Silicon.
* **W9:** анонимизация внешнего API (снимает ограничение `remote`).
* Живая проверка **HTTPS через TLS-прокси** на машине с сертификатом (сейчас подтверждён
  HTTP-гейт `401`/`200` и живой клиент с Bearer по LAN).

**Ссылки:**

* Дорожная карта: `PLAN_SERVER_MULTIUSER.md` (§2 workstreams W1–W10, §5 этапы Э1–Э11, §9 статусы).
* Планы этапов: `PLAN_M1_ACCESS_PROJECTS.md`, `PLAN_M2_SERVER_PROFILE.md`,
  `PLAN_M3_NETWORK_PERIMETER.md`, `PLAN_M4_CONCURRENCY_SLA.md`, `PLAN_M5_EXTERNAL_CHAT.md`,
  `PLAN_M6_GPU_MULTIVENDOR.md`, `PLAN_M7_UI_CONTROL.md`, `PLAN_M8_INSTALL_PACKAGES.md`,
  `PLAN_M9_CLIENT_PACKAGE.md`.
* Код-ревью: `M2_CODE_REVIEW.md` … `M7_CODE_REVIEW.md`; статус — `STATUS.md`;
  промты передачи контекста — `M1_HANDOFF_PROMPT.md` … `M9_HANDOFF_PROMPT.md`.
* Смежные подсистемы: `PLAN_AUTO_TRANSCRIBE.md` (автотранскрибация + диаризация),
  `PLAN_INDEX_QUALITY.md` (качество индекса), `PLAN_SHARED_LLAMA_RUNTIME.md`, `PLAN_W2_LLM_HOST.md`.
* Формат хранилища доступа: `data/access.example.yaml`.
