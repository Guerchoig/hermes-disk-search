//! Дерево папок индексации — порт `hds/ui_server.py::_build_trees`: статус каталога
//! `done`/`partial`/`none` (свёртка по потомкам), глубина ≤ 4, лимит детей 40.
//!
//! Источники: (а) обход корней `index.roots` (файлы известных видов + mtime);
//! (б) БД (`files.path/status/indexed_at`) — сколько проиндексировано и когда.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use hds_core::config::{dig, load};
use serde_json::{json, Value};

const MAX_DEPTH: usize = 4;
const CHILD_LIMIT: usize = 40;

fn level(s: &str) -> u8 {
    match s {
        "done" => 2,
        "partial" => 1,
        _ => 0,
    }
}

fn level_name(l: u8) -> &'static str {
    ["none", "partial", "done"][l as usize]
}

/// Узел дерева.
struct Node {
    name: String,
    path: String,
    status: String,
    files: i64,
    indexed: i64,
    children: Vec<Node>,
}

impl Node {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name, "path": self.path, "status": self.status,
            "files": self.files, "indexed": self.indexed,
            "children": self.children.iter().map(|c| c.to_json()).collect::<Vec<_>>(),
        })
    }
}

/// Обход корня: `<dir> -> (файлов, max mtime)` для файлов «известных» видов.
/// Останавливается по `deadline` (возвращает `true`, если обход прерван).
fn disk_index(root: &Path, excl: &HashSet<String>, deadline: std::time::Instant) -> (BTreeMap<PathBuf, (i64, f64)>, bool) {
    let mut out: BTreeMap<PathBuf, (i64, f64)> = BTreeMap::new();
    let walk = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
        !e.file_type().is_dir() || !excl.contains(&e.file_name().to_string_lossy().to_lowercase())
    });
    for entry in walk.flatten() {
        if std::time::Instant::now() >= deadline {
            return (out, true);
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();
        if hds_index::kinds::kind_of(&ext).is_none() {
            continue;
        }
        let dir = entry.path().parent().map(Path::to_path_buf).unwrap_or_default();
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let e = out.entry(dir).or_insert((0, 0.0));
        e.0 += 1;
        if mtime > e.1 {
            e.1 = mtime;
        }
    }
    (out, false)
}

/// Из БД: `<dir> -> (всего файлов, проиндексировано, max indexed_at)`.
fn db_info(cfg: &hds_core::config::Config) -> HashMap<PathBuf, (i64, i64, f64)> {
    let mut info: HashMap<PathBuf, (i64, i64, f64)> = HashMap::new();
    if let Ok(conn) = crate::connect(cfg) {
        if let Ok(mut st) = conn.prepare("SELECT path, status, indexed_at FROM files") {
            if let Ok(rows) = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                ))
            }) {
                for row in rows.flatten() {
                    let dir = Path::new(&row.0).parent().map(Path::to_path_buf).unwrap_or_default();
                    let e = info.entry(dir).or_insert((0, 0, 0.0));
                    e.0 += 1;
                    if row.1.as_deref() == Some("indexed") {
                        e.1 += 1;
                    }
                    if let Some(iat) = row.2 {
                        if iat > e.2 {
                            e.2 = iat;
                        }
                    }
                }
            }
        }
    }
    info
}

/// Статус каталога (при walk): `tot==0 -> none`; `mtime > iat+2 -> partial`; `idx>=files -> done`; иначе partial.
#[allow(dead_code)]
fn status_of(
    disk: &HashMap<PathBuf, (i64, f64)>,
    dbinfo: &HashMap<PathBuf, (i64, i64, f64)>,
) -> HashMap<PathBuf, (String, i64, i64)> {
    let mut out = HashMap::new();
    let mut all: HashSet<PathBuf> = disk.keys().cloned().collect();
    all.extend(dbinfo.keys().cloned());
    for d in all {
        let (dn, mtime) = disk.get(&d).copied().unwrap_or((0, 0.0));
        let (tot, idx, iat) = dbinfo.get(&d).copied().unwrap_or((0, 0, 0.0));
        if dn == 0 && tot == 0 {
            continue;
        }
        let st = if tot == 0 {
            "none"
        } else if dn > 0 && mtime > iat + 2.0 {
            "partial"
        } else if idx >= dn {
            "done"
        } else {
            "partial"
        };
        out.insert(d, (st.to_string(), dn, idx));
    }
    out
}


/// Рекурсивно собрать `Node` из карт узлов/детей (дети — по имени).
fn materialize(
    path: &Path,
    nodes: &HashMap<PathBuf, Node>,
    children: &HashMap<PathBuf, Vec<PathBuf>>,
) -> Option<Node> {
    let base = nodes.get(path)?;
    let mut kids = children.get(path).cloned().unwrap_or_default();
    kids.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    let child_nodes = kids.iter().filter_map(|p| materialize(p, nodes, children)).collect();
    Some(Node {
        name: base.name.clone(),
        path: base.path.clone(),
        status: base.status.clone(),
        files: base.files,
        indexed: base.indexed,
        children: child_nodes,
    })
}

