//! Метки анонимной зоны: разбор (TR-1.4) и нормализация для FTS (TR-4.6).
//!
//! Метка — `[PERSON_3]` или `[PERSON_3|дат]`. Синтаксис задан один раз
//! (`PLAN_AGENT_PRIVACY_ZONE.md` §5.1, TR-1.4) и используется Python-воркером
//! (`sidecar/hds_anonymize/vault.py`), Rust-клиентом A1
//! (`hds_anonymize::protocol`) и индексом — второй регулярки быть не должно,
//! поэтому [`LABEL_PATTERN`] и [`label_parts`] живут здесь, а
//! `hds_anonymize::protocol` их переэкспортирует.
//!
//! Зачем нормализация (TR-4.6): стандартный токенизатор FTS5 режет
//! `[PERSON_3]` на `person` и `3`, и запрос «[PERSON_3]» находил бы любые
//! фрагменты со словом «person» и цифрой 3. Поэтому при индексации и в запросе
//! метка превращается в **один** токен `xperson3x` (суффикс `|дат` токен не
//! меняет), а векторный поиск получает текст как есть.
//!
//! Разбор ручной (без `regex`): формат фиксированный, метки встречаются в
//! каждом чанке, разбор должен быть дешёвым и не паниковать.

/// Синтаксис метки (TR-1.4) — единый для Python, Rust-клиента A1 и FTS:
/// `\[([A-Z_]+)_(\d+)(?:\|([а-яё,]+))?\]`, то есть `[PERSON_3]` или
/// `[PERSON_3|дат]` / `[PERSON_3|род,мн]`.
pub const LABEL_PATTERN: &str = r"\[([A-Z_]+)_(\d+)(?:\|([а-яё,]+))?\]";

/// Падеж pymorphy → суффикс метки (TR-2.3). Именительный ед. ч. — без суффикса.
///
/// В `forms.gram` базы меток хранятся коды pymorphy (`datv`), а в метке —
/// русский суффикс (`дат`); [`gram_to_suffix`] переводит одно в другое, чтобы
/// фильтр исходящего и деанонимизатор не заводили вторую систему граммем.
pub const CASE_SUFFIX: [(&str, &str); 6] = [
    ("nomn", ""),
    ("gent", "род"),
    ("datv", "дат"),
    ("accs", "вин"),
    ("ablt", "твор"),
    ("loct", "пред"),
];

/// Граммема множественного числа в суффиксе (`|дат,мн`) и в базе (`datv,мн`).
pub const PLURAL_GRAM: &str = "мн";

/// Код падежа pymorphy → суффикс метки: `datv` → `дат`, `nomn` → ``.
///
/// Уже русский суффикс возвращается как есть (идемпотентность), множественное
/// число сохраняется: `nomn,мн` → `,мн`, `datv,мн` → `дат,мн` (так же, как
/// `morph.suffix_for` в Python).
pub fn gram_to_suffix(gram: &str) -> String {
    let mut cases: Vec<String> = Vec::new();
    let mut plural = false;
    for raw in gram.split(',') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        let mapped = match part {
            "nomn" => "",
            "gent" => "род",
            "datv" => "дат",
            "accs" => "вин",
            "ablt" => "твор",
            "loct" => "пред",
            "plur" | PLURAL_GRAM => {
                plural = true;
                continue;
            }
            other => other,
        };
        if !mapped.is_empty() && !cases.iter().any(|c| c == mapped) {
            cases.push(mapped.to_string());
        }
    }
    match (cases.is_empty(), plural) {
        (true, false) => String::new(),
        (true, true) => format!(",{PLURAL_GRAM}"),
        (false, true) => format!("{},{}", cases.join(","), PLURAL_GRAM),
        (false, false) => cases.join(","),
    }
}

/// Допустимый суффикс метки: русские падежи (TR-2.3) и коды pymorphy
/// (терпимый разбор уже выпущенных меток `[PERSON_1|nomn]`, A3.4).
fn suffix_valid(suffix: &str) -> bool {
    if suffix.is_empty() {
        return false;
    }
    let mut seen = false;
    for part in suffix.split(',') {
        if part.is_empty() {
            continue; // `,мн` — именительный множественного
        }
        let known = CASE_SUFFIX
            .iter()
            .any(|(code, ru)| *code == part || *ru == part)
            || part == PLURAL_GRAM
            || part == "plur";
        if !known {
            return false;
        }
        seen = true;
    }
    seen
}

