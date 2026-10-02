//! FTS-ветка поиска — порт `hds/search.py`: токены запроса, лемматизация тем же
//! способом, что и `chunks_fts` (через `Lemmatizer`), AND→OR с префиксом хвоста.

use hds_index::Lemmatizer;
use rusqlite::Connection;

/// `TOKEN_RE = [\w]{2,}` (Unicode): последовательности ≥2 «словных» символов.
pub fn find_tokens(text: &str) -> Vec<String> {
    fn flush(cur: &mut String, out: &mut Vec<String>) {
        if cur.chars().count() >= 2 {
            out.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            cur.push(ch);
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// `lemmatize_token`: лемма одного токена через воркер (`normalize` одного текста).
///
/// Python `normalize` — это `леmmatize_token`, применённый к каждому токену; для
/// одного токена результат — одна лемма (берём первое слово).
pub fn lemmatize_token(lem: &dyn Lemmatizer, token: &str) -> String {
    let tok = token.to_lowercase();
    match lem.normalize_many(std::slice::from_ref(&tok)) {
        Ok(mut v) if !v.is_empty() => v
            .pop()
            .unwrap_or_default()
            .split_whitespace()
            .next()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(tok),
        _ => tok,
    }
}

/// `_fts_tokens`: леммы первых 12 токенов запроса.
pub fn fts_tokens(lem: &dyn Lemmatizer, q: &str) -> Vec<String> {
    find_tokens(q)
        .into_iter()
        .take(12)
        .map(|t| lemmatize_token(lem, &t))
        .collect()
}

/// `_quoted`: `"token"` с удвоением кавычек внутри.
pub fn quoted(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect()
}

/// `fts_query`: OR-выражение по леммам (`None` — пустой запрос).
pub fn fts_query(lem: &dyn Lemmatizer, q: &str) -> Option<String> {
    let tokens = fts_tokens(lem, q);
    if tokens.is_empty() {
        return None;
    }
    Some(quoted(&tokens).join(" OR "))
}

/// `fts_search_ids`: топ-`k` `chunk_id` по FTS (AND, при пустом — OR; хвост с `*`).
pub fn fts_search_ids(
    conn: &Connection,
    lem: &dyn Lemmatizer,
    q: &str,
    k: usize,
    kinds: Option<&[String]>,
) -> Vec<i64> {
    let tokens = fts_tokens(lem, q);
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut quoted = quoted(&tokens);
    if let Some(last) = quoted.last_mut() {
        last.push('*'); // префиксный матчинг последнего токена
    }

    let (join, kind_sql) = match kinds {
        Some(ks) if !ks.is_empty() => (
            "JOIN chunks c ON c.id = cf.rowid JOIN files f ON f.id = c.file_id ",
            format!(" AND f.kind IN ({})", vec!["?"; ks.len()].join(",")),
        ),
        _ => ("", String::new()),
    };

    let run = |expr: &str| -> Vec<i64> {
        let sql = format!(
            "SELECT cf.rowid FROM chunks_fts cf {join}WHERE chunks_fts MATCH ?{kind_sql} \
             ORDER BY bm25(chunks_fts) LIMIT ?"
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(expr.to_string())];
        if let Some(ks) = kinds {
            for kk in ks {
                params.push(Box::new(kk.clone()));
            }
        }
        params.push(Box::new(k as i64));
        let mut st = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[fts] ошибка поиска: {e}");
                return Vec::new();
            }
        };
        let rows = st.query_map(
            rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())),
            |r| r.get::<_, i64>(0),
        );
        match rows {
            Ok(it) => it.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                eprintln!("[fts] ошибка поиска: {e}");
                Vec::new()
            }
        }
    };

    let mut ids = run(&quoted.join(" AND "));
    if ids.is_empty() && quoted.len() > 1 {
        ids = run(&quoted.join(" OR "));
    }
    ids
}
