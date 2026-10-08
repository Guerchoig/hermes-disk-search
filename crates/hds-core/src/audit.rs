//! Аудит событий доступа (M3 T2, `PLAN_M3_NETWORK_PERIMETER.md` §3.2).
//!
//! Append-only JSONL: одно событие — одна строка. Файл по умолчанию
//! `<state_dir>/data/logs/audit.jsonl` (server — ProgramData / Application
//! Support, desktop — корень проекта). Ротация по размеру (`audit.max_mb`,
//! дефолт 50): `audit.jsonl` → `audit.jsonl.1` (предыдущая ротация
//! перезаписывается).
//!
//! Включение: ключ `audit.enabled`; без явного ключа — **включён в профиле
//! `server`, выключен в `desktop`** (не шумим локально). Не логируем тела
//! запросов и токены (только имена/логины/пути/статусы).
//!
//! Писатель — потокобезопасный в рамках процесса (глобальный мьютекс,
//! короткая append-запись); между процессами (`mcp-http` + `ui`) логи
//! независимы — это документированное ограничение (общий лимит — задача прокси).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::config::{dig, state_dir, Config};

/// Писатель аудита: путь к JSONL + порог ротации. `from_config` возвращает
/// `None`, когда аудит выключен — вызовы записи даже не компилируются в
/// ветке `else` (конструкция `if let Some(a) = ...` в вызывающем коде).
#[derive(Debug, Clone)]
pub struct Audit {
    enabled: bool,
    /// Файл JSONL (по умолчанию `<state_dir>/data/logs/audit.jsonl`).
    pub path: PathBuf,
    /// Порог ротации, МБ (`audit.max_mb`, дефолт 50).
    pub max_mb: u64,
}

/// Событие аудита (`{ts, event, user?, ip, method, path, tool?, projects?,
/// token?, status, ms}`); `user` — логин/имя пользователя, `projects` — проекты
/// токена, `token` — **префикс** хэша токена (для `token_issue`/`token_revoke`,
/// без самого токена: по нему видно «какой токен», но восстановить нельзя).
#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    /// Unix-время события (сек).
    pub ts: i64,
    /// Тип события: `login_ok`/`login_fail`/`token_issue`/`token_revoke`/
    /// `tool_call`/`admin_op`/`unauthorized`.
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Клиентский IP (`-` для CLI-событий).
    pub ip: String,
    /// HTTP-метод (`-` для CLI).
    pub method: String,
    /// Путь запроса (или CLI-команда).
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    /// Префикс `token_sha256` выданного/отозванного токена (без самого токена).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// HTTP-статус ответа (0 — для CLI).
    pub status: u16,
    /// Длительность обработки, мс.
    pub ms: u64,
}

impl AuditEvent {
    /// Текущее unix-время в поле `ts` (удобно при конструировании).
    pub fn now(mut self) -> Self {
        self.ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self
    }
}
/// Аудит включён? Ключ `audit.enabled`; без него — `profile: server`.
pub fn enabled(cfg: &Config) -> bool {
    match dig(cfg, "audit.enabled").and_then(|v| v.as_bool()) {
        Some(v) => v,
        None => crate::config::profile(cfg).is_server(),
    }
}

/// Путь к JSONL: `audit.path`, иначе `<state_dir>/data/logs/audit.jsonl`.
pub fn default_path(cfg: &Config) -> PathBuf {
    match dig(cfg, "audit.path").and_then(|v| v.as_str()) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => state_dir(cfg).join("data").join("logs").join("audit.jsonl"),
    }
}

/// Порог ротации в МБ: `audit.max_mb` (дефолт 50).
pub fn max_mb(cfg: &Config) -> u64 {
    dig(cfg, "audit.max_mb")
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0)
        .unwrap_or(50)
}

impl Audit {
    /// Аудит из конфига; `None` — выключен (desktop без явного включения).
    pub fn from_config(cfg: &Config) -> Option<Audit> {
        if !enabled(cfg) {
            return None;
        }
        Some(Audit {
            enabled: true,
            path: default_path(cfg),
            max_mb: max_mb(cfg),
        })
    }

    /// Тестовая конфигурация с явным путём (юнит-тесты).
    pub fn at(path: PathBuf, max_mb: u64) -> Audit {
        Audit {
            enabled: true,
            path,
            max_mb,
        }
    }

    /// Дописать событие одной JSONL-строкой (+ ротация по размеру).
    ///
    /// Ошибки не поднимаются наверх (аудит не должен ломать обработку
    /// запроса) — возвращают `false`.
    pub fn record(&self, ev: &AuditEvent) -> bool {
        if !self.enabled {
            return false;
        }
        static WRITE_LOCK: Mutex<()> = Mutex::new(());
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let line = match serde_json::to_string(ev) {
            Ok(s) => s,
            Err(_) => return false,
        };
        // Ротация: текущий файл + новая строка выше порога → .1 (замена).
        if let Ok(meta) = std::fs::metadata(&self.path) {
            if meta.len() + line.len() as u64 + 1 > self.max_mb * 1024 * 1024 {
                let rot = rotate_path(&self.path);
                let _ = std::fs::remove_file(&rot);
                let _ = std::fs::rename(&self.path, &rot);
            }
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut f) => f.write_all(format!("{line}\n").as_bytes()).is_ok(),
            Err(_) => false,
        }
    }
}

