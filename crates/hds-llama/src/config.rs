//! Чтение `config.yaml` для `llm-host` (A2): маппинг
//! `llm_server.*` / `llm.*` / `gpu.*` → параметры инстансов.
//!
//! Правила совместимости (`PLAN_W2_LLM_HOST.md` §A2):
//! * старые ключи `llm_server.<role>.{port,model,ctx_per_slot,extra_args}`
//!   продолжают работать; `--ctx-size` в llama-server был `parallel * ctx_per_slot`,
//!   поэтому `n_ctx` считаем так же;
//! * из `extra_args` понимаем `-ngl/--n-gpu-layers`, `--batch-size`, `--ubatch-size`,
//!   `-t/--threads`; остальные флаги (например `--cache-type-k`) **не** молча
//!   игнорируются, а попадают в `warnings` для `hdsw check`;
//! * новые ключи `llm.<role>.{model,n_ctx,retention,grace_seconds}` и `gpu.*`
//!   имеют приоритет;
//! * `shared:<role>` резолвится в общий рантайм машины (`%LOCALAPPDATA%\llama-runtime`),
//!   а не в каталог проекта — иначе повторяется дефект `SPIKES.md` §14.7.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_yaml::Value;

use crate::error::{EngineError, Result};
use crate::ffi::{model_kind, retention};

/// Роли, которые поднимает `llm-host` (порядок важен для логов/`status`).
pub const ROLES: [&str; 4] = ["chat", "embedding", "rerank", "whisper"];

/// Режим работы LLM (`llm_server.mode`, §8.7 плана).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Встроенный рантайм (движок) — цель W2.
    Embedded,
    /// Фасад совместимости поверх внешнего владельца.
    Facade,
    /// LLM выключен (только FTS) — аварийный режим.
    Off,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Embedded => "embedded",
            Mode::Facade => "facade",
            Mode::Off => "off",
        }
    }

    /// Разбор значения ключа (неизвестное ⇒ `Embedded`, как было по умолчанию).
    pub fn parse(s: &str) -> Mode {
        match s.trim().to_lowercase().as_str() {
            "facade" => Mode::Facade,
            "off" | "none" | "disabled" => Mode::Off,
            _ => Mode::Embedded,
        }
    }
}

/// `gpu.*`: бюджет VRAM, устройство и приоритеты вытеснения (§A4).
#[derive(Debug, Clone)]
pub struct GpuConfig {
    /// Наш индекс устройства: `0` — CPU, `1` — первый GPU (§A2).
    pub device_index: i32,
    /// Явно ли задан `gpu.n_gpu_layers` в конфиге (иначе уважаем legacy `-ngl`).
    pub n_gpu_layers_set: bool,
    pub n_gpu_layers: i32,
    pub reserve_mb: u64,
    pub vram_budget_mb: Option<u64>,
    pub evict_idle_sec: u64,
    /// Приоритеты вытеснения: chat 100, embedding 40, rerank 30, whisper 20.
    pub priorities: BTreeMap<String, i32>,
}

impl Default for GpuConfig {
    fn default() -> Self {
        let mut priorities = BTreeMap::new();
        priorities.insert("chat".to_string(), 100);
        priorities.insert("embedding".to_string(), 40);
        priorities.insert("rerank".to_string(), 30);
        priorities.insert("whisper".to_string(), 20);
        GpuConfig {
            device_index: 1, // первый GPU: без явного выбора движок может уйти на CPU (R32)
            n_gpu_layers_set: false,
            n_gpu_layers: -1,
            reserve_mb: 1024,
            vram_budget_mb: None,
            evict_idle_sec: 600, // решение заказчика 29.09.2026
            priorities,
        }
    }
}

