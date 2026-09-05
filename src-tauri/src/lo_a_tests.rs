//! LO-A (GT-D24) — the leftover listing: one kind, one table, one write door.
//!
//! What is pinned: the closed set grew by leftover.listed (register tier); the
//! four sentences fire in the signed order 1, 4, 3, 2; a listing is capped by
//! that crop-day's harvested ounces at 0.1; it is capacity-free; it replays
//! and verifies; an undone harvest is carried, not refused or rewritten.

use crate::db;
use crate::event_file;
use crate::event_partition::{self, EventClass, EventDomain, REGISTER_KINDS};
use crate::events::{self, EventRecord, Kind};
use crate::identity::is_farm_truth;
use crate::leftover::{
    self, LeftoverListedPayload, LEFTOVER_ALREADY_LISTED_LINE, LEFTOVER_LISTED_PAYLOAD_FIELD_NAMES,
    LEFTOVER_LISTINGS_COLUMNS, LEFTOVER_NEEDS_HARVEST_LINE, LEFTOVER_OVER_HARVEST_LINE,
    LEFTOVER_ZERO_LINE,
};
use crate::projection;
use crate::trays;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn today() -> String {
    db::local_date_today()
}

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-lo-a-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Two kale trays, sown, advanced, harvested today at 6.0 oz together.
fn harvest_kale_today(conn: &mut Connection) -> Vec<String> {
    let mut ids = Vec::new();
    for _ in 0..2 {
        let t = trays::sow_tray(conn, "kale", 1).unwrap();
        trays::advance_tray(conn, &t.id).unwrap();
        ids.push(t.id);
    }
    trays::harvest_trays(conn, &ids, 6.0).unwrap();
    ids
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

#[test]
fn lo_a_kind_set_is_fifty_one_and_leftover_listed_is_register_sale_side() {
    assert_eq!(Kind::ALL.len(), 54);
    assert_eq!(Kind::parse("leftover.listed"), Ok(Kind::LeftoverListed));
    assert_eq!(Kind::LeftoverListed.as_str(), "leftover.listed");
    assert_eq!(
        Kind::LeftoverListed.tier(),
        (EventDomain::Register, Some(EventClass::SaleFarmOsPath))
    );
    assert!(REGISTER_KINDS.contains(&"leftover.listed"));
    assert!(event_partition::register_kinds().contains(&"leftover.listed"));
    assert_eq!(event_partition::register_kinds(), REGISTER_KINDS.to_vec());
    assert!(is_farm_truth(Kind::LeftoverListed));
    assert_eq!(
        LEFTOVER_LISTED_PAYLOAD_FIELD_NAMES,
        &["listing_id", "crop_id", "harvested_on", "listed_oz"]
    );
}

#[test]
fn lo_a_schema_v39_has_the_table_its_unique_key_and_its_delete_guard() {
    let conn = mem();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    assert_eq!(
        table_columns(&conn, "leftover_listings"),
        LEFTOVER_LISTINGS_COLUMNS
    );
    let unique: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND tbl_name = 'leftover_listings' AND sql IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        unique, 2,
        "the PRIMARY KEY (listing_id) and UNIQUE (crop_id, harvested_on) autoindexes"
    );
    let trigger: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'trigger' AND name = 'leftover_listings_before_delete'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(trigger, 1);
    assert!(projection::EXCLUSION_LIST
        .iter()
        .all(|l| !l.contains("leftover_listings")));
}

#[test]
fn lo_a_no_harvest_is_sentence_1_and_writes_nothing() {
    let mut conn = mem();
    let err = leftover::list_leftover(&mut conn, "kale", &today(), 2.0).unwrap_err();
    assert_eq!(err, LEFTOVER_NEEDS_HARVEST_LINE);
    // Sentence 1 comes before sentence 3: no harvest, zero ounces.
    let err = leftover::list_leftover(&mut conn, "kale", &today(), 0.0).unwrap_err();
    assert_eq!(err, LEFTOVER_NEEDS_HARVEST_LINE);
    assert_eq!(count_table(&conn, "leftover_listings"), 0);
    assert_eq!(count_kind(&conn, "leftover.listed"), 0);
}

#[test]
fn lo_a_over_the_harvest_is_sentence_2_and_at_the_harvest_lists() {
    let mut conn = mem();
    harvest_kale_today(&mut conn);
    let day = today();
    let err = leftover::list_leftover(&mut conn, "kale", &day, 6.1).unwrap_err();
    assert_eq!(err, LEFTOVER_OVER_HARVEST_LINE);
    assert_eq!(count_table(&conn, "leftover_listings"), 0);
    let row = leftover::list_leftover(&mut conn, "kale", &day, 6.0).unwrap();
    assert_eq!(row.crop_id, "kale");
    assert_eq!(row.crop_name, "Kale");
    assert_eq!(row.harvested_on, day);
    assert!(
        (row.harvested_oz - 6.0).abs() < 1e-9,
        "{}",
        row.harvested_oz
    );
    assert!((row.listed_oz - 6.0).abs() < 1e-9, "{}", row.listed_oz);
    assert!(!row.created_at.is_empty());
    assert_eq!(count_table(&conn, "leftover_listings"), 1);
    assert_eq!(count_kind(&conn, "leftover.listed"), 1);
    let listed = leftover::listings(&conn).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].listing_id, row.listing_id);
}

