//! SEED-A (GT-D25) — seed in: one kind, one table, one write door.
//!
//! What is pinned: the closed set grew by seed.received (register tier,
//! physical_consumption class beside the jar's OUT side); the two sentences
//! fire in the signed order 1, 2; a receipt is add-only at 0.1 oz; it is
//! capacity-free and touches no consumption row; the sow path's oz-out row is
//! byte-identical with a receipt on the log, a blank sow stays unknown and an
//! undone sow leaves its oz-out standing; it replays and verifies; the v42
//! triggers refuse it until v43 reinstalls them.

use crate::db;
use crate::event_file;
use crate::event_partition::{self, EventClass, EventDomain, REGISTER_KINDS};
use crate::events::{self, EventRecord, Kind};
use crate::identity::is_farm_truth;
use crate::projection;
use crate::seed::{
    self, SeedReceivedPayload, SEED_RECEIPTS_COLUMNS, SEED_RECEIVED_PAYLOAD_FIELD_NAMES,
    SEED_UNKNOWN_CROP_LINE, SEED_ZERO_LINE,
};
use crate::trays;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-seed-a-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn count_table(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn count_kind(conn: &Connection, kind: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM event_log WHERE kind = ?1",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn capacity_snapshot(conn: &Connection) -> String {
    serde_json::to_string(&trays::capacity_by_harvest_date(conn).unwrap()).unwrap()
}

/// Every consumption row, in insertion order: (variety_or_item, unit, quantity, sow_event_id).
fn consumption_rows(conn: &Connection) -> Vec<(String, String, f64, Option<String>)> {
    let mut stmt = conn
        .prepare(
            "SELECT variety_or_item, unit, quantity, sow_event_id FROM consumption_events
             ORDER BY rowid",
        )
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn seed_a_kind_set_is_fifty_four_and_seed_received_is_register_physical() {
    assert_eq!(Kind::ALL.len(), 54);
    assert_eq!(Kind::parse("seed.received"), Ok(Kind::SeedReceived));
    assert_eq!(Kind::SeedReceived.as_str(), "seed.received");
    assert_eq!(
        Kind::SeedReceived.tier(),
        (EventDomain::Register, Some(EventClass::PhysicalConsumption))
    );
    assert_eq!(EventClass::ALL.len(), 8);
    assert!(REGISTER_KINDS.contains(&"seed.received"));
    assert!(event_partition::register_kinds().contains(&"seed.received"));
    assert_eq!(event_partition::register_kinds(), REGISTER_KINDS.to_vec());
    assert!(is_farm_truth(Kind::SeedReceived));
    assert_eq!(
        SEED_RECEIVED_PAYLOAD_FIELD_NAMES,
        &["receipt_id", "crop_id", "received_oz"]
    );
}

#[test]
fn seed_a_schema_v43_has_the_table_and_its_guards() {
    let conn = mem();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    assert_eq!(table_columns(&conn, "seed_receipts"), SEED_RECEIPTS_COLUMNS);
    let unique: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND tbl_name = 'seed_receipts' AND sql IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unique, 1, "the PRIMARY KEY (receipt_id) autoindex");
    for trigger in ["seed_receipts_before_update", "seed_receipts_before_delete"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                [trigger],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{trigger}");
    }
    assert!(projection::EXCLUSION_LIST
        .iter()
        .all(|l| !l.contains("seed_receipts")));
    // The installed event_log trigger knows the kind: the v43 reinstall ran.
    let installed: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'trigger' AND name = 'event_log_before_insert'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(installed.contains("'seed.received'"));
}

#[test]
fn seed_a_unknown_crop_is_sentence_1_and_writes_nothing() {
    let mut conn = mem();
    let err = seed::receive_seed(&mut conn, "not-a-crop", 16.0).unwrap_err();
    assert_eq!(err, SEED_UNKNOWN_CROP_LINE);
    // Sentence 1 comes before sentence 2: unknown crop, zero ounces.
    let err = seed::receive_seed(&mut conn, "not-a-crop", 0.0).unwrap_err();
    assert_eq!(err, SEED_UNKNOWN_CROP_LINE);
    let err = seed::receive_seed(&mut conn, "", 16.0).unwrap_err();
    assert_eq!(err, SEED_UNKNOWN_CROP_LINE);
    assert_eq!(count_table(&conn, "seed_receipts"), 0);
    assert_eq!(count_kind(&conn, "seed.received"), 0);
}

#[test]
fn seed_a_zero_negative_non_finite_and_under_a_tenth_are_sentence_2() {
    let mut conn = mem();
    for bad in [0.0, -1.0, 0.04, f64::NAN, f64::INFINITY] {
        let err = seed::receive_seed(&mut conn, "dun-peas", bad).unwrap_err();
        assert_eq!(err, SEED_ZERO_LINE, "{bad}");
    }
    assert_eq!(count_table(&conn, "seed_receipts"), 0);
    assert_eq!(count_kind(&conn, "seed.received"), 0);
}

#[test]
fn seed_a_records_at_a_tenth_and_is_add_only_per_crop() {
    let mut conn = mem();
    let first = seed::receive_seed(&mut conn, "dun-peas", 16.04).unwrap();
    assert_eq!(first.crop_id, "dun-peas");
    assert_eq!(first.crop_name, "Dun peas");
    assert!(
        (first.received_oz - 16.0).abs() < 1e-9,
        "{}",
        first.received_oz
    );
    assert!(!first.created_at.is_empty());
    // A second receipt for the same crop stands beside the first: no key, no cap.
    let second = seed::receive_seed(&mut conn, "dun-peas", 80.0).unwrap();
    assert_ne!(second.receipt_id, first.receipt_id);
    assert_eq!(count_table(&conn, "seed_receipts"), 2);
    assert_eq!(count_kind(&conn, "seed.received"), 2);
    let rows = seed::receipts(&conn).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r.receipt_id == first.receipt_id));
    assert!(rows.iter().any(|r| r.receipt_id == second.receipt_id));
    // The row is what the payload froze: crop id, ounces, the event's created_at.
    let (crop_id, oz, created_at): (String, f64, String) = conn
        .query_row(
            "SELECT crop_id, received_oz, created_at FROM seed_receipts WHERE receipt_id = ?1",
            [&first.receipt_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(crop_id, "dun-peas");
    assert!((oz - 16.0).abs() < 1e-9, "{oz}");
    let event_created: String = conn
        .query_row(
            "SELECT created_at FROM event_log WHERE kind = 'seed.received' AND entity_id = ?1",
            [&first.receipt_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(created_at, event_created);
    // Add-only at the database: no update, no delete.
    let err = conn
        .execute(
            "UPDATE seed_receipts SET received_oz = 1.0 WHERE receipt_id = ?1",
            [&first.receipt_id],
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("seed_receipts is append-only"), "{err}");
    let err = conn
        .execute(
            "DELETE FROM seed_receipts WHERE receipt_id = ?1",
            [&first.receipt_id],
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("seed_receipts is append-only"), "{err}");
    assert_eq!(count_table(&conn, "seed_receipts"), 2);
}

#[test]
fn seed_a_receipt_is_capacity_free_and_touches_no_consumption_row() {
    let mut conn = mem();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(16.0)).unwrap();
    let before = capacity_snapshot(&conn);
    let trays_before = count_table(&conn, "trays");
    let orders_before = count_table(&conn, "orders");
    let wholesale_before = count_table(&conn, "wholesale_orders");
    let consumption_before = consumption_rows(&conn);
    let cost_before = count_table(&conn, "cost_events");
    seed::receive_seed(&mut conn, "dun-peas", 80.0).unwrap();
    assert_eq!(capacity_snapshot(&conn), before);
    assert_eq!(count_table(&conn, "trays"), trays_before);
    assert_eq!(count_table(&conn, "orders"), orders_before);
    assert_eq!(count_table(&conn, "wholesale_orders"), wholesale_before);
    assert_eq!(consumption_rows(&conn), consumption_before);
    assert_eq!(count_table(&conn, "cost_events"), cost_before);
    assert_eq!(count_kind(&conn, "consumption.physical"), 2);
}

#[test]
fn seed_a_sow_oz_out_is_byte_identical_blank_sow_stays_unknown_undo_leaves_it_standing() {
    let mut conn = mem();
    seed::receive_seed(&mut conn, "dun-peas", 80.0).unwrap();
    // A typed sow weight still lands as the one oz-out row, crop NAME as before.
    let sown = trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(16.0)).unwrap();
    let rows = consumption_rows(&conn);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].0, "tray");
    assert_eq!(rows[0].1, "tray");
    assert!((rows[0].2 - 2.0).abs() < 1e-9);
    assert_eq!(rows[1].0, "Dun peas");
    assert_eq!(rows[1].1, "oz");
    assert!((rows[1].2 - 16.0).abs() < 1e-9);
    assert_eq!(rows[0].3, rows[1].3);
    assert!(rows[1].3.is_some());
    // A blank sow weight writes no oz row: unknown, not zero, receipt or none.
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let rows = consumption_rows(&conn);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[2].1, "tray");
    assert_eq!(
        rows.iter().filter(|r| r.1 == "oz").count(),
        1,
        "the blank sow added no oz row"
    );
    // A receipt after the sows is inert to Undo: undo_last reaches the newest
    // sow beneath it, and both oz-out rows stand — nothing compensates.
    seed::receive_seed(&mut conn, "dun-peas", 8.0).unwrap();
    let before = consumption_rows(&conn);
    let undone = trays::undo_last(&mut conn).unwrap().expect("the blank sow");
    assert_eq!(undone.undone_kind, "tray.sown");
    assert_eq!(consumption_rows(&conn), before);
    let undone = trays::undo_last(&mut conn)
        .unwrap()
        .expect("the weighed sow");
    assert_eq!(undone.undone_kind, "tray.sown");
    assert_eq!(consumption_rows(&conn), before);
    assert_eq!(count_table(&conn, "seed_receipts"), 2);
    assert_eq!(count_kind(&conn, "seed.received"), 2);
    let gone: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM trays WHERE id = ?1",
            [&sown.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(gone, 0);
    // Nothing reversible is left: the receipts never become an undo target.
    assert!(trays::undo_last(&mut conn).unwrap().is_none());
    assert_eq!(count_kind(&conn, "seed.received"), 2);
}

