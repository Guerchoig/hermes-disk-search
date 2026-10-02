//! Реестр инстансов (A2): конфиг + устройства → план `InstanceSpec` для `llm-host`.
//!
//! Маппинг (`PLAN_W2_LLM_HOST.md` §A2 + находки A1):
//! * устройство — `gpu.device_index` → **числовой** `manual_devices_csv`
//!   (`0` = CPU → индекс CPU-устройства, `1` = первый GPU);
//! * `allow_cpu = false` для GPU-ролей, чтобы молчаливый откат на CPU был ошибкой;
//! * `n_gpu_layers`: `gpu.n_gpu_layers` приоритетнее legacy `-ngl`; `-ngl 0`
//!   сохраняем как «роль на CPU» (так работал llama-server), но переводим в
//!   явное CPU-устройство + `allow_cpu = true`;
//! * модель резолвится через общий рантайм (`shared:<role>` → `current.json`).

use std::path::{Path, PathBuf};

use crate::cluster::{Device, InstanceSpec};
use crate::config::{LlmHostConfig, RoleConfig};
use crate::device::{cpu_device, selection_from_config_index, DeviceSelection};
use crate::error::Result;
use crate::runtime;

/// Один запланированный инстанс: роль + готовые параметры + пояснения.
#[derive(Debug, Clone)]
pub struct PlannedInstance {
    pub role: String,
    pub port: u16,
    pub model_spec: String,
    pub model_path: PathBuf,
    pub spec: InstanceSpec,
    /// Пояснения для лога/`hdsw status` (перенос legacy-ключей, CPU-роль и пр.).
    pub notes: Vec<String>,
}

impl PlannedInstance {
    /// Человекочитаемая строка для `llm-host status`/логов.
    pub fn summary(&self) -> String {
        format!(
            "{:<9} port={:<5} kind={:<10} devices={:<6} n_ctx={:<6} ngl={:<4} allow_cpu={:<5} \
             retention={} model={}",
            self.role,
            self.port,
            self.spec.model_kind.unwrap_or(-1),
            self.spec.manual_devices_csv.as_deref().unwrap_or("—"),
            self.spec.n_ctx.unwrap_or(0),
            self.spec.n_gpu_layers.unwrap_or(-1),
            self.spec.allow_cpu.unwrap_or(true),
            self.spec.retention_mode.unwrap_or(0),
            self.model_path.display()
        )
    }
}

/// Результат планирования одной роли.
// `Ready` крупнее `Failed`; боксить ради второго варианта не стоит (горячий план).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum RolePlan {
    /// Роль готова к подъёму.
    Ready(PlannedInstance),
    /// Роль не спланирована (нет модели и т.п.) — остальные роли это не блокирует.
    Failed { role: String, error: String },
}

impl RolePlan {
    pub fn role(&self) -> &str {
        match self {
            RolePlan::Ready(p) => &p.role,
            RolePlan::Failed { role, .. } => role,
        }
    }

    pub fn ready(&self) -> Option<&PlannedInstance> {
        match self {
            RolePlan::Ready(p) => Some(p),
            RolePlan::Failed { .. } => None,
        }
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, RolePlan::Failed { .. })
    }
}

/// Собрать план по всем ролям конфига.
///
/// Ошибки **не** прерывают планирование: роли в Python-версии стартуют
/// независимо (`SPIKES.md` §14.7 — чат падал с «модель не найдена», остальные
/// роли при этом работали), поэтому вызывающий видит состояние каждой роли.
pub fn plan(config: &LlmHostConfig, runtime_root: &Path, devices: &[Device]) -> Vec<RolePlan> {
    config
        .roles
        .iter()
        .map(|rc| match plan_role(config, rc, runtime_root, devices) {
            Ok(p) => RolePlan::Ready(p),
            Err(e) => RolePlan::Failed {
                role: rc.role.clone(),
                error: e.to_string(),
            },
        })
        .collect()
}

/// Строгий вариант: ошибка на первой неуспешной роли (для старта `llm-host`,
/// когда поднимать частичный набор нежелательно).
pub fn plan_strict(
    config: &LlmHostConfig,
    runtime_root: &Path,
    devices: &[Device],
) -> Result<Vec<PlannedInstance>> {
    config
        .roles
        .iter()
        .map(|rc| plan_role(config, rc, runtime_root, devices))
        .collect()
}

/// План одной роли.
fn plan_role(
    config: &LlmHostConfig,
    rc: &RoleConfig,
    runtime_root: &Path,
    devices: &[Device],
) -> Result<PlannedInstance> {
    let mut notes = rc.notes.clone();
    let model_path = runtime::resolve_model_checked(runtime_root, &rc.model_spec, &rc.role)?;

    // gpu.n_gpu_layers приоритетнее legacy -ngl; дефолт — полный офлоад
    let n_gpu_layers = if config.gpu.n_gpu_layers_set {
        config.gpu.n_gpu_layers
    } else {
        rc.legacy_n_gpu_layers.unwrap_or(config.gpu.n_gpu_layers)
    };
    let cpu_role = n_gpu_layers == 0 && rc.role != "whisper";

    let selection: DeviceSelection = if cpu_role {
        notes.push(format!(
            "{}: n_gpu_layers = 0 → роль на CPU (как было в llama-server); чтобы перенести \
             на GPU, уберите -ngl 0 и задайте gpu.n_gpu_layers = -1",
            rc.role
        ));
        match cpu_device(devices) {
            Some(d) => DeviceSelection::Csv(d.bridge_device_index.to_string()),
            None => DeviceSelection::Auto,
        }
    } else {
        selection_from_config_index(config.gpu.device_index, devices)?
    };

    let mut spec = InstanceSpec::new(&rc.role, &model_path.to_string_lossy());
    spec.manual_devices_csv = selection.csv();
    // для GPU-роли откат на CPU — ошибка (A1: allow_cpu=false не мешает GPU)
    spec.allow_cpu = Some(cpu_role);
    spec.model_kind = Some(rc.model_kind);
    spec.embedding = Some(rc.embedding);
    spec.reranking = Some(rc.reranking);
    spec.retention_mode = Some(rc.retention_mode);
    spec.load_on_demand_grace_seconds = Some(rc.grace_seconds);
    if rc.n_ctx > 0 {
        spec.n_ctx = Some(rc.n_ctx);
    }
    spec.n_batch = rc.n_batch;
    spec.n_ubatch = rc.n_ubatch;
    spec.n_threads = rc.n_threads;
    // KV-кэш роли (наш патч движка): q8_0 экономит ~половину KV у чата.
    spec.cache_type_k = rc.cache_type_k;
    spec.cache_type_v = rc.cache_type_v;
    if !cpu_role {
        spec.n_gpu_layers = Some(n_gpu_layers);
    }

    Ok(PlannedInstance {
        role: rc.role.clone(),
        port: rc.port,
        model_spec: rc.model_spec.clone(),
        model_path,
        spec,
        notes,
    })
}
