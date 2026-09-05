//! LO-B-CASH (GT-D24-B) — the desk close: money that arrived outside Stripe.
//!
//! What is pinned: cash writes leftover.paid + income.received in one
//! transaction with NO session id and never an orders row; a never-minted
//! listing takes the typed dollars as its total; refusals fire verbatim and
//! write nothing; cash on a minted-but-unpaid link is allowed and a late
//! session lands as leftover_already_paid; the Stripe close still carries its
//! session; it all replays.
use crate::db;
use crate::event_file;
use crate::leftover::{
    self, LEFTOVER_ALREADY_PAID_LINE, LEFTOVER_CASH_UNPRICED_LINE, LEFTOVER_CLIENT_REFERENCE_PREFIX,
};
use crate::money::fake::FakeGateway;
use crate::money::{self, PaidSession};
use crate::projection;
use crate::trays;
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
fn mem() -> Connection {
    db::open_in_memory().unwrap()
}
fn today() -> String {
    db::local_date_today()
}
fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-lo-cash-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}
/// Two kale trays harvested today at 6.0 oz, listed at 2.0 oz.
fn listed_kale(conn: &mut Connection) -> leftover::LeftoverListingView {
    let mut ids = Vec::new();
    for _ in 0..2 {
        let t = trays::sow_tray(conn, "kale", 1).unwrap();
        trays::advance_tray(conn, &t.id).unwrap();
        ids.push(t.id);
    }
    trays::harvest_trays(conn, &ids, 6.0).unwrap();
    leftover::list_leftover(conn, "kale", &today(), 2.0).unwrap()
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
fn paid_session(reference: &str, amount_cents: i64, session_id: &str) -> PaidSession {
    PaidSession {
        session_id: session_id.to_string(),
        payment_intent: Some(format!("pi_{session_id}")),
        lines: Vec::new(),
        currency: "cad".to_string(),
        customer_email: None,
        paid_at: projection::handler_now(),
        created: 1,
        amount_cents,
        client_reference: Some(reference.to_string()),
        payment_link: None,
    }
}
#[test]
fn lo_cash_close_books_paid_and_income_with_no_session_and_no_orders_row() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    leftover::pay_listing_cash(
        &mut conn,
        &listing.listing_id,
        700,
        &today(),
        Some("cash at the stand".to_string()),
    )
    .unwrap();
    let paid = &leftover::listings(&conn).unwrap()[0];
    assert_eq!(paid.paid_at.as_deref(), Some(today().as_str()));
    assert!(paid.paid_session_id.is_none());
    assert_eq!(paid.priced_total_cents, Some(700));
    assert_eq!(count_kind(&conn, "leftover.paid"), 1);
    assert_eq!(count_kind(&conn, "income.received"), 1);
    let (source, category, amount, descriptor): (String, String, i64, String) = conn
        .query_row(
            "SELECT source, canonical_category, amount_cents, descriptor FROM income_events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(source, format!("Leftover Kale · {}", listing.harvested_on));
    assert_eq!(category, "produce_you_grew");
    assert_eq!(amount, 700);
    assert_eq!(descriptor, "cash at the stand");
    assert_eq!(count_table(&conn, "orders"), 0);
    assert_eq!(count_kind(&conn, "wholesale.paid"), 0);
}
#[test]
fn lo_cash_never_minted_listing_takes_typed_dollars_as_total() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    leftover::pay_listing_cash(&mut conn, &listing.listing_id, 500, &today(), None).unwrap();
    let paid = &leftover::listings(&conn).unwrap()[0];
    assert_eq!(paid.priced_total_cents, Some(500));
    assert_eq!(paid.paid_at.as_deref(), Some(today().as_str()));
    assert!(paid.paid_session_id.is_none());
    assert!(paid.payment_link_id.is_none());
    assert!(paid.payment_link_url.is_none());
}
#[test]
fn lo_cash_refusals_fire_verbatim_and_write_nothing() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    for bad in [0, -500] {
        let err = leftover::pay_listing_cash(&mut conn, &listing.listing_id, bad, &today(), None)
            .unwrap_err();
        assert_eq!(err, LEFTOVER_CASH_UNPRICED_LINE, "{bad}");
    }
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let err = leftover::pay_listing_cash(&mut conn, &listing.listing_id, 999, &today(), None)
        .unwrap_err();
    assert_eq!(err, "cash payment does not match the listing total");
    let missing =
        leftover::pay_listing_cash(&mut conn, "no-such-listing", 700, &today(), None).unwrap_err();
    assert!(missing.contains("not found"), "{missing}");
    assert_eq!(count_kind(&conn, "leftover.paid"), 0);
    assert_eq!(count_kind(&conn, "income.received"), 0);
}
#[test]
fn lo_cash_on_minted_unpaid_link_then_late_session_is_already_paid() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    leftover::pay_listing_cash(&mut conn, &listing.listing_id, 700, &today(), None).unwrap();
    let second = leftover::pay_listing_cash(&mut conn, &listing.listing_id, 700, &today(), None)
        .unwrap_err();
    assert_eq!(second, LEFTOVER_ALREADY_PAID_LINE);
    let reference = format!("{LEFTOVER_CLIENT_REFERENCE_PREFIX}{}", listing.listing_id);
    let session = paid_session(&reference, 700, "cs_lo_cash_late");
    let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
    assert!(matches!(
        outcome,
        crate::models::AppliedOutcome::Rejected { .. }
    ));
    let status: String = conn
        .query_row(
            "SELECT status FROM stripe_unapplied_facts WHERE stripe_id = 'cs_lo_cash_late'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "leftover_already_paid");
    assert_eq!(count_kind(&conn, "leftover.paid"), 1);
    assert_eq!(count_kind(&conn, "income.received"), 1);
}
#[test]
fn lo_cash_stripe_close_still_carries_its_session() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let now = projection::handler_now();
    leftover::pay_listing_from_link_session(
        &mut conn,
        &listing.listing_id,
        "cs_still_stripe",
        None,
        700,
        &now,
    )
    .unwrap();
    let paid = &leftover::listings(&conn).unwrap()[0];
    assert_eq!(paid.paid_session_id.as_deref(), Some("cs_still_stripe"));
    assert_eq!(paid.priced_total_cents, Some(700));
    let descriptor: String = conn
        .query_row("SELECT descriptor FROM income_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(descriptor, "Stripe payment link · cs_still_stripe");
}
#[test]
fn lo_cash_close_replays_and_verify_passes() {
    let dir = temp_dir("verify");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let listing = listed_kale(&mut conn);
    leftover::pay_listing_cash(
        &mut conn,
        &listing.listing_id,
        600,
        &today(),
        Some("check #112".to_string()),
    )
    .unwrap();
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);
    let outcome =
        projection::verify_replay_paths(&dir.join("farm.db"), &event_file::events_path(&dir), &dir)
            .unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify failed: {}",
        outcome.summary_line()
    );
    assert_eq!(outcome.report().flush_lag, 0);
    let _ = fs::remove_dir_all(&dir);
}
