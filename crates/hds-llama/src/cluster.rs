//! Безопасная обёртка над cluster API: устройства, инстансы, инференс.
//!
//! Ошибки/таймауты — по документации движка (§11.5 п.4): `rc == 0` означает лишь
//! успешный путь вызова, у результатов дополнительно проверяется `ok`/`error`.

use std::ffi::CString;
use std::sync::Arc;

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::engine::{cstr_or_empty, ClusterApi};
use crate::error::{EngineError, Result};
use crate::ffi::{self, state, ClusterRaw, InstanceId, InstanceParamsRaw};

/// Устройство кластера (`list_devices`) — источник подсказки о памяти;
/// бюджет VRAM ведём по NVML (R29), `memory_free` движка ненадёжен.
#[derive(Debug, Clone, Serialize)]
pub struct Device {
    /// Индекс устройства в bridge-API (это значение идёт в `manual_devices_csv`).
    pub bridge_device_index: i32,
    /// Тип устройства по классификации движка (`1` — дискретный GPU).
    pub device_type: i32,
    pub memory_free: u64,
    pub memory_total: u64,
    pub backend: String,
    pub name: String,
    pub description: String,
}

impl Device {
    /// Ускоритель (CUDA/Vulkan/Metal), а не CPU.
    pub fn is_accelerator(&self) -> bool {
        !self.backend.eq_ignore_ascii_case("CPU")
    }

    pub fn memory_free_mib(&self) -> f64 {
        self.memory_free as f64 / (1024.0 * 1024.0)
    }
}

/// Инстанс кластера (`list_instances`).
#[derive(Debug, Clone, Serialize)]
pub struct Instance {
    pub id: InstanceId,
    pub name: String,
    pub model_path: String,
    pub state: i32,
    pub state_name: String,
    pub retention_mode: i32,
    pub model_kind: i32,
    pub active_request_count: i32,
    pub queued_request_count: i32,
    pub last_error: String,
}

impl Instance {
    pub fn is_loaded(&self) -> bool {
        state::is_loaded(self.state)
    }

    pub fn is_failed(&self) -> bool {
        self.state == state::FAILED
    }
}

/// Метрики инференса движка (нужны для проверки «GPU против CPU» и времени).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Metrics {
    pub loaded_this_call: i32,
    pub prompt_tokens: i32,
    pub decoded_tokens: i32,
    pub prompt_ms: f64,
    pub predicted_ms: f64,
    pub request_total_ms: f64,
    pub prompt_tokens_per_second: f64,
    pub total_tokens_per_second: f64,
}

impl From<ffi::InferenceMetricsRaw> for Metrics {
    fn from(m: ffi::InferenceMetricsRaw) -> Self {
        Metrics {
            loaded_this_call: m.loaded_this_call,
            prompt_tokens: m.prompt_tokens,
            decoded_tokens: m.decoded_tokens,
            prompt_ms: m.prompt_ms,
            predicted_ms: m.predicted_ms,
            request_total_ms: m.request_total_ms,
            prompt_tokens_per_second: m.prompt_tokens_per_second,
            total_tokens_per_second: m.total_tokens_per_second,
        }
    }
}

/// Результат JSON-вызовов движка (embeddings/rerank/audio).
#[derive(Debug, Clone, Serialize)]
pub struct JsonOutcome {
    /// `rc` вызова: `0` — путь вызова успешен (не гарантия результата!).
    pub rc: i32,
    pub ok: bool,
    pub status: i32,
    pub json: String,
    pub error: String,
    pub metrics: Metrics,
}

impl JsonOutcome {
    /// Документированное правило SDK: проверять `ok` и `error`, а не только `rc`.
    pub fn ensure_ok(&self) -> Result<()> {
        if self.rc != 0 {
            return Err(EngineError::Call {
                rc: self.rc,
                last_error: self.error.clone(),
            });
        }
        if !self.ok {
            return Err(EngineError::Other(format!(
                "движок вернул ok=0 (status={}): {}",
                self.status, self.error
            )));
        }
        Ok(())
    }
}

/// Результат чат-вызова.
#[derive(Debug, Clone, Serialize)]
pub struct ChatOutcome {
    pub rc: i32,
    pub ok: bool,
    pub text: String,
    pub error: String,
    pub metrics: Metrics,
}

