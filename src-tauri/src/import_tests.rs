//! Track 7 — import proofs (IM1-IM13).

use crate::assets::{self, RecordAssetInput};
use crate::cost_per_tray::{self, CostPerTrayOutcome, CostPerTrayRequest};
use crate::costs::{self, RecordCostInput};
use crate::db;
use crate::events::{self, EventRecord, Kind};
use crate::export::{self, Manifest};
use crate::import::{self, ImportRefusal};
use crate::mileage::{self, RecordMileageTripInput};
use crate::projection;
use crate::trays;
use chrono::Local;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("farm-os-import-{}-{}", label, uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn rust_src(rel: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join(rel),
    )
    .unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn strip_doc_comments(src: &str) -> String {
    src.lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("//!") && !t.starts_with("///")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_cfg_test_modules(src: &str) -> String {
    let mut out = String::new();
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' && chars.peek() == Some(&'[') {
            let rest: String = chars.clone().collect();
            if rest.starts_with("[cfg(test)]") {
                for _ in 0.."[cfg(test)]".len() {
                    chars.next();
                }
                while matches!(
                    chars.peek(),
                    Some(' ') | Some('\n') | Some('\r') | Some('\t')
                ) {
                    chars.next();
                }
                let after: String = chars.clone().collect();
                if after.trim_start().starts_with("mod ") {
                    while matches!(
                        chars.peek(),
                        Some(' ') | Some('\n') | Some('\r') | Some('\t')
                    ) {
                        chars.next();
                    }
                    for _ in 0.."mod".len() {
                        chars.next();
                    }
                    while matches!(chars.peek(), Some(ch) if ch.is_whitespace()) {
                        chars.next();
                    }
                    while matches!(chars.peek(), Some(ch) if ch.is_alphanumeric() || *ch == '_') {
                        chars.next();
                    }
                    while matches!(chars.peek(), Some(ch) if ch.is_whitespace()) {
                        chars.next();
                    }
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        let mut depth = 1;
                        while depth > 0 {
                            match chars.next() {
                                Some('{') => depth += 1,
                                Some('}') => depth -= 1,
                                None => break,
                                _ => {}
                            }
                        }
                        continue;
                    }
                }
            }
        }
        out.push(c);
    }
    out
}

fn production_scan(rel: &str) -> String {
    let scan = strip_cfg_test_modules(&strip_doc_comments(&rust_src(rel)));
    assert!(
        scan.contains("pub fn apply_import"),
        "canary: production scan of {rel} must contain pub fn apply_import"
    );
    scan
}

fn open_farm(dir: &Path) -> Connection {
    db::open_and_migrate(&dir.join("farm.db")).unwrap()
}

fn flush(conn: &Connection, dir: &Path) {
    crate::event_file::flush_events(conn, dir).unwrap();
}

fn seed_and_export(label: &str) -> (PathBuf, PathBuf) {
    let dir = tempfile_dir(label);
    let mut conn = open_farm(&dir);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 500,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 4.0,
            purpose: Some("market".into()),
        },
    )
    .unwrap();
    assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Shelf".into(),
            placed_in_service_on: today(),
            cost_cents: 1000,
            disposal_date: None,
        },
    )
    .unwrap();
    flush(&conn, &dir);
    let result = export::export_bundle(&conn, &dir).unwrap();
    (dir, PathBuf::from(result.bundle_path))
}

fn empty_target(label: &str) -> (PathBuf, Connection) {
    let dir = tempfile_dir(label);
    let conn = open_farm(&dir);
    (dir, conn)
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

fn walk_files(dir: &Path) -> BTreeMap<String, (u64, String, SystemTime)> {
    let mut out = BTreeMap::new();
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, (u64, String, SystemTime)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let bytes = fs::read(&path).unwrap();
                let mtime = fs::metadata(&path).unwrap().modified().unwrap();
                out.insert(rel, (bytes.len() as u64, sha256_hex(&bytes), mtime));
            }
        }
    }
    walk(dir, dir, &mut out);
    out
}

struct TargetSnap {
    counts: HashMap<String, i64>,
    max_seq: i64,
    sqlite_sequence: Option<i64>,
    events_sha: String,
    event_ids: Vec<String>,
}

