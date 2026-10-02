//! Сниппет и локация результата — порт `hds/search.py::make_snippet`/`format_location`.
//!
//! Позиции — в **символах** (Python `str` = code points), поэтому работаем через
//! `Vec<char>`; нижний регистр для поиска токена (как `.lower()`).

use hds_index::Lemmatizer;

use crate::fts::{find_tokens, lemmatize_token};

/// Найти подстроку `needle` (в виде символов) в `hay`, начиная с 0; индекс в символах.
fn find_sub(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// Последний «конец предложения» в `[from, to)` (`[.!?…]\s` или `\n`), как `finditer`.
fn last_sent_end(chars: &[char], from: usize, to: usize) -> Option<usize> {
    let to = to.min(chars.len());
    let mut last = None;
    let mut i = from.min(to);
    while i < to {
        let c = chars[i];
        if c == '\n' {
            last = Some(i + 1);
        } else if matches!(c, '.' | '!' | '?' | '…') && i + 1 < to && chars[i + 1].is_whitespace() {
            last = Some(i + 2);
        }
        i += 1;
    }
    last
}

/// `make_snippet`: окно вокруг первого вхождения токена запроса, с границами предложений.
pub fn make_snippet(text: &str, q: &str, max_len: usize, lem: &dyn Lemmatizer) -> String {
    let tokens: Vec<String> = find_tokens(q)
        .into_iter()
        .take(20)
        .map(|t| t.to_lowercase())
        .collect();
    let textchars: Vec<char> = text.chars().collect();
    let lowerchars: Vec<char> = text.to_lowercase().chars().collect();
    // защита: если длины разошлись (редкие случаи `lower()`), ищем по исходным
    let hay: &[char] = if lowerchars.len() == textchars.len() {
        &lowerchars
    } else {
        &textchars
    };

    let mut pos: i64 = -1;
    for t in &tokens {
        let needle: Vec<char> = t.chars().collect();
        if let Some(p) = find_sub(hay, &needle) {
            if pos == -1 || (p as i64) < pos {
                pos = p as i64;
            }
        }
    }
    if pos == -1 && !tokens.is_empty() {
        for t in &tokens {
            let lem_t = lemmatize_token(lem, t);
            let needle: Vec<char> = lem_t.chars().collect();
            if let Some(p) = find_sub(hay, &needle) {
                if pos == -1 || (p as i64) < pos {
                    pos = p as i64;
                }
            }
        }
    }
    if pos == -1 {
        pos = 0;
    }
    let n = textchars.len();
    let pos = pos as usize;
    let mut start = pos.saturating_sub(max_len / 3);
    let mut end = (start + max_len).min(n);
    if start > 0 {
        if let Some(e) = last_sent_end(&textchars, start.saturating_sub(150), pos + 1) {
            start = e.min(pos);
        }
    }
    if end < n {
        if let Some(e) = last_sent_end(&textchars, pos.min(start), (end + 150).min(n)) {
            end = e.max((pos + 1).min(n));
        }
    }
    let snip: String = textchars[start..end]
        .iter()
        .collect::<String>()
        .replace('\n', " ");
    let snip = snip.trim();
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(snip);
    if end < n {
        out.push('…');
    }
    out
}

/// `format_location`: путь + `(стр. N)` + `[ЧЧ:ММ:СС]`.
pub fn format_location(path: &str, page: Option<i64>, t_start: Option<f64>) -> String {
    let mut loc = path.to_string();
    if let Some(p) = page {
        loc.push_str(&format!(" (стр. {p})"));
    }
    if let Some(t) = t_start {
        let h = (t / 3600.0) as i64;
        let m = ((t / 60.0) as i64) % 60;
        let s = (t as i64) % 60;
        loc.push_str(&format!(" [{h:02}:{m:02}:{s:02}]"));
    }
    loc
}
