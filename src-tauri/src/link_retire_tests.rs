//! J1 LINK-RETIRE — deactivate the parent's Payment Link after its local close.

use crate::db;
use crate::income::{self, RecordIncomeInput};
use crate::leftover;
use crate::money::{self, fake::FakeGateway, OrderBill, PaidSession, StripeGateway};
use crate::poll;
use crate::stripe_client::{fake_http::FakeHttp, StripeClient};
use crate::trays;
use crate::wholesale::{self, OrderLine, WholesaleOrderView};
use rusqlite::Connection;
use serde_json::json;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn venue(conn: &mut Connection) -> crate::marketing::VenueView {
    crate::marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap()
}

fn priced_ordered(conn: &mut Connection) -> WholesaleOrderView {
    let v = venue(conn);
    let harvest = db::local_date_today();
    wholesale::record_order(
        conn,
        &v.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 1,
            price_cents_per_tray: Some(600),
        }],
        false,
    )
    .unwrap()
}

fn priced_delivered(conn: &mut Connection) -> WholesaleOrderView {
    let order = priced_ordered(conn);
    wholesale::deliver_order(conn, &order.id, None).unwrap()
}

fn listed_kale(conn: &mut Connection) -> leftover::LeftoverListingView {
    let mut ids = Vec::new();
    for _ in 0..2 {
        let t = trays::sow_tray(conn, "kale", 1).unwrap();
        trays::advance_tray(conn, &t.id).unwrap();
        ids.push(t.id);
    }
    trays::harvest_trays(conn, &ids, 6.0).unwrap();
    leftover::list_leftover(conn, "kale", &db::local_date_today(), 2.0).unwrap()
}

fn link_session(
    session_id: &str,
    created: i64,
    amount: i64,
    reference: Option<&str>,
    plink: Option<&str>,
) -> PaidSession {
    PaidSession {
        session_id: session_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        lines: Vec::new(),
        currency: "usd".into(),
        customer_email: None,
        paid_at: db::utc_now_rfc3339(),
        created,
        amount_cents: amount,
        client_reference: reference.map(|s| s.to_string()),
        payment_link: plink.map(|s| s.to_string()),
    }
}

fn count_table(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn count_kind(conn: &Connection, kind: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM event_log WHERE kind = ?1",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

fn count_facts(conn: &Connection, status: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM stripe_unapplied_facts WHERE status = ?1",
        [status],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn j1_wholesale_link_pay_retires_the_link_after_the_local_close() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let reference = wholesale::client_reference_for(&order.id);
    let plink = format!("plink_fake_{}", order.id);
    gw.push_session(link_session(
        "cs_j1_wo_1",
        1_700_000_000,
        600,
        Some(&reference),
        Some(&plink),
    ));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_applied, 1);
    let paid = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(paid.state, "paid");
    assert_eq!(count_table(&conn, "income_events"), 1);
    assert_eq!(gw.state.lock().unwrap().deactivated_links, vec![plink]);
}

#[test]
fn j1_wholesale_second_session_on_a_spent_link_stays_already_settled_and_keeps_the_money() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let reference = wholesale::client_reference_for(&order.id);
    let plink = format!("plink_fake_{}", order.id);
    gw.push_session(link_session(
        "cs_j1_wo_1",
        1_700_000_000,
        600,
        Some(&reference),
        Some(&plink),
    ));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_applied, 1);
    gw.push_session(link_session(
        "cs_j1_wo_2",
        1_700_000_001,
        600,
        Some(&reference),
        Some(&plink),
    ));
    let r2 = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r2.ok, "{:?}", r2.error);
    assert_eq!(r2.sessions_rejected, 1);
    assert_eq!(count_facts(&conn, "wholesale_already_settled"), 1);
    assert_eq!(count_table(&conn, "income_events"), 1);
    assert_eq!(count_kind(&conn, "wholesale.paid"), 1);
    assert_eq!(gw.state.lock().unwrap().deactivated_links.len(), 1);
}

