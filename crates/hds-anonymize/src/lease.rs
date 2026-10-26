//! P3 (§2.2): lease-клиент GPU-аренды у `llm_host`.
//!
//! Аренда берётся **до** spawn/RPC и держится, пока идёт NER; sidecar остаётся
//! «глупым». Только loopback (`http://127.0.0.1:<порт чата>`): facade отдаёт
//! `/internal/*` лишь себе. Падение `llm_host`/недоступность — НЕ ошибка акта:
//! fallback CPU с предупреждением в отчёт (`anonymize.gpu: auto|off`).
//!
//! Ответ `enter` 200: `{"granted": true, "gpu_mem_limit_mb": N}` — лимит
//! передаётся воркеру (`--gpu-mem-limit-mb` → ORT CUDA EP `gpu_mem_limit`);
//! 503 `gpu_busy` — GPU занят chat-сеансом/lease уже выдан.

use std::time::Duration;

use hds_core::http::request_url;
use serde_json::json;

use crate::error::{AnonymizeError, ErrorKind};

/// Порт роли `chat` резидента (`llm_server.chat.port`, §3.5 плана W2).
pub const DEFAULT_LLM_HOST_URL: &str = "http://127.0.0.1:8010";

/// Страж аренды: на `Drop` шлёт `exit` (в лучшем случае, 2 с).
///
/// Паника/ранний выход владельца всё равно освобождают GPU — exit идемпотентен.
#[derive(Debug)]
pub struct LeaseGuard {
    url: String,
}

impl LeaseGuard {
    /// Явное освобождение (идемпотентно; ошибки не роняют акт).
    pub fn release(&self) {
        let _ = request_url(
            &format!("{}/internal/anon/exit", self.url),
            "POST",
            &[("Content-Type", "application/json")],
            Some("{}"),
            Duration::from_secs(2),
        );
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.release();
    }
}

/// Выданная аренда: лимит VRAM для ORT + страж.
#[derive(Debug)]
pub struct LeaseGrant {
    pub gpu_mem_limit_mb: u64,
    pub _guard: LeaseGuard,
}

/// Ошибка входа в аренду: GPU занят (повтор позже) или хост недоступен
/// (fallback CPU).
#[derive(Debug)]
pub enum LeaseError {
    /// `503 gpu_busy`: chat-сеанс не закончился за бюджет / lease уже выдан.
    GpuBusy { message: String },
    /// `llm_host` недоступен/не умеет роль — аргумент для fallback на CPU.
    Unavailable(String),
}

impl LeaseError {
    pub fn message(&self) -> String {
        match self {
            LeaseError::GpuBusy { message } => message.clone(),
            LeaseError::Unavailable(m) => m.clone(),
        }
    }
}

/// Войти в аренду. `timeout` — сетевой таймаут самого POST (не бюджет ожидания:
/// его держит `llm_host`, тело `lease_timeout_sec`).
pub fn acquire(
    base_url: &str,
    lease_timeout_sec: u64,
    reserve_mb: u64,
    timeout: Duration,
) -> Result<LeaseGrant, LeaseError> {
    let url = base_url.trim_end_matches('/').to_string();
    let body = json!({
        "lease_timeout_sec": lease_timeout_sec,
        "reserve_mb": reserve_mb,
    })
    .to_string();
    let resp = request_url(
        &format!("{url}/internal/anon/enter"),
        "POST",
        &[("Content-Type", "application/json")],
        Some(&body),
        timeout,
    )
    .map_err(|e| LeaseError::Unavailable(format!("llm_host недоступен ({e})")))?;

    match resp.status {
        200 => {
            let v = resp
                .json()
                .map_err(|e| LeaseError::Unavailable(format!("llm_host: ответ не JSON ({e})")))?;
            if v.get("granted").and_then(|g| g.as_bool()) != Some(true) {
                return Err(LeaseError::Unavailable(
                    "llm_host ответил без granted=true".to_string(),
                ));
            }
            let limit = v
                .get("gpu_mem_limit_mb")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            Ok(LeaseGrant {
                gpu_mem_limit_mb: limit,
                _guard: LeaseGuard { url },
            })
        }
        503 => {
            let message = resp
                .json()
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| resp.body.clone());
            Err(LeaseError::GpuBusy { message })
        }
        code => Err(LeaseError::Unavailable(format!(
            "llm_host: /internal/anon/enter → HTTP {code}: {}",
            resp.body
        ))),
    }
}

/// Доменная ошибка аренды (для отчёта акта).
pub fn to_anonymize_error(e: &LeaseError) -> AnonymizeError {
    match e {
        LeaseError::GpuBusy { message } => AnonymizeError::new(ErrorKind::GpuBusy, message.clone())
            .with_hint("GPU занят chat-сеансом: повторите позже или поставьте anonymize.gpu: off"),
        LeaseError::Unavailable(m) => AnonymizeError::new(ErrorKind::Internal, m.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Фейк-сервер: первый запрос — enter (заготовленный ответ), второй — exit.
    fn fake_server(reply_status: u16, reply_body: &'static str) -> u16 {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut s, _) = match l.accept() {
                    Ok(x) => x,
                    Err(_) => return,
                };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Connection: close\r\nContent-Length: {len}\r\n\r\n{body}",
                    status = reply_status,
                    len = reply_body.len(),
                    body = reply_body,
                );
                let _ = s.write_all(reply.as_bytes());
            }
        });
        port
    }

    #[test]
    fn acquire_granted_and_exit_on_drop() {
        let port = fake_server(200, r#"{"granted": true, "gpu_mem_limit_mb": 4096}"#);
        let grant = acquire(
            &format!("http://127.0.0.1:{port}"),
            120,
            2048,
            Duration::from_secs(5),
        )
        .expect("lease должен быть выдан");
        assert_eq!(grant.gpu_mem_limit_mb, 4096);
        drop(grant); // exit ушёл фейк-серверу (второй accept)
    }

    #[test]
    fn gpu_busy_maps_to_error() {
        let port = fake_server(
            503,
            r#"{"error": {"code": -1, "message": "GPU занят (chat), бюджет истёк"}}"#,
        );
        let err = acquire(
            &format!("http://127.0.0.1:{port}"),
            1,
            2048,
            Duration::from_secs(5),
        )
        .expect_err("503 → GpuBusy");
        match err {
            LeaseError::GpuBusy { message } => {
                assert!(message.contains("GPU занят"), "{message}");
            }
            other => panic!("ожидали GpuBusy, получено: {other:?}"),
        }
    }

    #[test]
    fn unreachable_host_is_unavailable_not_busy() {
        // порт 1 — TCP connect почти наверняка отклонён
        let err = acquire("http://127.0.0.1:1", 1, 2048, Duration::from_millis(1500))
            .expect_err("нет слушателя → Unavailable");
        assert!(
            matches!(err, LeaseError::Unavailable(_)),
            "недоступность — НЕ gpu_busy: {err:?}"
        );
    }
}