/// Параметры одной роли для `llm-host`.
#[derive(Debug, Clone)]
pub struct RoleConfig {
    pub role: String,
    /// Спецификатор модели из конфига (`shared:<role>` или путь).
    pub model_spec: String,
    pub port: u16,
    pub n_ctx: i32,
    /// `retention_mode`: chat — `KEEP_LOADED`, остальные — `LOAD_ON_DEMAND`.
    pub retention_mode: i32,
    pub grace_seconds: i32,
    pub model_kind: i32,
    pub embedding: bool,
    pub reranking: bool,
    pub n_batch: Option<i32>,
    pub n_ubatch: Option<i32>,
    pub n_threads: Option<i32>,
    /// Legacy `-ngl` из `llm_server.<role>.extra_args` (используется, если
    /// `gpu.n_gpu_layers` не задан явно).
    pub legacy_n_gpu_layers: Option<i32>,
    /// Замечания по роли (идут в `warnings` и `hdsw check`).
    pub notes: Vec<String>,
}

/// Конфиг `llm-host`: всё, что нужно треку A (без чтения индекса/поиска).
#[derive(Debug, Clone)]
pub struct LlmHostConfig {
    pub path: PathBuf,
    pub host: String,
    pub parallel: i32,
    pub autostart: bool,
    pub start_timeout: u64,
    pub mode: Mode,
    /// `llm.model_policy`: `fixed` (без авто-деградации) по умолчанию.
    pub model_policy: String,
    pub gpu: GpuConfig,
    pub roles: Vec<RoleConfig>,
    pub warnings: Vec<String>,
}

impl LlmHostConfig {
    /// Роль по имени.
    pub fn role(&self, name: &str) -> Option<&RoleConfig> {
        self.roles.iter().find(|r| r.role == name)
    }
}

/// Прочитать YAML-конфиг (BOM допускается, как в `hds/config.py`).
pub fn load(path: &Path) -> Result<LlmHostConfig> {
    let text = std::fs::read_to_string(path).map_err(|e| EngineError::Config {
        path: path.to_path_buf(),
        detail: format!("не читается: {e}"),
    })?;
    let root: Value = serde_yaml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| {
        EngineError::Config {
            path: path.to_path_buf(),
            detail: format!("не разобран как YAML: {e}"),
        }
    })?;
    Ok(build(path, &root))
}

/// Достать значение по пути вида `index.roots` (порт `hds/config.py:dig`).
pub fn dig<'a>(root: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut cur = root;
    for part in dotted.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

