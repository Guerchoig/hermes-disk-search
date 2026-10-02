//! Наблюдаемость `hdsw llm-host status` (A4 шаг 2 → A6).
//!
//! Требования, из которых собран формат:
//! * §A4: по каждому инстансу — `name/state/retention/active_request_count/
//!   queued_request_count/last_error` и занимаемая VRAM (**NVML − baseline**);
//!   эти же поля читает UI («карточка GPU и модели»);
//! * §A6: смысловые строки совпадают с `python -m hds.llama_server status`
//!   (`role`/`state`/`port`/`model`/`ctx`), чтобы UI не переписывать целиком;
//! * A-6/A-7: видны бюджет (`gpu.reserve_mb`, cap, вычет), фактический свободный
//!   остаток VRAM, состояние `index.pause`/heartbeat и **точные цифры нехватки**.
//!
//! Модуль не делает ввода-вывода: свежие данные (устройства, инстансы, NVML,
//! пауза) собирает вызывающий (`bin/llm_host_status`, далее — `llm-host`), поэтому
//! отчёт проверяется тестами без движка.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::cluster::{Device, Instance};
use crate::config::{LlmHostConfig, RoleConfig};
use crate::dispatch::Plan;
use crate::ffi::{retention, state};
use crate::pause::HeartbeatState;
use crate::registry::RolePlan;
use crate::vram::{VramSnapshot, VramSource};

/// Статус одной роли: план (`A2`) + живое состояние инстанса (`list_instances`).
#[derive(Debug, Clone, Default)]
pub struct RoleStatus {
    pub role: String,
    pub port: u16,
    /// `base_url` как у Python-версии (`http://host:port/v1`).
    pub base_url: String,
    pub model: String,
    pub model_ready: bool,
    /// Настроенный контекст (`parallel * ctx_per_slot`, как `ctx_configured`).
    pub n_ctx: i32,
    pub devices_csv: String,
    pub n_gpu_layers: i32,
    /// `keep` | `on_demand`.
    pub retention: String,
    /// `LOADED`/`UNLOADED`/`GRACE`/`FAILED`/`—` (роль не поднята в этом процессе).
    pub state: String,
    pub state_code: Option<i32>,
    pub running: bool,
    pub active_requests: i32,
    pub queued_requests: i32,
    pub last_error: String,
    /// Оценка «модель + KV» (МиБ) — из `vram_budget`/`budget::estimate_need_mib`.
    pub need_mib: u64,
    /// Доля занятой VRAM, приходящаяся на роль (только если известен baseline).
    pub vram_measured_mib: Option<u64>,
    pub notes: Vec<String>,
}

impl RoleStatus {
    /// Строка как у `PlannedInstance::summary` (роль → состояние/порт/модель/контекст).
    pub fn line(&self) -> String {
        let measured = self
            .vram_measured_mib
            .map(|v| format!(" замер≈{v} МиБ"))
            .unwrap_or_default();
        format!(
            "{:<9} port={:<5} state={:<8} retention={:<9} n_ctx={:<6} devices={:<4} ngl={:<4} \
             нужно {:>6} МиБ{} модель={}",
            self.role,
            self.port,
            self.state,
            self.retention,
            self.n_ctx,
            if self.devices_csv.is_empty() {
                "—"
            } else {
                self.devices_csv.as_str()
            },
            self.n_gpu_layers,
            self.need_mib,
            measured,
            self.model
        )
    }
}

/// Всё, что нужно для отчёта (собирается вызывающим).
pub struct StatusInput<'a> {
    pub config: &'a LlmHostConfig,
    pub runtime_root: &'a Path,
    pub engine_dir: Option<&'a Path>,
    pub devices: &'a [Device],
    pub instances: &'a [Instance],
    pub vram: Option<VramSnapshot>,
    pub vram_source: VramSource,
    /// Чем был занят GPU на старте `llm-host` (для «NVML − baseline»).
    pub baseline_used_mib: Option<u64>,
    pub paused: bool,
    pub pause_file: &'a Path,
    pub heartbeat: Option<HeartbeatState>,
    /// План инстансов (A2): модель, устройство, контекст, retention по ролям.
    pub planned: &'a [RolePlan],
    /// Оценка «модель + KV» по ролям, МиБ.
    pub needs: &'a BTreeMap<String, u64>,
    /// Последнее решение диспетчера VRAM (A4 шаг 2) — для строки «диспетчер».
    pub decision: Option<&'a Plan>,
    /// Прогноз: что решит диспетчер, если запрос роли придёт сейчас
    /// (роль + её план). В резидентном `llm-host` прогноз не нужен — там есть
    /// фактическое последнее решение; разовому `status` он показывает «а что будет».
    pub forecast: Option<(&'a str, &'a Plan)>,
}

