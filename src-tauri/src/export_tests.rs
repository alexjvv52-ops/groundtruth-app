//! Track 6 — export bundle proofs (EX1-EX12).

use crate::assets::{self, RecordAssetInput};
use crate::categories;
use crate::costs::{self, RecordCostInput};
use crate::db;
use crate::events::Kind;
use crate::export::{self, Manifest};
use crate::export_scrub;
use crate::income::{self, RecordIncomeInput};
use crate::marketing;
use crate::mileage::{self, RecordMileageTripInput};
use crate::money;
use crate::scans;
use crate::trays;
use crate::wholesale::{self, OrderLine, WriteOffInput};
use chrono::{Duration, Local};
use rusqlite::{params, Connection};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

pub(crate) fn tempfile_dir(label: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("farm-os-export-{}-{}", label, uuid::Uuid::new_v4()));
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
        scan.contains("pub fn export_bundle"),
        "canary: production scan of {rel} must contain pub fn export_bundle"
    );
    scan
}

fn walk_rs(dir: &Path, f: &mut dyn FnMut(&Path, &str)) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            walk_rs(&path, f);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let src = fs::read_to_string(&path).unwrap();
            f(&path, &src);
        }
    }
}

pub(crate) fn open_farm(dir: &Path) -> Connection {
    db::open_and_migrate(&dir.join("farm.db")).unwrap()
}

pub(crate) fn flush(conn: &Connection, dir: &Path) {
    crate::event_file::flush_events(conn, dir).unwrap();
}

fn write_receipt(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    p
}

pub(crate) fn parse_manifest(bundle: &Path) -> Manifest {
    let text = fs::read_to_string(bundle.join("manifest.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn walk_bundle_files(bundle: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    fn walk(base: &Path, dir: &Path, out: &mut BTreeSet<String>) {
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
                out.insert(rel);
            }
        }
    }
    walk(bundle, bundle, &mut out);
    out
}

fn bundle_root_names(bundle: &Path) -> BTreeSet<String> {
    fs::read_dir(bundle)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
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

fn max_seq(conn: &Connection) -> i64 {
    conn.query_row("SELECT IFNULL(MAX(seq), 0) FROM event_log", [], |r| {
        r.get(0)
    })
    .unwrap()
}

fn sorted_table_digest(conn: &Connection, table: &str) -> String {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
    let col_count = stmt.column_count();
    let mut rows: Vec<String> = stmt
        .query_map([], |r| {
            let mut cells = Vec::with_capacity(col_count);
            for i in 0..col_count {
                let v: rusqlite::types::Value = r.get(i)?;
                cells.push(format!("{v:?}"));
            }
            Ok(cells.join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows.sort();
    rows.join("\n")
}

pub(crate) fn parse_csv(text: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let chars: Vec<char> = text.chars().collect();
    let mut idx = 0;
    while idx < chars.len() {
        let ch = chars[idx];
        if quoted {
            if ch == '"' {
                if idx + 1 < chars.len() && chars[idx + 1] == '"' {
                    field.push('"');
                    idx += 2;
                    continue;
                }
                quoted = false;
                idx += 1;
                continue;
            }
            field.push(ch);
            idx += 1;
            continue;
        }
        match ch {
            '"' => {
                quoted = true;
                idx += 1;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                idx += 1;
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                idx += 1;
            }
            '\r' => idx += 1,
            _ => {
                field.push(ch);
                idx += 1;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    let header = rows.first().cloned().unwrap_or_default();
    let data = if rows.len() > 1 {
        rows[1..].to_vec()
    } else {
        Vec::new()
    };
    (header, data)
}

fn seed_farm(dir: &Path) -> Connection {
    let mut conn = open_farm(dir);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    let receipt = write_receipt(dir, "recv-a.bin", b"hello-receipt");
    costs::record_cost(
        &mut conn,
        dir,
        RecordCostInput {
            amount_cents: 500,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(receipt.to_string_lossy().into()),
        },
    )
    .unwrap();
    costs::record_cost(
        &mut conn,
        dir,
        RecordCostInput {
            amount_cents: 250,
            payee: "Booth LLC".into(),
            category_id: "market_stall_booth".into(),
            date_paid: today(),
            descriptor: Some("Saturday market".into()),
            receipt_source_path: None,
        },
    )
    .unwrap();
    let t1 = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 10.0,
            purpose: Some("market run".into()),
        },
    )
    .unwrap();
    let t2 = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 3.5,
            purpose: Some("supplies".into()),
        },
    )
    .unwrap();
    mileage::void_trip(&mut conn, &t2.trip_id).unwrap();
    let a1 = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Shelf unit".into(),
            placed_in_service_on: today(),
            cost_cents: 12000,
            disposal_date: None,
        },
    )
    .unwrap();
    let a2 = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Old light".into(),
            placed_in_service_on: today(),
            cost_cents: 4000,
            disposal_date: None,
        },
    )
    .unwrap();
    assets::void_asset(&mut conn, &a2.asset_id).unwrap();
    let _ = (t1, a1);
    flush(&conn, dir);
    conn
}

