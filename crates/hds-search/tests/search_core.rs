//! Чистые тесты ядра поиска (без БД/сети/воркера): токены, RRF-запрос, сниппет, локация.

use hds_core::error::Result;
use hds_index::Lemmatizer;
use hds_search::{find_tokens, format_location, fts_query, make_snippet};

/// Лемматизатор-заглушка: возвращает токены в нижнем регистре (identity-лемма).
struct LowerLem;
impl Lemmatizer for LowerLem {
    fn normalize_many(&self, texts: &[String]) -> Result<Vec<String>> {
        Ok(texts.iter().map(|t| t.to_lowercase()).collect())
    }
}

#[test]
fn tokens_are_word_runs_of_len_ge_2() {
    assert_eq!(
        find_tokens("Работа с документооборотом, 1С:Документооборот!"),
        vec!["Работа", "документооборотом", "1С", "Документооборот"]
    );
    // одиночные символы и пунктуация отбрасываются (как `[\w]{2,}`)
    assert_eq!(find_tokens("a бб в"), vec!["бб"]);
    assert!(find_tokens("... !!! ,,, ").is_empty());
}

#[test]
fn fts_query_ors_quoted_lemmas() {
    let q = fts_query(&LowerLem, "Настройки Отчёта").unwrap();
    assert_eq!(q, "\"настройки\" OR \"отчёта\"");
    assert!(fts_query(&LowerLem, "").is_none());
}

#[test]
fn snippet_marks_edges_and_strips_newlines() {
    let text = "Первое предложение без ключевых слов.\n\
                Второе предложение содержит маркер цель поиска и продолжается дальше достаточно \
                длинным текстом, чтобы окно пришлось обрезать с обеих сторон.\n\
                Третье предложение тоже есть.";
    let snip = make_snippet(text, "маркер", 60, &LowerLem);
    assert!(snip.contains("маркер"), "snippet={snip}");
    assert!(!snip.contains('\n'), "переносы должны стать пробелами");
}

#[test]
fn snippet_whole_text_when_short() {
    let snip = make_snippet("короткий текст с ключом", "ключом", 500, &LowerLem);
    assert_eq!(snip, "короткий текст с ключом");
}

#[test]
fn location_formats_page_and_timecode() {
    assert_eq!(format_location("D:\\a.pdf", Some(1), None), "D:\\a.pdf (стр. 1)");
    assert_eq!(
        format_location("D:\\a.wav", None, Some(65.0)),
        "D:\\a.wav [00:01:05]"
    );
    assert_eq!(format_location("D:\\a.txt", None, None), "D:\\a.txt");
    assert_eq!(
        format_location("D:\\a.mp4", Some(3), Some(3725.0)),
        "D:\\a.mp4 (стр. 3) [01:02:05]"
    );
}
