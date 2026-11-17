//! Тест клиента на mock-воркере (stdlib-only) — изолирует транспорт,
//! типизацию ответов и пропуск progress-нотификаций от реального Python.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_anonymize::error::ErrorKind;
use hds_anonymize::{discover_python, Client, ClientConfig};

fn mock_cfg() -> Option<ClientConfig> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = root.parent()?.parent()?.to_path_buf();
    let py = discover_python(&repo)?;
    let mut cfg = ClientConfig::new(&py, &repo);
    // mock-воркер вместо штатного (поле script, как у hds-extract тестов)
    cfg.script = Some(root.join("tests").join("mock_worker.py"));
    cfg.request_timeout = Duration::from_secs(20);
    Some(cfg)
}

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hds-anon-client-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn mock_handshake_hello_and_warmup() {
    let cfg = match mock_cfg() {
        Some(c) => c,
        None => {
            println!("пропуск: нет интерпретатора Python");
            return;
        }
    };
    let mut client = Client::spawn(cfg).expect("mock-воркер должен запуститься");
    let hello = client.hello().unwrap();
    assert_eq!(hello.protocol, 1);
    assert_eq!(hello.model, "mock/gliner-pii");
    assert!(hello.has("anonymize"));
    assert!(hello.formats.contains(&"xml".to_string()));

    let warm = client.warmup().unwrap();
    assert_eq!(warm.backend, "mock onnx");
    assert_eq!(warm.providers, vec!["MockExecutionProvider".to_string()]);

    client.shutdown();
}

