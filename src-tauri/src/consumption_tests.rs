//! Track 4 Phase 1 — sow consumption + seed rate proofs (T1–T9).

use crate::consumption::{
    self, validate_consumption_payload, ConsumptionPayload, CONSUMPTION_PAYLOAD_FIELD_NAMES,
    CONSUMPTION_PAYLOAD_KEYS, FORBIDDEN_MONETARY_KEYS, TRAY_VARIETY_OR_ITEM, UNIT_OZ,
    UNIT_PLANTING, UNIT_TRAY,
};
use crate::costs::{self, RecordCostInput};
use crate::db;
use crate::event_partition::{EventClass, EventDomain, Kind};
use crate::events::{self, EventRecord};
use crate::models::{HarvestInput, RecountEntry};
use crate::projection;
use crate::seed_prefill::{self, SeedFieldState};
use crate::trays;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "farm-os-consumption-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn consumption_rows(conn: &Connection) -> Vec<(String, String, String, f64, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT e.id, c.variety_or_item, c.unit, c.quantity, c.origin
             FROM consumption_events c
             JOIN event_log e ON e.id = c.event_id
             ORDER BY e.seq ASC",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .map(|x| x.unwrap())
    .collect()
}

fn event_payloads_for_kind(conn: &Connection, kind: &str) -> Vec<Value> {
    let mut stmt = conn
        .prepare("SELECT payload FROM event_log WHERE kind = ?1 ORDER BY seq ASC")
        .unwrap();
    stmt.query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|s| serde_json::from_str(&s.unwrap()).unwrap())
        .collect()
}

/// T1 — sow confirm writes exactly two consumption records with §3 shape.
#[test]
fn t1_sow_writes_two_consumption_records() {
    let mut conn = mem();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 3, Some(22.5)).unwrap();
    let rows = consumption_rows(&conn);
    assert_eq!(rows.len(), 2, "expected tray + seed consumption");

    let tray = &rows[0];
    assert_eq!(tray.1, TRAY_VARIETY_OR_ITEM);
    assert_eq!(tray.2, UNIT_TRAY);
    assert_eq!(tray.3, 3.0);
    assert_eq!(tray.4, "farm_os");

    let seed = &rows[1];
    assert_eq!(seed.1, "Dun peas");
    assert_eq!(seed.2, UNIT_OZ);
    assert_eq!(seed.3, 22.5);
    assert_eq!(seed.4, "farm_os");

    for (event_id, _, _, _, _) in &rows {
        assert!(!event_id.is_empty());
        let (origin, domain, class, kind): (String, String, Option<String>, String) = conn
            .query_row(
                "SELECT origin, event_domain, event_class, kind FROM event_log WHERE id = ?1",
                [event_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(origin, "farm_os");
        assert_eq!(domain, "register");
        assert_eq!(class.as_deref(), Some("physical_consumption"));
        assert_eq!(kind, Kind::ConsumptionPhysical.as_str());
    }

    let (tier_d, tier_c) = Kind::ConsumptionPhysical.tier();
    assert_eq!(tier_d, EventDomain::Register);
    assert_eq!(tier_c, Some(EventClass::PhysicalConsumption));
}

/// T1b — NULL-rate crop, blank seed → exactly one consumption (trays).
#[test]
fn t1b_null_rate_blank_seed_writes_one_tray_record() {
    let mut conn = mem();
    let rate: Option<f64> = conn
        .query_row(
            "SELECT seed_rate_oz_per_tray FROM crops WHERE id = 'sunflower'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(rate.is_none());
    trays::sow_tray_with_seed(&mut conn, "sunflower", 2, None).unwrap();
    let rows = consumption_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, TRAY_VARIETY_OR_ITEM);
    assert_eq!(rows[0].2, UNIT_TRAY);
    assert_eq!(rows[0].3, 2.0);
}

/// T2 — harvest yield is never a consumption quantity (planting record is separate).
#[test]
fn t2_harvest_yield_is_never_a_consumption_quantity() {
    let mut conn = mem();
    let tray = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    // Advance blackout → light for harvest.
    trays::advance_tray(&mut conn, &tray.id).unwrap();
    let before = consumption_rows(&conn).len();
    assert!(before >= 1, "sow already wrote tray consumption");

    trays::harvest_groups(
        &mut conn,
        &[HarvestInput {
            tray_ids: vec![tray.id.clone()],
            actual_yield_oz: 18.5,
        }],
    )
    .unwrap();

    let after = consumption_rows(&conn);
    assert_eq!(
        after.len(),
        before + 1,
        "harvest emits exactly one planting consumption record"
    );
    for (_id, _v, unit, qty, _) in &after {
        assert!(
            !(*unit == UNIT_OZ && (*qty - 18.5).abs() < f64::EPSILON),
            "actualYieldOz must never become a consumption quantity"
        );
    }
    let harvest_payloads = event_payloads_for_kind(&conn, "trays.harvested");
    assert_eq!(harvest_payloads.len(), 1);
    let harvested_oz = harvest_payloads[0]["groups"][0]["actualYieldOz"]
        .as_f64()
        .unwrap();
    assert!((harvested_oz - 18.5).abs() < f64::EPSILON);
    let cons_oz: Vec<f64> = after
        .iter()
        .filter(|r| r.2 == UNIT_OZ)
        .map(|r| r.3)
        .collect();
    assert!(
        !cons_oz.iter().any(|q| (*q - 18.5).abs() < f64::EPSILON),
        "actualYieldOz {harvested_oz} must not appear as consumption quantity"
    );
}

/// T3a — shelf wins: pre-fill 24.0, correct to 22.5, stored 22.5.
#[test]
fn t3a_shelf_wins_stored_quantity() {
    let mut field = SeedFieldState::fresh_proposal(Some(8.0), 3);
    assert_eq!(field.value, "24.0");
    field.on_operator_edit("22.5".into());
    field.on_proposal_inputs_changed(Some(8.0), 3);
    assert_eq!(field.confirm_quantity().unwrap(), Some(22.5));

    let mut conn = mem();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 3, Some(22.5)).unwrap();
    let seed = consumption_rows(&conn)
        .into_iter()
        .find(|r| r.2 == UNIT_OZ)
        .unwrap();
    assert_eq!(seed.3, 22.5);
}

