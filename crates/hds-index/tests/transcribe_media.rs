//! W3: тесты медиа-ветки `hds-index` — маршрутизация вида `media` на владельца
//! GPU (`/internal/transcribe`) и деградация без владельца (файл не падает).
//!
//! Заглушка владельца — минимальный TCP-сервер на loopback (без зависимостей):
//! принимает один запрос и отдаёт фиксированный JSON.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use hds_core::error::Result;
use hds_index::chunker::Segment;
use hds_index::sidecar::Extractor;
use hds_index::transcribe::{MediaRouter, TranscribeClient, TranscribeConfig};

/// Внутренний извлекатель-заглушка (для не-медиа).
struct FakeInner;

impl Extractor for FakeInner {
    fn extract(&self, _p: &Path) -> Result<(String, Vec<Segment>)> {
        Ok((
            "text".into(),
            vec![Segment {
                text: "inner".into(),
                ..Default::default()
            }],
        ))
    }
}

fn temp_file(tag: &str, name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hds-media-{}-{}", std::process::id(), tag));
    let _ = std::fs::create_dir_all(&d);
    let p = d.join(name);
    std::fs::write(&p, b"x").unwrap();
    p
}

/// Заглушка владельца: один запрос → JSON-тело. Возвращает (url, handle).
fn stub_server(body: String) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let h = std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 16384];
            let _ = s.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes());
            let _ = s.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), h)
}

fn client(url: &str, enabled: bool) -> TranscribeClient {
    TranscribeClient::new(TranscribeConfig {
        url: url.into(),
        enabled,
        ..Default::default()
    })
}

#[test]
fn media_routed_to_owner() {
    // второй сегмент пустой — должен отфильтроваться (как в Whisper-парсере)
    let body = r#"{"segments":[{"text":"привет мир","t_start":0.5,"t_end":2.25},"#.to_string()
        + r#"{"text":"  ","t_start":3.0,"t_end":4.0}]}"#;
    let (url, h) = stub_server(body);
    let router = MediaRouter::with_client(FakeInner, client(&url, true), true);
    let p = temp_file("route", "клип.wav");
    let (kind, segs) = router.extract(&p).unwrap();
    h.join().unwrap();
    assert_eq!(kind, "media");
    assert_eq!(segs.len(), 2, "ведущий + один непустой сегмент");
    assert!(segs[0].text.starts_with("Медиафайл: "));
    assert!(segs[0].text.contains("клип.wav"));
    assert_eq!(segs[1].text, "привет мир");
    assert_eq!(segs[1].t_start, Some(0.5));
    assert_eq!(segs[1].t_end, Some(2.25));
}

#[test]
fn non_media_delegates_to_inner() {
    let router = MediaRouter::with_client(FakeInner, client("http://127.0.0.1:1", true), true);
    let p = temp_file("txt", "заметка.txt");
    let (kind, segs) = router.extract(&p).unwrap();
    assert_eq!(kind, "text");
    assert_eq!(segs[0].text, "inner");
}

#[test]
fn owner_down_is_non_fatal() {
    // порт 1 закрыт → connect error; медиа-файл всё равно «индексируется»
    let router = MediaRouter::with_client(FakeInner, client("http://127.0.0.1:1", true), true);
    let p = temp_file("down", "клип.mp4");
    let (kind, segs) = router.extract(&p).unwrap();
    assert_eq!(kind, "media");
    assert_eq!(segs.len(), 2);
    assert!(
        segs[1].text.starts_with("Транскрипция недоступна"),
        "seg[1]={}",
        segs[1].text
    );
}

#[test]
fn transcribe_disabled_lead_only() {
    let router = MediaRouter::with_client(FakeInner, client("http://127.0.0.1:1", false), false);
    let p = temp_file("off", "клип.wav");
    let (kind, segs) = router.extract(&p).unwrap();
    assert_eq!(kind, "media");
    assert_eq!(segs.len(), 1);
    assert!(segs[0].text.starts_with("Медиафайл: "));
}

#[test]
fn from_config_reads_keys() {
    let cfg: hds_core::config::Config = serde_yaml::from_str(
        "index:\n  transcribe: true\n  whisper_mode: speech\n  whisper_custom: 7\n\
         \x20 whisper_gpu: 2\n  transcribe_url: \"http://127.0.0.1:9990/\"\n\
         gpu:\n  device_index: 1\n",
    )
    .unwrap();
    let tc = TranscribeConfig::from_config(&cfg);
    assert_eq!(tc.url, "http://127.0.0.1:9990");
    assert_eq!(tc.mode, "speech");
    assert_eq!(tc.custom, "7");
    assert_eq!(tc.gpu, 2);
    assert!(tc.enabled);
}