fn dig_str(root: &Value, dotted: &str) -> Option<String> {
    dig(root, dotted)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn dig_i64(root: &Value, dotted: &str) -> Option<i64> {
    dig(root, dotted).and_then(|v| v.as_i64())
}

fn dig_bool(root: &Value, dotted: &str) -> Option<bool> {
    dig(root, dotted).and_then(|v| v.as_bool())
}

/// Собрать конфиг из уже разобранного YAML (тестируется без файловой системы).
pub fn build(path: &Path, root: &Value) -> LlmHostConfig {
    let mut warnings: Vec<String> = Vec::new();
    let mut gpu = GpuConfig::default();
    if let Some(v) = dig_i64(root, "gpu.device_index") {
        gpu.device_index = v as i32;
    }
    if let Some(v) = dig_i64(root, "gpu.n_gpu_layers") {
        gpu.n_gpu_layers = v as i32;
        gpu.n_gpu_layers_set = true;
    }
    if let Some(v) = dig_i64(root, "gpu.reserve_mb") {
        gpu.reserve_mb = v.max(0) as u64;
    }
    if let Some(v) = dig_i64(root, "gpu.vram_budget_mb") {
        gpu.vram_budget_mb = Some(v.max(0) as u64);
    }
    if let Some(v) = dig_i64(root, "gpu.evict_idle_sec") {
        gpu.evict_idle_sec = v.max(0) as u64;
    }
    if let Some(map) = dig(root, "gpu.priorities").and_then(|v| v.as_mapping()) {
        for (k, v) in map {
            if let (Some(name), Some(num)) = (k.as_str(), v.as_i64()) {
                gpu.priorities.insert(name.to_string(), num as i32);
            }
        }
    }

    let parallel = dig_i64(root, "llm_server.parallel").unwrap_or(1).max(1) as i32;
    let mut roles: Vec<RoleConfig> = Vec::new();
    for role in ROLES {
        if let Some(rc) = build_role(root, role, parallel, &gpu, &mut warnings) {
            roles.push(rc);
        }
    }

    LlmHostConfig {
        path: path.to_path_buf(),
        host: dig_str(root, "llm_server.host").unwrap_or_else(|| "127.0.0.1".to_string()),
        parallel,
        autostart: dig_bool(root, "llm_server.autostart").unwrap_or(true),
        start_timeout: dig_i64(root, "llm_server.start_timeout").unwrap_or(300).max(1) as u64,
        mode: Mode::parse(
            &dig_str(root, "llm_server.mode").unwrap_or_else(|| "embedded".to_string()),
        ),
        model_policy: dig_str(root, "llm.model_policy").unwrap_or_else(|| "fixed".to_string()),
        gpu,
        roles,
        warnings,
    }
}

/// Умолчания роли — из `hds/llama_server.py` (`_role_cfg` и `DEFAULTS`).
fn role_defaults(role: &str) -> (u16, i32, &'static str) {
    match role {
        "chat" => (8010, 32768, "--cache-type-k q8_0 --cache-type-v q8_0 -ngl 99"),
        "embedding" => (8011, 8192, "--batch-size 8192 --ubatch-size 8192 -ngl 99"),
        "rerank" => (8012, 8192, "-ngl 0"),
        // whisper — не HTTP-роль: ASR-инстанс поднимается в W3
        _ => (0, 0, ""),
    }
}

/// Параметры роли: новые ключи `llm.<role>.*` приоритетнее старых `llm_server.<role>.*`.
fn build_role(
    root: &Value,
    role: &str,
    parallel: i32,
    gpu: &GpuConfig,
    warnings: &mut Vec<String>,
) -> Option<RoleConfig> {
    let (def_port, def_ctx, def_extra) = role_defaults(role);
    let mut notes: Vec<String> = Vec::new();

    // Модель: llm.<role>.model → llm_server.<role>.model → index.whisper_model_path
    let model_spec = dig_str(root, &format!("llm.{role}.model"))
        .or_else(|| dig_str(root, &format!("llm_server.{role}.model")))
        .or_else(|| {
            if role == "whisper" {
                dig_str(root, "index.whisper_model_path")
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            if role == "whisper" {
                String::new()
            } else {
                // дефолт W2: модель из общего рантайма, а не каталог проекта (§14.7)
                format!("shared:{role}")
            }
        });
    if role == "whisper" && model_spec.is_empty() {
        return None; // ASR-инстанс появится в W3
    }

    let port = dig_i64(root, &format!("llm.{role}.port"))
        .or_else(|| dig_i64(root, &format!("llm_server.{role}.port")))
        .unwrap_or(def_port as i64) as u16;
    let ctx_per_slot =
        dig_i64(root, &format!("llm_server.{role}.ctx_per_slot")).unwrap_or(def_ctx as i64);
    // как у llama-server: --ctx-size = parallel * ctx_per_slot
    let n_ctx = dig_i64(root, &format!("llm.{role}.n_ctx"))
        .unwrap_or(parallel as i64 * ctx_per_slot)
        .max(0) as i32;

    let extra = dig_str(root, &format!("llm_server.{role}.extra_args"))
        .unwrap_or_else(|| def_extra.to_string());
    let ea = parse_extra_args(&extra);
    if !ea.unknown.is_empty() {
        let msg = format!(
            "{role}: флаги extra_args не распознаны и игнорируются: {} — перенесите их \
             в llm.{role}.* / gpu.* (W2)",
            ea.unknown.join(", ")
        );
        warnings.push(msg.clone());
        notes.push(msg);
    }

    let default_retention = if role == "chat" {
        retention::KEEP_LOADED
    } else {
        retention::LOAD_ON_DEMAND
    };
    let retention_mode = match dig_str(root, &format!("llm.{role}.retention")).as_deref() {
        Some("keep_loaded") | Some("keep-loaded") | Some("keep") => retention::KEEP_LOADED,
        Some("load_on_demand") | Some("load-on-demand") | Some("ondemand") => {
            retention::LOAD_ON_DEMAND
        }
        Some(other) => {
            let msg = format!(
                "{role}: неизвестное llm.{role}.retention = «{other}» (ожидается \
                 keep_loaded|load_on_demand) — беру значение по умолчанию"
            );
            warnings.push(msg.clone());
            notes.push(msg);
            default_retention
        }
        None => default_retention,
    };
    let grace_seconds =
        dig_i64(root, &format!("llm.{role}.grace_seconds")).unwrap_or(300).max(0) as i32;

    let (kind, embedding, reranking) = match role {
        "embedding" => (model_kind::EMBEDDINGS, true, false),
        "rerank" => (model_kind::RERANK, false, true),
        "whisper" => (model_kind::WHISPER, false, false),
        _ => (model_kind::TEXT, false, false),
    };

    // legacy -ngl: уважаем, если gpu.n_gpu_layers не задан явно
    let legacy_n_gpu_layers = ea.n_gpu_layers;
    if let Some(n) = legacy_n_gpu_layers {
        if gpu.n_gpu_layers_set {
            if role != "whisper" {
                let msg = format!(
                    "{role}: -ngl {n} из extra_args игнорируется — задано \
                     gpu.n_gpu_layers = {}",
                    gpu.n_gpu_layers
                );
                warnings.push(msg.clone());
                notes.push(msg);
            }
        } else if role != "whisper" {
            notes.push(format!(
                "{role}: -ngl {n} взят из legacy extra_args; перенесите в gpu.n_gpu_layers"
            ));
        }
    }

    Some(RoleConfig {
        role: role.to_string(),
        model_spec,
        port,
        n_ctx,
        retention_mode,
        grace_seconds,
        model_kind: kind,
        embedding,
        reranking,
        n_batch: ea.n_batch,
        n_ubatch: ea.n_ubatch,
        n_threads: ea.n_threads,
        legacy_n_gpu_layers,
        notes,
    })
}

/// Разобранные флаги legacy `extra_args`.
#[derive(Debug, Default, PartialEq)]
pub struct ExtraArgs {
    pub n_gpu_layers: Option<i32>,
    pub n_batch: Option<i32>,
    pub n_ubatch: Option<i32>,
    pub n_threads: Option<i32>,
    /// Нераспознанные флаги (с значениями) — для `hdsw check`.
    pub unknown: Vec<String>,
}

/// Разобрать строку `extra_args` (как её писал llama-server).
pub fn parse_extra_args(s: &str) -> ExtraArgs {
    let mut out = ExtraArgs::default();
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0usize;
    while i < tokens.len() {
        let t = tokens[i];
        // значение флага — следующий токен, если он не начинается с '-'
        let value = |i: &mut usize| -> Option<String> {
            match tokens.get(*i + 1).copied() {
                Some(v) if !v.starts_with('-') => {
                    *i += 1;
                    Some(v.to_string())
                }
                _ => None,
            }
        };
        match t {
            "-ngl" | "--n-gpu-layers" | "--gpu-layers" => {
                out.n_gpu_layers = value(&mut i).and_then(|v| v.parse().ok());
            }
            "-b" | "--batch-size" => {
                out.n_batch = value(&mut i).and_then(|v| v.parse().ok());
            }
            "-ub" | "--ubatch-size" => {
                out.n_ubatch = value(&mut i).and_then(|v| v.parse().ok());
            }
            "-t" | "--threads" => {
                out.n_threads = value(&mut i).and_then(|v| v.parse().ok());
            }
            other if other.starts_with('-') => {
                let v = value(&mut i);
                out.unknown.push(match v {
                    Some(v) => format!("{other} {v}"),
                    None => other.to_string(),
                });
            }
            other => out.unknown.push(other.to_string()),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Legacy-строка из реального конфига: понятые флаги + список непонятых.
    #[test]
    fn extra_args_parses_known_flags() {
        let ea = parse_extra_args("--cache-type-k q8_0 --cache-type-v q8_0 -ngl 99");
        assert_eq!(ea.n_gpu_layers, Some(99));
        assert_eq!(
            ea.unknown,
            vec![
                "--cache-type-k q8_0".to_string(),
                "--cache-type-v q8_0".to_string()
            ]
        );

        let ea = parse_extra_args("--batch-size 8192 --ubatch-size 8192 -ngl 99");
        assert_eq!(ea.n_batch, Some(8192));
        assert_eq!(ea.n_ubatch, Some(8192));
        assert_eq!(ea.n_gpu_layers, Some(99));
        assert!(ea.unknown.is_empty());

        let ea = parse_extra_args("-ngl 0");
        assert_eq!(ea.n_gpu_layers, Some(0));
        assert!(ea.unknown.is_empty());

        let ea = parse_extra_args("-fa --jinja");
        assert_eq!(ea.n_gpu_layers, None);
        assert_eq!(ea.unknown, vec!["-fa".to_string(), "--jinja".to_string()]);
    }

    /// Маппинг конфига: `shared:<role>`, `n_ctx = parallel * ctx_per_slot`,
    /// retention, модель-виды и warning про нераспознанные флаги.
    #[test]
    fn config_maps_roles_like_python() {
        let yaml = r#"
index:
  whisper_model_path: ""
llm_server:
  host: "127.0.0.1"
  parallel: 1
  chat:
    port: 8010
    model: "shared:chat"
    ctx_per_slot: 32768
    extra_args: "--cache-type-k q8_0 -ngl 99"
  embedding:
    port: 8011
    model: "shared:embedding"
    ctx_per_slot: 8192
  rerank:
    port: 8012
    model: "shared:rerank"
    ctx_per_slot: 8192
    extra_args: "-ngl 0"
"#;
        let root: Value = serde_yaml::from_str(yaml).expect("yaml");
        let cfg = build(Path::new("test.yaml"), &root);

        assert_eq!(cfg.mode, Mode::Embedded);
        assert_eq!(cfg.model_policy, "fixed");
        assert_eq!(cfg.gpu.device_index, 1);
        assert_eq!(cfg.parallel, 1);
        assert_eq!(cfg.roles.len(), 3, "whisper без модели роль не планируем");

        let chat = cfg.role("chat").expect("chat");
        assert_eq!(chat.model_spec, "shared:chat");
        assert_eq!(chat.n_ctx, 32768);
        assert_eq!(chat.retention_mode, retention::KEEP_LOADED);
        assert_eq!(chat.model_kind, model_kind::TEXT);
        assert_eq!(chat.legacy_n_gpu_layers, Some(99));

        let emb = cfg.role("embedding").expect("embedding");
        assert_eq!(emb.model_kind, model_kind::EMBEDDINGS);
        assert!(emb.embedding && !emb.reranking);
        assert_eq!(emb.retention_mode, retention::LOAD_ON_DEMAND);
        assert_eq!(emb.n_batch, Some(8192));
        assert_eq!(emb.n_ubatch, Some(8192));

        let rr = cfg.role("rerank").expect("rerank");
        assert_eq!(rr.model_kind, model_kind::RERANK);
        assert!(rr.reranking && !rr.embedding);
        assert_eq!(rr.legacy_n_gpu_layers, Some(0), "реранкер по legacy — на CPU");

        assert!(
            cfg.warnings.iter().any(|w| w.contains("--cache-type-k q8_0")),
            "должно быть предупреждение про нераспознанный флаг: {:?}",
            cfg.warnings
        );
    }
}



