//! Метки анонимной зоны в FTS (TR-4.6): `[PERSON_3]` ищется как одно слово.
//!
//! Проверяется поведение, которое даёт конвейер: в `chunks.text` остаётся
//! исходный текст с меткой (его видит векторная ветка и выдача), в `chunks_fts`
//! метка заменена токеном `xperson3x` (`pipeline::commit_file` /
//! `reindex-fts`). Запрос нормализуется так же (`fts_tokens`).

use std::path::PathBuf;

use hds_core::db;
use hds_core::labels::split_labels;
use hds_index::Lemmatizer;
use hds_search::fts_search_ids;

/// Лемматизатор-заглушка: нижний регистр (identity-лемма).
struct LowerLem;
impl Lemmatizer for LowerLem {
    fn normalize_many(&self, texts: &[String]) -> Result<Vec<String>, hds_core::error::CoreError> {
        Ok(texts.iter().map(|t| t.to_lowercase()).collect())
    }
}

fn tmp_db(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-labels-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("index.db")
}

/// FTS-текст чанка так, как его пишет конвейер: текст без меток (лемматизация)
/// + токены меток.
fn fts_text(text: &str) -> String {
    let (plain, tokens) = split_labels(text);
    let mut out = plain.to_lowercase();
    for t in tokens {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&t);
    }
    out
}

/// `path -> список chunk_id`, найденных по запросу.
fn paths_of(conn: &rusqlite::Connection, ids: &[i64]) -> Vec<String> {
    ids.iter()
        .map(|cid| {
            conn.query_row(
                "SELECT f.path FROM chunks c JOIN files f ON f.id=c.file_id WHERE c.id=?1",
                [cid],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
        })
        .collect()
}

#[test]
fn label_is_one_fts_word() {
    let conn = db::connect(&tmp_db("one"), 4).unwrap();
    let fid = |path: &str| {
        db::upsert_file(&conn, path, ".md", "text", 10, 1.0, None, Some("P0001")).unwrap()
    };
    let a = fid("D:/hds-anon/P0001/workspace/a.md");
    let b = fid("D:/hds-anon/P0001/workspace/b.md");
    let c = fid("D:/hds-anon/P0001/workspace/c.md");

    let a_text = "Поручено [PERSON_3|дат] подготовить отчёт по этапу 2.";
    let b_text = "[PERSON_30] согласовал план-график.";
    let c_text = "В тексте встречаются слова person и 3 отдельно.";
    for (fid, text) in [(a, a_text), (b, b_text), (c, c_text)] {
        db::add_chunk(&conn, fid, 0, Some(1), None, None, text, &fts_text(text)).unwrap();
        db::finish_file(&conn, fid, "indexed", None, Some(1), 1.0).unwrap();
    }

    // Метка без суффикса и с суффиксом — один и тот же токен (TR-4.6).
    for q in ["[PERSON_3]", "[PERSON_3|дат]", "что поручено [PERSON_3]?"] {
        let ids = fts_search_ids(&conn, &LowerLem, q, 10, None, None);
        assert_eq!(
            paths_of(&conn, &ids),
            vec!["D:/hds-anon/P0001/workspace/a.md".to_string()],
            "запрос {q}"
        );
    }
    // Другая метка — другой токен: [PERSON_30] не находит [PERSON_3].
    let ids = fts_search_ids(&conn, &LowerLem, "[PERSON_30]", 10, None, None);
    assert_eq!(
        paths_of(&conn, &ids),
        vec!["D:/hds-anon/P0001/workspace/b.md".to_string()]
    );
    // Слово «person» не должно находить фрагменты с метками.
    let ids = fts_search_ids(&conn, &LowerLem, "person", 10, None, None);
    assert_eq!(
        paths_of(&conn, &ids),
        vec!["D:/hds-anon/P0001/workspace/c.md".to_string()],
        "метка не должна индексироваться как слово person"
    );
    // Обычный текст чанка с меткой ищется по своим словам.
    let ids = fts_search_ids(&conn, &LowerLem, "поручено отчёт", 10, None, None);
    assert_eq!(
        paths_of(&conn, &ids),
        vec!["D:/hds-anon/P0001/workspace/a.md".to_string()]
    );
}

#[test]
fn label_token_survives_suffix_variants_in_query() {
    // Запрос из одной метки с падежным суффиксом и «мусором» вокруг.
    let conn = db::connect(&tmp_db("suffix"), 4).unwrap();
    let fid = db::upsert_file(
        &conn,
        "D:/hds-anon/P0002/workspace/p.md",
        ".md",
        "text",
        10,
        1.0,
        None,
        Some("P0002"),
    )
    .unwrap();
    let text = "Передать [ORG_21|дат] требования.";
    db::add_chunk(&conn, fid, 0, None, None, None, text, &fts_text(text)).unwrap();
    db::finish_file(&conn, fid, "indexed", None, Some(1), 1.0).unwrap();

    for q in ["[ORG_21]", "[ORG_21|дат]", "[ORG_21|род,мн]"] {
        let ids = fts_search_ids(&conn, &LowerLem, q, 10, None, None);
        assert_eq!(ids.len(), 1, "запрос {q}");
    }
    // Чужой проект не видит фрагмент даже с верным запросом (I5).
    let ids = fts_search_ids(
        &conn,
        &LowerLem,
        "[ORG_21]",
        10,
        None,
        Some(&["P0001".to_string()]),
    );
    assert!(ids.is_empty(), "фильтр проекта обязан применяться к меткам");
}