/// T3b — tray count change after edit does not clobber.
#[test]
fn t3b_shelf_wins_under_recompute() {
    let mut field = SeedFieldState::fresh_proposal(Some(8.0), 3);
    field.on_operator_edit("22.5".into());
    field.on_proposal_inputs_changed(Some(8.0), 7);
    assert_eq!(field.value, "22.5");
    assert_eq!(field.confirm_quantity().unwrap(), Some(22.5));
}

/// T4 — choke point rejects bad quantities.
#[test]
fn t4_choke_rejects_bad_quantity() {
    let mut conn = mem();
    let before = trays::count_event_log(&conn).unwrap();
    for bad in [json!(0.0), json!(-1.0), Value::Null, json!("missing-sim")] {
        let mut payload = json!({
            "eventId": "ev-bad-q",
            "origin": "farm_os",
            "occurredAt": "2026-08-07T12:00:00.000Z",
            "varietyOrItem": "tray",
            "unit": "tray",
            "quantity": bad,
        });
        if bad == json!("missing-sim") {
            payload.as_object_mut().unwrap().remove("quantity");
        }
        let err = validate_consumption_payload(&payload).unwrap_err();
        assert!(err.contains("quantity") || err.contains("missing"), "{err}");
    }
    // NaN / Infinity via raw JSON numbers are not representable in serde_json::Number
    // the same way — exercise the finite check through f64 injection.
    for bad_f in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut map = serde_json::Map::new();
        map.insert("eventId".into(), json!("ev-nan"));
        map.insert("origin".into(), json!("farm_os"));
        map.insert("occurredAt".into(), json!("2026-08-07T12:00:00.000Z"));
        map.insert("varietyOrItem".into(), json!("tray"));
        map.insert("unit".into(), json!("tray"));
        map.insert(
            "quantity".into(),
            Value::Number(serde_json::Number::from_f64(bad_f).unwrap_or_else(|| {
                // Number::from_f64 rejects non-finite — build via validate path with a
                // finite placeholder then swap using a custom check.
                serde_json::Number::from_f64(1.0).unwrap()
            })),
        );
        if !bad_f.is_finite() {
            // Direct unit test of the finite gate.
            assert!(
                consumption::build_consumption_event(consumption::RecordConsumptionInput {
                    variety_or_item: "tray".into(),
                    unit: "tray".into(),
                    quantity: bad_f,
                    occurred_at: "2026-08-07T12:00:00.000Z".into(),
                    sow_event_id: None,
                    linked_cost_event_id: None,
                    notes: None,
                })
                .is_err()
            );
        } else {
            let _ = map;
        }
    }
    // Write path: reject zero via insert.
    let event = EventRecord::originated(
        Kind::ConsumptionPhysical,
        "consumption",
        "ev-zero",
        json!({
            "eventId": "ev-zero",
            "origin": "farm_os",
            "occurredAt": "2026-08-07T12:00:00.000Z",
            "varietyOrItem": "tray",
            "unit": "tray",
            "quantity": 0.0,
        }),
        json!({ "op": "none" }),
        "2026-08-07T12:00:00.000Z",
        None,
        None,
        Some("ev-zero".into()),
    );
    {
        let tx = conn.transaction().unwrap();
        let err = events::write_event(&tx, &event).unwrap_err();
        assert!(err.contains("quantity") || err.contains("zero") || err.contains("greater"));
        drop(tx);
    }
    assert_eq!(trays::count_event_log(&conn).unwrap(), before);
    assert_eq!(consumption_rows(&conn).len(), 0);
}

/// T5 — sealed: extra keys rejected (parameterized monetary names).
#[test]
fn t5_sealed_rejects_extra_keys() {
    for key in [
        "dollars",
        "amount",
        "price",
        "cost",
        "unit_cost",
        "value",
        "total",
        "usd",
        "cents",
    ] {
        let mut payload = json!({
            "eventId": "ev-extra",
            "origin": "farm_os",
            "occurredAt": "2026-08-07T12:00:00.000Z",
            "varietyOrItem": "tray",
            "unit": "tray",
            "quantity": 1.0,
        });
        payload
            .as_object_mut()
            .unwrap()
            .insert(key.into(), json!(1));
        let err = validate_consumption_payload(&payload).unwrap_err();
        assert!(
            err.contains("unknown") || err.contains("monetary") || err.contains(key),
            "key {key}: {err}"
        );

        let mut conn = mem();
        let event = EventRecord::originated(
            Kind::ConsumptionPhysical,
            "consumption",
            "ev-extra",
            payload,
            json!({ "op": "none" }),
            "2026-08-07T12:00:00.000Z",
            None,
            None,
            Some("ev-extra".into()),
        );
        let tx = conn.transaction().unwrap();
        assert!(events::write_event(&tx, &event).is_err());
        drop(tx);
        assert_eq!(consumption_rows(&conn).len(), 0);
    }
}

