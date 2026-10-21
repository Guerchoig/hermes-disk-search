//! M8 T1/T2: unattended-параметры серверных установщиков (Windows/macOS) — паритет.
//!
//! Статический контракт (`PLAN_M8_INSTALL_PACKAGES.md` §3): `install_server.ps1` и
//! `install_server.command` экспонируют один и тот же unattended-набор и генерируют
//! server-конфиг. Тест ловит рассинхрон/потерю параметров между сборками.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("прочитать {}: {e}", p.display()))
}

/// Ожидаемый unattended-набор: (PowerShell-параметр, bash-флаг).
const PARAMS: &[(&str, &str)] = &[
    ("StateDir", "--statedir"),
    ("McpHost", "--mcp-host"),
    ("McpPort", "--mcp-port"),
    ("UiHost", "--ui-host"),
    ("UiPort", "--ui-port"),
    ("Parallel", "--parallel"),
    ("GpuDevice", "--gpu-device"),
    ("GpuPreset", "--gpu-preset"),
    ("Roots", "--roots"),
    ("EnableAccess", "--enable-access"),
    ("AdminLogin", "--admin-login"),
    ("AdminPassword", "--admin-password"),
    ("Fetch", "--fetch"),
    ("NoFetch", "--no-fetch"),
    ("SkipDeps", "--skip-deps"),
    ("NoConfigPin", "--no-config-pin"),
    ("WhatIf", "--whatif"),
];

/// Маркеры шагов установки, общие для обоих установщиков. YAML-детали сюда больше
/// не входят: server-конфиг рендерит `hds config init` из шаблона.
const CONFIG_NEEDLES: &[&str] = &["fetch_engine_runtime.ps1", "token new --user"];

/// Server-конфиг генерируется **одним** кодом, а не двумя скриптами.
///
/// Регрессия 0.20.5: `install_server.ps1::New-ServerConfig` и
/// `install_server.command::gen_config` собирали конфиг строчками независимо
/// друг от друга и разъехались — в серверном конфиге не оказалось
/// `index.exclude_dirs`, и стенд индексировал `D:\$RECYCLE.BIN`
/// (`D:\$RECYCLE.BIN\S-1-5-18\desktop.ini` попал в БД) и `System Volume Information`.
///
/// Теперь: шаблон `server_config.example.yaml` (вход рендера, он же документация) +
/// `hds config init` (рендер, проверка YAML, merge дефолтов в существующий конфиг).
/// Установщики YAML не строят — только передают параметры.
#[test]
fn server_config_is_generated_by_one_rust_code_path() {
    // 1) Шаблон с серверными исключениями на месте.
    let tpl = read("server_config.example.yaml");
    assert!(
        tpl.contains("exclude_dirs:"),
        "в server_config.example.yaml нет index.exclude_dirs"
    );
    for must in [
        "$RECYCLE.BIN",
        "System Volume Information",
        "WindowsApps",
        "node_modules",
    ] {
        assert!(
            tpl.contains(must),
            "в server_config.example.yaml нет '{must}'"
        );
    }

    // 2) Генератор живёт в Rust: рендер шаблона и merge дефолтов.
    let rust = read("crates/hds-cli/src/cmd/config.rs");
    for needle in [
        "render_server_config",
        "merge_defaults_into",
        "default_exclude_dirs",
        "TEMPLATE_NAME",
    ] {
        assert!(
            rust.contains(needle),
            "в cmd/config.rs нет '{needle}' — серверный генератор потерян"
        );
    }

    // 3) Windows-установщик только вызывает генератор и не строит YAML сам.
    let ps1 = read("installers/install_server.ps1");
    for needle in ["\"config\", \"init\"", "--merge-defaults", "--print"] {
        assert!(
            ps1.contains(needle),
            "в install_server.ps1 нет '{needle}' — конфиг не будет отрендерен/смержен"
        );
    }
    for stale in ["New-ServerConfig", "server_exclude_dirs.txt", "cfgBody"] {
        assert!(
            !ps1.contains(stale),
            "install_server.ps1 всё ещё строит YAML сам ('{stale}')"
        );
    }

    // 4) macOS-установщик — то же самое.
    let sh = read("installers/install_server.command");
    for needle in [
        "config init --profile server",
        "--merge-defaults",
        "--print",
    ] {
        assert!(
            sh.contains(needle),
            "в install_server.command нет '{needle}' — конфиг не будет отрендерен/смержен"
        );
    }
    for stale in ["gen_config", "server_exclude_dirs.txt", "CFG_BODY"] {
        assert!(
            !sh.contains(stale),
            "install_server.command всё ещё строит YAML сам ('{stale}')"
        );
    }
}