#[test]
fn ex1_one_action_produces_the_full_bundle() {
    let dir = tempfile_dir("ex1");
    let conn = seed_farm(&dir);
    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);

    let names = bundle_root_names(&bundle);
    let expected: BTreeSet<String> = [
        "farm.db",
        "events.jsonl",
        "receipts",
        "costs.csv",
        "income.csv",
        "assets.csv",
        "mileage.csv",
        "marketing.csv",
        "wholesale.csv",
        "receivables.csv",
        "money-corrections.csv",
        "income-corrections.csv",
        "write-offs.csv",
        "categories.json",
        "manifest.json",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(
        names, expected,
        "bundle root must be exactly the fifteen items"
    );

    let exported = Connection::open(bundle.join("farm.db")).unwrap();
    let ver: i32 = exported
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(ver, db::SCHEMA_VERSION);
    let _ = fs::remove_dir_all(&dir);
}

/// B9 — the manual's bundle list is a truth surface. It drifted from 8 to 14
/// unnoticed because nothing compared it to the exporter. This does.
#[test]
fn ex_manual_bundle_list_names_every_exported_file() {
    let dir = tempfile_dir("ex-manual-bundle");
    let conn = seed_farm(&dir);
    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let names = bundle_root_names(&bundle);

    let manual_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/OPERATOR-MANUAL.md");
    let manual = fs::read_to_string(&manual_path)
        .unwrap_or_else(|e| panic!("docs/OPERATOR-MANUAL.md missing: {e}"));

    let missing: Vec<String> = names
        .into_iter()
        .filter(|name| !manual.contains(name.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "OPERATOR-MANUAL.md is missing exported bundle names: {}",
        missing.join(", ")
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex2_manifest_is_the_contract_both_directions() {
    let dir = tempfile_dir("ex2");
    let conn = seed_farm(&dir);
    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let manifest = parse_manifest(&bundle);

    let on_disk = walk_bundle_files(&bundle);
    let mut disk_minus_manifest = on_disk.clone();
    assert!(disk_minus_manifest.remove("manifest.json"));
    let listed: BTreeSet<String> = manifest.files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(
        listed, disk_minus_manifest,
        "manifest files[] must equal bundle files minus manifest.json"
    );
    assert!(listed.contains("costs.csv"));
    assert!(listed.contains("income.csv"));

    for entry in &manifest.files {
        let abs = bundle.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let bytes = fs::read(&abs).unwrap();
        assert_eq!(bytes.len() as u64, entry.size_bytes, "{}", entry.path);
        assert_eq!(sha256_hex(&bytes), entry.sha256, "{}", entry.path);
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex3_receipts_are_bit_identical() {
    let dir = tempfile_dir("ex3");
    let mut conn = open_farm(&dir);
    let a_bytes: Vec<u8> = vec![0x48, 0x69, 0x0D, 0x0A, 0x1A, 0xFF, 0x00, 0x42];
    let b_bytes = b"plain-text-receipt\nline2".to_vec();
    let a_path = write_receipt(&dir, "src-a.bin", &a_bytes);
    let b_path = write_receipt(&dir, "src-b.txt", &b_bytes);
    costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 100,
            payee: "A".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(a_path.to_string_lossy().into()),
        },
    )
    .unwrap();
    costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 200,
            payee: "B".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(b_path.to_string_lossy().into()),
        },
    )
    .unwrap();
    flush(&conn, &dir);
    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);

    for (orig, name_hint) in [(&a_bytes[..], "a"), (&b_bytes[..], "b")] {
        let digest = sha256_hex(orig);
        let mut matched = false;
        for entry in fs::read_dir(bundle.join("receipts")).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&digest) {
                let got = fs::read(entry.path()).unwrap();
                assert_eq!(&got[..], orig, "bit-identical for {name_hint}");
                let stem = Path::new(&name).file_stem().unwrap().to_str().unwrap();
                assert_eq!(stem, digest);
                matched = true;
            }
        }
        assert!(matched, "missing receipt for {name_hint}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex4_export_completes_with_no_network_and_no_account() {
    let dir = tempfile_dir("ex4");
    let conn = seed_farm(&dir);
    assert!(export::export_bundle(&conn, &dir).is_ok());

    let scan = production_scan("export.rs");
    for needle in ["ureq", "reqwest", "hyper", "tokio", "TcpStream", "http::"] {
        assert!(
            !scan.contains(needle),
            "export.rs must not contain {needle}"
        );
    }

    for line in rust_src("export.rs").lines() {
        let t = line.trim_start();
        if t.starts_with("use crate::") {
            for forbidden in ["stripe_client", "poll", "shop", "money", "offers"] {
                assert!(
                    !t.contains(forbidden),
                    "export.rs must not import crate::{forbidden}: {t}"
                );
            }
        }
    }

    for needle in [
        "api_key",
        "token",
        "login",
        "account",
        "subscription",
        "fee",
    ] {
        assert!(
            !scan.contains(needle),
            "export.rs production source must not contain {needle}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex5_every_exported_cost_row_carries_both_tax_lines_and_a_descriptor() {
    let dir = tempfile_dir("ex5");
    let mut conn = open_farm(&dir);
    let receipt_bytes = b"tax-prep-receipt-bytes";
    let receipt_path = write_receipt(&dir, "tax-receipt.pdf", receipt_bytes);
    costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 111,
            payee: "Ordinary".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    let with_receipt = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 222,
            payee: "Other Cat".into(),
            category_id: "market_stall_booth".into(),
            date_paid: today(),
            descriptor: Some("Friday booth".into()),
            receipt_source_path: Some(receipt_path.to_string_lossy().into()),
        },
    )
    .unwrap();
    flush(&conn, &dir);
    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let manifest = parse_manifest(&bundle);

    // (a) exported farm.db
    let exported = Connection::open(bundle.join("farm.db")).unwrap();
    let mut stmt = exported
        .prepare("SELECT schedule_f_line, schedule_c_line, descriptor FROM cost_events")
        .unwrap();
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(!rows.is_empty());
    for (f, c, d) in &rows {
        assert!(!f.is_empty());
        assert!(!c.is_empty());
        if categories::line_is_other(f) || categories::line_is_other(c) {
            assert!(!d.is_empty(), "descriptor required for other line");
        }
    }

    // (b) events.jsonl
    let jsonl = fs::read_to_string(bundle.join("events.jsonl")).unwrap();
    let mut money_out = 0;
    for line in jsonl.lines() {
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v.get("kind").and_then(|k| k.as_str()) == Some("cost.money_out") {
            money_out += 1;
            let p = &v["payload"];
            assert!(p.get("scheduleFLine").and_then(|x| x.as_str()).is_some());
            assert!(p.get("scheduleCLine").and_then(|x| x.as_str()).is_some());
            assert!(p.get("descriptor").and_then(|x| x.as_str()).is_some());
        }
    }
    assert!(money_out >= 2);

    // (c) costs.csv
    let csv = fs::read_to_string(bundle.join("costs.csv")).unwrap();
    let (header, data) = parse_csv(&csv);
    assert_eq!(
        header,
        [
            "event_id",
            "date_paid",
            "amount_cents",
            "payee",
            "canonical_category",
            "schedule_f_line",
            "schedule_c_line",
            "descriptor",
            "receipt_file_ref",
        ]
    );
    assert_eq!(data.len() as i64, manifest.counts.cost_events);
    let mut saw_receipt = false;
    for row in &data {
        assert!(!row[5].is_empty());
        assert!(!row[6].is_empty());
        if categories::line_is_other(&row[5]) || categories::line_is_other(&row[6]) {
            assert!(!row[7].is_empty());
        }
        if row[0] == with_receipt.event_id {
            assert!(!row[8].is_empty());
            let ref_path = bundle.join(row[8].replace('/', std::path::MAIN_SEPARATOR_STR));
            assert!(ref_path.exists(), "receipt ref must resolve inside bundle");
            assert_eq!(fs::read(&ref_path).unwrap(), receipt_bytes);
            saw_receipt = true;
        }
    }
    assert!(saw_receipt);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex6_second_export_of_the_same_state_matches() {
    let dir = tempfile_dir("ex6");
    let conn = seed_farm(&dir);
    let a = export::export_bundle(&conn, &dir).unwrap();
    let b = export::export_bundle(&conn, &dir).unwrap();
    let bundle_a = PathBuf::from(&a.bundle_path);
    let bundle_b = PathBuf::from(&b.bundle_path);

    for name in [
        "events.jsonl",
        "costs.csv",
        "income.csv",
        "assets.csv",
        "mileage.csv",
        "marketing.csv",
        "wholesale.csv",
        "receivables.csv",
        "income-corrections.csv",
        "categories.json",
    ] {
        assert_eq!(
            fs::read(bundle_a.join(name)).unwrap(),
            fs::read(bundle_b.join(name)).unwrap(),
            "{name} must be byte-identical"
        );
    }
    for entry in fs::read_dir(bundle_a.join("receipts")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        assert_eq!(
            fs::read(entry.path()).unwrap(),
            fs::read(bundle_b.join("receipts").join(&name)).unwrap()
        );
    }

    // VACUUM INTO is a SQLite binary artifact — byte-stability is not a
    // guarantee this project should depend on. Instead verify each export's
    // manifest checksum against its own farm.db, then compare content digests
    // of the projection tables as sorted row strings.
    let man_a = parse_manifest(&bundle_a);
    let man_b = parse_manifest(&bundle_b);
    let farm_a = man_a.files.iter().find(|f| f.path == "farm.db").unwrap();
    let farm_b = man_b.files.iter().find(|f| f.path == "farm.db").unwrap();
    let bytes_a = fs::read(bundle_a.join("farm.db")).unwrap();
    let bytes_b = fs::read(bundle_b.join("farm.db")).unwrap();
    assert_eq!(sha256_hex(&bytes_a), farm_a.sha256);
    assert_eq!(sha256_hex(&bytes_b), farm_b.sha256);

    let db_a = Connection::open(bundle_a.join("farm.db")).unwrap();
    let db_b = Connection::open(bundle_b.join("farm.db")).unwrap();
    for table in [
        "event_log",
        "cost_events",
        "consumption_events",
        "mileage_trips",
        "assets",
    ] {
        assert_eq!(
            sorted_table_digest(&db_a, table),
            sorted_table_digest(&db_b, table),
            "{table} content digest"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex7_export_mutates_nothing() {
    let dir = tempfile_dir("ex7");
    let conn = seed_farm(&dir);

    let before_seq = max_seq(&conn);
    let tables = table_names(&conn);
    let before_counts: HashMap<String, i64> = tables
        .iter()
        .map(|t| (t.clone(), row_count(&conn, t)))
        .collect();
    let events_path = crate::event_file::events_path(&dir);
    let before_events = sha256_hex(&fs::read(&events_path).unwrap_or_default());
    let mut before_receipts: HashMap<String, String> = HashMap::new();
    let receipts_dir = dir.join("receipts");
    if receipts_dir.exists() {
        for entry in fs::read_dir(&receipts_dir).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                before_receipts.insert(name, sha256_hex(&fs::read(entry.path()).unwrap()));
            }
        }
    }

    for _ in 0..3 {
        export::export_bundle(&conn, &dir).unwrap();
    }

    assert_eq!(max_seq(&conn), before_seq);
    for t in &tables {
        assert_eq!(row_count(&conn, t), before_counts[t], "table {t}");
    }
    assert_eq!(
        sha256_hex(&fs::read(&events_path).unwrap_or_default()),
        before_events
    );
    if receipts_dir.exists() {
        let mut after: HashMap<String, String> = HashMap::new();
        for entry in fs::read_dir(&receipts_dir).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                after.insert(name, sha256_hex(&fs::read(entry.path()).unwrap()));
            }
        }
        assert_eq!(after, before_receipts);
    }

    let kinds: Vec<String> = conn
        .prepare("SELECT kind FROM event_log")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(!kinds.iter().any(|k| k == "snapshot.taken"));

    let scan = production_scan("export.rs");
    for needle in [
        "take_snapshot",
        "try_take_snapshot",
        "flush_events",
        "try_flush_after_commit",
    ] {
        assert!(
            !scan.contains(needle),
            "export.rs must not contain {needle}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex8_csv_escaping_and_stability() {
    let dir = tempfile_dir("ex8");
    let mut conn = open_farm(&dir);
    costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 50,
            payee: r#"Acme, "Preferred" Supplier"#.into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Rack, \"tall\"\nunit".into(),
            placed_in_service_on: today(),
            cost_cents: 9000,
            disposal_date: None,
        },
    )
    .unwrap();
    mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 7.25,
            purpose: Some("Trip, \"local\"\nerrand".into()),
        },
    )
    .unwrap();
    flush(&conn, &dir);
    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);

    for name in ["costs.csv", "assets.csv", "mileage.csv"] {
        let bytes = fs::read(bundle.join(name)).unwrap();
        assert!(!bytes.contains(&0x0D), "{name} must use \\n only (no CR)");
    }

    let costs = fs::read_to_string(bundle.join("costs.csv")).unwrap();
    let (h, data) = parse_csv(&costs);
    assert_eq!(h.len(), 9);
    assert_eq!(
        h,
        [
            "event_id",
            "date_paid",
            "amount_cents",
            "payee",
            "canonical_category",
            "schedule_f_line",
            "schedule_c_line",
            "descriptor",
            "receipt_file_ref",
        ]
    );
    assert_eq!(data[0][3], r#"Acme, "Preferred" Supplier"#);

    let assets_csv = fs::read_to_string(bundle.join("assets.csv")).unwrap();
    let (h, data) = parse_csv(&assets_csv);
    assert_eq!(
        h,
        [
            "asset_id",
            "description",
            "placed_in_service_on",
            "cost_cents",
            "disposal_date",
        ]
    );
    assert_eq!(data[0][1], "Rack, \"tall\"\nunit");

    let miles = fs::read_to_string(bundle.join("mileage.csv")).unwrap();
    let (h, data) = parse_csv(&miles);
    assert_eq!(h, ["trip_id", "trip_date", "miles", "purpose"]);
    assert_eq!(data[0][3], "Trip, \"local\"\nerrand");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex9_voided_rows_excluded_and_counted() {
    let dir = tempfile_dir("ex9");
    let mut conn = open_farm(&dir);
    let t1 = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 1.0,
            purpose: Some("keep".into()),
        },
    )
    .unwrap();
    let t2 = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 2.0,
            purpose: Some("void me".into()),
        },
    )
    .unwrap();
    mileage::void_trip(&mut conn, &t2.trip_id).unwrap();
    let a1 = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "keep".into(),
            placed_in_service_on: today(),
            cost_cents: 100,
            disposal_date: None,
        },
    )
    .unwrap();
    let a2 = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "void me".into(),
            placed_in_service_on: today(),
            cost_cents: 200,
            disposal_date: None,
        },
    )
    .unwrap();
    assets::void_asset(&mut conn, &a2.asset_id).unwrap();
    flush(&conn, &dir);
    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let manifest = parse_manifest(&bundle);

    let (_, miles) = parse_csv(&fs::read_to_string(bundle.join("mileage.csv")).unwrap());
    let (_, assets_rows) = parse_csv(&fs::read_to_string(bundle.join("assets.csv")).unwrap());
    assert_eq!(miles.len(), 1);
    assert_eq!(assets_rows.len(), 1);
    assert_eq!(miles[0][0], t1.trip_id);
    assert_eq!(assets_rows[0][0], a1.asset_id);
    assert_eq!(manifest.counts.mileage_trips_excluded_voided, 1);
    assert_eq!(manifest.counts.assets_excluded_voided, 1);

    let notes = manifest.notes.join(" ");
    assert!(notes.contains("1 voided mileage"));
    assert!(notes.contains("1 voided assets") || notes.contains("1 voided asset"));

    let jsonl = fs::read_to_string(bundle.join("events.jsonl")).unwrap();
    assert!(jsonl.contains(&t2.trip_id));
    assert!(jsonl.contains(&a2.asset_id));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex10_flush_lag_refuses_and_writes_nothing() {
    let dir = tempfile_dir("ex10");
    let mut conn = open_farm(&dir);
    flush(&conn, &dir);
    // Advance event_log without flushing — watermark stays behind.
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, None).unwrap();
    let exports = dir.join("exports");
    let before = if exports.exists() {
        walk_bundle_files(&exports)
    } else {
        BTreeSet::new()
    };

    let err = export::export_bundle(&conn, &dir).unwrap_err();
    assert!(
        err.contains("Close Farm OS") && err.to_ascii_lowercase().contains("open"),
        "{err}"
    );
    let after = if exports.exists() {
        walk_bundle_files(&exports)
    } else {
        BTreeSet::new()
    };
    assert_eq!(before, after, "flush-lag refusal must write no new bundle");

    flush(&conn, &dir);
    let result = export::export_bundle(&conn, &dir).unwrap();
    let manifest = parse_manifest(Path::new(&result.bundle_path));
    assert_eq!(manifest.flush_lag, 0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ex11_manifest_notes_are_generated() {
    let dir_a = tempfile_dir("ex11a");
    let mut conn_a = open_farm(&dir_a);
    costs::record_cost(
        &mut conn_a,
        &dir_a,
        RecordCostInput {
            amount_cents: 10,
            payee: "A".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(
                write_receipt(&dir_a, "r1.bin", b"one")
                    .to_string_lossy()
                    .into(),
            ),
        },
    )
    .unwrap();
    let t = mileage::record_trip(
        &mut conn_a,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 1.0,
            purpose: None,
        },
    )
    .unwrap();
    mileage::void_trip(&mut conn_a, &t.trip_id).unwrap();
    flush(&conn_a, &dir_a);
    let a = export::export_bundle(&conn_a, &dir_a).unwrap();
    let man_a = parse_manifest(Path::new(&a.bundle_path));

    let dir_b = tempfile_dir("ex11b");
    let mut conn_b = open_farm(&dir_b);
    let a1 = assets::record_asset(
        &mut conn_b,
        RecordAssetInput {
            description: "x".into(),
            placed_in_service_on: today(),
            cost_cents: 1,
            disposal_date: None,
        },
    )
    .unwrap();
    let a2 = assets::record_asset(
        &mut conn_b,
        RecordAssetInput {
            description: "y".into(),
            placed_in_service_on: today(),
            cost_cents: 2,
            disposal_date: None,
        },
    )
    .unwrap();
    assets::void_asset(&mut conn_b, &a1.asset_id).unwrap();
    assets::void_asset(&mut conn_b, &a2.asset_id).unwrap();
    costs::record_cost(
        &mut conn_b,
        &dir_b,
        RecordCostInput {
            amount_cents: 20,
            payee: "B".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(
                write_receipt(&dir_b, "r2.bin", b"two")
                    .to_string_lossy()
                    .into(),
            ),
        },
    )
    .unwrap();
    costs::record_cost(
        &mut conn_b,
        &dir_b,
        RecordCostInput {
            amount_cents: 30,
            payee: "C".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(
                write_receipt(&dir_b, "r3.bin", b"three")
                    .to_string_lossy()
                    .into(),
            ),
        },
    )
    .unwrap();
    flush(&conn_b, &dir_b);
    let b = export::export_bundle(&conn_b, &dir_b).unwrap();
    let man_b = parse_manifest(Path::new(&b.bundle_path));

    assert_ne!(man_a.notes, man_b.notes);
    let notes_a = man_a.notes.join("\n");
    let notes_b = man_b.notes.join("\n");
    assert!(notes_a.contains(&man_a.counts.mileage_trips_excluded_voided.to_string()));
    assert!(notes_b.contains(&man_b.counts.assets_excluded_voided.to_string()));

    for notes in [&notes_a, &notes_b] {
        let lower = notes.to_ascii_lowercase();
        assert!(lower.contains("schedule f") && lower.contains("schedule c"));
        assert!(lower.contains("descriptor"));
        assert!(lower.contains("receipt"));
        assert!(lower.contains("integer cents"));
        assert!(lower.contains("cost per tray"));
        assert!(lower.contains("miles") && lower.contains("dollar"));
        assert!(lower.contains("income.csv") && lower.contains("record_type"));
        assert!(lower.contains("voided income") && lower.contains("refunded or disputed"));
    }

    let man_a_json = serde_json::to_string(&man_a).unwrap().to_ascii_lowercase();
    let man_b_json = serde_json::to_string(&man_b).unwrap().to_ascii_lowercase();
    assert!(!man_a_json.contains("costpertray"));
    assert!(!man_b_json.contains("costpertray"));

    let _ = fs::remove_dir_all(&dir_a);
    let _ = fs::remove_dir_all(&dir_b);
}

