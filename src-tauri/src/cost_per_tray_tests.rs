//! Track 5 — derived cost-per-tray proofs (CP1-CP13).

use crate::assets::{self, RecordAssetInput};
use crate::consumption::UNIT_TRAY;
use crate::cost_per_tray::{self, CostPerTrayOutcome, CostPerTrayRequest};
use crate::costs::{self, RecordCostInput};
use crate::db;
use crate::events::Kind;
use crate::mileage::{self, RecordMileageTripInput};
use crate::models::HarvestInput;
use crate::projection;
use crate::trays;
use chrono::{Duration, Local};
use rusqlite::Connection;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn yesterday() -> String {
    (Local::now() - Duration::days(1))
        .format("%Y-%m-%d")
        .to_string()
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "farm-os-cost-per-tray-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn frontend_src(rel: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(rel),
    )
    .unwrap_or_else(|e| panic!("read {rel}: {e}"))
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
    strip_cfg_test_modules(&strip_doc_comments(&rust_src(rel)))
}

fn basic_cost(amount_cents: i64) -> RecordCostInput {
    RecordCostInput {
        amount_cents,
        payee: "Seed Co".into(),
        category_id: "growing_medium".into(),
        date_paid: today(),
        descriptor: None,
        receipt_source_path: None,
    }
}

fn record_cost(conn: &mut Connection, amount_cents: i64) -> costs::CostEventView {
    let dir = tempfile_dir("cost");
    costs::record_cost(conn, &dir, basic_cost(amount_cents)).unwrap()
}

fn record_cost_cat(
    conn: &mut Connection,
    amount_cents: i64,
    category_id: &str,
) -> costs::CostEventView {
    let dir = tempfile_dir("cost-cat");
    costs::record_cost(
        conn,
        &dir,
        RecordCostInput {
            amount_cents,
            payee: "Vendor".into(),
            category_id: category_id.into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap()
}

fn req(window: &str) -> CostPerTrayRequest {
    CostPerTrayRequest {
        window: window.into(),
        from: None,
        to: None,
        category_ids: None,
    }
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

fn walk_frontend(dir: &Path, f: &mut dyn FnMut(&Path, &str)) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name == "node_modules" || name == "dist" {
                continue;
            }
            walk_frontend(&path, f);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("ts") | Some("tsx")
        ) {
            let src = fs::read_to_string(&path).unwrap();
            f(&path, &src);
        }
    }
}

#[test]
fn cp1_figure_is_total_paid_over_total_trays() {
    let mut conn = mem();
    record_cost(&mut conn, 500);
    record_cost(&mut conn, 250);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, None).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, None).unwrap();

    let outcome = cost_per_tray::cost_per_tray(&conn, req("last_90")).unwrap();
    match outcome {
        CostPerTrayOutcome::Computed { figure, .. } => {
            assert_eq!(figure.total_paid_cents, 750);
            assert_eq!(figure.total_trays, 3.0);
            assert_eq!(figure.cents_per_tray, 750.0 / 3.0);
        }
        CostPerTrayOutcome::Refused { reason, .. } => {
            panic!("expected Computed, got Refused: {reason}")
        }
    }
}

#[test]
fn cp2_derivation_stores_nothing() {
    let mut conn = mem();
    record_cost(&mut conn, 400);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, None).unwrap();

    let tables = [
        "event_log",
        "cost_events",
        "consumption_events",
        "mileage_trips",
        "assets",
        "trays",
        "orders",
    ];
    let before_counts: Vec<i64> = tables.iter().map(|t| row_count(&conn, t)).collect();
    let before_names = table_names(&conn);

    let _ = cost_per_tray::cost_per_tray(&conn, req("last_30")).unwrap();
    let _ = cost_per_tray::cost_per_tray(&conn, req("ytd")).unwrap();
    let _ = cost_per_tray::cost_per_tray(
        &conn,
        CostPerTrayRequest {
            window: "last_90".into(),
            from: None,
            to: None,
            category_ids: Some(vec!["growing_medium".into()]),
        },
    )
    .unwrap();

    let after_counts: Vec<i64> = tables.iter().map(|t| row_count(&conn, t)).collect();
    let after_names = table_names(&conn);
    assert_eq!(before_counts, after_counts);
    assert_eq!(before_names, after_names);

    for name in &after_names {
        let lower = name.to_ascii_lowercase();
        assert!(
            !lower.contains("cost_per_tray")
                && !lower.contains("per_tray")
                && !lower.contains("derived"),
            "unexpected table name {name}"
        );
    }

    let scan = production_scan("cost_per_tray.rs");
    for needle in [
        "INSERT",
        "UPDATE ",
        "DELETE",
        "CREATE ",
        "DROP ",
        "ALTER ",
        "&mut Connection",
        "transaction(",
    ] {
        assert!(
            !scan.contains(needle),
            "cost_per_tray.rs production scan must not contain {needle}"
        );
    }
}

