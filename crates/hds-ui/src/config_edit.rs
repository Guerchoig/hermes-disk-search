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
        // R2.1: каталоги автотранскрибации, исключаемые из индексации неявно (рантайм).
        "implicit_excludes": hds_index::pipeline::implicit_excludes(&cfg),
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

/// `POST /api/config/anonymize-dirs`: выходной каталог анонимизации
/// (`PLAN_ANONYMIZER_SIDECAR.md` §2.5/§2.6).
///
/// Секции `anonymize` в `config.yaml` может не быть (карточка появилась позже
/// остальных секций) — тогда она дописывается в конец файла: без этого первое
/// сохранение из UI падало бы на «не найдена секция».
pub fn set_anonymize_out_dir(out: &str) -> Value {
    let out = out.trim().trim_matches('"').trim_matches('\'').to_string();
    if out.is_empty() {
        return json!({ "ok": false, "msg": "пустой выходной каталог" });
    }
    let text = match std::fs::read_to_string(config_path()) {
        Ok(t) => t,
        Err(e) => return json!({ "ok": false, "msg": format!("чтение config.yaml: {e}") }),
    };
    let has_section = text
        .split('\n')
        .any(|l| l == "anonymize:" || l.starts_with("anonymize:"));
    let new_text = if has_section {
        match replace_scalar(&text, "anonymize", "output_dir", &out) {
            Ok(t) => t,
            Err(e) => return json!({ "ok": false, "msg": e }),
        }
    } else {
        let mut t = text.clone();
        if !t.ends_with('\n') {
            t.push('\n');
        }
        t.push_str("\nanonymize:\n  output_dir: '");
        t.push_str(&out.replace('\'', "''"));
        t.push_str("'\n");
        t
    };
    let cfg = match load_text(&new_text) {
        Ok(c) => c,
        Err(e) => {
            return json!({ "ok": false, "msg": format!("итоговый config.yaml некорректен: {e}") })
        }
    };
    if scal(&cfg, "anonymize.output_dir") != out {
        return json!({ "ok": false,
            "msg": "значение не применилось — проверьте секцию anonymize в config.yaml" });
    }
    match write_config(&new_text) {
        Ok(()) => json!({ "ok": true, "out_dir": out,
            "msg": "Сохранено. Применяется к новым актам анонимизации." }),
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

/// Отступ строки в пробелах.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Конец диапазона детей строки-заголовка: первая непустая строка с отступом
/// не больше, чем у заголовка (то есть следующая секция/ключ того же уровня).
fn child_end(lines: &[String], header: usize) -> usize {
    let indent = indent_of(&lines[header]);
    let mut end = header + 1;
    while end < lines.len() {
        let line = &lines[end];
        if line.trim().is_empty() {
            end += 1;
            continue;
        }
        if indent_of(line) <= indent {
            break;
        }
        end += 1;
    }
    end
}

/// Прямой потомок `key:` на уровне `indent` в диапазоне `start..end`.
fn find_child(
    lines: &[String],
    start: usize,
    end: usize,
    key: &str,
    indent: usize,
) -> Option<usize> {
    let needle = format!("{key}:");
    (start..end)
        .find(|&i| indent_of(&lines[i]) == indent && lines[i].trim_start().starts_with(&needle))
}

/// Строки блока «ключ(и) → значение» с отступом `indent`.
fn block_lines(keys: &[&str], literal: &str, indent: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut pad = indent;
    for (i, key) in keys.iter().enumerate() {
        if i + 1 == keys.len() {
            out.push(format!("{}{key}: {literal}", " ".repeat(pad)));
        } else {
            out.push(format!("{}{key}:", " ".repeat(pad)));
            pad += 2;
        }
    }
    out
}

/// Расставить скаляр по пути `keys` внутри диапазона `start..end`.
///
/// Значение — готовый YAML-литерал (`true`, `120`, `'label'`): кавычки и тип
/// выбирает вызывающий, поэтому здесь нет ни разбора значений, ни регулярок.
/// Хвостовой комментарий строки сохраняется, отсутствующие подсекции
/// создаются.
fn set_nested(
    lines: &mut Vec<String>,
    start: usize,
    end: usize,
    keys: &[&str],
    literal: &str,
    indent: usize,
) -> Result<(), String> {
    let Some((key, rest)) = keys.split_first() else {
        return Err("пустой путь ключа config.yaml".to_string());
    };
    match find_child(lines, start, end, key, indent) {
        Some(index) => {
            if rest.is_empty() {
                let tail = &lines[index][indent..];
                let comment = match tail.split_once('#') {
                    Some((_, comment)) => format!("  #{comment}"),
                    None => String::new(),
                };
                lines[index] = format!("{}{key}: {literal}{comment}", " ".repeat(indent));
                return Ok(());
            }
            let end = child_end(lines, index);
            if end == index + 1 {
                // Подсекция без детей (`filter:` и всё) — вставляем блок.
                let block = block_lines(rest, literal, indent + 2);
                for (n, line) in block.into_iter().enumerate() {
                    lines.insert(index + 1 + n, line);
                }
                return Ok(());
            }
            set_nested(lines, index + 1, end, rest, literal, indent + 2)
        }
        None => {
            let mut at = end;
            // Не уезжаем за пустые строки в конце секции.
            while at > start && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            let block = block_lines(keys, literal, indent);
            for (n, line) in block.into_iter().enumerate() {
                lines.insert(at + n, line);
            }
            Ok(())
        }
    }
}

/// Строчный редактор секции: `values` — пары «путь через точку» → «YAML-литерал».
///
/// Комментарии и порядок остальных ключей сохраняются (тот же приём, что у
/// [`replace_scalar`]): меняется ровно строка нужного ключа, отсутствующий
/// ключ/подсекция вставляется. Нет самой секции — она дописывается в конец
/// файла.
pub fn apply_scalars(text: &str, section: &str, values: &[(&str, &str)]) -> Result<String, String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let head = format!("{section}:");
    if !lines.iter().any(|line| is_section_header(line, &head)) {
        if !lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(head.clone());
    }
    for (dotted, literal) in values {
        let keys: Vec<&str> = dotted.split('.').map(str::trim).collect();
        if keys.is_empty() || keys.iter().any(|key| key.is_empty()) {
            return Err(format!("пустой ключ в пути {dotted:?}"));
        }
        // Заголовок ищем на каждой итерации: вставка сдвигает номера строк.
        let header = lines
            .iter()
            .position(|line| is_section_header(line, &head))
            .ok_or_else(|| format!("не найдена секция {section} в config.yaml"))?;
        let end = child_end(&lines, header);
        set_nested(&mut lines, header + 1, end, &keys, literal, 2)?;
    }
    Ok(lines.join("\n"))
}

/// Строка — заголовок секции `head` (`agent:`, `agent: # комментарий`).
fn is_section_header(line: &str, head: &str) -> bool {
    line == head || line.starts_with(&format!("{head} ")) || line.starts_with(&format!("{head}#"))
}

/// Записать настройки `agent:` из панели (A5.6): правка и проверка YAML.
///
/// Проверка — не «файл разбирается», а «каждый путь получил ровно это
/// значение»: иначе опечатка в литерале тихо оставила бы панель с одним
/// значением, а конфиг с другим.
pub fn set_agent_values(values: &[(&str, &str)]) -> Result<(), String> {
    if values.is_empty() {
        return Ok(());
    }
    let path = config_path();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("чтение config.yaml: {e}"))?;
    let new_text = apply_scalars(&text, "agent", values)?;
    let parsed =
        load_text(&new_text).map_err(|e| format!("итоговый config.yaml некорректен: {e}"))?;
    for (dotted, literal) in values {
        let expected: Config = serde_yaml::from_str(literal)
            .map_err(|e| format!("значение {dotted} не разбирается: {e}"))?;
        let full = format!("agent.{dotted}");
        if dig(&parsed, &full) != Some(&expected) {
            return Err(format!(
                "agent.{dotted} не принял значение {literal} в итоговом config.yaml"
            ));
        }
    }
    write_config(&new_text)
}

