//! R3 harvest ↔ order link (GT-D19): informational commitments + optional durable mark.
use crate::db;
use crate::event_file;
use crate::event_partition::{marketing_kinds, EventDomain, MARKETING_KINDS};
use crate::events::{self, EventRecord, Kind};
use crate::identity::is_farm_truth;
use crate::marketing::{self, CoverageRef, HARVEST_COVERED_PAYLOAD_FIELD_NAMES};
use crate::projection::{self, EXCLUSION_LIST};
use crate::trays;
use crate::wholesale::{self, OrderLine};
use rusqlite::Connection;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
fn mem() -> Connection {
    db::open_in_memory().unwrap()
}
fn today() -> String {
    db::local_date_today()
}
fn add_days(d: &str, n: i64) -> String {
    (chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap() + chrono::Duration::days(n))
        .format("%Y-%m-%d")
        .to_string()
}
fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-r3-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}
fn venue(conn: &mut Connection, name: &str) -> marketing::VenueView {
    marketing::record_venue(
        conn,
        name,
        "cafe",
        Some("Sam".into()),
        None,
        Some("1 Main St".into()),
        None,
    )
    .unwrap()
}
fn standing(conn: &mut Connection, venue_id: &str, targets: &[(&str, i64)]) {
    let mut m = BTreeMap::new();
    for (k, n) in targets {
        m.insert(k.to_string(), *n);
    }
    let total: i64 = targets.iter().map(|(_, n)| n).sum();
    marketing::change_stage(
        conn,
        venue_id,
        "standing",
        &today(),
        Some(total),
        Some(targets.iter().map(|(k, _)| k.to_string()).collect()),
        Some(m),
        None,
    )
    .unwrap();
}
fn standing_unsplit(conn: &mut Connection, venue_id: &str, names: &[&str], total: i64) {
    marketing::change_stage(
        conn,
        venue_id,
        "standing",
        &today(),
        Some(total),
        Some(names.iter().map(|s| s.to_string()).collect()),
        None,
        None,
    )
    .unwrap();
}
fn order(
    conn: &mut Connection,
    venue_id: &str,
    crop_id: &str,
    harvest_date: &str,
    trays: i64,
) -> String {
    wholesale::record_order(
        conn,
        venue_id,
        harvest_date,
        vec![OrderLine {
            crop_id: crop_id.into(),
            trays,
            price_cents_per_tray: Some(500),
        }],
        true,
    )
    .unwrap()
    .id
}
fn harvest_kale_today(conn: &mut Connection) -> Vec<String> {
    let mut ids = Vec::new();
    for _ in 0..2 {
        let t = trays::sow_tray(conn, "kale", 1).unwrap();
        trays::advance_trays(conn, std::slice::from_ref(&t.id)).unwrap();
        ids.push(t.id);
    }
    trays::harvest_trays(conn, &ids, 6.0).unwrap();
    ids
}
fn cov(kind: &str, id: &str) -> CoverageRef {
    CoverageRef {
        kind: kind.into(),
        id: id.into(),
    }
}
fn line_texts(v: &marketing::HarvestCommitmentsView) -> Vec<String> {
    v.lines.iter().map(|l| l.text.clone()).collect()
}
#[test]
fn r3a_kind_in_closed_set_table_compared_payload_sealed() {
    assert_eq!(Kind::ALL.len(), 54);
    let k = Kind::parse("harvest.covered").unwrap();
    assert_eq!(k, Kind::HarvestCovered);
    assert_eq!(k.as_str(), "harvest.covered");
    assert_eq!(k.tier(), (EventDomain::Marketing, None));
    assert!(MARKETING_KINDS.contains(&"harvest.covered"));
    assert!(marketing_kinds().contains(&"harvest.covered"));
    assert!(!is_farm_truth(k));
    assert_eq!(
        HARVEST_COVERED_PAYLOAD_FIELD_NAMES,
        &["coverage_id", "crop_id", "harvested_on", "covers"]
    );
    let conn = mem();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='harvest_coverage'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
    assert!(
        EXCLUSION_LIST
            .iter()
            .all(|l| !l.contains("harvest_coverage")),
        "compared, not excluded"
    );
    // Sealed: unknown field, bad kind, money key all refused; nothing written.
    let mut conn = conn;
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();
    for payload in [
        json!({"coverageId":"c","cropId":"kale","harvestedOn":"2026-08-17","covers":[],"extra":1}),
        json!({"coverageId":"c","cropId":"kale","harvestedOn":"2026-08-17","covers":[{"kind":"retail","id":"x"}]}),
        json!({"coverageId":"c","cropId":"kale","harvestedOn":"2026-08-17","covers":[],"amount":5}),
        json!({"coverageId":"c","cropId":"","harvestedOn":"2026-08-17","covers":[]}),
        json!({"coverageId":"c","cropId":"kale","harvestedOn":"yesterday","covers":[]}),
    ] {
        let ev = EventRecord::originated(
            Kind::HarvestCovered,
            "harvest_coverage",
            "c".to_string(),
            payload,
            json!({"op":"none"}),
            "2026-08-17T20:00:00.000Z".to_string(),
            None,
            None,
            Some(projection::handler_new_id()),
        );
        let tx = conn.transaction().unwrap();
        assert!(events::write_event(&tx, &ev).is_err());
        std::mem::drop(tx);
    }
    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
}
#[test]
fn r3b_commitments_lines_exact_bytes_window_and_exclusions() {
    let mut conn = mem();
    let t = today();
    let a = venue(&mut conn, "Alder Cafe");
    let b = venue(&mut conn, "Birch Bistro");
    let c = venue(&mut conn, "Cedar Bar");
    let d = venue(&mut conn, "Dogwood Deli");
    standing(&mut conn, &a.venue_id, &[("Kale", 3)]);
    standing_unsplit(&mut conn, &b.venue_id, &["Kale", "Sunflower"], 4);
    standing(&mut conn, &c.venue_id, &[("Sunflower", 2)]);
    let _ = d;
    let soon = order(&mut conn, &d.venue_id, "kale", &add_days(&t, 2), 3);
    let overdue = order(&mut conn, &a.venue_id, "kale", &add_days(&t, -1), 1);
    let _far = order(&mut conn, &b.venue_id, "kale", &add_days(&t, 5), 2);
    let delivered = order(&mut conn, &c.venue_id, "kale", &t, 2);
    wholesale::deliver_order(&mut conn, &delivered, Some(t.clone())).unwrap();
    let _sun = order(&mut conn, &c.venue_id, "sunflower", &t, 1);
    let v = marketing::harvest_commitments_on(&conn, "kale", &t).unwrap();
    assert_eq!(v.header, "Committed for Kale:");
    assert_eq!(v.empty_line, "No open commitments for Kale.");
    let when_soon = crate::reachability::format_mon_d_local(&add_days(&t, 2)).unwrap();
    let when_over = crate::reachability::format_mon_d_local(&add_days(&t, -1)).unwrap();
    assert_eq!(
        line_texts(&v),
        vec![
            "Alder Cafe · standing 3/week".to_string(),
            "Birch Bistro · standing, not split by variety".to_string(),
            format!("Alder Cafe · order {when_over} · 1 tray · overdue"),
            format!("Dogwood Deli · order {when_soon} · 3 trays"),
        ]
    );
    assert!(v.lines.iter().all(|l| !l.covered));
    assert_eq!(v.lines[2].id, overdue);
    assert_eq!(v.lines[3].id, soon);
    assert_eq!(v.lines[0].kind, "standing");
    assert_eq!(v.lines[3].kind, "wholesale");
    let s = marketing::harvest_commitments_on(&conn, "sunflower", &t).unwrap();
    assert_eq!(
        line_texts(&s).len(),
        3,
        "Birch unsplit + Cedar 2/week + Cedar order"
    );
    assert!(line_texts(&s).contains(&"Cedar Bar · standing 2/week".to_string()));
    let none = marketing::harvest_commitments_on(&conn, "dun-peas", &t).unwrap();
    assert!(none.lines.is_empty());
    assert_eq!(none.empty_line, "No open commitments for Dun peas.");
    assert!(marketing::harvest_commitments_on(&conn, "no-such-crop", &t).is_err());
}
#[test]
fn r3c_mark_is_durable_hidden_without_harvest_undoable_and_replays() {
    let dir = temp_dir("r3c");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let t = today();
    let a = venue(&mut conn, "Alder Cafe");
    standing(&mut conn, &a.venue_id, &[("Kale", 3)]);
    let ord = order(&mut conn, &a.venue_id, "kale", &t, 2);
    // A mark before any harvest today is recorded but hidden.
    let v0 =
        marketing::record_harvest_coverage(&mut conn, "kale", vec![cov("standing", &a.venue_id)])
            .unwrap();
    assert!(
        v0.lines.iter().all(|l| !l.covered),
        "hidden: no harvest of kale today yet"
    );
    harvest_kale_today(&mut conn);
    let v1 = marketing::harvest_commitments(&conn, "kale").unwrap();
    assert!(
        v1.lines
            .iter()
            .find(|l| l.kind == "standing")
            .unwrap()
            .covered
    );
    // Mark the order too; then undo the standing mark by writing the reduced list.
    let v2 = marketing::record_harvest_coverage(
        &mut conn,
        "kale",
        vec![cov("standing", &a.venue_id), cov("wholesale", &ord)],
    )
    .unwrap();
    assert!(v2.lines.iter().all(|l| l.covered));
    let v3 = marketing::record_harvest_coverage(&mut conn, "kale", vec![cov("wholesale", &ord)])
        .unwrap();
    assert!(
        !v3.lines
            .iter()
            .find(|l| l.kind == "standing")
            .unwrap()
            .covered
    );
    assert!(
        v3.lines
            .iter()
            .find(|l| l.kind == "wholesale")
            .unwrap()
            .covered
    );
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM harvest_coverage WHERE crop_id='kale'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 3, "append-only: three notes, newest wins");
    let err = conn
        .execute("DELETE FROM harvest_coverage", [])
        .unwrap_err()
        .to_string();
    assert!(err.contains("append-only"), "{err}");
    // Nothing else moved: standing target and the order are as they were.
    let demand = marketing::standing_demand(&conn).unwrap();
    assert!(demand
        .varieties
        .iter()
        .any(|x| x.name == "Kale" && x.trays_week == 3));
    let state: String = conn
        .query_row(
            "SELECT state FROM wholesale_orders WHERE id = ?1",
            [&ord],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "ordered");
    // Undo the harvest (undo steps over the marketing notes): marks hide, rows stay.
    trays::undo_last(&mut conn).unwrap();
    let harvested: i64 = conn.query_row(
        "SELECT COUNT(*) FROM trays WHERE crop_id='kale' AND state='harvested' AND harvested_on=?1", [&t], |r| r.get(0)).unwrap();
    assert_eq!(harvested, 0);
    let v4 = marketing::harvest_commitments(&conn, "kale").unwrap();
    assert!(v4.lines.iter().all(|l| !l.covered), "hidden after undo");
    let rows_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM harvest_coverage WHERE crop_id='kale'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows_after, 3);
    // Handler refusals.
    assert!(marketing::record_harvest_coverage(&mut conn, "no-such-crop", vec![]).is_err());
    assert!(
        marketing::record_harvest_coverage(&mut conn, "kale", vec![cov("retail", "x")]).is_err()
    );
    assert!(marketing::record_harvest_coverage(
        &mut conn,
        "kale",
        vec![cov("standing", "no-venue")]
    )
    .is_err());
    assert!(marketing::record_harvest_coverage(
        &mut conn,
        "kale",
        vec![cov("wholesale", "no-order")]
    )
    .is_err());
    // Replay reproduces every note; the table is compared.
    event_file::flush_events(&conn, &dir).unwrap();
    std::mem::drop(conn);
    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    let _ = fs::remove_dir_all(&dir);
}
#[test]
fn r3d_v31_triggers_refuse_the_kind_until_the_v32_reinstall() {
    const MARKETING_KINDS_V31: &[&str] = &[
        "venue.recorded",
        "venue.corrected",
        "venue.archived",
        "sample.dropped",
        "touch.logged",
        "followup.set",
        "followup.cleared",
        "stage.changed",
        "reviews.observed",
        "review.requested",
        "standing.requested",
        "standing.request_decided",
    ];
    let conn = mem();
    db::drop_event_log_triggers(&conn).unwrap();
    conn.execute_batch(&crate::event_partition::schema_event_log_triggers_sql(
        &crate::event_partition::grow_kinds(),
        &crate::event_partition::register_kinds(),
        crate::event_partition::EVENT_CLASSES,
        MARKETING_KINDS_V31,
    ))
    .unwrap();
    conn.pragma_update(None, "user_version", 31).unwrap();
    let ev = EventRecord::originated(
        Kind::HarvestCovered,
        "harvest_coverage",
        "c1".to_string(),
        json!({"coverageId":"c1","cropId":"kale","harvestedOn":"2026-08-17","covers":[]}),
        json!({"op":"none"}),
        "2026-08-17T20:00:00.000Z".to_string(),
        None,
        None,
        Some("ev-c1".to_string()),
    );
    let mut conn = conn;
    let tx = conn.transaction().unwrap();
    let err = events::write_event(&tx, &ev).unwrap_err();
    assert!(err.contains("kind invalid for marketing"), "{err}");
    std::mem::drop(tx);
    db::migrate(&conn).unwrap();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &ev).unwrap();
    events::write_event(&tx, &ev).unwrap();
    tx.commit().unwrap();
}
