//! Клиент воркера `sidecar/hds_transcribe/worker.py`.
//!
//! Транспорт (`Worker`) переиспользуется из `hds-extract`; здесь — типизация,
//! путь к скрипту, проверка `hello.capabilities` и маппинг ошибок.
//! `retry_dead = false`: падение воркера посреди 30-минутного задания не
//! повторяем внутри вызова (повтор — на уровне демона, `max_attempts`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_extract::{Worker, WorkerConfig};
use serde_json::{json, Value};

use crate::error::{ErrorKind, TranscribeError};
use crate::params::{
    HelloResult, PostprocessParams, PostprocessResult, TranscribeParams, TranscribeResult,
};

/// Параметры запуска воркера (таймаут ~7 ч на длинные записи).
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub python: PathBuf,
    pub root: PathBuf,
    pub profile: String,
    pub idle_timeout: Duration,
    pub request_timeout: Duration,
    pub max_requests: u64,
    pub stderr_log: Option<PathBuf>,
    /// Дополнительные переменные окружения воркера (`HDS_MODELS_DIR`, `HF_HOME`).
    pub env: Vec<(String, String)>,
}

impl ClientConfig {
    pub fn new(python: &Path, root: &Path, profile: &str) -> Self {
        ClientConfig {
            python: python.to_path_buf(),
            root: root.to_path_buf(),
            profile: profile.to_string(),
            idle_timeout: Duration::from_secs(600),
            request_timeout: Duration::from_secs(25_200),
            max_requests: 20,
            stderr_log: None,
            env: Vec::new(),
        }
    }

    pub fn from_settings(python: &Path, root: &Path, settings: &crate::settings::Settings) -> Self {
        let mut cfg = ClientConfig::new(python, root, &settings.profile);
        cfg.idle_timeout = Duration::from_secs(settings.idle_unload_sec.max(1));
        cfg.request_timeout = Duration::from_secs(settings.request_timeout_sec.max(60));
        cfg.max_requests = settings.max_requests;
        cfg.env = settings.worker_env();
        cfg
    }
}

/// Интерпретатор воркера транскрибации (env → venv профиля → sidecar/python).
pub fn discover_python(root: &Path, profile: &str) -> Option<PathBuf> {
    let venv = if profile == "dml" {
        "venv-transcribe-dml"
    } else {
        "venv-transcribe"
    };
    hds_extract::discover_python_with(root, "HDS_TRANSCRIBE_PYTHON", Some(venv))
}

/// Запущенный воркер транскрибации.
pub struct Client {
    worker: Worker,
}

impl Client {
    /// Запустить воркер и проверить, что он умеет `transcribe`.
    pub fn spawn(cfg: ClientConfig) -> Result<Client, TranscribeError> {
        let script = cfg
            .root
            .join("sidecar")
            .join("hds_transcribe")
            .join("worker.py");
        let mut wcfg = WorkerConfig::new(&cfg.python, &cfg.root);
        wcfg.script = script;
        wcfg.idle_timeout = cfg.idle_timeout;
        wcfg.request_timeout = cfg.request_timeout;
        wcfg.max_requests = cfg.max_requests;
        wcfg.stderr_log = cfg.stderr_log.clone();
        wcfg.extra_args = vec!["--profile".to_string(), cfg.profile.clone()];
        wcfg.env = cfg.env.clone();
        let worker =
            Worker::spawn(wcfg).map_err(|e| TranscribeError::from_transport(&e, None, None))?;
        if !worker.capabilities().has("transcribe") {
            return Err(TranscribeError::new(
                ErrorKind::Internal,
                format!(
                    "воркер не заявил возможность transcribe (capabilities: {:?})",
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
    pub fn hello(&mut self) -> Result<HelloResult, TranscribeError> {
        let value = self.call("hello", json!({"protocol": 1}))?;
        serde_json::from_value(value)
            .map_err(|e| TranscribeError::new(ErrorKind::Internal, format!("hello: {e}")))
    }

    /// Тяжёлая диагностика стека (torch/CUDA, ORT EP, ffmpeg, кэши).
    pub fn probe(&mut self) -> Result<Value, TranscribeError> {
        self.call("probe", json!({}))
    }

    /// Полный конвейер: `transcribe` → структура блоков/спикеров.
    pub fn transcribe(
        &mut self,
        params: &TranscribeParams,
    ) -> Result<TranscribeResult, TranscribeError> {
        let payload = serde_json::to_value(params)
            .map_err(|e| TranscribeError::new(ErrorKind::BadRequest, format!("params: {e}")))?;
        let value = self.call("transcribe", payload)?;
        serde_json::from_value(value).map_err(|e| {
            TranscribeError::new(ErrorKind::Internal, format!("разбор transcribe: {e}"))
        })
    }

    /// Постобработка по готовому `transcript.json` (в P1 — passthrough).
    pub fn postprocess(
        &mut self,
        params: &PostprocessParams,
    ) -> Result<PostprocessResult, TranscribeError> {
        let payload = serde_json::to_value(params)
            .map_err(|e| TranscribeError::new(ErrorKind::BadRequest, format!("params: {e}")))?;
        let value = self.call("postprocess", payload)?;
        serde_json::from_value(value).map_err(|e| {
            TranscribeError::new(ErrorKind::Internal, format!("разбор postprocess: {e}"))
        })
    }

    /// Отмена активного задания (второй путь — файл `transcribe.stop`).
    pub fn cancel(&mut self, job_id: &str) -> Result<Value, TranscribeError> {
        self.call("cancel", json!({"job_id": job_id}))
    }

    /// Штатная остановка (закрытие stdin → EOF).
    pub fn shutdown(mut self) {
        self.worker.shutdown();
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, TranscribeError> {
        match self.worker.request(method, params, false) {
            Ok(value) => Ok(value),
            Err(err) => {
                let rpc = self.worker.last_error().cloned();
                let kind = rpc.as_ref().and_then(|r| r.kind.clone());
                let hint = rpc.as_ref().and_then(|r| r.hint.clone());
                Err(TranscribeError::from_transport(
                    &err,
                    kind.as_deref(),
                    hint.as_deref(),
                ))
            }
        }
    }
}
