//! Реестр проектов анонимной зоны — `data\agent\projects.json` (TR-8.1).
//!
//! Формат: `{ "projects": [ {id, name, path, provider, model, created} ] }`.
//! Файл лежит в зоне пользователя, путь папки проекта хранится здесь же —
//! по нему [`crate::deanon`] переводит пути workspace обратно в пути проекта.
//!
//! Запись атомарная: временный файл рядом + `hds_core::config::replace_file`
//! (тот же приём, что у `secrets.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use hds_core::config::{replace_file, Config};
use hds_core::roots::is_agent_project;

use crate::settings;

/// Провайдер по умолчанию (решение В-12): ключ проекта — в `secrets.json`.
pub const DEFAULT_PROVIDER: &str = "openrouter";

/// Модель по умолчанию (решение В-12).
pub const DEFAULT_MODEL: &str = "~deepseek/deepseek-flash-latest";

/// Проект анонимной зоны (TR-8.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    /// Unix-время создания, секунды.
    #[serde(default)]
    pub created: i64,
}

/// Содержимое `projects.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projects {
    #[serde(default)]
    pub projects: Vec<Project>,
}

impl Projects {
    /// Найти проект по идентификатору (регистр не важен, как у `is_agent_project`).
    pub fn get(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id.eq_ignore_ascii_case(id))
    }

    /// Добавить или заменить проект по идентификатору.
    pub fn upsert(&mut self, project: Project) {
        match self
            .projects
            .iter_mut()
            .find(|p| p.id.eq_ignore_ascii_case(&project.id))
        {
            Some(slot) => *slot = project,
            None => self.projects.push(project),
        }
    }

    /// Удалить проект; `true`, если запись была.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.projects.len();
        self.projects.retain(|p| !p.id.eq_ignore_ascii_case(id));
        self.projects.len() != before
    }
}

/// Загрузить `projects.json`; отсутствующий файл — пустой реестр.
pub fn load_at(path: &Path) -> Result<Projects, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Projects::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Сохранить реестр атомарно.
pub fn save_at(path: &Path, projects: &Projects) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(projects).map_err(|e| format!("projects.json: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    replace_file(&tmp, path).map_err(|e| e.message())?;
    Ok(())
}

/// Загрузить реестр по конфигу.
pub fn load(cfg: &Config) -> Result<Projects, String> {
    load_at(&settings::projects_file(cfg))
}

/// Сохранить реестр по конфигу.
pub fn save(cfg: &Config, projects: &Projects) -> Result<(), String> {
    save_at(&settings::projects_file(cfg), projects)
}

/// Найти проект по конфигу.
pub fn get(cfg: &Config, id: &str) -> Option<Project> {
    load(cfg).ok().and_then(|p| p.get(id).cloned())
}

/// Проверить запись проекта до записи в реестр.
fn validate(project: &Project) -> Result<(), String> {
    if !is_agent_project(&project.id) {
        return Err(format!(
            "идентификатор проекта должен быть P + 4 цифры (D21), получено {:?}",
            project.id
        ));
    }
    if project.name.trim().is_empty() {
        return Err("у проекта должно быть имя".to_string());
    }
    if project.path.trim().is_empty() {
        return Err("у проекта должен быть путь папки".to_string());
    }
    // `path` — папка проекта в зоне пользователя: по ней деанонимизатор
    // подставляет пути, поэтому относительный путь бессмысленен.
    if !Path::new(&project.path).is_absolute() {
        return Err(format!(
            "путь папки проекта должен быть абсолютным, получено {:?}",
            project.path
        ));
    }
    Ok(())
}

/// Добавить проект (с проверкой идентификатора D21 и уникальности пути).
pub fn add(cfg: &Config, project: Project) -> Result<(), String> {
    validate(&project)?;
    let mut projects = load(cfg)?;
    if let Some(existing) = projects.get(&project.id) {
        if !existing.path.eq_ignore_ascii_case(&project.path) {
            return Err(format!(
                "проект {} уже зарегистрирован с другой папкой",
                project.id
            ));
        }
    }
    projects.upsert(project);
    save(cfg, &projects)
}

/// Удалить проект из реестра.
pub fn remove(cfg: &Config, id: &str) -> Result<bool, String> {
    let mut projects = load(cfg)?;
    let removed = projects.remove(id);
    if removed {
        save(cfg, &projects)?;
    }
    Ok(removed)
}

/// Путь папки проекта, если проект зарегистрирован.
pub fn project_path(cfg: &Config, id: &str) -> Option<PathBuf> {
    get(cfg, id).map(|p| PathBuf::from(p.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hds-agent-projects-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn project(id: &str, path: &str) -> Project {
        Project {
            id: id.to_string(),
            name: format!("Проект {id}"),
            path: path.to_string(),
            provider: "openrouter".to_string(),
            model: "openai/gpt-4.1-mini".to_string(),
            created: 1_700_000_000,
        }
    }

    #[test]
    fn roundtrip_and_upsert() {
        let dir = tmpdir("roundtrip");
        let path = dir.join("projects.json");
        let mut projects = Projects::default();
        projects.upsert(project("P0001", r"C:\work\one"));
        save_at(&path, &projects).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.get("p0001").unwrap().name, "Проект P0001");

        projects.upsert(project("P0001", r"C:\work\one"));
        save_at(&path, &projects).unwrap();
        assert_eq!(load_at(&path).unwrap().projects.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_empty_registry() {
        let dir = tmpdir("missing");
        let projects = load_at(&dir.join("none.json")).unwrap();
        assert!(projects.projects.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_project() {
        let mut projects = Projects::default();
        projects.upsert(project("P0001", r"C:\work\one"));
        projects.upsert(project("P0002", r"C:\work\two"));
        assert!(projects.remove("P0001"));
        assert!(!projects.remove("P0001"));
        assert_eq!(projects.projects.len(), 1);
        assert_eq!(projects.projects[0].id, "P0002");
    }

    #[test]
    fn validate_requires_absolute_path_and_pnnnn() {
        let good = project("P0001", r"C:\work\one");
        assert!(validate(&good).is_ok());
        let mut relative = good.clone();
        relative.path = "work\\one".to_string();
        assert!(validate(&relative).unwrap_err().contains("абсолютн"));
        let mut bad_id = good.clone();
        bad_id.id = "P01".to_string();
        assert!(validate(&bad_id).is_err());
        let mut no_name = good.clone();
        no_name.name = "  ".to_string();
        assert!(validate(&no_name).is_err());
    }
}
