//! Track 4 residual — mileage + asset register proofs (M1-M7, A1-A6, S1-S5).

use crate::assets::{
    self, AssetPayload, CorrectAssetInput, RecordAssetInput, ASSETS_COLUMNS,
    ASSET_FORBIDDEN_COMPUTED_KEYS, ASSET_PAYLOAD_FIELD_NAMES, ASSET_PAYLOAD_KEYS,
    ASSET_SPINE_COLUMNS, ASSET_VOID_PAYLOAD_KEYS,
};
use crate::costs::{self, RecordCostInput};
use crate::db;
use crate::event_partition::{self, grow_kinds, register_kinds, GROW_KINDS, REGISTER_KINDS};
use crate::events::{self, EventRecord, Kind};
use crate::mileage::{
    self, CorrectMileageTripInput, MileageTripPayload, RecordMileageTripInput,
    MILEAGE_FORBIDDEN_KEYS, MILEAGE_SPINE_COLUMNS, MILEAGE_TRIPS_COLUMNS,
    MILEAGE_TRIP_PAYLOAD_FIELD_NAMES, MILEAGE_TRIP_PAYLOAD_KEYS,
};
use crate::projection;
use crate::trays;
use chrono::{Duration, Local};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

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

fn tomorrow() -> String {
    (Local::now() + Duration::days(1))
        .format("%Y-%m-%d")
        .to_string()
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "farm-os-mileage-asset-{}-{}",
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

fn strip_forbidden_keys_block(src: &str) -> String {
    const BEGIN: &str = "// FORBIDDEN-KEYS-BEGIN";
    const END: &str = "// FORBIDDEN-KEYS-END";
    if let (Some(a), Some(b)) = (src.find(BEGIN), src.find(END)) {
        let mut out = String::new();
        out.push_str(&src[..a]);
        out.push_str(&src[b + END.len()..]);
        out
    } else {
        src.to_string()
    }
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
    // Drop `#[cfg(test)] mod … { … }` so seal tests may name forbidden keys.
    let mut out = String::new();
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' && chars.peek() == Some(&'[') {
            let rest: String = chars.clone().collect();
            if rest.starts_with("[cfg(test)]") {
                // skip attribute
                for _ in 0.."[cfg(test)]".len() {
                    chars.next();
                }
                // skip whitespace/newlines
                while matches!(
                    chars.peek(),
                    Some(' ') | Some('\n') | Some('\r') | Some('\t')
                ) {
                    chars.next();
                }
                // skip `mod name`
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

fn production_scan_src(rel: &str) -> String {
    let raw = rust_src(rel);
    strip_cfg_test_modules(&strip_doc_comments(&strip_forbidden_keys_block(&raw)))
}

fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|c| c.unwrap())
        .collect()
}

/// Full PRAGMA table_info tuple list (name, type, notnull, dflt_value, pk), in order.
fn table_info_tuples(
    conn: &Connection,
    table: &str,
) -> Vec<(String, String, i32, Option<String>, i32)> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i32>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, i32>(5)?,
        ))
    })
    .unwrap()
    .map(|c| c.unwrap())
    .collect()
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

fn event_log_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap()
}

fn mileage_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM mileage_trips", [], |r| r.get(0))
        .unwrap()
}

fn assets_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
        .unwrap()
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

fn open_v12_fixture() -> Connection {
    let conn = db::open_v11_in_memory().unwrap();
    // v12 additive column.
    conn.execute_batch("ALTER TABLE consumption_events ADD COLUMN sow_event_id TEXT;")
        .unwrap();
    db::drop_event_log_triggers(&conn).unwrap();
    conn.execute_batch(&event_partition::schema_v12_event_log_triggers_sql())
        .unwrap();
    conn.pragma_update(None, "user_version", 12).unwrap();
    conn
}

fn norm_sql(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn master_sql(conn: &Connection, name: &str) -> String {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE name = ?1",
        [name],
        |r| r.get::<_, Option<String>>(0),
    )
    .unwrap()
    .unwrap_or_default()
}

// ─── MILEAGE ───────────────────────────────────────────────────────────────

#[test]
fn m1_mileage_trip_lands_register_mileage_farm_os() {
    let mut conn = mem();
    let view = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 12.4,
            purpose: Some("market".into()),
        },
    )
    .unwrap();

    let (kind, domain, class, origin, id): (String, String, String, String, String) = conn
        .query_row(
            "SELECT kind, event_domain, event_class, origin, id FROM event_log
             WHERE kind = 'mileage.trip' ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(kind, "mileage.trip");
    assert_eq!(domain, "register");
    assert_eq!(class, "mileage");
    assert_eq!(origin, "farm_os");
    assert!(!id.is_empty());
    assert_eq!(view.trip_id, id);
    assert_eq!(view.origin, "farm_os");

    let (trip_id, miles, row_origin): (String, f64, String) = conn
        .query_row(
            "SELECT trip_id, miles, origin FROM mileage_trips WHERE trip_id = ?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(trip_id, id);
    assert_eq!(miles, 12.4);
    assert_eq!(row_origin, "farm_os");
}

#[test]
fn m2_mileage_stored_value_is_miles_not_derived() {
    let mut conn = mem();
    mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 12.4,
            purpose: None,
        },
    )
    .unwrap();
    let stored: f64 = conn
        .query_row("SELECT miles FROM mileage_trips", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, 12.4);

    let cols = table_columns(&conn, "mileage_trips");
    assert!(cols.iter().any(|c| c == "miles"));
    for c in &cols {
        let lower = c.to_ascii_lowercase();
        for needle in [
            "cent", "dollar", "amount", "price", "rate", "value", "cost", "usd", "total",
        ] {
            assert!(
                !lower.contains(needle),
                "mileage_trips column {c} looks monetary"
            );
        }
        assert_ne!(c.as_str(), "unit");
        assert_ne!(c.as_str(), "km");
        assert_ne!(c.as_str(), "kilometres");
        assert_ne!(c.as_str(), "kilometers");
    }
}