/// `install_server.ps1`: unattended-параметры, server-конфиг, fetch-хуки, хинт токена.
#[test]
fn install_server_ps1_has_unattended_params() {
    let s = read("installers/install_server.ps1");
    for (ps, _) in PARAMS {
        assert!(
            s.contains(&format!("${ps}")),
            "в install_server.ps1 нет параметра ${ps}"
        );
    }
    for needle in CONFIG_NEEDLES {
        assert!(s.contains(needle), "в install_server.ps1 нет '{needle}'");
    }
    // Обратная совместимость: прежние параметры M2 на месте.
    for p in ["Root", "Roles", "KeepAtLogOn"] {
        assert!(
            s.contains(&format!("${p}")),
            "прежний параметр ${p} удалён из install_server.ps1"
        );
    }
}

/// `install_server.command` (macOS): тот же unattended-набор (bash-флаги) + конфиг.
#[test]
fn install_server_command_has_unattended_params() {
    let s = read("installers/install_server.command");
    for (_, flag) in PARAMS {
        assert!(
            s.contains(flag),
            "в install_server.command нет флага {flag}"
        );
    }
    for needle in CONFIG_NEEDLES {
        assert!(
            s.contains(needle),
            "в install_server.command нет '{needle}'"
        );
    }
    // macOS-специфика: LaunchDaemon, ad-hoc подпись, снятие карантина, sudo-гейт.
    for needle in ["launchctl", "codesign", "xattr", "run with sudo"] {
        assert!(
            s.contains(needle),
            "в install_server.command нет '{needle}'"
        );
    }
    // Обратная совместимость: позиционные аргументы и прежний набор ролей.
    assert!(
        s.contains("watch,mcp-http,ui,llm-host"),
        "роли по умолчанию потеряны"
    );
}

/// `update.ps1` (M8 T4): серверный профиль (стоп/старт SCM-служб), окно обслуживания,
/// откат и листинг версий — плюс сохранённая desktop-ветка (Планировщик).
#[test]
fn update_ps1_is_server_profile_aware() {
    let s = read("installers/update.ps1");
    for needle in [
        "[ValidateSet(\"desktop\", \"server\")]",
        "$Profile",
        "hermes-disk-search-",
        "sc.exe stop",
        "sc.exe start",
        "Wait-ScState",
        "install_app_version.ps1",
        "app\\current",
        "-RollbackTo",
        "-ListVersions",
        "$WhatIf",
    ] {
        assert!(s.contains(needle), "в update.ps1 нет '{needle}'");
    }
    // Прежняя desktop-ветка (Планировщик) и останов резидента сохранены.
    for needle in [
        "Stop-ScheduledTask",
        "Start-ScheduledTask",
        "HermesDiskSearchLlmHost",
        "llm_host.exe",
    ] {
        assert!(s.contains(needle), "в update.ps1 нет '{needle}'");
    }
}