/// T6a — poisoned fixture crop fields never reach events; no cost event.
#[test]
fn t6a_poisoned_rate_fixture_writes_no_money() {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PoisonedCropFixture {
        id: &'static str,
        name: &'static str,
        seed_rate_oz_per_tray: f64,
        dollars: f64,
        amount: f64,
        price: f64,
        cost: f64,
        unit_cost: f64,
        value: f64,
        total: f64,
        usd: f64,
        cents: i64,
    }
    let fixture = PoisonedCropFixture {
        id: "fixture-poison-rate",
        name: "Fixture poison rate",
        seed_rate_oz_per_tray: 2.0,
        dollars: 9.99,
        amount: 9.99,
        price: 4.50,
        cost: 1.0,
        unit_cost: 0.5,
        value: 100.0,
        total: 200.0,
        usd: 9.99,
        cents: 999,
    };
    let mut conn = mem();
    conn.execute(
        "INSERT INTO crops
         (id, name, growth_days, blackout_days, expected_yield_oz, sort_order,
          seed_rate_oz_per_tray)
         VALUES (?1, ?2, 8, 3, 5.0, 99, ?3)",
        rusqlite::params![fixture.id, fixture.name, fixture.seed_rate_oz_per_tray],
    )
    .unwrap();

    let seed = seed_prefill::proposed_seed_oz(Some(fixture.seed_rate_oz_per_tray), 2).unwrap();
    trays::sow_tray_with_seed(&mut conn, fixture.id, 2, Some(seed)).unwrap();

    let cost_n: i64 = conn
        .query_row("SELECT COUNT(*) FROM cost_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cost_n, 0);

    let mut stmt = conn
        .prepare("SELECT payload FROM event_log ORDER BY seq ASC")
        .unwrap();
    let payloads: Vec<Value> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|s| serde_json::from_str(&s.unwrap()).unwrap())
        .collect();
    for p in payloads {
        let obj = p.as_object().unwrap();
        for key in obj.keys() {
            for forbidden in FORBIDDEN_MONETARY_KEYS {
                assert!(
                    !key.eq_ignore_ascii_case(forbidden),
                    "event payload carried monetary key {key}"
                );
            }
        }
    }
    // Fixture itself carries money fields — prove they exist on the fixture only.
    let fixture_json = serde_json::to_value(&fixture).unwrap();
    assert!(fixture_json.get("dollars").is_some());
}

/// T6b — seed_prefill module has no reachability edge to costs/money.
#[test]
fn t6b_seed_prefill_no_money_reachability() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut visited = HashSet::new();
    let mut stack = vec!["seed_prefill".to_string()];
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();

    while let Some(module) = stack.pop() {
        if !visited.insert(module.clone()) {
            continue;
        }
        let path = module_source_path(&manifest, &module);
        let src =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let imports = crate_imports_from_source(&src);
        edges.insert(module.clone(), imports.clone());
        for dep in imports {
            if dep == "costs"
                || dep == "money"
                || dep.starts_with("costs::")
                || dep.starts_with("money::")
            {
                panic!("seed_prefill reaches money module via {module} -> {dep}");
            }
            // Only walk intra-crate modules that exist as files.
            if module_source_path(&manifest, &dep).exists() {
                stack.push(dep);
            }
        }
    }
    assert!(visited.contains("seed_prefill"));
    // Explicit: no record_cost symbol in the seed_prefill source tree walk.
    for mod_name in &visited {
        let src = fs::read_to_string(module_source_path(&manifest, mod_name)).unwrap();
        assert!(
            !src.contains("record_cost") && !src.contains("costs::"),
            "{mod_name} must not reference costs::record_cost"
        );
    }
}

fn module_source_path(src_root: &Path, module: &str) -> PathBuf {
    let as_file = src_root.join(format!("{module}.rs"));
    if as_file.exists() {
        return as_file;
    }
    src_root.join(module).join("mod.rs")
}

fn crate_imports_from_source(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("use crate::") {
            let rest = rest.trim_end_matches(';');
            let head = rest.split("::").next().unwrap_or(rest);
            let head = head.split('{').next().unwrap_or(head).trim();
            if !head.is_empty() {
                out.push(head.to_string());
            }
        }
    }
    out
}

/// T6c — type/schema level: payload admits no monetary key.
#[test]
fn t6c_payload_type_no_monetary_keys() {
    for name in CONSUMPTION_PAYLOAD_FIELD_NAMES {
        for forbidden in FORBIDDEN_MONETARY_KEYS {
            assert!(!name.eq_ignore_ascii_case(forbidden));
        }
    }
    assert_eq!(
        CONSUMPTION_PAYLOAD_KEYS.len(),
        CONSUMPTION_PAYLOAD_FIELD_NAMES.len()
    );
    let bad = r#"{
        "eventId":"e","origin":"farm_os","occurredAt":"2026-08-07T00:00:00.000Z",
        "varietyOrItem":"tray","unit":"tray","quantity":1.0,"price":3.0
    }"#;
    assert!(serde_json::from_str::<ConsumptionPayload>(bad).is_err());
}

/// T6b (crops) — crops has no monetary column.
#[test]
fn t6b_crops_has_no_monetary_column() {
    let conn = mem();
    let mut stmt = conn.prepare("PRAGMA table_info(crops)").unwrap();
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|c| c.unwrap())
        .collect();
    assert!(cols.contains(&"seed_rate_oz_per_tray".into()));
    for col in &cols {
        let lower = col.to_ascii_lowercase();
        for needle in [
            "cost", "price", "dollar", "cent", "amount", "money", "usd", "value", "total",
        ] {
            assert!(
                !lower.contains(needle),
                "crops must not gain monetary column {col}"
            );
        }
        // "rate" alone is allowed only as seed_rate_oz_per_tray (physical).
        if lower.contains("rate") {
            assert_eq!(col, "seed_rate_oz_per_tray");
        }
    }
}

