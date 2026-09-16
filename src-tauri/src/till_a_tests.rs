//! TILL-A (GT-D22) — mint → show → close.

use crate::db;
use crate::event_partition::{self, EventClass, EventDomain, REGISTER_KINDS};
use crate::events::Kind;
use crate::marketing;
use crate::money::{self, fake::FakeGateway, PaidLine, PaidSession, RefundRecord};
use crate::poll;
use crate::wholesale::{self, OrderLine, WholesaleOrderView};
use rusqlite::Connection;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn venue(conn: &mut Connection) -> marketing::VenueView {
    marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap()
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

fn session(
    _order_id: &str,
    amount: i64,
    reference: Option<&str>,
    plink: Option<&str>,
) -> PaidSession {
    PaidSession {
        session_id: "cs_till_a_1".into(),
        payment_intent: Some("pi_till_a_1".into()),
        lines: vec![PaidLine {
            price_id: "price_fake_bill".into(),
            quantity: 1,
            amount_cents: amount,
        }],
        currency: "usd".into(),
        customer_email: None,
        paid_at: db::utc_now_rfc3339(),
        created: 1_700_000_000,
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
fn till_a_mint_refuses_an_unpriced_order() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let harvest = db::local_date_today();
    let id = crate::wholesale_tests::inherited_unpriced_order(
        &mut conn,
        &v.venue_id,
        &harvest,
        "kale",
        1,
    );
    wholesale::deliver_order(&mut conn, &id, None).unwrap();
    let gw = FakeGateway::new();
    let err = wholesale::mint_payment_link_with(&mut conn, &gw, &id).unwrap_err();
    assert_eq!(err, wholesale::UNPRICED_SETTLEMENT_LINE);
    assert!(gw.state.lock().unwrap().order_links_created.is_empty());
    assert_eq!(count_kind(&conn, "wholesale.link_minted"), 0);
}

#[test]
fn till_a_mint_refuses_before_delivery() {
    let mut conn = mem();
    let order = priced_ordered(&mut conn);
    let gw = FakeGateway::new();
    let err = wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap_err();
    assert_eq!(err, wholesale::MINT_NOT_DELIVERED_LINE);
    assert!(gw.state.lock().unwrap().order_links_created.is_empty());
    assert_eq!(count_kind(&conn, "wholesale.link_minted"), 0);
}

#[test]
fn till_a_second_mint_is_refused_and_makes_no_second_stripe_object() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    let minted = wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let id = &order.id;
    assert_eq!(
        minted.payment_link_id.as_deref(),
        Some(format!("plink_fake_{id}").as_str())
    );
    assert_eq!(
        minted.payment_link_url.as_deref(),
        Some(format!("https://buy.stripe.com/test/wo/{id}?client_reference_id=wo-{id}").as_str())
    );
    assert!(minted.payment_link_minted_at.is_some());
    let err = wholesale::mint_payment_link_with(&mut conn, &gw, id).unwrap_err();
    assert_eq!(err, wholesale::MINT_ALREADY_LINKED_LINE);
    assert_eq!(gw.state.lock().unwrap().order_links_created.len(), 1);
    assert_eq!(count_kind(&conn, "wholesale.link_minted"), 1);
    assert_eq!(minted.state, "delivered");
    let again = wholesale::get_order(&conn, id).unwrap();
    assert_eq!(again.state, "delivered");
}

#[test]
fn till_a_poll_attaches_a_link_payment_to_the_wholesale_row_without_a_retail_order() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let reference = format!("wo-{}", order.id);
    let plink = format!("plink_fake_{}", order.id);
    gw.push_session(session(&order.id, 600, Some(&reference), Some(&plink)));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_applied, 1);
    assert_eq!(r.sessions_rejected, 0);
    let paid = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(paid.state, "paid");
    assert!(paid.income_event_id.is_some());
    let income_n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM income_events WHERE amount_cents = 600",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(income_n, 1);
    assert_eq!(count_table(&conn, "orders"), 0);
    let paid_n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log
             WHERE kind = 'wholesale.paid'
               AND json_extract(payload, '$.stripeSessionId') = ?1",
            ["cs_till_a_1"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(paid_n, 1);
    assert_eq!(count_table(&conn, "stripe_unapplied_facts"), 0);

    let r2 = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r2.ok, "{:?}", r2.error);
    assert_eq!(r2.sessions_applied, 0);
    let income_n2: i64 = conn
        .query_row("SELECT COUNT(*) FROM income_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(income_n2, 1);
    assert_eq!(count_kind(&conn, "wholesale.paid"), 1);
}

