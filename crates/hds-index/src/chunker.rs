//! Структурный чанкинг сегментов — дословный порт `hds/chunker.py`
//! (`PLAN_W2_LLM_HOST.md` §5, задача B3).
//!
//! Свойства, которые обязаны сохраниться (иначе «поедет» поиск и golden-паритет):
//! * рекурсивное разбиение «абзац → строка → предложение → слово» с целью ~`size`
//!   символов;
//! * `overlap` — **целые предложения** (не хвост из N символов);
//! * чанк не смешивает сегменты с разными `page`/`t_start`/`t_end` и сегменты
//!   разных секций (`head`), путь заголовков идёт в начало каждого чанка секции.
//!
//! Важная деталь порта: Python-версия везде считает **символы** (`len(str)`),
//! поэтому здесь все длины — `chars().count()`, а смещения для срезов — байтовые
//! (строки в Rust индексируются байтами; разделители ASCII, поэтому оба подхода
//! дают одинаковые куски).
//!
//! Известное осознанное расхождение: `\s` в Python-регулярке покрывает также
//! управляющие символы `\x1c`–`\x1f` и `\x85`; Rust `char::is_whitespace`
//! (Unicode White_Space) их не включает. В документах индекса такие символы
//! не встречаются — паритет на 16 golden-файлах сходится (см. отчёт W2).

use serde::{Deserialize, Serialize};

/// Размер чанка по умолчанию (`chunk.size` в `config.yaml`).
pub const DEFAULT_SIZE: usize = 800;
/// Перекрытие по умолчанию (`chunk.overlap` в `config.yaml`).
pub const DEFAULT_OVERLAP: i64 = 120;

/// Разделители рекурсивного разбиения: от крупной структуры к мелкой.
const SEPS: [&str; 7] = ["\n\n", "\n", ". ", "! ", "? ", " ", ""];

/// Сегмент текста — выход извлекателя (Python: `dict` с теми же полями).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Segment {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub t_start: Option<f64>,
    #[serde(default)]
    pub t_end: Option<f64>,
    /// Путь заголовков секции (md/docx), например «# Раздел / ## Подраздел».
    #[serde(default)]
    pub head: Option<String>,
}

/// Чанк — то, что пишется в `chunks` (Python: `{"text", "page", "t_start", "t_end"}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub text: String,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub t_start: Option<f64>,
    #[serde(default)]
    pub t_end: Option<f64>,
}

/// Число символов (как `len(str)` в Python; `str::len` в Rust считает байты).
fn chars(s: &str) -> usize {
    s.chars().count()
}

/// Порт `_split_keep`: `split` по `sep` с сохранением разделителя в конце куска.
fn split_keep<'a>(sep: &str, text: &'a str) -> Vec<&'a str> {
    if sep.is_empty() {
        // list(text) — посимвольно
        return text
            .char_indices()
            .map(|(i, c)| &text[i..i + c.len_utf8()])
            .collect();
    }
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0usize;
    loop {
        match text[start..].find(sep) {
            None => {
                parts.push(&text[start..]);
                return parts;
            }
            Some(rel) => {
                let idx = start + rel;
                parts.push(&text[start..idx + sep.len()]);
                start = idx + sep.len();
            }
        }
    }
}

/// Порт `_split_text`: рекурсивно разрезать текст на куски не длиннее `size`
/// (одиночное «слово» длиннее `size` не режется посимвольно — отдаётся целиком).
fn split_text(text: &str, size: usize, sep_idx: usize) -> Vec<String> {
    if chars(text) <= size {
        return vec![text.to_string()];
    }
    for (i, sep) in SEPS.iter().enumerate().skip(sep_idx) {
        let parts = split_keep(sep, text);
        if parts.len() > 1 {
            return merge_parts(&parts, size, i);
        }
    }
    vec![text.to_string()]
}