#[test]
fn m3_mileage_choke_rejects_monetary_key_zero_delta() {
    let mut conn = mem();
    let before_events = event_log_count(&conn);
    let before_trips = mileage_count(&conn);

    let event_id = "m3-trip";
    let base = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "tripId": event_id,
        "tripDate": today(),
        "miles": 10.0,
    });

    for (key, val) in [
        ("rate", json!(0.7)),
        ("amountCents", json!(500)),
        ("dollars", json!(3.5)),
    ] {
        let mut payload = base.clone();
        payload
            .as_object_mut()
            .unwrap()
            .insert(key.into(), val.clone());
        assert!(
            serde_json::from_value::<MileageTripPayload>(payload.clone()).is_err(),
            "serde must reject {key}"
        );

        let event = EventRecord::originated(
            Kind::MileageTripLogged,
            "mileage_trip",
            event_id,
            payload,
            json!({ "op": "none" }),
            projection::handler_now(),
            None,
            None,
            Some(event_id.into()),
        );
        let tx = conn.transaction().unwrap();
        let err = events::insert_event(&tx, &event).unwrap_err();
        assert!(
            err.contains("monetary") || err.contains(key),
            "{key}: {err}"
        );
        drop(tx);

        assert_eq!(event_log_count(&conn), before_events);
        assert_eq!(mileage_count(&conn), before_trips);
    }
    let _ = base;
}

#[test]
fn m4_mileage_correction_updates_row_and_appends_event() {
    let mut conn = mem();
    let original = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: yesterday(),
            miles: 5.0,
            purpose: Some("old".into()),
        },
    )
    .unwrap();

    let original_event: (String, String, Option<String>, Option<i64>, String) = conn
        .query_row(
            "SELECT id, kind, reverses_event_id, undoes_seq, payload FROM event_log WHERE id = ?1",
            [&original.trip_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    let created_at_before: String = conn
        .query_row(
            "SELECT created_at FROM mileage_trips WHERE trip_id = ?1",
            [&original.trip_id],
            |r| r.get(0),
        )
        .unwrap();

    let corrected = mileage::correct_trip(
        &mut conn,
        CorrectMileageTripInput {
            trip_id: original.trip_id.clone(),
            trip_date: today(),
            miles: 9.5,
            purpose: Some("new".into()),
        },
    )
    .unwrap();

    let rows: i64 = mileage_count(&conn);
    assert_eq!(rows, 1);
    assert_eq!(corrected.miles, 9.5);
    assert_eq!(corrected.trip_date, today());
    assert_eq!(corrected.purpose.as_deref(), Some("new"));
    assert_eq!(corrected.last_event_id, corrected.last_event_id);
    assert_ne!(corrected.last_event_id, original.trip_id);

    let (miles, trip_date, purpose, last_event_id, created_at, updated_at): (
        f64,
        String,
        Option<String>,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT miles, trip_date, purpose, last_event_id, created_at, updated_at
             FROM mileage_trips WHERE trip_id = ?1",
            [&original.trip_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(miles, 9.5);
    assert_eq!(trip_date, today());
    assert_eq!(purpose.as_deref(), Some("new"));
    assert_eq!(last_event_id, corrected.last_event_id);
    assert_eq!(created_at, created_at_before);

    let corr_created: String = conn
        .query_row(
            "SELECT created_at FROM event_log WHERE id = ?1",
            [&last_event_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(updated_at, corr_created);

    let log_n = event_log_count(&conn);
    assert_eq!(log_n, 2);

    let (rev, undoes): (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT reverses_event_id, undoes_seq FROM event_log WHERE id = ?1",
            [&last_event_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rev.as_deref(), Some(original.trip_id.as_str()));
    assert!(undoes.is_none());

    let after_original: (String, String, Option<String>, Option<i64>, String) = conn
        .query_row(
            "SELECT id, kind, reverses_event_id, undoes_seq, payload FROM event_log WHERE id = ?1",
            [&original.trip_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(after_original, original_event);
}

#[test]
fn m5_mileage_void_retires_trip_and_list_excludes_it() {
    let mut conn = mem();
    let a = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 1.0,
            purpose: None,
        },
    )
    .unwrap();
    let b = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: yesterday(),
            miles: 2.0,
            purpose: None,
        },
    )
    .unwrap();

    mileage::void_trip(&mut conn, &a.trip_id).unwrap();
    assert_eq!(mileage_count(&conn), 2);
    let voided: Option<String> = conn
        .query_row(
            "SELECT voided_at FROM mileage_trips WHERE trip_id = ?1",
            [&a.trip_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(voided.is_some());
    let listed = mileage::list_trips(&conn).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].trip_id, b.trip_id);

    let before = event_log_count(&conn);
    assert!(mileage::void_trip(&mut conn, &a.trip_id).is_err());
    assert_eq!(event_log_count(&conn), before);

    assert!(mileage::correct_trip(
        &mut conn,
        CorrectMileageTripInput {
            trip_id: a.trip_id,
            trip_date: today(),
            miles: 3.0,
            purpose: None,
        },
    )
    .is_err());
    assert_eq!(event_log_count(&conn), before);
}

#[test]
fn m6_mileage_rejects_future_date_and_bad_miles() {
    let mut conn = mem();
    let before_e = event_log_count(&conn);
    let before_t = mileage_count(&conn);

    assert!(mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: tomorrow(),
            miles: 1.0,
            purpose: None,
        },
    )
    .is_err());
    assert_eq!(event_log_count(&conn), before_e);
    assert_eq!(mileage_count(&conn), before_t);

    for miles in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            mileage::record_trip(
                &mut conn,
                RecordMileageTripInput {
                    trip_date: today(),
                    miles,
                    purpose: None,
                },
            )
            .is_err(),
            "miles={miles}"
        );
        assert_eq!(event_log_count(&conn), before_e);
        assert_eq!(mileage_count(&conn), before_t);
    }
}

