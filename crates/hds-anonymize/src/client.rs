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
    anonymize_from, deanonymize_from, hello_from, image_from, inspect_from, label_from,
    merge_outcome_from, office_from, out_path, project_labels_from, snapshot_from, text_from,
    unmerge_outcome_from, warmup_from, AnonymizeResult, DeanonymizeResult, HelloInfo,
    InspectResult, LabelForValue, MergeOutcome, Mode, OfficeResult, ProjectLabels, Snapshot,
    TextImageResult, TextResult, UnmergeOutcome, WarmupResult,
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

/// Маркер образа `hds-agent/<P>/<id>` (TR-7.4/7.7).
///
/// `marker == None` — файл создан агентом (нет маркера демона): это
/// `agent_new` обратного пути (TR-7.5). Маркер чужого проекта тоже не наш.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkerInfo {
    pub marker: Option<String>,
    pub project_id: Option<String>,
    pub artifact_id: Option<i64>,
}

/// Обратное к `anonymize_path`: путь с реальными значениями проекта (TR-7.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeanonymizedPath {
    pub path: String,
    pub components: Vec<String>,
    /// Метки, которых нет в базе проекта: остались в имени как есть.
    pub unresolved: Vec<String>,
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

    // ── уровень 1: анонимная зона (TR-1.6/1.7, TR-7.4, D18) ─────────────

    /// Образ анонимной зоны: `mode: "agent"` (TR-7.4) — метки проекта,
    /// картинки `[IMAGE_n]`, маркер `hds-agent/<P>/<artifact_id>`.
    /// Выход — `out_dir/<имя оригинала>` (имя файла сохраняется).
    pub fn anonymize_agent(
        &mut self,
        in_path: &Path,
        out_dir: &Path,
        project_id: &str,
        artifact_id: Option<&str>,
    ) -> Result<AnonymizeResult, AnonymizeError> {
        let out = out_path(in_path, out_dir, Mode::Agent);
        self.anonymize_agent_to(in_path, &out, project_id, artifact_id)
    }

    /// `anonymize` с `mode:"agent"` и **явным** выходным путём (TR-7.1):
    /// имя образа задаёт демон (анонимизированное имя), а не `out_path` по
    /// имени оригинала. Маркер `hds-agent/<P>/<artifact_id>` пишет воркер.
    pub fn anonymize_agent_to(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        artifact_id: Option<&str>,
    ) -> Result<AnonymizeResult, AnonymizeError> {
        let mut params = json!({
            "in_path": path_str(in_path),
            "out_dir": path_str(out_path.parent().unwrap_or_else(|| Path::new("."))),
            "out_path": path_str(out_path),
            "project_id": project_id,
            "mode": "agent",
        });
        if let Some(id) = artifact_id {
            params["artifact_id"] = json!(id);
        }
        let value = self.call("anonymize", params)?;
        Ok(anonymize_from(&value))
    }

    /// `anonymize_path` (TR-7.1, A4.2): анонимизировать относительный путь —
    /// NER по компонентам, метки проекта **без** падежного суффикса (`|`
    /// запрещён Windows), санитайзинг имени. Оригинал не читается.
    pub fn anonymize_path(
        &mut self,
        project_id: &str,
        path: &str,
    ) -> Result<String, AnonymizeError> {
        let value = self.call(
            "anonymize_path",
            json!({"project_id": project_id, "path": path, "suffix": false}),
        )?;
        value
            .get("path")
            .and_then(|p| p.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                AnonymizeError::new(
                    ErrorKind::Internal,
                    "anonymize_path: в ответе воркера нет поля path",
                )
            })
    }

    /// Деанонимизация с метками проекта: `[PERSON_3|дат]` → форма в падеже
    /// (TR-2.4). Выход — `<stem>.deanon.<ext>`.
    pub fn deanonymize_project(
        &mut self,
        in_path: &Path,
        out_dir: &Path,
        project_id: &str,
        tokens: Option<&[String]>,
    ) -> Result<DeanonymizeResult, AnonymizeError> {
        let out = out_path(in_path, out_dir, Mode::Deanonymize);
        let mut params = json!({
            "in_path": path_str(in_path),
            "out_dir": path_str(out_dir),
            "out_path": path_str(&out),
            "project_id": project_id,
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

    /// Деанонимизация образа в файл пользователя с явным `out_path` (TR-7.5).
    ///
    /// `restore_from` — снимок основания `.hds-agent\base`: из него Python
    /// возвращает `docProps` и (при `images=true`) картинки тела на места
    /// меток `[IMAGE_n]`. `None` — новый файл агента: восстанавливать нечего,
    /// маркер вычищается.
    pub fn deanonymize_agent_to(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        restore_from: Option<&Path>,
        images: bool,
    ) -> Result<DeanonymizeResult, AnonymizeError> {
        let mut params = json!({
            "in_path": path_str(in_path),
            "out_dir": path_str(out_path.parent().unwrap_or_else(|| Path::new("."))),
            "out_path": path_str(out_path),
            "project_id": project_id,
            "images": images,
        });
        if let Some(base) = restore_from {
            params["restore_from"] = json!(path_str(base));
        }
        let value = self.call("deanonymize", params)?;
        Ok(deanonymize_from(&value))
    }

    /// `read_marker` (TR-7.4/7.7): маркер образа из свойств файла или первой
    /// строки текстового образа.
    pub fn read_marker(&mut self, path: &Path) -> Result<MarkerInfo, AnonymizeError> {
        let value = self.call("read_marker", json!({"path": path_str(path)}))?;
        let text = |key: &str| {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        Ok(MarkerInfo {
            marker: text("marker"),
            project_id: text("project_id"),
            artifact_id: value.get("artifact_id").and_then(|a| {
                a.as_i64()
                    .or_else(|| a.as_str().and_then(|s| s.parse().ok()))
            }),
        })
    }

    /// `deanonymize_path` (TR-7.5): метки проекта → реальные значения.
    ///
    /// Неизвестные метки остаются в имени и возвращаются в `unresolved`:
    /// терять компонент пути нельзя.
    pub fn deanonymize_path(
        &mut self,
        project_id: &str,
        path: &str,
    ) -> Result<DeanonymizedPath, AnonymizeError> {
        let value = self.call(
            "deanonymize_path",
            json!({"project_id": project_id, "path": path}),
        )?;
        let list = |key: &str| {
            value
                .get(key)
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(DeanonymizedPath {
            path: value
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or_default()
                .to_string(),
            components: list("components"),
            unresolved: list("unresolved"),
        })
    }

    /// `image_map` (TR-7.4, В-4): `[IMAGE_n]` → часть пакета (медиа/якорь).
    pub fn image_map(&mut self, path: &Path) -> Result<serde_json::Value, AnonymizeError> {
        let value = self.call("image_map", json!({"path": path_str(path)}))?;
        Ok(value.get("images").cloned().unwrap_or_else(|| json!([])))
    }

    /// `anonymize_text` (TR-1.7): реплики, вложения и текстовые образы.
    /// Значения PII наружу не отдаются — только текст с метками и счётчики.
    pub fn anonymize_text(
        &mut self,
        project_id: &str,
        text: &str,
    ) -> Result<TextResult, AnonymizeError> {
        let value = self.call(
            "anonymize_text",
            json!({"project_id": project_id, "text": text}),
        )?;
        Ok(text_from(&value))
    }

    /// `vault_snapshot` (TR-1.6): метки проекта с формами + «прочие» + keep.
    /// Ответ содержит исходные значения — Rust держит его только в памяти.
    pub fn snapshot(&mut self, project_id: &str) -> Result<Snapshot, AnonymizeError> {
        let value = self.call("vault_snapshot", json!({"project_id": project_id}))?;
        Ok(snapshot_from(&value))
    }

    /// `vault_rev` — дешёвая проверка актуальности снимка (TR-1.6).
    pub fn vault_rev(&mut self) -> Result<i64, AnonymizeError> {
        let value = self.call("vault_rev", json!({}))?;
        Ok(value.get("rev").and_then(|r| r.as_i64()).unwrap_or(0))
    }

    /// `label_for_value` — «Скрыть ещё…» (TR-1.7).
    pub fn label_for_value(
        &mut self,
        project_id: &str,
        value: &str,
        entity_type: &str,
    ) -> Result<LabelForValue, AnonymizeError> {
        let value = self.call(
            "label_for_value",
            json!({"project_id": project_id, "value": value, "type": entity_type}),
        )?;
        Ok(label_from(&value))
    }

    /// `merge_entities` — «Объединить метки» (TR-2.6).
    ///
    /// Возвращает и номер склейки (`merge_log`, A5.5): без него «Разъединить»
    /// не сможет отменить именно это объединение.
    pub fn merge_entities(
        &mut self,
        project_id: &str,
        keep: &str,
        drop: &[String],
    ) -> Result<MergeOutcome, AnonymizeError> {
        let value = self.call(
            "merge_entities",
            json!({"project_id": project_id, "keep": keep, "drop": drop}),
        )?;
        Ok(merge_outcome_from(&value))
    }

    /// `unmerge_entities` — «Разъединить»: отмена склейки из `merge_log` (A5.5).
    pub fn unmerge_entities(
        &mut self,
        project_id: &str,
        merge_id: i64,
    ) -> Result<UnmergeOutcome, AnonymizeError> {
        let value = self.call(
            "unmerge_entities",
            json!({"project_id": project_id, "merge_id": merge_id}),
        )?;
        Ok(unmerge_outcome_from(&value))
    }

    /// `vault_project_labels` (A5.5, TR-2.6): метки проекта с формами и
    /// псевдонимами, журнал склеек, список «не анонимизировать».
    ///
    /// Ответ содержит значения форм: это зона пользователя, как `snapshot`;
    /// в журналы они не пишутся (I8).
    pub fn project_labels(&mut self, project_id: &str) -> Result<ProjectLabels, AnonymizeError> {
        let value = self.call("vault_project_labels", json!({"project_id": project_id}))?;
        Ok(project_labels_from(&value))
    }

    /// `vault_drop_project` — удалить проект и его метки (глобальные остаются).
    pub fn drop_project(&mut self, project_id: &str) -> Result<serde_json::Value, AnonymizeError> {
        self.call("vault_drop_project", json!({"project_id": project_id}))
    }

    /// `keep_terms` (D19): список «не анонимизировать».
    pub fn keep_terms(&mut self) -> Result<Vec<String>, AnonymizeError> {
        let value = self.call("keep_terms", json!({"action": "list"}))?;
        Ok(value
            .get("keep")
            .and_then(|k| k.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `keep_terms` action=add — «Не скрывать» (TR-2.6).
    pub fn add_keep_term(&mut self, value: &str) -> Result<bool, AnonymizeError> {
        let answer = self.call("keep_terms", json!({"action": "add", "value": value}))?;
        Ok(answer
            .get("added")
            .and_then(|a| a.as_bool())
            .unwrap_or(false))
    }

    /// `text_image` (D18): `<имя>.<ext>.md` с метками проекта.
    /// `text` — текст от `hds_extract` (для `mpp` обязателен).
    pub fn text_image(
        &mut self,
        in_path: &Path,
        out_dir: &Path,
        project_id: &str,
        artifact_id: Option<&str>,
        text: Option<&str>,
    ) -> Result<TextImageResult, AnonymizeError> {
        let mut params = json!({
            "in_path": path_str(in_path),
            "out_dir": path_str(out_dir),
            "project_id": project_id,
        });
        if let Some(id) = artifact_id {
            params["artifact_id"] = json!(id);
        }
        if let Some(text) = text {
            params["text"] = json!(text);
        }
        let value = self.call("text_image", params)?;
        Ok(image_from(&value))
    }

    /// `text_image` с **явным** выходным путём (TR-7.1): имя текстового
    /// образа `<анонимное имя>.<ext>.md` задаёт демон, а не имя оригинала.
    pub fn text_image_to(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        artifact_id: Option<&str>,
        text: Option<&str>,
    ) -> Result<TextImageResult, AnonymizeError> {
        let mut params = json!({
            "in_path": path_str(in_path),
            "out_dir": path_str(out_path.parent().unwrap_or_else(|| Path::new("."))),
            "out_path": path_str(out_path),
            "project_id": project_id,
        });
        if let Some(id) = artifact_id {
            params["artifact_id"] = json!(id);
        }
        if let Some(text) = text {
            params["text"] = json!(text);
        }
        let value = self.call("text_image", params)?;
        Ok(image_from(&value))
    }

    /// `office_read` (TR-3.5): абзацы/таблицы/листы/слайды.
    pub fn office_read(&mut self, path: &Path) -> Result<OfficeResult, AnonymizeError> {
        let value = self.call("office_read", json!({"path": path_str(path)}))?;
        Ok(office_from(&value))
    }

    /// `office_edit`: операции `replace_text`/`set_cell`/… (TR-3.5).
    pub fn office_edit(
        &mut self,
        path: &Path,
        ops: &[serde_json::Value],
    ) -> Result<OfficeResult, AnonymizeError> {
        let value = self.call("office_edit", json!({"path": path_str(path), "ops": ops}))?;
        Ok(office_from(&value))
    }

    /// `office_create`: новый docx/xlsx/pptx (TR-3.5).
    pub fn office_create(
        &mut self,
        path: &Path,
        kind: &str,
        content: &serde_json::Value,
    ) -> Result<OfficeResult, AnonymizeError> {
        let value = self.call(
            "office_create",
            json!({"path": path_str(path), "kind": kind, "content": content}),
        )?;
        Ok(office_from(&value))
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
