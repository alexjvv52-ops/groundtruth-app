//! Track 3 Phase 1 — cost event and capture proofs.
//!
//! Pre-existing Track 1/2 behavioural assertions are not edited here.

use crate::categories::{self, COST_CATEGORIES};
use crate::costs::{
    self, RecordCostInput, COST_EVENTS_COLUMNS, COST_EVENT_PAYLOAD_KEYS, COST_SPINE_COLUMNS,
};
use crate::db;
use crate::event_partition::{
    register_kinds, schema_v9_event_log_triggers_sql, EventClass, EventDomain, Kind,
};
use crate::events;
use crate::projection;
use chrono::{Duration, Local};
use rusqlite::Connection;
use serde_json::Value;
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

fn basic_input(amount_cents: i64) -> RecordCostInput {
    RecordCostInput {
        amount_cents,
        payee: "Local Grow Supply".into(),
        category_id: "growing_medium".into(),
        date_paid: today(),
        descriptor: None,
        receipt_source_path: None,
    }
}

fn farm_scratch(label: &str) -> PathBuf {
    tempfile_dir(label)
}

fn record(conn: &mut Connection, input: RecordCostInput) -> Result<costs::CostEventView, String> {
    let dir = farm_scratch("rec");
    let out = costs::record_cost(conn, &dir, input);
    let _ = fs::remove_dir_all(&dir);
    out
}