/// Параметры создания инстанса — безопасный слой над `InstanceParamsRaw`.
///
/// Незаданные поля остаются в значениях движка (`default_instance_params()`;
/// правило SDK: структуры инициализируются его хелперами).
#[derive(Debug, Clone, Default)]
pub struct InstanceSpec {
    pub name: String,
    pub model_path: String,
    /// `manual_devices_csv` — упорядоченные **bridge-индексы** устройств инстанса.
    /// Единственный способ задать устройство в cluster API (§11.2): поля
    /// `gpu`/`devices` есть только у bridge-API.
    pub manual_devices_csv: Option<String>,
    pub retention_mode: Option<i32>,
    pub load_on_demand_grace_seconds: Option<i32>,
    pub embedding: Option<bool>,
    pub reranking: Option<bool>,
    pub model_kind: Option<i32>,
    pub allow_cpu: Option<bool>,
    pub n_ctx: Option<i32>,
    pub n_batch: Option<i32>,
    pub n_ubatch: Option<i32>,
    pub n_parallel: Option<i32>,
    pub n_threads: Option<i32>,
    pub n_gpu_layers: Option<i32>,
}

impl InstanceSpec {
    pub fn new(name: &str, model_path: &str) -> Self {
        Self {
            name: name.to_string(),
            model_path: model_path.to_string(),
            ..Default::default()
        }
    }

    /// Применить заданные поля к дефолтам движка. Возвращает параметры и «якоря»
    /// C-строк — они обязаны жить до конца вызова `create_instance`.
    fn build(&self, base: InstanceParamsRaw) -> Result<(InstanceParamsRaw, Vec<CString>)> {
        let mut keep: Vec<CString> = Vec::new();
        let mut p = base;
        p.name = anchor(&self.name, &mut keep)?;
        p.model_path = anchor(&self.model_path, &mut keep)?;
        p.manual_devices_csv = match &self.manual_devices_csv {
            Some(csv) => anchor(csv, &mut keep)?,
            None => std::ptr::null(),
        };
        if let Some(v) = self.retention_mode {
            p.retention_mode = v;
        }
        if let Some(v) = self.load_on_demand_grace_seconds {
            p.load_on_demand_grace_seconds = v;
        }
        if let Some(v) = self.embedding {
            p.embedding = v as i32;
        }
        if let Some(v) = self.reranking {
            p.reranking = v as i32;
        }
        if let Some(v) = self.model_kind {
            p.model_kind = v;
        }
        if let Some(v) = self.allow_cpu {
            p.allow_cpu = v as i32;
        }
        if let Some(v) = self.n_ctx {
            p.n_ctx = v;
        }
        if let Some(v) = self.n_batch {
            p.n_batch = v;
        }
        if let Some(v) = self.n_ubatch {
            p.n_ubatch = v;
        }
        if let Some(v) = self.n_parallel {
            p.n_parallel = v;
        }
        if let Some(v) = self.n_threads {
            p.n_threads = v;
        }
        if let Some(v) = self.n_gpu_layers {
            p.n_gpu_layers = v;
        }
        Ok((p, keep))
    }
}

/// C-строка, живущая в `keep` (указатели остаются валидными при росте вектора:
/// `CString` владеет буфером в куче, перемещается только сам указатель-обёртка).
fn anchor(s: &str, keep: &mut Vec<CString>) -> Result<*const std::os::raw::c_char> {
    let c = CString::new(s).map_err(|_| EngineError::Other(format!("NUL в строке: {s:?}")))?;
    let ptr = c.as_ptr();
    keep.push(c);
    Ok(ptr)
}

/// Кластер движка: владеет устройствами и инстансами (один на процесс `llm-host`).
///
/// `!Send`/`!Sync` намеренно: состояние инстансов принадлежит движку, а вызовы
/// идут из одного потока-владельца (§3 плана W2 «один владелец GPU»).
pub struct Cluster {
    api: Arc<ClusterApi>,
    raw: *mut ClusterRaw,
}

impl Cluster {
    pub(crate) fn from_raw(api: Arc<ClusterApi>, raw: *mut ClusterRaw) -> Self {
        Cluster { api, raw }
    }

    /// Последняя ошибка кластера (`*_last_error`) — заполняется при неудачах.
    pub fn last_error(&self) -> String {
        let p = unsafe { (self.api.cluster_last_error)(self.raw) };
        cstr_or_empty(p)
    }

    /// Устройства движка (`list_devices`). `memory_free` — только подсказка (R29):
    /// замеры W0 показали расхождение с `nvidia-smi` до +10 ГБ.
    pub fn devices(&self) -> Result<Vec<Device>> {
        let mut p: *mut ffi::DeviceInfoRaw = std::ptr::null_mut();
        let mut n: usize = 0;
        let rc = unsafe { (self.api.list_devices)(self.raw, &mut p, &mut n) };
        if rc != 0 {
            return Err(EngineError::Call {
                rc,
                last_error: self.last_error(),
            });
        }
        let mut out = Vec::with_capacity(n);
        if !p.is_null() {
            for i in 0..n {
                let d = unsafe { &*p.add(i) };
                out.push(Device {
                    bridge_device_index: d.bridge_device_index,
                    device_type: d.type_,
                    memory_free: d.memory_free,
                    memory_total: d.memory_total,
                    backend: cstr_or_empty(d.backend),
                    name: cstr_or_empty(d.name),
                    description: cstr_or_empty(d.description),
                });
            }
            unsafe { (self.api.free_devices)(p, n) };
        }
        Ok(out)
    }