/// Абсолютен ли путь (Windows `D:\`/`D:/`, UNC, POSIX).
pub(crate) fn looks_abs(p: &str) -> bool {
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

    /// A5.6: правка `agent:` сохраняет комментарии (хвостовые и отдельные
    /// строки) и не трогает соседние ключи/секции.
    #[test]
    fn agent_scalars_keep_comments_and_neighbours() {
        let src = "agent:\n  enabled: true          # зона включена\n  root: 'D:\\zone'\n  \
                   # filter.max_stale_sec: 300  # комментарий по умолчанию\n  \
                   ask_timeout_sec: 600   # хвост\n\nllm:\n  temperature: 0.2\n";
        let out = apply_scalars(
            src,
            "agent",
            &[
                ("ask_timeout_sec", "601"),
                ("filter.unknown", "'block'"),
                ("egress_log", "true"),
            ],
        )
        .unwrap();
        assert!(
            out.contains("  enabled: true          # зона включена"),
            "{out}"
        );
        assert!(out.contains("  ask_timeout_sec: 601  # хвост"), "{out}");
        assert!(
            out.contains("  # filter.max_stale_sec: 300  # комментарий по умолчанию"),
            "{out}"
        );
        assert!(out.contains("  filter:\n    unknown: 'block'"), "{out}");
        assert!(out.contains("  egress_log: true"), "{out}");
        assert!(out.contains("\nllm:\n  temperature: 0.2"), "{out}");
        // Итоговый файл валиден и значения видны `dig`.
        let cfg: Config = serde_yaml::from_str(&out).unwrap();
        assert_eq!(
            dig(&cfg, "agent.filter.unknown").and_then(|v| v.as_str()),
            Some("block")
        );
        assert_eq!(
            dig(&cfg, "agent.ask_timeout_sec").and_then(|v| v.as_u64()),
            Some(601)
        );
        assert_eq!(
            dig(&cfg, "agent.egress_log").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    /// A5.6: нет секции `agent:` — она дописывается; подсекция с детьми
    /// получает новый ключ, а не дубль заголовка.
    #[test]
    fn agent_scalars_create_section_and_subsections() {
        let out = apply_scalars(
            "index:\n  clip: false\n",
            "agent",
            &[("mcp.tavily.enabled", "false")],
        )
        .unwrap();
        assert!(
            out.contains("agent:\n  mcp:\n    tavily:\n      enabled: false"),
            "{out}"
        );
        let src = "agent:\n  filter:\n    max_stale_sec: 300\n";
        let out = apply_scalars(src, "agent", &[("filter.unknown", "'label'")]).unwrap();
        assert!(
            out.contains("  filter:\n    max_stale_sec: 300\n    unknown: 'label'"),
            "{out}"
        );
        assert_eq!(out.matches("filter:").count(), 1, "дубль подсекции: {out}");
    }
}