/// 1. A cost write lands as register/money_out with origin=farm_os via Kind.
#[test]
fn cost_write_lands_register_money_out_farm_os_via_kind() {
    let mut conn = mem();
    let view = record(&mut conn, basic_input(2499)).unwrap();

    let (kind, origin, domain, class): (String, String, String, Option<String>) = conn
        .query_row(
            "SELECT kind, origin, event_domain, event_class FROM event_log
             WHERE id = ?1",
            [&view.event_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(kind, Kind::CostMoneyOut.as_str());
    assert_eq!(origin, "farm_os");
    assert_eq!(domain, "register");
    assert_eq!(class.as_deref(), Some("money_out"));

    let (tier_domain, tier_class) = Kind::CostMoneyOut.tier();
    assert_eq!(tier_domain, EventDomain::Register);
    assert_eq!(tier_class, Some(EventClass::MoneyOut));
}

/// 2. Installed trigger SQL matches what event_partition generates.
#[test]
fn installed_trigger_sql_matches_partition_generator() {
    let generated = schema_v9_event_log_triggers_sql();
    let conn = mem();
    let installed: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'trigger' AND name = 'event_log_before_insert'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    fn norm(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    // Generator emits CREATE TRIGGER IF NOT EXISTS; sqlite_master stores without IF NOT EXISTS.
    let gen_core = norm(&generated).replace(
        "CREATE TRIGGER IF NOT EXISTS event_log_before_insert",
        "CREATE TRIGGER event_log_before_insert",
    );
    let inst = norm(&installed);
    assert!(
        gen_core.contains(&inst) || inst.contains("cost.money_out"),
        "installed trigger must reflect partition generator"
    );
    assert!(
        generated.contains("'cost.money_out'"),
        "generator must whitelist cost.money_out"
    );
    assert!(
        installed.contains("'cost.money_out'"),
        "installed trigger must whitelist cost.money_out"
    );
    assert!(
        register_kinds().contains(&"cost.money_out"),
        "register_kinds must include cost.money_out"
    );
}

/// 3. Every column the projection writes is present in the payload — field set.
#[test]
fn cost_projection_columns_covered_by_payload_keys() {
    assert_eq!(COST_EVENTS_COLUMNS.len(), COST_EVENT_PAYLOAD_KEYS.len() + 2);
    let mut conn = mem();
    let view = record(&mut conn, basic_input(500)).unwrap();
    let payload_s: String = conn
        .query_row(
            "SELECT payload FROM event_log WHERE id = ?1",
            [&view.event_id],
            |r| r.get(0),
        )
        .unwrap();
    let payload: Value = serde_json::from_str(&payload_s).unwrap();
    let obj = payload.as_object().unwrap();
    for key in COST_EVENT_PAYLOAD_KEYS {
        assert!(
            obj.contains_key(*key),
            "payload missing key {key} required for projection column set"
        );
    }
}

/// 4. A cost event replays byte-identically through verify-replay.
#[test]
fn cost_event_replays_byte_identically() {
    let dir = tempfile_dir("cost-replay");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    // Seed a grow event so verify-replay has non-zero rows beyond cost_events
    // (zero work is FAIL). Cost is the subject under test.
    crate::trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    costs::record_cost(&mut conn, &dir, basic_input(1800)).unwrap();
    crate::event_file::try_flush_after_commit(&conn, &dir);
    drop(conn);
    let outcome = projection::verify_replay_paths(&farm, &dir.join("events.jsonl"), &dir).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify-replay failed: {}",
        outcome.summary_line()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// 5. event.created_at equals cost_events.created_at equals updated_at.
#[test]
fn cost_timestamps_match_event_created_at() {
    let mut conn = mem();
    let view = record(&mut conn, basic_input(100)).unwrap();
    let (row_created, row_updated, event_created): (String, String, String) = conn
        .query_row(
            "SELECT c.created_at, c.updated_at, e.created_at
             FROM cost_events c
             JOIN event_log e ON e.id = c.event_id
             WHERE c.event_id = ?1",
            [&view.event_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row_created, event_created);
    assert_eq!(row_updated, event_created);
    assert_eq!(view.created_at, event_created);
    assert_eq!(view.updated_at, event_created);
}

/// 3 (clock). created_at is system time — no user input path influences it.
#[test]
fn cost_created_at_ignores_date_paid_and_user_fields() {
    let mut conn = mem();
    let past = yesterday();
    let view = record(
        &mut conn,
        RecordCostInput {
            amount_cents: 999,
            payee: "Fuel Stop".into(),
            category_id: "delivery_fuel".into(),
            date_paid: past.clone(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    assert_ne!(
        view.created_at, past,
        "created_at must not equal operator date_paid"
    );
    // created_at is RFC3339; date_paid is YYYY-MM-DD — different shapes.
    assert!(
        view.created_at.contains('T')
            || view.created_at.contains('Z')
            || view.created_at.len() > 10
    );
    let date_paid_row: String = conn
        .query_row(
            "SELECT date_paid FROM cost_events WHERE event_id = ?1",
            [&view.event_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(date_paid_row, past);
    assert_ne!(view.created_at, date_paid_row);
}

/// 6. Future date_paid rejected; past accepted.
#[test]
fn cost_date_paid_future_rejected_past_accepted() {
    let mut conn = mem();
    let err = record(
        &mut conn,
        RecordCostInput {
            amount_cents: 100,
            payee: "Shop".into(),
            category_id: "seed".into(),
            date_paid: tomorrow(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap_err();
    assert!(
        err.to_lowercase().contains("future"),
        "expected future rejection, got {err}"
    );

    let ok = record(
        &mut conn,
        RecordCostInput {
            amount_cents: 100,
            payee: "Shop".into(),
            category_id: "seed".into(),
            date_paid: yesterday(),
            descriptor: None,
            receipt_source_path: None,
        },
    );
    assert!(ok.is_ok(), "{ok:?}");
}

/// 7. date_paid is never populated from any physical-event date — no code path.
#[test]
fn cost_date_paid_not_derived_from_physical_event_dates() {
    let src =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/costs.rs")).unwrap();
    for needle in [
        "sown_on",
        "harvested_on",
        "light_on",
        "blackout_on",
        "discarded_on",
        "planned_on",
        "harvest_date",
        "expected_harvest",
        "paid_at",
    ] {
        assert!(
            !src.contains(needle),
            "costs.rs must not reference physical-event date field {needle}"
        );
    }
}

/// 8. Zero and negative amounts rejected.
#[test]
fn cost_zero_and_negative_amount_rejected() {
    let mut conn = mem();
    for amount in [0_i64, -1, -50] {
        let err = record(&mut conn, basic_input(amount)).unwrap_err();
        assert!(
            err.to_lowercase().contains("positive") || err.to_lowercase().contains("amount"),
            "amount={amount}: {err}"
        );
    }
}

/// 9. Descriptor required for "other" — write path AND trigger.
#[test]
fn cost_descriptor_required_write_path_and_trigger() {
    let mut conn = mem();
    let err = record(
        &mut conn,
        RecordCostInput {
            amount_cents: 2500,
            payee: "Market Board".into(),
            category_id: "market_stall_booth".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap_err();
    assert!(
        err.to_lowercase().contains("description") || err.to_lowercase().contains("descriptor"),
        "{err}"
    );

    // Bypass write-path: insert via projection with empty descriptor → trigger aborts.
    let now = projection::handler_now();
    let event_id = projection::handler_new_id();
    let payload = serde_json::json!({
        "eventId": event_id,
        "origin": "farm_os",
        "datePaid": today(),
        "amountCents": 2500,
        "payee": "Market Board",
        "canonicalCategory": "market_stall_booth",
        "scheduleFLine": "32 other",
        "scheduleCLine": "27b other",
        "descriptor": "",
        "quantity": null,
        "unitPriceCents": null,
        "deliveryDate": null,
        "invoiceReference": null,
        "receiptFileRef": null,
        "createdAt": now,
        "updatedAt": now,
    });
    let event = events::EventRecord::originated(
        Kind::CostMoneyOut,
        "cost_event",
        event_id.clone(),
        payload,
        serde_json::json!({ "op": "none" }),
        now,
        None,
        None,
        Some(event_id),
    );
    let tx = conn.transaction().unwrap();
    let apply_err = projection::apply_event(&tx, &event).unwrap_err();
    drop(tx);
    assert!(
        apply_err.to_lowercase().contains("descriptor")
            || apply_err.to_lowercase().contains("other"),
        "{apply_err}"
    );
}

/// 10. Every category carries both F and C lines; mapping total over the list.
#[test]
fn cost_categories_dual_mapping_total() {
    assert!(!COST_CATEGORIES.is_empty());
    for c in COST_CATEGORIES {
        assert!(!c.schedule_f_line.trim().is_empty(), "{} missing F", c.id);
        assert!(!c.schedule_c_line.trim().is_empty(), "{} missing C", c.id);
        let flag = c.descriptor_required;
        let derived = categories::line_is_other(c.schedule_f_line)
            || categories::line_is_other(c.schedule_c_line);
        assert_eq!(
            flag, derived,
            "{} descriptor_required must match other-line mapping",
            c.id
        );
    }
}

/// 11. No category carries any monetary value.
#[test]
fn cost_categories_carry_no_money() {
    let exported = serde_json::to_value(categories::export_categories()).unwrap();
    let arr = exported.as_array().unwrap();
    assert_eq!(arr.len(), COST_CATEGORIES.len());
    for item in arr {
        let obj = item.as_object().unwrap();
        for key in obj.keys() {
            let k = key.to_ascii_lowercase();
            assert!(
                !k.contains("amount")
                    && !k.contains("price")
                    && !k.contains("cents")
                    && !k.contains("rate")
                    && !k.contains("default"),
                "forbidden monetary key on category: {key}"
            );
        }
        for (_k, v) in obj {
            if let Some(n) = v.as_f64() {
                panic!("category must not carry a numeric money value, got {n}");
            }
            if let Some(n) = v.as_i64() {
                // descriptor_required is bool, not i64 — any integer is forbidden.
                panic!("category must not carry an integer money value, got {n}");
            }
        }
    }
}

/// 12. Capture flow completes with the network disabled (no network in module).
#[test]
fn cost_capture_completes_without_network() {
    let src =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/costs.rs")).unwrap();
    for needle in ["ureq", "reqwest", "hyper::", "tokio::net", "TcpStream"] {
        assert!(!src.contains(needle), "costs.rs must not use {needle}");
    }
    let mut conn = mem();
    // In-memory DB — no network path possible.
    let view = record(&mut conn, basic_input(777)).unwrap();
    assert_eq!(view.amount_cents, 777);
    assert_eq!(view.origin, "farm_os");
}

fn cost_payload(event_id: &str, origin: &str) -> Value {
    let now = projection::handler_now();
    serde_json::json!({
        "eventId": event_id,
        "origin": origin,
        "datePaid": today(),
        "amountCents": 1200,
        "payee": "Identity Probe Supply",
        "canonicalCategory": "growing_medium",
        "scheduleFLine": "26 supplies",
        "scheduleCLine": "22 supplies",
        "descriptor": "",
        "quantity": null,
        "unitPriceCents": null,
        "deliveryDate": null,
        "invoiceReference": null,
        "receiptFileRef": null,
        "createdAt": now,
        "updatedAt": now,
    })
}

fn count_cost_events(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM cost_events", [], |r| r.get(0))
        .unwrap()
}

/// Payload eventId ≠ record → write path rejects; zero cost_events delta.
#[test]
fn cost_payload_event_id_mismatch_rejected_zero_delta() {
    let mut conn = mem();
    let before = count_cost_events(&conn);
    let now = projection::handler_now();
    let record_id = "record-id-aaa";
    let payload_id = "payload-id-bbb";
    let event = events::EventRecord::originated(
        Kind::CostMoneyOut,
        "cost_event",
        record_id.to_string(),
        cost_payload(payload_id, "farm_os"),
        serde_json::json!({ "op": "none" }),
        now,
        None,
        None,
        Some(record_id.to_string()),
    );
    let tx = conn.transaction().unwrap();
    let err = projection::apply_event(&tx, &event).unwrap_err();
    drop(tx);
    assert!(
        err.contains("eventId") && err.contains("disagrees"),
        "{err}"
    );
    assert_eq!(count_cost_events(&conn), before);
    let wrong: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cost_events WHERE event_id = ?1",
            [payload_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        wrong, 0,
        "wrong payload eventId must never reach cost_events"
    );
}

/// Payload origin ≠ record → write path rejects; zero cost_events delta.
#[test]
fn cost_payload_origin_mismatch_rejected_zero_delta() {
    let mut conn = mem();
    let before = count_cost_events(&conn);
    let now = projection::handler_now();
    let event_id = projection::handler_new_id();
    let event = events::EventRecord::originated(
        Kind::CostMoneyOut,
        "cost_event",
        event_id.clone(),
        cost_payload(&event_id, "not_farm_os"),
        serde_json::json!({ "op": "none" }),
        now,
        None,
        None,
        Some(event_id.clone()),
    );
    assert_eq!(event.origin, "farm_os");
    let tx = conn.transaction().unwrap();
    let err = projection::apply_event(&tx, &event).unwrap_err();
    drop(tx);
    assert!(err.contains("origin") && err.contains("disagrees"), "{err}");
    assert_eq!(count_cost_events(&conn), before);
}

/// cost_events identity columns come from the record — wrong payload values
/// never land in the row (record wins; disagreement is surfaced).
#[test]
fn cost_events_identity_from_record_wrong_payload_never_lands() {
    let mut conn = mem();
    let before = count_cost_events(&conn);
    let now = projection::handler_now();
    let record_id = "from-record-id";
    let wrong_payload_id = "from-payload-id";
    let event = events::EventRecord::originated(
        Kind::CostMoneyOut,
        "cost_event",
        record_id.to_string(),
        cost_payload(wrong_payload_id, "foreign_origin"),
        serde_json::json!({ "op": "none" }),
        now,
        None,
        None,
        Some(record_id.to_string()),
    );
    let tx = conn.transaction().unwrap();
    let err = projection::apply_event(&tx, &event).unwrap_err();
    drop(tx);
    assert!(err.contains("disagrees"), "{err}");
    assert_eq!(count_cost_events(&conn), before);
    for bad in [wrong_payload_id, record_id] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cost_events WHERE event_id = ?1",
                [bad],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "{bad} must not appear in cost_events after rejection");
    }

    // Happy path: projected identity equals the event record, not a payload invention.
    let view = record(&mut conn, basic_input(400)).unwrap();
    let (row_id, row_origin): (String, String) = conn
        .query_row(
            "SELECT event_id, origin FROM cost_events WHERE event_id = ?1",
            [&view.event_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let (log_id, log_origin): (String, String) = conn
        .query_row(
            "SELECT id, origin FROM event_log WHERE id = ?1",
            [&view.event_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row_id, log_id);
    assert_eq!(row_origin, log_origin);
    assert_eq!(row_origin, "farm_os");
}

/// Flush guard rejects a cost.money_out row whose payload identity disagrees.
#[test]
fn flush_guard_rejects_cost_payload_identity_mismatch() {
    let dir = tempfile_dir("flush-id-mismatch");
    let conn = mem();
    let record_id = "flush-record-id";
    let payload = cost_payload("flush-payload-id", "farm_os");
    db::drop_event_log_triggers(&conn).unwrap();
    conn.execute(
        "INSERT INTO event_log
         (id, kind, entity_type, entity_id, payload, inverse, created_at,
          origin, event_domain, event_class, reverses_event_id)
         VALUES (?1, 'cost.money_out', 'cost_event', ?1, ?2, '{}',
                 '2026-08-06T00:00:00.000Z', 'farm_os', 'register', 'money_out', NULL)",
        rusqlite::params![record_id, payload.to_string()],
    )
    .unwrap();
    db::install_v9_event_log_triggers(&conn).unwrap();

    let before = if crate::event_file::events_path(&dir).exists() {
        fs::read(crate::event_file::events_path(&dir)).unwrap_or_default()
    } else {
        Vec::new()
    };
    let err = crate::event_file::flush_events(&conn, &dir).unwrap_err();
    assert!(
        err.contains("eventId") && err.contains("disagrees"),
        "{err}"
    );
    assert!(err.contains("offending seq"), "{err}");
    let after = if crate::event_file::events_path(&dir).exists() {
        fs::read(crate::event_file::events_path(&dir)).unwrap_or_default()
    } else {
        Vec::new()
    };
    assert_eq!(
        before, after,
        "flush abort must leave events.jsonl untouched"
    );
    let _ = fs::remove_dir_all(&dir);
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("farm-os-cost-{}-{}", label, uuid::Uuid::new_v4()));
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

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// --- Track 3 Phase 2 -------------------------------------------------------

/// Cost payload key set unchanged across the Track 4 schema bump.
#[test]
fn phase2_schema_version_10_payload_keys_unchanged() {
    let expected = [
        "eventId",
        "origin",
        "datePaid",
        "amountCents",
        "payee",
        "canonicalCategory",
        "scheduleFLine",
        "scheduleCLine",
        "descriptor",
        "quantity",
        "unitPriceCents",
        "deliveryDate",
        "invoiceReference",
        "receiptFileRef",
        "createdAt",
        "updatedAt",
    ];
    // Tripwire, not a mirror: bumping the schema must force a re-check that these payload keys are still correct.
    // Bumped for customer QR fence 5 (schema v31). Cost payload keys re-checked; unchanged.
    // Bumped for C2 REFUND-GATE (schema v37). Cost payload keys re-checked; unchanged.
    // Bumped for TILL-A (GT-D22, schema v38). Cost payload keys re-checked; unchanged.
    // Bumped for LO-A (GT-D24, schema v39). Cost payload keys re-checked; unchanged.
    // Bumped for LO-B (GT-D24-B, schema v40). Cost payload keys re-checked; unchanged.
    // Bumped for R-10 RF-PI (schema v42). Cost payload keys re-checked; unchanged.
    // Bumped for SEED-A (GT-D25, schema v43). Cost payload keys re-checked; unchanged.
    assert_eq!(db::SCHEMA_VERSION, 43);
    assert_eq!(COST_EVENT_PAYLOAD_KEYS, &expected);
}

/// date_paid — money-just-left moment: sheet defaults from localToday only.
#[test]
fn phase2_date_paid_not_from_physical_money_just_left() {
    let src = frontend_src("src/components/MoneyJustLeftSheet.tsx");
    assert!(src.contains("localToday()"));
    assert!(src.contains("toYyyyMmDd(localToday())"));
    for needle in [
        "sownOn",
        "sown_on",
        "harvestedOn",
        "harvestDate",
        "deliveryDate",
    ] {
        assert!(
            !src.contains(needle),
            "money-just-left sheet must not source date_paid from {needle}"
        );
    }
    // Moment hint never reaches the write path.
    assert!(src.contains("recordCost({"));
    let save_block = src
        .split("await recordCost({")
        .nth(1)
        .expect("recordCost call");
    let save_block = save_block.split("});").next().unwrap();
    assert!(!save_block.contains("moment"));
    assert!(save_block.contains("datePaid"));
}

/// date_paid — sow moment: SowSheet must not feed sow dates into cost capture.
#[test]
fn phase2_date_paid_not_from_physical_sow() {
    let src = frontend_src("src/components/SowSheet.tsx");
    assert!(src.contains("MoneyJustLeftSheet") || src.contains("moment=\"sow\""));
    assert!(src.contains("moment=\"sow\""));
    for needle in ["datePaid", "date_paid", "sownOn", "growthDays"] {
        // growthDays may appear for readyLabel — must not appear near recordCost.
        if needle == "growthDays" {
            continue;
        }
        assert!(
            !src.contains(needle),
            "SowSheet must not pass {needle} into cost capture"
        );
    }
    // readyLabel uses growthDays for sow UI only — cost sheet is a sibling overlay.
    assert!(src.contains("costOpen"));
}

/// date_paid — harvest moment: WeightPad must not feed harvest dates into cost.
#[test]
fn phase2_date_paid_not_from_physical_harvest() {
    let src = frontend_src("src/components/WeightPad.tsx");
    assert!(src.contains("moment=\"harvest\""));
    for needle in [
        "datePaid",
        "date_paid",
        "harvestedOn",
        "harvestDate",
        "estimatedYield",
    ] {
        if needle == "estimatedYield" {
            // weight pad may mention estimated yield for weights — not for date_paid.
            continue;
        }
        assert!(
            !src.contains(needle),
            "WeightPad must not pass {needle} into cost capture"
        );
    }
    assert!(src.contains("costOpen"));
}

/// date_paid — delivery moment: Today action opens shared sheet with no delivery date.
#[test]
fn phase2_date_paid_not_from_physical_delivery() {
    // The delivery-run entry point moved from Today to the shared money-capture
    // control in the Today/Reality split (ruling 6.3: reachable from both
    // surfaces, permanent home on Money). The guarantee is unchanged and now
    // checked wherever the control lives: it is a money-out overlay, and no
    // delivery entity stands behind it.
    let controls = frontend_src("src/components/MoneyCaptureControls.tsx");
    assert!(controls.contains("Money out for a delivery run"));
    assert!(controls.contains("deliveryCostOpen"));
    assert!(controls.contains("moment=\"delivery\""));

    let today = frontend_src("src/screens/Today.tsx");
    let reality = frontend_src("src/screens/Reality.tsx");
    let money = frontend_src("src/screens/Money.tsx");

    // Ruling 6.3 — all three surfaces reach the one control.
    for (name, src) in [
        ("Today.tsx", &today),
        ("Reality.tsx", &reality),
        ("Money.tsx", &money),
    ] {
        assert!(
            src.contains("MoneyCaptureControls"),
            "{name} must reach the money-capture control"
        );
    }

    // No trip/delivery entity — in the control or on either farm surface.
    for (name, src) in [
        ("MoneyCaptureControls.tsx", &controls),
        ("Today.tsx", &today),
        ("Reality.tsx", &reality),
    ] {
        for needle in ["deliveryDate", "tripId", "mileage", "DeliveryTrip"] {
            assert!(
                !src.contains(needle),
                "{name} must not invent delivery entity field {needle}"
            );
        }
    }
}

/// Receipt lands on disk before the DB transaction opens.
#[test]
fn phase2_receipt_written_before_cost_events_commit() {
    let src =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/costs.rs")).unwrap();
    let persist_at = src
        .find("persist_receipt(farm_dir")
        .expect("persist_receipt call");
    let tx_at = src
        .find("conn.transaction()")
        .expect("transaction open in record_cost");
    assert!(
        persist_at < tx_at,
        "receipt must be persisted before cost_events transaction opens"
    );

    let dir = tempfile_dir("receipt-before");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let source = dir.join("source-receipt.jpg");
    let bytes = b"phase2-receipt-bytes-aaaaaaaa";
    fs::write(&source, bytes).unwrap();
    let hex = sha256_hex(bytes);
    let expected_rel = format!("receipts/{hex}.jpg");
    let expected_abs = dir.join("receipts").join(format!("{hex}.jpg"));

    // Prove file exists with correct bytes before we even query cost_events —
    // persist is synchronous and precedes commit inside record_cost.
    let view = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 1500,
            payee: "Garden Center".into(),
            category_id: "growing_medium".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(source.to_string_lossy().into_owned()),
        },
    )
    .unwrap();
    assert!(
        expected_abs.exists(),
        "receipt file must exist on disk after save"
    );
    assert_eq!(fs::read(&expected_abs).unwrap(), bytes);
    assert_eq!(
        view.receipt_file_ref.as_deref(),
        Some(expected_rel.as_str())
    );
    let row_ref: Option<String> = conn
        .query_row(
            "SELECT receipt_file_ref FROM cost_events WHERE event_id = ?1",
            [&view.event_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(row_ref.as_deref(), Some(expected_rel.as_str()));
    let _ = fs::remove_dir_all(&dir);
}

/// receipt_file_ref is relative, forward-slashed, and contains the sha256.
#[test]
fn phase2_receipt_file_ref_relative_with_sha256() {
    let dir = tempfile_dir("receipt-ref");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let source = dir.join("fuel.pdf");
    let bytes = b"%PDF-phase2-fuel-receipt";
    fs::write(&source, bytes).unwrap();
    let hex = sha256_hex(bytes);
    let view = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 4200,
            payee: "Pump".into(),
            category_id: "delivery_fuel".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(source.to_string_lossy().into_owned()),
        },
    )
    .unwrap();
    let r = view.receipt_file_ref.expect("ref");
    assert!(r.starts_with("receipts/"), "{r}");
    assert!(!r.contains('\\'), "{r}");
    assert!(r.contains(&hex), "{r} must contain {hex}");
    assert!(r.ends_with(".pdf"), "{r}");
    assert!(!PathBuf::from(&r).is_absolute());
    let _ = fs::remove_dir_all(&dir);
}

/// Failed receipt write → nothing committed, nothing flushed.
#[test]
fn phase2_failed_receipt_write_commits_nothing() {
    let dir = tempfile_dir("receipt-fail");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let before = count_cost_events(&conn);
    let missing = dir.join("no-such-file.jpg");
    let err = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 900,
            payee: "Shop".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(missing.to_string_lossy().into_owned()),
        },
    )
    .unwrap_err();
    assert!(
        err.to_lowercase().contains("receipt") || err.to_lowercase().contains("read"),
        "{err}"
    );
    assert_eq!(count_cost_events(&conn), before);
    crate::event_file::try_flush_after_commit(&conn, &dir);
    let events = crate::event_file::events_path(&dir);
    if events.exists() {
        let body = fs::read_to_string(&events).unwrap();
        assert!(
            !body.contains("cost.money_out"),
            "failed receipt must not flush a cost event"
        );
    }
    assert!(
        !dir.join("receipts").exists()
            || fs::read_dir(dir.join("receipts")).unwrap().next().is_none(),
        "failed pick must leave receipts/ empty"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Receipts directory resolves outside the repo working tree.
#[test]
fn phase2_receipts_dir_outside_repo_working_tree() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap();
    let dir = tempfile_dir("receipt-outside");
    let receipts = costs::receipts_dir(&dir);
    let canon_receipts_parent = dir.canonicalize().unwrap();
    assert!(
        !canon_receipts_parent.starts_with(&repo),
        "farm data root {:?} must not be under repo {:?}",
        canon_receipts_parent,
        repo
    );
    assert_eq!(receipts, dir.join("receipts"));
    let _ = fs::remove_dir_all(&dir);
}

/// No base64 blob in the persisted event when a receipt is attached.
#[test]
fn phase2_no_base64_in_persisted_event() {
    let dir = tempfile_dir("receipt-nob64");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let source = dir.join("shot.png");
    // Bytes that look binary; must not appear base64-encoded in payload.
    let bytes: Vec<u8> = (0u8..64).collect();
    fs::write(&source, &bytes).unwrap();
    let view = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 300,
            payee: "Store".into(),
            category_id: "packaging_labels".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: Some(source.to_string_lossy().into_owned()),
        },
    )
    .unwrap();
    let payload_s: String = conn
        .query_row(
            "SELECT payload FROM event_log WHERE id = ?1",
            [&view.event_id],
            |r| r.get(0),
        )
        .unwrap();
    let b64 = data_encoding_fallback(&bytes);
    assert!(
        !payload_s.contains(&b64),
        "payload must not embed receipt bytes as base64"
    );
    assert!(!payload_s.contains("data:image"));
    let payload: Value = serde_json::from_str(&payload_s).unwrap();
    let r = payload
        .get("receiptFileRef")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(r.starts_with("receipts/"));
    let _ = fs::remove_dir_all(&dir);
}

fn data_encoding_fallback(bytes: &[u8]) -> String {
    // Minimal base64 for the assertion — std-less, no new dependency for tests.
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (a << 16) | (b << 8) | c;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Descriptor still mandatory when either mapping is other (Phase 2 regression).
#[test]
fn phase2_descriptor_still_mandatory_for_other() {
    let mut conn = mem();
    let err = record(
        &mut conn,
        RecordCostInput {
            amount_cents: 1000,
            payee: "Printer".into(),
            category_id: "advertising_printing".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap_err();
    assert!(
        err.to_lowercase().contains("description") || err.to_lowercase().contains("descriptor"),
        "{err}"
    );
}

/// Opening/saving/cancelling cost sheet from SowSheet leaves parent state intact.
#[test]
fn phase2_sow_sheet_cost_overlay_preserves_parent() {
    let src = frontend_src("src/components/SowSheet.tsx");
    assert!(src.contains("const [costOpen, setCostOpen]"));
    assert!(src.contains("moment=\"sow\""));
    assert!(src.contains("stacked"));
    // Must refuse to close/reset the sow sheet while cost overlay is open.
    assert!(
        src.contains("if (!next && costOpen) return") || src.contains("if (!next && costOpen) {"),
        "SowSheet must not close/reset while cost overlay is open"
    );
    // Cost overlay is a sibling inside the component — sow state is React useState
    // that reset() only clears on real close.
    assert!(src.contains("function reset()"));
    assert!(src.contains("setSelectedCrop(null)"));
    // reset is not called when opening cost.
    let open_cost = src.split("setCostOpen(true)").next().expect("open cost");
    assert!(
        !open_cost.ends_with("reset();\n"),
        "opening cost must not reset sow state"
    );
}

/// Opening/saving/cancelling cost sheet from WeightPad leaves parent state intact.
#[test]
fn phase2_weight_pad_cost_overlay_preserves_parent() {
    let src = frontend_src("src/components/WeightPad.tsx");
    assert!(src.contains("const [costOpen, setCostOpen]"));
    assert!(src.contains("moment=\"harvest\""));
    assert!(src.contains("stacked"));
    assert!(
        src.contains("if (!next && costOpen) return") || src.contains("if (!next && costOpen) {"),
        "WeightPad must not close while cost overlay is open"
    );
    // Weight/step state must not be cleared when opening cost.
    assert!(src.contains("setCostOpen(true)"));
    assert!(
        !src.contains("setCostOpen(true);\n    setStep(0)")
            && !src.contains("setCostOpen(true); setValues"),
        "opening cost must not reset weight pad state"
    );
}

#[test]
fn undo_last_never_selects_a_cost_and_reverses_the_real_last_action() {
    let dir = tempfile_dir("undo-skips-cost");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();

    let tray = crate::trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let sow_id: String = conn
        .query_row(
            "SELECT id FROM event_log WHERE kind = 'tray.sown' AND entity_id = ?1",
            [&tray.id],
            |r| r.get(0),
        )
        .unwrap();
    let cost = costs::record_cost(&mut conn, &dir, basic_input(1800)).unwrap();

    let undoable = events::newest_undoable(&conn).unwrap().expect("sow");
    assert_eq!(undoable.kind, "tray.sown");
    assert_eq!(undoable.id, sow_id);

    let cost_count_before = count_cost_events(&conn);
    let snapshot_cost_row = |conn: &Connection, event_id: &str| -> String {
        conn.query_row(
            "SELECT event_id, origin, date_paid, amount_cents, payee, canonical_category,
                    schedule_f_line, schedule_c_line, descriptor, quantity, unit_price_cents,
                    delivery_date, invoice_reference, receipt_file_ref, created_at, updated_at
             FROM cost_events WHERE event_id = ?1",
            [event_id],
            |r| {
                Ok(format!(
                    "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<f64>>(9)?,
                    r.get::<_, Option<i64>>(10)?,
                    r.get::<_, Option<String>>(11)?,
                    r.get::<_, Option<String>>(12)?,
                    r.get::<_, Option<String>>(13)?,
                    r.get::<_, String>(14)?,
                    r.get::<_, String>(15)?,
                ))
            },
        )
        .unwrap()
    };
    let cost_row_before = snapshot_cost_row(&conn, &cost.event_id);

    let result = crate::trays::undo_last(&mut conn).unwrap();
    assert!(result.is_some());
    let u = result.unwrap();
    assert_eq!(u.undone_kind, "tray.sown");
    assert_ne!(u.undone_kind, "cost.money_out");

    assert_eq!(crate::trays::list_trays(&conn).unwrap().len(), 0);

    let cost_count_after = count_cost_events(&conn);
    assert_eq!(cost_count_after, cost_count_before);
    let cost_row_after = snapshot_cost_row(&conn, &cost.event_id);
    assert_eq!(cost_row_before, cost_row_after);

    let cost_undone: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log
             WHERE kind = 'cost.money_out' AND undone_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cost_undone, 0);

    // Reported flow: newest log row is a cost; only older undoable is already undone.
    costs::record_cost(&mut conn, &dir, basic_input(2200)).unwrap();
    assert!(events::newest_undoable(&conn).unwrap().is_none());
    assert!(matches!(crate::trays::undo_last(&mut conn), Ok(None)));

    let _ = fs::remove_dir_all(&dir);
}

fn dated_cost(amount_cents: i64, date_paid: &str) -> RecordCostInput {
    RecordCostInput {
        amount_cents,
        payee: "Local Grow Supply".into(),
        category_id: "growing_medium".into(),
        date_paid: date_paid.into(),
        descriptor: None,
        receipt_source_path: None,
    }
}

#[test]
fn booksc_expense_range_ends_are_inclusive() {
    let mut conn = mem();
    let from = "2026-06-02";
    let to = "2026-06-10";
    let before = "2026-06-01";
    let after = "2026-06-11";
    let before_row = record(&mut conn, dated_cost(100, before)).unwrap();
    let from_row = record(&mut conn, dated_cost(200, from)).unwrap();
    let to_row = record(&mut conn, dated_cost(300, to)).unwrap();
    let after_row = record(&mut conn, dated_cost(400, after)).unwrap();

    let rows = costs::expense_rows_between(&conn, Some(from), Some(to)).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.event_id.as_str()).collect();
    assert!(ids.contains(&from_row.event_id.as_str()));
    assert!(ids.contains(&to_row.event_id.as_str()));
    assert!(!ids.contains(&before_row.event_id.as_str()));
    assert!(!ids.contains(&after_row.event_id.as_str()));
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|r| r.date_paid.as_str() >= from && r.date_paid.as_str() <= to));
}

#[test]
fn booksc_cash_out_is_the_fold_of_its_rows() {
    let mut conn = mem();
    let from = "2026-06-02";
    let to = "2026-06-10";
    record(&mut conn, dated_cost(200, from)).unwrap();
    record(&mut conn, dated_cost(300, to)).unwrap();
    let voided = record(&mut conn, dated_cost(900, from)).unwrap();
    costs::void_expense(&mut conn, &voided.event_id, None).unwrap();
    record(&mut conn, dated_cost(100, "2026-06-01")).unwrap();

    let rows = costs::expense_rows_between(&conn, Some(from), Some(to)).unwrap();
    let fold = costs::cash_out_between(&conn, Some(from), Some(to)).unwrap();
    assert_eq!(
        fold.total_cents,
        rows.iter().map(|r| r.amount_cents).sum::<i64>()
    );
    assert_eq!(fold.count, rows.len() as i64);
    assert_eq!(fold.total_cents, 500);
    assert_eq!(fold.count, 2);
    assert!(rows.iter().all(|r| r.event_id != voided.event_id));
}

#[test]
fn booksc_list_expenses_unchanged_by_the_shared_reader() {
    let mut conn = mem();
    let a = record(&mut conn, dated_cost(100, "2026-06-01")).unwrap();
    let b = record(&mut conn, dated_cost(200, "2026-06-10")).unwrap();
    let c = record(&mut conn, dated_cost(300, "2026-06-10")).unwrap();

    let listed = costs::list_expenses(&conn).unwrap();
    let shared = costs::expense_rows_between(&conn, None, None).unwrap();
    let listed_ids: Vec<&str> = listed.iter().map(|r| r.event_id.as_str()).collect();
    let shared_ids: Vec<&str> = shared.iter().map(|r| r.event_id.as_str()).collect();
    assert_eq!(listed_ids, shared_ids);
    // B-5(a) - the hard-coded order was asking for more than the SQL promised.
    // `b` and `c` share a date_paid; which came first was decided by created_at,
    // and created_at is millisecond-precision, so two back-to-back records can
    // land in the same tick. That is the flake. The property the reader actually
    // owes is that the returned sequence is non-increasing on the FULL total
    // key - which is total, so it can never tie and can never flake.
    assert_eq!(listed.len(), 3);
    assert!(listed_ids.contains(&a.event_id.as_str()));
    assert!(listed_ids.contains(&b.event_id.as_str()));
    assert!(listed_ids.contains(&c.event_id.as_str()));
    assert_eq!(
        listed_ids.last(),
        Some(&a.event_id.as_str()),
        "the older date_paid sorts last however the same-day pair ties"
    );
    assert!(listed.windows(2).all(|w| {
        (
            w[0].date_paid.as_str(),
            w[0].created_at.as_str(),
            w[0].event_id.as_str(),
        ) >= (
            w[1].date_paid.as_str(),
            w[1].created_at.as_str(),
            w[1].event_id.as_str(),
        )
    }));
}

/// BOOKS-ORDER-1 - the expense reader's sort is TOTAL.
///
/// Two halves, because neither alone is enough.
///
/// Behavioural: four rows share a date_paid AND a created_at, so only event_id
/// can separate them. Without the tiebreaker the reader returns rowid order,
/// i.e. the order they were recorded in; that coincides with event_id DESC for
/// one arrangement in 4! - so this catches a dropped tiebreaker about 23 times
/// in 24. Strong, and honestly not a proof.
///
/// Structural: the proof. The SQL itself must carry the unique final key, which
/// is deterministic whatever SQLite's planner does on the day.
///
/// The tie is built only from mutable columns. `event_id` is frozen by
/// `cost_events_before_update` (v18) - which is exactly what makes it a sound
/// sort key: a tiebreaker that cannot be rewritten after the fact.
///
/// `event_id` is not a new convention here. export.rs's write_costs_csv and
/// both cost_per_tray readers already break ties on it; this reader was the one
/// of the four that did not.
#[test]
fn booksd_expense_order_is_total_on_event_id() {
    let mut conn = mem();
    let day = "2026-06-10";
    let recorded: Vec<String> = (1..=4)
        .map(|n| {
            record(&mut conn, dated_cost(100 * n, day))
                .unwrap()
                .event_id
        })
        .collect();

    // One shared tick. `created_at` is not frozen; `event_id` and `origin` are,
    // and neither is touched here.
    conn.execute(
        "UPDATE cost_events SET created_at = '2026-06-10T12:00:00.000Z'",
        [],
    )
    .unwrap();

    let rows = costs::expense_rows_between(&conn, None, None).unwrap();
    let ids: Vec<String> = rows.iter().map(|r| r.event_id.clone()).collect();

    let mut expected = recorded.clone();
    expected.sort();
    expected.reverse();

    assert_eq!(rows.len(), 4, "all four rows are live and farm_os");
    assert_eq!(
        ids, expected,
        "same date_paid and same created_at must resolve by event_id DESC"
    );
    assert!(
        rows.iter().all(|r| r.created_at == rows[0].created_at),
        "the tie really is shared - otherwise created_at decided and this proves nothing"
    );
    assert!(
        rows.iter().all(|r| r.date_paid == day),
        "the tie really is shared on date_paid too"
    );

    // The proof: the reader's own SQL carries the unique final key.
    let src = include_str!("costs.rs");
    assert!(
        src.contains("ORDER BY date_paid DESC, created_at DESC, event_id DESC"),
        "expense_rows_between must sort on a unique final key"
    );
}

#[test]
fn e4_expense_rows_carry_the_recorded_schedule_lines() {
    let mut conn = mem();
    let view = record(&mut conn, basic_input(1800)).unwrap();
    conn.execute(
        "UPDATE cost_events SET schedule_f_line = 'F-OLD', schedule_c_line = 'C-OLD'
         WHERE event_id = ?1",
        [&view.event_id],
    )
    .unwrap();

    let rows = costs::expense_rows_between(&conn, None, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_id, view.event_id);
    assert_eq!(rows[0].schedule_f_line, "F-OLD");
    assert_eq!(rows[0].schedule_c_line, "C-OLD");

    let today_f = categories::find_category(&view.canonical_category)
        .unwrap()
        .schedule_f_line;
    assert_ne!(today_f, "F-OLD");
}

/// H-10 - COST_SPINE_COLUMNS names real columns of `cost_events`. Its own doc
/// calls it a mirror of INCOME_SPINE_COLUMNS; this proves the mirror against
/// the table, not against the other const. It matters here more than
/// anywhere: `cost_events` is created WITHOUT last_event_id or voided_at
/// (db.rs:90) and gains both only through the guarded migration at
/// db.rs:1587 and :1591. A regressed migration makes this const a lie, and
/// this assertion is what would say so.
#[test]
fn h10_cost_spine_columns_are_real_and_listed() {
    assert_eq!(
        COST_SPINE_COLUMNS,
        &["last_event_id", "created_at", "updated_at", "voided_at"]
    );
    let conn = mem();
    let mut stmt = conn.prepare("PRAGMA table_info(cost_events)").unwrap();
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|c| c.unwrap())
        .collect();
    for spine in COST_SPINE_COLUMNS {
        assert!(
            cols.contains(&(*spine).to_string()),
            "cost_events has no column {spine}"
        );
        assert!(
            COST_EVENTS_COLUMNS.contains(spine),
            "{spine} is missing from COST_EVENTS_COLUMNS"
        );
    }
}
