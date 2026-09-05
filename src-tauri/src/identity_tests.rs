//! Wave 0 — Groundtruth identity. The freeze guards this file once held were
//! retired 2026-08-12 (identity::CUTOVER_LIVE) and deleted in C4 (INT-004);
//! the lock tests that remain exercise the pure classifiers and the paths
//! the freeze never gated, and int004_ pins that the lock never activates.

use crate::db;
use crate::event_file;
use crate::events::{self, EventRecord, Kind};
use crate::identity::{
    assert_data_dir_allowed, enforce_cutover_lock, is_farm_truth, read_application_id_from_file,
    stamp_or_refuse, GROUNDTRUTH_APPLICATION_ID, LOCK_ACTIVE_IN_TEST,
};
use crate::projection;
use crate::snapshots;
use crate::trays;
use rusqlite::Connection;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CUTOVER_REFUSAL: &str = "Farm truth lives in Farm OS until cutover.";
const FOREIGN_DB_REFUSAL: &str =
    "This database belongs to another app. Groundtruth will not open it.";
const FARMOS_DIR_REFUSAL: &str = "Groundtruth refuses to run inside the Farm OS data folder.";

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-identity-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn application_id(conn: &Connection) -> i32 {
    conn.query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn t1_enforce_cutover_lock_refuses_grow_and_register() {
    let grow_err = enforce_cutover_lock(Kind::TraySown).unwrap_err();
    assert_eq!(grow_err, CUTOVER_REFUSAL);

    let register_err = enforce_cutover_lock(Kind::CostMoneyOut).unwrap_err();
    assert_eq!(register_err, CUTOVER_REFUSAL);
}

#[test]
fn t2_stamp_or_refuse_stamps_fresh_db_then_accepts() {
    let conn = Connection::open_in_memory().unwrap();
    stamp_or_refuse(&conn).unwrap();
    let id: i32 = conn
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap();
    assert_eq!(id, GROUNDTRUTH_APPLICATION_ID);
    stamp_or_refuse(&conn).unwrap();
}

#[test]
fn t3_stamp_or_refuse_refuses_populated_unstamped_event_log() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE event_log (
            seq INTEGER PRIMARY KEY,
            id TEXT NOT NULL
        );
        INSERT INTO event_log (id) VALUES ('e1');",
    )
    .unwrap();
    let err = stamp_or_refuse(&conn).unwrap_err();
    assert_eq!(err, FOREIGN_DB_REFUSAL);
}

#[test]
fn t4_assert_data_dir_allowed_refuses_farmos_allows_groundtruth() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = std::env::temp_dir().join(format!("groundtruth-identity-{stamp}"));
    let farmos = base.join("com.prairieroots.farmos");
    let groundtruth = base.join("com.prairieroots.groundtruth");
    fs::create_dir_all(&farmos).unwrap();
    fs::create_dir_all(&groundtruth).unwrap();

    let refuse = assert_data_dir_allowed(&farmos).unwrap_err();
    assert_eq!(refuse, FARMOS_DIR_REFUSAL);
    assert_data_dir_allowed(&groundtruth).unwrap();

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn t5_housekeeping_kinds_are_not_farm_truth() {
    assert!(!is_farm_truth(Kind::SnapshotTaken));
    assert!(!is_farm_truth(Kind::AttentionResolved));
    assert!(is_farm_truth(Kind::TraySown));
    assert!(is_farm_truth(Kind::CostMoneyOut));
}

