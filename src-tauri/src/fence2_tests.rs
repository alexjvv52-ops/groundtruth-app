//! Fence II — findings 12, 14, 10, 22, 23.

use crate::attention;
use crate::db;
use crate::event_file;
use crate::events::{self, EventRecord, Kind};
use crate::export::{self, Manifest};
use crate::import;
use crate::projection;
use crate::trays;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn mem() -> Connection {
    db::open_in_memory().expect("open in-memory db")
}

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-f2-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn open_farm(dir: &Path) -> Connection {
    db::open_and_migrate(&dir.join("farm.db")).unwrap()
}

fn flush(conn: &Connection, dir: &Path) {
    event_file::flush_events(conn, dir).unwrap();
}

fn insert_snapshot_taken(conn: &mut Connection, n: usize) {
    for i in 0..n {
        let now = db::utc_now_rfc3339();
        let event = EventRecord::originated(
            Kind::SnapshotTaken,
            "snapshot",
            format!("snap-{i}"),
            json!({ "path": format!("farm-snap-{i}.db") }),
            json!({ "op": "none" }),
            now,
            None,
            None,
            None,
        );
        let tx = conn.transaction().unwrap();
        projection::apply_event(&tx, &event).unwrap();
        events::write_event(&tx, &event).unwrap();
        tx.commit().unwrap();
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn remanifest(bundle: &Path) {
    let man_path = bundle.join("manifest.json");
    let mut manifest: Manifest =
        serde_json::from_str(&fs::read_to_string(&man_path).unwrap()).unwrap();
    let mut files = Vec::new();
    for entry in &manifest.files {
        let abs = bundle.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let bytes = fs::read(&abs).unwrap();
        files.push(crate::export::ManifestFile {
            path: entry.path.clone(),
            size_bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    manifest.files = files;
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    fs::write(&man_path, format!("{json}\n")).unwrap();
}

fn rewrite_events_line(bundle: &Path, mutator: impl FnOnce(&mut Vec<Value>)) {
    let path = bundle.join("events.jsonl");
    let text = fs::read_to_string(&path).unwrap();
    let mut lines: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    mutator(&mut lines);
    let mut out = String::new();
    for v in lines {
        out.push_str(&serde_json::to_string(&v).unwrap());
        out.push('\n');
    }
    fs::write(&path, out).unwrap();
    remanifest(bundle);
}

fn undo_event(reverses_event_id: Option<&str>) -> EventRecord {
    EventRecord::originated(
        Kind::Undo,
        "event",
        "1",
        json!({ "undoesSeq": 1, "undoneKind": "tray.sown" }),
        json!({ "op": "none" }),
        db::utc_now_rfc3339(),
        Some(1),
        reverses_event_id,
        None,
    )
}

fn event_count(conn: &Connection) -> i64 {
    trays::count_event_log(conn).unwrap()
}

fn tray_count(conn: &Connection) -> usize {
    trays::list_trays(conn).unwrap().len()
}

fn planting_rows(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM consumption_events WHERE unit = 'planting'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn sow_event_id(conn: &Connection, tray_id: &str) -> String {
    conn.query_row(
        "SELECT id FROM event_log WHERE kind = 'tray.sown' AND entity_id = ?1",
        [tray_id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn f2_12_advance_refuses_a_tray_already_in_light() {
    let mut conn = mem();
    let tray = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let id = tray.id.clone();
    trays::advance_trays(&mut conn, std::slice::from_ref(&id)).unwrap();
    let lit = trays::get_tray(&conn, &id).unwrap();
    assert_eq!(lit.state, "light");

    let before = trays::count_event_kind(&conn, "trays.advanced").unwrap();
    let err = trays::advance_trays(&mut conn, std::slice::from_ref(&id)).unwrap_err();
    assert!(
        err.contains("already under light"),
        "expected already-under-light refusal, got: {err}"
    );

    let still = trays::get_tray(&conn, &id).unwrap();
    assert_eq!(still.state, "light");
    assert!(still.harvested_on.is_none());
    assert!(still.actual_yield_oz.is_none());
    assert_eq!(
        trays::count_event_kind(&conn, "trays.advanced").unwrap(),
        before,
        "a refused advance must not write trays.advanced"
    );
}

#[test]
fn f2_12_a_double_tap_leaves_the_trays_harvestable() {
    let mut conn = mem();
    let tray = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let id = tray.id.clone();
    // Cover check at sown+3; harvest at sown+9. Shift so both doors have arrived.
    trays::test_shift_dates(&mut conn, &id, -9).unwrap();

    trays::advance_trays(&mut conn, std::slice::from_ref(&id)).unwrap();
    let err = trays::advance_trays(&mut conn, std::slice::from_ref(&id)).unwrap_err();
    assert!(
        err.contains("already under light"),
        "second advance must refuse, got: {err}"
    );

    let view = trays::today_view(&conn).unwrap();
    assert!(
        view.harvests.iter().any(|g| g.tray_ids.contains(&id)),
        "today_view must still offer the tray in a harvest group"
    );

    let planting_before = planting_rows(&conn);
    trays::harvest_trays(&mut conn, std::slice::from_ref(&id), 12.5).unwrap();
    let harvested = trays::get_tray(&conn, &id).unwrap();
    assert_eq!(harvested.state, "harvested");
    assert!((harvested.actual_yield_oz.unwrap() - 12.5).abs() < f64::EPSILON);
    assert_eq!(planting_rows(&conn), planting_before + 1);
}

#[test]
fn f2_14_move_now_leaves_todays_sow_under_cover() {
    let mut conn = mem();
    let old = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    trays::test_shift_dates(&mut conn, &old.id, -6).unwrap();
    let today_sow = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();

    let items = attention::check_attention(&conn).unwrap();
    let card = items
        .iter()
        .find(|i| i.kind == "tray.overdue_light")
        .expect("overdue light card");
    let result = attention::resolve_attention(&mut conn, &card.id, "move_now").unwrap();
    let today = db::local_date_today();
    let due = trays::due_for_light_ids_for_crop(&conn, "dun-peas", &today).unwrap();
    assert_eq!(due, vec![old.id.clone()]);
    assert_eq!(result.tray_ids, vec![old.id.clone()]);

    trays::advance_trays(&mut conn, &result.tray_ids).unwrap();
    let moved = trays::get_tray(&conn, &old.id).unwrap();
    assert_eq!(moved.state, "light");
    let still = trays::get_tray(&conn, &today_sow.id).unwrap();
    assert_eq!(still.state, "blackout");
    assert!(still.light_on.is_none());
}

#[test]
fn f2_10_an_imported_undo_reverses_the_event_it_named() {
    let a_dir = temp_dir("10-a");
    let mut a = open_farm(&a_dir);
    let x = trays::sow_tray(&mut a, "dun-peas", 1).unwrap();
    let y = trays::sow_tray(&mut a, "sunflower", 1).unwrap();
    let y_sow_id = sow_event_id(&a, &y.id);
    trays::undo_last(&mut a).unwrap();
    flush(&a, &a_dir);
    let a_events = event_count(&a);
    let exported = export::export_bundle(&a, &a_dir).unwrap();
    let bundle = PathBuf::from(exported.bundle_path);

    let b_dir = temp_dir("10-b");
    let mut b = open_farm(&b_dir);
    insert_snapshot_taken(&mut b, 1);
    let b_pre = event_count(&b);
    assert_eq!(b_pre, 1, "launch snapshot.taken sits at seq 1");

    import::apply_import(&mut b, &bundle).unwrap();
    assert_eq!(event_count(&b), a_events + b_pre);

    assert!(trays::get_tray(&b, &x.id).is_ok(), "X's tray exists in B");
    assert!(
        trays::get_tray(&b, &y.id).is_err(),
        "Y's tray must not exist in B"
    );

    let (undone_id, undone_kind): (String, String) = b
        .query_row(
            "SELECT id, kind FROM event_log WHERE undone_at IS NOT NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(undone_id, y_sow_id);
    assert_eq!(undone_kind, "tray.sown");
    let at_stored_ordinal: Option<String> = b
        .query_row(
            "SELECT id FROM event_log WHERE seq = (
                 SELECT undoes_seq FROM event_log WHERE kind = 'undo' LIMIT 1
             )",
            [],
            |r| r.get(0),
        )
        .ok();
    assert_eq!(
        at_stored_ordinal.as_deref(),
        Some(y_sow_id.as_str()),
        "Y's sow sits at the stored undoes_seq on B (INT-010: this farm's ordinal)"
    );

    flush(&b, &b_dir);
    let outcome = projection::verify_replay_paths(
        &b_dir.join("farm.db"),
        &event_file::events_path(&b_dir),
        &b_dir,
    )
    .unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify_replay_paths must PASS, got {}",
        outcome.summary_line()
    );

    let _ = fs::remove_dir_all(&a_dir);
    let _ = fs::remove_dir_all(&b_dir);
}

#[test]
fn f2_10_an_undo_that_names_nothing_is_refused() {
    let mut conn = mem();
    {
        let tx = conn.transaction().unwrap();
        let err = trays::apply_undo(&tx, &undo_event(None)).unwrap_err();
        assert!(
            err.contains("Ruling 4"),
            "unnamed undo must name Ruling 4, got: {err}"
        );
        tx.rollback().unwrap();
    }
    {
        let tx = conn.transaction().unwrap();
        let err = trays::apply_undo(&tx, &undo_event(Some("no-such-event"))).unwrap_err();
        assert!(
            err.contains("undo names an event this farm does not have"),
            "absent target must refuse by name, got: {err}"
        );
        tx.rollback().unwrap();
    }

    // Import-shaped transaction: a sow is applied, then a nameless undo fails
    // and the whole transaction rolls back — nothing half-applied.
    let trays_before = tray_count(&conn);
    let events_before = event_count(&conn);
    {
        let sow = EventRecord::originated(
            Kind::TraySown,
            "tray",
            "tray-import-undo-refuse",
            json!({
                "cropId": "dun-peas",
                "quantity": 1,
                "sownOn": "2026-08-06",
                "blackoutOn": "2026-08-06"
            }),
            json!({ "op": "delete_tray", "trayId": "tray-import-undo-refuse" }),
            "2026-08-06T00:00:00.000Z",
            None,
            None,
            Some("sow-for-rollback".into()),
        );
        let tx = conn.transaction().unwrap();
        projection::apply_event(&tx, &sow).unwrap();
        events::write_imported_event(&tx, &sow).unwrap();
        let err = projection::apply_event(&tx, &undo_event(None)).unwrap_err();
        assert!(err.contains("Ruling 4"), "got: {err}");
        tx.rollback().unwrap();
    }
    assert_eq!(tray_count(&conn), trays_before);
    assert_eq!(event_count(&conn), events_before);

    let a_dir = temp_dir("10-refuse-a");
    let mut a = open_farm(&a_dir);
    trays::sow_tray(&mut a, "dun-peas", 1).unwrap();
    trays::sow_tray(&mut a, "sunflower", 1).unwrap();
    trays::undo_last(&mut a).unwrap();
    flush(&a, &a_dir);
    let exported = export::export_bundle(&a, &a_dir).unwrap();
    let bundle = PathBuf::from(exported.bundle_path);
    rewrite_events_line(&bundle, |lines| {
        for line in lines {
            if line.get("kind").and_then(|k| k.as_str()) == Some("undo") {
                line["reverses_event_id"] = Value::Null;
            }
        }
    });

    let b_dir = temp_dir("10-refuse-b");
    let mut b = open_farm(&b_dir);
    let events_b = event_count(&b);
    let trays_b = tray_count(&b);
    let err = import::apply_import(&mut b, &bundle).unwrap_err();
    assert!(!err.is_empty(), "import of an unnamed undo must refuse");
    assert_eq!(event_count(&b), events_b);
    assert_eq!(tray_count(&b), trays_b);

    let _ = fs::remove_dir_all(&a_dir);
    let _ = fs::remove_dir_all(&b_dir);
}

#[test]
fn f2_23_a_dismissed_tray_card_holds_for_the_day_and_returns_when_the_fact_moves() {
    let mut conn = mem();
    let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
    trays::test_shift_dates(&mut conn, &t.id, -13).unwrap();

    let a = attention::check_attention(&conn).unwrap();
    let card = a
        .iter()
        .find(|i| i.kind == "tray.overdue_harvest")
        .expect("overdue harvest card");
    assert!(card.message.contains("4 days"));
    attention::dismiss_attention(&mut conn, &card.id).unwrap();

    let held = attention::check_attention(&conn).unwrap();
    assert!(
        held.iter().all(|i| i.kind != "tray.overdue_harvest"),
        "episode must hold for the day"
    );

    trays::test_shift_dates(&mut conn, &t.id, -1).unwrap();
    let again = attention::check_attention(&conn).unwrap();
    let reopened = again
        .iter()
        .find(|i| i.kind == "tray.overdue_harvest")
        .expect("card returns when the fact moves");
    assert!(reopened.message.contains("5 days"));
    assert_ne!(reopened.id, card.id);
}

#[test]
fn f2_23_undo_of_a_dismiss_reopens_the_card_without_a_constraint_error() {
    let mut conn = mem();
    let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
    trays::test_shift_dates(&mut conn, &t.id, -13).unwrap();

    let a = attention::check_attention(&conn).unwrap();
    let id = a
        .iter()
        .find(|i| i.kind == "tray.overdue_harvest")
        .expect("overdue harvest card")
        .id
        .clone();
    attention::dismiss_attention(&mut conn, &id).unwrap();

    let undone = trays::undo_last(&mut conn).unwrap();
    assert!(undone.is_some(), "undo_last must return Some");
    let open = attention::check_attention(&conn).unwrap();
    assert!(
        open.iter()
            .any(|i| i.id == id && i.kind == "tray.overdue_harvest"),
        "the same attention id must be open again"
    );

    attention::dismiss_attention(&mut conn, &id).unwrap();
    attention::raise(
        &conn,
        "tray.overdue_harvest",
        Some("crop"),
        Some("dun-peas"),
        "2 trays of Dun peas were ready to harvest 4 days ago.",
        &["harvest_now", "dismiss"],
    )
    .unwrap();
    let colliding: String = conn
        .query_row(
            "SELECT id FROM attention
             WHERE kind = 'tray.overdue_harvest' AND resolved_at IS NULL AND id <> ?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();

    {
        let tx = conn.transaction().unwrap();
        attention::reopen_attention(&tx, &id).unwrap();
        tx.commit().unwrap();
    }
    let still_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention WHERE id = ?1 AND resolved_at IS NULL",
            [&colliding],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(still_open, 1, "the open row must still be standing");
    let dismissed_stays: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention WHERE id = ?1 AND resolved_at IS NOT NULL",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dismissed_stays, 1);
}