/// 0.20.3: после подмены версии update.ps1 перерегистрирует **точки запуска**.
///
/// Инцидент 16.10.2026: и службы (`binPath`), и задания Планировщика, и ярлык на
/// рабочем столе хранят **абсолютный** путь к бинарю. Плоская установка
/// (`<корень>\bin\hds.exe`) после `update.cmd` оставалась на прежних файлах, потому
/// что обновление подменяет только каталог версии за указателем `app\current`.
#[test]
fn update_ps1_repoints_launch_points() {
    let s = read("installers/update.ps1");
    for needle in [
        "Repoint-Layout",
        "function Get-ScBinPath",
        "function Pin-ServiceEnv",
        "service install $role",
        "install_ui_task.ps1",
        "install_llm_host_task.ps1",
        "install_autostart.ps1",
        "create_shortcut.ps1",
        "LaunchDaemons/com.hermes-disk-search.",
    ] {
        assert!(s.contains(needle), "в update.ps1 нет '{needle}'");
    }
}

/// 0.20.7 (инцидент 20.10.2026): `update.ps1` снимает **авто-перезапуск** служб на
/// время окна обновления.
///
/// SCM-действия «при сбое» поднимают роль через ~5 с. Пока копируется новая
/// версия, поднявшаяся роль снова создаёт `app\current` **реальным** каталогом
/// (писатель heartbeat), и `install_app_version.ps1` падает с «Папка не пуста»:
/// стенд остаётся без служб, указатель — на старой версии (живой прогон
/// 20.10.2026 при установке 0.20.6). Поэтому:
/// * снять авто-перезапуск **до** `sc.exe stop` (иначе роль вернётся в окно копирования);
/// * вернуть его после переключения указателя **и** в `finally` — прерванное
///   обновление не должно оставить роли без автоматического перезапуска;
/// * пустой аргумент `actions= ""` идёт через `cmd /c` (PowerShell 5.1 его не передаёт).
#[test]
fn update_ps1_suspends_auto_restart_while_switching() {
    let s = read("installers/update.ps1");
    for needle in [
        "function Suspend-Restart",
        "function Restore-Restart",
        "reset= 0 actions= \"\"",
        "cmd /c",
        "reset= 86400",
        "restart/5000/restart/5000/restart/30000",
        // SCM-действия есть только в Windows: на macOS скрипт не должен их трогать.
        "$env:OS -ne 'Windows_NT'",
    ] {
        assert!(s.contains(needle), "в update.ps1 нет '{needle}'");
    }
    // Порядок обязателен: авто-перезапуск снимается ДО остановки службы.
    let suspend = s
        .find("Suspend-Restart $n")
        .expect("update.ps1 не снимает авто-перезапуск перед остановкой роли");
    let stop = s
        .find("sc.exe stop $n")
        .expect("в update.ps1 нет остановки службы");
    assert!(
        suspend < stop,
        "авто-перезапуск снимается после sc.exe stop — роль успеет подняться"
    );
    // Восстановление: happy path + finally (и откат).
    assert!(
        s.matches("Restore-Restart").count() >= 4,
        "Restore-Restart вызывается меньше 4 раз: нет восстановления в finally/откате"
    );
    assert!(
        s.contains("} finally {"),
        "нет finally: прерванное обновление оставит роли без авто-перезапуска"
    );
}

