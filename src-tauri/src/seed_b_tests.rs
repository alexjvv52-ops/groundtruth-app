//! JAR-READER Job A — the jar's read side: what is still in it, per crop.
//!
//! What is pinned: a crop has a row only once it has a receipt; ounces out
//! are joined to the crop by id through the sow event, never by name, so a
//! rename after the sow changes nothing and a NULL sow_event_id row is
//! reported unattributed rather than guessed; the window opens at the first
//! receipt and earlier ounces are reported, not subtracted; one unweighed sow
//! in the window makes the figure unknown; an undone sow still subtracts and
//! is reported; short is printed, not clamped; every figure is at 0.1; the
//! read writes nothing, reads no clock, and leaves the farm replayable.
use crate::db;
use crate::event_file;
use crate::projection;
use crate::seed::{self, SeedOnHandRow};
use crate::trays;
use rusqlite::Connection;
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
    let dir = std::env::temp_dir().join(format!("groundtruth-seed-b-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn count_table(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

/// The one row for `crop_id`, or a panic naming what the read returned.
fn row_for(conn: &Connection, crop_id: &str) -> SeedOnHandRow {
    let rows = seed::seed_on_hand(conn).unwrap();
    rows.iter()
        .find(|r| r.crop_id == crop_id)
        .cloned()
        .unwrap_or_else(|| panic!("no jar row for {crop_id}: {rows:?}"))
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
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
fn seed_b_receipt_minus_a_weighed_sow_after_it_is_the_jar() {
    let mut conn = mem();
    let receipt = seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(8.0)).unwrap();
    let rows = seed::seed_on_hand(&conn).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row.crop_id, "dun-peas");
    assert_eq!(row.crop_name, "Dun peas");
    assert!(near(row.received_oz, 16.0), "{row:?}");
    assert_eq!(row.since, receipt.created_at);
    assert!(near(row.sown_oz, 8.0), "{row:?}");
    assert_eq!(row.unweighed_sows, 0);
    assert_eq!(row.unweighed_trays, 0);
    assert!(near(row.undone_sown_oz, 0.0), "{row:?}");
    assert!(near(row.before_first_receipt_oz, 0.0), "{row:?}");
    assert!(near(row.unattributed_oz, 0.0), "{row:?}");
    assert_eq!(row.on_hand_oz, Some(8.0));
    // The wire shape is the locked camelCase field set; onHandOz is a number here.
    let wire = serde_json::to_value(row).unwrap();
    let mut keys: Vec<&str> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "beforeFirstReceiptOz",
            "cropId",
            "cropName",
            "onHandOz",
            "receivedOz",
            "since",
            "sownOz",
            "unattributedOz",
            "undoneSownOz",
            "unweighedSows",
            "unweighedTrays",
        ]
    );
    assert_eq!(wire["onHandOz"], serde_json::json!(8.0));
}

#[test]
fn seed_b_a_blank_sow_after_the_receipt_makes_the_figure_unknown() {
    let mut conn = mem();
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let row = row_for(&conn, "dun-peas");
    assert_eq!(row.on_hand_oz, None, "{row:?}");
    assert_eq!(row.unweighed_sows, 1);
    assert_eq!(row.unweighed_trays, 3);
    assert!(near(row.sown_oz, 0.0), "{row:?}");
    assert!(near(row.received_oz, 16.0), "{row:?}");
    // A weighed sow beside it is counted, and the figure stays unknown: the
    // blank sow is still in the window. Unknown, not zero, not the recipe.
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    let row = row_for(&conn, "dun-peas");
    assert_eq!(row.on_hand_oz, None, "{row:?}");
    assert_eq!(row.unweighed_sows, 1);
    assert_eq!(row.unweighed_trays, 3);
    assert!(near(row.sown_oz, 8.0), "{row:?}");
    let wire = serde_json::to_value(&row).unwrap();
    assert!(wire["onHandOz"].is_null());
}

