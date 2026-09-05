//! Bounce — signed `wholesale.payment_reversed`. One money act, one transaction.
//! R1/R2 stay on the operator doors.

use crate::db;
use crate::events::{self, EventRecord, Kind};
use crate::income;
use crate::marketing;
use crate::projection;
use crate::reachability;
use crate::trays;
use crate::wholesale::{self, OrderLine, PaymentReversedPayload, WriteOffInput};
use rusqlite::Connection;
use serde_json::json;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn today() -> String {
    db::local_date_today()
}

fn paid_order(conn: &mut Connection) -> (String, String, i64) {
    let v = marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(conn, "dun-peas", 6).unwrap();
    let d = today();
    let order = wholesale::record_order(
        conn,
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
    wholesale::deliver_order(conn, &order.id, None).unwrap();
    let paid = wholesale::pay_order(conn, &order.id, 1600, &d, None, true, false, None).unwrap();
    let income_id = paid.income_event_id.clone().expect("income_event_id");
    (order.id, income_id, 1600)
}

fn paid_order_short(conn: &mut Connection) -> (String, String) {
    let v = marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(conn, "dun-peas", 6).unwrap();
    let d = today();
    let order = wholesale::record_order(
        conn,
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
    wholesale::deliver_order(conn, &order.id, None).unwrap();
    let paid = wholesale::pay_order(
        conn,
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
    let income_id = paid.income_event_id.clone().expect("income_event_id");
    (order.id, income_id)
}

fn order_row(conn: &Connection, order_id: &str) -> (String, Option<String>, Option<String>) {
    conn.query_row(
        "SELECT state, paid_on, income_event_id FROM wholesale_orders WHERE id = ?1",
        [order_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

fn live_income(conn: &Connection, income_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM income_events WHERE income_id = ?1 AND voided_at IS NULL",
        [income_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn cash_in(conn: &Connection) -> i64 {
    income::list_income(conn)
        .unwrap()
        .into_iter()
        .map(|r| r.amount_cents)
        .sum()
}

fn r1(venue: &str, paid_on: &str) -> String {
    let d = reachability::format_mon_d_local(paid_on).unwrap();
    format!(
        "That payment is the record of a paid order - {venue}, {d}. It cannot be voided while the order stands paid."
    )
}

fn kind_count(conn: &Connection, kind: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM event_log WHERE kind = ?1",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

fn dual_truth(conn: &Connection) -> (i64, i64) {
    let paid_without_income: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wholesale_orders o
             WHERE o.state = 'paid'
               AND (
                 o.income_event_id IS NULL
                 OR NOT EXISTS (
                   SELECT 1 FROM income_events i
                   WHERE i.income_id = o.income_event_id AND i.voided_at IS NULL
                 )
               )",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let income_without_paid: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM income_events i
             WHERE i.voided_at IS NULL
               AND EXISTS (
                 SELECT 1 FROM wholesale_orders o
                 WHERE o.income_event_id = i.income_id AND o.state <> 'paid'
               )",
            [],
            |r| r.get(0),
        )
        .unwrap();
    (paid_without_income, income_without_paid)
}

#[test]
fn reverse_from_paid_returns_delivered_voids_income_and_drops_cash_in() {
    let mut conn = mem();
    let (order_id, income_id, amount) = paid_order(&mut conn);
    let before = cash_in(&conn);
    assert!(before >= amount);

    let view = wholesale::reverse_payment(&mut conn, &order_id, "").unwrap();
    assert_eq!(view.state, "delivered");
    let (state, paid_on, pointed) = order_row(&conn, &order_id);
    assert_eq!(state, "delivered");
    assert!(paid_on.is_none(), "paid_on must be cleared");
    assert!(pointed.is_none(), "income_event_id must be cleared");
    assert_eq!(live_income(&conn, &income_id), 0);
    assert_eq!(cash_in(&conn), before - amount);
    assert_eq!(kind_count(&conn, "wholesale.payment_reversed"), 1);
    assert_eq!(kind_count(&conn, "income.voided"), 1);
}

#[test]
fn reverse_refuses_from_ordered_delivered_and_voided() {
    let mut conn = mem();
    let v =
        marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
    let d = today();
    let ordered = wholesale::record_order(
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
    let err = wholesale::reverse_payment(&mut conn, &ordered.id, "").unwrap_err();
    assert!(err.contains("ordered") && err.contains("not paid"), "{err}");

    let delivered = wholesale::deliver_order(&mut conn, &ordered.id, None).unwrap();
    let err = wholesale::reverse_payment(&mut conn, &delivered.id, "").unwrap_err();
    assert!(
        err.contains("delivered") && err.contains("not paid"),
        "{err}"
    );

    let other = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    wholesale::void_order(&mut conn, &other.id, Some("changed mind".into())).unwrap();
    let err = wholesale::reverse_payment(&mut conn, &other.id, "").unwrap_err();
    assert!(err.contains("voided") && err.contains("not paid"), "{err}");
}

#[test]
fn r1_intact_void_income_on_paid_linked_row_still_refuses() {
    let mut conn = mem();
    let (_order_id, income_id, _) = paid_order(&mut conn);
    let (venue, paid_on): (String, String) = conn
        .query_row(
            "SELECT v.name, o.paid_on
             FROM wholesale_orders o
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE o.income_event_id = ?1",
            [&income_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let err = income::void_income(&mut conn, &income_id).unwrap_err();
    assert_eq!(err, r1(&venue, &paid_on));
    assert_eq!(live_income(&conn, &income_id), 1);
}

#[test]
fn reentrancy_reverse_then_pay_again_reaches_paid() {
    let mut conn = mem();
    let (order_id, first_income, _) = paid_order(&mut conn);
    wholesale::reverse_payment(&mut conn, &order_id, "").unwrap();
    let (state, _, _) = order_row(&conn, &order_id);
    assert_eq!(state, "delivered");
    assert_eq!(live_income(&conn, &first_income), 0);

    let again = wholesale::pay_order(
        &mut conn,
        &order_id,
        1600,
        &today(),
        None,
        true,
        false,
        None,
    )
    .unwrap();
    assert_eq!(again.state, "paid");
    let second = again.income_event_id.expect("second income");
    assert_ne!(second, first_income);
    assert_eq!(live_income(&conn, &second), 1);
    let (state, paid_on, pointed) = order_row(&conn, &order_id);
    assert_eq!(state, "paid");
    assert!(paid_on.is_some());
    assert_eq!(pointed.as_deref(), Some(second.as_str()));
}

#[test]
fn historic_paid_with_no_reversal_event_stays_paid() {
    let mut conn = mem();
    let (order_id, income_id, _) = paid_order(&mut conn);
    assert_eq!(kind_count(&conn, "wholesale.payment_reversed"), 0);
    let (state, paid_on, pointed) = order_row(&conn, &order_id);
    assert_eq!(state, "paid");
    assert!(paid_on.is_some());
    assert_eq!(pointed.as_deref(), Some(income_id.as_str()));
    assert_eq!(live_income(&conn, &income_id), 1);
}

#[test]
fn replay_new_kind_applies_through_projection() {
    let mut conn = mem();
    let (order_id, income_id, _) = paid_order(&mut conn);
    let payload = PaymentReversedPayload {
        order_id: order_id.clone(),
        income_event_id: income_id.clone(),
        reason: None,
    };
    let event = EventRecord::originated(
        Kind::WholesalePaymentReversed,
        "wholesale_order",
        order_id.clone(),
        serde_json::to_value(&payload).unwrap(),
        json!({ "op": "none" }),
        projection::handler_now(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &event).unwrap();
    tx.commit().unwrap();
    let (state, paid_on, pointed) = order_row(&conn, &order_id);
    assert_eq!(state, "delivered");
    assert!(paid_on.is_none());
    assert!(pointed.is_none());
}

#[test]
fn undo_last_does_not_reverse_a_payment_reversal() {
    let mut conn = mem();
    let (order_id, income_id, _) = paid_order(&mut conn);
    wholesale::reverse_payment(&mut conn, &order_id, "").unwrap();

    let newest = events::newest_undoable(&conn).unwrap();
    if let Some(e) = &newest {
        assert_ne!(e.kind, "wholesale.payment_reversed");
        assert_ne!(e.kind, "income.voided");
    }
    let _ = trays::undo_last(&mut conn).unwrap();
    let (state, paid_on, pointed) = order_row(&conn, &order_id);
    assert_eq!(state, "delivered");
    assert!(paid_on.is_none());
    assert!(pointed.is_none());
    assert_eq!(live_income(&conn, &income_id), 0);
    assert_eq!(kind_count(&conn, "wholesale.payment_reversed"), 1);
}

#[test]
fn no_dual_truth_after_reversal() {
    let mut conn = mem();
    let (order_id, income_id, _) = paid_order(&mut conn);
    wholesale::reverse_payment(&mut conn, &order_id, "").unwrap();
    let (paid_without_income, income_without_paid) = dual_truth(&conn);
    assert_eq!(
        paid_without_income, 0,
        "order must not read paid with income gone"
    );
    assert_eq!(
        income_without_paid, 0,
        "income must not stand with no paid order"
    );
    assert_eq!(order_row(&conn, &order_id).0, "delivered");
    assert_eq!(live_income(&conn, &income_id), 0);
}

#[test]
fn payment_reversed_payload_field_names_pinned() {
    assert_eq!(
        crate::wholesale::PAYMENT_REVERSED_PAYLOAD_FIELD_NAMES,
        &["order_id", "income_event_id", "reason"]
    );
}

#[test]
fn wa_allowance_counts_while_the_order_stands_paid() {
    let mut conn = mem();
    let (_order_id, _income_id) = paid_order_short(&mut conn);
    let summary = wholesale::write_off_summary(&conn).unwrap();
    assert_eq!(summary.total_shortfall_cents, 400);
    let sales = summary
        .by_category
        .iter()
        .find(|r| r.category == "sales_discount")
        .expect("sales_discount row");
    assert_eq!(sales.count, 1);
}

#[test]
fn wa_reversed_payment_drops_the_allowance_from_the_summary() {
    let mut conn = mem();
    let (order_id, _income_id) = paid_order_short(&mut conn);
    wholesale::reverse_payment(&mut conn, &order_id, "").unwrap();
    let summary = wholesale::write_off_summary(&conn).unwrap();
    assert_eq!(summary.total_shortfall_cents, 0);
    for row in &summary.by_category {
        assert_eq!(row.count, 0);
        assert_eq!(row.total_cents, 0);
    }
    let table_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM wholesale_write_offs", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(table_count, 1);
    assert_eq!(kind_count(&conn, "wholesale.write_off"), 1);
}

#[test]
fn wa_bad_debt_after_reversal_keeps_the_allowance() {
    let mut conn = mem();
    let (order_id, _income_id) = paid_order_short(&mut conn);
    wholesale::reverse_payment(&mut conn, &order_id, "").unwrap();
    let confirm = wholesale::bad_debt_confirm_line(&conn, &order_id)
        .unwrap()
        .expect("eligible after reverse");
    let view = wholesale::write_off_bad_debt(&mut conn, &order_id, &confirm).unwrap();
    assert_eq!(view.state, "written_off");
    let summary = wholesale::write_off_summary(&conn).unwrap();
    assert_eq!(summary.total_shortfall_cents, 400);
}