/// Порт `_merge_parts`: склеить мелкие куски до `size`, крупные резать следующим
/// разделителем. Пустые/пробельные куски отбрасываются (как `if p.strip()`).
fn merge_parts(parts: &[&str], size: usize, sep_idx: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    for p in parts {
        if p.is_empty() {
            continue;
        }
        if chars(p) > size {
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf));
            }
            out.extend(split_text(p, size, sep_idx + 1));
            continue;
        }
        if !buf.is_empty() && chars(&buf) + chars(p) > size {
            out.push(std::mem::take(&mut buf));
            buf = (*p).to_string();
        } else {
            buf.push_str(p);
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

/// Длина ведущей последовательности пробельных символов (`\s+`).
fn ws_run_len(s: &str) -> usize {
    s.char_indices()
        .take_while(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0)
}

/// Длина ведущей последовательности переводов строк (`\n+`).
fn nl_run_len(s: &str) -> usize {
    let mut n = 0usize;
    for c in s.chars() {
        if c == '\n' {
            n += 1;
        } else {
            break;
        }
    }
    n
}

/// Порт `_SENT_END_RE.split(text)` для шаблона `(?<=[.!?…])\s+|\n+`:
/// разделители поглощаются и в результат не попадают.
fn split_sentences(text: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    let mut prev = 0usize;
    let mut last: Option<char> = None;
    let mut i = 0usize;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        if c.is_whitespace() {
            let after_end = matches!(last, Some('.') | Some('!') | Some('?') | Some('…'));
            let ws = if after_end { ws_run_len(&text[i..]) } else { 0 };
            let m = if ws > 0 { ws } else { nl_run_len(&text[i..]) };
            if m > 0 {
                out.push(&text[prev..i]);
                prev = i + m;
                i = prev;
                last = None;
                continue;
            }
        }
        last = Some(c);
        i += c.len_utf8();
    }
    out.push(&text[prev..]);
    out
}

/// Порт `_overlap_tail`: хвост предыдущего чанка **целыми предложениями**
/// суммарной длины не больше `overlap`. Последнее предложение берётся, даже если
/// оно чуть длиннее, но не длиннее `size / 2` — иначе дубликат съест чанк.
fn overlap_tail(text: &str, overlap: usize, size: usize) -> String {
    if overlap == 0 {
        return String::new();
    }
    let sents: Vec<String> = split_sentences(text)
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if sents.is_empty() {
        return String::new();
    }
    let mut tail: Vec<&str> = Vec::new();
    let mut total = 0usize;
    for s in sents.iter().rev() {
        if total + chars(s) > overlap {
            break;
        }
        tail.push(s);
        total += chars(s) + 1;
    }
    if tail.is_empty() {
        let last = sents.last().expect("непустой список предложений");
        if chars(last) <= size / 2 {
            tail.push(last);
        }
    }
    tail.iter().rev().copied().collect::<Vec<_>>().join(" ")
}

/// Метаданные сегмента, открывшего текущий чанк.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Meta {
    page: Option<i64>,
    t_start: Option<f64>,
    t_end: Option<f64>,
}

/// Состояние сборки чанков (порт `st` из `make_chunks`).
#[derive(Debug, Default)]
struct State {
    chunks: Vec<Chunk>,
    body: Vec<String>,
    len: usize,
    meta: Option<Meta>,
    head: Option<String>,
    hbudget: usize,
}

impl State {
    /// Порт `flush()`: записать чанк (если тело непустое) и сбросить буфер.
    fn flush(&mut self) {
        let text = self.body.concat().trim().to_string();
        if !text.is_empty() {
            let text = match &self.head {
                Some(h) => format!("{h}\n{text}"),
                None => text,
            };
            let m = self
                .meta
                .expect("метаданные выставляются при открытии чанка");
            self.chunks.push(Chunk {
                text,
                page: m.page,
                t_start: m.t_start,
                t_end: m.t_end,
            });
        }
        self.body.clear();
        self.len = 0;
    }
}

