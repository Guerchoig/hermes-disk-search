//! Задача UI «Транскрибация» (`PLAN_AUTO_TRANSCRIBE` §8): список файлов `out_dir`,
//! чтение превью, детект меток спикеров и **подстановка имён** на месте (+`.bak`).
//!
//! Правила из плана:
//! * **имя файла разрешается строго внутри `out_dir`** (§8.3): запрещены `..`,
//!   абсолютные пути, разделители каталогов, `:`/устройства; финальная проверка —
//!   через `canonicalize` (уход по репарс-точкам тоже исключается);
//! * метки спикеров распознаёт **общий рукописный сканер** `hds_index::detect_speaker_slots`
//!   (§5.4 — без `regex`), поэтому UI и демон видят одни и те же токены;
//! * подстановка **частичная**: пустое имя или совпадающее с меткой — пропускается;
//! * запись **атомарная** (temp + `replace_file`), перед ней — копия `.bak`
//!   (одна, перезаписываемая);
//! * sidecar `<stem>.speakers.json` обновляется (`{placeholder: name}`), поэтому
//!   повторное открытие окна подставляет уже сохранённые имена (§8.3).
//!
//! Изменяется **только строка-заголовок реплики** (`### SPEAKER_NN [ … ]`) — упоминания
//! метки в тексте не трогаем (то же решение, что у сканера).

use std::path::{Path, PathBuf};

use hds_core::config::{dig, load, project_root, replace_file, Config};
use hds_index::autotranscribe::{self, AutoTranscribeConfig, FileMeta};
use hds_index::transcribe::{detect_speaker_slots, DEFAULT_OUT_FORMAT, DEFAULT_UNASSIGNED_LABEL};
use serde_json::{json, Value};

/// Предел чтения файла в превью (защита от гигантских выходов).
const MAX_PREVIEW_BYTES: u64 = 8 << 20;

/// Каталог вывода `auto_transcribe.out_dir` (относительный — от корня проекта).
fn out_dir(cfg: &Config) -> Option<PathBuf> {
    let raw = dig(cfg, "auto_transcribe.out_dir").and_then(|v| v.as_str())?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let p = PathBuf::from(raw);
    Some(if p.is_absolute() {
        p
    } else {
        project_root().join(p)
    })
}

/// Формат выхода (`auto_transcribe.out_format`, по умолчанию `md`).
fn out_format(cfg: &Config) -> String {
    dig(cfg, "auto_transcribe.out_format")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().trim_start_matches('.').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_OUT_FORMAT.to_string())
}

/// Метка неприсвоенной реплики (`auto_transcribe.unassigned_label`).
fn unassigned_label(cfg: &Config) -> String {
    dig(cfg, "auto_transcribe.unassigned_label")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_UNASSIGNED_LABEL.to_string())
}

/// Проверка «имени» из UI: это **имя файла**, а не путь (§8.3, path-safety).
///
/// Отклоняем: пустое, с разделителями, с `..`, с `:`/устройствами, начинающееся с
/// `/` или `\` (абсолютное/UNC) и содержащее управляющие символы.
pub fn is_safe_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    if name == "." || name == ".." || name.contains("..") {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return false;
    }
    if name.starts_with('~') {
        return false;
    }
    if name.chars().any(|c| (c as u32) < 0x20) {
        return false;
    }
    true
}

/// Разрешить имя в путь внутри `out_dir` (и убедиться, что файл там и лежит).
///
/// Двойная защита: лексическая (имя не содержит пути) + фактическая
/// (`canonicalize` обеих сторон и проверка префикса — симлинки/джанкшены не уведут).
pub fn resolve_out_file(cfg: &Config, name: &str) -> Result<PathBuf, String> {
    if !is_safe_name(name) {
        return Err("недопустимое имя файла".to_string());
    }
    let dir = out_dir(cfg).ok_or_else(|| "не задан auto_transcribe.out_dir".to_string())?;
    let cand = dir.join(name);
    if !cand.is_file() {
        return Err(format!("файл не найден в out_dir: {name}"));
    }
    let dir_real = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let cand_real = std::fs::canonicalize(&cand).map_err(|e| format!("{}: {e}", cand.display()))?;
    if !cand_real.starts_with(&dir_real) {
        return Err("путь вне out_dir".to_string());
    }
    Ok(cand)
}

