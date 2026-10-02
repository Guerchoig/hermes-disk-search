//! Редактирование `config.yaml` из UI с сохранением комментариев — порт
//! `hds/ui_server.py::_set_roots`/`_set_exclude_paths` (строчный редактор, без regex:
//! список под ключом `  <key>:` заменяется целиком, атомарная запись, проверка YAML).

use hds_core::config::{config_path, load, replace_file, Config};
use hds_core::dig;
use serde_json::{json, Value};

/// Текущие настройки для формы UI.
pub fn get_config() -> Value {
    let cfg = load().unwrap_or(Config::Null);
    let arr = |k: &str| {
        dig(&cfg, k)
            .and_then(|v| v.as_sequence())
            .map(|s| s.iter().filter_map(|x| x.as_str()).map(|x| x.to_string()).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    json!({
        "roots": arr("index.roots"),
        "exclude_paths": arr("index.exclude_paths"),
        "exclude_dirs": arr("index.exclude_dirs"),
    })
}

/// Значения списка `key` как строки (для проверки после записи).
fn list_of(cfg: &Config, key: &str) -> Option<Vec<String>> {
    dig(cfg, key)
        .and_then(|v| v.as_sequence())
        .map(|s| s.iter().filter_map(|x| x.as_str()).map(|x| x.to_string()).collect())
}

/// Заменить блок списка `key` в тексте, сохранив остальное (комментарии/соседние ключи).
fn replace_list(text: &str, key: &str, items: &[String]) -> Result<String, String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let header = format!("  {key}:");
    let h = lines
        .iter()
        .position(|l| l.starts_with(&header))
        .ok_or_else(|| format!("не найден ключ {key} в config.yaml"))?;

    let inline = lines[h]
        .split_once(':')
        .map(|(_, v)| !v.trim().is_empty())
        .unwrap_or(false);
    let mut end = h + 1;
    if !inline {
        while end < lines.len() && lines[end].trim_start().starts_with('-') {
            end += 1;
        }
    }

    let mut block = String::new();
    if items.is_empty() {
        block.push_str(&format!("  {key}: []\n"));
    } else {
        block.push_str(&format!("  {key}:\n"));
        for it in items {
            block.push_str(&format!("    - '{}'\n", it.replace('\'', "''")));
        }
    }
    let mut res = lines[..h].join("\n");
    res.push('\n');
    res.push_str(&block);
    res.push_str(&lines[end.min(lines.len())..].join("\n"));
    Ok(res)
}

/// Атомарная запись текста в `config.yaml` (temp + replace).
fn write_config(text: &str) -> Result<(), String> {
    let path = config_path();
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("запись {tmp:?}: {e}"))?;
    replace_file(&tmp, &path).map_err(|e| e.message())
}

fn load_text(text: &str) -> Result<Config, String> {
    let t = text.trim_start_matches('\u{feff}');
    serde_yaml::from_str(t).map_err(|e| e.to_string())
}

/// `index.roots` из UI (непустые строки, как есть — абсолютные пути диска).
pub fn set_roots(roots: &[String]) -> Value {
    let items: Vec<String> = roots
        .iter()
        .map(|r| r.trim().trim_matches('\'').trim_matches('"').to_string())
        .filter(|r| !r.is_empty())
        .collect();
    set_list("roots", &items)
}

/// `index.exclude_paths` из UI (нормализация: trim/кавычки, дедуп по lower, абсолютизация).
pub fn set_exclude_paths(paths: &[String]) -> Value {
    let mut norm: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in paths {
        let p = p.trim().trim_matches('"').trim_matches('\'').to_string();
        if p.is_empty() {
            continue;
        }
        let ap = if looks_abs(&p) {
            p
        } else {
            std::env::current_dir()
                .map(|c| c.join(&p).to_string_lossy().into_owned())
                .unwrap_or(p)
        };
        if seen.insert(ap.to_lowercase()) {
            norm.push(ap);
        }
    }
    set_list("exclude_paths", &norm)
}

/// Общий путь: заменить список `key`, проверить YAML и записать атомарно.
fn set_list(key: &str, items: &[String]) -> Value {
    let text = match std::fs::read_to_string(config_path()) {
        Ok(t) => t,
        Err(e) => return json!({ "ok": false, "msg": format!("чтение config.yaml: {e}") }),
    };
    let new_text = match replace_list(&text, key, items) {
        Ok(t) => t,
        Err(e) => return json!({ "ok": false, "msg": e }),
    };
    let dotted = format!("index.{key}");
    match load_text(&new_text) {
        Ok(c) if list_of(&c, &dotted).is_some() => {}
        Ok(_) => return json!({ "ok": false, "msg": format!("{dotted} не список в итоговом YAML") }),
        Err(e) => {
            return json!({ "ok": false, "msg": format!("итоговый config.yaml некорректен: {e}") })
        }
    }
    match write_config(&new_text) {
        Ok(()) => json!({ "ok": true, "items": items,
            "msg": format!("Сохранено ({key}): {}. Применяется к новым запускам — перезапустите watcher/индексацию.", items.len()) }),
        Err(e) => json!({ "ok": false, "msg": e }),
    }
}

/// Абсолютен ли путь (Windows `D:\`/`D:/`, UNC, POSIX).
fn looks_abs(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || p.starts_with('\\')
        || p.starts_with('/')
}