/// F3 (ревью B3): `update.ps1` докачивает модель Nemotron-3 после переключения
/// версии — иначе обновлённая установка получает `diarization.backend: nemotron3`
/// без файла модели и каждое задание автотранскрибации падает и «забраковывается».
///
/// Требования: новый ключ `-NoModels`, каталог моделей по профилю
/// (`HDS_MODELS_DIR` → машинный для server → каталог движка для desktop), вызов
/// `fetch_nemotron3_model.ps1` из **новой** версии, и сбой загрузки **не** валит
/// обновление (предупреждение, не `throw`).
#[test]
fn update_ps1_fetches_nemotron3_model() {
    let s = read("installers/update.ps1");
    assert!(
        s.is_ascii(),
        "installers/update.ps1 не ASCII-only: PS 5.1 прочитает его как ANSI"
    );
    for needle in [
        "[switch]$NoModels",
        "fetch_nemotron3_model.ps1",
        "-ModelsDir $modelsDir",
        "$env:HDS_MODELS_DIR",
        "hermes-disk-search\\models",
        "OpenResearchTools\\models",
        "-not $WhatIf -and -not $NoModels",
        "elseif ($WhatIf -and -not $NoModels)",
    ] {
        assert!(
            s.contains(needle),
            "в update.ps1 нет '{needle}' — модель не докачается (или шаг не пропускается)"
        );
    }
    // Сбой загрузки — предупреждение, а не падение обновления.
    let block_start = s
        .find("Nemotron-3 model top-up")
        .expect("в update.ps1 нет шага докачки модели");
    let block = &s[block_start..];
    let block = &block[..block.find("--- 4.").unwrap_or(block.len())];
    assert!(block.contains("try {"), "загрузка модели без try/catch");
    assert!(block.contains("catch"), "загрузка модели без catch");
    assert!(
        !block.contains("throw"),
        "ошибка загрузки модели валит обновление (throw): модель можно докачать вручную"
    );
    // Шаг идёт после установки новой версии (указатель уже переключён).
    let install = s
        .find("install_app_version.ps1")
        .expect("update.ps1 не ставит новую версию");
    assert!(
        install < block_start,
        "докачка модели раньше установки новой версии — fetch-скрипта ещё нет"
    );
}

/// F3: macOS-обёртка объясняет докачку модели и даёт ручную команду
/// (`-NoModels` пробрасывается в `update.ps1` через `"$@"`).
#[test]
fn update_command_fetches_nemotron3_model() {
    let s = read("installers/update.command");
    assert!(s.is_ascii(), "update.command должен быть ASCII-only");
    for needle in ["fetch_nemotron3_model.ps1", "-NoModels", "\"$@\""] {
        assert!(
            s.contains(needle),
            "в update.command нет '{needle}' — на macOS модель не докачается/не объяснится"
        );
    }
}

/// Подсказки установщиков не должны ссылаться на несуществующую подкоманду:
/// `hds service` знает `run|install|uninstall|start|stop|status|root`, а `restart` есть
/// только у `mcp-http`. Перезапуск роли — это `service stop <role>` + `service start <role>`
/// (живая находка при правке документации, 16.10.2026).
#[test]
fn installers_hint_a_working_restart() {
    for f in [
        "installers/install_server.ps1",
        "installers/install_server.command",
    ] {
        let s = read(f);
        assert!(
            !s.contains("service restart"),
            "в {f} подсказка ссылается на несуществующую подкоманду hds service restart"
        );
        assert!(
            s.contains("service stop <role>"),
            "в {f} нет подсказки, как перезапустить роль (stop + start)"
        );
    }
}

/// PowerShell 5.1 **не передаёт сторонней программе пустой аргумент**, поэтому запись
/// `sc.exe failure <имя> reset= 0 actions= ""` прямо в PowerShell падает с «недопустимый
/// параметр» — пустое значение не доходит до `sc.exe` (живая находка 16.10.2026: читатель
/// скопировал команду из руководства и получил ошибку). Каждая такая команда в документации
/// обязана запускаться через `cmd /c` (в той же строке или в следующих двух — шаблон команды
/// может быть вынесен в переменную).
#[test]
fn docs_wrap_the_empty_sc_failure_arg_in_cmd() {
    let doc = read("README_SERVER_MULTIUSER.md");
    let lines: Vec<&str> = doc.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !(line.contains("sc.exe failure") && line.contains("actions=") && line.contains("\"\""))
        {
            continue;
        }
        let nearby = lines[i..lines.len().min(i + 3)].join(" ");
        assert!(
            nearby.contains("cmd /c"),
            "README_SERVER_MULTIUSER.md:{} — команда с пустым аргументом должна идти через cmd /c: {line}",
            i + 1
        );
    }
}

