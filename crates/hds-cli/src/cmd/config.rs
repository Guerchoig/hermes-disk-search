//! `hds config init` — единственный генератор **серверного** `config.yaml`.
//!
//! Зачем (M8, релиз 0.20.6): Windows- и macOS-установщики собирали config.yaml
//! строками в двух независимых местах (`install_server.ps1::New-ServerConfig` и
//! `install_server.command::gen_config`), и эти копии разъезжались: серверный
//! конфиг генерируется с нуля и НЕ наследует desktop-шаблон `config.example.yaml`,
//! поэтому в нём не оказалось `index.exclude_dirs` — серверный стенд индексировал
//! `D:\$RECYCLE.BIN` (инцидент 0.20.5).
//!
//! Теперь вход один — шаблон `server_config.example.yaml` с плейсхолдерами `${NAME}`:
//!
//! * строка с `${NAME}` получает значение параметра (комментарии шаблона не
//!   трогаются — в них плейсхолдеры просто документированы);
//! * если параметр пуст, **строка удаляется** (а заголовок секции, оставшийся без
//!   детей, удаляется вместе с ней);
//! * результат **разбирается как YAML** до записи: битый шаблон или битое значение
//!   останавливают установку, а не оставляют полувалидный конфиг;
//! * `--merge-defaults` дописывает в **существующий** конфиг отсутствующие ключи
//!   из шаблона (`index.exclude_dirs`) — установщик по-прежнему никогда не
//!   перезаписывает конфиг стенда (строчная вставка, комментарии сохраняются).
//!
//! Установщики только вызывают эту подкоманду (`install_server.ps1`,
//! `install_server.command`), YAML руками больше не строят.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_yaml::Value;

/// Имя шаблона в корне поставки (`build_rust_release*.ps1|sh` кладут его рядом с
/// `config.example.yaml`; `install_app_version.ps1` копирует весь stage в версию).
pub const TEMPLATE_NAME: &str = "server_config.example.yaml";

/// Параметры `hds config init` (те же, что раньше пробрасывали установщики).
#[derive(Debug, Clone, Default)]
pub struct InitOpts {
    /// `--profile` — сейчас поддерживается только `server`.
    pub profile: Option<String>,
    /// `--template <путь>` (по умолчанию — [`find_template`]).
    pub template: Option<String>,
    /// `--roots "D:\;E:\Archive"` (разделители `;`/`,`).
    pub roots: Option<String>,
    /// `--exclude-paths` (префиксы путей; каталоги автотранскрибации добавятся сами).
    pub exclude_paths: Option<String>,
    /// `--parallel N` (слоты движка; `0`/нет — ключ не пишется).
    pub parallel: Option<i64>,
    /// `--gpu-device N` (взаимоисключим с `--gpu-preset`).
    pub gpu_device: Option<String>,
    /// `--gpu-preset NAME`.
    pub gpu_preset: Option<String>,
    pub mcp_host: Option<String>,
    pub mcp_port: Option<u16>,
    pub ui_host: Option<String>,
    pub ui_port: Option<u16>,
    /// `--access-enabled true|false` (Bearer/логин; профиль сам его не включает).
    pub access_enabled: Option<bool>,
    /// `--enable-transcribe` — `auto_transcribe.enabled: true`.
    pub enable_transcribe: bool,
    pub media_inbox: Option<String>,
    pub media_out: Option<String>,
    /// `--source-disposal delete|keep|move` (по умолчанию `delete`).
    pub source_disposal: Option<String>,
    /// `--path <config.yaml>` (по умолчанию — путь конфига ядра).
    pub path: Option<String>,
    /// `--force` — перезаписать существующий конфиг.
    pub force: bool,
    /// `--print` — напечатать рендер в stdout и ничего не писать (режим `-WhatIf`).
    pub print: bool,
    /// `--merge-defaults` — дописать недостающие ключи шаблона в существующий конфиг.
    pub merge_defaults: bool,
}

/// `hds config <sub> [opts]`; `sub` — `init`. Код: 0 — успех, 1 — ошибка
/// (шаблон/значение/запись), 2 — неверная команда.
pub fn cmd_config(sub: &str, opts: InitOpts) -> i32 {
    match sub {
        "init" => init(&opts),
        other => {
            eprintln!("config: неизвестная подкоманда '{other}' (init)");
            2
        }
    }
}

