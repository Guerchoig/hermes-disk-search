//! A4 шаг 2: шлюз паузы `index.pause` и чтение heartbeat.
//!
//! Главное, что проверяется: файл создаётся/удаляется **как в Python-версии**
//! (пустой `index.pause` в корне проекта), вложенные запросы держат паузу до
//! последнего, а **пользовательскую** паузу `llm-host` не снимает за него.

use std::sync::Arc;

use hds_llama::pause::{read_heartbeat, stop_requested, IndexPause, HEARTBEAT_FILE, PAUSE_FILE};

/// Временный каталог сигналов (аналог корня проекта).
struct Tmp {
    dir: std::path::PathBuf,
}

impl Tmp {
    fn new(name: &str) -> Tmp {
        let dir = std::env::temp_dir().join("hds-llama-tests").join(format!(
            "pause-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Tmp { dir }
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Аренда создаёт пустой `index.pause` (как UI) и снимает его на `Drop`.
#[test]
fn lease_creates_and_releases_pause_file() {
    let tmp = Tmp::new("lease");
    let gate = Arc::new(IndexPause::new(&tmp.dir));
    assert_eq!(gate.path().file_name().unwrap(), PAUSE_FILE);
    assert!(!gate.is_paused());

    let lease = gate.lease("тест: запрос чата").expect("пауза");
    assert!(gate.is_paused(), "файл-пауза обязан появиться");
    assert_eq!(
        std::fs::metadata(gate.path()).expect("файл").len(),
        0,
        "файл пустой — так же пишет UI (`open(_PAUSE, \"w\").close()`)"
    );
    assert_eq!(gate.depth(), 1);

    drop(lease);
    assert!(!gate.is_paused(), "после запроса пауза снимается (ARB-2)");
    assert_eq!(gate.depth(), 0);
}

/// Два одновременных запроса: пауза снимается только после последнего.
#[test]
fn nested_leases_keep_pause_until_last_request() {
    let tmp = Tmp::new("nested");
    let gate = Arc::new(IndexPause::new(&tmp.dir));
    let first = gate.lease("запрос 1").expect("1");
    let second = gate.lease("запрос 2").expect("2");
    assert_eq!(gate.depth(), 2);

    drop(first);
    assert!(gate.is_paused(), "второй запрос ещё идёт — пауза нужна");
    assert_eq!(gate.depth(), 1);

    drop(second);
    assert!(!gate.is_paused());
}

/// Паузу пользователя `llm-host` не снимает: её снимает только сам пользователь
/// (кнопка «Продолжить» в UI) или аварийный `force_resume`.
#[test]
fn user_pause_is_preserved() {
    let tmp = Tmp::new("user");
    let gate = Arc::new(IndexPause::new(&tmp.dir));
    std::fs::write(gate.path(), b"").expect("пауза пользователя");
    assert!(gate.is_paused());

    let lease = gate.lease("запрос чата").expect("pause");
    assert!(
        gate.user_paused(),
        "файл существовал до нас — это пауза пользователя"
    );
    drop(lease);
    assert!(
        gate.is_paused(),
        "чужую паузу не снимаем — иначе запрос молча возобновит индексацию"
    );

    assert!(
        gate.force_resume().expect("force"),
        "аварийное снятие работает"
    );
    assert!(!gate.is_paused());
}

/// `resume()`/`force_resume()` без нашей паузы не трогают чужой файл.
///
/// Регресс, пойманный на живом прогоне A4 шага 2: `resume()` вызывался «на всякий
/// случай» после запроса и удалял `index.pause`, поставленный пользователем.
#[test]
fn resume_without_our_pause_keeps_user_file() {
    let tmp = Tmp::new("no-lease");
    let gate = Arc::new(IndexPause::new(&tmp.dir));
    std::fs::write(gate.path(), b"").expect("пауза пользователя");

    assert!(!gate.resume().expect("resume"), "снимать нечего");
    assert!(gate.is_paused(), "чужой файл остаётся на месте");
    assert!(!gate.user_paused(), "мы её не ставили");

    // то же после того, как наша аренда уже снялась (файл пользователя вернулся)
    let lease = gate.lease("наш запрос").expect("lease");
    drop(lease);
    assert!(!gate.resume().expect("resume"));
    assert!(gate.is_paused());
}

/// Аренда поверх паузы пользователя: на `Drop` файл остаётся, `force_resume` снимает.
#[test]
fn lease_over_user_pause_keeps_it_until_force() {
    let tmp = Tmp::new("over-user");
    let gate = Arc::new(IndexPause::new(&tmp.dir));
    std::fs::write(gate.path(), b"").expect("пауза пользователя");
    {
        let _lease = gate.lease("наш запрос").expect("lease");
        assert!(gate.is_paused() && gate.user_paused());
    }
    assert!(
        gate.is_paused(),
        "чужую паузу аренда не снимает (Drop → resume → no-op)"
    );
    assert!(gate.force_resume().expect("force"));
    assert!(!gate.is_paused());
}

#[test]
fn heartbeat_reports_pause_and_counters() {
    let tmp = Tmp::new("hb");
    assert!(read_heartbeat(&tmp.dir).is_none(), "файла нет — нет данных");
    assert!(!stop_requested(&tmp.dir));

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(
        tmp.dir.join(HEARTBEAT_FILE),
        format!(
            r#"{{"ts": {now}, "seen": 120, "processed": 118, "errors": 2, "chunks": 4567,
                "paused": true, "total": 500, "eta_sec": 30, "path": "D:\\big.pdf", "phase": "ocr"}}"#
        ),
    )
    .expect("heartbeat");

    let hb = read_heartbeat(&tmp.dir).expect("heartbeat");
    assert!(hb.fresh);
    assert!(hb.paused);
    assert_eq!(hb.seen, 120);
    assert_eq!(hb.chunks, 4567);
    assert_eq!(hb.total, Some(500));
    assert_eq!(hb.current_path.as_deref(), Some("D:\\big.pdf"));
    assert_eq!(hb.phase.as_deref(), Some("ocr"));
    assert_eq!(hb.label(), "пауза (index.pause)");
    assert!(!hb.is_live(), "паузный прогон — не живой (R30)");

    // старый heartbeat (свежесть 30 с) — прогон завершён или завис
    std::fs::write(
        tmp.dir.join(HEARTBEAT_FILE),
        format!(r#"{{"ts": {}, "paused": false}}"#, now - 100),
    )
    .expect("heartbeat");
    let hb = read_heartbeat(&tmp.dir).expect("heartbeat");
    assert!(!hb.fresh);
    assert_eq!(
        hb.label(),
        "нет свежего heartbeat (прогон завершён или завис)"
    );

    // мусорный файл без `ts` не должен выглядеть как пауза
    std::fs::write(tmp.dir.join(HEARTBEAT_FILE), br#"{"paused": true}"#).expect("heartbeat");
    let hb = read_heartbeat(&tmp.dir).expect("heartbeat");
    assert!(!hb.fresh && !hb.paused, "без ts доверять нечему");

    std::fs::write(tmp.dir.join("index.stop"), b"").expect("stop");
    assert!(stop_requested(&tmp.dir));
}