/// Каталог сервисных файлов (sidecar, `.orig`, `.bak`): `auto_transcribe.state_dir`
/// (по умолчанию `data/auto-transcribe`) — чтобы в `out_dir` оставался только выход.
fn service_dir(cfg: &Config) -> PathBuf {
    let raw = dig(cfg, "auto_transcribe.state_dir")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "data/auto-transcribe".to_string());
    let p = PathBuf::from(&raw);
    if p.is_absolute() {
        p
    } else {
        project_root().join(p)
    }
}

/// Имя sidecar по стему выхода (`клип.md` → `клип.speakers.json`).
fn sidecar_name(file: &Path) -> String {
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    format!("{stem}.speakers.json")
}

/// Путь sidecar для **чтения**: в каталоге сервисных файлов, а если его там ещё
/// нет — рядом с выходом (совместимость с прогонами до переноса сервисных файлов).
pub fn sidecar_of(cfg: &Config, file: &Path) -> PathBuf {
    let name = sidecar_name(file);
    let svc = service_dir(cfg).join(&name);
    if svc.exists() {
        return svc;
    }
    file.with_file_name(name)
}

/// Прочитать карту спикеров из sidecar (`{}` — если нет/битый).
fn read_sidecar(cfg: &Config, file: &Path) -> Value {
    std::fs::read_to_string(sidecar_of(cfg, file))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or_else(|| json!({}))
}

/// Присвоены ли имена (в карте sidecar есть хотя бы одно непустое значение).
fn is_renamed(sidecar: &Value) -> bool {
    sidecar
        .get("speakers")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.values()
                .any(|v| v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false))
        })
        .unwrap_or(false)
}

