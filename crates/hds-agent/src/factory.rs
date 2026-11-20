//! Ленивые фабрики воркеров (A4.2 → A5.1): Python поднимается только тогда,
//! когда он реально нужен, и переиспользуется между проходами.
//!
//! Живут в `hds-agent`, а не в CLI: один и тот же способ поднятия воркера нужен
//! демону (`hds agent sync`), решению (`hds agent result`) и вкладке «Агент»
//! (A5). Второй копии быть не должно — иначе разойдутся таймауты и подмены
//! `HDS_*_WORKER`, на которых держатся тесты и стенды.
//!
//! Подмены окружения (диагностика и тесты): `HDS_ANONYMIZE_PYTHON`,
//! `HDS_ANONYMIZE_WORKER`, `HDS_EXTRACT_PYTHON`, `HDS_EXTRACT_WORKER`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_core::config::Config;

use crate::mirror::MirrorError;

/// Ленивое поднятие воркера анонимизации (Python стартует при первом файле,
/// которому нужен образ).
pub fn anonymizer_factory(
    cfg: &Config,
    root: &Path,
) -> impl FnMut() -> Result<hds_anonymize::Client, MirrorError> {
    let root = root.to_path_buf();
    let cfg = cfg.clone();
    move || {
        let python = hds_anonymize::discover_python(&root).ok_or_else(|| {
            MirrorError::Config(
                "не найден интерпретатор воркера анонимизации \
                 (HDS_ANONYMIZE_PYTHON / sidecar\\venv-anonymize / sidecar\\python)"
                    .to_string(),
            )
        })?;
        let anon = hds_anonymize::Settings::from_config(&cfg);
        let mut client_cfg = hds_anonymize::ClientConfig::new(&python, &root);
        client_cfg.idle_timeout = Duration::from_secs(anon.idle_timeout_sec.max(1));
        client_cfg.request_timeout = Duration::from_secs(anon.request_timeout_sec.max(60));
        client_cfg.extra_args = anon.worker_args(&root);
        if let Some(script) = std::env::var_os("HDS_ANONYMIZE_WORKER") {
            if !script.is_empty() {
                client_cfg.script = Some(PathBuf::from(script));
            }
        }
        hds_anonymize::Client::spawn(client_cfg).map_err(|e| MirrorError::Anonymizer(e.to_string()))
    }
}

/// Ленивое поднятие воркера извлечения (нужен только pdf/pptx/mpp).
///
/// `HDS_EXTRACT_WORKER` подменяет скрипт воркера: при `HDS_ROOT`, указывающем на
/// каталог данных, штатный `root/sidecar/...` не находится.
pub fn extractor_factory(root: &Path) -> impl FnMut() -> Result<hds_extract::Worker, MirrorError> {
    let root = root.to_path_buf();
    move || {
        let python = hds_extract::discover_python(&root).ok_or_else(|| {
            MirrorError::Config(
                "не найден интерпретатор воркера извлечения \
                 (HDS_EXTRACT_PYTHON / sidecar\\python / .venv)"
                    .to_string(),
            )
        })?;
        let mut worker_cfg = hds_extract::WorkerConfig::new(&python, &root);
        worker_cfg.idle_timeout = Duration::from_secs(600);
        if let Some(script) = std::env::var_os("HDS_EXTRACT_WORKER") {
            if !script.is_empty() {
                worker_cfg.script = PathBuf::from(script);
            }
        }
        hds_extract::Worker::spawn(worker_cfg).map_err(|e| MirrorError::Extractor(e.to_string()))
    }
}