    /// Инстансы кластера (`list_instances`) — источник статуса для `llm-host status`.
    pub fn instances(&self) -> Result<Vec<Instance>> {
        let mut p: *mut ffi::InstanceInfoRaw = std::ptr::null_mut();
        let mut n: usize = 0;
        let rc = unsafe { (self.api.list_instances)(self.raw, &mut p, &mut n) };
        if rc != 0 {
            return Err(EngineError::Call {
                rc,
                last_error: self.last_error(),
            });
        }
        let mut out = Vec::with_capacity(n);
        if !p.is_null() {
            for i in 0..n {
                let it = unsafe { &*p.add(i) };
                out.push(Instance {
                    id: it.instance_id,
                    name: cstr_or_empty(it.name),
                    model_path: cstr_or_empty(it.model_path),
                    state: it.state,
                    state_name: state::name(it.state).to_string(),
                    retention_mode: it.retention_mode,
                    model_kind: it.model_kind,
                    active_request_count: it.active_request_count,
                    queued_request_count: it.queued_request_count,
                    last_error: cstr_or_empty(it.last_error),
                });
            }
            unsafe { (self.api.free_instances)(p, n) };
        }
        Ok(out)
    }

    /// Инстанс по имени (или `None`, если его нет).
    pub fn instance_by_name(&self, name: &str) -> Result<Option<Instance>> {
        Ok(self.instances()?.into_iter().find(|i| i.name == name))
    }

    /// Инстанс по `instance_id`.
    pub fn instance_by_id(&self, id: InstanceId) -> Result<Option<Instance>> {
        Ok(self.instances()?.into_iter().find(|i| i.id == id))
    }

    /// Создать инстанс (без загрузки модели — загрузка отдельным шагом).
    pub fn create_instance(&self, spec: &InstanceSpec) -> Result<InstanceId> {
        let base = unsafe { (self.api.default_instance_params)() };
        let (raw, keep) = spec.build(base)?;
        let id = unsafe { (self.api.create_instance)(self.raw, &raw) };
        drop(keep); // C-строки жили до конца вызова
        if id <= 0 {
            return Err(EngineError::Call {
                rc: id as i32,
                last_error: self.last_error(),
            });
        }
        Ok(id)
    }

    /// Найти инстанс по имени (`find_instance_by_name`); `None` — не найден.
    pub fn find_instance_by_name(&self, name: &str) -> Result<Option<InstanceId>> {
        let cname = CString::new(name)
            .map_err(|_| EngineError::Other(format!("NUL в имени инстанса: {name:?}")))?;
        let id = unsafe { (self.api.find_instance_by_name)(self.raw, cname.as_ptr()) };
        if id <= 0 {
            return Ok(None);
        }
        Ok(Some(id))
    }

    /// Удалить инстанс (сначала выгрузив его, если он загружен).
    pub fn remove_instance(&self, id: InstanceId) -> Result<()> {
        let rc = unsafe { (self.api.remove_instance)(self.raw, id) };
        check(rc, &self.last_error())
    }

    /// Сменить режим удержания инстанса (`KEEP_LOADED`/`LOAD_ON_DEMAND`).
    pub fn set_retention(&self, id: InstanceId, mode: i32) -> Result<()> {
        let rc = unsafe { (self.api.set_instance_retention_mode)(self.raw, id, mode) };
        check(rc, &self.last_error())
    }

    /// Загрузить модель инстанса (для `LOAD_ON_DEMAND` происходит само при запросе).
    pub fn load(&self, id: InstanceId) -> Result<()> {
        let rc = unsafe { (self.api.load_instance)(self.raw, id) };
        check(rc, &self.last_error())
    }

    /// Выгрузить модель инстанса (освободить VRAM) — ядро диспетчера (§A4).
    pub fn unload(&self, id: InstanceId) -> Result<()> {
        let rc = unsafe { (self.api.unload_instance)(self.raw, id) };
        check(rc, &self.last_error())
    }