/// Метки спикеров файла: сканируем **текст** (он авторитетнее — в `speaker_spans`
/// движка записи `UNASSIGNED` может не быть, спайк T0.1 §3); sidecar — фолбэк.
fn speakers_of_file(file: &Path, sidecar: &Value) -> Vec<String> {
    if let Ok(md) = std::fs::metadata(file) {
        if md.len() <= MAX_PREVIEW_BYTES {
            if let Ok(text) = std::fs::read_to_string(file) {
                return detect_speaker_slots(&text);
            }
        }
    }
    sidecar
        .get("speakers")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// `GET /api/transcribe/list`: файлы `out_dir` (выходы; sidecar/`.orig`/`.bak` скрыты).
pub fn list_json() -> Value {
    let cfg = load().unwrap_or(Config::Null);
    let Some(dir) = out_dir(&cfg) else {
        return json!({ "dir": Value::Null, "files": [],
            "error": "не задан auto_transcribe.out_dir (см. config.yaml)" });
    };
    if !dir.is_dir() {
        return json!({ "dir": dir.display().to_string(), "files": [],
            "error": "каталог out_dir не найден (создастся при первом задании)" });
    }
    let fmt = out_format(&cfg);
    let mut files: Vec<Value> = Vec::new();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // выходы — только `<stem>.<fmt>`; `.orig.<fmt>`, sidecar и `.bak` не показываем
        if name.contains(".orig.") || name.ends_with(".bak") {
            continue;
        }
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != fmt {
            continue;
        }
        let md = e.metadata().ok();
        let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = md
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let sidecar = read_sidecar(&cfg, &p);
        let speakers = speakers_of_file(&p, &sidecar);
        files.push(json!({
            "name": name,
            "size": size,
            "mtime": mtime,
            "speakers": speakers,
            "renamed": is_renamed(&sidecar),
        }));
    }
    // свежие сверху
    files.sort_by(|a, b| {
        b["mtime"]
            .as_f64()
            .partial_cmp(&a["mtime"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    json!({
        "dir": dir.display().to_string(),
        "format": fmt,
        "unassigned_label": unassigned_label(&cfg),
        "files": files,
    })
}

/// `GET /api/transcribe/file?name=…`: текст выхода + метки спикеров + уже
/// сохранённые имена из sidecar (окно открывается с прошлыми значениями).
pub fn file_json(name: &str) -> Value {
    let cfg = load().unwrap_or(Config::Null);
    let path = match resolve_out_file(&cfg, name) {
        Ok(p) => p,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let meta = std::fs::metadata(&path).ok();
    if meta
        .as_ref()
        .map(|m| m.len() > MAX_PREVIEW_BYTES)
        .unwrap_or(false)
    {
        return json!({ "ok": false, "error": "файл слишком большой для превью" });
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return json!({ "ok": false, "error": format!("чтение: {e}") }),
    };
    let sidecar = read_sidecar(&cfg, &path);
    // Список для окна имён = найденные в тексте метки + сохранённые в sidecar:
    // после подстановки имён меток в тексте уже нет, и без sidecar окно «теряло» бы
    // строки (повторное открытие должно показывать уже присвоенные имена, §8.3).
    let mut speakers = detect_speaker_slots(&text);
    if let Some(m) = sidecar.get("speakers").and_then(|v| v.as_object()) {
        for k in m.keys() {
            if !speakers.iter().any(|s| s == k) {
                speakers.push(k.clone());
            }
        }
    }
    json!({
        "ok": true,
        "name": name,
        "path": path.display().to_string(),
        "text": text,
        "speakers": speakers,
        "saved": sidecar.get("speakers").cloned().unwrap_or_else(|| json!({})),
        "renamed": is_renamed(&sidecar),
        "unassigned_label": unassigned_label(&cfg),
    })
}

/// Подставить имена в **заголовки реплик** (`### SPEAKER_NN [ … ]`).
///
/// Рукописно, без `regex`: идём по строкам, у строки-заголовка берём первый токен
/// (до пробела) и, если он есть в карте, заменяем его. Упоминания метки в тексте
/// реплики не трогаем — как и сканер. Возвращает `(текст, число замен)`.
pub fn replace_speaker_labels(text: &str, map: &[(String, String)]) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut replaced = 0usize;
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let indent = line.len() - line.trim_start().len();
        let rest = &line[indent..];
        let Some(after_hash) = rest.strip_prefix("###") else {
            out.push_str(line);
            continue;
        };
        let after_ws = after_hash.trim_start();
        let ws = after_hash.len() - after_ws.len();
        let token_len = after_ws.find(char::is_whitespace).unwrap_or(after_ws.len());
        let token = &after_ws[..token_len];
        match map
            .iter()
            .find(|(k, v)| k == token && !v.trim().is_empty() && v != token)
        {
            Some((_, name)) => {
                let token_start = indent + 3 + ws;
                out.push_str(&line[..token_start]);
                out.push_str(name);
                out.push_str(&line[token_start + token_len..]);
                replaced += 1;
            }
            None => out.push_str(line),
        }
    }
    (out, replaced)
}

/// Атомарная запись текста: временный файл рядом + `replace_file` (ретраи на Windows).
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("dat");
    let tmp = path.with_extension(format!("{ext}.tmp"));
    std::fs::write(&tmp, text).map_err(|e| format!("запись {}: {e}", tmp.display()))?;
    replace_file(&tmp, path).map_err(|e| e.message())
}

/// Обновить sidecar: мержим переданные имена в существующую карту (§8.3).
/// Пишем в каталог сервисных файлов (`state_dir`), рядом с выходом не сорим.
fn update_sidecar(
    cfg: &Config,
    path: &Path,
    name: &str,
    map: &[(String, String)],
) -> Result<(), String> {
    let side = read_sidecar(cfg, path);
    let mut obj = side
        .get("speakers")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    for (k, v) in map {
        obj.insert(k.clone(), json!(v));
    }
    let out = json!({
        "speakers": Value::Object(obj),
        "unassigned_label": unassigned_label(cfg),
        "source": side.get("source").cloned().unwrap_or(Value::Null),
        "out": path.display().to_string(),
        "file": name,
    });
    let text = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
    let side_path = service_dir(cfg).join(sidecar_name(path));
    if let Some(dir) = side_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    write_atomic(&side_path, &text)
}

/// Пары замен «что ищем → на что меняем» для подстановки имён (§8.3).
///
/// * `saved` — карта sidecar (`{speakers:{placeholder:name}}`) — что уже подставлено;
/// * `names` — запрос из UI (`{placeholder: name}`).
///
/// Источник замены — **прошлое имя**, если оно уже подставлялось, иначе метка:
/// вторая правка («Иван» → «Пётр») должна найти в тексте «Иван», а не `SPEAKER_00`
/// (метки там уже нет). Пустые имена и пары «не изменилось» отбрасываются — это и
/// есть частичная подстановка («подставляются только заполненные»).
pub fn rename_pairs(saved: &Value, names: &Value) -> Vec<(String, String)> {
    let prev_of = |placeholder: &str| {
        saved
            .get("speakers")
            .and_then(|v| v.get(placeholder))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    names
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.trim().to_string())))
                .filter(|(k, v)| !v.is_empty() && v != k)
                .map(|(k, v)| {
                    let prev = prev_of(&k);
                    let from = if prev.is_empty() { k } else { prev };
                    (from, v)
                })
                .filter(|(from, to)| from != to)
                .collect()
        })
        .unwrap_or_default()
}