/// T7 — origin unweakened; non-farm_os consumption rejected.
#[test]
fn t7_origin_unweakened() {
    assert_eq!(
        Kind::ConsumptionPhysical.tier(),
        (EventDomain::Register, Some(EventClass::PhysicalConsumption))
    );
    let mut conn = mem();
    let mut event = EventRecord::originated(
        Kind::ConsumptionPhysical,
        "consumption",
        "ev-bad-origin",
        json!({
            "eventId": "ev-bad-origin",
            "origin": "farm_os",
            "occurredAt": "2026-08-07T12:00:00.000Z",
            "varietyOrItem": "tray",
            "unit": "tray",
            "quantity": 1.0,
        }),
        json!({ "op": "none" }),
        "2026-08-07T12:00:00.000Z",
        None,
        None,
        Some("ev-bad-origin".into()),
    );
    event.origin = "commercial_app".into();
    let tx = conn.transaction().unwrap();
    let err = events::write_event(&tx, &event).unwrap_err();
    assert!(err.contains("origin") || err.contains("farm_os"), "{err}");
    drop(tx);
    assert_eq!(consumption_rows(&conn).len(), 0);

    // Payload origin mismatch also rejected.
    let event2 = EventRecord::originated(
        Kind::ConsumptionPhysical,
        "consumption",
        "ev-payload-origin",
        json!({
            "eventId": "ev-payload-origin",
            "origin": "commercial_app",
            "occurredAt": "2026-08-07T12:00:00.000Z",
            "varietyOrItem": "tray",
            "unit": "tray",
            "quantity": 1.0,
        }),
        json!({ "op": "none" }),
        "2026-08-07T12:00:00.000Z",
        None,
        None,
        Some("ev-payload-origin".into()),
    );
    let tx = conn.transaction().unwrap();
    assert!(events::write_event(&tx, &event2).is_err());
}

/// T8 — v10 fixture migrates to v11 with rates, intact events, new kind accepted.
#[test]
fn t8_v10_migrates_to_v11() {
    let conn = db::open_v10_in_memory().unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 10);
    assert!(!crops_has_column(&conn, "seed_rate_oz_per_tray"));

    // Pre-existing grow event at v10 (cannot use sow_tray — it now emits
    // consumption.physical, which v10 triggers reject).
    let prior_id = "prior-grow-v10";
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
           '{\"op\":\"delete_tray\",\"trayId\":\"prior-grow-v10\"}',
           '2026-08-06T12:00:00.000Z', 'farm_os', 'grow', NULL)",
        [prior_id],
    )
    .unwrap();

    // v10 triggers must reject the new kind before migration.
    let reject = conn.execute(
        "INSERT INTO event_log
         (id, kind, entity_type, entity_id, payload, inverse, created_at,
          origin, event_domain, event_class)
         VALUES ('pre-mig-cons', 'consumption.physical', 'consumption', 'x',
           '{}', '{\"op\":\"none\"}', '2026-08-06T12:00:00.000Z',
           'farm_os', 'register', 'physical_consumption')",
        [],
    );
    assert!(
        reject.is_err(),
        "v10 triggers must reject consumption.physical"
    );

    let events_before: Vec<(i64, String, String)> = {
        let mut stmt = conn
            .prepare("SELECT seq, id, kind FROM event_log ORDER BY seq")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|x| x.unwrap())
            .collect()
    };
    assert!(!events_before.is_empty());

    db::migrate(&conn).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, db::SCHEMA_VERSION);
    assert!(crops_has_column(&conn, "seed_rate_oz_per_tray"));

    let rates: HashMap<String, Option<f64>> = {
        let mut stmt = conn
            .prepare("SELECT id, seed_rate_oz_per_tray FROM crops")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))
            .unwrap()
            .map(|x| x.unwrap())
            .collect()
    };
    assert_eq!(rates.get("dun-peas").copied().flatten(), Some(8.0));
    assert_eq!(rates.get("mellow-mix").copied().flatten(), Some(0.6));
    assert_eq!(rates.get("spicy-mix").copied().flatten(), Some(0.6));
    assert_eq!(rates.get("red-arrow-radish").copied().flatten(), Some(1.0));
    assert_eq!(rates.get("purple-kohlrabi").copied().flatten(), Some(0.6));
    assert!(rates.get("sunflower").unwrap().is_none());
    assert!(rates.get("broccoli").unwrap().is_none());
    assert!(rates.get("kale").unwrap().is_none());

    let events_after: Vec<(i64, String, String)> = {
        let mut stmt = conn
            .prepare("SELECT seq, id, kind FROM event_log ORDER BY seq")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|x| x.unwrap())
            .collect()
    };
    assert_eq!(
        events_after, events_before,
        "migration must not touch prior events"
    );

    // Reinstalled triggers accept the new kind.
    let mut conn = conn;
    trays::sow_tray_with_seed(&mut conn, "kale", 1, None).unwrap();
    assert!(
        consumption_rows(&conn)
            .iter()
            .any(|r| r.1 == TRAY_VARIETY_OR_ITEM),
        "post-migration sow must land consumption.physical"
    );
}