/// Свёртка статуса родителя по потомкам (порт `aggregate`).
fn aggregate(n: &mut Node) {
    for c in n.children.iter_mut() {
        aggregate(c);
    }
    if !n.children.is_empty() {
        let lvls: HashSet<u8> = n.children.iter().map(|c| level(&c.status)).collect();
        n.status = if lvls.len() == 1 {
            level_name(*lvls.iter().next().unwrap()).to_string()
        } else {
            "partial".to_string()
        };
    }
}

/// Обрезка списка детей (порт `trim`).
fn trim(n: &mut Node) {
    if n.children.len() > CHILD_LIMIT {
        let rest: Vec<Node> = n.children.drain(CHILD_LIMIT..).collect();
        let lvl = rest.iter().map(|c| level(&c.status)).min().unwrap_or(0);
        n.children.push(Node {
            name: format!("… ещё {} папок", rest.len()),
            path: String::new(),
            status: level_name(lvl).to_string(),
            files: 0,
            indexed: 0,
            children: Vec::new(),
        });
    }
    for c in n.children.iter_mut() {
        trim(c);
    }
}

/// Построить дерево одного корня (глубина отображения ≤ `MAX_DEPTH`).
fn build_root(root: &Path, st: &HashMap<PathBuf, (String, i64, i64)>) -> Option<Node> {
    let root_depth = root.components().count();
    if !st.keys().any(|d| d.starts_with(root)) && !st.contains_key(root) {
        return None;
    }
    let mut nodes: HashMap<PathBuf, Node> = HashMap::new();
    let mut children: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mk = |d: &Path, is_root: bool| {
        let info = st.get(d);
        Node {
            name: if is_root {
                d.to_string_lossy().into_owned()
            } else {
                d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            },
            path: d.to_string_lossy().into_owned(),
            status: info.map(|v| v.0.clone()).unwrap_or_else(|| "none".to_string()),
            files: info.map(|v| v.1).unwrap_or(0),
            indexed: info.map(|v| v.2).unwrap_or(0),
            children: Vec::new(),
        }
    };
    nodes.insert(root.to_path_buf(), mk(root, true));
    // все нужные пути (каталоги + их предки до корня) — по одному разу
    let mut needed: HashSet<PathBuf> = HashSet::new();
    for d in st.keys().filter(|d| d.starts_with(root) && *d != root) {
        if d.components().count().saturating_sub(root_depth) > MAX_DEPTH {
            continue;
        }
        let mut cur = d.clone();
        while cur != *root {
            needed.insert(cur.clone());
            cur = match cur.parent() {
                Some(p) if p.starts_with(root) => p.to_path_buf(),
                _ => break,
            };
        }
    }
    for p in &needed {
        nodes.entry(p.clone()).or_insert_with(|| mk(p, false));
    }
    for p in &needed {
        if let Some(par) = p.parent() {
            children.entry(par.to_path_buf()).or_default().push(p.clone());
        }
    }
    let mut node = materialize(root, &nodes, &children)?;
    aggregate(&mut node);
    trim(&mut node);
    Some(node)
}

/// `/api/tree`. `walk` — обходить диск (медленно на огромных корнях; по умолчанию
/// дерево строится по БД — быстро и достаточно для навигации).
pub fn tree_json(walk: bool) -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            return json!({ "error": e.message(), "trees": [], "dirs": 0, "disk_files": 0, "truncated": false })
        }
    };
    let excl: HashSet<String> = dig(&cfg, "index.exclude_dirs")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().filter_map(|x| x.as_str()).map(|s| s.to_lowercase()).collect())
        .unwrap_or_default();
    let roots: Vec<PathBuf> = dig(&cfg, "index.roots")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|x| x.as_str())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();

    let mut disk: HashMap<PathBuf, (i64, f64)> = HashMap::new();
    let mut truncated = false;
    if walk {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        for root in &roots {
            if root.is_dir() {
                let (m, tr) = disk_index(root, &excl, deadline);
                truncated |= tr;
                for (d, v) in m {
                    let e = disk.entry(d).or_insert((0, 0.0));
                    e.0 += v.0;
                    if v.1 > e.1 {
                        e.1 = v.1;
                    }
                }
            }
        }
    }
    let dbinfo = db_info(&cfg);
    // статус из БД: idx>=tot -> done, иначе partial; (при walk — ещё none/partial по диску)
    let mut st: HashMap<PathBuf, (String, i64, i64)> = HashMap::new();
    for (d, (tot, idx, iat)) in &dbinfo {
        if *tot == 0 {
            continue;
        }
        let (dn, mtime) = disk.get(d).copied().unwrap_or((0, 0.0));
        let status = if walk && dn > 0 && mtime > iat + 2.0 {
            "partial"
        } else if idx >= tot {
            "done"
        } else {
            "partial"
        };
        st.insert(d.clone(), (status.to_string(), if walk { dn } else { *tot }, *idx));
    }
    if walk {
        for (d, (dn, _m)) in &disk {
            if !st.contains_key(d) {
                st.insert(d.clone(), ("none".to_string(), *dn, 0));
            }
        }
    }
    let trees: Vec<Value> = roots
        .iter()
        .filter_map(|r| build_root(r, &st).map(|n| n.to_json()))
        .collect();
    json!({
        "trees": trees,
        "dirs": st.len(),
        "disk_files": disk.values().map(|v| v.0).sum::<i64>(),
        "truncated": truncated,
    })
}