/// Отчёт `llm-host status`: печатается человеку и отдаётся `--json` (UI/MCP).
#[derive(Debug, Clone, Default)]
pub struct StatusReport {
    pub config_path: String,
    pub mode: String,
    pub host: String,
    pub parallel: i32,
    pub model_policy: String,
    pub engine_dir: Option<String>,
    pub runtime_root: String,
    pub devices: Vec<String>,
    pub vram: Option<VramSnapshot>,
    pub vram_source: String,
    pub effective_free_mib: Option<u64>,
    pub reserve_mib: u64,
    pub external_vram_mb: u64,
    pub vram_budget_mb: Option<u64>,
    pub baseline_used_mib: Option<u64>,
    /// Наша занятость = NVML used − baseline (если baseline известен), МиБ.
    pub used_by_us_mib: Option<u64>,
    pub policy: String,
    pub pause_index_on_query: bool,
    pub priorities: BTreeMap<String, i32>,
    pub paused: bool,
    pub pause_file: String,
    pub heartbeat: Option<HeartbeatState>,
    pub roles: Vec<RoleStatus>,
    pub warnings: Vec<String>,
    /// Строки последнего решения диспетчера (что и почему выгружалось).
    pub decision_lines: Vec<String>,
    /// Роль прогноза («если запрос придёт сейчас») и строки решения.
    pub forecast_role: Option<String>,
    pub forecast_lines: Vec<String>,
}

impl StatusReport {
    /// Собрать отчёт из снимков (без ввода-вывода).
    pub fn build(input: StatusInput<'_>) -> StatusReport {
        let cfg = input.config;
        let effective_free = cfg.gpu.effective_free_mib(input.vram.map(|v| v.free_mib));

        // инстансы: имя == роль (A2), поэтому ищем по имени
        let find_inst = |role: &str| input.instances.iter().find(|i| i.name == role);
        let mut roles: Vec<RoleStatus> = cfg
            .roles
            .iter()
            .map(|rc| role_status(cfg, rc, &input, find_inst(&rc.role)))
            .collect();
        // инстансы, которых нет среди ролей конфига — показываем (диагностика)
        for inst in input.instances {
            if !cfg.roles.iter().any(|r| r.role == inst.name) {
                roles.push(RoleStatus {
                    role: inst.name.clone(),
                    state: inst.state_name.clone(),
                    state_code: Some(inst.state),
                    running: inst.is_loaded(),
                    retention: retention_label(inst.retention_mode),
                    model: inst.model_path.clone(),
                    model_ready: PathBuf::from(&inst.model_path).is_file(),
                    active_requests: inst.active_request_count,
                    queued_requests: inst.queued_request_count,
                    last_error: inst.last_error.clone(),
                    notes: vec![
                        "инстанс вне плана: роль не описана в конфиге (создан в этом процессе)"
                            .to_string(),
                    ],
                    ..Default::default()
                });
            }
        }

        // «занимаемая VRAM (NVML − baseline)»: per-instance метрик памяти кластер не
        // даёт, поэтому дельту распределяем пропорционально оценке «модель + KV».
        let used_by_us = match (input.vram, input.baseline_used_mib) {
            (Some(v), Some(base)) => Some(v.used_mib.saturating_sub(base)),
            _ => None,
        };
        if let Some(used) = used_by_us {
            let loaded: Vec<usize> = roles
                .iter()
                .enumerate()
                .filter(|(_, r)| r.running && r.need_mib > 0)
                .map(|(i, _)| i)
                .collect();
            let sum: u64 = loaded.iter().map(|&i| roles[i].need_mib).sum();
            for &i in &loaded {
                if let Some(v) = (used * roles[i].need_mib).checked_div(sum) {
                    roles[i].vram_measured_mib = Some(v);
                }
            }
        }

        let mut warnings = cfg.warnings.clone();
        for r in &roles {
            if !r.last_error.is_empty() {
                warnings.push(format!("{}: last_error: {}", r.role, r.last_error));
            }
        }
        if input.vram.is_none() {
            warnings.push(
                "свободная VRAM не измерена (NVML недоступен) — решения диспетчера будут \
                 без гарантий; на macOS источник — Metal (задел W2-7)"
                    .to_string(),
            );
        }

        StatusReport {
            config_path: cfg.path.display().to_string(),
            mode: cfg.mode.as_str().to_string(),
            host: cfg.host.clone(),
            parallel: cfg.parallel,
            model_policy: cfg.model_policy.clone(),
            engine_dir: input.engine_dir.map(|p| p.display().to_string()),
            runtime_root: input.runtime_root.display().to_string(),
            devices: input.devices.iter().map(device_line).collect(),
            vram: input.vram,
            vram_source: input.vram_source.as_config_key().to_string(),
            effective_free_mib: effective_free,
            reserve_mib: cfg.gpu.reserve_mb,
            external_vram_mb: cfg.gpu.external_vram_mb,
            vram_budget_mb: cfg.gpu.vram_budget_mb,
            baseline_used_mib: input.baseline_used_mib,
            used_by_us_mib: used_by_us,
            policy: cfg.gpu.policy.as_str().to_string(),
            pause_index_on_query: cfg.gpu.pause_index_on_query,
            priorities: cfg.gpu.priorities.clone(),
            paused: input.paused,
            pause_file: input.pause_file.display().to_string(),
            heartbeat: input.heartbeat,
            roles,
            warnings,
            decision_lines: input.decision.map(|d| d.lines()).unwrap_or_default(),
            forecast_role: input.forecast.map(|(role, _)| role.to_string()),
            forecast_lines: input
                .forecast
                .map(|(_, plan)| plan.lines())
                .unwrap_or_default(),
        }
    }

