//! B7 — undo refuses inert inverses instead of recording a lie (L9).

use crate::attention;
use crate::costs::{self, RecordCostInput};
use crate::db;
use crate::events;
use crate::marketing;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use chrono::Local;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("farm-os-b7-{}-{}", label, uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn basic_cost(amount_cents: i64) -> RecordCostInput {
    RecordCostInput {
        amount_cents,
        payee: "Local Grow Supply".into(),
        category_id: "growing_medium".into(),
        date_paid: today(),
        descriptor: None,
        receipt_source_path: None,
    }
}

fn snapshot_wholesale_order(conn: &Connection, order_id: &str) -> String {
    conn.query_row(
        "SELECT id, venue_id, harvest_date, state, ordered_on, delivered_on,
                paid_on, income_event_id, voided_at, void_reason, created_at, updated_at
         FROM wholesale_orders WHERE id = ?1",
        [order_id],
        |r| {
            Ok(format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, String>(11)?,
            ))
        },
    )
    .unwrap()
}

fn snapshot_stage(conn: &Connection, venue_id: &str) -> String {
    conn.query_row(
        "SELECT venue_id, stage, trays_week, varieties, variety_targets,
                changed_on, note, updated_at
         FROM mkt_stages WHERE venue_id = ?1",
        [venue_id],
        |r| {
            Ok(format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
            ))
        },
    )
    .unwrap()
}