#[test]
fn from_config_defaults() {
    let cfg: hds_core::config::Config = serde_yaml::from_str("index:\n  roots: []\n").unwrap();
    let tc = TranscribeConfig::from_config(&cfg);
    assert_eq!(tc.url, hds_index::DEFAULT_TRANSCRIBE_URL);
    assert_eq!(tc.mode, "subtitle");
    assert_eq!(tc.custom, "4.5");
    assert_eq!(tc.gpu, 0);
    assert!(tc.enabled, "index.transcribe по умолчанию включён");
}

#[test]
fn segments_from_json_filters_empty() {
    let v: serde_json::Value = serde_json::from_str(
        r#"{"segments":[{"text":"a","t_start":1.0,"t_end":2.0},{"text":""},{"text":" b "}]}"#,
    )
    .unwrap();
    let segs = hds_index::transcribe::segments_from_json(&v);
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].text, "a");
    assert_eq!(segs[1].text, "b");
}

/// T2.1: `transcribe_to_file` пишет выход в `out_dir`, а `.orig` и sidecar — в
/// каталог сервисных файлов (`service_dir`), чтобы `out_dir` оставался чистым (§5.3).
#[test]
fn transcribe_to_file_writes_output_sidecar_and_orig() {
    let body = serde_json::json!({"mode":"transcript","out_ext":"md","text":"### SPEAKER_00 [00:00:01 - 00:00:11]\nПривет.\n\n### UNASSIGNED [00:00:11 - 00:00:12]\nХвост.\n"}).to_string();
    let (url, h) = stub_server(body.to_string());
    let out = std::env::temp_dir().join(format!("hds-auto-out-{}", std::process::id()));
    let svc = std::env::temp_dir().join(format!("hds-auto-svc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&svc);
    let src = temp_file("auto", "встреча.wav");
    let tc = TranscribeConfig {
        url: url.clone(),
        out_dir: Some(out.clone()),
        service_dir: Some(svc.clone()),
        mode: "transcript".into(),
        ..Default::default()
    };
    let res = TranscribeClient::new(tc).transcribe_to_file(&src).unwrap();
    h.join().unwrap();
    let name = |p: &Path| p.file_name().unwrap().to_str().unwrap().to_string();
    assert_eq!(name(&res.out_path), "встреча.md");
    assert!(res.out_path.exists() && res.orig_path.exists() && res.speakers_path.exists());
    // Сервисные файлы — в service_dir, в out_dir остаётся только выход.
    assert_eq!(res.orig_path.parent().unwrap(), svc);
    assert_eq!(res.speakers_path.parent().unwrap(), svc);
    assert_eq!(
        std::fs::read_dir(&out).unwrap().count(),
        1,
        "в out_dir только выход"
    );
    assert_eq!(res.speakers, vec!["SPEAKER_00", "UNASSIGNED"]);
    assert!(res.chars > 0);
    let md = std::fs::read_to_string(&res.out_path).unwrap();
    assert!(md.contains("### SPEAKER_00"), "метки движка как есть: {md}");
    let side = std::fs::read_to_string(&res.speakers_path).unwrap();
    assert!(side.contains("\"SPEAKER_00\": \"\""), "sidecar={side}");
    assert!(side.contains("\"unassigned_label\": \"UNASSIGNED\""));
    std::fs::remove_dir_all(&out).ok();
    std::fs::remove_dir_all(&svc).ok();
}

/// T2.1: коллизия имён выхода — суффикс `-2` (§5.3), sidecar рядом с фактическим стемом.
#[test]
fn transcribe_to_file_avoids_name_collision() {
    let (url, h) = stub_server(
        serde_json::json!({"text":"### SPEAKER_00 [00:00:01 - 00:00:02]\nA\n","out_ext":"md"})
            .to_string(),
    );
    let out = std::env::temp_dir().join(format!("hds-auto-clash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("клип.md"), "старое").unwrap();
    let src = temp_file("clash", "клип.wav");
    let tc = TranscribeConfig {
        url,
        out_dir: Some(out.clone()),
        mode: "transcript".into(),
        ..Default::default()
    };
    let res = TranscribeClient::new(tc).transcribe_to_file(&src).unwrap();
    h.join().unwrap();
    assert_eq!(
        res.out_path.file_name().unwrap().to_str().unwrap(),
        "клип-2.md"
    );
    assert_eq!(
        res.speakers_path.file_name().unwrap().to_str().unwrap(),
        "клип-2.speakers.json"
    );
    assert_eq!(
        std::fs::read_to_string(out.join("клип.md")).unwrap(),
        "старое"
    );
    std::fs::remove_dir_all(&out).ok();
}
