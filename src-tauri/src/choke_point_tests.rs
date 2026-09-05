//! Track 1 Phase 5 — typed choke point tests.
//!
//! Behavioural assertions in the pre-existing suite are not edited here.

use crate::db;
use crate::event_partition::{
    grow_kinds, register_kinds, schema_v9_event_log_triggers_sql, EventClass, EventDomain, Kind,
    EVENT_CLASSES, GROW_KINDS, REGISTER_KINDS,
};
use crate::events::{self, EventRecord};
use crate::money;
use crate::projection;
use crate::trays;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn event_log_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap()
}

/// 1. Every Kind maps to exactly one (domain, class); mapping is total over the enum.
#[test]
fn kind_tier_is_total_over_full_enum() {
    for kind in Kind::ALL {
        let (domain, class) = kind.tier();
        match domain {
            EventDomain::Grow => assert!(class.is_none(), "{:?} grow must have NULL class", kind),
            EventDomain::Marketing => {
                assert!(class.is_none(), "{:?} marketing must have NULL class", kind)
            }
            EventDomain::Register => {
                assert!(class.is_some(), "{:?} register must have a class", kind);
                let c = class.unwrap();
                assert!(
                    EventClass::ALL.contains(&c),
                    "{:?} class {:?} outside the seven",
                    kind,
                    c
                );
            }
        }
        // Exactly one pair: calling twice is identical.
        assert_eq!(kind.tier(), (domain, class));
    }
    // Inventory count tracks the closed set size (silent-shortfall residual).
    assert_eq!(Kind::ALL.len(), 54);
}

/// 7. Adding a Kind variant without a partition mapping fails to COMPILE —
///    proven by the exhaustive `match` in `Kind::tier`. This test keeps the
///    runtime surface honest: every ALL entry is exercised.
#[test]
fn kind_tier_exhaustive_match_covers_all_variants() {
    let _ = Kind::ALL.map(Kind::tier);
}

/// Const string tables stay aligned with the enum-derived lists (flush guard).
#[test]
fn partition_const_tables_match_kind_tier() {
    assert_eq!(grow_kinds(), GROW_KINDS.to_vec());
    assert_eq!(register_kinds(), REGISTER_KINDS.to_vec());
    assert_eq!(
        crate::event_partition::marketing_kinds(),
        crate::event_partition::MARKETING_KINDS.to_vec()
    );
    let class_strs: Vec<_> = EventClass::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(class_strs, EVENT_CLASSES.to_vec());
}

/// Commercial-app classes are not EventClass variants.
#[test]
fn commercial_classes_are_unrepresentable() {
    for s in [
        "commercial_order",
        "commercial_payment",
        "commercial_stock_movement",
        "commercial_expense",
    ] {
        assert!(EventClass::parse(s).is_err(), "{s}");
        assert!(!EVENT_CLASSES.contains(&s), "{s}");
    }
}

