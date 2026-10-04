//! Подкоманды CLI. Каждая — тонкий порт соответствующей функции `hds/cli.py`
//! (или `hds/dbops.py`/`hds/diag.py`); общие помощники — в [`crate::support`].

pub mod ask;
pub mod check;
pub mod cline_sync;
pub mod clip_index;
pub mod db_move;
pub mod forget;
pub mod index;
pub mod mcp;
pub mod mcp_http;
pub mod reindex;
pub mod reindex_fts;
pub mod search;
pub mod status;
pub mod stop;
pub mod transcribe;
pub mod ui;
pub mod watch;
pub mod whisper_check;