#[test]
fn j1_wholesale_cash_paid_retires_the_link() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let today = db::local_date_today();
    let view =
        wholesale::pay_order(&mut conn, &order.id, 600, &today, None, false, false, None).unwrap();
    wholesale::retire_order_link(&gw, &view);
    assert_eq!(view.state, "paid");
    let plink = format!("plink_fake_{}", order.id);
    assert_eq!(gw.state.lock().unwrap().deactivated_links, vec![plink]);
}

#[test]
fn j1_wholesale_apply_income_retires_the_link() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let today = db::local_date_today();
    let income = income::record_income(
        &mut conn,
        std::env::temp_dir().as_path(),
        RecordIncomeInput {
            amount_cents: 600,
            source: order.venue_name.clone(),
            category_id: "produce_you_grew".into(),
            date_received: today,
            descriptor: None,
            receipt_source_path: None,
        },
        false,
    )
    .unwrap();
    let view =
        wholesale::settle_order_with_income(&mut conn, &order.id, &income.income_id, false, None)
            .unwrap();
    wholesale::retire_order_link(&gw, &view);
    assert_eq!(view.state, "paid");
    let plink = format!("plink_fake_{}", order.id);
    assert_eq!(gw.state.lock().unwrap().deactivated_links, vec![plink]);
}

#[test]
fn j1_wholesale_void_retires_the_link() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let view = wholesale::void_order(&mut conn, &order.id, None).unwrap();
    wholesale::retire_order_link(&gw, &view);
    assert_eq!(view.state, "voided");
    let plink = format!("plink_fake_{}", order.id);
    assert_eq!(gw.state.lock().unwrap().deactivated_links, vec![plink]);
}

#[test]
fn j1_wholesale_never_minted_order_retires_nothing_and_no_key_is_silent() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    let today = db::local_date_today();
    let view =
        wholesale::pay_order(&mut conn, &order.id, 600, &today, None, false, false, None).unwrap();
    wholesale::retire_order_link(&gw, &view);
    assert!(gw.state.lock().unwrap().deactivated_links.is_empty());

    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let view = wholesale::void_order(&mut conn, &order.id, None).unwrap();
    wholesale::retire_order_link_from_db(&conn, &view);
    assert_eq!(view.state, "voided");
    assert_eq!(count_kind(&conn, "wholesale.voided"), 1);
}

#[test]
fn j1_wholesale_stripe_error_on_retire_never_rolls_back_the_local_close() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    gw.fail_deactivate("stripe down");
    let reference = wholesale::client_reference_for(&order.id);
    let plink = format!("plink_fake_{}", order.id);
    gw.push_session(link_session(
        "cs_j1_wo_err",
        1_700_000_000,
        600,
        Some(&reference),
        Some(&plink),
    ));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_applied, 1);
    let paid = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(paid.state, "paid");
    assert_eq!(count_table(&conn, "income_events"), 1);
    assert_eq!(count_kind(&conn, "wholesale.paid"), 1);
    assert!(gw.state.lock().unwrap().deactivated_links.is_empty());
}

#[test]
fn j1_leftover_link_pay_retires_the_link_after_the_local_close() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let reference = leftover::client_reference_for_listing(&listing.listing_id);
    let plink = format!("plink_fake_{}", listing.listing_id);
    gw.push_session(link_session(
        "cs_j1_lo_1",
        1_700_000_000,
        700,
        Some(&reference),
        Some(&plink),
    ));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_applied, 1);
    assert!(leftover::listings(&conn).unwrap()[0].paid_at.is_some());
    assert_eq!(gw.state.lock().unwrap().deactivated_links, vec![plink]);
}

#[test]
fn j1_leftover_cash_paid_retires_the_link() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let today = db::local_date_today();
    let view =
        leftover::pay_listing_cash(&mut conn, &listing.listing_id, 700, &today, None).unwrap();
    leftover::retire_listing_link(&gw, &view);
    assert!(view.paid_at.is_some());
    let plink = format!("plink_fake_{}", listing.listing_id);
    assert_eq!(gw.state.lock().unwrap().deactivated_links, vec![plink]);
}

