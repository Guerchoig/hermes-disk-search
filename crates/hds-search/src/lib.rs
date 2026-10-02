//! `hds-search` — гибридный поиск (`MIGRATION_PLAN_RUST.md` §4.1 W1): порт
//! `hds/search.py`. FTS5 (BM25) + `chunks_vec` (семантика) + CLIP по картинкам,
//! слияние **RRF**; сниппет с границами предложений.
//!
//! Зависимости только по HTTP/через воркер, как в конвейере: эмбеддинги — фасад
//! (`hds-index::Embedder`), лемматизация токенов — Python-воркер
//! (`hds_index::Lemmatizer`), CLIP — `hds_clip::shared`. Ошибки ветвей глушатся
//! (как Python): поиск не должен падать из-за недоступной роли.

pub mod fts;
pub mod rag;
pub mod rerank;
pub mod snippet;

use std::collections::HashMap;

use hds_core::config::{dig, Config};
use hds_index::{vector_blob, Embedder, Lemmatizer};
use rusqlite::Connection;

pub use fts::{find_tokens, fts_query, fts_search_ids, fts_tokens, lemmatize_token};
pub use rag::{ask, build_context, Answer};
pub use snippet::{format_location, make_snippet};

/// Результат поиска (поля — как словарь `search()`; `location` — `format_location`).
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: String,
    pub ext: String,
    pub kind: String,
    pub page: Option<i64>,
    pub t_start: Option<f64>,
    pub t_end: Option<f64>,
    pub text: String,
    pub snippet: String,
    pub score: f64,
}

impl SearchResult {
    /// Локация для отображения (`path (стр. N) [ЧЧ:ММ:СС]`).
    pub fn location(&self) -> String {
        format_location(&self.path, self.page, self.t_start)
    }

    /// JSON-представление словаря `search()` (без `location` — как Python).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "path": self.path,
            "ext": self.ext,
            "kind": self.kind,
            "page": self.page,
            "t_start": self.t_start,
            "t_end": self.t_end,
            "text": self.text,
            "snippet": self.snippet,
            "score": self.score,
        })
    }
}

/// Параметры RRF из конфига (`search.*`).
struct Rrf {
    vec_k: usize,
    fts_k: usize,
    rrf_k: i64,
    fts_w: f64,
    vec_w: f64,
    clip_w: f64,
    snippet_chars: usize,
}

impl Rrf {
    fn from_config(cfg: &Config) -> Rrf {
        Rrf {
            vec_k: dig(cfg, "search.vec_k").and_then(|v| v.as_i64()).unwrap_or(40) as usize,
            fts_k: dig(cfg, "search.fts_k").and_then(|v| v.as_i64()).unwrap_or(40) as usize,
            rrf_k: dig(cfg, "search.rrf_k").and_then(|v| v.as_i64()).unwrap_or(60),
            fts_w: dig(cfg, "search.fts_weight").and_then(|v| v.as_f64()).unwrap_or(1.0),
            vec_w: dig(cfg, "search.vec_weight").and_then(|v| v.as_f64()).unwrap_or(1.0),
            clip_w: dig(cfg, "search.clip_weight").and_then(|v| v.as_f64()).unwrap_or(1.0),
            snippet_chars: dig(cfg, "search.snippet_chars").and_then(|v| v.as_i64()).unwrap_or(500)
                as usize,
        }
    }
}

/// Накопитель RRF: балл по `chunk_id` + порядок первого появления (для стабильности).
struct Scores {
    map: HashMap<i64, f64>,
    order: Vec<i64>,
}