/// `POST /api/transcribe/speakers` (тело `{name, names:{"SPEAKER_00":"Иван"}}`).
///
/// Подставляются **только непустые** имена, отличные от самой метки (§8.3 —
/// «частичное заполнение»). Перед правкой — копия `.bak` (одна, перезаписываемая).
pub fn speakers_json(name: &str, names: &Value) -> Value {
    let cfg = load().unwrap_or(Config::Null);
    let path = match resolve_out_file(&cfg, name) {
        Ok(p) => p,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let map: Vec<(String, String)> = names
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.trim().to_string())))
                .filter(|(k, v)| !v.is_empty() && v != k)
                .collect()
        })
        .unwrap_or_default();
    if map.is_empty() {
        return json!({ "ok": false, "error": "нет непустых имён для подстановки" });
    }
    let sidecar = read_sidecar(&cfg, &path);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return json!({ "ok": false, "error": format!("чтение: {e}") }),
    };
    let pairs = rename_pairs(&sidecar, names);
    if pairs.is_empty() {
        return json!({ "ok": false, "error": "нет изменений: имена уже такие" });
    }
    let (new_text, replaced) = replace_speaker_labels(&text, &pairs);
    if replaced == 0 {
        return json!({ "ok": false, "error": "метки не найдены в заголовках реплик" });
    }
    let bak = service_dir(&cfg).join(format!("{name}.bak"));
    if let Some(dir) = bak.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return json!({ "ok": false, "error": format!("каталог сервисных файлов: {e}") });
        }
    }
    if let Err(e) = std::fs::write(&bak, &text) {
        return json!({ "ok": false, "error": format!("копия .bak: {e}") });
    }
    if let Err(e) = write_atomic(&path, &new_text) {
        return json!({ "ok": false, "error": e });
    }
    match update_sidecar(&cfg, &path, name, &map) {
        Ok(()) => json!({ "ok": true, "replaced": replaced, "saved": true,
            "msg": format!("Применено замен: {replaced}. Копия: {}", bak.display()) }),
        Err(e) => json!({ "ok": true, "replaced": replaced, "saved": false,
            "error": e, "msg": format!("Имена подставлены ({replaced}), но sidecar не обновлён: {e}") }),
    }
}