#[test]
fn seed_a_payload_is_sealed_at_the_choke_point() {
    let mut conn = mem();
    let good = SeedReceivedPayload {
        receipt_id: "sr-1".into(),
        crop_id: "dun-peas".into(),
        received_oz: 16.0,
    };
    let mut extra = serde_json::to_value(&good).unwrap();
    extra["cropName"] = json!("Dun peas");
    let mut money = serde_json::to_value(&good).unwrap();
    money["priceCents"] = json!(500);
    let bad = [
        (extra, "unknown field"),
        (money, "unknown field"),
        (
            json!({"receiptId":"sr-2","cropId":"dun-peas","receivedOz":0.0}),
            "received_oz must be > 0",
        ),
        (
            json!({"receiptId":"sr-3","cropId":" ","receivedOz":16.0}),
            "requires receipt_id and crop_id",
        ),
    ];
    for (payload, needle) in bad {
        let ev = EventRecord::originated(
            Kind::SeedReceived,
            "seed_receipt",
            "sr-x".to_string(),
            payload,
            json!({ "op": "none" }),
            "2026-09-01T12:00:00.000Z".to_string(),
            None,
            None,
            None,
        );
        let tx = conn.transaction().unwrap();
        let err = events::write_event(&tx, &ev).unwrap_err();
        assert!(err.contains(needle), "{needle}: {err}");
        drop(tx);
    }
    assert_eq!(count_kind(&conn, "seed.received"), 0);
    assert_eq!(count_table(&conn, "seed_receipts"), 0);
}