    /// Атрибуция занятой VRAM по процессам (L1, `W4_REPORT.md` §15).
    ///
    /// Считается по запросу, а не в `build`: PDH-замер (наш процесс vs чужие) нужен
    /// только там, где цифру показывают — в человеческой строке и JSON. `None` —
    /// NVML/PDH недоступны (тогда честнее промолчать, чем нарисовать разбивку).
    fn attribution(&self) -> Option<crate::gpuattr::VramAttribution> {
        self.vram
            .map(|v| crate::gpuattr::attribution_for_current_process(v.used_mib))
            .filter(|a| a.total_used_mib > 0)
    }

    /// Человекочитаемый отчёт (то, что печатает `hdsw llm-host status`).
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!(
            "конфиг: {} | режим llm: {} | host {} | parallel {} | модель: {}",
            self.config_path, self.mode, self.host, self.parallel, self.model_policy
        ));
        out.push(format!(
            "диспетчер VRAM: gpu.policy {} | пауза индексации при запросе {} | приоритеты {:?}",
            self.policy, self.pause_index_on_query, self.priorities
        ));
        out.push(format!(
            "движок: {} | рантайм: {}",
            self.engine_dir.as_deref().unwrap_or("не проверялся"),
            self.runtime_root
        ));
        out.push(format!("устройства: {}", self.devices.join("; ")));
        match self.vram {
            Some(v) => {
                out.push(format!(
                    "VRAM ({}): занято {} / всего {} МиБ, свободно {} МиБ; к использованию {} МиБ \
                     (резерв {}, вычет на чужих {}, cap {})",
                    self.vram_source,
                    v.used_mib,
                    v.total_mib,
                    v.free_mib,
                    self.effective_free_mib
                        .map(|f| f.to_string())
                        .unwrap_or_else(|| "—".into()),
                    self.reserve_mib,
                    self.external_vram_mb,
                    self.vram_budget_mb
                        .map(|c| format!("{c} МиБ"))
                        .unwrap_or_else(|| "не задан".into())
                ));
            }
            None => out.push(format!(
                "VRAM: замер недоступен (источник {}), бюджет не проверяется",
                self.vram_source
            )),
        }
        // L1: «занято 10765 МиБ» без имени — бесполезно (`W4_REPORT.md` §14): показываем,
        // сколько держим мы, а сколько чужие (PDH-счётчик Windows).
        if let Some(attribution) = self.attribution() {
            out.push(attribution.line());
        }
        if let Some(used) = self.used_by_us_mib {
            out.push(format!(
                "наша занятость (NVML − baseline {} МиБ): {} МиБ",
                self.baseline_used_mib.unwrap_or(0),
                used
            ));
        }
        out.push(format!(
            "индексация: пауза {} ({}){}",
            if self.paused { "ДА" } else { "нет" },
            self.pause_file,
            self.heartbeat
                .as_ref()
                .map(|hb| format!(" | heartbeat: {}", heartbeat_line(hb)))
                .unwrap_or_else(|| " | heartbeat: нет данных".to_string())
        ));
        out.push(format!("роли ({}):", self.roles.len()));
        for r in &self.roles {
            out.push(format!("  {}", r.line()));
            for n in &r.notes {
                out.push(format!("    ^ {n}"));
            }
        }
        if !self.decision_lines.is_empty() {
            out.push("последнее решение диспетчера:".to_string());
            out.extend(self.decision_lines.iter().map(|l| format!("  {l}")));
        }
        if !self.forecast_lines.is_empty() {
            out.push(format!(
                "прогноз диспетчера, если запрос роли '{}' придёт сейчас:",
                self.forecast_role.as_deref().unwrap_or("?")
            ));
            out.extend(self.forecast_lines.iter().map(|l| format!("  {l}")));
        }
        if !self.warnings.is_empty() {
            out.push("предупреждения:".to_string());
            out.extend(self.warnings.iter().map(|w| format!("  [warn] {w}")));
        }
        out
    }

    /// Машинночитаемый отчёт (`--json`): поля — из §A4 (то, что читает UI).
    pub fn json(&self) -> serde_json::Value {
        let roles: Vec<serde_json::Value> = self
            .roles
            .iter()
            .map(|r| {
                json!({
                    "role": r.role,
                    "port": r.port,
                    "base_url": r.base_url,
                    "state": r.state,
                    "state_code": r.state_code,
                    "running": r.running,
                    "retention": r.retention,
                    "model": r.model,
                    "model_ready": r.model_ready,
                    "n_ctx": r.n_ctx,
                    "devices_csv": r.devices_csv,
                    "n_gpu_layers": r.n_gpu_layers,
                    "active_request_count": r.active_requests,
                    "queued_request_count": r.queued_requests,
                    "last_error": r.last_error,
                    "need_mib": r.need_mib,
                    "vram_measured_mib": r.vram_measured_mib,
                    "notes": r.notes,
                })
            })
            .collect();
        json!({
            "config": self.config_path,
            "mode": self.mode,
            "host": self.host,
            "parallel": self.parallel,
            "model_policy": self.model_policy,
            "engine_dir": self.engine_dir,
            "runtime_root": self.runtime_root,
            "devices": self.devices,
            "vram": self.vram.map(|v| json!({
                "total_mib": v.total_mib, "used_mib": v.used_mib, "free_mib": v.free_mib,
            })),
            "vram_source": self.vram_source,
            "effective_free_mib": self.effective_free_mib,
            "reserve_mib": self.reserve_mib,
            "external_vram_mb": self.external_vram_mb,
            "vram_budget_mb": self.vram_budget_mb,
            "baseline_used_mib": self.baseline_used_mib,
            "used_by_us_mib": self.used_by_us_mib,
            "gpu": {
                "policy": self.policy,
                "pause_index_on_query": self.pause_index_on_query,
                "priorities": self.priorities,
            },
            "index": {
                "paused": self.paused,
                "pause_file": self.pause_file,
                "heartbeat": self.heartbeat.as_ref().map(|hb| json!({
                    "fresh": hb.fresh,
                    "paused": hb.paused,
                    "age_secs": hb.age_secs,
                    "seen": hb.seen,
                    "processed": hb.processed,
                    "errors": hb.errors,
                    "chunks": hb.chunks,
                    "total": hb.total,
                    "eta_sec": hb.eta_sec,
                    "current_path": hb.current_path,
                    "phase": hb.phase,
                })),
            },
            "roles": roles,
            "dispatcher": {
                "last_decision": self.decision_lines,
                "forecast_role": self.forecast_role,
                "forecast": self.forecast_lines,
            },
            "warnings": self.warnings,
        })
    }
}