/// T9 — new kind round-trips events.jsonl; verify_replay passes.
#[test]
fn t9_spine_round_trip_verify_replay() {
    let dir = tempfile_dir("t9-spine");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();

    // Pre-existing grow + cost, then additional consumption via a second sow.
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 500,
            payee: "Seed Co".into(),
            category_id: "growing_medium".into(),
            date_paid: chrono::Local::now().format("%Y-%m-%d").to_string(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    trays::sow_tray_with_seed(&mut conn, "mellow-mix", 2, Some(1.2)).unwrap();
    crate::event_file::try_flush_after_commit(&conn, &dir);
    drop(conn);

    let jsonl = dir.join("events.jsonl");
    assert!(jsonl.exists());
    let text = fs::read_to_string(&jsonl).unwrap();
    assert!(
        text.contains("consumption.physical"),
        "events.jsonl must carry the new kind"
    );
    assert!(text.contains("cost.money_out") || text.contains("tray.sown"));

    let outcome = projection::verify_replay_paths(&farm, &jsonl, &dir).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify_replay failed: {}",
        outcome.summary_line()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Pass 5 T1 — sow links both consumption records to the tray.sown event id.
#[test]
fn pass5_t1_sow_links_both_records_via_sow_event_id() {
    let mut conn = mem();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 3, Some(22.5)).unwrap();

    let sow_id: String = conn
        .query_row(
            "SELECT id FROM event_log WHERE kind = 'tray.sown' ORDER BY seq ASC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let payloads = event_payloads_for_kind(&conn, "consumption.physical");
    assert_eq!(payloads.len(), 2, "expected tray + seed consumption");
    for p in &payloads {
        assert_eq!(
            p.get("sowEventId").and_then(|v| v.as_str()),
            Some(sow_id.as_str()),
            "both consumption payloads must carry sowEventId = tray.sown id"
        );
    }
}

/// Pass 5 T2 — legacy payloads without sowEventId still deserialize (no backfill).
#[test]
fn pass5_t2_legacy_payload_deserializes_without_sow_event_id() {
    let legacy = r#"{
        "eventId": "ev-legacy",
        "origin": "farm_os",
        "occurredAt": "2026-08-07T12:00:00.000Z",
        "varietyOrItem": "tray",
        "unit": "tray",
        "quantity": 1.0
    }"#;
    let parsed: ConsumptionPayload = serde_json::from_str(legacy).unwrap();
    assert_eq!(parsed.sow_event_id, None);
}

/// Pass 5 T3 — sowEventId link survives undo; consumption is not compensated.
#[test]
fn pass5_t3_sow_event_id_survives_undo() {
    let mut conn = mem();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(16.0)).unwrap();

    let sow_id: String = conn
        .query_row(
            "SELECT id FROM event_log WHERE kind = 'tray.sown' ORDER BY seq ASC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let before_cons = consumption_rows(&conn).len();
    assert_eq!(before_cons, 2);

    trays::undo_last(&mut conn).unwrap().expect("undo sow");

    assert_eq!(
        trays::list_trays(&conn).unwrap().len(),
        0,
        "tray row deleted"
    );
    let after_cons = consumption_rows(&conn);
    assert_eq!(
        after_cons.len(),
        before_cons,
        "consumption.physical rows survive; no compensating event"
    );

    let payloads = event_payloads_for_kind(&conn, "consumption.physical");
    assert_eq!(payloads.len(), 2);
    for p in &payloads {
        let linked = p
            .get("sowEventId")
            .and_then(|v| v.as_str())
            .expect("sowEventId present");
        assert_eq!(linked, sow_id);
        let undone_at: Option<String> = conn
            .query_row(
                "SELECT undone_at FROM event_log WHERE id = ?1",
                [linked],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            undone_at.is_some(),
            "sow event_log row must survive with undone_at set"
        );
    }

    let cons_kind_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE kind = 'consumption.physical'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        cons_kind_count, 2,
        "no compensating consumption.physical appended on undo"
    );
}

fn crops_has_column(conn: &Connection, name: &str) -> bool {
    let mut stmt = conn.prepare("PRAGMA table_info(crops)").unwrap();
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|c| c.unwrap())
        .collect();
    rows.iter().any(|c| c == name)
}

fn consumption_events_has_column(conn: &Connection, name: &str) -> bool {
    let mut stmt = conn
        .prepare("PRAGMA table_info(consumption_events)")
        .unwrap();
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|c| c.unwrap())
        .collect();
    rows.iter().any(|c| c == name)
}

fn consumption_events_sql(conn: &Connection) -> String {
    conn.query_row(
        "SELECT sql FROM sqlite_master
         WHERE type = 'table' AND name = 'consumption_events'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// Pass 12 — v11 fixture migrates to v12; sow_event_id present and NULL on prior rows.
#[test]
fn pass12_v11_migrates_to_v12_sow_event_id_null_on_prior_rows() {
    let conn = db::open_v11_in_memory().unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 11);
    assert!(!consumption_events_has_column(&conn, "sow_event_id"));

    conn.execute(
        "INSERT INTO consumption_events
         (event_id, origin, occurred_at, variety_or_item, unit, quantity,
          linked_cost_event_id, notes)
         VALUES ('ev-prior-v11', 'farm_os', '2026-08-06T12:00:00.000Z', 'tray',
                 'tray', 1.0, NULL, NULL)",
        [],
    )
    .unwrap();

    db::migrate(&conn).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, db::SCHEMA_VERSION);
    assert!(consumption_events_has_column(&conn, "sow_event_id"));

    let sow_id: Option<String> = conn
        .query_row(
            "SELECT sow_event_id FROM consumption_events WHERE event_id = 'ev-prior-v11'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        sow_id.is_none(),
        "pre-existing rows stay NULL — no backfill"
    );
}

/// Pass 12 — ladder-from-0 and v11→v12 migrate converge on consumption_events schema.
#[test]
fn pass12_fresh_and_migrated_consumption_events_schema_converge() {
    let migrated = db::open_v11_in_memory().unwrap();
    db::migrate(&migrated).unwrap();
    let fresh = db::open_in_memory().unwrap();

    let migrated_sql = consumption_events_sql(&migrated);
    let fresh_sql = consumption_events_sql(&fresh);
    assert_eq!(
        migrated_sql, fresh_sql,
        "consumption_events sqlite_master SQL must match\nmigrated:\n{migrated_sql}\nfresh:\n{fresh_sql}"
    );
    assert!(
        migrated_sql.to_ascii_lowercase().contains("sow_event_id"),
        "converged schema must include sow_event_id"
    );
}

/// Pass 12 — projection mirrors sowEventId into consumption_events.sow_event_id.
#[test]
fn pass12_projection_mirrors_sow_event_id_for_both_rows() {
    let mut conn = mem();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 3, Some(22.5)).unwrap();

    let sow_id: String = conn
        .query_row(
            "SELECT id FROM event_log WHERE kind = 'tray.sown' ORDER BY seq ASC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let mut stmt = conn
        .prepare(
            "SELECT sow_event_id FROM consumption_events
             ORDER BY event_id",
        )
        .unwrap();
    let ids: Vec<Option<String>> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|x| x.unwrap())
        .collect();
    assert_eq!(ids.len(), 2, "expected tray + seed consumption rows");
    for id in &ids {
        assert_eq!(
            id.as_deref(),
            Some(sow_id.as_str()),
            "both consumption_events rows must mirror tray.sown id"
        );
    }
}