#[test]
fn lo_a_ounces_are_compared_at_a_tenth() {
    let mut conn = mem();
    harvest_kale_today(&mut conn);
    let day = today();
    // 6.04 rounds to 6.0: at the cap, listed. 0.04 rounds to 0.0: sentence 3.
    let err = leftover::list_leftover(&mut conn, "kale", &day, 0.04).unwrap_err();
    assert_eq!(err, LEFTOVER_ZERO_LINE);
    let row = leftover::list_leftover(&mut conn, "kale", &day, 6.04).unwrap();
    assert!((row.listed_oz - 6.0).abs() < 1e-9, "{}", row.listed_oz);
}

#[test]
fn lo_a_zero_negative_and_non_finite_are_sentence_3() {
    let mut conn = mem();
    harvest_kale_today(&mut conn);
    let day = today();
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let err = leftover::list_leftover(&mut conn, "kale", &day, bad).unwrap_err();
        assert_eq!(err, LEFTOVER_ZERO_LINE, "{bad}");
    }
    assert_eq!(count_table(&conn, "leftover_listings"), 0);
}

#[test]
fn lo_a_second_listing_for_the_key_is_sentence_4_before_3_and_2() {
    let mut conn = mem();
    harvest_kale_today(&mut conn);
    let day = today();
    leftover::list_leftover(&mut conn, "kale", &day, 2.5).unwrap();
    for again in [2.5, 0.0, 99.0] {
        let err = leftover::list_leftover(&mut conn, "kale", &day, again).unwrap_err();
        assert_eq!(err, LEFTOVER_ALREADY_LISTED_LINE, "{again}");
    }
    assert_eq!(count_table(&conn, "leftover_listings"), 1);
    assert_eq!(count_kind(&conn, "leftover.listed"), 1);
}

#[test]
fn lo_a_listing_is_capacity_free() {
    let mut conn = mem();
    harvest_kale_today(&mut conn);
    let day = today();
    let before = capacity_snapshot(&conn);
    let trays_before = count_table(&conn, "trays");
    let orders_before = count_table(&conn, "orders");
    let wholesale_before = count_table(&conn, "wholesale_orders");
    leftover::list_leftover(&mut conn, "kale", &day, 3.0).unwrap();
    assert_eq!(capacity_snapshot(&conn), before);
    assert_eq!(count_table(&conn, "trays"), trays_before);
    assert_eq!(count_table(&conn, "orders"), orders_before);
    assert_eq!(count_table(&conn, "wholesale_orders"), wholesale_before);
    let harvested: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM trays WHERE crop_id = 'kale' AND state = 'harvested'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(harvested, 2);
}

#[test]
fn lo_a_undone_harvest_is_carried_not_refused_or_rewritten() {
    let mut conn = mem();
    harvest_kale_today(&mut conn);
    let day = today();
    let row = leftover::list_leftover(&mut conn, "kale", &day, 4.0).unwrap();
    // The listing is inert to Undo, so undo_last reaches the harvest beneath it.
    let undone = trays::undo_last(&mut conn).unwrap().expect("the harvest");
    assert_eq!(undone.undone_kind, "trays.harvested");
    let listed = leftover::listings(&conn).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].listing_id, row.listing_id);
    assert!((listed[0].listed_oz - 4.0).abs() < 1e-9);
    assert!(
        listed[0].harvested_oz.abs() < 1e-9,
        "{}",
        listed[0].harvested_oz
    );
    // Gate order holds on the same key: no harvest now, so sentence 1 first.
    let err = leftover::list_leftover(&mut conn, "kale", &day, 1.0).unwrap_err();
    assert_eq!(err, LEFTOVER_NEEDS_HARVEST_LINE);
    assert_eq!(count_table(&conn, "leftover_listings"), 1);
}

#[test]
fn lo_a_payload_is_sealed_at_the_choke_point() {
    let mut conn = mem();
    harvest_kale_today(&mut conn);
    let day = today();
    let good = LeftoverListedPayload {
        listing_id: "lo-1".into(),
        crop_id: "kale".into(),
        harvested_on: day.clone(),
        listed_oz: 1.0,
    };
    let mut extra = serde_json::to_value(&good).unwrap();
    extra["priceCents"] = json!(500);
    let bad = [
        (extra, "unknown field"),
        (
            json!({"listingId":"lo-2","cropId":"kale","harvestedOn":day,"listedOz":0.0}),
            "listed_oz must be > 0",
        ),
        (
            json!({"listingId":"lo-3","cropId":"kale","harvestedOn":"today","listedOz":1.0}),
            "YYYY-MM-DD",
        ),
    ];
    for (payload, needle) in bad {
        let ev = EventRecord::originated(
            Kind::LeftoverListed,
            "leftover_listing",
            "lo-x".to_string(),
            payload,
            json!({ "op": "none" }),
            "2026-08-27T12:00:00.000Z".to_string(),
            None,
            None,
            None,
        );
        let tx = conn.transaction().unwrap();
        let err = events::write_event(&tx, &ev).unwrap_err();
        assert!(err.contains(needle), "{needle}: {err}");
        drop(tx);
    }
    assert_eq!(count_kind(&conn, "leftover.listed"), 0);
}

#[test]
fn lo_a_listing_replays_and_verify_passes() {
    let dir = temp_dir("verify");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    harvest_kale_today(&mut conn);
    let day = today();
    leftover::list_leftover(&mut conn, "kale", &day, 2.0).unwrap();
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
    let _ = fs::remove_dir_all(&dir);
}
