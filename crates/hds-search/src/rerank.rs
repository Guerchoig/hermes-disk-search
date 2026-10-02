//! Клиент реранкера — порт `hds/rerank.py` (роль rerank, `/v1/rerank`).
//!
//! По умолчанию выключен (`rerank.enabled: false`); при недоступности или
//! превышении `rerank.max_latency` — деградация без падения (исходный порядок),
//! авто-отключение на время процесса после тайм-аута.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use hds_core::config::{dig, Config};
use hds_core::http;
use serde_json::json;

use crate::SearchResult;

static DISABLED: AtomicBool = AtomicBool::new(false);

/// Реранк `top_n` из `results`; `None` — реранкер выключен/недоступен/медленный.
pub fn rerank_results(
    cfg: &Config,
    query: &str,
    results: &[SearchResult],
    top_n: usize,
) -> Option<Vec<SearchResult>> {
    if DISABLED.load(Ordering::Relaxed) || results.is_empty() {
        return None;
    }
    let base = dig(cfg, "rerank.url")
        .and_then(|v| v.as_str())
        .unwrap_or("http://localhost:8012/v1")
        .trim_end_matches('/');
    let (host, port, prefix) = hds_index::embed::split_base(base).ok()?;
    let path = format!("{prefix}/rerank");
    let payload = json!({
        "model": dig(cfg, "rerank.model").and_then(|v| v.as_str()).unwrap_or("bge-reranker-v2-m3"),
        "query": query,
        "documents": results.iter().map(|r| r.text.clone()).collect::<Vec<_>>(),
    });
    let timeout = std::time::Duration::from_secs(
        dig(cfg, "rerank.timeout").and_then(|v| v.as_i64()).unwrap_or(30).max(1) as u64,
    );
    let t0 = Instant::now();
    let resp = match http::request(
        &host,
        port,
        "POST",
        &path,
        &[("Content-Type", "application/json")],
        Some(&payload.to_string()),
        timeout,
    ) {
        Ok(r) if r.status == 200 => r,
        Ok(r) => {
            eprintln!(
                "[rerank] реранкер ответил статусом {} — порядок без реранкинга; включите: rerank.enabled",
                r.status
            );
            return None;
        }
        Err(e) => {
            eprintln!(
                "[rerank] реранкер недоступен ({}) — порядок без реранкинга; включите: rerank.enabled",
                e.message()
            );
            return None;
        }
    };
    let elapsed = t0.elapsed().as_secs_f64();
    let max_lat = dig(cfg, "rerank.max_latency").and_then(|v| v.as_f64()).unwrap_or(15.0);
    if elapsed > max_lat {
        DISABLED.store(true, Ordering::Relaxed);
        eprintln!(
            "[rerank] латентность {elapsed:.1} с превышает лимит {max_lat:.1} с — реранкер авто-отключён до перезапуска процесса"
        );
        return None;
    }
    let data: serde_json::Value = resp.json().ok()?;
    let ranked = data
        .get("results")
        .or_else(|| data.get("data"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut order: Vec<(usize, f64)> = Vec::new();
    for item in &ranked {
        let idx = match item.get("index").and_then(|v| v.as_i64()) {
            Some(i) if i >= 0 => i as usize,
            _ => continue,
        };
        let score = item
            .get("relevance_score")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        order.push((idx, score));
    }
    if order.is_empty() {
        return Some(results.iter().take(top_n).cloned().collect());
    }
    order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::new();
    for (idx, _) in order {
        if idx < results.len() {
            out.push(results[idx].clone());
        }
        if out.len() >= top_n {
            break;
        }
    }
    if out.is_empty() {
        Some(results.iter().take(top_n).cloned().collect())
    } else {
        Some(out)
    }
}