#[test]
fn t7_snapshot_and_attention_pass_while_locked() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));
    let mut conn = mem();

    let snap = EventRecord::originated(
        Kind::SnapshotTaken,
        "snapshot",
        "snap-lock-t7",
        json!({ "path": "farm-snap-t7.db" }),
        json!({ "op": "none" }),
        "2026-08-10T00:00:00.000Z",
        None,
        None,
        Some("ev-snap-t7".into()),
    );
    {
        let tx = conn.transaction().unwrap();
        events::write_event(&tx, &snap).unwrap();
        tx.commit().unwrap();
    }
    let snap_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE id = 'ev-snap-t7'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(snap_count, 1);

    let attention = EventRecord::originated(
        Kind::AttentionResolved,
        "attention",
        "att-lock-t7",
        json!({
            "attentionId": "att-lock-t7",
            "action": "dismiss",
            "kind": "snapshot_failed",
        }),
        json!({ "op": "none" }),
        "2026-08-10T00:00:00.000Z",
        None,
        None,
        Some("ev-att-t7".into()),
    );
    {
        let tx = conn.transaction().unwrap();
        events::write_event(&tx, &attention).unwrap();
        tx.commit().unwrap();
    }
    let att_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE id = 'ev-att-t7'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(att_count, 1);

    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
}

#[test]
fn t8_imported_path_is_not_locked() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));
    let mut conn = mem();
    let event = EventRecord::originated(
        Kind::TraySown,
        "tray",
        "tray-import-t8",
        json!({
            "cropId": "dun-peas",
            "quantity": 1,
            "sownOn": "2026-08-10",
            "blackoutOn": "2026-08-10",
        }),
        json!({ "op": "delete_tray", "trayId": "tray-import-t8" }),
        "2026-08-10T00:00:00.000Z",
        None,
        None,
        Some("ev-import-t8".into()),
    );
    {
        let tx = conn.transaction().unwrap();
        events::write_imported_event(&tx, &event).unwrap();
        tx.commit().unwrap();
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE id = 'ev-import-t8'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
}

#[test]
fn t9_open_and_migrate_stamps_a_fresh_database() {
    let dir = temp_dir("t9");
    let path = dir.join("farm.db");
    let conn = db::open_and_migrate(&path).unwrap();
    assert_eq!(application_id(&conn), GROUNDTRUTH_APPLICATION_ID);
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t10_open_and_migrate_refuses_a_foreign_database_before_migrating() {
    let dir = temp_dir("t10");
    let path = dir.join("foreign.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 0;
             CREATE TABLE event_log (
                 seq INTEGER PRIMARY KEY,
                 id TEXT NOT NULL
             );
             INSERT INTO event_log (id) VALUES ('e1');",
        )
        .unwrap();
        assert_eq!(application_id(&conn), 0);
    }
    let err = db::open_and_migrate(&path).unwrap_err();
    assert_eq!(err, FOREIGN_DB_REFUSAL);
    {
        let conn = Connection::open(&path).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 0);
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t11_vacuum_into_preserves_the_stamp() {
    let dir = temp_dir("t11");
    let src = dir.join("farm.db");
    let dest = dir.join("copy.db");
    {
        let conn = db::open_and_migrate(&src).unwrap();
        assert_eq!(application_id(&conn), GROUNDTRUTH_APPLICATION_ID);
        let dest_str = dest.to_str().unwrap();
        conn.execute("VACUUM INTO ?1", rusqlite::params![dest_str])
            .unwrap();
    }
    let copy = Connection::open(&dest).unwrap();
    assert_eq!(application_id(&copy), GROUNDTRUTH_APPLICATION_ID);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t12_validate_farm_file_refuses_an_unstamped_farm_database() {
    let dir = temp_dir("t12");
    let path = dir.join("farmos.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "PRAGMA user_version = {};
             CREATE TABLE crops (id TEXT);
             CREATE TABLE trays (id TEXT);
             CREATE TABLE event_log (id TEXT);",
            db::SCHEMA_VERSION
        ))
        .unwrap();
        assert_eq!(application_id(&conn), 0);
    }
    let err = snapshots::validate_farm_file(&path).unwrap_err();
    assert_eq!(err, FOREIGN_DB_REFUSAL);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t13_validate_farm_file_accepts_a_groundtruth_snapshot() {
    let dir = temp_dir("t13");
    let farm = dir.join("farm.db");
    let snap_dir = dir.join("snapshots");
    fs::create_dir_all(&snap_dir).unwrap();
    let mut conn = db::open_and_migrate(&farm).unwrap();
    let info = snapshots::take_snapshot(&mut conn, &snap_dir).unwrap();
    snapshots::validate_farm_file(std::path::Path::new(&info.path)).unwrap();
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

/// Build an unstamped (application_id 0) farm directory with valid schema,
/// one sow, and a flushed events.jsonl. Mimics a Farm OS data directory.
fn unstamped_populated_farm_dir(label: &str) -> PathBuf {
    let dir = temp_dir(label);
    let path = dir.join("farm.db");
    let mut conn = Connection::open(&path).unwrap();
    db::configure(&conn).unwrap();
    db::migrate(&conn).unwrap();
    assert_eq!(application_id(&conn), 0);
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    // Helper used: event_file::flush_events (same as lib.rs flush_dir)
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);
    dir
}

fn dir_snapshot(dir: &Path) -> BTreeMap<String, SystemTime> {
    let mut map = BTreeMap::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        let mtime = entry.metadata().unwrap().modified().unwrap();
        map.insert(name, mtime);
    }
    map
}

