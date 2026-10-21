//! `hds config init`: рендер серверного конфига из `server_config.example.yaml`.
//!
//! Зачем (релиз 0.20.6): серверный конфиг раньше собирали **два** независимых
//! генератора (PowerShell и bash), и они разъехались — в серверном конфиге не
//! оказалось `index.exclude_dirs`, который есть в desktop-шаблоне. Тест фиксирует:
//! * шаблон рендерится в валидный YAML и содержит серверные дефолты;
//! * пустые параметры убирают строки, пустые секции — целиком;
//! * список исключений совпадает с desktop-шаблоном (нет drift между стендами);
//! * merge дефолтов в существующий конфиг идемпотентен и не портит комментарии.

use std::path::PathBuf;

use hds_cli::cmd::config::{
    default_exclude_dirs, merge_defaults_into, render_server_config, InitOpts, MergeOutcome,
};
use serde_yaml::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn server_template() -> String {
    let p = repo_root().join("server_config.example.yaml");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("прочитать {}: {e}", p.display()))
}

fn render(o: &InitOpts) -> Value {
    let text = render_server_config(&server_template(), o).expect("рендер должен пройти");
    serde_yaml::from_str(&text).expect("отрендеренный конфиг обязан быть валидным YAML")
}

fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

/// Минимальный набор параметров (как `install_server.ps1` по умолчанию).
fn defaults() -> InitOpts {
    InitOpts {
        profile: Some("server".into()),
        mcp_host: Some("127.0.0.1".into()),
        mcp_port: Some(8787),
        ui_host: Some("127.0.0.1".into()),
        ui_port: Some(8765),
        ..Default::default()
    }
}

#[test]
fn template_renders_valid_config_with_server_defaults() {
    let cfg = render(&defaults());

    assert_eq!(at(&cfg, "profile").and_then(Value::as_str), Some("server"));
    assert_eq!(
        at(&cfg, "access.enabled").and_then(Value::as_bool),
        Some(false),
        "профиль сам по себе не включает авторизацию"
    );
    assert_eq!(
        at(&cfg, "access.token_ttl_days").and_then(Value::as_i64),
        Some(90)
    );

    // Исключения — серверный дефолт (регрессия 0.20.5: их не было вовсе).
    let excl: Vec<&str> = at(&cfg, "index.exclude_dirs")
        .and_then(Value::as_sequence)
        .expect("index.exclude_dirs")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(excl.len(), 20, "{excl:?}");
    assert!(excl.contains(&"$RECYCLE.BIN"), "{excl:?}");
    assert!(excl.contains(&"System Volume Information"), "{excl:?}");

    // Пустые параметры убирают строки и осиротевшие секции.
    assert!(at(&cfg, "index.roots").is_none(), "roots не задан");
    assert!(
        at(&cfg, "index.exclude_paths").is_none(),
        "нет media-каталогов"
    );
    assert!(cfg.get("gpu").is_none(), "ни preset, ни device не заданы");
    assert!(cfg.get("llm_server").is_none(), "parallel не задан");

    // Автотранскрибация выключена, но секция задокументирована.
    assert_eq!(
        at(&cfg, "auto_transcribe.enabled").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        at(&cfg, "auto_transcribe.source_disposal").and_then(Value::as_str),
        Some("delete")
    );
    assert!(at(&cfg, "auto_transcribe.inbox_dir").is_none());

    assert_eq!(
        at(&cfg, "mcp_http.port").and_then(Value::as_i64),
        Some(8787)
    );
    assert_eq!(at(&cfg, "ui.port").and_then(Value::as_i64), Some(8765));
}

/// W5: серверный шаблон (его рендерит и `hds config init`) несёт новый выбор
/// диаризации — Nemotron-3 по умолчанию, и в секции, и в теле запроса.
#[test]
fn template_renders_nemotron3_diarization() {
    let cfg = render(&defaults());
    assert_eq!(
        at(&cfg, "diarization.backend").and_then(Value::as_str),
        Some("nemotron3")
    );
    assert_eq!(
        at(&cfg, "diarization.required").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        at(&cfg, "diarization.nemotron3.device").and_then(Value::as_str),
        Some("auto")
    );
    assert_eq!(
        at(&cfg, "diarization.nemotron3.chunk_min").and_then(Value::as_f64),
        Some(10.0)
    );
    assert_eq!(
        at(&cfg, "diarization.asr.trim_leading_silence").and_then(Value::as_bool),
        Some(true)
    );
    // Тело запроса сильнее секции — клиент тоже должен просить nemotron3.
    assert_eq!(
        at(&cfg, "auto_transcribe.diarization_backend").and_then(Value::as_str),
        Some("nemotron3")
    );
}

