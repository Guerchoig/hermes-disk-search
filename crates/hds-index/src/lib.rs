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

pub mod chunker;
pub mod embed;
pub mod hash;
pub mod heartbeat;
pub mod kinds;
pub mod pipeline;
pub mod progress;
pub mod sidecar;
pub mod walk;
pub mod watch;

pub use chunker::{make_chunks, Chunk, Segment, DEFAULT_OVERLAP, DEFAULT_SIZE};
pub use embed::{vector_blob, Embedder};
pub use hash::{content_hash, hash_of_parts, HASH_WINDOW};
pub use heartbeat::{index_running, session_state, HeartbeatFile, SessionState};
pub use kinds::{kind_of, Kind, MEDIA_KINDS};
pub use pipeline::{process_file, run_index, RunIndexArgs};
pub use progress::ProgressReporter;
pub use sidecar::{Extractor, Lemmatizer, Sidecar, TokenLemmatizer};
pub use walk::{
    walk_files, Excludes, FileFilter, IndexLimits, PreCheck, WalkEvent, WalkOptions,
};
pub use watch::{run_watch, WatchEvent, WatchLock, WatchState};