#[test]
fn m7_mileage_source_carries_no_money_vocabulary() {
    let src = production_scan_src("mileage.rs");
    let lower = src.to_ascii_lowercase();
    for needle in ["cents", "dollar", "0.7", "0.67", "irs"] {
        assert!(
            !lower.contains(needle),
            "mileage.rs production source contains {needle}"
        );
    }
    // "* rate" — word "rate" as a token (not e.g. "crate")
    for token in lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        assert_ne!(token, "rate", "mileage.rs production source contains rate");
    }

    let sheet = frontend_src("src/components/MilesSheet.tsx");
    assert!(!sheet.contains('$'), "MilesSheet must not show $");
    assert!(!sheet.contains("Cents"));
    assert!(!sheet.contains("recordCost"));
    assert!(!sheet.contains("parseDollarsToCents"));

    let api = frontend_src("src/farm/api.ts");
    let start = api
        .find("export function listMileageTrips")
        .expect("mileage wrappers");
    let end = api[start..]
        .find("export function listAssets")
        .map(|i| start + i)
        .unwrap_or(api.len());
    let wrappers = &api[start..end];
    assert!(!wrappers.contains("recordCost"));
    assert!(!wrappers.contains("parseDollars"));
    assert!(!wrappers.contains("$"));
}

// ─── ASSETS ────────────────────────────────────────────────────────────────

#[test]
fn a1_asset_lands_register_asset_register_farm_os() {
    let mut conn = mem();
    let view = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Walk-in cooler".into(),
            placed_in_service_on: today(),
            cost_cents: 250_000,
            disposal_date: None,
        },
    )
    .unwrap();

    let (kind, domain, class, origin, id): (String, String, String, String, String) = conn
        .query_row(
            "SELECT kind, event_domain, event_class, origin, id FROM event_log
             WHERE kind = 'asset.recorded' ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(kind, "asset.recorded");
    assert_eq!(domain, "register");
    assert_eq!(class, "asset_register");
    assert_eq!(origin, "farm_os");
    assert!(!id.is_empty());
    assert_eq!(view.asset_id, id);
    assert_eq!(view.origin, "farm_os");
    assert_eq!(view.cost_cents, 250_000);
}

#[test]
fn a2_asset_stores_exactly_the_four_operator_fields() {
    let conn = mem();
    let cols: HashSet<String> = table_columns(&conn, "assets").into_iter().collect();
    let expected: HashSet<String> = ASSETS_COLUMNS.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(cols, expected);

    for c in &cols {
        let lower = c.to_ascii_lowercase();
        for needle in [
            "deprec",
            "179",
            "macrs",
            "salvage",
            "basis",
            "book",
            "remaining",
            "life",
            "method",
            "convention",
            "schedule",
        ] {
            assert!(
                !lower.contains(needle),
                "assets column {c} looks computed ({needle})"
            );
        }
    }
}

#[test]
fn a3_asset_computes_nothing() {
    let mut conn = mem();
    let view = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Tray rack".into(),
            placed_in_service_on: yesterday(),
            cost_cents: 12_500,
            disposal_date: None,
        },
    )
    .unwrap();
    let v = serde_json::to_value(&view).unwrap();
    let obj = v.as_object().unwrap();
    let keys: HashSet<&str> = obj.keys().map(|s| s.as_str()).collect();
    let expected: HashSet<&str> = [
        "assetId",
        "origin",
        "description",
        "placedInServiceOn",
        "costCents",
        "disposalDate",
        "lastEventId",
        "createdAt",
        "updatedAt",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected);

    let src = production_scan_src("assets.rs");
    for pat in [
        "cost_cents *",
        "cost_cents /",
        "* cost_cents",
        "/ cost_cents",
    ] {
        assert!(
            !src.contains(pat),
            "assets.rs operates on cost_cents: {pat}"
        );
    }
    let lower = src.to_ascii_lowercase();
    for forbidden in ASSET_FORBIDDEN_COMPUTED_KEYS {
        assert!(
            !lower.contains(&forbidden.to_ascii_lowercase()),
            "assets.rs contains forbidden vocabulary {forbidden}"
        );
    }
}

#[test]
fn a4_asset_disposal_date_set_later_via_correction() {
    let mut conn = mem();
    let original = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Van".into(),
            placed_in_service_on: yesterday(),
            cost_cents: 900_000,
            disposal_date: None,
        },
    )
    .unwrap();
    let disposal_before: Option<String> = conn
        .query_row(
            "SELECT disposal_date FROM assets WHERE asset_id = ?1",
            [&original.asset_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(disposal_before.is_none());
    let created_at = original.created_at.clone();

    let corrected = assets::correct_asset(
        &mut conn,
        CorrectAssetInput {
            asset_id: original.asset_id.clone(),
            description: original.description.clone(),
            placed_in_service_on: original.placed_in_service_on.clone(),
            cost_cents: original.cost_cents,
            disposal_date: Some(today()),
        },
    )
    .unwrap();

    assert_eq!(corrected.disposal_date.as_deref(), Some(today().as_str()));
    assert_eq!(corrected.description, original.description);
    assert_eq!(
        corrected.placed_in_service_on,
        original.placed_in_service_on
    );
    assert_eq!(corrected.cost_cents, original.cost_cents);
    assert_eq!(corrected.created_at, created_at);
    assert_eq!(corrected.last_event_id, corrected.last_event_id);
    assert_ne!(corrected.last_event_id, original.asset_id);
    assert_eq!(event_log_count(&conn), 2);

    let rev: Option<String> = conn
        .query_row(
            "SELECT reverses_event_id FROM event_log WHERE id = ?1",
            [&corrected.last_event_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rev.as_deref(), Some(original.asset_id.as_str()));
}

#[test]
fn a5_asset_rejects_bad_input_zero_delta() {
    let mut conn = mem();
    let before_e = event_log_count(&conn);
    let before_a = assets_count(&conn);

    assert!(assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "   ".into(),
            placed_in_service_on: today(),
            cost_cents: 100,
            disposal_date: None,
        },
    )
    .is_err());
    for cost in [0_i64, -1] {
        assert!(assets::record_asset(
            &mut conn,
            RecordAssetInput {
                description: "x".into(),
                placed_in_service_on: today(),
                cost_cents: cost,
                disposal_date: None,
            },
        )
        .is_err());
    }
    assert!(assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "x".into(),
            placed_in_service_on: tomorrow(),
            cost_cents: 100,
            disposal_date: None,
        },
    )
    .is_err());
    assert!(assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "x".into(),
            placed_in_service_on: today(),
            cost_cents: 100,
            disposal_date: Some(yesterday()),
        },
    )
    .is_err());
    assert_eq!(event_log_count(&conn), before_e);
    assert_eq!(assets_count(&conn), before_a);

    let event_id = "a5-asset";
    for key in ["depreciation", "section179", "usefulLife"] {
        let mut payload = json!({
            "eventId": event_id,
            "origin": "farm_os",
            "assetId": event_id,
            "description": "x",
            "placedInServiceOn": today(),
            "costCents": 100,
        });
        payload
            .as_object_mut()
            .unwrap()
            .insert(key.into(), json!(1));
        let event = EventRecord::originated(
            Kind::AssetRecorded,
            "asset",
            event_id,
            payload,
            json!({ "op": "none" }),
            projection::handler_now(),
            None,
            None,
            Some(event_id.into()),
        );
        let tx = conn.transaction().unwrap();
        let err = events::insert_event(&tx, &event).unwrap_err();
        assert!(
            err.contains("computed") || err.contains(key),
            "{key}: {err}"
        );
        drop(tx);
        assert_eq!(event_log_count(&conn), before_e);
        assert_eq!(assets_count(&conn), before_a);
    }
}