/// 2. Paid session through the Kind signature lands as register/sale_farm_os_path.
#[test]
fn paid_session_via_kind_lands_register_sale_farm_os_path() {
    let mut conn = mem();
    let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
    let hd = trays::get_tray(&conn, &t.id)
        .unwrap()
        .expected_harvest_date
        .unwrap();
    let price_id = format!("price_{hd}_dun-peas");
    conn.execute(
        "INSERT INTO offers
         (id, harvest_date, crop_id, price_cents, stripe_price_id,
          stripe_link_id, stripe_link_url, created_at)
         VALUES ('off_choke', ?1, 'dun-peas', 1200, ?2, NULL, NULL, '2026-08-06T00:00:00.000Z')",
        rusqlite::params![&hd, &price_id],
    )
    .unwrap();
    let session = money::PaidSession {
        session_id: "cs_choke_kind".into(),
        payment_intent: Some("pi_choke_kind".into()),
        lines: vec![money::PaidLine {
            price_id,
            quantity: 1,
            amount_cents: 1200,
        }],
        currency: "cad".into(),
        customer_email: None,
        paid_at: "2026-08-06T12:00:00.000Z".into(),
        created: 1,
        amount_cents: 1200,
        client_reference: None,
        payment_link: None,
    };
    money::apply_paid_session(&mut conn, &session).unwrap();
    let (domain, class): (String, Option<String>) = conn
        .query_row(
            "SELECT event_domain, event_class FROM event_log
             WHERE kind = 'stripe.session_paid' ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(domain, "register");
    assert_eq!(class.as_deref(), Some("sale_farm_os_path"));
    // Kind map is the authority — same pair as the write path used.
    assert_eq!(
        Kind::StripeSessionPaid.tier(),
        (EventDomain::Register, Some(EventClass::SaleFarmOsPath))
    );
}

/// 3. try_write_event_kind with each commercial string returns Err; delta 0.
#[test]
fn try_write_event_kind_refuses_commercial_strings_zero_delta() {
    let mut conn = mem();
    let before = event_log_count(&conn);
    let payload = json!({});
    let inverse = json!({ "op": "none" });
    for s in [
        "commercial_order",
        "commercial_payment",
        "commercial_stock_movement",
        "commercial_expense",
    ] {
        let tx = conn.transaction().unwrap();
        let err = events::try_write_event_kind(
            &tx,
            s,
            "x",
            "y",
            &payload,
            &inverse,
            "2026-08-06T00:00:00.000Z",
            None,
            None,
        )
        .unwrap_err();
        assert!(
            err.contains("unknown event kind"),
            "expected kind parse error for {s}: {err}"
        );
        tx.commit().unwrap();
        assert_eq!(event_log_count(&conn) - before, 0, "{s}");
    }
    assert_eq!(event_log_count(&conn), before);
}

/// write_event derives tier from Kind even if EventRecord fields lie.
#[test]
fn write_event_ignores_caller_domain_class_fields() {
    let mut conn = mem();
    let mut event = EventRecord::originated(
        Kind::StripeSessionPaid,
        "stripe_session",
        "cs_lie",
        json!({
            "orderId": "ord_lie",
            "cropId": "dun-peas",
            "quantity": 1,
            "amountCents": 100,
            "sessionId": "cs_lie",
            "paymentIntent": "pi_lie",
            "harvestDate": "2099-01-01",
            "currency": "cad",
            "customerEmail": null,
            "paidAt": "2026-08-06T00:00:00.000Z"
        }),
        json!({ "op": "none" }),
        "2026-08-06T00:00:00.000Z",
        None,
        None,
        Some("ev-lie".into()),
    );
    // Attempt the Phase 1 mislabel — must not stick.
    event.event_domain = "grow".into();
    event.event_class = None;
    {
        let tx = conn.transaction().unwrap();
        projection::apply_event(&tx, &event).unwrap();
        events::write_event(&tx, &event).unwrap();
        tx.commit().unwrap();
    }
    let (domain, class): (String, Option<String>) = conn
        .query_row(
            "SELECT event_domain, event_class FROM event_log WHERE id = 'ev-lie'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(domain, "register");
    assert_eq!(class.as_deref(), Some("sale_farm_os_path"));
}

/// 6. Handler event and rows carry byte-identical timestamps.
#[test]
fn sow_handler_event_and_tray_timestamps_byte_identical() {
    let mut conn = mem();
    let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let (tray_created, tray_updated, event_created): (String, String, String) = conn
        .query_row(
            "SELECT t.created_at, t.updated_at, e.created_at
             FROM trays t
             JOIN event_log e ON e.entity_id = t.id AND e.kind = 'tray.sown'
             WHERE t.id = ?1",
            [&t.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(tray_created, event_created);
    assert_eq!(tray_updated, event_created);
}

/// 8. Trigger SQL generated from the enum matches what migrations installed.
#[test]
fn trigger_sql_from_enum_matches_installed_migration_sql() {
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
    for kind in grow_kinds() {
        let needle = format!("'{kind}'");
        assert!(
            generated.contains(&needle),
            "grow kind missing from generated SQL: {kind}"
        );
        assert!(
            installed.contains(&needle),
            "grow kind missing from installed trigger: {kind}"
        );
    }
    for kind in register_kinds() {
        let needle = format!("'{kind}'");
        assert!(generated.contains(&needle), "register kind missing: {kind}");
        assert!(
            installed.contains(&needle),
            "register kind missing from installed trigger: {kind}"
        );
    }
    for class in EVENT_CLASSES {
        let needle = format!("'{class}'");
        assert!(generated.contains(&needle), "class missing: {class}");
        assert!(
            installed.contains(&needle),
            "class missing from installed trigger: {class}"
        );
    }
    for s in [
        "commercial_order",
        "commercial_payment",
        "commercial_stock_movement",
        "commercial_expense",
    ] {
        assert!(!generated.contains(s), "{s} in generated trigger SQL");
        assert!(!installed.contains(s), "{s} in installed trigger SQL");
    }
}

/// 4. Commercial class strings appear only in trigger SQL and tests.
///    Trigger SQL has none (see above); production .rs (non-test) must have none.
#[test]
fn commercial_class_strings_only_in_tests() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden = [
        "commercial_order",
        "commercial_payment",
        "commercial_stock_movement",
        "commercial_expense",
    ];
    let mut offenders = Vec::new();
    walk_rs(&root, &mut |path, src| {
        if is_test_source(path, src) {
            return;
        }
        for s in forbidden {
            if src.contains(s) {
                offenders.push(format!("{}: {s}", path.display()));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "commercial class strings outside tests:\n{}",
        offenders.join("\n")
    );
}

/// 5. Clock calls appear only at handler entry points and in tests —
///    never in apply_*, the write path, or event_partition.
#[test]
fn clock_calls_absent_from_write_path_apply_and_partition() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let needles = ["Utc::now", "Local::now", "SystemTime::now"];
    let always_forbidden = [
        root.join("event_partition.rs"),
        root.join("events.rs"),
        root.join("projection").join("kind.rs"),
    ];
    for path in &always_forbidden {
        let src = fs::read_to_string(path).unwrap();
        for n in needles {
            assert!(!src.contains(n), "{} must not contain {n}", path.display());
        }
    }
    // apply_* bodies: scan for `fn apply_` … next top-level fn; no clock needles.
    for rel in [
        "trays.rs",
        "money.rs",
        "attention.rs",
        "costs.rs",
        "mileage.rs",
        "assets.rs",
        "income.rs",
    ] {
        let path = root.join(rel);
        let src = fs::read_to_string(&path).unwrap();
        for block in apply_fn_bodies(&src) {
            for n in needles {
                assert!(
                    !block.contains(n),
                    "{rel} apply_* body contains {n}:\n{block}"
                );
            }
        }
    }
}

#[test]
fn verify_replay_cli_names_unexpected_argument() {
    let msg = crate::verify_replay_cli_error(Some("--ledger"));
    assert!(msg.contains("unexpected argument: --ledger"), "{msg}");
    assert!(
        msg.contains("Usage: verify_replay <farm-directory>"),
        "{msg}"
    );
}

fn is_test_source(path: &Path, src: &str) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name == "lib.rs" {
        // Only the tests module in lib.rs may mention commercial strings.
        return true;
    }
    if name.ends_with("_tests.rs") || name == "choke_point_tests.rs" {
        return true;
    }
    src.contains("#[cfg(test)]")
        && path.components().any(|c| {
            c.as_os_str()
                .to_str()
                .map(|s| s == "tests")
                .unwrap_or(false)
        })
}

fn walk_rs(dir: &Path, f: &mut dyn FnMut(&Path, &str)) {
    let entries = fs::read_dir(dir).unwrap();
    for entry in entries {
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

fn apply_fn_bodies(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_start();
        if line.starts_with("fn apply_")
            || line.starts_with("pub fn apply_")
            || line.starts_with("pub(crate) fn apply_")
        {
            let start = i;
            // Find body open.
            while i < lines.len() && !lines[i].contains('{') {
                i += 1;
            }
            if i >= lines.len() {
                break;
            }
            let mut depth = 0usize;
            let mut end = i;
            while end < lines.len() {
                for ch in lines[end].chars() {
                    if ch == '{' {
                        depth += 1;
                    } else if ch == '}' {
                        depth = depth.saturating_sub(1);
                    }
                }
                if depth == 0 && end > i {
                    break;
                }
                end += 1;
            }
            out.push(lines[start..=end.min(lines.len() - 1)].join("\n"));
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}
