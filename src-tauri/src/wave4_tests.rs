//! Wave 4 — GT-D2 lineage, authorised cutover import, drill honesty.

use crate::attention;
use crate::db;
use crate::event_file;
use crate::events::{self, EventRecord, Kind};
use crate::export;
use crate::identity::{is_farm_truth, LOCK_ACTIVE_IN_TEST};
use crate::import::{self, ImportRefusal};
use crate::marketing;
use crate::projection;
use crate::trays;
use chrono::NaiveDate;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "groundtruth-wave4-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn seed_farm_bundle(label: &str) -> (PathBuf, PathBuf) {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let dir = temp_dir(label);
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    event_file::flush_events(&conn, &dir).unwrap();
    let result = export::export_bundle(&conn, &dir).unwrap();
    (dir, PathBuf::from(result.bundle_path))
}

fn insert_snapshot_taken(conn: &mut Connection) {
    let now = db::utc_now_rfc3339();
    let event = EventRecord::originated(
        Kind::SnapshotTaken,
        "snapshot",
        "snap-w4".to_string(),
        json!({ "path": "farm-snap-w4.db" }),
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

/// Marketing history plus the F2 trap row (attention.resolved) and a snapshot.
fn marketing_only_with_trap(label: &str) -> (PathBuf, Connection) {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));
    let dir = temp_dir(label);
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    marketing::set_followup(&mut conn, &venue.venue_id, "2020-01-01", "call back").unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();
    let items = attention::check_attention(&conn).unwrap();
    let due = items
        .iter()
        .find(|i| i.kind == "marketing.followup_due")
        .expect("overdue follow-up attention");
    marketing::resolve_followup(&mut conn, &due.id, "visit", None).unwrap();
    insert_snapshot_taken(&mut conn);

    let resolved: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE kind = 'attention.resolved'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(resolved >= 1, "F2 trap requires attention.resolved");
    let farm_truth: i64 = conn
        .prepare("SELECT kind FROM event_log")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .filter(|k| Kind::parse(k).map(is_farm_truth).unwrap_or(true))
        .count() as i64;
    assert_eq!(
        farm_truth, 0,
        "marketing-only target must hold no farm truth"
    );
    event_file::flush_events(&conn, &dir).unwrap();
    (dir, conn)
}

#[test]
fn w1_marketing_only_plus_trap_imports_farm_without_different_farm() {
    let (_src, bundle) = seed_farm_bundle("w1-src");
    let (target_dir, conn) = marketing_only_with_trap("w1-tgt");

    let plan = import::preview_import(&conn, &bundle).unwrap();
    assert!(
        !plan
            .refusals
            .iter()
            .any(|r| matches!(r, ImportRefusal::DifferentFarm { .. })),
        "GT-D2: marketing + attention.resolved must not look like a different farm: {:?}",
        plan.explanations
    );
    assert!(plan.can_apply, "{:?}", plan.explanations);

    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    drop(conn);
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn w2_real_farm_truth_still_refuses_different_farm() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let (_src_a, _bundle_a) = seed_farm_bundle("w2-a");
    let (_src_b, bundle_b) = seed_farm_bundle("w2-b");

    let target_dir = temp_dir("w2-tgt");
    let mut conn = db::open_and_migrate(&target_dir.join("farm.db")).unwrap();
    trays::sow_tray_with_seed(&mut conn, "sunflower", 1, Some(5.0)).unwrap();
    event_file::flush_events(&conn, &target_dir).unwrap();

    let plan = import::preview_import(&conn, &bundle_b).unwrap();
    let diff = plan
        .refusals
        .iter()
        .find(|r| matches!(r, ImportRefusal::DifferentFarm { .. }));
    assert!(
        diff.is_some(),
        "expected DifferentFarm, got {:?}",
        plan.refusals
    );
    assert!(
        plan.explanations.iter().any(|s| s
            .contains("This looks like a different farm's records. Two farms cannot be merged.")),
        "{:?}",
        plan.explanations
    );

    drop(conn);
    let _ = fs::remove_dir_all(&_src_a);
    let _ = fs::remove_dir_all(&_src_b);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn w4_cutover_import_refuses_each_gate_with_its_own_sentence() {
    let (farm_src, farm_bundle) = seed_farm_bundle("w4-farm");

    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));
    let mkt_src = temp_dir("w4-mkt-src");
    let mut mkt_conn = db::open_and_migrate(&mkt_src.join("farm.db")).unwrap();
    marketing::record_venue(&mut mkt_conn, "Cafe", "cafe", None, None, None, None).unwrap();
    event_file::flush_events(&mkt_conn, &mkt_src).unwrap();
    let mkt_bundle = PathBuf::from(
        export::export_bundle(&mkt_conn, &mkt_src)
            .unwrap()
            .bundle_path,
    );
    drop(mkt_conn);

    let target_dir = temp_dir("w4-tgt");
    let mut conn = db::open_and_migrate(&target_dir.join("farm.db")).unwrap();
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));

    let wrong = import::apply_cutover_import(&mut conn, &farm_bundle, "cutover").unwrap_err();
    assert_eq!(
        wrong,
        "Type CUTOVER exactly to bring in the Farm OS cutover bundle."
    );

    // Dirty preview: target already has farm truth from a different farm.
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    trays::sow_tray_with_seed(&mut conn, "sunflower", 1, Some(5.0)).unwrap();
    event_file::flush_events(&conn, &target_dir).unwrap();
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));
    let dirty = import::apply_cutover_import(&mut conn, &farm_bundle, "CUTOVER").unwrap_err();
    assert_eq!(
        dirty,
        "Cutover import refused: the bundle preview is not clean."
    );

    // Fresh empty target + marketing-only bundle → no farm truth.
    let empty_dir = temp_dir("w4-empty");
    let mut empty = db::open_and_migrate(&empty_dir.join("farm.db")).unwrap();
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));
    let no_farm = import::apply_cutover_import(&mut empty, &mkt_bundle, "CUTOVER").unwrap_err();
    assert_eq!(
        no_farm,
        "This bundle has no farm truth. Use Bring in a bundle instead."
    );

    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    drop(conn);
    drop(empty);
    let _ = fs::remove_dir_all(&farm_src);
    let _ = fs::remove_dir_all(&mkt_src);
    let _ = fs::remove_dir_all(&target_dir);
    let _ = fs::remove_dir_all(&empty_dir);
}