/// Предел длины метки при сканировании текста: защита от «[» без закрывающей
/// скобки (иначе сканирование квадратично на длинных строках).
const MAX_LABEL_CHARS: usize = 80;

/// Разобрать метку: `"[PERSON_3|дат]"` → `("PERSON", 3, Some("дат"))`.
pub fn label_parts(token: &str) -> Option<(String, u32, Option<String>)> {
    let inner = token.strip_prefix('[')?.strip_suffix(']')?;
    let (body, suffix) = match inner.split_once('|') {
        Some((body, suffix)) if !suffix.is_empty() => (body, Some(suffix.to_string())),
        Some(_) => return None,
        None => (inner, None),
    };
    let (kind, number) = body.rsplit_once('_')?;
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        return None;
    }
    let number: u32 = number.parse().ok()?;
    if let Some(suffix) = &suffix {
        // суффикс — русские падежи (`дат`, `род,мн`) или уже выпущенные коды
        // pymorphy (`nomn`, `datv`): оба варианта должны деанонимизироваться.
        if !suffix_valid(suffix) {
            return None;
        }
    }
    Some((kind.to_string(), number, suffix))
}

/// Является ли строка меткой проекта (TR-1.4).
pub fn is_label(token: &str) -> bool {
    label_parts(token).is_some()
}

/// Токен FTS для метки: `[PERSON_3]` и `[PERSON_3|дат]` → `xperson3x`.
///
/// Форма `x…x` не содержит пробелов и пунктуации, поэтому токенизатор FTS5
/// (`unicode61`) и лексический разбор запроса (`find_tokens`) видят в ней
/// ровно одно слово. Число сохраняется, так что `[PERSON_3]` и `[PERSON_30]`
/// остаются разными метками.
pub fn fts_token(token: &str) -> Option<String> {
    let (kind, number, _suffix) = label_parts(token)?;
    Some(format!("x{}{}x", kind.to_ascii_lowercase(), number))
}

/// Длина метки в байтах, если `s` начинается с корректной метки.
fn label_len_at(s: &str) -> Option<usize> {
    let close = s
        .char_indices()
        .take(MAX_LABEL_CHARS)
        .find(|(_, c)| *c == ']')?
        .0;
    let candidate = &s[..=close];
    is_label(candidate).then_some(candidate.len())
}

/// Разделить текст на «текст без меток» и FTS-токены меток (в порядке появления).
///
/// Текст без меток отдаётся лемматизатору (метка ему не нужна и может быть
/// испорчена морфологией), токены дописываются к FTS-тексту чанка как есть.
pub fn split_labels(text: &str) -> (String, Vec<String>) {
    let mut plain = String::with_capacity(text.len());
    let mut tokens: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        match label_len_at(&rest[start..]) {
            Some(len) => {
                plain.push_str(&rest[..start]);
                if let Some(t) = fts_token(&rest[start..start + len]) {
                    if !tokens.contains(&t) {
                        tokens.push(t);
                    }
                }
                rest = &rest[start + len..];
            }
            None => {
                // Не метка — «[» остаётся частью текста, идём дальше.
                plain.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
            }
        }
    }
    plain.push_str(rest);
    (plain, tokens)
}

/// Текст с метками, заменёнными на их FTS-токены (для запроса).
///
/// `«что поручено [PERSON_3]?»` → `«что поручено? xperson3x»`.
pub fn normalize_labels(text: &str) -> String {
    let (plain, tokens) = split_labels(text);
    if tokens.is_empty() {
        return text.to_string();
    }
    let mut out = plain.trim_end().to_string();
    for t in tokens {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&t);
    }
    out
}