/// Статус роли из плана (A2) и живого инстанса (если он есть в этом процессе).
fn role_status(
    cfg: &LlmHostConfig,
    rc: &RoleConfig,
    input: &StatusInput<'_>,
    inst: Option<&Instance>,
) -> RoleStatus {
    let planned_role = input.planned.iter().find(|p| p.role() == rc.role);
    let planned = match planned_role {
        Some(RolePlan::Ready(inst)) => Some(inst),
        _ => None,
    };
    let mut notes = rc.notes.clone();
    if let Some(RolePlan::Failed { error, .. }) = planned_role {
        notes.push(format!("роль не спланирована: {error}"));
    }
    let model = planned
        .map(|p| p.model_path.display().to_string())
        .or_else(|| inst.map(|i| i.model_path.clone()))
        .unwrap_or_default();
    let retention_mode = planned
        .and_then(|p| p.spec.retention_mode)
        .or_else(|| inst.map(|i| i.retention_mode))
        .unwrap_or(rc.retention_mode);
    let n_gpu_layers =
        planned
            .and_then(|p| p.spec.n_gpu_layers)
            .unwrap_or(match rc.legacy_n_gpu_layers {
                Some(0) => 0,
                _ => cfg.gpu.n_gpu_layers,
            });
    RoleStatus {
        role: rc.role.clone(),
        port: rc.port,
        base_url: format!("http://{}:{}/v1", cfg.host, rc.port),
        model_ready: !model.is_empty() && PathBuf::from(&model).is_file(),
        model,
        n_ctx: planned.and_then(|p| p.spec.n_ctx).unwrap_or(rc.n_ctx),
        devices_csv: planned
            .and_then(|p| p.spec.manual_devices_csv.clone())
            .unwrap_or_default(),
        n_gpu_layers,
        retention: retention_label(retention_mode),
        state: inst
            .map(|i| i.state_name.clone())
            .unwrap_or_else(|| "—".to_string()),
        state_code: inst.map(|i| i.state),
        running: inst.map(|i| i.is_loaded()).unwrap_or(false),
        active_requests: inst.map(|i| i.active_request_count).unwrap_or(0),
        queued_requests: inst.map(|i| i.queued_request_count).unwrap_or(0),
        last_error: inst.map(|i| i.last_error.clone()).unwrap_or_default(),
        need_mib: input.needs.get(&rc.role).copied().unwrap_or(0),
        vram_measured_mib: None,
        notes,
    }
}