#[test]
fn a6_asset_source_carries_no_depreciation_vocabulary() {
    let src = production_scan_src("assets.rs");
    let lower = src.to_ascii_lowercase();
    for forbidden in ASSET_FORBIDDEN_COMPUTED_KEYS {
        assert!(
            !lower.contains(&forbidden.to_ascii_lowercase()),
            "assets.rs contains {forbidden}"
        );
    }
    let sheet = frontend_src("src/components/EquipmentSheet.tsx");
    let sheet_l = sheet.to_ascii_lowercase();
    for needle in [
        "depreciation",
        "section 179",
        "section179",
        "macrs",
        "book value",
        "useful life",
    ] {
        assert!(
            !sheet_l.contains(needle),
            "EquipmentSheet contains {needle}"
        );
    }
}

#[test]
fn a7_asset_void_retires_row_and_list_excludes_it() {
    let mut conn = mem();
    let a = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Keep".into(),
            placed_in_service_on: yesterday(),
            cost_cents: 1000,
            disposal_date: None,
        },
    )
    .unwrap();
    let b = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Remove".into(),
            placed_in_service_on: today(),
            cost_cents: 2000,
            disposal_date: None,
        },
    )
    .unwrap();

    #[allow(clippy::type_complexity)]
    // H-7 Class E: a query_row tuple, not a signature. Nothing here to narrow.
    let survivor_before: (
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        String,
        String,
        String,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT asset_id, origin, description, placed_in_service_on, cost_cents,
                    disposal_date, last_event_id, created_at, updated_at, voided_at
             FROM assets WHERE asset_id = ?1",
            [&a.asset_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .unwrap();

    assets::void_asset(&mut conn, &b.asset_id).unwrap();
    assert_eq!(assets_count(&conn), 2);

    let (voided_at, last_event_id): (Option<String>, String) = conn
        .query_row(
            "SELECT voided_at, last_event_id FROM assets WHERE asset_id = ?1",
            [&b.asset_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(voided_at.is_some());
    let (void_created_at, void_id): (String, String) = conn
        .query_row(
            "SELECT created_at, id FROM event_log
             WHERE kind = 'asset.voided' AND entity_id = ?1
             ORDER BY seq DESC LIMIT 1",
            [&b.asset_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(voided_at.as_deref(), Some(void_created_at.as_str()));
    assert_eq!(last_event_id, void_id);

    let listed = assets::list_assets(&conn).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].asset_id, a.asset_id);

    #[allow(clippy::type_complexity)]
    // H-7 Class E: a query_row tuple, not a signature. Nothing here to narrow.
    let survivor_after: (
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        String,
        String,
        String,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT asset_id, origin, description, placed_in_service_on, cost_cents,
                    disposal_date, last_event_id, created_at, updated_at, voided_at
             FROM assets WHERE asset_id = ?1",
            [&a.asset_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(survivor_before, survivor_after);
}

#[test]
fn a8_second_void_and_correction_of_voided_asset_error() {
    let mut conn = mem();
    let a = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Mis-entered".into(),
            placed_in_service_on: today(),
            cost_cents: 5000,
            disposal_date: None,
        },
    )
    .unwrap();
    assets::void_asset(&mut conn, &a.asset_id).unwrap();

    let before_e = event_log_count(&conn);
    let row_before: (Option<String>, String, String) = conn
        .query_row(
            "SELECT voided_at, last_event_id, updated_at FROM assets WHERE asset_id = ?1",
            [&a.asset_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    let err_void = assets::void_asset(&mut conn, &a.asset_id).unwrap_err();
    assert_eq!(err_void, "that equipment was removed");
    assert_eq!(event_log_count(&conn), before_e);
    let row_after_void: (Option<String>, String, String) = conn
        .query_row(
            "SELECT voided_at, last_event_id, updated_at FROM assets WHERE asset_id = ?1",
            [&a.asset_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row_before, row_after_void);

    let err_correct = assets::correct_asset(
        &mut conn,
        CorrectAssetInput {
            asset_id: a.asset_id.clone(),
            description: "Nope".into(),
            placed_in_service_on: today(),
            cost_cents: 1,
            disposal_date: None,
        },
    )
    .unwrap_err();
    assert_eq!(err_correct, "that equipment was removed");
    assert_eq!(event_log_count(&conn), before_e);
    let row_after_correct: (Option<String>, String, String) = conn
        .query_row(
            "SELECT voided_at, last_event_id, updated_at FROM assets WHERE asset_id = ?1",
            [&a.asset_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row_before, row_after_correct);

    let void_eid = projection::handler_new_id();
    let void_event = EventRecord::originated(
        Kind::AssetVoided,
        "asset",
        a.asset_id.clone(),
        json!({
            "eventId": void_eid,
            "origin": "farm_os",
            "assetId": a.asset_id,
        }),
        json!({ "op": "none" }),
        projection::handler_now(),
        None,
        Some(&a.last_event_id),
        Some(void_eid),
    );
    {
        let tx = conn.transaction().unwrap();
        let err = assets::apply_asset_voided(&tx, &void_event).unwrap_err();
        assert_eq!(
            err,
            "asset.voided names an asset that is not in the register"
        );
        drop(tx);
    }

    let corr_eid = projection::handler_new_id();
    let corr_event = EventRecord::originated(
        Kind::AssetCorrected,
        "asset",
        a.asset_id.clone(),
        json!({
            "eventId": corr_eid,
            "origin": "farm_os",
            "assetId": a.asset_id,
            "description": "Nope",
            "placedInServiceOn": today(),
            "costCents": 1,
        }),
        json!({ "op": "none" }),
        projection::handler_now(),
        None,
        Some(&a.last_event_id),
        Some(corr_eid),
    );
    {
        let tx = conn.transaction().unwrap();
        let err = assets::apply_asset_corrected(&tx, &corr_event).unwrap_err();
        assert_eq!(
            err,
            "asset.corrected names an asset that is not in the register"
        );
        drop(tx);
    }
}

#[test]
fn a9_asset_void_identity_and_seal() {
    let mut conn = mem();
    let a = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Seal me".into(),
            placed_in_service_on: today(),
            cost_cents: 900,
            disposal_date: None,
        },
    )
    .unwrap();
    let prior_last = a.last_event_id.clone();
    assets::void_asset(&mut conn, &a.asset_id).unwrap();

    let (kind, domain, class, origin, undoes_seq, reverses, payload_s): (
        String,
        String,
        String,
        String,
        Option<i64>,
        Option<String>,
        String,
    ) = conn
        .query_row(
            "SELECT kind, event_domain, event_class, origin, undoes_seq,
                    reverses_event_id, payload
             FROM event_log WHERE kind = 'asset.voided' ORDER BY seq DESC LIMIT 1",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(kind, "asset.voided");
    assert_eq!(domain, "register");
    assert_eq!(class, "asset_register");
    assert_eq!(origin, "farm_os");
    assert!(undoes_seq.is_none());
    assert_eq!(reverses.as_deref(), Some(prior_last.as_str()));

    let payload: Value = serde_json::from_str(&payload_s).unwrap();
    let keys: HashSet<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    let expected: HashSet<&str> = ASSET_VOID_PAYLOAD_KEYS.iter().copied().collect();
    assert_eq!(keys, expected);

    let before_e = event_log_count(&conn);
    let before_a = assets_count(&conn);
    for key in ["depreciation", "section179", "costCents", "usefulLife"] {
        let eid = format!("a9-void-{key}");
        let mut payload = json!({
            "eventId": eid,
            "origin": "farm_os",
            "assetId": a.asset_id,
        });
        payload
            .as_object_mut()
            .unwrap()
            .insert(key.into(), json!(1));
        let event = EventRecord::originated(
            Kind::AssetVoided,
            "asset",
            a.asset_id.clone(),
            payload,
            json!({ "op": "none" }),
            projection::handler_now(),
            None,
            Some(&prior_last),
            Some(eid),
        );
        let tx = conn.transaction().unwrap();
        let err = events::insert_event(&tx, &event).unwrap_err();
        assert!(
            err.contains("computed")
                || err.contains("unknown")
                || err.contains(key)
                || err.contains("rejects"),
            "{key}: {err}"
        );
        drop(tx);
        assert_eq!(event_log_count(&conn), before_e);
        assert_eq!(assets_count(&conn), before_a);
    }

    {
        let eid = "a9-self-id";
        let event = EventRecord::originated(
            Kind::AssetVoided,
            "asset",
            eid,
            json!({
                "eventId": eid,
                "origin": "farm_os",
                "assetId": eid,
            }),
            json!({ "op": "none" }),
            projection::handler_now(),
            None,
            Some(&prior_last),
            Some(eid.into()),
        );
        let tx = conn.transaction().unwrap();
        let err = events::insert_event(&tx, &event).unwrap_err();
        assert!(err.contains("must not equal event record id"), "{err}");
        drop(tx);
        assert_eq!(event_log_count(&conn), before_e);
        assert_eq!(assets_count(&conn), before_a);
    }

    {
        let eid = "a9-commercial";
        let event = EventRecord::originated(
            Kind::AssetVoided,
            "asset",
            a.asset_id.clone(),
            json!({
                "eventId": eid,
                "origin": "commercial_app",
                "assetId": a.asset_id,
            }),
            json!({ "op": "none" }),
            projection::handler_now(),
            None,
            Some(&prior_last),
            Some(eid.into()),
        );
        let tx = conn.transaction().unwrap();
        let err = events::insert_event(&tx, &event).unwrap_err();
        assert!(
            err.contains("farm_os") || err.contains("commercial_app") || err.contains("origin"),
            "{err}"
        );
        drop(tx);
        assert_eq!(event_log_count(&conn), before_e);
        assert_eq!(assets_count(&conn), before_a);
    }
}

// ─── SPINE / REGRESSION ────────────────────────────────────────────────────

#[test]
fn s1_v12_migrates_to_v13() {
    let conn = open_v12_fixture();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 12);

    let prior_id = "prior-grow-v12";
    conn.execute(
        "INSERT INTO trays
         (id, crop_id, state, quantity, growth_days_at_sow, blackout_days_at_sow,
          sown_on, blackout_on, created_at, updated_at)
         VALUES (?1, 'dun-peas', 'blackout', 1, 9, 3, '2026-08-06', '2026-08-06',
                 '2026-08-06T12:00:00.000Z', '2026-08-06T12:00:00.000Z')",
        [prior_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO event_log
         (id, kind, entity_type, entity_id, payload, inverse, created_at,
          origin, event_domain, event_class)
         VALUES (?1, 'tray.sown', 'tray', ?1,
           '{\"cropId\":\"dun-peas\",\"quantity\":1,\"sownOn\":\"2026-08-06\",\"blackoutOn\":\"2026-08-06\"}',
           '{\"op\":\"delete_tray\",\"trayId\":\"prior-grow-v12\"}',
           '2026-08-06T12:00:00.000Z', 'farm_os', 'grow', NULL)",
        [prior_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO event_log
         (id, kind, entity_type, entity_id, payload, inverse, created_at,
          origin, event_domain, event_class)
         VALUES ('prior-cost-v12', 'cost.money_out', 'cost_event', 'prior-cost-v12',
           '{\"eventId\":\"prior-cost-v12\",\"origin\":\"farm_os\",\"datePaid\":\"2026-08-06\",\"amountCents\":500,\"payee\":\"Seed Co\",\"canonicalCategory\":\"growing_medium\",\"scheduleFLine\":\"f\",\"scheduleCLine\":\"c\",\"descriptor\":\"\",\"quantity\":null,\"unitPriceCents\":null,\"deliveryDate\":null,\"invoiceReference\":null,\"receiptFileRef\":null,\"createdAt\":\"2026-08-06T12:00:00.000Z\",\"updatedAt\":\"2026-08-06T12:00:00.000Z\"}',
           '{\"op\":\"none\"}',
           '2026-08-06T12:00:00.000Z', 'farm_os', 'register', 'money_out')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO cost_events
         (event_id, origin, date_paid, amount_cents, payee, canonical_category,
          schedule_f_line, schedule_c_line, descriptor, created_at, updated_at)
         VALUES ('prior-cost-v12', 'farm_os', '2026-08-06', 500, 'Seed Co',
                 'growing_medium', 'f', 'c', '', '2026-08-06T12:00:00.000Z',
                 '2026-08-06T12:00:00.000Z')",
        [],
    )
    .unwrap();

    // v12 triggers must reject the new kinds before migration.
    let reject = conn.execute(
        "INSERT INTO event_log
         (id, kind, entity_type, entity_id, payload, inverse, created_at,
          origin, event_domain, event_class)
         VALUES ('pre-mig-miles', 'mileage.trip', 'mileage_trip', 'x',
           '{}', '{\"op\":\"none\"}', '2026-08-06T12:00:00.000Z',
           'farm_os', 'register', 'mileage')",
        [],
    );
    assert!(reject.is_err(), "v12 triggers must reject mileage.trip");

    let events_before: Vec<(i64, String, String)> = {
        let mut stmt = conn
            .prepare("SELECT seq, id, kind FROM event_log ORDER BY seq")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|x| x.unwrap())
            .collect()
    };

    db::migrate(&conn).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, db::SCHEMA_VERSION);
    assert!(table_exists(&conn, "mileage_trips"));
    assert!(table_exists(&conn, "assets"));

    let trigger_sql = master_sql(&conn, "event_log_before_insert");
    for kind in [
        "mileage.trip",
        "mileage.trip_corrected",
        "mileage.trip_voided",
        "asset.recorded",
        "asset.corrected",
    ] {
        assert!(trigger_sql.contains(kind), "v13 trigger missing {kind}");
    }

    let events_after: Vec<(i64, String, String)> = {
        let mut stmt = conn
            .prepare("SELECT seq, id, kind FROM event_log ORDER BY seq")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|x| x.unwrap())
            .collect()
    };
    assert_eq!(events_after, events_before);

    let mut conn = conn;
    mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 3.0,
            purpose: None,
        },
    )
    .unwrap();
    assert_eq!(mileage_count(&conn), 1);
}

#[test]
fn s2_fresh_and_migrated_v13_schemas_converge() {
    let fresh = mem();
    let migrated = open_v12_fixture();
    db::migrate(&migrated).unwrap();

    for name in [
        "mileage_trips",
        "assets",
        "mileage_trips_before_insert",
        "mileage_trips_before_update",
        "assets_before_insert",
        "assets_before_update",
    ] {
        let a = norm_sql(&master_sql(&fresh, name));
        let b = norm_sql(&master_sql(&migrated, name));
        assert_eq!(a, b, "sqlite_master sql diverged for {name}");
    }

    // Third path: early v13 shape (nine-column assets, user_version already 13).
    // Section 2.2 must add voided_at. ALTER TABLE ADD COLUMN rewrites the stored
    // DDL string with different comma and newline placement, so the DDL text is
    // not the right equality for a repaired table; the column tuple list is.
    let early = open_v12_fixture();
    db::migrate(&early).unwrap();
    early
        .execute_batch(
            r#"
DROP TABLE assets;
CREATE TABLE assets (
  asset_id              TEXT PRIMARY KEY,
  origin                TEXT NOT NULL CHECK (origin = 'farm_os'),
  description           TEXT NOT NULL CHECK (length(trim(description)) > 0),
  placed_in_service_on  TEXT NOT NULL,
  cost_cents            INTEGER NOT NULL CHECK (cost_cents > 0),
  disposal_date         TEXT,
  last_event_id         TEXT NOT NULL,
  created_at            TEXT NOT NULL,
  updated_at            TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS assets_before_insert
BEFORE INSERT ON assets
BEGIN
  SELECT CASE
    WHEN NEW.cost_cents IS NULL OR NEW.cost_cents <= 0
      THEN RAISE(ABORT, 'assets.cost_cents must be positive')
    WHEN NEW.description IS NULL OR trim(NEW.description) = ''
      THEN RAISE(ABORT, 'assets.description required')
    WHEN NEW.placed_in_service_on > date('now', 'localtime')
      THEN RAISE(ABORT, 'assets.placed_in_service_on cannot be future')
    WHEN NEW.disposal_date IS NOT NULL
         AND NEW.disposal_date < NEW.placed_in_service_on
      THEN RAISE(ABORT, 'assets.disposal_date before placed_in_service_on')
  END;
END;
CREATE TRIGGER IF NOT EXISTS assets_before_update
BEFORE UPDATE ON assets
BEGIN
  SELECT CASE
    WHEN NEW.asset_id IS NOT OLD.asset_id
      THEN RAISE(ABORT, 'assets.asset_id immutable')
    WHEN NEW.origin IS NOT OLD.origin
      THEN RAISE(ABORT, 'assets.origin immutable')
    WHEN NEW.created_at IS NOT OLD.created_at
      THEN RAISE(ABORT, 'assets.created_at immutable')
    WHEN NEW.cost_cents IS NULL OR NEW.cost_cents <= 0
      THEN RAISE(ABORT, 'assets.cost_cents must be positive')
    WHEN NEW.disposal_date IS NOT NULL
         AND NEW.disposal_date < NEW.placed_in_service_on
      THEN RAISE(ABORT, 'assets.disposal_date before placed_in_service_on')
  END;
END;
"#,
        )
        .unwrap();
    early.pragma_update(None, "user_version", 13).unwrap();
    assert!(!table_columns(&early, "assets")
        .iter()
        .any(|c| c == "voided_at"));

    db::migrate(&early).unwrap();
    assert!(table_columns(&early, "assets")
        .iter()
        .any(|c| c == "voided_at"));

    let fresh_info = table_info_tuples(&fresh, "assets");
    let repaired_info = table_info_tuples(&early, "assets");
    assert_eq!(
        fresh_info, repaired_info,
        "repaired early-v13 assets column tuples must match fresh"
    );
}

#[test]
fn s3_spine_round_trip_verify_replay_covers_new_tables() {
    let dir = tempfile_dir("s3-spine");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();

    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    costs::record_cost(&mut conn, &dir, basic_cost(500)).unwrap();

    let t1 = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 10.0,
            purpose: Some("a".into()),
        },
    )
    .unwrap();
    let t2 = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: yesterday(),
            miles: 4.0,
            purpose: None,
        },
    )
    .unwrap();
    mileage::correct_trip(
        &mut conn,
        CorrectMileageTripInput {
            trip_id: t1.trip_id.clone(),
            trip_date: today(),
            miles: 11.0,
            purpose: Some("b".into()),
        },
    )
    .unwrap();
    mileage::void_trip(&mut conn, &t2.trip_id).unwrap();

    let asset = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Pump".into(),
            placed_in_service_on: yesterday(),
            cost_cents: 40_000,
            disposal_date: None,
        },
    )
    .unwrap();
    assets::correct_asset(
        &mut conn,
        CorrectAssetInput {
            asset_id: asset.asset_id.clone(),
            description: asset.description.clone(),
            placed_in_service_on: asset.placed_in_service_on.clone(),
            cost_cents: asset.cost_cents,
            disposal_date: Some(today()),
        },
    )
    .unwrap();

    let asset2 = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Mis-keyed".into(),
            placed_in_service_on: today(),
            cost_cents: 100,
            disposal_date: None,
        },
    )
    .unwrap();
    assets::void_asset(&mut conn, &asset2.asset_id).unwrap();

    crate::event_file::try_flush_after_commit(&conn, &dir);
    let snaps = dir.join("snapshots");
    fs::create_dir_all(&snaps).unwrap();
    crate::snapshots::take_snapshot(&mut conn, &snaps).unwrap();
    crate::event_file::try_flush_after_commit(&conn, &dir);
    drop(conn);

    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify failed: {}",
        outcome.summary_line()
    );
    let report = outcome.report();
    assert_eq!(report.flush_lag, 0);
    assert_eq!(report.tables_compared, 26);

    // Live DB still holds both asset rows; verify-replay compared voided_at via
    // ASSETS_COLUMNS, so a clean pass reproduces the voided row on rebuild.
    let live = Connection::open(&farm).unwrap();
    assert_eq!(assets_count(&live), 2);
    let voided: Option<String> = live
        .query_row(
            "SELECT voided_at FROM assets WHERE asset_id = ?1",
            [&asset2.asset_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(voided.is_some());
    let kept_voided: Option<String> = live
        .query_row(
            "SELECT voided_at FROM assets WHERE asset_id = ?1",
            [&asset.asset_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(kept_voided.is_none());
    drop(live);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn s4_undo_last_never_selects_mileage_or_asset() {
    let mut conn = mem();
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let trip = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date: today(),
            miles: 6.0,
            purpose: None,
        },
    )
    .unwrap();
    let asset = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description: "Shelf".into(),
            placed_in_service_on: today(),
            cost_cents: 1000,
            disposal_date: None,
        },
    )
    .unwrap();

    let undoable = events::newest_undoable(&conn).unwrap().expect("sow");
    assert_eq!(undoable.kind, "tray.sown");

    let miles_before = mileage_count(&conn);
    let assets_before = assets_count(&conn);
    let trip_row_before: String = conn
        .query_row(
            "SELECT last_event_id FROM mileage_trips WHERE trip_id = ?1",
            [&trip.trip_id],
            |r| r.get(0),
        )
        .unwrap();
    let asset_row_before: String = conn
        .query_row(
            "SELECT last_event_id FROM assets WHERE asset_id = ?1",
            [&asset.asset_id],
            |r| r.get(0),
        )
        .unwrap();

    trays::undo_last(&mut conn).unwrap();

    assert_eq!(mileage_count(&conn), miles_before);
    assert_eq!(assets_count(&conn), assets_before);
    let trip_row_after: String = conn
        .query_row(
            "SELECT last_event_id FROM mileage_trips WHERE trip_id = ?1",
            [&trip.trip_id],
            |r| r.get(0),
        )
        .unwrap();
    let asset_row_after: String = conn
        .query_row(
            "SELECT last_event_id FROM assets WHERE asset_id = ?1",
            [&asset.asset_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(trip_row_before, trip_row_after);
    assert_eq!(asset_row_before, asset_row_after);
}

#[test]
fn s5_existing_track4_core_path_unchanged() {
    let grow = grow_kinds();
    assert_eq!(grow.len(), 11);
    assert_eq!(grow, GROW_KINDS.to_vec());

    let register = register_kinds();
    // Bumped for the silent-shortfall residual (wholesale.write_off), then
    // again for Shape B (stripe.fact_unapplied), then for bounce
    // (wholesale.payment_reversed), then for bad debt (wholesale.bad_debt),
    // then for TILL-A (wholesale.link_minted), then for LO-A (leftover.listed),
    // then for LO-B (leftover.link_minted, leftover.paid), then for SEED-A
    // (seed.received).
    assert_eq!(register.len(), 30);
    assert_eq!(register, REGISTER_KINDS.to_vec());

    let conn = mem();
    // consumption_events / cost_events shapes unchanged.
    let cons = table_columns(&conn, "consumption_events");
    for col in [
        "event_id",
        "origin",
        "occurred_at",
        "variety_or_item",
        "unit",
        "quantity",
        "sow_event_id",
        "linked_cost_event_id",
        "notes",
    ] {
        assert!(cons.iter().any(|c| c == col), "missing {col}");
    }
    let cost = table_columns(&conn, "cost_events");
    for col in [
        "event_id",
        "origin",
        "date_paid",
        "amount_cents",
        "payee",
        "canonical_category",
    ] {
        assert!(cost.iter().any(|c| c == col), "missing {col}");
    }
    let _ = MILEAGE_TRIPS_COLUMNS;
    let _ = MILEAGE_FORBIDDEN_KEYS;
    let _ = AssetPayload {
        event_id: String::new(),
        origin: String::new(),
        asset_id: String::new(),
        description: String::new(),
        placed_in_service_on: String::new(),
        cost_cents: 1,
        disposal_date: None,
    };
}

/// H-10 - ASSET_SPINE_COLUMNS names real columns of `assets`, and every one
/// is listed in ASSETS_COLUMNS. This is not a formality: `assets.voided_at`
/// exists only because of the v13 shape-convergence ALTER (db.rs:1520), which
/// is guarded by a runtime column check. If that path regressed, the const
/// would be a lie and nothing else in the suite would see it. PRAGMA is the
/// T6b idiom. The order mirrors the table's own declaration order.
#[test]
fn h10_asset_spine_columns_are_real_and_listed() {
    assert_eq!(
        ASSET_SPINE_COLUMNS,
        &["last_event_id", "created_at", "updated_at", "voided_at"]
    );
    let conn = mem();
    let mut stmt = conn.prepare("PRAGMA table_info(assets)").unwrap();
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|c| c.unwrap())
        .collect();
    for spine in ASSET_SPINE_COLUMNS {
        assert!(
            cols.contains(&(*spine).to_string()),
            "assets has no column {spine}"
        );
        assert!(
            ASSETS_COLUMNS.contains(spine),
            "{spine} is missing from ASSETS_COLUMNS"
        );
    }
}

/// H-10 - ASSET_PAYLOAD_FIELD_NAMES is the snake_case twin of
/// ASSET_PAYLOAD_KEYS and matches AssetPayload's own shape, proved by a
/// deserialisation the struct's `deny_unknown_fields` would refuse if the
/// names drifted. The m3 idiom.
///
/// Deliberately no forbidden-money-key loop. An asset carries costCents
/// lawfully - it is the register - and banning it here would assert a claim
/// this domain does not make.
#[test]
fn h10_asset_payload_field_names_match_the_struct() {
    assert_eq!(
        ASSET_PAYLOAD_FIELD_NAMES,
        &[
            "event_id",
            "origin",
            "asset_id",
            "description",
            "placed_in_service_on",
            "cost_cents",
            "disposal_date",
        ]
    );
    assert_eq!(
        ASSET_PAYLOAD_KEYS.len(),
        ASSET_PAYLOAD_FIELD_NAMES.len(),
        "the camelCase and snake_case lists must stay the same length"
    );
    let _: AssetPayload = serde_json::from_value(json!({
        "eventId": "e",
        "origin": "farm_os",
        "assetId": "a",
        "description": "Shelf rack",
        "placedInServiceOn": "2026-08-11",
        "costCents": 12345,
    }))
    .unwrap();
}

/// H-10 - MILEAGE_SPINE_COLUMNS names real columns of `mileage_trips`. It
/// leads with `voided_at` because the table declares it before
/// `last_event_id` (db.rs:150-151); the const mirrors the table rather than
/// the other three spines, and this records that on purpose.
#[test]
fn h10_mileage_spine_columns_are_real_and_listed() {
    assert_eq!(
        MILEAGE_SPINE_COLUMNS,
        &["voided_at", "last_event_id", "created_at", "updated_at"]
    );
    let conn = mem();
    let mut stmt = conn.prepare("PRAGMA table_info(mileage_trips)").unwrap();
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|c| c.unwrap())
        .collect();
    for spine in MILEAGE_SPINE_COLUMNS {
        assert!(
            cols.contains(&(*spine).to_string()),
            "mileage_trips has no column {spine}"
        );
        assert!(
            MILEAGE_TRIPS_COLUMNS.contains(spine),
            "{spine} is missing from MILEAGE_TRIPS_COLUMNS"
        );
    }
}

