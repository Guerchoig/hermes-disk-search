//! Тесты помощников `support` (чистые функции, без сети).

mod common;

use common::TempDir;
use hds_cli::support::{norm_path, parse_kinds, parse_roots, resolve_model};
use hds_core::config::load_from;

#[test]
fn parse_roots_splits_and_trims() {
    let r = parse_roots(" D:\\ ; C:\\x ;; ");
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].to_string_lossy(), "D:\\");
}

#[test]
fn parse_kinds_none_when_empty() {
    assert!(parse_kinds("").is_none());
    assert!(parse_kinds(" ,  , ").is_none());
    assert_eq!(parse_kinds("text, pdf ,").unwrap(), vec!["text", "pdf"]);
}

#[test]
fn norm_path_unifies_separators_and_case() {
    assert_eq!(norm_path("C:\\A\\B"), norm_path("c:/a/b"));
}

#[test]
fn resolve_model_absolute_path() {
    let td = TempDir::new("resolve-abs");
    let gguf = td.join("m.gguf");
    let cfg_path = td.write_config(&format!(
        "llm_server:\n  chat:\n    model: '{}'\n",
        gguf.to_string_lossy()
    ));
    let cfg = load_from(&cfg_path).unwrap();
    assert_eq!(resolve_model(&cfg, "chat"), gguf);
}

#[test]
fn resolve_model_shared_variants() {
    let td = TempDir::new("resolve-shared");
    let runtime = td.join("runtime");
    let models = runtime.join("models");
    std::env::set_var("LLAMA_RUNTIME_DIR", &runtime);

    // 1) манифест есть → путь из манифеста
    let chat = models.join("chat");
    std::fs::create_dir_all(&chat).unwrap();
    let chat_gguf = chat.join("Q.gguf");
    std::fs::write(&chat_gguf, b"x").unwrap();
    std::fs::write(chat.join("current.json"), "{\"file\": \"Q.gguf\"}").unwrap();
    let cfg_chat = load_from(&td.write_config("llm_server:\n  chat:\n    model: 'shared:chat'\n")).unwrap();
    assert_eq!(resolve_model(&cfg_chat, "chat"), chat_gguf);

    // 2) манифеста нет, ровно один *.gguf → он и есть активная модель
    let emb = models.join("embedding");
    std::fs::create_dir_all(&emb).unwrap();
    let emb_gguf = emb.join("bge-m3-Q8_0.gguf");
    std::fs::write(&emb_gguf, b"x").unwrap();
    let cfg_emb = load_from(&td.write_config("llm_server:\n  embedding:\n    model: 'shared:embedding'\n")).unwrap();
    assert_eq!(resolve_model(&cfg_emb, "embedding"), emb_gguf);

    // 3) манифеста нет, gguf несколько → каталог роли (как Python при ошибке)
    let rerank = models.join("rerank");
    std::fs::create_dir_all(&rerank).unwrap();
    std::fs::write(rerank.join("a.gguf"), b"x").unwrap();
    std::fs::write(rerank.join("b.gguf"), b"x").unwrap();
    let cfg_rerank = load_from(&td.write_config("llm_server:\n  rerank:\n    model: 'shared:rerank'\n")).unwrap();
    assert_eq!(resolve_model(&cfg_rerank, "rerank"), rerank);

    std::env::remove_var("LLAMA_RUNTIME_DIR");
}