#[test]
fn till_a_poll_matches_by_payment_link_when_the_reference_is_missing() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let plink = format!("plink_fake_{}", order.id);
    gw.push_session(session(&order.id, 600, None, Some(&plink)));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_applied, 1);
    assert_eq!(r.sessions_rejected, 0);
    let paid = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(paid.state, "paid");
    assert!(paid.income_event_id.is_some());
    let income_n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM income_events WHERE amount_cents = 600",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(income_n, 1);
    assert_eq!(count_table(&conn, "orders"), 0);
    let paid_n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log
             WHERE kind = 'wholesale.paid'
               AND json_extract(payload, '$.stripeSessionId') = ?1",
            ["cs_till_a_1"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(paid_n, 1);
    assert_eq!(count_table(&conn, "stripe_unapplied_facts"), 0);

    let r2 = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r2.ok, "{:?}", r2.error);
    assert_eq!(r2.sessions_applied, 0);
    let income_n2: i64 = conn
        .query_row("SELECT COUNT(*) FROM income_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(income_n2, 1);
    assert_eq!(count_kind(&conn, "wholesale.paid"), 1);
}

#[test]
fn till_a_poll_names_a_link_payment_before_delivery_and_books_no_cash() {
    let mut conn = mem();
    let order = priced_ordered(&mut conn);
    let gw = FakeGateway::new();
    let reference = format!("wo-{}", order.id);
    gw.push_session(session(&order.id, 600, Some(&reference), None));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_rejected, 1);
    let still = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(still.state, "ordered");
    assert_eq!(count_table(&conn, "income_events"), 0);
    assert_eq!(count_table(&conn, "orders"), 0);
    assert_eq!(count_facts(&conn, "wholesale_not_delivered"), 1);
    let r2 = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r2.ok, "{:?}", r2.error);
    assert_eq!(count_facts(&conn, "wholesale_not_delivered"), 1);
}

#[test]
fn till_a_poll_names_a_wrong_amount_and_books_no_cash() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let reference = format!("wo-{}", order.id);
    let plink = format!("plink_fake_{}", order.id);
    gw.push_session(session(&order.id, 599, Some(&reference), Some(&plink)));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(count_facts(&conn, "wholesale_amount_mismatch"), 1);
    let still = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(still.state, "delivered");
    assert_eq!(count_table(&conn, "income_events"), 0);
    assert_eq!(count_table(&conn, "orders"), 0);
}

#[test]
fn till_a_refund_on_a_link_payment_is_named_and_the_walk_advances() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let reference = format!("wo-{}", order.id);
    let plink = format!("plink_fake_{}", order.id);
    gw.push_session(session(&order.id, 600, Some(&reference), Some(&plink)));
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_applied, 1);
    gw.push_refund(RefundRecord {
        refund_id: "re_till_a".into(),
        payment_intent: Some("pi_till_a_1".into()),
        session_id: None,
        created: 1_700_000_100,
        amount_cents: Some(600),
        currency: Some("usd".into()),
        status: Some("succeeded".into()),
    });
    let r2 = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r2.ok, "{:?}", r2.error);
    assert_eq!(r2.refunds_applied, 0);
    let paid = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(paid.state, "paid");
    assert_eq!(count_table(&conn, "income_events"), 1);
    let fact_n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM stripe_unapplied_facts
             WHERE stripe_object = 'refund' AND status = 'wholesale_payment'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fact_n, 1);
    let refunds_since: String = conn
        .query_row(
            "SELECT refunds_since FROM stripe_cursor WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(refunds_since, "1700000100");
}

#[test]
fn till_a_live_key_is_accepted() {
    assert_eq!(
        money::validate_restricted_key("rk_live_abc").unwrap(),
        "live"
    );
}

#[test]
fn till_a_kind_set_is_fifty() {
    assert_eq!(Kind::ALL.len(), 54);
    assert_eq!(
        Kind::parse("wholesale.link_minted"),
        Ok(Kind::WholesaleLinkMinted)
    );
    assert_eq!(Kind::WholesaleLinkMinted.as_str(), "wholesale.link_minted");
    assert_eq!(
        Kind::parse(Kind::WholesaleLinkMinted.as_str()),
        Ok(Kind::WholesaleLinkMinted)
    );
    assert_eq!(
        Kind::WholesaleLinkMinted.tier(),
        (EventDomain::Register, Some(EventClass::SaleFarmOsPath))
    );
    assert!(REGISTER_KINDS.contains(&"wholesale.link_minted"));
    assert!(event_partition::register_kinds().contains(&"wholesale.link_minted"));
}