#[test]
fn t16_read_application_id_from_file() {
    let dir = temp_dir("t16");

    let stamped = dir.join("stamped.db");
    {
        let conn = db::open_and_migrate(&stamped).unwrap();
        assert_eq!(application_id(&conn), GROUNDTRUTH_APPLICATION_ID);
        drop(conn);
    }
    assert_eq!(
        read_application_id_from_file(&stamped).unwrap(),
        GROUNDTRUTH_APPLICATION_ID
    );

    let zero = dir.join("zero.db");
    {
        let mut header = vec![0u8; 100];
        header[0..16].copy_from_slice(b"SQLite format 3\0");
        // application_id at bytes 68..72 stays 0
        let mut f = fs::File::create(&zero).unwrap();
        f.write_all(&header).unwrap();
    }
    assert_eq!(read_application_id_from_file(&zero).unwrap(), 0);

    let plain = dir.join("plain.txt");
    fs::write(&plain, b"not a sqlite database").unwrap();
    let err = read_application_id_from_file(&plain).unwrap_err();
    assert_eq!(err, "That file isn't a Farm OS farm.");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t17_farm_dir_verify_refuses_an_unstamped_farm_dir() {
    // Helper used: event_file::flush_events (same as lib.rs flush_dir)
    let dir = unstamped_populated_farm_dir("t17");
    let err = projection::farm_dir_verify(&dir).unwrap_err();
    assert_eq!(err, FOREIGN_DB_REFUSAL);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t18_refusal_writes_nothing_into_the_directory() {
    // Helper used: event_file::flush_events (same as lib.rs flush_dir)
    let dir = unstamped_populated_farm_dir("t18");
    let before = dir_snapshot(&dir);
    let err = projection::farm_dir_verify(&dir).unwrap_err();
    assert_eq!(err, FOREIGN_DB_REFUSAL);
    let after = dir_snapshot(&dir);
    assert_eq!(
        before, after,
        "refusal must leave the directory byte-for-byte identical \
         (no last-verify-replay.txt, no -wal/-shm, no mtime changes)"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t19_farm_dir_verify_still_passes_on_a_groundtruth_dir() {
    // Helper used: event_file::flush_events (same as lib.rs flush_dir)
    let dir = temp_dir("t19");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);

    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    assert!(
        dir.join("last-verify-replay.txt").is_file(),
        "stamped Groundtruth dir must still receive the verify report"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// C4 (INT-004). The tripwire under every "retired freeze" comment: with the
/// test opt-in set, the lock still never activates, which is only possible
/// while `CUTOVER_LIVE` is true. Flip the constant and this fails before any
/// comment in identity.rs becomes a lie again.
#[test]
fn int004_freeze_is_retired_the_lock_never_activates() {
    let retired = crate::identity::CUTOVER_LIVE;
    assert!(retired, "the freeze is retired by constant");
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));
    assert!(
        !crate::identity::lock_is_active(),
        "CUTOVER_LIVE makes the freeze gate inert; the test opt-in is never consulted"
    );
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    assert!(!crate::identity::lock_is_active());
}
