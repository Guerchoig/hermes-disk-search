//! W3: тесты `hds whisper-check` — путь через стаб-владельца GPU (без движка).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

use hds_cli::cmd::whisper_check::cmd_whisper_check_cfg;
use hds_core::config::{load_from, Config};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hds-wcheck-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn media_file(tag: &str) -> PathBuf {
    let d = temp_dir(tag);
    let p = d.join("клип.wav");
    std::fs::write(&p, b"x").unwrap();
    p
}

fn cfg_with_url(tag: &str, url: &str) -> Config {
    let d = temp_dir(tag);
    let path = d.join("config.yaml");
    std::fs::write(
        &path,
        format!("index:\n  transcribe: true\n  transcribe_url: \"{url}\"\n"),
    )
    .unwrap();
    load_from(&path).unwrap()
}

/// Заглушка владельца: один запрос → JSON-тело.
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

#[test]
fn stub_owner_ok() {
    let body = r#"{"segments":[{"text":"раз","t_start":0.0,"t_end":1.0},"#.to_string()
        + r#"{"text":"два","t_start":1.0,"t_end":2.0}]}"#;
    let (url, h) = stub_server(body);
    let cfg = cfg_with_url("ok", &url);
    let file = media_file("ok");
    let code = cmd_whisper_check_cfg(&cfg, Some(file.display().to_string()), false);
    h.join().unwrap();
    assert_eq!(code, 0);
}

#[test]
fn json_mode_ok() {
    let body = r#"{"segments":[{"text":"раз","t_start":0.0,"t_end":1.0}]}"#.to_string();
    let (url, h) = stub_server(body);
    let cfg = cfg_with_url("json", &url);
    let file = media_file("json");
    let code = cmd_whisper_check_cfg(&cfg, Some(file.display().to_string()), true);
    h.join().unwrap();
    assert_eq!(code, 0);
}

#[test]
fn owner_down_returns_1() {
    let cfg = cfg_with_url("down", "http://127.0.0.1:1");
    let file = media_file("down");
    let code = cmd_whisper_check_cfg(&cfg, Some(file.display().to_string()), false);
    assert_eq!(code, 1);
}
