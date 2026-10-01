//! Подкоманды CLI. Каждая — тонкий порт соответствующей функции `hds/cli.py`
//! (или `hds/dbops.py`/`hds/diag.py`); общие помощники — в [`crate::support`].

pub mod check;
pub mod clip_index;
pub mod db_move;
pub mod forget;
pub mod index;
pub mod reindex;
pub mod reindex_fts;
pub mod status;
pub mod stop;
pub mod watch;
