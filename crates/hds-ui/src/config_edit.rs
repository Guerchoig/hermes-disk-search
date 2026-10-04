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
            .map(|s| {
                s.iter()
                    .filter_map(|x| x.as_str())
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    json!({
        "roots": arr("index.roots"),
        "exclude_paths": arr("index.exclude_paths"),
        "exclude_dirs": arr("index.exclude_dirs"),
        // §8.4: папки конвейера автотранскрибации — для формы на закладке «Транскрибация»
        "auto_transcribe": {
            "inbox_dir": scal(&cfg, "auto_transcribe.inbox_dir"),
            "out_dir": scal(&cfg, "auto_transcribe.out_dir"),
            "enabled": dig(&cfg, "auto_transcribe.enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            "source_disposal": scal(&cfg, "auto_transcribe.source_disposal"),
        },
    })
}

/// Скаляр конфига строкой (`""` — нет ключа/не строка).
fn scal(cfg: &Config, key: &str) -> String {
    dig(cfg, key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Заменить значение **скаляра** `key` внутри секции `section`, сохранив хвостовой
/// комментарий строки (строчный редактор без `regex` — как [`replace_list`]).
///
/// Комментарий — всё от первого `#` в строке; если `#` встречается внутри значения
/// (экзотика для путей), оно потеряется — та же «наивность», что у Python-редактора UI.
/// Если ключа в секции нет — он вставляется первой строкой секции.
fn replace_scalar(text: &str, section: &str, key: &str, value: &str) -> Result<String, String> {
    let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
    let head = format!("{section}:");
    let h = lines
        .iter()
        .position(|l| l == &head || l.starts_with(&head))
        .ok_or_else(|| format!("не найдена секция {section} в config.yaml"))?;
    // конец секции — первая строка без отступа (другая секция) или конец файла
    let mut end = h + 1;
    while end < lines.len() {
        let l = &lines[end];
        if !l.trim().is_empty() && !l.starts_with(' ') && !l.starts_with('\t') {
            break;
        }
        end += 1;
    }
    let quoted = format!("'{}'", value.replace('\'', "''"));
    let needle = format!("{key}:");
    for line in lines.iter_mut().take(end).skip(h + 1) {
        let indent_len = line.len() - line.trim_start().len();
        if !line[indent_len..].starts_with(&needle) {
            continue;
        }
        let indent = " ".repeat(indent_len);
        let comment = match line[indent_len..].split_once('#') {
            Some((_, c)) => format!("  #{c}"),
            None => String::new(),
        };
        *line = format!("{indent}{key}: {quoted}{comment}");
        return Ok(lines.join("\n"));
    }
    lines.insert(h + 1, format!("  {key}: {quoted}"));
    Ok(lines.join("\n"))
}

/// `POST /api/config/transcribe-dirs`: сохранить `inbox_dir`/`out_dir` (§8.4).
///
/// Пустые значения допустимы — каталоги можно заполнять по одному. Невалидная пара
/// (вложенные каталоги и т.п.) **не блокирует** сохранение: возвращаем `warn` с
/// текстом от `AutoTranscribeConfig::validate`.
pub fn set_transcribe_dirs(inbox: &str, out: &str) -> Value {
    let inbox = inbox.trim().to_string();
    let out = out.trim().to_string();
    let text = match std::fs::read_to_string(config_path()) {
        Ok(t) => t,
        Err(e) => return json!({ "ok": false, "msg": format!("чтение config.yaml: {e}") }),
    };
    let mut new_text = match replace_scalar(&text, "auto_transcribe", "inbox_dir", &inbox) {
        Ok(t) => t,
        Err(e) => return json!({ "ok": false, "msg": e }),
    };
    new_text = match replace_scalar(&new_text, "auto_transcribe", "out_dir", &out) {
        Ok(t) => t,
        Err(e) => return json!({ "ok": false, "msg": e }),
    };
    let cfg = match load_text(&new_text) {
        Ok(c) => c,
        Err(e) => {
            return json!({ "ok": false, "msg": format!("итоговый config.yaml некорректен: {e}") })
        }
    };
    if scal(&cfg, "auto_transcribe.inbox_dir") != inbox
        || scal(&cfg, "auto_transcribe.out_dir") != out
    {
        return json!({ "ok": false,
            "msg": "значения не применились — проверьте секцию auto_transcribe в config.yaml" });
    }
    let warn = match hds_index::autotranscribe::AutoTranscribeConfig::from_config(&cfg).validate() {
        Ok(()) => Value::Null,
        Err(e) => json!(e.message()),
    };
    match write_config(&new_text) {
        Ok(()) => json!({ "ok": true, "inbox_dir": inbox, "out_dir": out, "warn": warn,
            "msg": "Сохранено. Применяется к новым запускам демона (`hds transcribe-watch`)." }),
        Err(e) => json!({ "ok": false, "msg": e }),
    }
}

/// Значения списка `key` как строки (для проверки после записи).
fn list_of(cfg: &Config, key: &str) -> Option<Vec<String>> {
    dig(cfg, key).and_then(|v| v.as_sequence()).map(|s| {
        s.iter()
            .filter_map(|x| x.as_str())
            .map(|x| x.to_string())
            .collect()
    })
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
        Ok(_) => {
            return json!({ "ok": false, "msg": format!("{dotted} не список в итоговом YAML") })
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> String {
        "index:\n  roots:\n    - 'D:\\'\nauto_transcribe:\n  enabled: false\n  \
         inbox_dir: ''      # ВХОД\n  out_dir: 'D:\\old'\n\nsearch:\n  vec_k: 40\n"
            .to_string()
    }

    /// §8.4: значение скаляра меняется, соседние ключи и секции на месте;
    /// хвостовой комментарий сохраняется (нормализуется в два пробела перед `#`).
    #[test]
    fn replace_scalar_keeps_neighbours() {
        let out = replace_scalar(&text(), "auto_transcribe", "inbox_dir", "D:\\media_in").unwrap();
        assert!(
            out.contains("  inbox_dir: 'D:\\media_in'  # ВХОД"),
            "комментарий потерян: {out}"
        );
        assert!(
            out.contains("  enabled: false"),
            "соседний ключ пропал: {out}"
        );
        assert!(
            out.contains("  out_dir: 'D:\\old'"),
            "соседний ключ изменён: {out}"
        );
        assert!(
            out.contains("\nsearch:\n"),
            "следующая секция пострадала: {out}"
        );
        assert!(
            out.starts_with("index:\n"),
            "начало файла пострадало: {out}"
        );
    }

    /// Отсутствующий ключ вставляется первой строкой секции (не в конец файла).
    #[test]
    fn replace_scalar_inserts_missing_key() {
        let out = replace_scalar(&text(), "auto_transcribe", "whisper_gpu", "0").unwrap();
        assert!(
            out.contains("auto_transcribe:\n  whisper_gpu: '0'\n  enabled: false"),
            "{out}"
        );
    }

    /// Неизвестная секция — понятная ошибка, файл не портим.
    #[test]
    fn replace_scalar_requires_section() {
        assert!(
            replace_scalar("index:\n  roots: []\n", "auto_transcribe", "inbox_dir", "x").is_err()
        );
    }
}
