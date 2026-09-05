//! Fence III-b — event authority on order-produced income. D6 = refuse.
//! Writer-door only. Replay of a historic income.voided must still apply.

use crate::db;
use crate::events::{EventRecord, Kind};
use crate::income::{self, CorrectIncomeInput, RecordIncomeInput};
use crate::marketing;
use crate::projection;
use crate::reachability;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn today() -> String {
    db::local_date_today()
}

fn farm_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "farm-os-fence3b-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn paid_order_income(conn: &mut Connection) -> (String, String, String, String) {
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
    (order.id, income_id, v.name, d)
}

fn r1(venue: &str, paid_on: &str) -> String {
    let d = reachability::format_mon_d_local(paid_on).unwrap();
    format!(
        "That payment is the record of a paid order - {venue}, {d}. It cannot be voided while the order stands paid."
    )
}

fn r2(venue: &str, paid_on: &str) -> String {
    let d = reachability::format_mon_d_local(paid_on).unwrap();
    format!(
        "That payment is the record of a paid order - {venue}, {d}. Its amount cannot be corrected while the order stands paid."
    )
}

fn live_income_rows(conn: &Connection, income_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM income_events
         WHERE income_id = ?1 AND voided_at IS NULL",
        [income_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn order_state(conn: &Connection, order_id: &str) -> String {
    conn.query_row(
        "SELECT state FROM wholesale_orders WHERE id = ?1",
        [order_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn basic_income(amount_cents: i64) -> RecordIncomeInput {
    RecordIncomeInput {
        amount_cents,
        source: "Market cash".into(),
        category_id: "produce_you_grew".into(),
        date_received: today(),
        descriptor: None,
        receipt_source_path: None,
    }
}

#[test]
fn void_income_on_paid_order_row_is_refused_with_r1() {
    let mut conn = mem();
    let (_order_id, income_id, venue, paid_on) = paid_order_income(&mut conn);
    let err = income::void_income(&mut conn, &income_id).unwrap_err();
    assert_eq!(err, r1(&venue, &paid_on));
}

#[test]
fn correct_income_on_paid_order_row_is_refused_with_r2() {
    let dir = farm_dir("correct-paid");
    let mut conn = mem();
    let (_order_id, income_id, venue, paid_on) = paid_order_income(&mut conn);
    let err = income::correct_income(
        &mut conn,
        &dir,
        CorrectIncomeInput {
            income_id,
            amount_cents: 1700,
            source: "Fixture Cafe".into(),
            category_id: "produce_you_grew".into(),
            date_received: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap_err();
    assert_eq!(err, r2(&venue, &paid_on));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn p_d_dual_truth_cannot_be_produced_after_refusal() {
    let mut conn = mem();
    let (order_id, income_id, venue, paid_on) = paid_order_income(&mut conn);
    let err = income::void_income(&mut conn, &income_id).unwrap_err();
    assert_eq!(err, r1(&venue, &paid_on));
    assert_eq!(order_state(&conn, &order_id), "paid");
    assert!(
        live_income_rows(&conn, &income_id) > 0,
        "income row must still be live after the writer-door refusal"
    );
}

#[test]
fn void_income_on_voided_order_pointer_still_succeeds() {
    let mut conn = mem();
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
    wholesale::void_order(&mut conn, &order.id, Some("changed mind".into())).unwrap();
    assert_eq!(order_state(&conn, &order.id), "voided");

    let dir = farm_dir("voided-pointer");
    let income = income::record_income(&mut conn, &dir, basic_income(1600), false).unwrap();
    conn.execute(
        "UPDATE wholesale_orders SET income_event_id = ?1 WHERE id = ?2",
        rusqlite::params![&income.income_id, &order.id],
    )
    .unwrap();

    income::void_income(&mut conn, &income.income_id).unwrap();
    assert_eq!(live_income_rows(&conn, &income.income_id), 0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn void_income_on_free_standing_row_still_succeeds() {
    let dir = farm_dir("void-free");
    let mut conn = mem();
    let income = income::record_income(&mut conn, &dir, basic_income(900), false).unwrap();
    income::void_income(&mut conn, &income.income_id).unwrap();
    assert_eq!(live_income_rows(&conn, &income.income_id), 0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn correct_income_on_free_standing_row_still_succeeds() {
    let dir = farm_dir("correct-free");
    let mut conn = mem();
    let income = income::record_income(&mut conn, &dir, basic_income(900), false).unwrap();
    let corrected = income::correct_income(
        &mut conn,
        &dir,
        CorrectIncomeInput {
            income_id: income.income_id.clone(),
            amount_cents: 1100,
            source: "Market cash".into(),
            category_id: "produce_you_grew".into(),
            date_received: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    assert_eq!(corrected.amount_cents, 1100);
    assert_eq!(live_income_rows(&conn, &income.income_id), 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn historic_income_voided_on_paid_order_row_still_replays() {
    let mut conn = mem();
    let (_order_id, income_id, _venue, _paid_on) = paid_order_income(&mut conn);
    let last: String = conn
        .query_row(
            "SELECT last_event_id FROM income_events WHERE income_id = ?1",
            [&income_id],
            |r| r.get(0),
        )
        .unwrap();
    let event_id = "historic-income-voided";
    let event = EventRecord::originated(
        Kind::IncomeVoided,
        "income",
        income_id.clone(),
        json!({
            "eventId": event_id,
            "origin": "farm_os",
            "incomeId": income_id,
        }),
        json!({ "op": "none" }),
        projection::handler_now(),
        None,
        Some(&last),
        Some(event_id.to_string()),
    );
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &event).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        live_income_rows(&conn, &income_id),
        0,
        "historic income.voided must still mark the row voided through the projection"
    );
}