#[test]
fn seed_b_an_undone_sow_still_subtracts_and_is_reported() {
    let mut conn = mem();
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    let sown = trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(8.0)).unwrap();
    let before = row_for(&conn, "dun-peas");
    assert_eq!(before.on_hand_oz, Some(8.0));
    assert!(near(before.undone_sown_oz, 0.0), "{before:?}");
    let undone = trays::undo_last(&mut conn).unwrap().expect("the sow");
    assert_eq!(undone.undone_kind, "tray.sown");
    let gone: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM trays WHERE id = ?1",
            [&sown.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(gone, 0, "the tray is gone; the oz-out row stands");
    // UNDONE A: the row stands and counts. The undone part is named beside it.
    let row = row_for(&conn, "dun-peas");
    assert!(near(row.sown_oz, 8.0), "{row:?}");
    assert!(near(row.undone_sown_oz, 8.0), "{row:?}");
    assert_eq!(row.on_hand_oz, Some(8.0));
    assert_eq!(row.unweighed_sows, 0);
}

#[test]
fn seed_b_a_sow_before_the_first_receipt_is_reported_not_subtracted() {
    let mut conn = mem();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    // Stamps are milliseconds; give the receipt a later one (lib.rs precedent).
    std::thread::sleep(std::time::Duration::from_millis(10));
    let receipt = seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    let first_sow: String = conn
        .query_row(
            "SELECT MIN(occurred_at) FROM consumption_events WHERE unit = 'oz'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        first_sow < receipt.created_at,
        "{first_sow} < {}",
        receipt.created_at
    );
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(4.0)).unwrap();
    // OPENING A: the window opens at the first receipt. Before it, reported only.
    let row = row_for(&conn, "dun-peas");
    assert_eq!(row.since, receipt.created_at);
    assert!(near(row.received_oz, 16.0), "{row:?}");
    assert!(near(row.sown_oz, 4.0), "{row:?}");
    assert!(near(row.before_first_receipt_oz, 8.0), "{row:?}");
    assert_eq!(row.on_hand_oz, Some(12.0));
    assert_eq!(row.unweighed_sows, 0);
}

#[test]
fn seed_b_a_rename_after_the_sow_keeps_the_ounces_by_id() {
    let mut conn = mem();
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(8.0)).unwrap();
    let renamed = trays::rename_crop(&mut conn, "dun-peas", "Peas, dun").unwrap();
    assert_eq!(renamed.name, "Peas, dun");
    // The oz-out row still carries the name it was written with.
    let rows = consumption_rows(&conn);
    let oz = rows.iter().find(|r| r.1 == "oz").expect("the oz-out row");
    assert_eq!(oz.0, "Dun peas");
    // The jar is joined by id: the ounces stay on the crop under its new name.
    let row = row_for(&conn, "dun-peas");
    assert_eq!(row.crop_name, "Peas, dun");
    assert!(near(row.sown_oz, 8.0), "{row:?}");
    assert_eq!(row.on_hand_oz, Some(8.0));
    assert!(near(row.unattributed_oz, 0.0), "{row:?}");
}

#[test]
fn seed_b_a_null_sow_event_id_row_is_unattributed_not_sown() {
    let mut conn = mem();
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    seed::receive_seed(&mut conn, "kale", 4.0).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(8.0)).unwrap();
    // A pre-v12 shape: oz out, no sow_event_id, the crop's own name on the row
    // (consumption_tests precedent). MATH A2: the name is never a fallback.
    conn.execute(
        "INSERT INTO consumption_events
         (event_id, origin, occurred_at, variety_or_item, unit, quantity,
          sow_event_id, linked_cost_event_id, notes)
         VALUES ('ev-prior-v11', 'farm_os', '2099-01-01T12:00:00.000Z', 'Dun peas',
                 'oz', 5.0, NULL, NULL, NULL)",
        [],
    )
    .unwrap();
    let peas = row_for(&conn, "dun-peas");
    assert!(near(peas.sown_oz, 8.0), "{peas:?}");
    assert!(near(peas.unattributed_oz, 5.0), "{peas:?}");
    assert_eq!(peas.on_hand_oz, Some(8.0));
    // Farm-wide: the same figure on every row, subtracted from none.
    let kale = row_for(&conn, "kale");
    assert!(near(kale.unattributed_oz, 5.0), "{kale:?}");
    assert!(near(kale.sown_oz, 0.0), "{kale:?}");
    assert_eq!(kale.on_hand_oz, Some(4.0));
}

