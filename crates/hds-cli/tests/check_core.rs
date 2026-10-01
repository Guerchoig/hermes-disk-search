//! Тесты чистых частей `check` (без сети): БД и корни.

mod common;

use common::TempDir;
use hds_core::config::load_from;

#[test]
fn check_db_ok_on_temp_db() {
    let td = TempDir::new("chkdb");
    let db = td.join("index.db");
    let cfg_path = td.write_config(&format!("db_path: '{}'\n", db.to_string_lossy()));
    let cfg = load_from(&cfg_path).unwrap();
    let c = hds_cli::cmd::check::check_db(&cfg);
    assert_eq!(c.status, "ok", "title: {}", c.title);
}

#[test]
fn check_roots_warn_on_missing() {
    let td = TempDir::new("chkroots-missing");
    let missing = td.path.join("nope");
    let cfg_path = td.write_config(&format!(
        "index:\n  roots:\n    - '{}'\ndb_path: 'x'\n",
        missing.to_string_lossy()
    ));
    let cfg = load_from(&cfg_path).unwrap();
    assert_eq!(hds_cli::cmd::check::check_roots(&cfg).status, "warn");
}

#[test]
fn check_roots_ok_on_present() {
    let td = TempDir::new("chkroots-ok");
    let cfg_path = td.write_config(&format!(
        "index:\n  roots:\n    - '{}'\ndb_path: 'x'\n",
        td.path.to_string_lossy()
    ));
    let cfg = load_from(&cfg_path).unwrap();
    assert_eq!(hds_cli::cmd::check::check_roots(&cfg).status, "ok");
}

#[test]
fn check_roots_warn_when_empty() {
    let td = TempDir::new("chkroots-empty");
    let cfg_path = td.write_config("db_path: 'x'\n");
    let cfg = load_from(&cfg_path).unwrap();
    assert_eq!(hds_cli::cmd::check::check_roots(&cfg).status, "warn");
}
