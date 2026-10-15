# hermes-disk-search — тонкий клиент (M9 / Э9)

Это **клиентский** пакет: он только подключает Cline к **удалённому** серверу
disk-search. Движка, моделей, индекса и sidecar здесь нет — всё на сервере.

## Что внутри
- `bin\hds.exe` (Windows) / `bin/hds` (macOS) — утилита проводки;
- `setup-client.ps1` / `setup-client.cmd` (Windows), `install_client.command` (macOS);
- `cline-rules\`, `hermes-skill\` — правило и скилл Cline (копируются в `~/.cline`);
- `version.txt` — версия пакета (мастер сверяет её с `version` из `/health` сервера);
- `sha256.txt` — контрольные суммы `bin/*`.

## Установка одной командой (рекомендуется)
Скачайте скрипт-установщик (для этого токен **не** нужен — скрипт отдаётся публично)
и запустите его с токеном:

```powershell
curl.exe -f https://host/mcp/client-install.ps1 -o client-install.ps1
powershell -ExecutionPolicy Bypass -File .\client-install.ps1 -Token <ТОКЕН> -ServerUrl https://host
```
```bash
curl -f https://host/mcp/client-install.sh -o client-install.sh
bash client-install.sh --token <ТОКЕН> --server-url https://host
```

Скрипт сам скачает клиентский пакет под вашу платформу (`?platform=`), распакует его,
снимет блокировку/карантин и запустит мастер (он же сверяет версии). Затем перезапустите
Cline. Права администратора не нужны.

## Как получить пакет вручную
Два канала (файл один и тот же):
1. **с сервера** (своим токеном; так удобнее без доступа к GitHub):
   ```powershell
   curl.exe -f -H "Authorization: Bearer <ТОКЕН>" -o hds-client.zip https://host/mcp/client-package.zip
   Expand-Archive .\hds-client.zip -DestinationPath .\hds-client -Force
   Get-ChildItem .\hds-client -Recurse | Unblock-File
   ```
   ```bash
   curl -f -H "Authorization: Bearer <ТОКЕН>" -o hds-client.zip https://host/mcp/client-package.zip && \
     unzip -q hds-client.zip -d hds-client && xattr -dr com.apple.quarantine hds-client
   ```
2. **ассетом релиза** `hds-client-<версия>-<платформа>.zip` со страницы релизов.
3. (для разработчика) собрать из репозитория: `installers/build_client_release.ps1 -Version <версия>`.

## Установка (Windows)
1. Токен выписывает администратор сервера: `hds token new --user <логин>`
   (plain-токен показывается **один раз**).
2. Запустите (права администратора **не** нужны):
   ```powershell
   .\setup-client.ps1 -ServerUrl https://host/ -Token <bearer>
   ```
   или `setup-client.cmd -ServerUrl https://host/ -Token <bearer>`.
   Мастер проверит `GET <ServerUrl>/health`, запишет настройки Cline и попросит
   перезапустить Cline. Флаг `-DryRun` показывает план без записи.

## Установка (macOS)
```bash
bash install_client.command --server-url https://host/ --token <bearer>
```
(`--dry-run`, `--skip-health`, `--mcp-path /mcp` — при необходимости.)

## Проверка
- `GET https://host/health` → `200`;
- в Cline появится MCP-сервер `disk-search` (URL `https://host/mcp`, заголовок
  `Authorization: Bearer …`);
- перезапустите Cline (Desktop/CLI) и задайте вопрос по файлам.

## Безопасность
- Токен передаётся в Cline как заголовок `Authorization`; в логи и аудит он **не**
  пишется; мастер передаёт его в `hds` через переменную окружения (не в командной строке);
- без TLS (`http://`) токен идёт открытым текстом — используйте `https://`
  (reverse-proxy на сервере, `installers/proxy/`).

Подробности — `README_SERVER_MULTIUSER.md` §8e, план `PLAN_M9_CLIENT_PACKAGE.md`.