/// `.../audit.jsonl` → `.../audit.jsonl.1`.
pub fn rotate_path(path: &std::path::Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".1");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::{enabled, rotate_path, Audit, AuditEvent};
    use crate::config::Config;

    fn ev(event: &str, user: Option<&str>) -> AuditEvent {
        AuditEvent {
            ts: 1799000000,
            event: event.into(),
            user: user.map(|s| s.into()),
            ip: "10.1.2.3".into(),
            method: "POST".into(),
            path: "/auth/login".into(),
            tool: None,
            projects: None,
            token: None,
            status: 200,
            ms: 12,
        }
    }

    fn cfg(yaml: &str) -> Config {
        serde_yaml::from_str(yaml).unwrap()
    }

    /// Дефолты по профилю: desktop — выключен, server — включён (план §3.2).
    #[test]
    fn defaults_follow_profile() {
        let d = cfg("profile: desktop");
        assert!(!enabled(&d), "desktop без ключа — выключен");
        assert!(Audit::from_config(&d).is_none());
        let s = cfg("profile: server");
        assert!(enabled(&s), "server без ключа — включён");
        let a = Audit::from_config(&s).expect("server — аудит включён");
        assert_eq!(a.max_mb, 50, "дефолт порога ротации");
        assert!(
            a.path.ends_with("data/logs/audit.jsonl")
                || a.path.ends_with("data\\logs\\audit.jsonl"),
            "путь по умолчанию под state_dir: {}",
            a.path.display()
        );
    }

    /// Явные ключи `audit.*` сильнее профиля.
    #[test]
    fn config_keys_override_profile() {
        let off = cfg("profile: server\naudit:\n  enabled: false");
        assert!(!enabled(&off), "явное выключение сильнее профиля");
        assert!(Audit::from_config(&off).is_none());
        let on = cfg("profile: desktop\naudit:\n  enabled: true");
        assert!(enabled(&on), "явное включение сильнее профиля");
        let tuned = cfg(
            "profile: desktop\naudit:\n  enabled: true\n  path: \"Z:/audit.jsonl\"\n  max_mb: 7",
        );
        let a = Audit::from_config(&tuned).unwrap();
        assert_eq!(a.path.to_string_lossy(), "Z:/audit.jsonl");
        assert_eq!(a.max_mb, 7);
    }

    /// JSONL: одна строка на событие; None-поля не пишутся.
    #[test]
    fn jsonl_lines_and_skip_none() {
        let tmp = std::env::temp_dir().join(format!("hds-audit-jsonl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let path = tmp.join("audit.jsonl");
        let a = Audit::at(path.clone(), 50);
        assert!(a.record(&ev("login_ok", Some("alice")).now()));
        assert!(a.record(&ev("login_fail", None).now()));
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["event"], "login_ok");
        assert_eq!(v["user"], "alice");
        assert_eq!(v["ip"], "10.1.2.3");
        assert_eq!(v["status"], 200);
        let v2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert!(v2.get("user").is_none(), "None-поля не пишутся: {v2}");
        assert!(v2.get("tool").is_none());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// Ротация: превышение порога → `audit.jsonl.1`, основной файл пуст.
    #[test]
    fn rotation_moves_to_dot_one() {
        let tmp = std::env::temp_dir().join(format!("hds-audit-rot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let path = tmp.join("audit.jsonl");
        let a = Audit::at(path.clone(), 1); // 1 МБ
        assert!(a.record(&ev("login_ok", Some("alice")).now()));
        // Набиваем файл выше порога (дописываем, не затирая первое событие) —
        // следующая запись уходит в .1.
        let mut big = std::fs::read(&path).unwrap();
        big.extend(std::iter::repeat_n(b'x', 1024 * 1024));
        std::fs::write(&path, &big).unwrap();
        assert!(a.record(&ev("login_fail", None).now()));
        let rot = rotate_path(&path);
        let rot_text = std::fs::read_to_string(&rot).unwrap();
        assert!(rot_text.contains("\"login_ok\""), "{rot_text}");
        let main_text = std::fs::read_to_string(&path).unwrap();
        assert!(
            main_text.contains("\"login_fail\"") && !main_text.contains("\"login_ok\""),
            "в основном файле — только события после ротации: {main_text}"
        );
        // Вторая ротация перезаписывает прежний .1.
        let mut big2 = std::fs::read(&path).unwrap();
        big2.extend(std::iter::repeat_n(b'x', 1024 * 1024));
        std::fs::write(&path, &big2).unwrap();
        assert!(a.record(&ev("token_revoke", Some("bob")).now()));
        // Новое событие — в основном файле; в .1 — то, что было до ротации.
        let main_text2 = std::fs::read_to_string(&path).unwrap();
        assert!(
            main_text2.contains("\"token_revoke\"") && !main_text2.contains("xxxxx"),
            "после второй ротации в основном файле только новое событие"
        );
        let rot_text2 = std::fs::read_to_string(&rot).unwrap();
        assert!(
            rot_text2.contains("\"login_fail\""),
            "старый .1 заменён содержимым файла до ротации"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// Выключенный аудит (`enabled: false` в структуре) не пишет.
    #[test]
    fn disabled_audit_records_nothing() {
        let tmp = std::env::temp_dir().join(format!("hds-audit-off-{}", std::process::id()));
        let path = tmp.join("audit.jsonl");
        let mut a = Audit::at(path.clone(), 50);
        a.enabled = false;
        assert!(!a.record(&ev("login_ok", None).now()));
        assert!(!path.exists(), "файл не создаётся");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