fn init(o: &InitOpts) -> i32 {
    if let Some(p) = o.profile.as_deref() {
        if p.trim() != "server" {
            eprintln!(
                "config init: профиль '{p}' не поддерживается (только server: desktop \
                 использует config.example.yaml)"
            );
            return 2;
        }
    }
    let tpl_path = match find_template(o.template.as_deref()) {
        Some(p) => p,
        None => {
            eprintln!(
                "config init: не найден шаблон {TEMPLATE_NAME} (ищите рядом с hds / в корне \
                 поставки или укажите --template <путь>)"
            );
            return 1;
        }
    };
    let template = match std::fs::read_to_string(&tpl_path) {
        Ok(t) => t.trim_start_matches('\u{feff}').to_string(),
        Err(e) => {
            eprintln!("config init: чтение {}: {e}", tpl_path.display());
            return 1;
        }
    };
    let rendered = match render_server_config(&template, o) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("config init: {e}");
            return 1;
        }
    };
    if o.print {
        print!("{rendered}");
        return 0;
    }
    let path = o
        .path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(hds_core::config::config_path);
    write_or_report(&path, &rendered, o)
}

/// Записать конфиг (если его нет или `--force`) и, при `--merge-defaults`,
/// дописать в него дефолты шаблона.
fn write_or_report(path: &Path, rendered: &str, o: &InitOpts) -> i32 {
    if !path.is_file() || o.force {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = write_atomic(path, rendered) {
            eprintln!("config init: запись {}: {e}", path.display());
            return 1;
        }
        println!("[ok] config created: {}", path.display());
    } else {
        println!("[ok] config exists (not overwritten): {}", path.display());
    }
    if o.merge_defaults {
        let defaults = match default_exclude_dirs(rendered) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("config init: {e}");
                return 1;
            }
        };
        let cur = std::fs::read_to_string(path).unwrap_or_default();
        let (text, outcome) = merge_defaults_into(&cur, &defaults);
        if outcome == MergeOutcome::Added {
            if let Err(e) = write_atomic(path, &text) {
                eprintln!("config init: запись {}: {e}", path.display());
                return 1;
            }
        }
        println!(
            "[ok] index.exclude_dirs: {} names ({})",
            defaults.len(),
            outcome.as_str()
        );
    }
    0
}

/// Шаблон серверного конфига: явный `--template` → `HDS_SERVER_CONFIG_TEMPLATE` →
/// рядом с исполняемым файлом / на уровень выше (`bin\hds.exe`, `app\<ver>`) →
/// корень проекта.
pub fn find_template(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(t) = explicit {
        let p = PathBuf::from(t);
        return p.is_file().then_some(p);
    }
    if let Some(v) = std::env::var_os("HDS_SERVER_CONFIG_TEMPLATE") {
        if !v.is_empty() {
            let p = PathBuf::from(v);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cands.push(dir.join(TEMPLATE_NAME));
            if let Some(up) = dir.parent() {
                cands.push(up.join(TEMPLATE_NAME));
            }
        }
    }
    cands.push(hds_core::config::project_root().join(TEMPLATE_NAME));
    cands.into_iter().find(|p| p.is_file())
}

/// Атомарная запись: временный файл рядом + [`hds_core::config::replace_file`]
/// (на Windows читатель конфига держит файл без `FILE_SHARE_DELETE` — нужны ретраи).
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text.as_bytes())?;
    hds_core::config::replace_file(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        std::io::Error::other(e.message())
    })
}

/// Чем кончился merge дефолтов в существующий конфиг.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Ключа не было — строку дописали.
    Added,
    /// Ключ уже есть (не трогаем: у стенда может быть свой список).
    AlreadyPresent,
}

impl MergeOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeOutcome::Added => "added",
            MergeOutcome::AlreadyPresent => "already present",
        }
    }
}