impl Scores {
    fn new() -> Self {
        Scores {
            map: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// Добавить вес ветки по rank (`w / (rrf_k + rank)`).
    fn add(&mut self, cid: i64, w: f64, rank: usize, rrf_k: i64) {
        let add = w / (rrf_k + rank as i64) as f64;
        if !self.map.contains_key(&cid) {
            self.order.push(cid);
        }
        *self.map.entry(cid).or_insert(0.0) += add;
    }
}

/// Гибридный поиск (порт `hds/search.py::search`).
///
/// `emb` — клиент эмбеддингов (фасад `:8011`); `lem` — лемматизатор токенов
/// (воркер). Возвращает до `limit` результатов; ошибки ветвей не прерывают поиск.
pub fn search(
    conn: &Connection,
    emb: Option<&Embedder>,
    lem: &dyn Lemmatizer,
    cfg: &Config,
    query: &str,
    kinds: Option<&[String]>,
    limit: usize,
) -> Vec<SearchResult> {
    let rrf = Rrf::from_config(cfg);
    let mut scores = Scores::new();

    // FTS-ветка
    for (rank, cid) in fts_search_ids(conn, lem, query, rrf.fts_k, kinds)
        .into_iter()
        .enumerate()
    {
        scores.add(cid, rrf.fts_w, rank, rrf.rrf_k);
    }

    // Семантическая ветка (эмбеддинги через фасад)
    if let Some(emb) = emb {
        if emb.is_available() {
            match emb.embed_query(query) {
                Ok(qv) => {
                    let blob = vector_blob(&qv);
                    let mut sql = String::from(
                        "SELECT v.rowid, v.distance FROM chunks_vec v \
                         JOIN chunks c ON c.id = v.rowid \
                         JOIN files f ON f.id = c.file_id \
                         WHERE v.embedding MATCH ? AND v.k = ?",
                    );
                    let mut params: Vec<Box<dyn rusqlite::ToSql>> =
                        vec![Box::new(blob), Box::new(rrf.vec_k as i64)];
                    if let Some(ks) = kinds {
                        if !ks.is_empty() {
                            sql += &format!(" AND f.kind IN ({})", vec!["?"; ks.len()].join(","));
                            for k in ks {
                                params.push(Box::new(k.clone()));
                            }
                        }
                    }
                    sql += " ORDER BY v.distance";
                    match query_ids(conn, &sql, params) {
                        Ok(rows) => {
                            for (rank, (cid, _d)) in rows.into_iter().enumerate() {
                                scores.add(cid, rrf.vec_w, rank, rrf.rrf_k);
                            }
                        }
                        Err(e) => eprintln!(
                            "[vec] семантический поиск недоступен ({e}); работает ключевой"
                        ),
                    }
                }
                Err(e) => eprintln!(
                    "[vec] семантический поиск недоступен ({}); работает ключевой",
                    e.message()
                ),
            }
        }
    }

    // CLIP: контентный поиск по картинкам (запрос на любом языке)
    let clip_wanted = kinds
        .map(|ks| ks.is_empty() || ks.iter().any(|k| k == "image"))
        .unwrap_or(true);
    let clip_on = dig(cfg, "index.clip").and_then(|v| v.as_bool()).unwrap_or(true);
    if clip_wanted && clip_on {
        if let Some(clip) = hds_clip::shared(cfg) {
            if let Ok(qv) = clip.embed_text(query) {
                let blob = vector_blob(&qv);
                let sql = "SELECT rowid, distance FROM images_vec WHERE embedding MATCH ? \
                           AND k = ? ORDER BY distance";
                let params: Vec<Box<dyn rusqlite::ToSql>> =
                    vec![Box::new(blob), Box::new(rrf.vec_k as i64)];
                match query_ids(conn, sql, params) {
                    Ok(rows) => {
                        let fids: Vec<i64> = rows.iter().map(|(f, _)| *f).collect();
                        let chmap = first_chunk_of_files(conn, &fids);
                        for (rank, (fid, _d)) in rows.into_iter().enumerate() {
                            if let Some(cid) = chmap.get(&fid) {
                                scores.add(*cid, rrf.clip_w, rank, rrf.rrf_k);
                            }
                        }
                    }
                    Err(e) => eprintln!("[clip] поиск по содержанию картинок недоступен ({e})"),
                }
            }
        }
    }


    if scores.map.is_empty() {
        return Vec::new();
    }
    // порядок: score по убыванию, стабильно (порядок первого появления — как dict)
    let mut items: Vec<(i64, f64)> = scores
        .order
        .iter()
        .map(|cid| (*cid, scores.map[cid]))
        .collect();
    items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut results = Vec::new();
    for (cid, sc) in items {
        let row = conn.query_row(
            "SELECT c.text, c.page, c.t_start, c.t_end, f.path, f.ext, f.kind \
             FROM chunks c JOIN files f ON f.id=c.file_id WHERE c.id=?1",
            [cid],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                    r.get::<_, Option<f64>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            },
        );
        let (text, page, t_start, t_end, path, ext, kind) = match row {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind = kind.unwrap_or_default();
        if let Some(ks) = kinds {
            if !ks.is_empty() && !ks.iter().any(|k| k == &kind) {
                continue;
            }
        }
        let snippet = make_snippet(&text, query, rrf.snippet_chars, lem);
        results.push(SearchResult {
            path,
            ext: ext.unwrap_or_default(),
            kind,
            page,
            t_start,
            t_end,
            text,
            snippet,
            score: round5(sc),
        });
        if results.len() >= limit {
            break;
        }
    }
    results
}


/// `round(x, 5)` (Python) — до 5 знаков.
fn round5(x: f64) -> f64 {
    (x * 1e5).round() / 1e5
}

/// Выполнить id-запрос `(i64, f64)`; ошибка — наружу.
fn query_ids(
    conn: &Connection,
    sql: &str,
    params: Vec<Box<dyn rusqlite::ToSql>>,
) -> rusqlite::Result<Vec<(i64, f64)>> {
    let mut st = conn.prepare(sql)?;
    let rows = st.query_map(
        rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())),
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<f64>>(1)?.unwrap_or(0.0))),
    )?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// `file_id → первый chunk_id` (один батч-запрос вместо N+1).
fn first_chunk_of_files(conn: &Connection, fids: &[i64]) -> HashMap<i64, i64> {
    let mut map = HashMap::new();
    if fids.is_empty() {
        return map;
    }
    let placeholders = vec!["?"; fids.len()].join(",");
    let sql =
        format!("SELECT id, file_id FROM chunks WHERE file_id IN ({placeholders}) ORDER BY id");
    let params: Vec<Box<dyn rusqlite::ToSql>> =
        fids.iter().map(|f| Box::new(*f) as Box<dyn rusqlite::ToSql>).collect();
    if let Ok(mut st) = conn.prepare(&sql) {
        if let Ok(rows) = st.query_map(
            rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())),
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        ) {
            for row in rows.flatten() {
                map.entry(row.1).or_insert(row.0);
            }
        }
    }
    map
}

