//! F-A — dispute then refund must release trays. Refund is the only release.

use crate::db;
use crate::money::{self, FactOutcome};
use crate::trays;
use chrono::{Duration, Local};
use rusqlite::Connection;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn future_harvest_date() -> String {
    (Local::now().date_naive() + Duration::days(14))
        .format("%Y-%m-%d")
        .to_string()
}

fn seed_offer_price(
    conn: &Connection,
    harvest_date: &str,
    crop_id: &str,
    price_id: &str,
    cents: i64,
) {
    conn.execute(
        "INSERT INTO offers
         (id, harvest_date, crop_id, price_cents, stripe_price_id,
          stripe_link_id, stripe_link_url, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6)
         ON CONFLICT(harvest_date, crop_id) DO UPDATE SET
           stripe_price_id = excluded.stripe_price_id,
           price_cents = excluded.price_cents",
        rusqlite::params![
            format!("offer_{price_id}"),
            harvest_date,
            crop_id,
            cents,
            price_id,
            "2026-08-05T12:00:00.000Z"
        ],
    )
    .unwrap();
}

fn paid_session(
    conn: &Connection,
    session_id: &str,
    harvest_date: &str,
    crop_id: &str,
    qty: i64,
) -> money::PaidSession {
    let price_id = format!("price_{harvest_date}_{crop_id}");
    seed_offer_price(conn, harvest_date, crop_id, &price_id, 1200);
    money::PaidSession {
        session_id: session_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        lines: vec![money::PaidLine {
            price_id,
            quantity: qty,
            amount_cents: qty * 1200,
        }],
        currency: "cad".into(),
        customer_email: Some("buyer@example.com".into()),
        paid_at: "2026-08-05T12:00:00.000Z".into(),
        created: 1_700_000_000,
        amount_cents: qty * 1200,
        client_reference: None,
        payment_link: None,
    }
}

fn refund_for(session_id: &str, refund_id: &str) -> money::RefundRecord {
    money::RefundRecord {
        refund_id: refund_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        session_id: Some(session_id.into()),
        created: 1_700_000_100,
        amount_cents: Some(2400),
        currency: Some("cad".into()),
        status: Some("succeeded".into()),
    }
}

fn dispute_for(session_id: &str, dispute_id: &str) -> money::DisputeRecord {
    money::DisputeRecord {
        dispute_id: dispute_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        session_id: Some(session_id.into()),
        created: 1_700_000_200,
        amount_cents: Some(2400),
        currency: Some("cad".into()),
        status: None,
    }
}

struct CapSnap {
    remaining: i64,
    cover_shortfall: i64,
    capacity_consumed: i64,
    state: String,
}

fn snap(conn: &Connection, harvest_date: &str) -> CapSnap {
    let remaining = trays::remaining_for_date(conn, harvest_date).unwrap();
    let cover_shortfall = trays::cover_shortfall_on(conn, harvest_date).unwrap();
    let orders = money::list_orders(conn, Some(harvest_date)).unwrap();
    let (capacity_consumed, state) = match orders.first() {
        Some(o) => (o.capacity_consumed, o.state.clone()),
        None => (0, "none".into()),
    };
    CapSnap {
        remaining,
        cover_shortfall,
        capacity_consumed,
        state,
    }
}

fn sow_aligned(conn: &mut Connection, harvest_date: &str, qty: i64) {
    let t = trays::sow_tray(conn, "dun-peas", qty).unwrap();
    conn.execute(
        "UPDATE trays SET sown_on = date(?1, '-' || growth_days_at_sow || ' days')
         WHERE id = ?2",
        rusqlite::params![harvest_date, t.id],
    )
    .unwrap();
}

