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
pub fn roots_paths(cfg: &Config) -> Vec<String> {
    roots_from_cfg(cfg).into_iter().map(|r| r.path).collect()
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
            && best.as_ref().map_or(true, |(n, _)| rc.len() > *n)
        {
            best = Some((rc.len(), r));
        }
    }
    best.map(|(_, r)| r.project.as_str())
}