#[test]
fn w6_cutover_receipt_names_manifest_sha256() {
    let (_src, bundle) = seed_farm_bundle("w6-src");
    let target_dir = temp_dir("w6-tgt");
    let mut conn = db::open_and_migrate(&target_dir.join("farm.db")).unwrap();

    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));
    import::apply_cutover_import(&mut conn, &bundle, "CUTOVER").unwrap();

    let receipt_path = target_dir.join("cutover-import.txt");
    assert!(receipt_path.is_file(), "receipt missing");
    let text = fs::read_to_string(&receipt_path).unwrap();
    let manifest_bytes = fs::read(bundle.join("manifest.json")).unwrap();
    let expected = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&manifest_bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    assert!(
        text.contains(&expected),
        "receipt must name manifest SHA-256 {expected}:\n{text}"
    );
    assert!(text.contains("manifest_sha256="), "{text}");

    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    drop(conn);
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn w7_drill_honesty_results_table_is_filled() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/dead-laptop-drill.md");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("docs/dead-laptop-drill.md missing: {e}"));

    let mut in_results = false;
    let mut rows: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.trim() == "## Results" {
            in_results = true;
            continue;
        }
        if in_results && line.starts_with("## ") {
            break;
        }
        if !in_results {
            continue;
        }
        let t = line.trim();
        if !t.starts_with('|') || t.contains("---") || t.contains("field") {
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(|c| c.trim()).collect();
        if cells.len() >= 2 {
            rows.push((cells[0].to_string(), cells[1].to_string()));
        }
    }
    assert!(!rows.is_empty(), "Results table has no data rows");

    for (field, value) in &rows {
        let lower = value.to_lowercase();
        assert!(
            !value.is_empty(),
            "Results row '{field}' has an empty value cell"
        );
        assert!(
            !value.contains("NOT YET RUN"),
            "Results row '{field}' still says NOT YET RUN"
        );
        assert!(
            lower != "n/a" && !lower.contains("n/a"),
            "Results row '{field}' contains n/a"
        );
        assert!(
            lower != "tbd" && !lower.contains("tbd"),
            "Results row '{field}' contains TBD"
        );
    }

    // Bumped 2026-08-13 (drill-honesty): two honest states exist. Either the
    // table records the removal decision on EVERY row, or it records a real
    // run (numeric elapsed, parseable date). A mixed or half-edited table
    // fails. Never delete this test.
    const REMOVAL: &str = "REMOVED FROM SCOPE by operator order, 2026-08-12";
    let removal_mode = rows.iter().all(|(_, v)| v == REMOVAL);
    if removal_mode {
        assert!(
            text.contains("permanently removed from scope by operator order on 2026-08-12"),
            "removal rows require the removal sentence above the table — \
             the decision must be stated, not implied"
        );
    } else {
        let elapsed = rows
            .iter()
            .find(|(f, _)| f == "elapsed minutes")
            .map(|(_, v)| v.as_str())
            .expect("elapsed minutes row");
        elapsed
            .parse::<f64>()
            .unwrap_or_else(|_| panic!("elapsed minutes must parse as a number, got {elapsed:?}"));
        let drill_date = rows
            .iter()
            .find(|(f, _)| f == "drill date")
            .map(|(_, v)| v.as_str())
            .expect("drill date row");
        NaiveDate::parse_from_str(drill_date, "%Y-%m-%d")
            .unwrap_or_else(|_| panic!("drill date must parse as YYYY-MM-DD, got {drill_date:?}"));
    }
}