    /// Дождаться `LOADED`/`SERVING`/`FAILED`, опрашивая `list_instances`.
    pub fn wait_loaded(
        &self,
        id: InstanceId,
        timeout: Duration,
        poll: Duration,
    ) -> Result<Instance> {
        let t0 = Instant::now();
        loop {
            if let Some(inst) = self.instance_by_id(id)? {
                if inst.is_loaded() || inst.is_failed() {
                    return Ok(inst);
                }
            } else {
                return Err(EngineError::InstanceNotFound {
                    name: format!("id={id}"),
                });
            }
            if t0.elapsed() >= timeout {
                let cur = self
                    .instance_by_id(id)?
                    .map(|i| i.state_name)
                    .unwrap_or_else(|| "НЕТ".to_string());
                return Err(EngineError::Other(format!(
                    "таймаут ожидания загрузки инстанса {id} ({:?}, состояние {cur})",
                    timeout
                )));
            }
            std::thread::sleep(poll);
        }
    }
}

/// Проверка кода возврата cluster API (у bridge-результатов — ещё и `ok`).
fn check(rc: i32, last_error: &str) -> Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(EngineError::Call {
            rc,
            last_error: last_error.to_string(),
        })
    }
}

impl Cluster {
    /// Эмбеддинги: `body_json` в формате `/v1/embeddings` (`oai_compat = true`).
    pub fn embeddings_json(
        &self,
        id: InstanceId,
        body_json: &str,
        oai_compat: bool,
    ) -> Result<JsonOutcome> {
        let body = json_cstr(body_json)?;
        let mut req = unsafe { (self.api.default_embeddings_request)() };
        req.instance_id = id;
        req.body_json = body.as_ptr();
        req.oai_compat = oai_compat as i32;
        let mut out = unsafe { (self.api.empty_json_result)() };
        let rc = unsafe { (self.api.embeddings)(self.raw, &req, &mut out) };
        Ok(self.take_json(rc, &mut out))
    }

    /// Реранк: `body_json` в формате `/v1/rerank`.
    pub fn rerank_json(&self, id: InstanceId, body_json: &str) -> Result<JsonOutcome> {
        let body = json_cstr(body_json)?;
        let mut req = unsafe { (self.api.default_rerank_request)() };
        req.instance_id = id;
        req.body_json = body.as_ptr();
        let mut out = unsafe { (self.api.empty_json_result)() };
        let rc = unsafe { (self.api.rerank)(self.raw, &req, &mut out) };
        Ok(self.take_json(rc, &mut out))
    }

    /// Чат. `reasoning`: `(mode, budget, format)` — семантика §11.4; маппинг
    /// «chat/chat-think» живёт в фасаде (A5), здесь только тонкий слой.
    pub fn chat_complete(
        &self,
        id: InstanceId,
        prompt: &str,
        n_predict: i32,
        temperature: f32,
        reasoning: Option<(&str, i32, Option<&str>)>,
    ) -> Result<ChatOutcome> {
        let prompt_c = json_cstr(prompt)?;
        let mut keep: Vec<CString> = Vec::new();
        let mut req = unsafe { (self.api.default_chat_request)() };
        req.instance_id = id;
        req.prompt = prompt_c.as_ptr();
        req.n_predict = n_predict;
        req.temperature = temperature;
        if let Some((mode, budget, format)) = reasoning {
            req.reasoning = anchor(mode, &mut keep)?;
            req.reasoning_budget = budget;
            if let Some(f) = format {
                req.reasoning_format = anchor(f, &mut keep)?;
            }
        }
        let mut out = unsafe { (self.api.empty_chat_result)() };
        let rc = unsafe { (self.api.chat_complete)(self.raw, &req, &mut out) };
        let res = ChatOutcome {
            rc,
            ok: out.ok == 1,
            text: cstr_or_empty(out.text),
            error: cstr_or_empty(out.error),
            metrics: out.metrics.into(),
        };
        drop(keep);
        unsafe { (self.api.chat_result_free)(&mut out) };
        Ok(res)
    }

    /// Скопировать результат JSON-вызова и освободить память движка: строки
    /// могли быть выделены даже при `ok = 0`, поэтому `*_free` вызывается всегда.
    fn take_json(&self, rc: i32, out: &mut ffi::JsonResultRaw) -> JsonOutcome {
        let res = JsonOutcome {
            rc,
            ok: out.ok == 1,
            status: out.status,
            json: cstr_or_empty(out.json),
            error: cstr_or_empty(out.error),
            metrics: out.metrics.into(),
        };
        unsafe { (self.api.json_result_free)(out) };
        res
    }
}

/// C-строка из JSON-тела/строки запроса.
fn json_cstr(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| EngineError::Other("NUL в теле запроса".into()))
}



impl Drop for Cluster {
    fn drop(&mut self) {
        unsafe { (self.api.cluster_destroy)(self.raw) };
    }
}