#[test]
fn cp3_method_statement_is_generated_not_static() {
    let mut conn = mem();
    record_cost(&mut conn, 300);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, None).unwrap();

    let a = match cost_per_tray::cost_per_tray(&conn, req("last_30")).unwrap() {
        CostPerTrayOutcome::Computed { method, .. } => method,
        CostPerTrayOutcome::Refused { reason, .. } => panic!("refused: {reason}"),
    };
    let b = match cost_per_tray::cost_per_tray(&conn, req("last_90")).unwrap() {
        CostPerTrayOutcome::Computed { method, .. } => method,
        CostPerTrayOutcome::Refused { reason, .. } => panic!("refused: {reason}"),
    };

    assert_ne!(a.payment_rule, b.payment_rule);
    assert_ne!(a.physical_rule, b.physical_rule);
    assert_ne!(a.window_label, b.window_label);
    assert!(a.payment_rule.contains(&a.window_from) && a.payment_rule.contains(&a.window_to));
    assert!(b.payment_rule.contains(&b.window_from) && b.payment_rule.contains(&b.window_to));
    assert!(a.payment_rule.contains(&a.payment_count.to_string()));
    assert!(b.payment_rule.contains(&b.payment_count.to_string()));
    assert!(a.physical_rule.contains(&a.window_from) && a.physical_rule.contains(&a.window_to));
    assert!(b.physical_rule.contains(&b.window_from) && b.physical_rule.contains(&b.window_to));
    assert!(a.physical_rule.contains(&a.tray_record_count.to_string()));
    assert!(b.physical_rule.contains(&b.tray_record_count.to_string()));

    assert_eq!(a.payments.len() as i64, a.payment_count);
    assert_eq!(a.tray_records.len() as i64, a.tray_record_count);
    for p in &a.payments {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cost_events WHERE event_id = ?1",
                [&p.event_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n, 1,
            "listed payment {} missing from cost_events",
            p.event_id
        );
    }

    record_cost_cat(&mut conn, 100, "seed");
    let narrowed = match cost_per_tray::cost_per_tray(
        &conn,
        CostPerTrayRequest {
            window: "last_90".into(),
            from: None,
            to: None,
            category_ids: Some(vec!["seed".into()]),
        },
    )
    .unwrap()
    {
        CostPerTrayOutcome::Computed { method, .. } => method,
        CostPerTrayOutcome::Refused { reason, .. } => panic!("refused: {reason}"),
    };
    assert!(
        narrowed.payment_rule.contains("Seed"),
        "expected category name Seed in {}",
        narrowed.payment_rule
    );
    assert!(!narrowed.payment_rule.contains("canonical_category"));
    assert!(
        narrowed
            .payment_rule
            .to_ascii_lowercase()
            .contains("not saved"),
        "{}",
        narrowed.payment_rule
    );
}

#[test]
fn cp4_zero_trays_in_window_refuses() {
    let mut conn = mem();
    record_cost(&mut conn, 500);

    match cost_per_tray::cost_per_tray(&conn, req("last_90")).unwrap() {
        CostPerTrayOutcome::Refused { reason, method } => {
            assert!(
                reason.to_ascii_lowercase().contains("nothing to divide by"),
                "{reason}"
            );
            assert!(!method.window_label.is_empty());
            assert!(!method.payment_rule.is_empty());
            assert!(!method.physical_rule.is_empty());
            assert!(!method.join_rule.is_empty());
            assert!(!method.exclusion_rule.is_empty());
            assert_eq!(method.payment_count, 1);
            assert_eq!(method.payments.len(), 1);
            assert_eq!(method.tray_record_count, 0);
        }
        CostPerTrayOutcome::Computed { .. } => panic!("expected Refused"),
    }
}