/// Исходник задания по стему выхода: медиафайл с тем же стемом в `inbox_dir`
/// (обход — по `auto_transcribe.recursive`).
fn find_source(at: &AutoTranscribeConfig, stem: &str) -> Option<PathBuf> {
    let inbox = at.inbox_dir.as_ref()?;
    let depth = if at.recursive { usize::MAX } else { 1 };
    let mut found: Vec<PathBuf> = walkdir::WalkDir::new(inbox)
        .max_depth(depth)
        .into_iter()
        .flatten()
        .map(|e| e.path().to_path_buf())
        .filter(|p| {
            p.is_file()
                && p.file_stem().and_then(|s| s.to_str()) == Some(stem)
                && hds_index::kinds::kind_of_path(p) == Some("media")
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

/// `POST /api/transcribe/apply`: поставить задание по файлу **заново** (§8.3).
///
/// Исходник ищем в `inbox_dir` по стему выхода. После успешной обработки исходник
/// удаляется (`source_disposal: delete`), поэтому «перезапустить» можно только пока
/// файл на месте — об этом честно сообщаем, а не делаем вид, что задание принято.
///
/// Задание кладём в **журнал очереди** (демон подхватывает его периодическим
/// merge'ом, ≤ `poll_seconds`): UI не ждёт транскрибации минутами и не держит HTTP.
pub fn apply_json(name: &str) -> Value {
    let cfg = load().unwrap_or(Config::Null);
    let path = match resolve_out_file(&cfg, name) {
        Ok(p) => p,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let at = AutoTranscribeConfig::from_config(&cfg);
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return json!({ "ok": false, "error": "не разобрать имя файла" });
    };
    let Some(src) = find_source(&at, stem) else {
        return json!({ "ok": false,
            "error": "исходник не найден в inbox_dir: после успеха он удаляется \
                      (`source_disposal`). Положите медиа во входную папку заново." });
    };
    let Some(meta) = FileMeta::of(&src) else {
        return json!({ "ok": false, "error": "не прочитать исходник" });
    };
    if let Err(e) = autotranscribe::append_queue(
        &at.queue_file,
        &autotranscribe::job_for(&src, meta, "pending"),
    ) {
        return json!({ "ok": false, "error": format!("журнал очереди: {}", e.message()) });
    }
    let daemon = project_root().join("transcribe.lock").exists();
    json!({
        "ok": true,
        "queued": src.display().to_string(),
        "daemon": daemon,
        "msg": if daemon {
            format!("Задание поставлено: {}. Демон подхватит в течение {} с",
                    src.display(), at.poll_seconds.max(1))
        } else {
            format!("Задание поставлено: {}. Демон не запущен (`hds transcribe-watch`) — \
                     заберёт его при старте", src.display())
        },
    })
}

/// Статус демона `hds transcribe-watch` (для кнопок на закладке «Транскрибация»).
///
/// «Запущен» определяется по `transcribe.lock` (демон держит его RAII-файлом);
/// «застрял» (`stale`) — lock остался от умершего процесса (его снимет следующий старт).
pub fn daemon_status() -> Value {
    let cfg = load().unwrap_or(Config::Null);
    let at = AutoTranscribeConfig::from_config(&cfg);
    let root = project_root();
    let lock = root.join("transcribe.lock");
    let stale = lock.exists() && hds_index::watch::lock_is_stale(&lock);
    let running = lock.exists() && !stale;
    let pid = std::fs::read_to_string(&lock)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|_| running);
    json!({
        "running": running,
        "stale": stale,
        "pid": pid,
        "lock": lock.display().to_string(),
        "stop_requested": root.join("transcribe.stop").exists(),
        "enabled": at.enabled,
        "inbox_dir": at.inbox_dir.as_ref().map(|p| p.display().to_string()),
        "out_dir": at.out_dir.as_ref().map(|p| p.display().to_string()),
        "poll_seconds": at.poll_seconds,
    })
}

/// Управление демоном из UI: `start` | `stop` | `restart` | `status`.
///
/// Демон запускается **detached** (без окна), как резидент в `/api/llm-host/restart`;
/// вывод демона отбрасывается (в UI журнала нет — смотреть `data/auto-transcribe*`).
/// Дубль невозможен: демон сам держит `transcribe.lock`; устаревший lock снимается.
pub fn daemon_json(action: &str) -> Value {
    let root = project_root();
    let lock = root.join("transcribe.lock");
    let stop = root.join("transcribe.stop");
    match action.trim() {
        "" | "status" => daemon_status(),
        "stop" => {
            if !daemon_status()["running"].as_bool().unwrap_or(false) {
                let _ = std::fs::remove_file(&stop);
                return json!({ "ok": true, "msg": "демон не запущен", "status": daemon_status() });
            }
            if let Err(e) = std::fs::write(&stop, b"") {
                return json!({ "ok": false, "msg": format!("не создать {}: {e}", stop.display()) });
            }
            if !crate::wait_file(&lock, false, 20) {
                return json!({ "ok": false,
                    "msg": "демон не остановился за 20 с — смотрите `data/auto-transcribe*` \
                            или повторите",
                    "status": daemon_status() });
            }
            json!({ "ok": true, "msg": "демон остановлен", "status": daemon_status() })
        }
        "start" => {
            let st = daemon_status();
            if st["running"].as_bool().unwrap_or(false) {
                return json!({ "ok": false, "msg": "демон уже запущен", "status": st });
            }
            let _ = std::fs::remove_file(&lock); // устаревший lock не мешает старту
            let _ = std::fs::remove_file(&stop);
            let Some(exe) = crate::hds_exe() else {
                return json!({ "ok": false,
                    "msg": "не найден бинарь hds (рядом с UI / bin / target/{release,debug})" });
            };
            if let Err(e) = crate::spawn_detached(&exe, &["transcribe-watch"], &root) {
                return json!({ "ok": false, "msg": format!("не запустить {}: {e}", exe.display()) });
            }
            if !crate::wait_file(&lock, true, 15) {
                let hint = if st["enabled"].as_bool().unwrap_or(false) {
                    "проверьте auto_transcribe.inbox_dir/out_dir (обе папки обязательны)"
                } else {
                    "auto_transcribe.enabled: false — демон выходит сразу, включите его в config.yaml"
                };
                return json!({ "ok": false,
                    "msg": format!("демон не занял transcribe.lock за 15 с: {hint}"),
                    "status": daemon_status() });
            }
            let mut out = json!({ "ok": true, "msg": "демон запущен", "status": daemon_status() });
            if !st["enabled"].as_bool().unwrap_or(false) {
                out["warn"] = json!(
                    "auto_transcribe.enabled: false — демон завершится сам; включите в config.yaml"
                );
            }
            out
        }
        "restart" => {
            let stopped = daemon_json("stop");
            if stopped["ok"].as_bool() != Some(true) {
                return stopped;
            }
            let mut res = daemon_json("start");
            res["msg"] = json!(format!(
                "перезапуск: {}",
                res["msg"].as_str().unwrap_or("готово")
            ));
            res
        }
        other => json!({ "ok": false,
            "msg": format!("неизвестное действие: {other} (start|stop|restart|status)") }),
    }
}

// Управление демоном использует общие хелперы `crate::{hds_exe, spawn_detached, wait_file}`
// (они же обслуживают резидент в `/api/llm-host/restart` и демон индексации).

#[cfg(test)]
mod tests {
    use super::*;

    /// §8.3: имя — это **имя файла**, любые «пути» отклоняются (path-safety).
    #[test]
    fn unsafe_names_are_rejected() {
        for ok in ["клип.md", "встреча-2.md", "a.b.md", "文件.md"] {
            assert!(is_safe_name(ok), "должно приниматься: {ok}");
        }
        for bad in [
            "",
            "..",
            "../secret.md",
            "..\\secret.md",
            "sub/файл.md",
            "sub\\файл.md",
            "C:\\Windows\\system32\\x.md",
            "D:/out/x.md",
            "C:secret.md",
            "/etc/passwd",
            "\\\\server\\share\\x.md",
            "x.md:stream",
            "~/x.md",
            "bad\nname.md",
            "~$x.md",
        ] {
            assert!(!is_safe_name(bad), "должно отклоняться: {bad:?}");
        }
        assert!(!is_safe_name(&"я".repeat(300)));
    }

    /// §5.4/§8.3: подстановка идёт **только в заголовках** и только для заполненных
    /// строк; пустые и «имя == метка» пропускаются (частичное заполнение).
    #[test]
    fn replace_labels_partial_and_header_only() {
        let text = "### SPEAKER_00 [00:00:01 - 00:00:02]\nПривет, SPEAKER_01 — это текст.\n\n\
                    ### SPEAKER_01 [00:00:03 - 00:00:04]\nОтвет.\n\n\
                    ### UNASSIGNED [00:00:05 - 00:00:06]\nХвост.\n";
        let map = vec![
            ("SPEAKER_00".to_string(), "Иван".to_string()),
            ("SPEAKER_01".to_string(), "".to_string()), // пусто → пропуск
            ("UNASSIGNED".to_string(), "UNASSIGNED".to_string()), // совпадает → пропуск
        ];
        let (out, n) = replace_speaker_labels(text, &map);
        assert_eq!(n, 1, "заменена только одна метка");
        assert!(out.starts_with("### Иван [00:00:01 - 00:00:02]"), "{out}");
        assert!(
            out.contains("### SPEAKER_01 [00:00:03 - 00:00:04]"),
            "пустое имя не подставляем: {out}"
        );
        assert!(
            out.contains("Привет, SPEAKER_01 — это текст."),
            "упоминание в теле реплики не трогаем: {out}"
        );
        assert!(
            out.contains("### UNASSIGNED [00:00:05"),
            "метка осталась: {out}"
        );
        assert_eq!(
            out.lines().count(),
            text.lines().count(),
            "число строк не меняется"
        );
    }

    /// Заменяются все вхождения метки; счётчик точно совпадает (идемпотентности
    /// повторного прогона не нарушает — после подстановки метки уже нет).
    #[test]
    fn replace_labels_counts_all_occurrences_and_is_idempotent() {
        let text =
            "### SPEAKER_00 [00:00:01 - 00:00:02]\nA\n\n### SPEAKER_00 [00:00:03 - 00:00:04]\nB\n";
        let map = vec![("SPEAKER_00".to_string(), "Мария".to_string())];
        let (out, n) = replace_speaker_labels(text, &map);
        assert_eq!(n, 2);
        assert_eq!(out.matches("### Мария").count(), 2);
        let (again, n2) = replace_speaker_labels(&out, &map);
        assert_eq!(
            (n2, again),
            (0, out),
            "повторная подстановка ничего не делает"
        );
    }

    /// Имя sidecar по стему выхода (`клип.md` → `клип.speakers.json`); чтение — из
    /// каталога сервисных файлов, а если его там нет — рядом с выходом (старые прогоны).
    /// Путь собирается через `join`, чтобы тест шёл и на Windows, и на Linux
    /// (строка `D:\out\…` на Linux — одна компонента без разделителей).
    #[test]
    fn sidecar_name_follows_stem() {
        let file = Path::new("D:").join("out").join("клип.md");
        assert_eq!(sidecar_name(&file), "клип.speakers.json");
        let cfg = Config::Null;
        assert_eq!(
            sidecar_of(&cfg, &file),
            Path::new("D:").join("out").join("клип.speakers.json"),
            "sidecar в service_dir не найден → читаем рядом с выходом"
        );
    }

    /// §8.3: первая правка — метка→имя; повторная — **прошлое имя**→новое;
    /// пустые и «не изменилось» отбрасываются (частичное заполнение).
    #[test]
    fn rename_pairs_first_repeat_and_partial() {
        let no_sidecar = json!({});
        let names = json!({
            "SPEAKER_00": "Иван",
            "SPEAKER_01": "",
            "UNASSIGNED": "UNASSIGNED",
        });
        assert_eq!(
            rename_pairs(&no_sidecar, &names),
            vec![("SPEAKER_00".to_string(), "Иван".to_string())],
            "первый раз: метка→имя; пустое и совпадающее с меткой — пропущены"
        );

        let saved = json!({ "speakers": { "SPEAKER_00": "Иван", "UNASSIGNED": "" } });
        assert!(
            rename_pairs(&saved, &json!({ "SPEAKER_00": "Иван" })).is_empty(),
            "то же имя — работы нет"
        );
        assert_eq!(
            rename_pairs(&saved, &json!({ "SPEAKER_00": "Пётр" })),
            vec![("Иван".to_string(), "Пётр".to_string())],
            "вторая правка меняет прошлое имя, а не метку"
        );
    }
}
