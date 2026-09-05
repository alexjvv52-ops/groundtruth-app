//! Fence III-c — D8 duplicate income warning + F-B unapplied read-side retirement.
//! Writer-door only. Replay of a historic income.received must still apply.

use crate::attention;
use crate::db;
use crate::events::{EventRecord, Kind};
use crate::income::{self, RecordIncomeInput};
use crate::marketing;
use crate::money;
use crate::projection;
use crate::reachability;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use chrono::{Duration, NaiveDate};
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

fn add_days(date: &str, days: i64) -> String {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
    (d + Duration::days(days)).format("%Y-%m-%d").to_string()
}

fn farm_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "farm-os-fence3c-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn s1(venue: &str, amount_cents: i64, date_received: &str) -> String {
    let amount = attention::dollars(amount_cents);
    let d = reachability::format_mon_d_local(date_received).unwrap();
    format!(
        "{venue} already has {amount} recorded on {d}. Recording this again makes two income rows for one payment."
    )
}

fn live_income_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM income_events WHERE voided_at IS NULL",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn live_for(conn: &Connection, source: &str, amount_cents: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM income_events
         WHERE voided_at IS NULL AND source = ?1 AND amount_cents = ?2",
        rusqlite::params![source, amount_cents],
        |r| r.get(0),
    )
    .unwrap()
}

fn basic(source: &str, amount_cents: i64, date_received: &str) -> RecordIncomeInput {
    RecordIncomeInput {
        amount_cents,
        source: source.into(),
        category_id: "produce_you_grew".into(),
        date_received: date_received.into(),
        descriptor: None,
        receipt_source_path: None,
    }
}