#[test]
fn cp5_zero_payments_in_window_refuses() {
    let mut conn = mem();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, None).unwrap();

    match cost_per_tray::cost_per_tray(&conn, req("last_90")).unwrap() {
        CostPerTrayOutcome::Refused { reason, method } => {
            assert!(reason.contains("were free"), "{reason}");
            assert_eq!(method.total_trays, 2.0);
            assert_eq!(method.tray_record_count, 1);
            assert!(!method.physical_rule.is_empty());
            assert!(!method.payment_rule.is_empty());
        }
        CostPerTrayOutcome::Computed { .. } => panic!("expected Refused"),
    }
}

#[test]
fn cp6_harvest_records_never_enter_the_denominator() {
    let mut conn = mem();
    record_cost(&mut conn, 100);
    let tray = trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, None).unwrap();
    trays::advance_tray(&mut conn, &tray.id).unwrap();
    trays::harvest_groups(
        &mut conn,
        &[HarvestInput {
            tray_ids: vec![tray.id.clone()],
            actual_yield_oz: 12.0,
        }],
    )
    .unwrap();

    match cost_per_tray::cost_per_tray(&conn, req("last_90")).unwrap() {
        CostPerTrayOutcome::Computed { figure, method } => {
            assert_eq!(figure.total_trays, 1.0);
            assert_eq!(method.tray_record_count, 1);
            for rec in &method.tray_records {
                assert_ne!(rec.variety_or_item, "Dun peas");
                assert_eq!(rec.variety_or_item, "tray");
            }
        }
        CostPerTrayOutcome::Refused { reason, .. } => panic!("refused: {reason}"),
    }
}

#[test]
fn cp7_seed_oz_records_never_enter_the_denominator() {
    let mut conn = mem();
    record_cost(&mut conn, 100);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(16.0)).unwrap();

    let oz_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM consumption_events WHERE unit = 'oz'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(oz_count, 1);

    match cost_per_tray::cost_per_tray(&conn, req("last_90")).unwrap() {
        CostPerTrayOutcome::Computed { figure, method } => {
            assert_eq!(method.tray_record_count, 1);
            assert_eq!(figure.total_trays, 2.0);
        }
        CostPerTrayOutcome::Refused { reason, .. } => panic!("refused: {reason}"),
    }
}

#[test]
fn cp8_commercial_origin_payments_excluded() {
    let mut conn = mem();
    record_cost(&mut conn, 200);
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, None).unwrap();

    let now = db::utc_now_rfc3339();
    conn.execute(
        "INSERT INTO cost_events
         (event_id, origin, date_paid, amount_cents, payee, canonical_category,
          schedule_f_line, schedule_c_line, descriptor, quantity, unit_price_cents,
          delivery_date, invoice_reference, receipt_file_ref, created_at, updated_at)
         VALUES (?1, 'commercial_app', ?2, 9999, 'Commercial Co', 'seed',
                 '26', '22', '', NULL, NULL, NULL, NULL, NULL, ?3, ?3)",
        rusqlite::params!["commercial-cost-1", today(), now],
    )
    .unwrap();

    match cost_per_tray::cost_per_tray(&conn, req("last_90")).unwrap() {
        CostPerTrayOutcome::Computed { figure, method } => {
            assert_eq!(method.origin_filter, "farm_os");
            assert_eq!(method.payment_count, 1);
            assert_eq!(figure.total_paid_cents, 200);
            assert!(!method
                .payments
                .iter()
                .any(|p| p.event_id == "commercial-cost-1"));
            assert!(!method.payments.iter().any(|p| p.amount_cents == 9999));
        }
        CostPerTrayOutcome::Refused { reason, .. } => panic!("refused: {reason}"),
    }
}