/// H-10 - MILEAGE_TRIP_PAYLOAD_FIELD_NAMES matches MileageTripPayload and
/// carries no money key. Mileage is the one domain of the three where the
/// forbidden-key loop belongs: a trip is miles, and MILEAGE_FORBIDDEN_KEYS
/// exists to keep a rate or an amount out of it. Full g4 + m3 idiom.
#[test]
fn h10_mileage_trip_payload_field_names_match_the_struct() {
    for name in MILEAGE_TRIP_PAYLOAD_FIELD_NAMES {
        for forbidden in MILEAGE_FORBIDDEN_KEYS {
            assert!(
                !name.eq_ignore_ascii_case(forbidden),
                "mileage payload field {name} is a forbidden money key"
            );
        }
    }
    assert_eq!(
        MILEAGE_TRIP_PAYLOAD_FIELD_NAMES,
        &[
            "event_id",
            "origin",
            "trip_id",
            "trip_date",
            "miles",
            "purpose"
        ]
    );
    assert_eq!(
        MILEAGE_TRIP_PAYLOAD_KEYS.len(),
        MILEAGE_TRIP_PAYLOAD_FIELD_NAMES.len(),
        "the camelCase and snake_case lists must stay the same length"
    );
    let _: MileageTripPayload = serde_json::from_value(json!({
        "eventId": "e",
        "origin": "farm_os",
        "tripId": "t",
        "tripDate": "2026-08-11",
        "miles": 12.5,
    }))
    .unwrap();
}