// --- Pass 16: operator update_crop_seed_rate ---------------------------------

fn rate_of(conn: &Connection, crop_id: &str) -> Option<f64> {
    trays::list_crops(conn)
        .unwrap()
        .into_iter()
        .find(|c| c.id == crop_id)
        .unwrap()
        .seed_rate_oz_per_tray
}

fn dump_event_log(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT seq, id, kind, entity_type, entity_id, payload, inverse,
                    undone_at, undoes_seq, created_at, origin, event_domain,
                    event_class, reverses_event_id
             FROM event_log ORDER BY seq ASC",
        )
        .unwrap();
    stmt.query_map([], |r| {
        let seq: i64 = r.get(0)?;
        let id: String = r.get(1)?;
        let kind: String = r.get(2)?;
        let entity_type: String = r.get(3)?;
        let entity_id: String = r.get(4)?;
        let payload: String = r.get(5)?;
        let inverse: String = r.get(6)?;
        let undone_at: Option<String> = r.get(7)?;
        let undoes_seq: Option<i64> = r.get(8)?;
        let created_at: String = r.get(9)?;
        let origin: Option<String> = r.get(10)?;
        let event_domain: Option<String> = r.get(11)?;
        let event_class: Option<String> = r.get(12)?;
        let reverses_event_id: Option<String> = r.get(13)?;
        Ok(format!(
            "{seq}|{id}|{kind}|{entity_type}|{entity_id}|{payload}|{inverse}|{undone_at:?}|{undoes_seq:?}|{created_at}|{origin:?}|{event_domain:?}|{event_class:?}|{reverses_event_id:?}"
        ))
    })
    .unwrap()
    .map(|x| x.unwrap())
    .collect()
}

fn dump_consumption_events(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT event_id, origin, occurred_at, variety_or_item, unit, quantity,
                    linked_cost_event_id, notes, sow_event_id
             FROM consumption_events ORDER BY event_id ASC",
        )
        .unwrap();
    stmt.query_map([], |r| {
        let event_id: String = r.get(0)?;
        let origin: String = r.get(1)?;
        let occurred_at: String = r.get(2)?;
        let variety_or_item: String = r.get(3)?;
        let unit: String = r.get(4)?;
        let quantity: f64 = r.get(5)?;
        let linked_cost_event_id: Option<String> = r.get(6)?;
        let notes: Option<String> = r.get(7)?;
        let sow_event_id: Option<String> = r.get(8)?;
        Ok(format!(
            "{event_id}|{origin}|{occurred_at}|{variety_or_item}|{unit}|{quantity}|{linked_cost_event_id:?}|{notes:?}|{sow_event_id:?}"
        ))
    })
    .unwrap()
    .map(|x| x.unwrap())
    .collect()
}

#[test]
fn pass16_update_crop_seed_rate_some_valid_persists() {
    let conn = mem();
    trays::update_crop_seed_rate(&conn, "broccoli", Some(1.25)).unwrap();
    assert_eq!(rate_of(&conn, "broccoli"), Some(1.25));
}

#[test]
fn pass16_update_crop_seed_rate_none_persists_null() {
    let conn = mem();
    assert_eq!(rate_of(&conn, "dun-peas"), Some(8.0));
    trays::update_crop_seed_rate(&conn, "dun-peas", None).unwrap();
    assert_eq!(rate_of(&conn, "dun-peas"), None);
}

#[test]
fn pass16_update_crop_seed_rate_rejects_zero() {
    let conn = mem();
    let before = rate_of(&conn, "dun-peas");
    let err = trays::update_crop_seed_rate(&conn, "dun-peas", Some(0.0)).unwrap_err();
    assert!(err.contains("must be > 0"), "err={err}");
    assert_eq!(rate_of(&conn, "dun-peas"), before);
}

#[test]
fn pass16_update_crop_seed_rate_rejects_negative() {
    let conn = mem();
    let before = rate_of(&conn, "dun-peas");
    let err = trays::update_crop_seed_rate(&conn, "dun-peas", Some(-1.0)).unwrap_err();
    assert!(err.contains("must be > 0"), "err={err}");
    assert_eq!(rate_of(&conn, "dun-peas"), before);
}

#[test]
fn pass16_update_crop_seed_rate_rejects_nan() {
    let conn = mem();
    let before = rate_of(&conn, "dun-peas");
    let err = trays::update_crop_seed_rate(&conn, "dun-peas", Some(f64::NAN)).unwrap_err();
    assert!(err.contains("finite"), "err={err}");
    assert_eq!(rate_of(&conn, "dun-peas"), before);
}

#[test]
fn pass16_update_crop_seed_rate_rejects_infinity() {
    let conn = mem();
    let before = rate_of(&conn, "dun-peas");
    let err = trays::update_crop_seed_rate(&conn, "dun-peas", Some(f64::INFINITY)).unwrap_err();
    assert!(err.contains("finite"), "err={err}");
    assert_eq!(rate_of(&conn, "dun-peas"), before);
}

#[test]
fn pass16_update_crop_seed_rate_rejects_unknown_crop() {
    let conn = mem();
    let err = trays::update_crop_seed_rate(&conn, "no-such-crop", Some(1.0)).unwrap_err();
    assert!(err.contains("unknown crop_id"), "err={err}");
}