/// 0.20.3: установщик сервера раскладывает поставку по версиям, когда корень
/// отличается от распакованного архива (иначе обновление/откат не меняли бы то, что
/// запускают роли). macOS-скрипт держит ту же логику.
#[test]
fn installers_lay_out_the_version() {
    let ps1 = read("installers/install_server.ps1");
    for needle in [
        "install_app_version.ps1",
        "app\\current\\bin\\hds.exe",
        "$installTarget",
        "flat layout",
    ] {
        assert!(ps1.contains(needle), "в install_server.ps1 нет '{needle}'");
    }
    let sh = read("installers/install_server.command");
    for needle in [
        "install_app_version.ps1",
        "app/current/bin/hds",
        "INSTALL_TARGET",
        "PLAN_LAYOUT",
    ] {
        assert!(
            sh.contains(needle),
            "в install_server.command нет '{needle}'"
        );
    }
}

/// Находка при установке 0.20.5: `schtasks /delete` для отсутствующей задачи пишет
/// в stderr, а установщик идёт с `$ErrorActionPreference='Stop'` — PowerShell 5.1
/// превращает это в `NativeCommandError` и установка падает. Снятие задач должно
/// быть best-effort: командлеты Планировщика либо защищённый fallback.
#[test]
fn install_server_clears_tasks_without_native_stderr_abort() {
    let s = read("installers/install_server.ps1");
    assert!(
        s.contains("Get-ScheduledTask") && s.contains("Unregister-ScheduledTask"),
        "install_server.ps1 должен снимать AtLogOn-задачи через Get-ScheduledTask/Unregister-ScheduledTask"
    );
    assert!(
        !s.contains("schtasks /delete /tn $task /f *> $null"),
        "небезопасный schtasks /delete вернулся: EAP=Stop + stderr снова уронят установку"
    );
    assert!(
        s.contains("$ErrorActionPreference = 'Continue'"),
        "fallback schtasks должен временно переключать ErrorActionPreference на Continue"
    );
}

/// Службы работают как LocalSystem: `-Fetch` обязан класть модели/движок в машинные
/// каталоги и закреплять те же пути в machine env, иначе на чистой машине сервер
/// поднимется без моделей (они окажутся в профиле установившего администратора).
#[test]
fn install_server_fetches_models_into_machine_dirs() {
    let s = read("installers/install_server.ps1");
    for needle in [
        "HDS_MODELS_DIR",
        "HDS_ENGINE_DIR",
        "LLAMA_RUNTIME_DIR",
        "SetEnvironmentVariable($Name, $p, \"Machine\")",
        "-ModelsDir",
        "-EngineDir",
        "-RuntimeDir",
        "fetch_whisper_model.ps1",
        "fetch_diarization_model.ps1",
        "fetch_clip_models.ps1",
        "clip_onnx",
    ] {
        assert!(
            s.contains(needle),
            "в install_server.ps1 нет '{needle}' — модели снова уйдут в профиль админа"
        );
    }
}

/// macOS: те же машинные каталоги внутри state dir + закрепление в LaunchDaemon.
#[test]
fn mac_installer_fetches_into_state_dirs() {
    let s = read("installers/install_server.command");
    for needle in [
        "MODELS_DIR=\"$STATE/models\"",
        "ENGINE_DIR=\"$STATE/engine\"",
        "LLAMA_DIR=\"$STATE/llama-runtime\"",
        "-ModelsDir",
        "-EngineDir",
        "-RuntimeDir",
        "HDS_MODELS_DIR",
        "HDS_ENGINE_DIR",
        "LLAMA_RUNTIME_DIR",
        "clip_onnx",
    ] {
        assert!(
            s.contains(needle),
            "в install_server.command нет '{needle}'"
        );
    }
}

/// Ошибка установки не должна вешать автоматизированный запуск на `pause`.
#[test]
fn install_server_cmd_pauses_only_interactively() {
    let s = read("install-server.cmd");
    assert!(
        s.contains("if not defined HDS_NONINTERACTIVE pause"),
        "install-server.cmd: pause должен быть условным (HDS_NONINTERACTIVE)"
    );
    assert!(
        !s.lines().any(|l| l.trim().eq_ignore_ascii_case("pause")),
        "install-server.cmd: безусловный pause снова появился"
    );
}