#[test]
fn j1_leftover_late_session_after_cash_stays_already_paid_and_keeps_the_money() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let today = db::local_date_today();
    let view =
        leftover::pay_listing_cash(&mut conn, &listing.listing_id, 700, &today, None).unwrap();
    leftover::retire_listing_link(&gw, &view);
    let reference = leftover::client_reference_for_listing(&listing.listing_id);
    let plink = format!("plink_fake_{}", listing.listing_id);
    gw.push_session(link_session(
        "cs_j1_lo_late",
        1_700_000_000,
        700,
        Some(&reference),
        Some(&plink),
    ));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_rejected, 1);
    assert_eq!(count_facts(&conn, "leftover_already_paid"), 1);
    assert_eq!(count_table(&conn, "income_events"), 1);
    assert_eq!(count_kind(&conn, "leftover.paid"), 1);
    assert_eq!(gw.state.lock().unwrap().deactivated_links.len(), 1);
}

#[test]
fn j1_leftover_never_minted_listing_retires_nothing_and_no_key_is_silent() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    let today = db::local_date_today();
    let view =
        leftover::pay_listing_cash(&mut conn, &listing.listing_id, 500, &today, None).unwrap();
    leftover::retire_listing_link(&gw, &view);
    assert!(gw.state.lock().unwrap().deactivated_links.is_empty());

    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let view =
        leftover::pay_listing_cash(&mut conn, &listing.listing_id, 700, &today, None).unwrap();
    leftover::retire_listing_link_from_db(&conn, &view);
    assert!(view.paid_at.is_some());
    assert_eq!(count_kind(&conn, "leftover.paid"), 1);
}

#[test]
fn j1_wholesale_mint_form_carries_completed_sessions_limit_one() {
    let http = FakeHttp::new();
    http.push_post("/v1/products", json!({"id": "prod_j1"}));
    http.push_post("/v1/prices", json!({"id": "price_j1"}));
    http.push_post(
        "/v1/payment_links",
        json!({"id": "plink_j1", "url": "https://buy.stripe.com/test_j1"}),
    );
    let client = StripeClient::new(http, "test");
    let minted = client
        .create_order_payment_link(&OrderBill {
            order_id: "wo1".into(),
            venue_name: "Fixture Cafe".into(),
            harvest_date: "2026-09-06".into(),
            amount_cents: 600,
            client_reference: "wo-wo1".into(),
        })
        .unwrap();
    assert_eq!(minted.link_id, "plink_j1");
    let forms = client.http().post_forms.lock().unwrap();
    let link_form = forms
        .iter()
        .find(|(path, _)| path == "/v1/payment_links")
        .map(|(_, form)| form)
        .expect("payment_links form");
    assert!(link_form.contains(&("restrictions[completed_sessions][limit]".into(), "1".into())));
    assert!(link_form.contains(&("line_items[0][price]".into(), "price_j1".into())));
    assert!(link_form.contains(&("metadata[wholesale_order_id]".into(), "wo1".into())));
}

#[test]
fn j1_leftover_mint_form_carries_completed_sessions_limit_one() {
    let http = FakeHttp::new();
    http.push_post("/v1/products", json!({"id": "prod_j1"}));
    http.push_post("/v1/prices", json!({"id": "price_j1"}));
    http.push_post(
        "/v1/payment_links",
        json!({"id": "plink_j1", "url": "https://buy.stripe.com/test_j1"}),
    );
    let client = StripeClient::new(http, "test");
    let minted = client
        .create_order_payment_link(&OrderBill {
            order_id: "lo1".into(),
            venue_name: "Fixture Cafe".into(),
            harvest_date: "2026-09-06".into(),
            amount_cents: 600,
            client_reference: "lo-lo1".into(),
        })
        .unwrap();
    assert_eq!(minted.link_id, "plink_j1");
    let forms = client.http().post_forms.lock().unwrap();
    let link_form = forms
        .iter()
        .find(|(path, _)| path == "/v1/payment_links")
        .map(|(_, form)| form)
        .expect("payment_links form");
    assert!(link_form.contains(&("restrictions[completed_sessions][limit]".into(), "1".into())));
    assert!(link_form.contains(&("metadata[leftover_listing_id]".into(), "lo1".into())));
}
