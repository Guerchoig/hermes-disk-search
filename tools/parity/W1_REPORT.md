# W1_REPORT.md — журнал волны W1 (резидентный слой: поиск/MCP/UI)

> Ветка `w2-llm-host` (W1 продолжаем в ней), начато **01.10.2026**. План —
> `MIGRATION_PLAN_RUST.md` §4.1 (W1). Правило волны: Python-версия — источник
> истины, паритет проверяется golden/тестами.

## 1. Порт поиска `hds/search.py` → `crates/hds-search` (01.10.2026)

**Что сделано.** Гибридный поиск (FTS5 BM25 + `chunks_vec` + CLIP, слияние RRF)
перенесён в Rust с сохранением поведения Python:

| Файл | Что внутри |
|---|---|
| `crates/hds-search/src/fts.rs` | `find_tokens` (`[\w]{2,}`), `lemmatize_token` (через воркер), `fts_tokens` (≤12), `quoted`, `fts_query`, `fts_search_ids` (AND→OR, префикс хвоста, pushdown `kinds`) |
| `crates/hds-search/src/snippet.rs` | `make_snippet` (окно по первому токену, границы предложений), `format_location` |
| `crates/hds-search/src/lib.rs` | `search()` — RRF по FTS+vec+CLIP, `SearchResult`/`to_json` |
| `crates/hds-cli/src/cmd/search.rs` | `hds search <запрос> [--kinds] [--limit] [--json]` |

**Ключевые решения.**
* Лемматизация токенов запроса — **через Python-воркер** (`hds_index::Lemmatizer`),
  тем же `pymorphy3`, что и `chunks_fts` (иначе FTS не совпадёт) — §6 плана.
* Эмбеддинги — фасад (`hds_index::Embedder`), CLIP — `hds_clip::shared`.
  Ошибки ветвей глушатся (как Python): поиск не падает из-за недоступной роли.
* Порядок результатов — стабильная сортировка по score (порядок первого появления
  = порядок dict в Python), `round(score, 5)`.

**Паритет (golden).** Против БД фикстур `tools/parity/out/index.db` (16 файлов,
6362 чанка, векторы от W0) и golden `search_*.json`: **все 10 контрольных запросов
— топ-20 совпал точно** (`path|page|t_start`, включая порядок).
```powershell
cargo test -p hds-search --test search_parity -- --ignored --nocapture   # Q01..Q10 ok
```
Живой CLI (та же БД, роль embedding `:8011`): `hds search 'Накладная дата склад'`
→ накладная.jpg (OCR), инструкция.md, скан.pdf — с локацией и сниппетом.

**Тесты:** `search_core` (5, чистые: токены, `fts_query`, сниппет, локация) +
`search_parity` (1, `#[ignore]`, golden). Итого `cargo test --workspace` —
**150 passed / 0 failed (+8 ignored)**.

**Артефакты:** `out/w1_search.yaml` (конфиг для CLI против БД фикстур).

**Далее по W1:** `hds-mcp`/`hds-ui` (поиск/`ask` через тот же крейт: `Clip::embed_text`
в поисковую ветку уже подключён), RAG (`rag.ask`).