#[test]
fn seed_a_receipt_replays_and_verify_passes() {
    let dir = temp_dir("verify");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    seed::receive_seed(&mut conn, "kale", 4.0).unwrap();
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);
    let outcome =
        projection::verify_replay_paths(&dir.join("farm.db"), &event_file::events_path(&dir), &dir)
            .unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify failed: {}",
        outcome.summary_line()
    );
    assert_eq!(outcome.report().flush_lag, 0);
    assert_eq!(outcome.report().tables_compared, 26);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn seed_a_v42_triggers_refuse_seed_received_until_v43_reinstalls_them() {
    let register_v42: Vec<&str> = REGISTER_KINDS
        .iter()
        .copied()
        .filter(|k| *k != "seed.received")
        .collect();
    let mut conn = mem();
    db::drop_event_log_triggers(&conn).unwrap();
    conn.execute_batch(&event_partition::schema_event_log_triggers_sql(
        &event_partition::grow_kinds(),
        &register_v42,
        event_partition::EVENT_CLASSES,
        &event_partition::marketing_kinds(),
    ))
    .unwrap();
    conn.pragma_update(None, "user_version", 42).unwrap();
    let err = seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap_err();
    assert!(err.contains("event_log.kind invalid for register"), "{err}");
    assert_eq!(count_table(&conn, "seed_receipts"), 0);
    assert_eq!(count_kind(&conn, "seed.received"), 0);
    db::migrate(&conn).unwrap();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    assert_eq!(count_table(&conn, "seed_receipts"), 1);
    assert_eq!(count_kind(&conn, "seed.received"), 1);
}