/// Дописать `index.exclude_dirs` в **существующий** конфиг (строчно, комментарии и
/// остальные ключи сохраняются). Идемпотентно; `index:` создаётся, если его нет.
///
/// Почему строчно, а не через `serde_yaml`: конфиг стенда — рабочий документ с
/// комментариями администратора, пересборка из `Value` их бы стёрла.
pub fn merge_defaults_into(text: &str, defaults: &[String]) -> (String, MergeOutcome) {
    if text
        .lines()
        .any(|l| l.trim_start().starts_with("exclude_dirs:"))
    {
        return (text.to_string(), MergeOutcome::AlreadyPresent);
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let ended = text.ends_with('\n');
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if ended {
        lines.pop();
    }
    let value = format!("  exclude_dirs: {}", yaml_flow_list(defaults));
    match lines.iter().position(|l| l.trim_end() == "index:") {
        Some(i) => lines.insert(i + 1, value),
        None => {
            if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("index:".to_string());
            lines.push(value);
        }
    }
    let mut out = lines.join(eol);
    if ended {
        out.push_str(eol);
    }
    (out, MergeOutcome::Added)
}

/// Дефолтные `index.exclude_dirs` из шаблона (для `--merge-defaults`).
///
/// Читается **только строка** `exclude_dirs:` (inline-список), а не весь файл:
/// шаблон содержит плейсхолдеры `${…}` и не обязан разбираться как YAML целиком.
pub fn default_exclude_dirs(template: &str) -> Result<Vec<String>, String> {
    let line = template
        .lines()
        .find(|l| l.trim_start().starts_with("exclude_dirs:"))
        .ok_or_else(|| {
            "шаблон: нет index.exclude_dirs — серверный стенд снова начнёт индексировать \
             $RECYCLE.BIN / System Volume Information"
                .to_string()
        })?;
    let raw = line.trim_start().trim_start_matches("exclude_dirs:").trim();
    if raw.is_empty() {
        return Err("шаблон: index.exclude_dirs должен быть inline-списком в одной строке".into());
    }
    let v: Value = serde_yaml::from_str(raw)
        .map_err(|e| format!("шаблон: index.exclude_dirs не разбирается: {e}"))?;
    let seq = v
        .as_sequence()
        .ok_or_else(|| "шаблон: index.exclude_dirs должен быть списком".to_string())?;
    let mut out = Vec::with_capacity(seq.len());
    for item in seq {
        let s = item
            .as_str()
            .ok_or_else(|| "шаблон: index.exclude_dirs — только строки".to_string())?;
        out.push(s.to_string());
    }
    if out.is_empty() {
        return Err("шаблон: index.exclude_dirs пуст".into());
    }
    Ok(out)
}

/// Отрендерить серверный конфиг из шаблона (чистая функция — покрыта тестами).
///
/// Правила — в шапке модуля. Перед возвратом результат **разбирается как YAML**:
/// ошибка шаблона/значения видна сразу, до записи файла.
pub fn render_server_config(template: &str, o: &InitOpts) -> Result<String, String> {
    let values = build_values(o)?;
    let mut out: Vec<String> = Vec::new();
    for line in template.lines() {
        // Комментарии шаблона — документация (в том числе список плейсхолдеров
        // `${NAME}` в шапке): подстановка идёт только по значениям YAML.
        if line.trim_start().starts_with('#') {
            out.push(line.to_string());
            continue;
        }
        // параметр пуст → строки нет
        if let Some(l) = substitute(line, &values)? {
            out.push(l);
        }
    }
    drop_empty_headers(&mut out);
    collapse_blank(&mut out);
    let text = format!("{}\n", out.join("\n"));
    serde_yaml::from_str::<Value>(&text)
        .map_err(|e| format!("шаблон после подстановки не является валидным YAML: {e}"))?;
    Ok(text)
}

/// Значения плейсхолдеров: `None` — параметр не задан (строку шаблона удалить).
fn build_values(o: &InitOpts) -> Result<BTreeMap<&'static str, Option<String>>, String> {
    let mut m: BTreeMap<&'static str, Option<String>> = BTreeMap::new();
    let access = o.access_enabled.unwrap_or(false);
    m.insert(
        "ACCESS_ENABLED",
        Some(if access { "true" } else { "false" }.into()),
    );

    let parallel = match o.parallel {
        // `0`/отрицательное — «не задано» (установщик передаёт 0, когда флага нет)
        Some(n) if n <= 0 => None,
        Some(n) if n > 64 => return Err(format!("--parallel {n}: ожидается 1..=64")),
        other => other,
    };
    m.insert("PARALLEL", parallel.map(|n| n.to_string()));

    let preset = o
        .gpu_preset
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let device = o
        .gpu_device
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if device.is_some() && preset.is_some() {
        return Err("--gpu-device и --gpu-preset взаимоисключимы".into());
    }
    if let Some(d) = device {
        if !d.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("--gpu-device '{d}': ожидается номер устройства"));
        }
    }
    // шаблон оборачивает значения в '…' сам: подставляем только экранированное
    m.insert("GPU_PRESET", preset.map(escape_single));
    m.insert("GPU_DEVICE", device.map(str::to_string));

    let roots = split_list(o.roots.as_deref());
    m.insert("ROOTS", (!roots.is_empty()).then(|| yaml_flow_list(&roots)));

    let mut excl = split_list(o.exclude_paths.as_deref());
    for dir in [&o.media_inbox, &o.media_out].into_iter().flatten() {
        let d = dir.trim();
        if !d.is_empty() && !excl.iter().any(|x| x == d) {
            excl.push(d.to_string());
        }
    }
    m.insert(
        "EXCLUDE_PATHS",
        (!excl.is_empty()).then(|| yaml_flow_list(&excl)),
    );

    m.insert(
        "TRANSCRIBE_ENABLED",
        Some(if o.enable_transcribe { "true" } else { "false" }.into()),
    );
    for (key, val) in [("MEDIA_INBOX", &o.media_inbox), ("MEDIA_OUT", &o.media_out)] {
        m.insert(
            key,
            val.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(escape_single),
        );
    }

    let disposal = o
        .source_disposal
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("delete");
    if !matches!(disposal, "delete" | "keep" | "move") {
        return Err(format!(
            "--source-disposal '{disposal}': ожидается delete|keep|move"
        ));
    }
    m.insert("SOURCE_DISPOSAL", Some(disposal.to_string()));

    m.insert(
        "MCP_HOST",
        Some(escape_single(o.mcp_host.as_deref().unwrap_or("127.0.0.1"))),
    );
    m.insert("MCP_PORT", Some(o.mcp_port.unwrap_or(8787).to_string()));
    m.insert(
        "UI_HOST",
        Some(escape_single(o.ui_host.as_deref().unwrap_or("127.0.0.1"))),
    );
    m.insert("UI_PORT", Some(o.ui_port.unwrap_or(8765).to_string()));
    Ok(m)
}