/// Порт `chunker.make_chunks`: сегменты → чанки.
///
/// `size` — целевой размер в **символах**, `overlap` — перекрытие целыми
/// предложениями (`<= 0` отключает перекрытие).
pub fn make_chunks(segments: &[Segment], size: usize, overlap: i64) -> Vec<Chunk> {
    let mut st = State::default();
    for seg in segments {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        let head = seg
            .head
            .as_deref()
            .map(|h| h.trim())
            .filter(|h| !h.is_empty())
            .map(|h| h.to_string());
        let keys = Meta {
            page: seg.page,
            t_start: seg.t_start,
            t_end: seg.t_end,
        };
        // смена страницы/таймкода/секции → новый чанк (не смешивать метаданные)
        if st.meta != Some(keys) || head != st.head {
            if st.meta.is_some() {
                st.flush();
            }
            st.meta = Some(keys);
            st.head = head;
            st.hbudget = st.head.as_ref().map(|h| chars(h) + 1).unwrap_or(0);
        }
        for piece in split_text(text, size, 0) {
            if !st.body.is_empty() && st.len + st.hbudget + chars(&piece) > size {
                st.flush();
                if !st.chunks.is_empty() && overlap > 0 {
                    let tail = overlap_tail(
                        &st.chunks.last().expect("чанк только что записан").text,
                        overlap as usize,
                        size,
                    );
                    if !tail.is_empty() {
                        st.len += chars(&tail) + 1;
                        st.body.push(format!("{tail}\n"));
                    }
                }
            }
            st.len += chars(&piece);
            st.body.push(piece);
        }
    }
    if !st.body.is_empty() {
        st.flush();
    }
    st.chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str) -> Segment {
        Segment {
            text: text.to_string(),
            ..Default::default()
        }
    }

    fn seg_meta(text: &str, page: Option<i64>, head: Option<&str>) -> Segment {
        Segment {
            text: text.to_string(),
            page,
            head: head.map(|h| h.to_string()),
            ..Default::default()
        }
    }

    /// Короткий сегмент — ровно один чанк, метаданные не выдумываются.
    #[test]
    fn short_text_is_one_chunk() {
        let ch = make_chunks(&[seg("Короткий текст.")], 800, 120);
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].text, "Короткий текст.");
        assert_eq!(ch[0].page, None);
        assert_eq!(ch[0].t_start, None);
    }

    /// Пустые/пробельные сегменты игнорируются (как в Python).
    #[test]
    fn blank_segments_are_skipped() {
        let ch = make_chunks(&[seg("   "), seg(""), seg("\n\n")], 800, 120);
        assert!(ch.is_empty());
    }

    /// `head` секции идёт в начало **каждого** чанка секции.
    #[test]
    fn head_is_prefixed_in_every_chunk_of_section() {
        let head = "# Раздел / ## Подраздел";
        let long = "Предложение номер один. ".repeat(60);
        let ch = make_chunks(&[seg_meta(&long, None, Some(head))], 400, 120);
        assert!(ch.len() >= 3, "ожидали ≥3 чанков, получили {}", ch.len());
        for c in &ch {
            assert!(
                c.text.starts_with(head),
                "чанк без head: {}",
                head_of_for_test(&c.text, 60)
            );
        }
    }

    /// Смена page/t_start/t_end или секции открывает новый чанк.
    #[test]
    fn metadata_change_starts_new_chunk() {
        let ch = make_chunks(
            &[
                Segment {
                    text: "текст страницы 1".into(),
                    page: Some(1),
                    ..Default::default()
                },
                Segment {
                    text: "текст страницы 2".into(),
                    page: Some(2),
                    ..Default::default()
                },
                seg_meta("другая секция", Some(2), Some("# Другой заголовок")),
            ],
            800,
            120,
        );
        assert_eq!(ch.len(), 3);
        assert_eq!(ch[0].page, Some(1));
        assert_eq!(ch[1].page, Some(2));
        assert!(ch[2].text.starts_with("# Другой заголовок"));
    }

    /// Перекрытие — начало следующего чанка повторяет хвост предыдущего.
    #[test]
    fn overlap_repeats_tail() {
        let s = "Договор поставки оборудования подписан сторонами. Оплата этапами по акту приёмки.";
        let text = format!("{s} {s} {s} {s} {s} {s}");
        let ch = make_chunks(&[seg(&text)], 160, 80);
        assert!(ch.len() >= 2, "ожидали ≥2 чанков, получили {}", ch.len());
        let prefix: String = ch[1].text.chars().take(20).collect();
        assert!(
            ch[0].text.contains(&prefix),
            "перекрытия нет: «{prefix}» отсутствует в предыдущем чанке"
        );
    }

    /// Очень длинное «слово» режется по символам (поведение Python-версии:
    /// в `_SEPS` последним стоит пустая строка).
    #[test]
    fn long_word_is_split_by_characters() {
        let text = "a".repeat(2000);
        let ch = make_chunks(&[seg(&text)], 800, 120);
        assert_eq!(ch.len(), 3);
        assert_eq!(ch[0].text.chars().count(), 800);
        assert_eq!(ch[1].text.chars().count(), 800);
        assert_eq!(ch[2].text.chars().count(), 400);
    }

    fn head_of_for_test(s: &str, n: usize) -> String {
        s.chars().take(n).collect()
    }
}