fn event_seq(conn: &Connection, kind: &str) -> i64 {
    conn.query_row(
        "SELECT seq FROM event_log WHERE kind = ?1 ORDER BY seq DESC LIMIT 1",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

fn undone_at(conn: &Connection, seq: i64) -> Option<String> {
    conn.query_row(
        "SELECT undone_at FROM event_log WHERE seq = ?1",
        [seq],
        |r| r.get(0),
    )
    .unwrap()
}

fn undo_count_for(conn: &Connection, undoes_seq: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM event_log WHERE kind = 'undo' AND undoes_seq = ?1",
        [undoes_seq],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn b7t1_wholesale_delivery_is_not_consumed_by_undo() {
    let mut conn = mem();
    let venue =
        marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let harvest_date = db::local_date_today();
    trays::advance_trays(&mut conn, std::slice::from_ref(&tray.id)).unwrap();
    trays::harvest_tray(&mut conn, &tray.id, 30.0).unwrap();

    let order = wholesale::record_order(
        &mut conn,
        &venue.venue_id,
        &harvest_date,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    let delivered = wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    assert_eq!(delivered.state, "delivered");

    let delivery_seq = event_seq(&conn, "wholesale.delivered");
    let order_before = snapshot_wholesale_order(&conn, &order.id);

    let result = trays::undo_last(&mut conn)
        .unwrap()
        .expect("harvest undoes");
    assert_eq!(result.undone_kind, "trays.harvested");
    assert_ne!(result.undoes_seq, delivery_seq);

    let tray = trays::get_tray(&conn, &tray.id).unwrap();
    assert_eq!(tray.state, "light");
    assert!(tray.harvested_on.is_none());

    assert!(
        undone_at(&conn, delivery_seq).is_none(),
        "wholesale.delivered must keep undone_at IS NULL"
    );
    assert_eq!(
        undo_count_for(&conn, delivery_seq),
        0,
        "no undo event may name the delivery seq"
    );
    assert_eq!(
        snapshot_wholesale_order(&conn, &order.id),
        order_before,
        "delivery projection row must be unchanged"
    );
}

#[test]
fn b7t2_marketing_stage_change_is_not_consumed_by_undo() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let mut targets = BTreeMap::new();
    targets.insert("Dun peas".into(), 3);
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &db::local_date_today(),
        Some(3),
        Some(vec!["Dun peas".into()]),
        Some(targets),
        None,
    )
    .unwrap();

    let stage_seq = event_seq(&conn, "stage.changed");
    let stage_before = snapshot_stage(&conn, &venue.venue_id);

    let result = trays::undo_last(&mut conn).unwrap().expect("sow undoes");
    assert_eq!(result.undone_kind, "tray.sown");
    assert_eq!(trays::list_trays(&conn).unwrap().len(), 0);

    assert!(
        undone_at(&conn, stage_seq).is_none(),
        "stage.changed must keep undone_at IS NULL"
    );
    assert_eq!(
        undo_count_for(&conn, stage_seq),
        0,
        "no undo event may name the stage seq"
    );
    assert_eq!(
        snapshot_stage(&conn, &venue.venue_id),
        stage_before,
        "stage projection row must be unchanged"
    );
}

#[test]
fn b7t3_only_inert_events_means_nothing_to_undo() {
    let mut conn = mem();
    marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();

    let undone_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE undone_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let undo_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE kind = 'undo'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let log_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();

    assert!(matches!(trays::undo_last(&mut conn), Ok(None)));

    let undone_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE undone_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let undo_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE kind = 'undo'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let log_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();

    assert_eq!(undone_before, 0);
    assert_eq!(undone_after, 0, "refusal must write no undone_at");
    assert_eq!(undo_before, 0);
    assert_eq!(undo_after, 0, "refusal must append no undo event");
    assert_eq!(log_before, log_after, "the refusal writes nothing at all");
}

#[test]
fn b7t4_attention_dismissal_still_undoes() {
    let mut conn = mem();
    attention::raise(
        &conn,
        "b7_fixture",
        Some("crop"),
        Some("dun-peas"),
        "fixture attention",
        &["dismiss"],
    )
    .unwrap();
    let attn_id: String = conn
        .query_row(
            "SELECT id FROM attention WHERE kind = 'b7_fixture' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    attention::dismiss_attention(&mut conn, &attn_id).unwrap();

    let resolved_seq = event_seq(&conn, "attention.resolved");
    let result = trays::undo_last(&mut conn)
        .unwrap()
        .expect("attention.resolved must remain undoable");
    assert_eq!(
        result.undone_kind, "attention.resolved",
        "if this fails, REVERSED_BY_UNDO_PATH is wrong — do not widen the predicate"
    );
    assert_eq!(result.undoes_seq, resolved_seq);

    let resolved_at: Option<String> = conn
        .query_row(
            "SELECT resolved_at FROM attention WHERE id = ?1",
            [&attn_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        resolved_at.is_none(),
        "dismiss undo must reopen: resolved_at back to NULL"
    );
    assert!(
        undone_at(&conn, resolved_seq).is_some(),
        "undone_at IS set on the attention.resolved row"
    );
}

#[test]
fn b7t5_real_inverses_and_the_cost_exemption_are_unchanged() {
    let dir = tempfile_dir("cost-skip");
    let mut conn = mem();

    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let first = trays::undo_last(&mut conn).unwrap().expect("sow undoes");
    assert_eq!(first.undone_kind, "tray.sown");
    assert_eq!(trays::list_trays(&conn).unwrap().len(), 0);

    let cost = costs::record_cost(&mut conn, &dir, basic_cost(1800)).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();

    let cost_count_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM cost_events", [], |r| r.get(0))
        .unwrap();
    let cost_row_before: String = conn
        .query_row(
            "SELECT event_id, amount_cents, payee FROM cost_events WHERE event_id = ?1",
            [&cost.event_id],
            |r| {
                Ok(format!(
                    "{:?}|{:?}|{:?}",
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .unwrap();

    let result = trays::undo_last(&mut conn).unwrap().expect("sow undoes");
    assert_eq!(result.undone_kind, "tray.sown");
    assert_ne!(result.undone_kind, "cost.money_out");
    assert_eq!(trays::list_trays(&conn).unwrap().len(), 0);

    let cost_count_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM cost_events", [], |r| r.get(0))
        .unwrap();
    let cost_row_after: String = conn
        .query_row(
            "SELECT event_id, amount_cents, payee FROM cost_events WHERE event_id = ?1",
            [&cost.event_id],
            |r| {
                Ok(format!(
                    "{:?}|{:?}|{:?}",
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(cost_count_after, cost_count_before);
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

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn b7t6_reverses_something_table() {
    let cases: &[(&str, &str, bool)] = &[
        ("wholesale.delivered", r#"{"op":"none"}"#, false),
        ("mkt.stage_changed", r#"{"op":"none"}"#, false),
        ("attention.resolved", r#"{"op":"none"}"#, true),
        ("trays.sown", r#"{"op":"delete_tray","trayId":"x"}"#, true),
        ("trays.sown", "not json", false),
        ("trays.sown", r#"{}"#, false),
    ];
    for (kind, inverse, expected) in cases {
        assert_eq!(
            events::reverses_something(kind, inverse),
            *expected,
            "reverses_something({kind:?}, {inverse:?})"
        );
    }
}