#[test]
fn cp9_unknown_seed_stays_unknown_never_zero() {
    let mut conn = mem();
    let paid = record_cost(&mut conn, 400);
    trays::sow_tray_with_seed(&mut conn, "sunflower", 1, None).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();

    match cost_per_tray::cost_per_tray(&conn, req("last_90")).unwrap() {
        CostPerTrayOutcome::Computed { figure, method } => {
            assert_eq!(method.tray_records_with_seed_recorded, 1);
            assert_eq!(method.tray_records_without_seed_recorded, 1);
            assert!(method.completeness_note.contains('1'));
            assert!(
                method.completeness_note.contains('2')
                    || method.completeness_note.contains("1 of 2")
            );
            assert_eq!(figure.total_trays, 2.0);
            assert_eq!(figure.total_paid_cents, paid.amount_cents);
            assert_eq!(figure.total_paid_cents, 400);
        }
        CostPerTrayOutcome::Refused { reason, .. } => panic!("refused: {reason}"),
    }
}

#[test]
fn cp10_window_edge_uses_local_calendar_day() {
    let offset_secs = Local::now().offset().local_minus_utc();
    if offset_secs == 0 {
        eprintln!(
            "cp10_window_edge_uses_local_calendar_day: skip — machine is UTC+00:00; \
             local and UTC calendar days cannot diverge"
        );
        return;
    }

    // Pick a UTC instant whose UTC day differs from the local calendar day.
    let utc = if offset_secs < 0 {
        // Americas: early UTC morning is still previous local evening.
        "2026-06-15T02:00:00.000Z"
    } else {
        // East of UTC: late UTC evening is next local morning.
        "2026-06-14T22:00:00.000Z"
    };
    let local_day = db::local_date_from_utc_rfc3339(utc).unwrap();
    let utc_day = &utc[..10];
    assert_ne!(
        local_day, utc_day,
        "fixture must cross the local/UTC day boundary"
    );

    let mut conn = mem();
    // Payment must share the local calendar day under test (not "today").
    let dir = tempfile_dir("cp10");
    costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 100,
            payee: "Seed Co".into(),
            category_id: "growing_medium".into(),
            date_paid: local_day.clone(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, None).unwrap();
    conn.execute(
        "UPDATE consumption_events SET occurred_at = ?1 WHERE unit = ?2",
        rusqlite::params![utc, UNIT_TRAY],
    )
    .unwrap();

    let in_local = cost_per_tray::cost_per_tray(
        &conn,
        CostPerTrayRequest {
            window: "custom".into(),
            from: Some(local_day.clone()),
            to: Some(local_day.clone()),
            category_ids: None,
        },
    )
    .unwrap();
    match in_local {
        CostPerTrayOutcome::Computed { figure, method } => {
            assert_eq!(figure.total_trays, 1.0);
            assert_eq!(method.tray_records[0].occurred_on, local_day);
        }
        CostPerTrayOutcome::Refused { reason, method } => {
            panic!(
                "local-day window should include the tray (trays={}, reason={reason})",
                method.total_trays
            )
        }
    }

    let in_utc = cost_per_tray::cost_per_tray(
        &conn,
        CostPerTrayRequest {
            window: "custom".into(),
            from: Some(utc_day.to_string()),
            to: Some(utc_day.to_string()),
            category_ids: None,
        },
    )
    .unwrap();
    let (utc_trays, utc_tray_records) = match in_utc {
        CostPerTrayOutcome::Computed { figure, method } => {
            (figure.total_trays, method.tray_record_count)
        }
        CostPerTrayOutcome::Refused { method, .. } => {
            (method.total_trays, method.tray_record_count)
        }
    };
    assert_eq!(
        utc_tray_records, 0,
        "UTC-day window must not include the tray"
    );
    assert_eq!(utc_trays, 0.0, "UTC-day window must not include the tray");
}