/// Ключевое слово retention (`keep` | `on_demand`) — как в конфиге.
pub fn retention_label(mode: i32) -> String {
    if mode == retention::KEEP_LOADED {
        "keep".to_string()
    } else {
        "on_demand".to_string()
    }
}

/// Имя состояния для человека («не в кластере» вместо прочерка).
pub fn state_label(code: Option<i32>) -> String {
    match code {
        Some(c) => state::name(c).to_string(),
        None => "не в кластере".to_string(),
    }
}

/// Одна строка описания устройства (как в `llm_host_plan`).
pub fn device_line(d: &Device) -> String {
    format!(
        "index={} backend={} name={} free={:.0} МиБ total={:.0} МиБ",
        d.bridge_device_index,
        d.backend,
        d.name,
        d.memory_free_mib(),
        d.memory_total as f64 / (1024.0 * 1024.0)
    )
}

/// Расшифровка heartbeat для строки «индексация» (R30: пауза ≠ живой прогон).
fn heartbeat_line(hb: &HeartbeatState) -> String {
    let mut s = format!(
        "{} ({} с назад): просмотрено {}, обработано {}, ошибок {}, чанков {}",
        hb.label(),
        hb.age_secs,
        hb.seen,
        hb.processed,
        hb.errors,
        hb.chunks
    );
    if let Some(total) = hb.total {
        s.push_str(&format!(", всего {total}"));
    }
    if let Some(eta) = hb.eta_sec {
        s.push_str(&format!(", ETA ≈ {eta} с"));
    }
    if let Some(path) = &hb.current_path {
        s.push_str(&format!(", сейчас: {path}"));
    }
    s
}