fn table_names(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn row_count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn snapshot_target(conn: &Connection, farm_dir: &Path) -> TargetSnap {
    let counts: HashMap<String, i64> = table_names(conn)
        .into_iter()
        .map(|t| (t.clone(), row_count(conn, &t)))
        .collect();
    let max_seq: i64 = conn
        .query_row("SELECT IFNULL(MAX(seq), 0) FROM event_log", [], |r| {
            r.get(0)
        })
        .unwrap();
    let sqlite_sequence: Option<i64> = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'event_log'",
            [],
            |r| r.get(0),
        )
        .ok();
    let events_path = crate::event_file::events_path(farm_dir);
    let events_sha = sha256_hex(&fs::read(&events_path).unwrap_or_default());
    let mut event_ids: Vec<String> = conn
        .prepare("SELECT id FROM event_log ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    event_ids.sort();
    TargetSnap {
        counts,
        max_seq,
        sqlite_sequence,
        events_sha,
        event_ids,
    }
}

fn assert_target_unchanged(conn: &Connection, farm_dir: &Path, before: &TargetSnap) {
    let after = snapshot_target(conn, farm_dir);
    assert_eq!(after.counts, before.counts);
    assert_eq!(after.max_seq, before.max_seq);
    assert_eq!(after.sqlite_sequence, before.sqlite_sequence);
    assert_eq!(after.events_sha, before.events_sha);
    assert_eq!(after.event_ids, before.event_ids);
}

fn event_log_digest(conn: &Connection) -> String {
    let mut stmt = conn
        .prepare(
            "SELECT seq, id, kind, entity_type, entity_id, payload, inverse, origin,
                    event_domain, event_class, reverses_event_id, undoes_seq,
                    undone_at, created_at
             FROM event_log ORDER BY seq",
        )
        .unwrap();
    let rows: Vec<String> = stmt
        .query_map([], |r| {
            Ok(format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}|{}",
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<i64>>(11)?,
                r.get::<_, Option<String>>(12)?,
                r.get::<_, String>(13)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows.join("\n")
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

#[test]
fn im1_second_import_performs_zero_writes() {
    let (_src, bundle) = seed_and_export("im1-src");
    let (target_dir, mut conn) = empty_target("im1-tgt");
    let first = import::apply_import(&mut conn, &bundle).unwrap();
    assert!(first.events_added > 0);
    crate::event_file::flush_events(&conn, &target_dir).unwrap();

    let before = snapshot_target(&conn, &target_dir);
    let second = import::apply_import(&mut conn, &bundle).unwrap();
    assert_eq!(second.events_added, 0);
    assert_eq!(second.events_skipped_identical, first.events_added);
    assert_target_unchanged(&conn, &target_dir, &before);
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im2_foreign_records_are_marked_and_excluded_from_totals() {
    // Construct so that counting the foreign 999_00 would visibly change
    // cost-per-tray: farm_os pays 500 + trays sown; foreign is 99900.
    let src = tempfile_dir("im2-src");
    let mut conn = open_farm(&src);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(8.0)).unwrap();
    costs::record_cost(
        &mut conn,
        &src,
        RecordCostInput {
            amount_cents: 500,
            payee: "Seed".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    let foreign_id = uuid::Uuid::new_v4().to_string();
    let now = db::utc_now_rfc3339();
    let foreign = EventRecord {
        seq: None,
        event_id: foreign_id.clone(),
        kind: Kind::CostMoneyOut,
        entity_type: "cost_event".into(),
        entity_id: foreign_id.clone(),
        payload: json!({
            "eventId": foreign_id,
            "origin": "commercial_app",
            "datePaid": today(),
            "amountCents": 99900,
            "payee": "Other System",
            "canonicalCategory": "seed",
            "scheduleFLine": "26",
            "scheduleCLine": "22",
            "descriptor": "",
            "quantity": null,
            "unitPriceCents": null,
            "deliveryDate": null,
            "invoiceReference": null,
            "receiptFileRef": null,
            "createdAt": now,
            "updatedAt": now,
        }),
        inverse: json!({ "op": "none" }),
        origin: "commercial_app".into(),
        event_domain: "register".into(),
        event_class: Some("money_out".into()),
        reverses_event_id: None,
        undoes_seq: None,
        undone_at: None,
        created_at: now,
    };
    {
        let tx = conn.transaction().unwrap();
        projection::apply_event(&tx, &foreign).unwrap();
        events::write_event(&tx, &foreign).unwrap();
        tx.commit().unwrap();
    }
    flush(&conn, &src);
    let exported = export::export_bundle(&conn, &src).unwrap();
    let bundle = PathBuf::from(exported.bundle_path);

    let (target_dir, mut target) = empty_target("im2-tgt");
    import::apply_import(&mut target, &bundle).unwrap();

    // (a) marked in both tables
    let origin_log: String = target
        .query_row(
            "SELECT origin FROM event_log WHERE id = ?1",
            [&foreign_id],
            |r| r.get(0),
        )
        .unwrap();
    let origin_cost: String = target
        .query_row(
            "SELECT origin FROM cost_events WHERE event_id = ?1",
            [&foreign_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(origin_log, "commercial_app");
    assert_eq!(origin_cost, "commercial_app");

    // (b) Totals exclude the foreign 99900 — asserted through the real
    // derivation, not through a query written in this file. A query here would
    // return 500 no matter what cost_per_tray.rs does, so it could never fail.
    // This can: if the origin filter ever leaves the numerator, total_paid_cents
    // reads 100400 and this breaks. Done-when 2 asks for a test that breaks if
    // foreign records are counted.
    let outcome = cost_per_tray::cost_per_tray(
        &target,
        CostPerTrayRequest {
            window: "all".into(),
            from: None,
            to: None,
            category_ids: None,
        },
    )
    .unwrap();
    match outcome {
        CostPerTrayOutcome::Computed { figure, method } => {
            assert_eq!(
                figure.total_paid_cents, 500,
                "the foreign 99900 must not reach the numerator"
            );
            assert_eq!(
                method.payment_count, 1,
                "only the farm_os payment may be counted"
            );
            assert!(
                !method.payments.iter().any(|p| p.event_id == foreign_id),
                "foreign event must not appear in the method statement"
            );
        }
        CostPerTrayOutcome::Refused { reason, .. } => {
            panic!("cost per tray refused unexpectedly: {reason}");
        }
    }
    let foreign_in_totals: i64 = target
        .query_row(
            "SELECT COUNT(*) FROM cost_events
             WHERE origin = 'farm_os' AND event_id = ?1",
            [&foreign_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(foreign_in_totals, 0);

    // (c) export excludes foreign from costs.csv
    flush(&target, &target_dir);
    let out = export::export_bundle(&target, &target_dir).unwrap();
    let out_bundle = PathBuf::from(out.bundle_path);
    let csv = fs::read_to_string(out_bundle.join("costs.csv")).unwrap();
    assert!(!csv.contains(&foreign_id));
    let man: Manifest =
        serde_json::from_str(&fs::read_to_string(out_bundle.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(man.counts.cost_events, 1);

    // (d) EventRecord::originated cannot reproduce commercial origin
    let attempt = EventRecord::originated(
        Kind::CostMoneyOut,
        "cost_event",
        foreign_id.clone(),
        foreign.payload.clone(),
        json!({ "op": "none" }),
        db::utc_now_rfc3339(),
        None,
        None,
        Some(foreign_id.clone()),
    );
    assert_eq!(attempt.origin, "farm_os");

    let _ = fs::remove_dir_all(&src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im3_refusal_event_without_stable_id() {
    let (_src, bundle) = seed_and_export("im3-src");
    // Variant 1: missing event_id key
    rewrite_events_line(&bundle, |lines| {
        lines[0].as_object_mut().unwrap().remove("event_id");
    });
    let (target_dir, mut conn) = empty_target("im3-tgt");
    let before = snapshot_target(&conn, &target_dir);
    let plan = import::preview_import(&conn, &bundle).unwrap();
    assert!(!plan.can_apply);
    assert!(plan.refusals.iter().any(|r| matches!(
        r,
        ImportRefusal::MissingEventId { line_no } if *line_no == 1
    )));
    assert!(plan.explanations.iter().any(|e| e.contains("Line 1")));
    assert!(import::apply_import(&mut conn, &bundle).is_err());
    assert_target_unchanged(&conn, &target_dir, &before);

    // Variant 2: whitespace event_id
    let (_src2, bundle2) = seed_and_export("im3-src2");
    rewrite_events_line(&bundle2, |lines| {
        lines[0]["event_id"] = json!("   ");
    });
    let plan2 = import::preview_import(&conn, &bundle2).unwrap();
    assert!(!plan2.can_apply);
    assert!(plan2
        .refusals
        .iter()
        .any(|r| matches!(r, ImportRefusal::MissingEventId { .. })));
    assert!(import::apply_import(&mut conn, &bundle2).is_err());
    assert_target_unchanged(&conn, &target_dir, &before);

    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&_src2);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im4_refusal_farm_os_conflict_surfaces_and_never_overwrites() {
    let (_src, bundle) = seed_and_export("im4-src");
    let (target_dir, mut conn) = empty_target("im4-tgt");
    import::apply_import(&mut conn, &bundle).unwrap();
    flush(&conn, &target_dir);

    let cost_id: String = conn
        .query_row(
            "SELECT id FROM event_log WHERE kind = 'cost.money_out' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let before_row: (String, String, i64) = conn
        .query_row(
            "SELECT payload, created_at, seq FROM event_log WHERE id = ?1",
            [&cost_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let before_digest = event_log_digest(&conn);
    let before_count = row_count(&conn, "event_log");

    // Conflicting copy of the SAME bundle: same event_id, different amount + created_at.
    // event_log.id is immutable — edit payload/created_at only, keep ids.
    let bundle2 = {
        let dir = tempfile_dir("im4-conflict-bundle");
        for (rel, _) in walk_files(&bundle) {
            let src = bundle.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            let dest = dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::copy(&src, &dest).unwrap();
        }
        rewrite_events_line(&dir, |lines| {
            for line in lines.iter_mut() {
                if line.get("event_id").and_then(|x| x.as_str()) == Some(cost_id.as_str()) {
                    line["created_at"] = json!("2000-01-01T00:00:00.000Z");
                    if let Some(p) = line.get_mut("payload") {
                        p["amountCents"] = json!(1);
                        p["createdAt"] = json!("2000-01-01T00:00:00.000Z");
                    }
                }
            }
        });
        let bconn = Connection::open(dir.join("farm.db")).unwrap();
        let payload: String = bconn
            .query_row(
                "SELECT payload FROM event_log WHERE id = ?1",
                [&cost_id],
                |r| r.get(0),
            )
            .unwrap();
        let mut p: Value = serde_json::from_str(&payload).unwrap();
        p["amountCents"] = json!(1);
        p["createdAt"] = json!("2000-01-01T00:00:00.000Z");
        bconn
            .execute(
                "UPDATE event_log SET created_at = ?1, payload = ?2 WHERE id = ?3",
                params![
                    "2000-01-01T00:00:00.000Z",
                    serde_json::to_string(&p).unwrap(),
                    &cost_id
                ],
            )
            .unwrap();
        bconn
            .execute(
                "UPDATE cost_events SET amount_cents = 1, created_at = ?1, updated_at = ?1
                 WHERE event_id = ?2",
                params!["2000-01-01T00:00:00.000Z", &cost_id],
            )
            .unwrap();
        drop(bconn);
        remanifest(&dir);
        dir
    };

    let plan = import::preview_import(&conn, &bundle2).unwrap();
    assert!(!plan.can_apply);
    let conflict = plan
        .refusals
        .iter()
        .find_map(|r| match r {
            ImportRefusal::FarmOsConflict {
                event_id,
                field,
                in_this_farm,
                in_the_bundle,
            } => Some((event_id, field, in_this_farm, in_the_bundle)),
            _ => None,
        })
        .expect("FarmOsConflict");
    assert_eq!(conflict.0, &cost_id);
    assert!(!conflict.1.is_empty());
    assert_ne!(conflict.2, conflict.3);
    assert!(import::apply_import(&mut conn, &bundle2).is_err());

    let after_row: (String, String, i64) = conn
        .query_row(
            "SELECT payload, created_at, seq FROM event_log WHERE id = ?1",
            [&cost_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(after_row, before_row);
    assert_eq!(event_log_digest(&conn), before_digest);
    assert_eq!(row_count(&conn, "event_log"), before_count);

    // Key-reordered payload must be identical, not a conflict.
    let reordered = {
        let text = fs::read_to_string(bundle.join("events.jsonl")).unwrap();
        let mut out = String::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(line).unwrap();
            if let Some(obj) = v.as_object() {
                let mut new_obj = serde_json::Map::new();
                for k in [
                    "created_at",
                    "kind",
                    "event_id",
                    "seq",
                    "origin",
                    "event_domain",
                    "event_class",
                    "entity_type",
                    "entity_id",
                    "payload",
                    "inverse",
                    "reverses_event_id",
                    "undoes_seq",
                    "undone_at",
                ] {
                    if let Some(val) = obj.get(k) {
                        if k == "payload" {
                            if let Some(p) = val.as_object() {
                                let mut pm = serde_json::Map::new();
                                for pk in p.keys().rev() {
                                    pm.insert(pk.clone(), p[pk].clone());
                                }
                                new_obj.insert(k.into(), Value::Object(pm));
                                continue;
                            }
                        }
                        new_obj.insert(k.into(), val.clone());
                    }
                }
                for (k, val) in obj {
                    new_obj.entry(k).or_insert(val.clone());
                }
                out.push_str(&Value::Object(new_obj).to_string());
            } else {
                out.push_str(line);
            }
            out.push('\n');
        }
        let dir = tempfile_dir("im4-reordered-bundle");
        for (rel, _) in walk_files(&bundle) {
            let src = bundle.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            let dest = dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::copy(&src, &dest).unwrap();
        }
        fs::write(dir.join("events.jsonl"), out).unwrap();
        remanifest(&dir);
        dir
    };
    let plan_r = import::preview_import(&conn, &reordered).unwrap();
    assert!(
        plan_r.can_apply,
        "reordered keys must be identical: {:?}",
        plan_r.explanations
    );
    assert_eq!(plan_r.would_be_added, 0);
    assert!(plan_r.already_present_identical > 0);

    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
    let _ = fs::remove_dir_all(&bundle2);
    let _ = fs::remove_dir_all(&reordered);
}

#[test]
fn im5_refusal_commercial_sale_or_payment_claiming_farm_os() {
    let scan = production_scan("import.rs");
    for s in [
        "commercial_order",
        "commercial_payment",
        "commercial_stock_movement",
        "commercial_expense",
    ] {
        assert!(!scan.contains(s), "import.rs must not name {s}");
    }

    let (target_dir, mut conn) = empty_target("im5-tgt");
    let before = snapshot_target(&conn, &target_dir);

    // (a) event_class that does not parse
    {
        let (_src, bundle) = seed_and_export("im5a");
        rewrite_events_line(&bundle, |lines| {
            lines[0]["event_class"] = json!("commercial_order");
            // grow rows have null class; force a register event
            for line in lines.iter_mut() {
                if line.get("kind").and_then(|k| k.as_str()) == Some("cost.money_out") {
                    line["event_class"] = json!("commercial_order");
                }
            }
        });
        let plan = import::preview_import(&conn, &bundle).unwrap();
        assert!(!plan.can_apply);
        assert!(plan.refusals.iter().any(|r| matches!(
            r,
            ImportRefusal::CommercialClaimingFarmOs { event_id, .. } if !event_id.is_empty()
        )));
        assert!(plan.explanations.iter().any(|e| e.contains("Event ")));
        assert!(import::apply_import(&mut conn, &bundle).is_err());
        assert_target_unchanged(&conn, &target_dir, &before);
        let _ = fs::remove_dir_all(&_src);
    }

    // (b) farm_os record, payload origin commercial_app
    {
        let (_src, bundle) = seed_and_export("im5b");
        rewrite_events_line(&bundle, |lines| {
            for line in lines.iter_mut() {
                if line.get("kind").and_then(|k| k.as_str()) == Some("cost.money_out") {
                    line["origin"] = json!("farm_os");
                    line["payload"]["origin"] = json!("commercial_app");
                }
            }
        });
        let plan = import::preview_import(&conn, &bundle).unwrap();
        assert!(plan
            .refusals
            .iter()
            .any(|r| matches!(r, ImportRefusal::CommercialClaimingFarmOs { .. })));
        assert!(import::apply_import(&mut conn, &bundle).is_err());
        assert_target_unchanged(&conn, &target_dir, &before);
        let _ = fs::remove_dir_all(&_src);
    }

    // (c) stripe.session_paid farm_os with commercial_app in payload
    {
        let (_src, bundle) = seed_and_export("im5c");
        // Append a hand-authored stripe.session_paid line and a matching stub
        // by rewriting: replace a cost line's kind — easier to append.
        let path = bundle.join("events.jsonl");
        let mut text = fs::read_to_string(&path).unwrap();
        let eid = uuid::Uuid::new_v4().to_string();
        let line = json!({
            "seq": 9999,
            "event_id": eid,
            "origin": "farm_os",
            "event_domain": "register",
            "event_class": "sale_farm_os_path",
            "kind": "stripe.session_paid",
            "reverses_event_id": null,
            "entity_type": "order",
            "entity_id": eid,
            "payload": { "commercial_app": true, "sessionId": "cs_x" },
            "inverse": { "op": "none" },
            "undone_at": null,
            "undoes_seq": null,
            "created_at": "2026-01-01T00:00:00.000Z",
        });
        text.push_str(&line.to_string());
        text.push('\n');
        fs::write(&path, text).unwrap();
        remanifest(&bundle);
        let plan = import::preview_import(&conn, &bundle).unwrap();
        assert!(plan.refusals.iter().any(|r| match r {
            ImportRefusal::CommercialClaimingFarmOs { event_id, .. } => event_id == &eid,
            _ => false,
        }));
        assert!(import::apply_import(&mut conn, &bundle).is_err());
        assert_target_unchanged(&conn, &target_dir, &before);
        let _ = fs::remove_dir_all(&_src);
    }

    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im6_log_versus_database_disagreement_halts() {
    let scan = production_scan("import.rs");
    assert!(scan.contains("verify_replay_paths"));
    assert!(!scan.contains("farm_dir_verify"));

    let (_src, bundle) = seed_and_export("im6-src");
    {
        let db_path = bundle.join("farm.db");
        let bconn = Connection::open(&db_path).unwrap();
        bconn
            .execute("UPDATE cost_events SET amount_cents = amount_cents + 1", [])
            .unwrap();
        drop(bconn);
        remanifest(&bundle);
    }

    let (target_dir, mut conn) = empty_target("im6-tgt");
    let before = snapshot_target(&conn, &target_dir);
    let plan = import::preview_import(&conn, &bundle).unwrap();
    assert!(!plan.can_apply);
    let detail = plan.refusals.iter().find_map(|r| match r {
        ImportRefusal::LogVersusDatabase { detail } => Some(detail.clone()),
        _ => None,
    });
    let detail = detail.expect("LogVersusDatabase");
    assert!(
        detail.contains("cost_events") && detail.contains("amount"),
        "{detail}"
    );
    assert!(import::apply_import(&mut conn, &bundle).is_err());
    assert_target_unchanged(&conn, &target_dir, &before);
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im7_no_upsert_no_merge_anywhere() {
    let scan = production_scan("import.rs");
    for needle in [
        "INSERT OR REPLACE",
        "INSERT OR IGNORE",
        "ON CONFLICT",
        "UPSERT",
        "UPDATE event_log",
        "DELETE FROM event_log",
        "REPLACE INTO",
        "EventRecord::originated",
        "insert_event_row",
    ] {
        assert!(!scan.contains(needle), "must not contain {needle}");
    }

    // Behavioural half covered by IM4 digest equality; restate briefly.
    let (_src, bundle) = seed_and_export("im7-src");
    let (target_dir, mut conn) = empty_target("im7-tgt");
    import::apply_import(&mut conn, &bundle).unwrap();
    let before = event_log_digest(&conn);
    let before_count = row_count(&conn, "event_log");
    let (_src2, bundle2) = seed_and_export("im7-conflict");
    // Make zero-overlap? Actually need conflict — reuse im4 pattern lightly:
    // preview a different farm's bundle against populated target → DifferentFarm
    // or conflict. Use DifferentFarm path: assert digest unchanged after Err.
    assert!(import::apply_import(&mut conn, &bundle2).is_err());
    assert_eq!(event_log_digest(&conn), before);
    assert_eq!(row_count(&conn, "event_log"), before_count);
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&_src2);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im8_import_never_mutates_the_source_bundle() {
    let (_src, bundle) = seed_and_export("im8-src");
    let before = walk_files(&bundle);
    let (target_dir, mut conn) = empty_target("im8-tgt");
    for _ in 0..3 {
        import::preview_import(&conn, &bundle).unwrap();
    }
    import::apply_import(&mut conn, &bundle).unwrap();
    let after = walk_files(&bundle);
    assert_eq!(before, after);
    assert!(!bundle.join("last-verify-replay.txt").exists());
    // Also prove we can open farm.db read-only
    let _ro = Connection::open_with_flags(bundle.join("farm.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im9_manifest_mismatch_refuses() {
    let (target_dir, mut conn) = empty_target("im9-tgt");
    let before = snapshot_target(&conn, &target_dir);

    // Missing listed file
    {
        let (_src, bundle) = seed_and_export("im9-miss");
        fs::remove_file(bundle.join("costs.csv")).unwrap();
        let plan = import::preview_import(&conn, &bundle).unwrap();
        assert!(plan.refusals.iter().any(|r| matches!(
            r,
            ImportRefusal::ManifestMismatch { path, .. } if path == "costs.csv"
        )));
        assert!(import::apply_import(&mut conn, &bundle).is_err());
        assert_target_unchanged(&conn, &target_dir, &before);
        let _ = fs::remove_dir_all(&_src);
    }

    // Altered checksum
    {
        let (_src, bundle) = seed_and_export("im9-hash");
        fs::write(bundle.join("costs.csv"), b"tampered\n").unwrap();
        // Do NOT remanifest — checksum must disagree.
        let plan = import::preview_import(&conn, &bundle).unwrap();
        assert!(plan.refusals.iter().any(|r| matches!(
            r,
            ImportRefusal::ManifestMismatch { path, .. } if path == "costs.csv"
        )));
        assert!(import::apply_import(&mut conn, &bundle).is_err());
        assert_target_unchanged(&conn, &target_dir, &before);
        let _ = fs::remove_dir_all(&_src);
    }

    // Extra unlisted file
    {
        let (_src, bundle) = seed_and_export("im9-extra");
        fs::write(bundle.join("extra.txt"), b"nope").unwrap();
        let plan = import::preview_import(&conn, &bundle).unwrap();
        assert!(plan.refusals.iter().any(|r| matches!(
            r,
            ImportRefusal::ManifestMismatch { path, .. } if path == "extra.txt"
        )));
        assert!(import::apply_import(&mut conn, &bundle).is_err());
        assert_target_unchanged(&conn, &target_dir, &before);
        let _ = fs::remove_dir_all(&_src);
    }

    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im10_preview_writes_nothing() {
    let (_src, bundle) = seed_and_export("im10-clean");
    let (_src2, conflict_bundle) = seed_and_export("im10-other");
    let (_src3, malformed) = seed_and_export("im10-mal");
    rewrite_events_line(&malformed, |lines| {
        lines[0].as_object_mut().unwrap().remove("event_id");
    });

    let (target_dir, mut conn) = empty_target("im10-tgt");
    // Put a real record so conflict_bundle triggers DifferentFarm
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, None).unwrap();
    flush(&conn, &target_dir);
    let before = snapshot_target(&conn, &target_dir);

    for _ in 0..5 {
        let _ = import::preview_import(&conn, &bundle);
        let _ = import::preview_import(&conn, &conflict_bundle);
        let _ = import::preview_import(&conn, &malformed);
    }
    assert_target_unchanged(&conn, &target_dir, &before);
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&_src2);
    let _ = fs::remove_dir_all(&_src3);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im11_schema_version_mismatch_refuses_both_directions() {
    let (_src, bundle) = seed_and_export("im11");
    let (target_dir, mut conn) = empty_target("im11-tgt");
    let before = snapshot_target(&conn, &target_dir);

    for ver in [12i32, 17] {
        let man_path = bundle.join("manifest.json");
        let mut man: Manifest =
            serde_json::from_str(&fs::read_to_string(&man_path).unwrap()).unwrap();
        man.app_schema_version = ver;
        fs::write(
            &man_path,
            format!("{}\n", serde_json::to_string_pretty(&man).unwrap()),
        )
        .unwrap();
        // Remanifest would restore schema from... no, remanifest keeps
        // app_schema_version from the struct we need to rewrite files only.
        // manifest.json itself is not checksummed in files[].
        let plan = import::preview_import(&conn, &bundle).unwrap();
        assert!(plan.refusals.iter().any(|r| matches!(
            r,
            ImportRefusal::SchemaVersion { bundle: b, this_app: t }
            if *b == ver && *t == db::SCHEMA_VERSION
        )));
        let expl = plan.explanations.join(" ");
        assert!(expl.contains(&ver.to_string()) && expl.contains(&db::SCHEMA_VERSION.to_string()));
        assert!(import::apply_import(&mut conn, &bundle).is_err());
        assert_target_unchanged(&conn, &target_dir, &before);
    }
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn im12_refusal_different_farm_and_the_snapshot_only_exception() {
    let (_src_a, _bundle_a_unused) = seed_and_export("im12-a");
    let (_src_b, bundle_b) = seed_and_export("im12-b");

    // (a) Farm A has real records; bundle from B; refuse DifferentFarm
    let (dir_a, mut farm_a) = empty_target("im12-farm-a");
    trays::sow_tray_with_seed(&mut farm_a, "dun-peas", 1, None).unwrap();
    flush(&farm_a, &dir_a);
    let before_a = snapshot_target(&farm_a, &dir_a);
    let plan = import::preview_import(&farm_a, &bundle_b).unwrap();
    assert!(!plan.can_apply);
    assert!(plan.refusals.iter().any(|r| matches!(
        r,
        ImportRefusal::DifferentFarm {
            farm_records_here,
            events_in_bundle
        } if *farm_records_here > 0 && *events_in_bundle > 0
    )));
    let expl = plan.explanations.join(" ");
    assert!(expl.contains("Two farms cannot be merged"));
    for bad in ["format", "convert", "anyway", "risk"] {
        assert!(!expl.to_ascii_lowercase().contains(bad), "{bad} in {expl}");
    }
    assert!(import::apply_import(&mut farm_a, &bundle_b).is_err());
    assert_target_unchanged(&farm_a, &dir_a, &before_a);

    // (b) Same bundle into genuinely empty farm — succeeds
    let (dir_empty, mut empty) = empty_target("im12-empty");
    let plan_b = import::preview_import(&empty, &bundle_b).unwrap();
    assert!(plan_b.can_apply, "{:?}", plan_b.explanations);
    let applied = import::apply_import(&mut empty, &bundle_b).unwrap();
    assert!(applied.events_added > 0);

    // Idempotent path: no DifferentFarm
    let plan_again = import::preview_import(&empty, &bundle_b).unwrap();
    assert!(plan_again.can_apply);
    assert!(!plan_again
        .refusals
        .iter()
        .any(|r| matches!(r, ImportRefusal::DifferentFarm { .. })));

    // (c) Target with ONLY snapshot.taken — import must succeed
    let (dir_snap, mut snap_only) = empty_target("im12-snap");
    insert_snapshot_taken(&mut snap_only, 2);
    flush(&snap_only, &dir_snap);
    let farm_records: i64 = snap_only
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE kind <> 'snapshot.taken'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(farm_records, 0);
    let plan_c = import::preview_import(&snap_only, &bundle_b).unwrap();
    assert!(
        plan_c.can_apply,
        "move-computers case must succeed: {:?}",
        plan_c.explanations
    );
    import::apply_import(&mut snap_only, &bundle_b).unwrap();

    // (d) snapshot.taken PLUS one real record — refuse
    let (dir_d, mut mixed) = empty_target("im12-mixed");
    insert_snapshot_taken(&mut mixed, 1);
    trays::sow_tray_with_seed(&mut mixed, "dun-peas", 1, None).unwrap();
    flush(&mixed, &dir_d);
    let plan_d = import::preview_import(&mixed, &bundle_b).unwrap();
    assert!(plan_d
        .refusals
        .iter()
        .any(|r| matches!(r, ImportRefusal::DifferentFarm { .. })));

    let _ = fs::remove_dir_all(&_src_a);
    let _ = fs::remove_dir_all(&_src_b);
    let _ = fs::remove_dir_all(&dir_a);
    let _ = fs::remove_dir_all(&dir_empty);
    let _ = fs::remove_dir_all(&dir_snap);
    let _ = fs::remove_dir_all(&dir_d);
}

#[test]
fn im13_track_7_adds_no_kind_no_table_no_schema_change() {
    // Bumped for the silent-shortfall residual (wholesale.write_off).
    assert_eq!(Kind::ALL.len(), 54);
    let conn = db::open_in_memory().unwrap();
    let names = table_names(&conn);
    let expected = [
        "assets",
        "attention",
        "business_profile",
        "consumption_events",
        "cost_events",
        "crop_aliases",
        "crops",
        "event_log",
        "farm_config",
        "field_devices",
        "harvest_coverage",
        "harvest_links",
        "health_evidence",
        "income_events",
        "leftover_listings",
        "mileage_trips",
        "mkt_followups",
        "mkt_review_requests",
        "mkt_reviews",
        "mkt_samples",
        "mkt_stages",
        "mkt_standing_requests",
        "mkt_touches",
        "mkt_venues",
        "money_corrections",
        "offers",
        "orders",
        "phone_proposals",
        "phone_pull_observations",
        "phone_pull_refusals",
        "scan_config",
        "scan_observations",
        "seed_receipts",
        "shelf_capacity",
        "sqlite_sequence",
        "standing_pull_observations",
        "standing_pull_refusals",
        "storefront_observations",
        "stripe_config",
        "stripe_cursor",
        "stripe_unapplied_facts",
        "trays",
        "wholesale_bad_debts",
        "wholesale_order_lines",
        "wholesale_orders",
        "wholesale_write_offs",
    ];
    assert_eq!(names, expected, "v18 table set after money_corrections");
    let _ = BTreeSet::<String>::new();
}

// --- C3 IMPORT-DOOR (INT-003 / INT-009 / INT-010) -----------------------------
// The stated rule — commercial_app records arrive marked, are never editable,
// and are excluded from totals — is true for exactly one kind, cost.money_out
// (im2). C3 makes the door admit only that kind from another system, refuses
// a third origin word before the trigger sees it, gates correct/void on
// origin, makes the export note count instead of claim, makes the cutover
// one-shot in fact, and re-derives an imported undo's ordinal from the name
// it carries.

/// The im2 foreign expense as one helper: a commercial_app cost.money_out
/// written through the same choke point the import uses, so the farm's
/// database and log agree and its bundle passes its own verify-replay.
fn write_foreign_cost(conn: &mut Connection, amount_cents: i64) -> String {
    let foreign_id = uuid::Uuid::new_v4().to_string();
    let now = db::utc_now_rfc3339();
    let foreign = EventRecord {
        seq: None,
        event_id: foreign_id.clone(),
        kind: Kind::CostMoneyOut,
        entity_type: "cost_event".into(),
        entity_id: foreign_id.clone(),
        payload: json!({
            "eventId": foreign_id,
            "origin": "commercial_app",
            "datePaid": today(),
            "amountCents": amount_cents,
            "payee": "Other System",
            "canonicalCategory": "seed",
            "scheduleFLine": "26",
            "scheduleCLine": "22",
            "descriptor": "",
            "quantity": null,
            "unitPriceCents": null,
            "deliveryDate": null,
            "invoiceReference": null,
            "receiptFileRef": null,
            "createdAt": now,
            "updatedAt": now,
        }),
        inverse: json!({ "op": "none" }),
        origin: "commercial_app".into(),
        event_domain: "register".into(),
        event_class: Some("money_out".into()),
        reverses_event_id: None,
        undoes_seq: None,
        undone_at: None,
        created_at: now,
    };
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &foreign).unwrap();
    events::write_event(&tx, &foreign).unwrap();
    tx.commit().unwrap();
    foreign_id
}

/// A commercial_app tray.sown written the same way: a kind the door must
/// refuse, inside a bundle that is otherwise clean, so the refusal is the
/// door's alone and not the bundle's own verify-replay.
fn write_foreign_sow(conn: &mut Connection) -> String {
    let tray_id = uuid::Uuid::new_v4().to_string();
    let now = db::utc_now_rfc3339();
    let foreign = EventRecord {
        seq: None,
        event_id: uuid::Uuid::new_v4().to_string(),
        kind: Kind::TraySown,
        entity_type: "tray".into(),
        entity_id: tray_id.clone(),
        payload: json!({
            "cropId": "dun-peas",
            "quantity": 1,
            "sownOn": today(),
            "blackoutOn": today(),
        }),
        inverse: json!({ "op": "delete_tray", "trayId": tray_id }),
        origin: "commercial_app".into(),
        event_domain: "grow".into(),
        event_class: None,
        reverses_event_id: None,
        undoes_seq: None,
        undone_at: None,
        created_at: now,
    };
    let event_id = foreign.event_id.clone();
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &foreign).unwrap();
    events::write_event(&tx, &foreign).unwrap();
    tx.commit().unwrap();
    event_id
}

fn read_manifest(bundle: &Path) -> Manifest {
    serde_json::from_str(&fs::read_to_string(bundle.join("manifest.json")).unwrap()).unwrap()
}

#[test]
fn int003_door_refuses_a_foreign_record_that_is_not_an_expense() {
    let src = tempfile_dir("int003-door-src");
    let mut conn = open_farm(&src);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    let foreign_id = write_foreign_sow(&mut conn);
    flush(&conn, &src);
    let bundle = PathBuf::from(export::export_bundle(&conn, &src).unwrap().bundle_path);
    let (target_dir, mut target) = empty_target("int003-door-tgt");
    let before = snapshot_target(&target, &target_dir);
    let plan = import::preview_import(&target, &bundle).unwrap();
    assert!(!plan.can_apply);
    assert!(plan.refusals.iter().any(|r| matches!(
        r,
        ImportRefusal::ForeignKindNotCarried { event_id, event_kind }
            if event_id == &foreign_id && event_kind.as_str() == "tray.sown"
    )));
    // The bundle itself is clean: the door alone refuses it.
    assert!(!plan
        .refusals
        .iter()
        .any(|r| matches!(r, ImportRefusal::LogVersusDatabase { .. })));
    assert_eq!(
        plan.foreign_records_in_bundle, 0,
        "a refused record is not a foreign record"
    );
    let expl = plan.explanations.join(" ");
    assert!(
        expl.contains("tray.sown record from another system"),
        "{expl}"
    );
    assert!(expl.contains("Nothing was brought in"), "{expl}");
    assert!(import::apply_import(&mut target, &bundle).is_err());
    assert_target_unchanged(&target, &target_dir, &before);
    assert_eq!(row_count(&target, "trays"), 0);
    let _ = fs::remove_dir_all(&src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn int003_door_refuses_foreign_kinds_the_validators_refuse_before_the_transaction() {
    // Before C3 a commercial_app mileage.trip passed the plan and died inside
    // the transaction on validate_mileage_event. Now the plan says so.
    let (_src, bundle) = seed_and_export("int003-val-src");
    rewrite_events_line(&bundle, |lines| {
        for line in lines.iter_mut() {
            if line.get("kind").and_then(|k| k.as_str()) == Some("mileage.trip") {
                line["origin"] = json!("commercial_app");
            }
        }
    });
    let (target_dir, mut target) = empty_target("int003-val-tgt");
    let before = snapshot_target(&target, &target_dir);
    let plan = import::preview_import(&target, &bundle).unwrap();
    assert!(!plan.can_apply);
    assert!(plan.refusals.iter().any(|r| matches!(
        r,
        ImportRefusal::ForeignKindNotCarried { event_kind, .. }
            if event_kind.as_str() == "mileage.trip"
    )));
    assert!(import::apply_import(&mut target, &bundle).is_err());
    assert_target_unchanged(&target, &target_dir, &before);
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn int003_unknown_origin_word_is_refused_at_the_door() {
    // Before C3 a third origin word passed the plan and died on the event_log
    // trigger inside the transaction.
    let (_src, bundle) = seed_and_export("int003-origin-src");
    rewrite_events_line(&bundle, |lines| {
        lines[0]["origin"] = json!("somewhere_else");
    });
    let (target_dir, mut target) = empty_target("int003-origin-tgt");
    let before = snapshot_target(&target, &target_dir);
    let plan = import::preview_import(&target, &bundle).unwrap();
    assert!(!plan.can_apply);
    assert!(plan.refusals.iter().any(|r| matches!(
        r,
        ImportRefusal::Malformed { detail, .. }
            if detail.contains("is not farm_os or commercial_app")
    )));
    assert!(import::apply_import(&mut target, &bundle).is_err());
    assert_target_unchanged(&target, &target_dir, &before);
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn int003_export_note_counts_foreign_records_instead_of_claiming_farm_os_only() {
    let clean = tempfile_dir("int003-note-clean");
    let mut conn = open_farm(&clean);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    flush(&conn, &clean);
    let bundle = PathBuf::from(export::export_bundle(&conn, &clean).unwrap().bundle_path);
    let man = read_manifest(&bundle);
    assert_eq!(man.counts.commercial_app_records, 0);
    let notes = man.notes.join("\n");
    assert!(
        notes.contains("this bundle contains only farm_os originated records"),
        "{notes}"
    );

    let mixed = tempfile_dir("int003-note-mixed");
    let mut conn = open_farm(&mixed);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    costs::record_cost(
        &mut conn,
        &mixed,
        RecordCostInput {
            amount_cents: 500,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    let foreign_id = write_foreign_cost(&mut conn, 99900);
    flush(&conn, &mixed);
    let bundle = PathBuf::from(export::export_bundle(&conn, &mixed).unwrap().bundle_path);
    let man = read_manifest(&bundle);
    assert_eq!(man.counts.commercial_app_records, 1);
    assert_eq!(
        man.counts.cost_events, 1,
        "costs.csv carries the farm_os expense only"
    );
    let notes = man.notes.join("\n");
    assert!(!notes.contains("only farm_os originated"), "{notes}");
    assert!(
        notes.contains("1 commercial_app originated record"),
        "{notes}"
    );
    assert!(
        notes.contains("excluded from every total and CSV"),
        "{notes}"
    );
    let csv = fs::read_to_string(bundle.join("costs.csv")).unwrap();
    assert!(!csv.contains(&foreign_id));
    // A manifest written before C3 has no commercialAppRecords key and still
    // parses at the door: the field defaults, the schema version did not move.
    let mut raw: Value =
        serde_json::from_str(&fs::read_to_string(bundle.join("manifest.json")).unwrap()).unwrap();
    raw["counts"]
        .as_object_mut()
        .unwrap()
        .remove("commercialAppRecords");
    let legacy: Manifest = serde_json::from_value(raw).unwrap();
    assert_eq!(legacy.counts.commercial_app_records, 0);
    let _ = fs::remove_dir_all(&clean);
    let _ = fs::remove_dir_all(&mixed);
}

#[test]
fn int003_a_foreign_expense_is_not_editable_here() {
    let dir = tempfile_dir("int003-edit");
    let mut conn = open_farm(&dir);
    let foreign_id = write_foreign_cost(&mut conn, 99900);
    let before_digest = event_log_digest(&conn);
    let err = costs::correct_expense(
        &mut conn,
        costs::CorrectExpenseInput {
            target_event_id: foreign_id.clone(),
            amount_cents: 100,
            payee: "Other System".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            reason: None,
        },
    )
    .unwrap_err();
    assert_eq!(
        err,
        "That expense came from another system and is not editable here."
    );
    let err = costs::void_expense(&mut conn, &foreign_id, None).unwrap_err();
    assert_eq!(
        err,
        "That expense came from another system and is not editable here."
    );
    let (amount, origin, voided): (i64, String, Option<String>) = conn
        .query_row(
            "SELECT amount_cents, origin, voided_at FROM cost_events WHERE event_id = ?1",
            [&foreign_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(amount, 99900);
    assert_eq!(origin, "commercial_app");
    assert!(voided.is_none());
    assert_eq!(
        event_log_digest(&conn),
        before_digest,
        "nothing was written"
    );
    assert!(costs::list_expenses(&conn)
        .unwrap()
        .iter()
        .all(|e| e.event_id != foreign_id));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn int009_cutover_is_one_shot_and_its_receipt_is_never_overwritten() {
    let (_src, bundle) = seed_and_export("int009-src");
    let (target_dir, mut target) = empty_target("int009-tgt");
    let first = import::apply_cutover_import(&mut target, &bundle, "CUTOVER").unwrap();
    assert!(first.events_added > 0);
    let receipt_path = target_dir.join("cutover-import.txt");
    let receipt = fs::read(&receipt_path).unwrap();
    assert!(!receipt.is_empty());
    let digest = event_log_digest(&target);
    let err = import::apply_cutover_import(&mut target, &bundle, "CUTOVER").unwrap_err();
    assert_eq!(
        err,
        "This farm already brought in its cutover bundle; the receipt is cutover-import.txt. \
         A cutover happens once — use Bring in a bundle for anything later."
    );
    assert_eq!(
        fs::read(&receipt_path).unwrap(),
        receipt,
        "the receipt is never overwritten"
    );
    assert_eq!(event_log_digest(&target), digest);
    // The ordinary door still answers the same bundle: nothing to add.
    let again = import::apply_import(&mut target, &bundle).unwrap();
    assert_eq!(again.events_added, 0);
    // No on-disk farm directory: refused before anything is applied.
    let mut mem = db::open_in_memory().unwrap();
    let before = row_count(&mem, "event_log");
    let err = import::apply_cutover_import(&mut mem, &bundle, "CUTOVER").unwrap_err();
    assert!(err.contains("on-disk farm directory"), "{err}");
    assert_eq!(row_count(&mem, "event_log"), before);
    let _ = fs::remove_dir_all(&_src);
    let _ = fs::remove_dir_all(&target_dir);
}

#[test]
fn int010_imported_undo_points_at_this_farms_row_and_reimports_cleanly() {
    let src = tempfile_dir("int010-src");
    let mut conn = open_farm(&src);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    trays::sow_tray(&mut conn, "spicy-mix", 1).unwrap();
    assert!(trays::undo_last(&mut conn).unwrap().is_some());
    let (source_ordinal, target_name): (i64, String) = conn
        .query_row(
            "SELECT undoes_seq, reverses_event_id FROM event_log WHERE kind = 'undo'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    flush(&conn, &src);
    let bundle = PathBuf::from(export::export_bundle(&conn, &src).unwrap().bundle_path);
    drop(conn);
    // A real farm never imports into an empty log: every launch has appended
    // snapshot.taken first. Three rows of offset make a wrong ordinal visible.
    let (target_dir, mut target) = empty_target("int010-tgt");
    insert_snapshot_taken(&mut target, 3);
    let plan = import::preview_import(&target, &bundle).unwrap();
    assert!(plan.can_apply, "{:?}", plan.explanations);
    let applied = import::apply_import(&mut target, &bundle).unwrap();
    assert!(applied.events_added > 0);
    let (stored, payload_s): (i64, String) = target
        .query_row(
            "SELECT undoes_seq, payload FROM event_log WHERE kind = 'undo'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let local_seq: i64 = target
        .query_row(
            "SELECT seq FROM event_log WHERE id = ?1",
            [&target_name],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        stored, local_seq,
        "the column is this farm's ordinal of the named event"
    );
    assert_eq!(
        local_seq,
        source_ordinal + 3,
        "the offset the launch rows put there"
    );
    assert_ne!(stored, source_ordinal);
    let payload: Value = serde_json::from_str(&payload_s).unwrap();
    assert_eq!(
        payload["undoesSeq"],
        json!(source_ordinal),
        "the record's own words are untouched"
    );
    // The undo did what it named: the second sow is gone here too.
    assert_eq!(row_count(&target, "trays"), 1);
    // Re-import of the same bundle: identical, nothing added, no conflict on
    // the ordinal — the name is what the conflict check compares.
    let again = import::preview_import(&target, &bundle).unwrap();
    assert!(again.can_apply, "{:?}", again.explanations);
    assert_eq!(again.would_be_added, 0);
    assert_eq!(again.already_present_identical, again.events_in_bundle);
    assert_eq!(
        import::apply_import(&mut target, &bundle)
            .unwrap()
            .events_added,
        0
    );
    // The farm this produced replays: the column and the file agree.
    flush(&target, &target_dir);
    drop(target);
    let outcome = projection::farm_dir_verify(&target_dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    let _ = fs::remove_dir_all(&src);
    let _ = fs::remove_dir_all(&target_dir);
}