fn delivered_order(conn: &mut Connection, venue_name: &str, amount_cents: i64) -> String {
    let v = marketing::record_venue(conn, venue_name, "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(conn, "dun-peas", 6).unwrap();
    let d = today();
    let trays_n = 2;
    let price = amount_cents / trays_n;
    let order = wholesale::record_order(
        conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: trays_n,
            price_cents_per_tray: Some(price),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(conn, &order.id, None).unwrap();
    order.id
}

#[test]
fn second_row_same_venue_amount_3_days_later_refused_with_s1() {
    let dir = farm_dir("ack-false");
    let mut conn = mem();
    let first_day = add_days(&today(), -3);
    income::record_income(
        &mut conn,
        &dir,
        basic("Fixture Cafe", 1600, &first_day),
        false,
    )
    .unwrap();
    let err = income::record_income(
        &mut conn,
        &dir,
        basic("Fixture Cafe", 1600, &today()),
        false,
    )
    .unwrap_err();
    assert_eq!(err, s1("Fixture Cafe", 1600, &first_day));
    assert_eq!(live_income_count(&conn), 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn second_row_same_venue_amount_with_ack_writes_two_rows() {
    let dir = farm_dir("ack-true");
    let mut conn = mem();
    let first_day = add_days(&today(), -3);
    income::record_income(
        &mut conn,
        &dir,
        basic("Fixture Cafe", 1600, &first_day),
        false,
    )
    .unwrap();
    income::record_income(&mut conn, &dir, basic("Fixture Cafe", 1600, &today()), true).unwrap();
    assert_eq!(live_income_count(&conn), 2);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn same_venue_amount_8_days_later_writes_with_no_warning() {
    let dir = farm_dir("eight-days");
    let mut conn = mem();
    let first_day = add_days(&today(), -8);
    income::record_income(
        &mut conn,
        &dir,
        basic("Fixture Cafe", 1600, &first_day),
        false,
    )
    .unwrap();
    assert!(
        income::duplicate_income_warning(&conn, "Fixture Cafe", 1600, &today())
            .unwrap()
            .is_none()
    );
    income::record_income(
        &mut conn,
        &dir,
        basic("Fixture Cafe", 1600, &today()),
        false,
    )
    .unwrap();
    assert_eq!(live_income_count(&conn), 2);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn same_venue_different_amount_writes_with_no_warning() {
    let dir = farm_dir("diff-amount");
    let mut conn = mem();
    income::record_income(
        &mut conn,
        &dir,
        basic("Fixture Cafe", 1600, &today()),
        false,
    )
    .unwrap();
    assert!(
        income::duplicate_income_warning(&conn, "Fixture Cafe", 1700, &today())
            .unwrap()
            .is_none()
    );
    income::record_income(
        &mut conn,
        &dir,
        basic("Fixture Cafe", 1700, &today()),
        false,
    )
    .unwrap();
    assert_eq!(live_income_count(&conn), 2);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn p_c_money_came_in_then_pay_order_warns_instead_of_silently_doubling() {
    let dir = farm_dir("pc");
    let mut conn = mem();
    let order_id = delivered_order(&mut conn, "Fixture Cafe", 1600);
    income::record_income(
        &mut conn,
        &dir,
        basic("Fixture Cafe", 1600, &today()),
        false,
    )
    .unwrap();
    assert_eq!(live_for(&conn, "Fixture Cafe", 1600), 1);

    let preview = income::duplicate_income_warning(&conn, "Fixture Cafe", 1600, &today())
        .unwrap()
        .expect("preview must name the same bytes as the gate");
    assert_eq!(preview, s1("Fixture Cafe", 1600, &today()));

    let err = wholesale::pay_order(
        &mut conn,
        &order_id,
        1600,
        &today(),
        None,
        true,
        false,
        None,
    )
    .unwrap_err();
    assert_eq!(err, preview);
    assert_eq!(live_for(&conn, "Fixture Cafe", 1600), 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn historic_income_received_that_would_now_warn_still_replays() {
    let dir = farm_dir("replay");
    let mut conn = mem();
    income::record_income(
        &mut conn,
        &dir,
        basic("Fixture Cafe", 1600, &today()),
        false,
    )
    .unwrap();
    let event_id = "historic-income-received-dup";
    let event = EventRecord::originated(
        Kind::IncomeReceived,
        "income",
        event_id,
        json!({
            "eventId": event_id,
            "origin": "farm_os",
            "incomeId": event_id,
            "dateReceived": today(),
            "amountCents": 1600,
            "source": "Fixture Cafe",
            "canonicalCategory": "produce_you_grew",
            "scheduleFLine": "2",
            "scheduleCLine": "1",
            "descriptor": "",
        }),
        json!({ "op": "none" }),
        projection::handler_now(),
        None,
        None,
        Some(event_id.to_string()),
    );
    assert!(
        income::duplicate_income_warning(&conn, "Fixture Cafe", 1600, &today())
            .unwrap()
            .is_some(),
        "the writer door would warn; replay must still apply"
    );
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &event).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        live_income_count(&conn),
        2,
        "historic income.received must still insert through the projection"
    );
    let _ = fs::remove_dir_all(&dir);
}

fn unmatched_session(session_id: &str, amount_cents: i64) -> money::PaidSession {
    money::PaidSession {
        session_id: session_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        lines: vec![money::PaidLine {
            price_id: "price_unknown".into(),
            quantity: 1,
            amount_cents,
        }],
        currency: "cad".into(),
        customer_email: Some("buyer@example.com".into()),
        paid_at: "2026-08-05T12:00:00.000Z".into(),
        created: 800,
        amount_cents,
        client_reference: None,
        payment_link: None,
    }
}

fn seed_offer(conn: &Connection, harvest_date: &str, crop_id: &str, price_id: &str) {
    conn.execute(
        "INSERT INTO offers
         (id, harvest_date, crop_id, price_cents, stripe_price_id,
          stripe_link_id, stripe_link_url, created_at)
         VALUES (?1, ?2, ?3, 2400, ?4, NULL, NULL, ?5)
         ON CONFLICT(harvest_date, crop_id) DO UPDATE SET
           stripe_price_id = excluded.stripe_price_id",
        rusqlite::params![
            format!("offer_{price_id}"),
            harvest_date,
            crop_id,
            price_id,
            "2026-08-05T12:00:00.000Z"
        ],
    )
    .unwrap();
}

#[test]
fn f_b_applied_fact_leaves_the_list_and_the_trail_row() {
    let mut conn = mem();
    let session = unmatched_session("cs_later_applied", 2400);
    let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
    assert!(matches!(
        outcome,
        crate::models::AppliedOutcome::Rejected { .. }
    ));
    assert_eq!(money::list_unapplied_facts(&conn).unwrap().len(), 1);
    let table_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM stripe_unapplied_facts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(table_before, 1);

    let tray = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
    let hd = trays::get_tray(&conn, &tray.id)
        .unwrap()
        .expected_harvest_date
        .expect("expected harvest date");
    seed_offer(&conn, &hd, "dun-peas", "price_unknown");

    let applied = money::apply_paid_session(&mut conn, &session).unwrap();
    assert!(matches!(
        applied,
        crate::models::AppliedOutcome::Applied { .. }
    ));
    assert!(
        money::list_unapplied_facts(&conn).unwrap().is_empty(),
        "an applied session must not remain on the unapplied surface"
    );
    let table_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM stripe_unapplied_facts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        table_after, 1,
        "the trail row must still exist; retirement is read-side only"
    );
}

/// Retail writes no income row.
///
/// `export::write_income_csv` (export.rs:512) reads `income_events` at :549 and
/// `orders WHERE state='paid'` at :578 as two separate selects. The manifest note
/// (export.rs:330-333) says income.csv "carries both manually recorded income and
/// paid Stripe orders, distinguished by record_type, so no dollar is counted twice".
/// That is true only while a paid retail session writes nothing to `income_events`.
///
/// Fence III-c inventory A4 recorded export.rs:578 as a carry-forward — "do not
/// change this select" — with no live test behind that premise. This is that test.
/// It replaces discovery probe P-E (fence3_discovery_tests.rs), which asserted the
/// opposite and failed by design; that file is deleted in the same land.
#[test]
fn retail_paid_session_writes_no_income_events_row() {
    let mut conn = mem();
    assert_eq!(
        live_income_count(&conn),
        0,
        "fixture must start with no income rows"
    );

    let tray = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
    let hd = trays::get_tray(&conn, &tray.id)
        .unwrap()
        .expected_harvest_date
        .expect("expected harvest date");
    seed_offer(&conn, &hd, "dun-peas", "price_unknown");

    let session = unmatched_session("cs_retail_no_income", 2400);
    let applied = money::apply_paid_session(&mut conn, &session).unwrap();
    assert!(
        matches!(applied, crate::models::AppliedOutcome::Applied { .. }),
        "the retail session must apply, or this test proves nothing"
    );

    let paid_orders: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM orders WHERE state = 'paid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        paid_orders, 1,
        "the receipt must land as exactly one paid order row"
    );

    assert_eq!(
        live_income_count(&conn),
        0,
        "a retail paid session must write no income_events row: export.rs:578 is the \
         only retail path into income.csv, and the manifest note's \"no dollar is \
         counted twice\" depends on this"
    );
}