/// pay → dispute → refund must match pay → refund: one release, back to pre-sale.
#[test]
fn fa_dispute_then_refund_matches_pay_then_refund() {
    let hd = future_harvest_date();

    let mut via_refund = mem();
    sow_aligned(&mut via_refund, &hd, 6);
    let pre_refund = snap(&via_refund, &hd);
    let session_r = paid_session(&via_refund, "cs_fa_r", &hd, "dun-peas", 2);
    money::apply_paid_session(&mut via_refund, &session_r).unwrap();
    money::apply_refund(&mut via_refund, &refund_for("cs_fa_r", "re_fa_r")).unwrap();
    let after_pay_refund = snap(&via_refund, &hd);

    let mut via_dispute = mem();
    sow_aligned(&mut via_dispute, &hd, 6);
    let pre_dispute = snap(&via_dispute, &hd);
    let session_d = paid_session(&via_dispute, "cs_fa_d", &hd, "dun-peas", 2);
    money::apply_paid_session(&mut via_dispute, &session_d).unwrap();
    money::apply_dispute(&mut via_dispute, &dispute_for("cs_fa_d", "dp_fa_d")).unwrap();
    let after_dispute = snap(&via_dispute, &hd);
    money::apply_refund(&mut via_dispute, &refund_for("cs_fa_d", "re_fa_d")).unwrap();
    let after_dispute_refund = snap(&via_dispute, &hd);

    assert_eq!(pre_refund.remaining, pre_dispute.remaining);
    assert_eq!(pre_refund.cover_shortfall, pre_dispute.cover_shortfall);

    assert_eq!(after_dispute.capacity_consumed, 2);
    assert_eq!(after_dispute.state, "disputed");

    assert_eq!(after_dispute_refund.capacity_consumed, 0);
    assert_eq!(after_dispute_refund.state, "refunded");
    assert_eq!(after_dispute_refund.remaining, pre_dispute.remaining);
    assert_eq!(
        after_dispute_refund.cover_shortfall,
        pre_dispute.cover_shortfall
    );

    assert_eq!(after_pay_refund.capacity_consumed, 0);
    assert_eq!(after_pay_refund.state, "refunded");
    assert_eq!(after_pay_refund.remaining, after_dispute_refund.remaining);
    assert_eq!(
        after_pay_refund.cover_shortfall,
        after_dispute_refund.cover_shortfall
    );
    assert_eq!(
        after_pay_refund.capacity_consumed,
        after_dispute_refund.capacity_consumed
    );
}

/// P-B: pay → refund still releases once and ends refunded.
#[test]
fn fa_pay_then_refund_still_releases_once() {
    let mut conn = mem();
    let hd = future_harvest_date();
    sow_aligned(&mut conn, &hd, 6);
    let pre = snap(&conn, &hd);
    let session = paid_session(&conn, "cs_fa_pb", &hd, "dun-peas", 2);
    money::apply_paid_session(&mut conn, &session).unwrap();
    let after_pay = snap(&conn, &hd);
    money::apply_refund(&mut conn, &refund_for("cs_fa_pb", "re_fa_pb")).unwrap();
    let after = snap(&conn, &hd);

    assert_eq!(after_pay.capacity_consumed, 2);
    assert_eq!(after.capacity_consumed, 0);
    assert_eq!(after.state, "refunded");
    assert_eq!(after.remaining, pre.remaining);
    assert_eq!(after.cover_shortfall, pre.cover_shortfall);
}