/// Корень-диск вида `D:\` в `-Roots "D:\"` PowerShell 5.1 разбирает как `D:\" …`.
/// Руководство обязано предупреждать (находка при установке 0.20.5).
#[test]
fn server_readme_documents_drive_root_quoting() {
    let s = read("README_SERVER_MULTIUSER.md");
    assert!(
        s.contains("обратный слэш"),
        "README_SERVER_MULTIUSER.md: нет предупреждения про `-Roots \"D:\\\"`"
    );
    assert!(
        s.contains("-Roots D:\\"),
        "README_SERVER_MULTIUSER.md: нет рабочего примера `-Roots D:\\`"
    );
}

/// Ручной запуск fetch-скриптов на сервере тоже обязан уважать машинный каталог:
/// иначе модели уйдут в профиль администратора и службы их не увидят.
#[test]
fn fetch_scripts_honour_hds_models_dir() {
    for f in [
        "installers/fetch_whisper_model.ps1",
        "installers/fetch_diarization_model.ps1",
    ] {
        let s = read(f);
        assert!(
            s.contains("$env:HDS_MODELS_DIR"),
            "в {f} нет fallback на HDS_MODELS_DIR"
        );
    }
}

/// W6: установщик модели Nemotron-3 — ASCII-only, пин ревизии (не `main`), размер и
/// sha256 модели, скачивание в `.part` с проверкой перед `Move-Item`.
#[test]
fn nemotron3_fetch_script_is_pinned_and_verified() {
    let s = read("installers/fetch_nemotron3_model.ps1");
    assert!(
        s.is_ascii(),
        "installers/fetch_nemotron3_model.ps1 не ASCII-only: PS 5.1 прочитает его как ANSI"
    );
    for needle in [
        // Ревизия и файл в Hugging Face (пин, не main).
        "4d2a8bc71f5c896ec40faa59732e6716295edaf2",
        "nemotron-3-diarization/nemotron3_diar_v3.onnx",
        // Размер и sha256 из W3 (`hds_diarize::{NEMOTRON_SIZE, NEMOTRON_SHA256}`).
        "400506656",
        "915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a",
        // Скачивание во временный файл, проверка, затем перемещение.
        ".part",
        "Get-FileHash",
        "Move-Item",
        // Каталоги моделей: службы видят только машинный/`HDS_MODELS_DIR`.
        "HDS_MODELS_DIR",
        "ProgramData",
    ] {
        assert!(
            s.contains(needle),
            "в fetch_nemotron3_model.ps1 нет '{needle}'"
        );
    }
    assert!(
        !s.contains("/resolve/main/"),
        "fetch_nemotron3_model.ps1 качает с main — ревизия должна быть пинована"
    );
}

/// W6: пакет кладёт `DirectML.dll` рядом с бинарями и не пакует веса моделей;
/// все три установщика зовут новый fetch-скрипт, `.gitignore` держит `*.onnx`.
#[test]
fn release_package_ships_directml_and_no_weights() {
    let build = read("installers/build_rust_release.ps1");
    for needle in ["DirectML.dll", "\"$b.exe\"", "*.onnx", "throw"] {
        assert!(
            build.contains(needle),
            "в build_rust_release.ps1 нет '{needle}' — пакет соберётся без DirectML/с весами"
        );
    }
    for f in [
        "installers/install_server.ps1",
        "installers/install_server.command",
        "setup.ps1",
    ] {
        assert!(
            read(f).contains("fetch_nemotron3_model.ps1"),
            "в {f} нет вызова fetch_nemotron3_model.ps1"
        );
    }
    let ignore = read(".gitignore");
    assert!(
        ignore.contains("*.onnx"),
        ".gitignore: веса Nemotron-3 (.onnx) могут попасть в git"
    );
}
