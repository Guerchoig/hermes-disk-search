//! `hds-index` — ядро индексации (трек B плана `PLAN_W2_LLM_HOST.md`).
//!
//! Правило волны W2: Python-версия (`hds/indexer.py`) — источник истины по
//! поведению, паритет проверяется golden-файлами (`tools/parity/compare.py`),
//! а не «на глаз». Поэтому здешние функции — дословный порт, вплоть до порядка
//! проверок и текстов сообщений; расхождения допустимы только осознанные и
//! описанные в doc-комментарии.
//!
//! Состав на текущий момент (шаги B1/B2):
//! * [`hash`] — `content_hash` (blake2b-16: `str(size)` + голова/хвост 256 КБ);
//! * [`kinds`] — таблицы расширений → вид файла (`Kind`, лимиты по виду);
//! * [`walk`] — обход корней с `exclude_dirs`/`exclude_paths`, лимиты размеров,
//!   `~$`-файлы Office, предварительные проверки файла (`FileFilter`).

pub mod hash;
pub mod kinds;
pub mod walk;

pub use hash::{content_hash, hash_of_parts, HASH_WINDOW};
pub use kinds::{kind_of, Kind, MEDIA_KINDS};
pub use walk::{
    walk_files, Excludes, FileFilter, IndexLimits, PreCheck, WalkEvent, WalkOptions,
};