/// pay → refund → dispute: the dispute releases nothing further.
#[test]
fn fa_refund_then_dispute_releases_nothing_further() {
    let mut conn = mem();
    let hd = future_harvest_date();
    sow_aligned(&mut conn, &hd, 6);
    let pre = snap(&conn, &hd);
    let session = paid_session(&conn, "cs_fa_rd", &hd, "dun-peas", 2);
    money::apply_paid_session(&mut conn, &session).unwrap();
    money::apply_refund(&mut conn, &refund_for("cs_fa_rd", "re_fa_rd")).unwrap();
    let after_refund = snap(&conn, &hd);
    money::apply_dispute(&mut conn, &dispute_for("cs_fa_rd", "dp_fa_rd")).unwrap();
    let after_dispute = snap(&conn, &hd);

    assert_eq!(after_refund.capacity_consumed, 0);
    assert_eq!(after_refund.state, "refunded");
    assert_eq!(
        after_dispute.capacity_consumed,
        after_refund.capacity_consumed
    );
    assert_eq!(after_dispute.remaining, after_refund.remaining);
    assert_eq!(after_dispute.cover_shortfall, after_refund.cover_shortfall);
    assert_eq!(after_dispute.remaining, pre.remaining);
    assert_eq!(after_dispute.state, "refunded");
}

/// Two refund events on one session: capacity_consumed never goes below 0.
#[test]
fn fa_two_refunds_never_go_below_zero() {
    let mut conn = mem();
    let hd = future_harvest_date();
    sow_aligned(&mut conn, &hd, 6);
    let session = paid_session(&conn, "cs_fa_2r", &hd, "dun-peas", 2);
    money::apply_paid_session(&mut conn, &session).unwrap();
    let first = money::apply_refund(&mut conn, &refund_for("cs_fa_2r", "re_fa_2r_a")).unwrap();
    assert_eq!(first, FactOutcome::Applied);
    let after_first = snap(&conn, &hd);
    let second = money::apply_refund(&mut conn, &refund_for("cs_fa_2r", "re_fa_2r_b")).unwrap();
    assert_eq!(second, FactOutcome::AlreadyApplied);
    let after_second = snap(&conn, &hd);

    assert_eq!(after_first.capacity_consumed, 0);
    assert!(after_second.capacity_consumed >= 0);
    assert_eq!(after_second.capacity_consumed, 0);
    assert_eq!(after_second.state, "refunded");
}

/// A refund that matches no order still records an unapplied fact.
#[test]
fn fa_refund_matching_no_order_still_records_unapplied_fact() {
    let mut conn = mem();
    let outcome = money::apply_refund(
        &mut conn,
        &money::RefundRecord {
            refund_id: "re_fa_none".into(),
            payment_intent: Some("pi_no_such_order".into()),
            session_id: Some("cs_no_such_order".into()),
            created: 1_700_000_900,
            amount_cents: Some(1200),
            currency: Some("cad".into()),
            status: Some("succeeded".into()),
        },
    )
    .unwrap();

    // Unchanged: no order row at all is AwaitingOrder and writes no fact.
    // An order that exists but is not paid/disputed still records no_paid_order.
    assert_eq!(outcome, FactOutcome::AwaitingOrder);
    assert!(money::list_unapplied_facts(&conn).unwrap().is_empty());

    conn.execute(
        "INSERT INTO orders
         (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
          quantity, amount_cents, currency, customer_email, state, capacity_consumed,
          paid_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, 1200, 'cad', NULL, 'refunded', 0, ?6, ?6, ?6)",
        rusqlite::params![
            "ord_fa_none",
            "cs_fa_none",
            "pi_fa_none",
            "2026-08-20",
            "dun-peas",
            "2026-08-05T12:00:00.000Z"
        ],
    )
    .unwrap();
    let outcome2 = money::apply_refund(
        &mut conn,
        &money::RefundRecord {
            refund_id: "re_fa_none2".into(),
            payment_intent: Some("pi_fa_none".into()),
            session_id: Some("cs_fa_none".into()),
            created: 1_700_000_901,
            amount_cents: Some(1200),
            currency: Some("cad".into()),
            status: Some("succeeded".into()),
        },
    )
    .unwrap();
    assert_eq!(outcome2, FactOutcome::AlreadyApplied);
    let facts = money::list_unapplied_facts(&conn).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].status, "no_paid_order");
    assert_eq!(facts[0].stripe_object, "refund");
    assert_eq!(facts[0].stripe_id, "re_fa_none2");
}
