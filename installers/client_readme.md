# hermes-disk-search — тонкий клиент (M9 / Э9)

Это **клиентский** пакет: он только подключает Cline к **удалённому** серверу
disk-search. Движка, моделей, индекса и sidecar здесь нет — всё на сервере.

## Что внутри
- `bin\hds.exe` (Windows) / `bin/hds` (macOS) — утилита проводки;
- `setup-client.ps1` / `setup-client.cmd` (Windows), `installers/install_client.command` (macOS);
- `cline-rules\`, `hermes-skill\` — правило и скилл Cline (копируются в `~/.cline`);
- `sha256.txt` — контрольные суммы `bin/*`.

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
bash installers/install_client.command --server-url https://host/ --token <bearer>
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