#[test]
fn seed_b_the_read_writes_nothing_and_reads_no_clock() {
    let mut conn = mem();
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    trays::sow_tray(&mut conn, "kale", 1).unwrap();
    let events = count_table(&conn, "event_log");
    let consumption = consumption_rows(&conn);
    let receipts = count_table(&conn, "seed_receipts");
    let trays_before = count_table(&conn, "trays");
    let first = db::with_clock_forbidden(|| {
        projection::with_nondeterminism_forbidden(|| seed::seed_on_hand(&conn).unwrap())
    });
    let second = seed::seed_on_hand(&conn).unwrap();
    assert_eq!(first, second);
    assert_eq!(count_table(&conn, "event_log"), events);
    assert_eq!(consumption_rows(&conn), consumption);
    assert_eq!(count_table(&conn, "seed_receipts"), receipts);
    assert_eq!(count_table(&conn, "trays"), trays_before);
}

#[test]
fn seed_b_the_read_leaves_the_farm_replayable_and_verify_passes() {
    let dir = temp_dir("verify");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    seed::receive_seed(&mut conn, "kale", 4.0).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(8.0)).unwrap();
    trays::sow_tray(&mut conn, "kale", 1).unwrap();
    trays::undo_last(&mut conn).unwrap().expect("the blank sow");
    let rows = seed::seed_on_hand(&conn).unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
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
fn seed_b_a_crop_without_a_receipt_has_no_row() {
    let mut conn = mem();
    assert!(seed::seed_on_hand(&conn).unwrap().is_empty());
    trays::sow_tray_with_seed(&mut conn, "kale", 1, Some(3.0)).unwrap();
    assert!(seed::seed_on_hand(&conn).unwrap().is_empty());
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    let rows = seed::seed_on_hand(&conn).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].crop_id, "dun-peas");
    assert!(rows.iter().all(|r| r.crop_id != "kale"));
    // Kale's ounces are kale's by id — not unattributed, just not reported.
    assert!(near(rows[0].unattributed_oz, 0.0), "{rows:?}");
    assert_eq!(rows[0].on_hand_oz, Some(16.0));
}

#[test]
fn seed_b_every_figure_is_at_a_tenth_and_short_is_printed() {
    let mut conn = mem();
    seed::receive_seed(&mut conn, "dun-peas", 0.2).unwrap();
    seed::receive_seed(&mut conn, "dun-peas", 0.1).unwrap();
    for _ in 0..3 {
        trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(0.1)).unwrap();
    }
    // 0.1 + 0.1 + 0.1 is not 0.3 in f64; at a tenth it is.
    assert_ne!(0.1_f64 + 0.1 + 0.1, 0.3);
    let row = row_for(&conn, "dun-peas");
    assert_eq!(row.received_oz, 0.3, "{row:?}");
    assert_eq!(row.sown_oz, 0.3, "{row:?}");
    assert_eq!(row.on_hand_oz, Some(0.0), "{row:?}");
    // NEGATIVE A: one more tenth out and the jar reads short, not clamped.
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(0.1)).unwrap();
    let row = row_for(&conn, "dun-peas");
    assert_eq!(row.sown_oz, 0.4, "{row:?}");
    assert_eq!(row.on_hand_oz, Some(-0.1), "{row:?}");
    assert_eq!(
        serde_json::to_value(&row).unwrap()["onHandOz"],
        serde_json::json!(-0.1)
    );
}