/// Найти все метки в тексте как есть, в порядке появления, без повторов.
///
/// В отличие от [`split_labels`] отдаёт исходные токены (`[PERSON_3|дат]`), а
/// не FTS-форму: это нужно шлюзу агента, который показывает пользователю
/// метки, для которых не нашлось значения.
pub fn find_labels(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        match label_len_at(&rest[start..]) {
            Some(len) => {
                let token = &rest[start..start + len];
                if !out.iter().any(|t| t == token) {
                    out.push(token.to_string());
                }
                rest = &rest[start + len..];
            }
            None => rest = &rest[start + 1..],
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{find_labels, fts_token, is_label, label_parts, normalize_labels, split_labels};

    #[test]
    fn label_parts_parse_tr14() {
        assert_eq!(
            label_parts("[PERSON_3]"),
            Some(("PERSON".to_string(), 3, None))
        );
        assert_eq!(
            label_parts("[PERSON_3|дат]"),
            Some(("PERSON".to_string(), 3, Some("дат".to_string())))
        );
        assert_eq!(
            label_parts("[ORG_21|род,мн]"),
            Some(("ORG".to_string(), 21, Some("род,мн".to_string())))
        );
        // Не метки.
        assert_eq!(label_parts("[person_3]"), None);
        assert_eq!(label_parts("[PERSON 3]"), None);
        assert_eq!(label_parts("[PERSON_3|]"), None);
        assert_eq!(label_parts("PERSON_3"), None);
        assert_eq!(label_parts("[PERSON_x]"), None);
        assert!(is_label("[IMAGE_1]"));
        assert_eq!(label_parts("[IMAGE_1]").unwrap().0, "IMAGE");
    }

    #[test]
    fn fts_token_is_one_word_and_ignores_suffix() {
        assert_eq!(fts_token("[PERSON_3]").as_deref(), Some("xperson3x"));
        // TR-4.6: падежный суффикс токен не меняет.
        assert_eq!(fts_token("[PERSON_3|дат]").as_deref(), Some("xperson3x"));
        assert_eq!(fts_token("[ORG_21|род,мн]").as_deref(), Some("xorg21x"));
        assert_eq!(fts_token("[IMAGE_12]").as_deref(), Some("ximage12x"));
        assert_eq!(fts_token("не метка"), None);
    }

    #[test]
    fn split_labels_plain_text_and_tokens() {
        let (plain, toks) = split_labels("Поручено [PERSON_3|дат] и [ORG_2]; срок — март.");
        assert_eq!(plain, "Поручено  и ; срок — март.");
        assert_eq!(toks, vec!["xperson3x".to_string(), "xorg2x".to_string()]);
        // Текст без меток не меняется.
        let (plain, toks) = split_labels("обычный текст");
        assert_eq!(plain, "обычный текст");
        assert!(toks.is_empty());
        // Одиночная «[» не ломает разбор.
        let (plain, toks) = split_labels("массив[0] и [PERSON_1]");
        assert_eq!(plain, "массив[0] и ");
        assert_eq!(toks, vec!["xperson1x".to_string()]);
    }

    #[test]
    fn find_labels_keeps_raw_tokens() {
        assert_eq!(
            find_labels("видел [PERSON_1] и [ORG_2|род], снова [PERSON_1]"),
            vec!["[PERSON_1]".to_string(), "[ORG_2|род]".to_string()]
        );
        // Не метки и одиночная «[» не попадают в выдачу.
        assert!(find_labels("массив[0] и [person_1]").is_empty());
    }

    #[test]
    fn normalize_labels_appends_tokens() {
        assert_eq!(
            normalize_labels("что поручено [PERSON_3]?"),
            "что поручено ? xperson3x"
        );
        assert_eq!(normalize_labels("без меток"), "без меток");
    }

    #[test]
    fn gram_to_suffix_translates_pymorphy_codes() {
        assert_eq!(super::gram_to_suffix("nomn"), "");
        assert_eq!(super::gram_to_suffix("datv"), "дат");
        assert_eq!(super::gram_to_suffix("gent"), "род");
        assert_eq!(super::gram_to_suffix("ablt"), "твор");
        assert_eq!(super::gram_to_suffix("loct"), "пред");
        assert_eq!(super::gram_to_suffix("datv,мн"), "дат,мн");
        assert_eq!(super::gram_to_suffix("nomn,мн"), ",мн");
        // Идемпотентность: русский суффикс остаётся русским.
        assert_eq!(super::gram_to_suffix("дат"), "дат");
        assert_eq!(super::gram_to_suffix(""), "");
    }

    #[test]
    fn latin_gram_suffix_is_tolerated() {
        // A3.4 успел выпустить `[PERSON_1|nomn]`: такие метки обязаны
        // распознаваться (и деанонимизироваться), пока живёт история.
        assert!(is_label("[PERSON_1|nomn]"));
        assert!(is_label("[PERSON_1|datv]"));
        assert!(is_label("[PERSON_1|nomn,мн]"));
        assert!(is_label("[PERSON_1|,мн]"));
        assert_eq!(
            label_parts("[PERSON_1|datv]"),
            Some(("PERSON".to_string(), 1, Some("datv".to_string())))
        );
        // Неизвестный «падеж» — не метка.
        assert!(!is_label("[PERSON_1|foo]"));
        assert!(!is_label("[PERSON_1|дат foo]"));
    }
}
