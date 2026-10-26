//! Клиент воркера `sidecar/hds_anonymize/worker.py`.
//!
//! Транспорт (`Worker`) переиспользуется из `hds-extract`; здесь — типизация,
//! построение выходных путей (§3), проверка `hello.capabilities` и маппинг
//! машинных кодов. Повтор при падении воркера разрешён (retry_dead=true):
//! запись акта атомарна, повтор безопасен.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_extract::{Worker, WorkerConfig};
use serde_json::json;

use crate::error::{from_transport, AnonymizeError, ErrorKind};
use crate::protocol::{
    anonymize_from, deanonymize_from, hello_from, inspect_from, out_path, warmup_from,
    AnonymizeResult, DeanonymizeResult, HelloInfo, InspectResult, Mode, WarmupResult,
};

/// Параметры запуска воркера (§2.6: RPC на файл до 600 с, idle 60 с).
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub python: PathBuf,
    pub root: PathBuf,
    pub idle_timeout: Duration,
    pub request_timeout: Duration,
    pub max_requests: u64,
    pub stderr_log: Option<PathBuf>,
    /// Дополнительные аргументы воркера (`--model … --db …`, §3).
    pub extra_args: Vec<String>,
    /// Переменные окружения воркера (`HDS_MODELS_DIR`, …).
    pub env: Vec<(String, String)>,
    /// Путь к скрипту воркера (None — штатный `sidecar/hds_anonymize/worker.py`;
    /// тесты подменяют на mock-воркер).
    pub script: Option<PathBuf>,
}

impl ClientConfig {
    pub fn new(python: &Path, root: &Path) -> Self {
        ClientConfig {
            python: python.to_path_buf(),
            root: root.to_path_buf(),
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(600),
            max_requests: 0,
            stderr_log: None,
            extra_args: Vec::new(),
            env: Vec::new(),
            script: None,
        }
    }

    pub fn from_settings(python: &Path, root: &Path, settings: &crate::settings::Settings) -> Self {
        let mut cfg = ClientConfig::new(python, root);
        cfg.idle_timeout = Duration::from_secs(settings.idle_timeout_sec.max(1));
        cfg.request_timeout = Duration::from_secs(settings.request_timeout_sec.max(60));
        cfg.extra_args = settings.worker_args(root);
        cfg
    }
}

/// Интерпретатор воркера анонимизации:
/// `HDS_ANONYMIZE_PYTHON` → `venv-anonymize` → `sidecar/python` → `.venv`.
pub fn discover_python(root: &Path) -> Option<PathBuf> {
    hds_extract::discover_python_with(root, "HDS_ANONYMIZE_PYTHON", Some("venv-anonymize"))
}

/// Запущенный воркер анонимизации.
pub struct Client {
    worker: Worker,
}

impl Client {
    /// Запустить воркер и проверить, что он умеет `anonymize`.
    pub fn spawn(cfg: ClientConfig) -> Result<Client, AnonymizeError> {
        let script = cfg.script.clone().unwrap_or_else(|| {
            cfg.root
                .join("sidecar")
                .join("hds_anonymize")
                .join("worker.py")
        });
        let mut wcfg = WorkerConfig::new(&cfg.python, &cfg.root);
        wcfg.script = script;
        wcfg.idle_timeout = cfg.idle_timeout;
        wcfg.request_timeout = cfg.request_timeout;
        wcfg.max_requests = cfg.max_requests;
        wcfg.stderr_log = cfg.stderr_log.clone();
        wcfg.extra_args = cfg.extra_args.clone();
        wcfg.env = cfg.env.clone();
        let worker = Worker::spawn(wcfg).map_err(|e| {
            from_transport(
                &e,
                None,
                Some("проверьте интерпретатор (HDS_ANONYMIZE_PYTHON)"),
            )
        })?;
        if !worker.capabilities().has("anonymize") {
            return Err(AnonymizeError::new(
                ErrorKind::Internal,
                format!(
                    "воркер не заявил возможность anonymize (capabilities: {:?})",
                    worker.capabilities().capabilities
                ),
            ));
        }
        Ok(Client { worker })
    }

    /// PID процесса (для логов/диагностики).
    pub fn pid(&self) -> u32 {
        self.worker.pid()
    }

    /// `hello` (повторный; handshake уже выполнен при запуске).
    pub fn hello(&mut self) -> Result<HelloInfo, AnonymizeError> {
        let value = self.call("hello", json!({}))?;
        Ok(hello_from(&value))
    }

    /// Прогрев NER-моделей (печатает backend/providers; `warmup` CLI).
    pub fn warmup(&mut self) -> Result<WarmupResult, AnonymizeError> {
        let value = self.call("warmup", json!({}))?;
        Ok(warmup_from(&value))
    }

    /// Разведка файла БЕЗ значений PII.
    pub fn inspect(&mut self, in_path: &Path) -> Result<InspectResult, AnonymizeError> {
        let value = self.call("inspect", json!({"in_path": path_str(in_path)}))?;
        Ok(inspect_from(&value))
    }

    /// Анонимизация: полный NER (`values=None`) или детерминированный список
    /// значений (`values=Some` → RPC `anonymize_values`). Выходной путь
    /// строит Rust (§3).
    pub fn anonymize(
        &mut self,
        in_path: &Path,
        out_dir: &Path,
        values: Option<&[String]>,
    ) -> Result<AnonymizeResult, AnonymizeError> {
        let out = out_path(in_path, out_dir, Mode::Anonymize);
        let mut params = json!({
            "in_path": path_str(in_path),
            "out_dir": path_str(out_dir),
            "out_path": path_str(&out),
        });
        let method = match values {
            Some(values) => {
                params["values"] = json!(values);
                "anonymize_values"
            }
            None => "anonymize",
        };
        let value = self.call(method, params)?;
        Ok(anonymize_from(&value))
    }

    /// Деанонимизация: все маппинги (`tokens=None`) или подмножество
    /// (`tokens=Some` → RPC `deanonymize_values`).
    pub fn deanonymize(
        &mut self,
        in_path: &Path,
        out_dir: &Path,
        tokens: Option<&[String]>,
    ) -> Result<DeanonymizeResult, AnonymizeError> {
        let out = out_path(in_path, out_dir, Mode::Deanonymize);
        let mut params = json!({
            "in_path": path_str(in_path),
            "out_dir": path_str(out_dir),
            "out_path": path_str(&out),
        });
        let method = match tokens {
            Some(tokens) => {
                params["tokens"] = json!(tokens);
                "deanonymize_values"
            }
            None => "deanonymize",
        };
        let value = self.call(method, params)?;
        Ok(deanonymize_from(&value))
    }

    /// Штатная остановка (закрытие stdin → EOF).
    pub fn shutdown(mut self) {
        self.worker.shutdown();
    }

    fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AnonymizeError> {
        match self.worker.request(method, params, true) {
            Ok(value) => Ok(value),
            Err(err) => {
                let rpc = self.worker.last_error().cloned();
                let kind = rpc.as_ref().and_then(|r| r.kind.clone());
                let hint = rpc.as_ref().and_then(|r| r.hint.clone());
                Err(from_transport(&err, kind.as_deref(), hint.as_deref()))
            }
        }
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}
