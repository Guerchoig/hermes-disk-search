# План: раскладка установки обязана поддерживать обновление и откат (инцидент 16.10.2026, релиз 0.20.3)

> Ветка `server-multiuser`. Этот документ фиксирует дефект, решение и приёмку — не
> переписывается (правило проекта: планы фиксируют решения).

## 1. Факт: обновление не меняло то, что запускается

Проверено чтением кода и живого состояния (16.10.2026):

| # | Факт | Где |
|---|---|---|
| Ф1 | Релизный архив **плоский**: `bin\`, `installers\`, `install-server.cmd`, `update.cmd`, `version.txt`; подпапки `app\` в нём нет | `RELEASING.md` §2 |
| Ф2 | Установщик берёт корень как папку над скриптом и **принимает** версионную раскладку, но **не создаёт** её | `installers/install_server.ps1:55-66` (до правки) |
| Ф3 | Службы регистрируются на **тот** `hds.exe`, который запущен (`sc create`/`sc config`) | `hds-core::service::{sc_install_raw,sc_config_raw}` |
| Ф4 | Обновление кладёт версию в `app\<ver>` и переключает `app\current`, **не трогая** точки запуска | `installers/update.ps1` → `install_app_version.ps1 -SetCurrent` |
| Ф5 | Корень из `binPath` разбирается одинаково для плоской и версионной раскладки, поэтому `update.cmd` «успешно» обновляется, ничего не подменяя | `hds-core::service::root_of_exe_str` |
| Ф6 | Задания Планировщика и ярлык на рабочем столе хранят **абсолютный** путь (`bin\hds.exe`), обновление их только останавливает/запускает | `install_autostart.ps1:55`, `installers/install_ui_task.ps1:49`, `install_llm_host_task.ps1:120`, `shortcuts/windows/create_shortcut.ps1` |
| Ф7 | Единственное место, где роли переводились на `app\current\bin` — документированный перенос установки | `installers/migrate_to_machine_root.ps1:218` |
| Ф8 | Живая установка на dev-машине: 4 SCM-службы с `binPath = …\app\current\bin\hds.exe`, раскладка версий есть, пункта диагностики о ней **не было** | `hds service root`, `sc qc` |

**Итог дефекта.** Установка, сделанная по прежней главе 5 руководства («распаковать в машинную
папку → `install-server.cmd`»), остаётся плоской. Первое `update.cmd` создаёт `app\<ver>` и
переключает указатель, но службы продолжают работать **прежними** файлами из `<папка>\bin`;
`-RollbackTo` переключает указатель, которым никто не пользуется; `-ListVersions` на такой
установке сообщает «нет папки app». То же по существу в настольной ветке — через задания
автозапуска и ярлык. Тихая деградация: обновление отчитывается об успехе.

## 2. Решение

1. **Установщик ставит раскладку по версиям** (`install_server.ps1`, `install_server.command`):
   если корень установки отличается от распакованного архива, сначала укладывается версия
   (`install_app_version.ps1`), а роли регистрируются на `app\current\bin`. Без `-Root`/первого
   аргумента поведение прежнее (плоская установка) — но в сводке прямо сказано, что обновление
   переведёт службы само.
2. **Обновление перерегистрирует точки запуска** (`update.ps1`): после подмены версии и в ветке
   отката — службы через `<app\current>\bin\hds.exe service install` (идемпотентный `sc config`,
   авторестарт применяется там же) + повторный пин `HDS_CONFIG` и машинных путей; задания
   автозапуска пересоздаются из `app\current` (`install_ui_task.ps1`, `install_llm_host_task.ps1`,
   `install_autostart.ps1`), ярлык — `create_shortcut.ps1`. macOS — перерегистрация LaunchDaemon
   (`service install`) по плейсту.
3. **Диагностика показывает раскладку** (`hds check`, пункт `layout`): из чего запускаются службы
   и задания; `warn` — если каталог версий есть, а запуск идёт не из него.
4. **Документация приведена к поведению**: серверное руководство (главы 5, 6, 15, 16, 18, 20),
   настольное (главы 3, 14, 17), правила выпуска (`RELEASING.md` §7 — новый пункт приёмки).

## 3. Что сделано

* код: `installers/install_server.ps1`, `installers/install_server.command`,
  `installers/update.ps1`; `crates/hds-core/src/service.rs` (`installed_exe`, `installed_root_of`,
  `exe_path_from_sc_qc`, `exe_path_from_plist`, `command_from_task_xml`, `logon_task_exes`),
  `crates/hds-index/src/diag.rs` (пункт `layout`);
* тесты: `hds-core` — `exe_path_parsers_pick_our_binary`, `task_xml_command_is_parsed`;
  `hds-index` — `layout_check_flags_flat_roles_next_to_a_versioned_tree`;
  `hds-cli/tests/install_server_parity.rs` — `update_ps1_repoints_launch_points`,
  `installers_lay_out_the_version`;
* документы: `README_SERVER_MULTIUSER.md`, `README.md`, `RELEASING.md`, этот план,
  `RELEASE_NOTES_v0.20.3.md`.

## 4. Приёмка

* `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` — зелёные локально (CI — на теге);
* PowerShell-скрипты разбираются парсером (`Parser::ParseFile`), `bash -n install_server.command`;
* живая проверка на dev-машине: `hds check` печатает
  `[ok] Раскладка по версиям: запуск из app\current\bin`;
* `install-server.cmd -WhatIf -Root …` показывает строку `Layout:` с будущей раскладкой.

## 5. Ограничения и что осталось

* macOS-ветка (перерегистрация LaunchDaemon, раскладка по версиям) живьём не проверялась —
  сборка для Mac «best-effort» (`RELEASING.md` §2);
* `hds_bin.ps1` (резолвер для ярлыков и скриптов) по-прежнему предпочитает `bin\` — при версионной
  раскладке это `app\current\bin`, то есть корректно; для уже «плоских» установок, обновлённых
  до 0.20.3, ярлык пересоздаётся обновлением 0.20.3+;
* клиентские ПК не затрагиваются: раскладка на сервере, версия `/health` берётся из `version.txt`;
* устаревшие (до 0.20.3) установки лечатся первым же `update.cmd`: он перерегистрирует точки
  запуска; диагностика до этого показывает `warn` пунктом `layout`.
