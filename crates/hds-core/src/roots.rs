//! Корни индексации → проекты (M1, `PLAN_M1_ACCESS_PROJECTS.md` §3.1).
//!
//! Контракт `index.roots`: элемент — строка (проект по умолчанию) **или**
//! объект `{path, project}`. Проект по умолчанию = имя младшей папки корня;
//! для корня диска — метка с двоеточием (`D:\` → `D:`). Один файл принадлежит
//! одному проекту: корень по **наибольшему совпадающему префиксу** пути
//! (сравнение по компонентам пути, регистр не важен — как `exclude_paths`).
//!
//! ```yaml
//! index:
//!   roots:
//!     - "E:\\Archive"                                 # project = "Archive"
//!     - { path: "D:\\Work\\Sales", project: "Sales" } # явно
//!     - { path: "F:\\Reports",     project: "Sales" } # тот же проект
//! ```

use serde_yaml::Value;

use crate::config::{dig, Config};

/// Корень индексации с именем проекта.
#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    /// Путь корня как в конфиге (строкой — для отображения и сопоставления).
    pub path: String,
    /// Имя проекта (явное или по умолчанию).
    pub project: String,
}

/// Компоненты пути (разделители `/` и `\`, пустые и `.` — долой, регистр вниз).
fn comps_lower(path: &str) -> Vec<String> {
    path.replace('\\', "/")
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(|c| c.to_lowercase())
        .collect()
}

/// Проект по умолчанию для корня: младшая папка; корень диска → `D:`.
///
/// `D:\` / `D:` / `D:/` → `D:`; `E:\Archive` → `Archive`; `\\srv\share` → `share`.
pub fn default_project(path: &str) -> String {
    let norm = path.replace('\\', "/");
    let raw: Vec<&str> = norm
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    match raw.len() {
        0 => path.trim().to_string(),
        // Последний компонент: для `D:\` это «D:» (корень диска — метка с
        // двоеточием), для `E:\Archive` — «Archive» (младшая папка).
        _ => raw[raw.len() - 1].to_string(),
    }
}

/// Корни из конфига: строки и объекты `{path, project}` (план §3.1).
pub fn roots_from_cfg(cfg: &Config) -> Vec<Root> {
    dig(cfg, "index.roots")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(Root {
                        path: s.clone(),
                        project: default_project(s),
                    }),
                    Value::Mapping(m) => {
                        let path = m.get("path")?.as_str()?;
                        let project = m
                            .get("project")
                            .and_then(|p| p.as_str())
                            .map(|s| s.to_string())
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| default_project(path));
                        Some(Root {
                            path: path.to_string(),
                            project,
                        })
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Только пути корней (для walker — `WalkOptions::roots`).
///
/// С учётом режима агента (TR-4.1): при `agent.enabled` это каталоги
/// `<agent.root>\P<NNNN>\workspace`, а не `index.roots`.
pub fn roots_paths(cfg: &Config) -> Vec<String> {
    effective_roots(cfg).into_iter().map(|r| r.path).collect()
}

/// Проект файла: корень с наибольшим совпадающим префиксом (компонентно,
/// регистр не важен). Файл вне всех корней → `None` (в server-режиме не виден).
pub fn project_for_path<'a>(roots: &'a [Root], path: &str) -> Option<&'a str> {
    let pc = comps_lower(path);
    let mut best: Option<(usize, &Root)> = None;
    for r in roots {
        let rc = comps_lower(&r.path);
        if !rc.is_empty()
            && pc.len() >= rc.len()
            && pc[..rc.len()] == rc[..]
            && best.as_ref().is_none_or(|(n, _)| rc.len() > *n)
        {
            best = Some((rc.len(), r));
        }
    }
    best.map(|(_, r)| r.project.as_str())
}

/// Включён ли режим анонимной зоны (`agent.enabled`, D9/TR-4.1).
pub fn agent_enabled(cfg: &Config) -> bool {
    dig(cfg, "agent.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Корень анонимной зоны (`agent.root`): строка без пробелов по краям, если задан.
pub fn agent_root(cfg: &Config) -> Option<String> {
    dig(cfg, "agent.root")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Имя каталога проекта анонимной зоны: `P` + ровно 4 цифры (D21).
///
/// Именно по id, а не по имени проекта: имя проекта само по себе содержит
/// данные заказчика. Формат совпадает с маршрутом `/mcp/agent/<P>` (TR-4.3).
pub fn is_agent_project(name: &str) -> bool {
    match name.strip_prefix('P') {
        Some(rest) => rest.len() == 4 && rest.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// Корни анонимной зоны: `<agent.root>\P<NNNN>\workspace` (TR-4.1, §4.4).
///
/// Берутся только существующие каталоги `P<NNNN>` с непустым `workspace`:
/// `agent-config` и `agent-state` лежат рядом и корнями не становятся.
/// Порядок — по имени проекта (детерминированный обход и стабильные тесты).
pub fn agent_roots(cfg: &Config) -> Vec<Root> {
    let Some(root) = agent_root(cfg) else {
        return Vec::new();
    };
    let base = std::path::Path::new(&root);
    let mut projects: Vec<String> = match std::fs::read_dir(base) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .filter(|name| is_agent_project(name))
            .collect(),
        Err(_) => return Vec::new(),
    };
    projects.sort();
    let mut out = Vec::new();
    for project in projects {
        let ws = base.join(&project).join("workspace");
        if !ws.is_dir() {
            continue;
        }
        out.push(Root {
            path: ws.to_string_lossy().to_string(),
            project,
        });
    }
    out
}

/// Эффективные корни индексации: при `agent.enabled` — только
/// `<agent.root>\P<NNNN>\workspace` (TR-4.1), иначе `index.roots`.
///
/// Это единственная точка, где режим агента меняет состав корней: её
/// используют индексатор, watcher, UI и `index_status`.
pub fn effective_roots(cfg: &Config) -> Vec<Root> {
    if agent_enabled(cfg) {
        agent_roots(cfg)
    } else {
        roots_from_cfg(cfg)
    }
}