#[test]
fn template_renders_all_parameters() {
    let o = InitOpts {
        roots: Some("D:\\;E:\\Archive, F:\\Docs".into()),
        exclude_paths: Some("D:\\Soft".into()),
        parallel: Some(2),
        gpu_preset: Some("rtx3060".into()),
        access_enabled: Some(true),
        enable_transcribe: true,
        media_inbox: Some("D:\\media-in".into()),
        media_out: Some("D:\\media-out".into()),
        source_disposal: Some("keep".into()),
        mcp_port: Some(9787),
        ui_host: Some("0.0.0.0".into()),
        ..defaults()
    };
    let cfg = render(&o);
    let roots: Vec<&str> = at(&cfg, "index.roots")
        .and_then(Value::as_sequence)
        .expect("roots")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(roots, vec!["D:\\", "E:\\Archive", "F:\\Docs"]);
    let excl_paths: Vec<&str> = at(&cfg, "index.exclude_paths")
        .and_then(Value::as_sequence)
        .expect("exclude_paths")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        excl_paths,
        vec!["D:\\Soft", "D:\\media-in", "D:\\media-out"],
        "каталоги автотранскрибации исключаются явно"
    );
    assert_eq!(
        at(&cfg, "llm_server.parallel").and_then(Value::as_i64),
        Some(2)
    );
    assert_eq!(
        at(&cfg, "gpu.preset").and_then(Value::as_str),
        Some("rtx3060"),
        "preset вписан и не испорчен кавычками"
    );
    assert!(
        at(&cfg, "gpu.device_index").is_none(),
        "preset и device_index взаимоисключимы"
    );
    assert_eq!(
        at(&cfg, "access.enabled").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        at(&cfg, "auto_transcribe.enabled").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        at(&cfg, "auto_transcribe.inbox_dir").and_then(Value::as_str),
        Some("D:\\media-in")
    );
    assert_eq!(
        at(&cfg, "auto_transcribe.out_dir").and_then(Value::as_str),
        Some("D:\\media-out")
    );
    assert_eq!(
        at(&cfg, "auto_transcribe.source_disposal").and_then(Value::as_str),
        Some("keep")
    );
    assert_eq!(
        at(&cfg, "mcp_http.port").and_then(Value::as_i64),
        Some(9787)
    );
    assert_eq!(at(&cfg, "ui.host").and_then(Value::as_str), Some("0.0.0.0"));
}

#[test]
fn gpu_device_is_rendered_when_preset_is_absent() {
    let o = InitOpts {
        gpu_device: Some("1".into()),
        ..defaults()
    };
    let cfg = render(&o);
    assert_eq!(
        at(&cfg, "gpu.device_index").and_then(Value::as_i64),
        Some(1)
    );
    assert!(at(&cfg, "gpu.preset").is_none());
}

#[test]
fn broken_parameters_are_rejected() {
    let both = InitOpts {
        gpu_device: Some("1".into()),
        gpu_preset: Some("rtx".into()),
        ..defaults()
    };
    assert!(render_server_config(&server_template(), &both).is_err());

    let bad_disposal = InitOpts {
        source_disposal: Some("shred".into()),
        ..defaults()
    };
    assert!(render_server_config(&server_template(), &bad_disposal).is_err());

    let bad_parallel = InitOpts {
        parallel: Some(100),
        ..defaults()
    };
    assert!(render_server_config(&server_template(), &bad_parallel).is_err());
}

#[test]
fn unknown_placeholder_in_template_is_an_error() {
    let tpl = "index:\n  roots: ${NOPE}\n";
    let err = render_server_config(tpl, &defaults()).expect_err("должен быть отказ");
    assert!(err.contains("NOPE"), "{err}");
}