/// SEALED-RECORD PROOF: rate edit cannot reach event_log or consumption_events.
#[test]
fn pass16_rate_edit_leaves_event_log_and_consumption_byte_identical() {
    let mut conn = mem();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 2, Some(16.0)).unwrap();

    let event_log_before = dump_event_log(&conn);
    let consumption_before = dump_consumption_events(&conn);
    assert!(
        !event_log_before.is_empty() && !consumption_before.is_empty(),
        "fixture must have sealed rows to compare"
    );

    trays::update_crop_seed_rate(&conn, "dun-peas", Some(9.0)).unwrap();
    assert_eq!(rate_of(&conn, "dun-peas"), Some(9.0));

    let event_log_after = dump_event_log(&conn);
    let consumption_after = dump_consumption_events(&conn);
    assert_eq!(
        event_log_after, event_log_before,
        "rate edit must not alter event_log"
    );
    assert_eq!(
        consumption_after, consumption_before,
        "rate edit must not alter consumption_events"
    );
}

/// H1 — harvest emits one planting consumption with §3 spine shape.
#[test]
fn h1_harvest_emits_one_planting_consumption() {
    let mut conn = mem();
    let tray = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    trays::advance_tray(&mut conn, &tray.id).unwrap();
    let before = consumption_rows(&conn).len();

    trays::harvest_groups(
        &mut conn,
        &[HarvestInput {
            tray_ids: vec![tray.id.clone()],
            actual_yield_oz: 18.5,
        }],
    )
    .unwrap();

    let after = consumption_rows(&conn);
    assert_eq!(after.len(), before + 1, "exactly one new consumption row");
    let planting = after
        .iter()
        .find(|r| r.2 == UNIT_PLANTING)
        .expect("planting consumption row");
    assert_eq!(planting.1, "Dun peas");
    assert_eq!(planting.2, UNIT_PLANTING);
    assert_eq!(planting.3, 2.0);
    assert_eq!(planting.4, "farm_os");

    let (origin, domain, class, kind): (String, String, Option<String>, String) = conn
        .query_row(
            "SELECT origin, event_domain, event_class, kind FROM event_log WHERE id = ?1",
            [&planting.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(origin, "farm_os");
    assert_eq!(domain, "register");
    assert_eq!(class.as_deref(), Some("physical_consumption"));
    assert_eq!(kind, Kind::ConsumptionPhysical.as_str());

    let (tier_d, tier_c) = Kind::ConsumptionPhysical.tier();
    assert_eq!(tier_d, EventDomain::Register);
    assert_eq!(tier_c, Some(EventClass::PhysicalConsumption));
}

/// H2 — harvest planting sow_event_id joins tray.sown and the sow tray unit row.
#[test]
fn h2_harvest_sow_event_id_joins() {
    let mut conn = mem();
    let tray = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    trays::advance_tray(&mut conn, &tray.id).unwrap();
    trays::harvest_groups(
        &mut conn,
        &[HarvestInput {
            tray_ids: vec![tray.id.clone()],
            actual_yield_oz: 18.5,
        }],
    )
    .unwrap();

    let tray_sown_id: String = conn
        .query_row(
            "SELECT id FROM event_log
             WHERE kind = 'tray.sown' AND entity_id = ?1
             ORDER BY seq ASC LIMIT 1",
            [&tray.id],
            |r| r.get(0),
        )
        .unwrap();
    let sow_tray_sow_event_id: String = conn
        .query_row(
            "SELECT sow_event_id FROM consumption_events
             WHERE unit = ?1 AND sow_event_id IS NOT NULL
             ORDER BY event_id ASC LIMIT 1",
            [UNIT_TRAY],
            |r| r.get(0),
        )
        .unwrap();
    let planting_sow_event_id: String = conn
        .query_row(
            "SELECT sow_event_id FROM consumption_events WHERE unit = ?1",
            [UNIT_PLANTING],
            |r| r.get(0),
        )
        .unwrap();

    assert_eq!(planting_sow_event_id, tray_sown_id);
    assert_eq!(planting_sow_event_id, sow_tray_sow_event_id);
}

/// H3 — multi-sow harvest group: per-row sowEventId, no cross-attribution.
#[test]
fn h3_multi_sow_group_no_cross_attribution() {
    let mut conn = mem();
    let a = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    let b = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    trays::advance_tray(&mut conn, &a.id).unwrap();
    trays::advance_tray(&mut conn, &b.id).unwrap();

    trays::harvest_groups(
        &mut conn,
        &[HarvestInput {
            tray_ids: vec![a.id.clone(), b.id.clone()],
            actual_yield_oz: 20.0,
        }],
    )
    .unwrap();

    let plantings: Vec<(String, Option<String>, f64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT e.id, c.sow_event_id, c.quantity
                 FROM consumption_events c
                 JOIN event_log e ON e.id = c.event_id
                 WHERE c.unit = ?1
                 ORDER BY e.seq ASC",
            )
            .unwrap();
        stmt.query_map([UNIT_PLANTING], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|x| x.unwrap())
            .collect()
    };
    assert_eq!(plantings.len(), 2);

    let sown_a: String = conn
        .query_row(
            "SELECT id FROM event_log WHERE kind = 'tray.sown' AND entity_id = ?1",
            [&a.id],
            |r| r.get(0),
        )
        .unwrap();
    let sown_b: String = conn
        .query_row(
            "SELECT id FROM event_log WHERE kind = 'tray.sown' AND entity_id = ?1",
            [&b.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(sown_a, sown_b);

    let ids: HashSet<String> = plantings
        .iter()
        .map(|p| p.1.clone().expect("sow_event_id present"))
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&sown_a));
    assert!(ids.contains(&sown_b));

    let by_sow: HashMap<String, f64> = plantings
        .into_iter()
        .map(|(_, sow, qty)| (sow.unwrap(), qty))
        .collect();
    assert_eq!(by_sow[&sown_a], 2.0);
    assert_eq!(by_sow[&sown_b], 3.0);
}

/// H4 — tray without tray.sown: planting written, sow_event_id NULL, quantity real.
#[test]
fn h4_unknown_sow_event_id_is_null_not_zero() {
    let mut conn = mem();
    let seeded = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    trays::advance_tray(&mut conn, &seeded.id).unwrap();
    trays::apply_recount(
        &mut conn,
        &[RecountEntry {
            crop_id: "dun-peas".into(),
            counted_quantity: 5, // +3 surplus row, no tray.sown
        }],
    )
    .unwrap();
    let surplus_id: String = conn
        .query_row(
            "SELECT id FROM trays
             WHERE crop_id = 'dun-peas' AND state = 'light' AND id <> ?1",
            [&seeded.id],
            |r| r.get(0),
        )
        .unwrap();
    let surplus = trays::get_tray(&conn, &surplus_id).unwrap();
    assert_eq!(surplus.quantity, 3);
    let sown_for_surplus: Option<String> = conn
        .query_row(
            "SELECT id FROM event_log
             WHERE kind = 'tray.sown' AND entity_id = ?1
             ORDER BY seq ASC LIMIT 1",
            [&surplus_id],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    assert!(sown_for_surplus.is_none());

    trays::harvest_groups(
        &mut conn,
        &[HarvestInput {
            tray_ids: vec![surplus_id.clone()],
            actual_yield_oz: 10.0,
        }],
    )
    .unwrap();

    let (qty, sow_event_id): (f64, Option<String>) = conn
        .query_row(
            "SELECT quantity, sow_event_id FROM consumption_events WHERE unit = ?1",
            [UNIT_PLANTING],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(qty, 3.0);
    assert!(sow_event_id.is_none());
    assert!(
        (qty - 0.0).abs() > f64::EPSILON,
        "must not write quantity 0.0"
    );
}

/// H5 — harvest must not inflate the unit='tray' denominator.
#[test]
fn h5_harvest_does_not_change_tray_unit_denominator() {
    let mut conn = mem();
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    trays::advance_tray(&mut conn, &tray.id).unwrap();
    trays::harvest_groups(
        &mut conn,
        &[HarvestInput {
            tray_ids: vec![tray.id.clone()],
            actual_yield_oz: 12.0,
        }],
    )
    .unwrap();

    let tray_sum: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(quantity),0) FROM consumption_events WHERE unit = 'tray'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tray_sum, 3.0);
}

/// H6 — undo harvest restores tray; consumption rows stay permanent.
#[test]
fn h6_undo_harvest_keeps_consumption_records() {
    let mut conn = mem();
    let tray = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    trays::advance_tray(&mut conn, &tray.id).unwrap();
    trays::harvest_groups(
        &mut conn,
        &[HarvestInput {
            tray_ids: vec![tray.id.clone()],
            actual_yield_oz: 18.5,
        }],
    )
    .unwrap();

    let before = dump_consumption_events(&conn);
    assert!(
        before.iter().any(|r| r.contains("|planting|")),
        "fixture must include planting consumption"
    );

    let undone = trays::undo_last(&mut conn).unwrap().expect("undo target");
    assert_eq!(undone.undone_kind, "trays.harvested");

    let restored = trays::get_tray(&conn, &tray.id).unwrap();
    assert_eq!(restored.state, "light");
    assert!(restored.harvested_on.is_none());
    assert!(restored.actual_yield_oz.is_none());

    let after = dump_consumption_events(&conn);
    assert_eq!(after, before, "consumption rows must be byte-identical");
    let neg_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM consumption_events WHERE quantity <= 0.0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(neg_count, 0, "no compensating or negative consumption row");
}

/// H7 — harvest planting survives flush + verify_replay (payload self-sufficient).
#[test]
fn h7_harvest_spine_round_trip_verify_replay() {
    let dir = tempfile_dir("h7-spine");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();

    let tray = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    trays::advance_tray(&mut conn, &tray.id).unwrap();
    trays::harvest_groups(
        &mut conn,
        &[HarvestInput {
            tray_ids: vec![tray.id.clone()],
            actual_yield_oz: 18.5,
        }],
    )
    .unwrap();
    crate::event_file::try_flush_after_commit(&conn, &dir);
    drop(conn);

    let jsonl = dir.join("events.jsonl");
    let outcome = projection::verify_replay_paths(&farm, &jsonl, &dir).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify_replay failed: {}",
        outcome.summary_line()
    );
    assert_eq!(outcome.report().flush_lag, 0);
    assert!(outcome.report().unknown_diffs.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

/// H8 — actualYieldOz never reaches planting quantity; single clock read.
#[test]
fn h8_yield_cannot_reach_planting_quantity() {
    fn harvest_planting_qty_and_times(yield_oz: f64) -> (f64, String, String) {
        let mut conn = mem();
        let tray = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        trays::advance_tray(&mut conn, &tray.id).unwrap();
        trays::harvest_groups(
            &mut conn,
            &[HarvestInput {
                tray_ids: vec![tray.id.clone()],
                actual_yield_oz: yield_oz,
            }],
        )
        .unwrap();
        let qty: f64 = conn
            .query_row(
                "SELECT quantity FROM consumption_events WHERE unit = ?1",
                [UNIT_PLANTING],
                |r| r.get(0),
            )
            .unwrap();
        let planting_occurred: String = conn
            .query_row(
                "SELECT occurred_at FROM consumption_events WHERE unit = ?1",
                [UNIT_PLANTING],
                |r| r.get(0),
            )
            .unwrap();
        let harvest_created: String = conn
            .query_row(
                "SELECT created_at FROM event_log WHERE kind = 'trays.harvested'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        (qty, planting_occurred, harvest_created)
    }

    let (q_a, occ_a, created_a) = harvest_planting_qty_and_times(18.5);
    let (q_b, occ_b, created_b) = harvest_planting_qty_and_times(3.0);
    assert_eq!(q_a, q_b);
    assert_eq!(q_a, 2.0);
    assert_eq!(occ_a, created_a);
    assert_eq!(occ_b, created_b);
}