#[test]
fn cp11_nothing_else_in_the_product_reads_the_derivation() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    // The guarantee is that PRODUCTION code does not read the derivation.
    // Test files are the exception as a CATEGORY, not by name — enumerating
    // them made this assertion fight every new track that needed to prove
    // something about the derivation. `_tests.rs` is the same predicate
    // choke_point_tests::is_test_source already uses to classify test sources.
    let allowed_production: HashSet<&str> = ["cost_per_tray.rs", "commands.rs", "lib.rs"]
        .into_iter()
        .collect();

    walk_rs(&root, &mut |path, src| {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.ends_with("_tests.rs") {
            return;
        }
        if src.contains("cost_per_tray") || src.contains("costPerTray") {
            assert!(
                allowed_production.contains(name),
                "{} must not mention cost_per_tray / costPerTray",
                path.display()
            );
        }
    });

    let fe_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("src");
    // Today.tsx no longer names the derivation — the door moved to the shared
    // money-capture control (ruling 6.3). Dropping Today.tsx here is a
    // tightening: an allowlist entry for a file that no longer needs one is a
    // hole waiting for the next edit.
    let allowed_fe: HashSet<&str> = [
        "CostPerTraySheet.tsx",
        "api.ts",
        "types.ts",
        "MoneyCaptureControls.tsx",
    ]
    .into_iter()
    .collect();

    walk_frontend(&fe_root, &mut |path, src| {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if src.contains("cost_per_tray") || src.contains("costPerTray") {
            assert!(
                allowed_fe.contains(name),
                "{} must not mention cost_per_tray / costPerTray",
                path.display()
            );
        }
    });

    // The sheet is opened from the shared control now. The rule it enforces is
    // the same one and is applied to every surface that reaches the control:
    // a surface opens the sheet, and never reads the derivation itself.
    let controls = frontend_src("src/components/MoneyCaptureControls.tsx");
    assert!(controls.contains("CostPerTraySheet"));
    assert!(controls.contains("costPerTrayOpen"));

    let today = frontend_src("src/screens/Today.tsx");
    let reality = frontend_src("src/screens/Reality.tsx");
    let money = frontend_src("src/screens/Money.tsx");
    for (name, src) in [
        ("MoneyCaptureControls.tsx", &controls),
        ("Today.tsx", &today),
        ("Reality.tsx", &reality),
        ("Money.tsx", &money),
    ] {
        assert!(
            !src.contains("costPerTray("),
            "{name} must never call costPerTray()"
        );
        assert!(
            !src.contains("costPerTray,") && !src.contains("{ costPerTray"),
            "{name} must not import costPerTray from the api"
        );
    }

    for sheet in [
        "MoneyJustLeftSheet.tsx",
        "SowSheet.tsx",
        "WeightPad.tsx",
        "RecountSheet.tsx",
        "SellOnlineSheet.tsx",
        "CropsSheet.tsx",
        "MilesSheet.tsx",
        "EquipmentSheet.tsx",
        "FarmBackupSheet.tsx",
    ] {
        let src = frontend_src(&format!("src/components/{sheet}"));
        assert!(
            !src.contains("cost_per_tray")
                && !src.contains("costPerTray")
                && !src.contains("CostPerTray"),
            "{sheet} must not mention cost-per-tray"
        );
    }
}

#[test]
fn cp12_track_5_adds_no_kind_and_no_table() {
    // Bumped for the silent-shortfall residual (wholesale.write_off).
    assert_eq!(Kind::ALL.len(), 54);

    let conn = mem();
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
}

#[test]
fn cp13_verify_replay_unaffected() {
    let dir = tempfile_dir("cp13");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();

    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    costs::record_cost(&mut conn, &dir, basic_cost(750)).unwrap();
    mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 12.0,
            purpose: Some("market".into()),
        },
    )
    .unwrap();
    assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Rack".into(),
            placed_in_service_on: yesterday(),
            cost_cents: 25_000,
            disposal_date: None,
        },
    )
    .unwrap();

    crate::event_file::try_flush_after_commit(&conn, &dir);
    let snaps = dir.join("snapshots");
    fs::create_dir_all(&snaps).unwrap();
    crate::snapshots::take_snapshot(&mut conn, &snaps).unwrap();

    for window in ["last_30", "last_90", "ytd", "all"] {
        let _ = cost_per_tray::cost_per_tray(&conn, req(window)).unwrap();
    }
    let _ = cost_per_tray::cost_per_tray(
        &conn,
        CostPerTrayRequest {
            window: "last_90".into(),
            from: None,
            to: None,
            category_ids: Some(vec!["growing_medium".into()]),
        },
    )
    .unwrap();

    crate::event_file::try_flush_after_commit(&conn, &dir);
    drop(conn);

    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify failed: {}",
        outcome.summary_line()
    );
    let line = outcome.summary_line();
    assert!(
        line.starts_with("VERIFY-REPLAY: PASS"),
        "expected PASS or PASS WITH KNOWN, got {line}"
    );
    let report = outcome.report();
    assert_eq!(report.flush_lag, 0);
    assert_eq!(report.tables_compared, 26);

    let _ = fs::remove_dir_all(&dir);
}