#[test]
fn ex12_track_6_adds_no_kind_no_table_no_schema_change() {
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

    // Keep walk_rs reachable for established-style helper inventory.
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut saw_export = false;
    walk_rs(&src_root, &mut |path, _src| {
        if path.file_name().and_then(|n| n.to_str()) == Some("export.rs") {
            saw_export = true;
        }
    });
    assert!(saw_export);

    let _ = Duration::days(0);
    let _ = HashSet::<String>::new();
}

#[test]
fn wa_reversed_allowance_leaves_write_offs_csv() {
    let dir = tempfile_dir("wa-write-offs");
    let mut conn = open_farm(&dir);
    let v =
        marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
    let d = today();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    wholesale::pay_order(
        &mut conn,
        &order.id,
        1200,
        &d,
        None,
        true,
        false,
        Some(WriteOffInput {
            category: "sales_discount".into(),
            reason: None,
        }),
    )
    .unwrap();
    flush(&conn, &dir);

    let first = export::export_bundle(&conn, &dir).unwrap();
    let bundle1 = PathBuf::from(&first.bundle_path);
    let csv1 = fs::read_to_string(bundle1.join("write-offs.csv")).unwrap();
    let (header1, data1) = parse_csv(&csv1);
    assert_eq!(data1.len(), 1);
    assert_eq!(data1[0][3], "400");

    wholesale::reverse_payment(&mut conn, &order.id, "").unwrap();
    flush(&conn, &dir);

    let second = export::export_bundle(&conn, &dir).unwrap();
    let bundle2 = PathBuf::from(&second.bundle_path);
    let csv2 = fs::read_to_string(bundle2.join("write-offs.csv")).unwrap();
    let (header2, data2) = parse_csv(&csv2);
    assert_eq!(header1, header2);
    assert!(data2.is_empty());

    let jsonl = fs::read_to_string(bundle2.join("events.jsonl")).unwrap();
    assert!(
        jsonl.contains("wholesale.write_off"),
        "history must keep the write-off event"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn booksa_income_csv_is_unchanged_by_the_shared_reader() {
    let dir = tempfile_dir("booksa-income-csv");
    let mut conn = open_farm(&dir);
    let recorded = income::record_income(
        &mut conn,
        &dir,
        RecordIncomeInput {
            amount_cents: 2500,
            source: "Market cash".into(),
            category_id: "produce_you_grew".into(),
            date_received: today(),
            descriptor: None,
            receipt_source_path: None,
        },
        false,
    )
    .unwrap();
    let tray = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let hd = trays::get_tray(&conn, &tray.id)
        .unwrap()
        .expected_harvest_date
        .expect("expected harvest date");
    conn.execute(
        "INSERT INTO offers
         (id, harvest_date, crop_id, price_cents, stripe_price_id,
          stripe_link_id, stripe_link_url, created_at)
         VALUES (?1, ?2, ?3, 2400, ?4, NULL, NULL, ?5)
         ON CONFLICT(harvest_date, crop_id) DO UPDATE SET
           stripe_price_id = excluded.stripe_price_id",
        params![
            "offer_price_unknown",
            hd,
            "dun-peas",
            "price_unknown",
            "2026-08-05T12:00:00.000Z"
        ],
    )
    .unwrap();
    let paid_at = "2026-08-05T12:00:00.000Z";
    let applied = money::apply_paid_session(
        &mut conn,
        &money::PaidSession {
            session_id: "cs_booksa_export".into(),
            payment_intent: Some("pi_cs_booksa_export".into()),
            lines: vec![money::PaidLine {
                price_id: "price_unknown".into(),
                quantity: 1,
                amount_cents: 2400,
            }],
            currency: "cad".into(),
            customer_email: Some("buyer@example.com".into()),
            paid_at: paid_at.into(),
            created: 800,
            amount_cents: 2400,
            client_reference: None,
            payment_link: None,
        },
    )
    .unwrap();
    assert!(matches!(
        applied,
        crate::models::AppliedOutcome::Applied { .. }
    ));
    let order_id: String = conn
        .query_row("SELECT id FROM orders WHERE state = 'paid'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let crop_name: String = conn
        .query_row("SELECT name FROM crops WHERE id = 'dun-peas'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let stripe_date = db::local_date_from_utc_rfc3339(paid_at).unwrap();
    let produce = categories::find_income_category("produce_you_grew").unwrap();
    flush(&conn, &dir);

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let csv = fs::read_to_string(bundle.join("income.csv")).unwrap();
    let (header, data) = parse_csv(&csv);
    assert_eq!(
        header,
        vec![
            "record_type",
            "income_id",
            "date_received",
            "amount_cents",
            "source",
            "canonical_category",
            "schedule_f_line",
            "schedule_c_line",
            "descriptor",
            "receipt_file_ref",
        ]
    );
    assert_eq!(data.len(), 2);

    let mut expected = vec![
        vec![
            "recorded".into(),
            recorded.income_id.clone(),
            recorded.date_received.clone(),
            "2500".into(),
            "Market cash".into(),
            produce.id.to_string(),
            produce.schedule_f_line.to_string(),
            produce.schedule_c_line.to_string(),
            String::new(),
            String::new(),
        ],
        vec![
            "stripe".into(),
            order_id,
            stripe_date,
            "2400".into(),
            format!("Online order — {crop_name}"),
            produce.id.to_string(),
            produce.schedule_f_line.to_string(),
            produce.schedule_c_line.to_string(),
            String::new(),
            String::new(),
        ],
    ];
    expected.sort_by(|a, b| (&a[2], &a[1]).cmp(&(&b[2], &b[1])));
    assert_eq!(data, expected);

    let manifest = parse_manifest(&bundle);
    assert_eq!(manifest.counts.income_events, 1);
    assert_eq!(manifest.counts.stripe_orders_paid, 1);
    let recorded_rows = data.iter().filter(|r| r[0] == "recorded").count() as i64;
    let stripe_rows = data.iter().filter(|r| r[0] == "stripe").count() as i64;
    assert_eq!(manifest.counts.income_events, recorded_rows);
    assert_eq!(manifest.counts.stripe_orders_paid, stripe_rows);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn booksc_costs_csv_is_unchanged_by_the_shared_reader() {
    let dir = tempfile_dir("booksc-costs-csv");
    let mut conn = open_farm(&dir);
    let early = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 500,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: "2026-06-02".into(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    let late = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 250,
            payee: "Booth LLC".into(),
            category_id: "market_stall_booth".into(),
            date_paid: "2026-06-10".into(),
            descriptor: Some("Saturday market".into()),
            receipt_source_path: None,
        },
    )
    .unwrap();
    flush(&conn, &dir);

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let csv = fs::read_to_string(bundle.join("costs.csv")).unwrap();
    let (header, data) = parse_csv(&csv);
    assert_eq!(
        header,
        vec![
            "event_id",
            "date_paid",
            "amount_cents",
            "payee",
            "canonical_category",
            "schedule_f_line",
            "schedule_c_line",
            "descriptor",
            "receipt_file_ref",
        ]
    );
    assert_eq!(data.len(), 2);
    assert!(data[0][1] <= data[1][1]);
    if data[0][1] == data[1][1] {
        assert!(data[0][0] <= data[1][0]);
    }

    let mut rows = costs::expense_rows_between(&conn, None, None).unwrap();
    rows.sort_by(|a, b| (&a.date_paid, &a.event_id).cmp(&(&b.date_paid, &b.event_id)));
    let expected: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let cat = categories::find_category(&row.canonical_category).unwrap();
            vec![
                row.event_id.clone(),
                row.date_paid.clone(),
                row.amount_cents.to_string(),
                row.payee.clone(),
                row.canonical_category.clone(),
                cat.schedule_f_line.to_string(),
                cat.schedule_c_line.to_string(),
                row.descriptor.clone(),
                row.receipt_file_ref.clone().unwrap_or_default(),
            ]
        })
        .collect();
    assert_eq!(data, expected);
    assert_eq!(data[0][0], early.event_id);
    assert_eq!(data[1][0], late.event_id);

    let manifest = parse_manifest(&bundle);
    assert_eq!(manifest.counts.cost_events, data.len() as i64);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e4_costs_csv_prints_the_recorded_schedule_lines() {
    let dir = tempfile_dir("e4-costs-csv");
    let mut conn = open_farm(&dir);
    let view = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 1800,
            payee: "Local Grow Supply".into(),
            category_id: "growing_medium".into(),
            date_paid: "2026-06-02".into(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    conn.execute(
        "UPDATE cost_events SET schedule_f_line = 'F-OLD', schedule_c_line = 'C-OLD'
         WHERE event_id = ?1",
        [&view.event_id],
    )
    .unwrap();
    flush(&conn, &dir);

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let csv = fs::read_to_string(bundle.join("costs.csv")).unwrap();
    let (header, data) = parse_csv(&csv);
    assert_eq!(
        header,
        vec![
            "event_id",
            "date_paid",
            "amount_cents",
            "payee",
            "canonical_category",
            "schedule_f_line",
            "schedule_c_line",
            "descriptor",
            "receipt_file_ref",
        ]
    );
    assert_eq!(data.len(), 1);
    assert_eq!(data[0][0], view.event_id);
    assert_eq!(data[0][1], view.date_paid);
    assert_eq!(data[0][2], view.amount_cents.to_string());
    assert_eq!(data[0][3], view.payee);
    assert_eq!(data[0][4], view.canonical_category);
    assert_eq!(data[0][5], "F-OLD");
    assert_eq!(data[0][6], "C-OLD");
    assert_eq!(data[0][7], view.descriptor);
    assert_eq!(
        data[0][8],
        view.receipt_file_ref.clone().unwrap_or_default()
    );

    let today_f = categories::find_category(&view.canonical_category)
        .unwrap()
        .schedule_f_line;
    assert_ne!(today_f, "F-OLD");
    let _ = fs::remove_dir_all(&dir);
}

/// EXPORT B (C1 security sweep). The bundle travels; the two connection
/// secrets stay home. The live farm.db keeps the Stripe restricted key and
/// the scan-endpoint pull token; the bundle's farm.db copy carries NULL in
/// both columns and not one byte of either value; the manifest hashed the
/// scrubbed file; the bundle root is still the fifteen items (no journal file
/// left beside the copy); the note says so in manifest.json. Snapshots are
/// not this fence's business and export never touches them (ex7).
#[test]
fn export_scrub_bundle_copy_keeps_secrets_home() {
    let dir = tempfile_dir("export-scrub");
    let conn = seed_farm(&dir);
    let key = "rk_test_export_scrub_stays_home_0001";
    let pull = "pull-token-export-scrub-stays-home-0001";
    money::store_stripe_key(
        &conn,
        key,
        &money::AccountInfo {
            account_id: "acct_export_scrub".into(),
            account_name: "Test Farm".into(),
            mode: "test".into(),
        },
    )
    .unwrap();
    scans::set_config(&conn, Some("https://scan.example.test"), Some(pull)).unwrap();
    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    // Live farm.db: both secrets still there.
    let live_key: Option<String> = conn
        .query_row(
            "SELECT restricted_key FROM stripe_config WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let live_pull: Option<String> = conn
        .query_row("SELECT pull_token FROM scan_config WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(live_key.as_deref(), Some(key));
    assert_eq!(live_pull.as_deref(), Some(pull));
    // Bundle copy: both NULL; the columns beside them untouched.
    let copy = Connection::open(bundle.join("farm.db")).unwrap();
    let (copy_key, copy_account): (Option<String>, Option<String>) = copy
        .query_row(
            "SELECT restricted_key, account_id FROM stripe_config WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let (copy_pull, copy_url): (Option<String>, Option<String>) = copy
        .query_row(
            "SELECT pull_token, endpoint_url FROM scan_config WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(copy_key, None);
    assert_eq!(copy_pull, None);
    assert_eq!(copy_account.as_deref(), Some("acct_export_scrub"));
    assert_eq!(copy_url.as_deref(), Some("https://scan.example.test"));
    drop(copy);
    // Not one byte of either value survives in the copy (secure_delete).
    let bytes = fs::read(bundle.join("farm.db")).unwrap();
    assert!(!bytes.windows(key.len()).any(|w| w == key.as_bytes()));
    assert!(!bytes.windows(pull.len()).any(|w| w == pull.as_bytes()));
    // The manifest hashed the scrubbed file; the root is still fifteen items.
    let manifest = parse_manifest(&bundle);
    let farm = manifest.files.iter().find(|f| f.path == "farm.db").unwrap();
    assert_eq!(sha256_hex(&bytes), farm.sha256);
    assert_eq!(farm.size_bytes, bytes.len() as u64);
    assert_eq!(bundle_root_names(&bundle).len(), 15);
    assert!(!bundle.join("farm.db-journal").exists());
    assert!(!bundle.join("farm.db-wal").exists());
    let noted = manifest
        .notes
        .iter()
        .any(|n| n == export_scrub::MANIFEST_NOTE);
    assert!(noted, "manifest.json must carry the EXPORT B note");
    let _ = fs::remove_dir_all(&dir);
}