#[test]
fn anonymize_values_and_deanonymize_roundtrip_with_progress() {
    let cfg = match mock_cfg() {
        Some(c) => c,
        None => return,
    };
    let dir = tmp_dir("roundtrip");
    let src = dir.join("doc.txt");
    std::fs::write(&src, "Исполнитель: Иванов Иван, тел. +7 912 345-67-89\n").unwrap();

    let mut client = Client::spawn(cfg).unwrap();
    let out_dir = dir.clone();
    let res = client
        .anonymize(
            &src,
            &out_dir,
            Some(&["Иванов Иван".to_string(), "+7 912 345-67-89".to_string()]),
        )
        .expect("anonymize_values должен пройти (нотификации пропущены)");
    // Rust строил путь: doc.anonymized.txt
    assert!(
        res.out_path.ends_with("doc.anonymized.txt"),
        "{}",
        res.out_path
    );
    assert!(Path::new(&res.out_path).is_file());
    assert_eq!(res.unmatched, Vec::<String>::new());
    let anon_text = std::fs::read_to_string(&res.out_path).unwrap();
    assert!(!anon_text.contains("Иванов Иван"));
    assert!(res.tokens.iter().any(|t| t.token == "[PERSON_1]"));

    // деанонимизация по тем же токенам
    let de = client
        .deanonymize(
            Path::new(&res.out_path),
            &out_dir,
            Some(&["[PERSON_1]".to_string(), "[MISC_1]".to_string()]),
        )
        .unwrap();
    assert!(de.out_path.ends_with("doc.deanon.txt"), "{}", de.out_path);
    let de_text = std::fs::read_to_string(&de.out_path).unwrap();
    assert!(de_text.contains("Иванов Иван"));
    assert!(de_text.contains("+7 912 345-67-89"));
    assert_eq!(de.unmatched, Vec::<String>::new());

    // inspect без значений PII
    let info = client.inspect(Path::new(&res.out_path)).unwrap();
    assert!(info.supported);
    assert!(info.placeholders.iter().any(|p| p.token == "[PERSON_1]"));
    assert!(!format!("{info:?}").contains("Иванов"));

    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn full_anonymize_without_models_maps_kind() {
    let cfg = match mock_cfg() {
        Some(c) => c,
        None => return,
    };
    let dir = tmp_dir("modelmissing");
    let src = dir.join("doc.txt");
    std::fs::write(&src, "текст\n").unwrap();

    let mut client = Client::spawn(cfg).unwrap();
    let err = client.anonymize(&src, &dir, None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ModelMissing);
    assert!(err.hint.is_some(), "hint про установщик должен дойти");

    // изоляция: после ошибки следующий запрос обслужен
    let hello = client.hello().unwrap();
    assert_eq!(hello.protocol, 1);
    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn partial_deanonymize_reports_unmatched() {
    let cfg = match mock_cfg() {
        Some(c) => c,
        None => return,
    };
    let dir = tmp_dir("partial");
    let src = dir.join("doc.txt");
    std::fs::write(&src, "Сотрудник: Иванов Иван\n").unwrap();

    let mut client = Client::spawn(cfg).unwrap();
    let res = client
        .anonymize(&src, &dir, Some(&["Иванов Иван".to_string()]))
        .unwrap();
    let de = client
        .deanonymize(
            Path::new(&res.out_path),
            &dir,
            Some(&["[PERSON_999]".to_string()]),
        )
        .unwrap();
    assert_eq!(de.unmatched, vec!["[PERSON_999]".to_string()]);
    assert!(de.restored.is_empty());
    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn agent_and_vault_acts_on_mock() {
    let cfg = match mock_cfg() {
        Some(c) => c,
        None => return,
    };
    let dir = tmp_dir("agent");
    let src = dir.join("ТЗ.txt");
    std::fs::write(&src, "Исполнитель: Иванов Иван\nПередать Иванову Ивану\n").unwrap();

    let mut client = Client::spawn(cfg).unwrap();
    // образ анонимной зоны: имя файла сохраняется, метки проекта с суффиксом
    let res = client
        .anonymize_agent(&src, &dir, "P0001", Some("42"))
        .expect("agent-акт должен пройти");
    assert!(res.out_path.ends_with("ТЗ.txt"), "{}", res.out_path);
    assert_eq!(res.mode, "agent");
    assert_eq!(res.images, 1);
    let anon = std::fs::read_to_string(&res.out_path).unwrap();
    assert!(anon.contains("[PERSON_1|дат]"), "{anon}");
    assert!(!anon.contains("Иванов Иван"));

    // текст реплики: метки проекта, значения наружу не отдаются
    let text = client.anonymize_text("P0001", "Писал Иванов Иван").unwrap();
    assert_eq!(text.project_id, "P0001");
    assert!(text.text.contains("[PERSON_1]"));
    assert!(!format!("{text:?}").contains("Иванов"));

    // снимок: rev/labels/others/keep (в тесте значения не печатаем)
    let snapshot = client.snapshot("P0001").unwrap();
    assert_eq!(snapshot.rev, 7);
    assert_eq!(snapshot.labels.len(), 1);
    assert_eq!(snapshot.labels[0].label, "[PERSON_1]");
    assert_eq!(snapshot.labels[0].forms.len(), 2);
    assert!(snapshot.keep.contains(&"1С:ERP".to_string()));
    assert_eq!(client.vault_rev().unwrap(), 7);

    // «Скрыть ещё…» и «Объединить метки»
    let label = client
        .label_for_value("P0001", "Петров Пётр", "PERSON")
        .unwrap();
    assert_eq!(label.label.as_deref(), Some("[PERSON_2]"));
    assert!(label.confident);
    let merged = client
        .merge_entities("P0001", "[PERSON_1]", &["[PERSON_3]".to_string()])
        .unwrap();
    assert_eq!(merged["keep"], "[PERSON_1]");

    // keep_terms (D19)
    let terms = client.keep_terms().unwrap();
    assert!(terms.iter().any(|t| t == "ТЗ"));
    assert!(client.add_keep_term("СБИС").unwrap());

    // текстовый образ (D18): <имя>.<ext>.md с маркером
    let image = client
        .text_image(&src, &dir, "P0001", Some("42"), None)
        .unwrap();
    assert!(image.out_path.ends_with("ТЗ.txt.md"), "{}", image.out_path);
    assert_eq!(image.marker, "<!-- hds-agent/P0001/42 -->");
    assert!(std::fs::read_to_string(&image.out_path)
        .unwrap()
        .contains("<!-- hds-agent/P0001/42 -->"));

    // office_* (TR-3.5)
    let read = client.office_read(&src).unwrap();
    assert_eq!(read.kind, "docx");
    assert!(read.markdown.contains("mock"));
    let edited = client
        .office_edit(&src, &[serde_json::json!({"op": "replace_text"})])
        .unwrap();
    assert_eq!(edited.applied, 1);

    client.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_path_inspect_maps_internal_error() {
    let cfg = match mock_cfg() {
        Some(c) => c,
        None => return,
    };
    let mut client = Client::spawn(cfg).unwrap();
    // mock-воркер падает на чтении несуществующего файла → runtime отдаёт
    // -32603; клиент маппит в Internal (изоляция: воркер жив)
    let missing = Path::new("Z:/точно/нет/такого.xml");
    let err = client.inspect(missing).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Internal);
    let hello = client.hello().unwrap();
    assert_eq!(hello.protocol, 1, "после ошибки воркер жив");
    client.shutdown();
}
