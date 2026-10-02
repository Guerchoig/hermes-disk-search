//! `hds` — бинарь CLI ядра (задача B7). Свой разбор argv (`clap` недоступен offline).
//!
//! Подкоманды: `index`, `status`, `check`, `reindex`, `reindex-fts`, `forget`,
//! `stop`, `clip-index`, `db-move`, `watch` (нужен `db-move` для перезапуска).

use std::process::exit;

use hds_cli::cmd;

fn usage() -> String {
    "использование: hds <команда> [аргументы]\n\
     \x20 index        [--roots a;b] [--kinds t,p] [--full] [--rechunk] [--limit N]\n\
     \x20              [--no-prune] [--confirm-delete] [--progress-sec N] [--quiet]\n\
     \x20 status       [--json]\n\
     \x20 search       <запрос> [--kinds t,p] [--limit N] [--json]\n\
     \x20 check\n\
     \x20 reindex      <path> [--no-force]\n\
     \x20 reindex-fts  [--progress-sec N]\n\
     \x20 forget       <path>\n\
     \x20 stop\n\
     \x20 clip-index\n\
     \x20 whisper-check [--file <медиа>] [--json]\n\
     \x20 db-move      --to <path> [--force]\n\
     \x20 watch        [--roots a;b]"
        .to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    exit(run(&args));
}

/// Диспетчер подкоманд (код возврата — как у Python-версии).
fn run(args: &[String]) -> i32 {
    let (cmd_name, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => {
            eprintln!("{}", usage());
            return 2;
        }
    };
    match cmd_name {
        "index" => cmd::index::cmd_index(cmd::index::IndexOpts {
            roots: flag_val(rest, "--roots"),
            kinds: flag_val(rest, "--kinds"),
            full: has(rest, "--full"),
            rechunk: has(rest, "--rechunk"),
            limit: flag_u64(rest, "--limit").map(|v| v as usize),
            no_prune: has(rest, "--no-prune"),
            confirm_delete: has(rest, "--confirm-delete"),
            progress_sec: flag_u64(rest, "--progress-sec").unwrap_or(3),
            quiet: has(rest, "--quiet"),
        }),
        "status" => cmd::status::cmd_status(has(rest, "--json")),
        "search" => match rest.first() {
            Some(q) if !q.starts_with("--") => cmd::search::cmd_search(
                q.clone(),
                flag_val(rest, "--kinds"),
                flag_u64(rest, "--limit").map(|v| v as usize).unwrap_or(8),
                has(rest, "--json"),
            ),
            _ => {
                eprintln!("search: нужен текст запроса");
                2
            }
        },
        "check" => cmd::check::cmd_check(),
        "reindex" => match rest.first() {
            Some(p) => cmd::reindex::cmd_reindex(p, has(rest, "--no-force")),
            None => {
                eprintln!("reindex: нужен путь к файлу/папке");
                2
            }
        },
        "reindex-fts" => cmd::reindex_fts::cmd_reindex_fts(flag_u64(rest, "--progress-sec").unwrap_or(3)),
        "forget" => match rest.first() {
            Some(p) => cmd::forget::cmd_forget(p),
            None => {
                eprintln!("forget: нужен путь к файлу");
                2
            }
        },
        "stop" => cmd::stop::cmd_stop(),
        "clip-index" => cmd::clip_index::cmd_clip_index(),
        "whisper-check" => {
            cmd::whisper_check::cmd_whisper_check(flag_val(rest, "--file"), has(rest, "--json"))
        }
        "db-move" => match flag_val(rest, "--to") {
            Some(to) => cmd::db_move::cmd_db_move(&to, has(rest, "--force")),
            None => {
                eprintln!("db-move: нужен --to <новый путь к index.db>");
                2
            }
        },
        "watch" => cmd::watch::cmd_watch(flag_val(rest, "--roots")),
        "--help" | "-h" | "help" => {
            println!("{}", usage());
            0
        }
        other => {
            eprintln!("неизвестная подкоманда '{other}'");
            eprintln!("{}", usage());
            2
        }
    }
}

/// Есть ли флаг `name` среди аргументов.
fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Значение флага `--name value` или `--name=value`.
fn flag_val(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(name) {
            if let Some(v) = rest.strip_prefix('=') {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Числовое значение флага.
fn flag_u64(args: &[String], name: &str) -> Option<u64> {
    flag_val(args, name).and_then(|v| v.trim().parse().ok())
}
