# RELEASE_NOTES v0.20.1 — обновление сервера одним файлом (`update.cmd`)

> Релиз-фича: **установка и обновление сервера стали одной командой.** В архиве появились обёртки
> `update.cmd` (Windows) и `installers/update.command` (macOS): они сами снимают метку «скачано из
> интернета»/карантин, **сами находят корень установки** (новая подкоманда `hds service root` читает
> `binPath` SCM-службы или плейст LaunchDaemon) и берут версию из `version.txt`, после чего зовут
> `installers\update.ps1`. Ручные `-Root`/`-From`/`-Version` и борьба с execution policy больше не
> нужны: политика применяется только к `.ps1`, а `.cmd` запускается всегда. Поведение
> поиска/транскрибации и схема данных не изменились — это PATCH-релиз логистики установки.

## Что скачивать

| Файл | Нужен? | Что это |
|---|---|---|
| hds-0.20.1-windows-x64.zip (+ .sha256.txt) | **Да, обязательно** (Windows) | полная поставка: `bin\` (`hds`, `hds_mcp`, `llm_host`), установщик `setup.cmd`, **обёртки `install-server.cmd` и `update.cmd`**, манифесты, sidecar-воркер, `client\` (пакеты + скрипты) |
| client-install.ps1 / client-install.sh | Для установки клиента | скрипт-установщик (токен аргументом): скачает пакет с сервера, распакует и подключит Cline |
| hds-client-0.20.1-windows-x64.zip (+ .sha256.txt) | Для клиента (если не через скрипт) | тонкий клиент: `bin\hds.exe`, мастер `setup-client.*`, правило/скилл, `version.txt` |
| hds-client-0.20.1-macos-arm64.zip (+ .sha256.txt) | Только для macOS | то же для Apple Silicon |
| hds-engine-runtime-windows-x64-cuda.zip | Только для **офлайн-установки** (NVIDIA) | рантайм движка CUDA; обычный установщик качает его сам по манифесту |
| hds-engine-runtime-windows-x64-vulkan.zip | Только для **офлайн-установки** (AMD/Intel) | рантайм движка Vulkan |
| hds-0.20.1-macos-arm64.zip (+ .sha256.txt) | Только для macOS (best-effort) | полная поставка для Apple Silicon; обновление — `sudo bash installers/update.command` (нужен `pwsh`) |
| hds-engine-runtime-macos-arm64-metal.zip | Только для macOS офлайн | рантайм движка Metal |
| hermes-disk-search-0.20.1-windows.zip / -macos.zip | Нет | архивы исходников (для разработчиков) |

Модели (LLM/whisper/диаризация/CLIP ONNX) в архив **не входят** — установщик скачивает их сам.

## Что нового

* **Обновление сервера — одним файлом.** Распакуйте новый архив в любую папку и запустите
  `update.cmd` (Windows; для серверных служб — от администратора) или
  `sudo bash installers/update.command` (macOS). Что делает обёртка:
  1. снимает MotW (`Unblock-File`) / карантин (`xattr -dr com.apple.quarantine`);
  2. находит **корень установки** через новую подкоманду `hds service root` — она читает путь к
     `hds.exe` из `binPath` SCM-службы (Windows) или из плейста LaunchDaemon (macOS) и понимает обе
     раскладки поставки (версионную `app\current` и плоскую `bin\`);
  3. берёт версию из `version.txt` и вызывает `installers\update.ps1 -Profile server` — тот сам
     останавливает службы/задачи и резидента, подменяет версию, переключает `app\current`, стартует
     всё обратно и проверяет `/health`.

  Аргументы пробрасываются как есть: `update.cmd -WhatIf`, `-Force`, `-RollbackTo <версия>`,
  `-Root <папка>` (если служб нет — десктопная установка или ручная распаковка).
* **Первичная установка сервера — `install-server.cmd`** (обёртка `installers\install_server.ps1`,
  тоже снимает MotW и избавляет от `-ExecutionPolicy Bypass`). Для десктопа остаётся `setup.cmd`.
* **Новая подкоманда `hds service root`** — печатает корень установки служб (полезна и вручную:
  видно, где живёт работающий сервер). В справке `hds --help` заодно уточнён список ролей
  (появилась `transcribe`, добавленная в 0.20.0).
* **Почему больше не мешает execution policy:** политика PowerShell (`RemoteSigned` и строже)
  применяется к `.ps1`/`.psm1`, а `.cmd` исполняется `cmd.exe` — поэтому обёртки запускаются даже
  там, где скрипты «из интернета» заблокированы. При прямом запуске `.ps1` по-прежнему нужен
  `-ExecutionPolicy Bypass` (либо снятая метка MotW).
* **Документация:** README (шаг «Обновление» для десктопа и сервера, раздел «Блокировки скачано из
  интернета», обновление на macOS, состав архивов, раздел служб с `hds service root`);
  README_SERVER_MULTIUSER (§8d — установка/обновление серверного пакета — и шпаргалка); клиентский
  README в пакете (исправлен путь macOS-мастера: `install_client.command` лежит в корне пакета, а не
  в `installers/`); RELEASING (§2 состав архива, §7 грабли).
* **CI:** в блокирующую джобу `test-rust` добавлена проверка синтаксиса shell-обёрток (`bash -n` для
  `installers/*.sh` и `installers/*.command`) — до этого macOS-скрипты не проверялись нигде (на
  Windows-машине разработчика bash нет).
* **Внутреннее:** разбор корня установки покрыт юнит-тестами и работает на строке, поэтому Windows-
  и POSIX-пути разбираются одинаково на всех платформах. Первая версия использовала `Path` и падала
  на Linux/macOS (красный `test-rust`) — это поймал CI, а не пользователь.

## Требования к обновлению

* **Штатное обновление поверх 0.20.0** (совместимо и с 0.19.x): распаковать архив и запустить
  `update.cmd` (Windows) или `sudo bash installers/update.command` (macOS, нужен `pwsh`).
  Переиндексация **не нужна**: схема БД, `config.yaml`, `data/access.yaml` и `index.db` не
  затрагиваются, поведение поиска/транскрибации не менялось.
* **Первичная установка сервера:** `install-server.cmd` (Windows, от администратора) либо
  `installers/install_server.ps1` / `installers/install_server.command` (macOS) — как раньше; для
  одной машины без служб — `setup.cmd`.
* **Серверным стендам** напоминание из 0.20.0: роль `transcribe` ставится отдельно
  (`hds service install transcribe`), а легаси-ключ `index.transcribe` из `config.yaml` можно убрать
  (`hds check` помечает его).
* Токены и `data/access.yaml` совместимы с 0.20.0; кабинет `/ui/user/` работает без изменений.
* macOS: автоопределение корня в `update.command` опирается на LaunchDaemon (серверный профиль);
  для десктопной установки укажите `-Root <папка>`.