#[test]
fn broken_yaml_after_substitution_is_an_error() {
    // шаблон сам по себе битый: рендер обязан отказать ДО записи файла
    let tpl = "index:\n  roots: ${ROOTS}\n  bad: [1,\n";
    let o = InitOpts {
        roots: Some("D:\\".into()),
        ..defaults()
    };
    let err = render_server_config(tpl, &o).expect_err("должен быть отказ");
    assert!(err.contains("YAML"), "{err}");
}

#[test]
fn server_and_desktop_templates_share_the_same_exclusions() {
    let server = default_exclude_dirs(&server_template()).expect("exclude_dirs в шаблоне");
    let desktop_text = std::fs::read_to_string(repo_root().join("config.example.yaml"))
        .expect("config.example.yaml");
    let desktop: Value = serde_yaml::from_str(&desktop_text).expect("desktop-шаблон");
    let desktop_list: Vec<String> = desktop
        .get("index")
        .and_then(|i| i.get("exclude_dirs"))
        .and_then(Value::as_sequence)
        .expect("index.exclude_dirs в desktop-шаблоне")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    assert_eq!(
        server, desktop_list,
        "серверный и desktop-шаблон должны пропускать одинаковые каталоги"
    );
    assert!(server.iter().any(|s| s == "$RECYCLE.BIN"));
}

#[test]
fn merge_defaults_adds_the_key_once_and_keeps_comments() {
    let dirs = default_exclude_dirs(&server_template()).expect("exclude_dirs");
    // конфиг стенда: комментарии администратора и свои значения
    let before = "\
profile: server
# мой комментарий: не терять!
index:
  roots:
    - 'D:\\'
  exclude_paths: ['D:\\Soft']
# хвостовой комментарий
";
    let (after, outcome) = merge_defaults_into(before, &dirs);
    assert_eq!(outcome, MergeOutcome::Added);

    let mut expected: Vec<String> = before.lines().map(str::to_string).collect();
    expected.insert(
        3,
        format!("  exclude_dirs: {}", {
            let quoted: Vec<String> = dirs.iter().map(|d| format!("'{d}'")).collect();
            format!("[{}]", quoted.join(", "))
        }),
    );
    let expected = format!("{}\n", expected.join("\n"));
    assert_eq!(
        after, expected,
        "добавлена ровно одна строка, остальное цело"
    );

    // YAML после вставки валиден, список на месте, свои значения сохранены
    let cfg: Value = serde_yaml::from_str(&after).expect("валидный YAML после merge");
    assert_eq!(
        at(&cfg, "index.exclude_dirs")
            .and_then(Value::as_sequence)
            .map(|s| s.len()),
        Some(dirs.len())
    );
    assert_eq!(
        at(&cfg, "index.roots")
            .and_then(Value::as_sequence)
            .map(|s| s.len()),
        Some(1)
    );

    // идемпотентно: второй прогон ничего не меняет
    let (again, outcome2) = merge_defaults_into(&after, &dirs);
    assert_eq!(outcome2, MergeOutcome::AlreadyPresent);
    assert_eq!(again, after);
}

#[test]
fn merge_defaults_creates_index_section_and_keeps_crlf() {
    let dirs = vec![
        "$RECYCLE.BIN".to_string(),
        "System Volume Information".to_string(),
    ];
    let before = "profile: server\r\naccess:\r\n  enabled: true\r\n";
    let (after, outcome) = merge_defaults_into(before, &dirs);
    assert_eq!(outcome, MergeOutcome::Added);
    assert!(
        after.contains("\r\n"),
        "переводы строк сохранены: {after:?}"
    );
    assert!(
        after.contains(
            "index:\r\n  exclude_dirs: ['$RECYCLE.BIN', 'System Volume Information']\r\n"
        ),
        "{after:?}"
    );
    // Ни один `\n`/`\r` не остался вне пары CRLF (последовательность `\r\n\r\n`
    // содержит `\n\r` — поэтому проверяем не подстроку, а остаток после вычистки).
    let bare = after.replace("\r\n", "");
    assert!(
        !bare.contains('\n') && !bare.contains('\r'),
        "смешанные окончания: {after:?}"
    );
    let cfg: Value = serde_yaml::from_str(&after).expect("валидный YAML");
    assert_eq!(
        at(&cfg, "index.exclude_dirs")
            .and_then(Value::as_sequence)
            .map(|s| s.len()),
        Some(2)
    );
}