/// Подставить плейсхолдеры в строку шаблона.
///
/// `Ok(None)` — в строке есть плейсхолдер с пустым значением: строку надо удалить.
fn substitute(
    line: &str,
    values: &BTreeMap<&'static str, Option<String>>,
) -> Result<Option<String>, String> {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    let mut any = false;
    let mut empty = false;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("шаблон: незакрытый плейсхолдер (строка: {})", line.trim()))?;
        let name = &after[..end];
        let slot = values.get(name).ok_or_else(|| {
            format!(
                "шаблон: неизвестный плейсхолдер ${{{name}}} (строка: {})",
                line.trim()
            )
        })?;
        match slot {
            Some(v) => {
                out.push_str(v);
                any = true;
            }
            None => empty = true,
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    if empty {
        return Ok(None);
    }
    if !any {
        return Ok(Some(line.to_string()));
    }
    Ok(Some(out))
}

/// Экранирование значения для YAML-скаляра в одинарных кавычках (кавычки кладёт шаблон).
fn escape_single(s: &str) -> String {
    s.replace('\'', "''")
}

/// YAML flow-последовательность: `['a', 'b']`.
fn yaml_flow_list(items: &[String]) -> String {
    let body: Vec<String> = items
        .iter()
        .map(|s| format!("'{}'", escape_single(s)))
        .collect();
    format!("[{}]", body.join(", "))
}

/// `"D:\;E:\Archive"` → `["D:\", "E:\Archive"]` (пустые элементы отбрасываются).
fn split_list(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split([';', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Отступ строки-заголовка секции (`foo:` без значения), иначе `None`.
fn header_indent(line: &str) -> Option<usize> {
    let trimmed = line.trim_end();
    let body = trimmed.trim_start();
    if body.is_empty() || body.starts_with('#') {
        return None;
    }
    let (key, rest) = body.split_once(':')?;
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
    {
        return None;
    }
    let rest = rest.trim();
    if !rest.is_empty() && !rest.starts_with('#') {
        return None;
    }
    Some(trimmed.len() - body.len())
}

/// Удалить заголовки секций, у которых не осталось детей (после удаления строк с
/// пустыми параметрами). Повторяется до устойчивого состояния — вложенность.
fn drop_empty_headers(lines: &mut Vec<String>) {
    loop {
        let victim = lines.iter().enumerate().find_map(|(i, line)| {
            let ind = header_indent(line)?;
            let has_child = lines[i + 1..]
                .iter()
                .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .is_some_and(|l| indent_of(l) > ind);
            (!has_child).then_some(i)
        });
        match victim {
            Some(i) => {
                lines.remove(i);
            }
            None => break,
        }
    }
}

/// Схлопнуть повторные пустые строки и убрать пустые в начале/конце.
fn collapse_blank(lines: &mut Vec<String>) {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines.drain(..) {
        if line.trim().is_empty() {
            if out.last().is_some_and(|l: &String| l.trim().is_empty()) {
                continue;
            }
            out.push(String::new());
        } else {
            out.push(line);
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    *lines = out;
}
