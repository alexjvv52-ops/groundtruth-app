//! LO-B (GT-D24-B) — the listing's money face: mint at an operator price,
//! poll close into income.received.
//!
//! What is pinned: the closed set grew by leftover.link_minted and
//! leftover.paid (register tier); the four mint sentences fire in the signed
//! order unpriced, linked, paid, stale, then Stripe; the poll matches lo-
//! references to listings and never to the order books; the close writes
//! leftover.paid + income.received in one transaction; schema v40 freezes the
//! five LO-A columns and opens only the six money columns; it all replays.

use crate::db;
use crate::event_file;
use crate::event_partition::{self, EventClass, EventDomain, REGISTER_KINDS};
use crate::events::{self, EventRecord, Kind};
use crate::identity::is_farm_truth;
use crate::income;
use crate::leftover::{
    self, LeftoverLinkMintedPayload, LEFTOVER_ALREADY_LINKED_LINE, LEFTOVER_ALREADY_PAID_LINE,
    LEFTOVER_CLIENT_REFERENCE_PREFIX, LEFTOVER_LINK_MINTED_PAYLOAD_FIELD_NAMES,
    LEFTOVER_LISTINGS_COLUMNS, LEFTOVER_PAID_PAYLOAD_FIELD_NAMES, LEFTOVER_STALE_HARVEST_LINE,
    LEFTOVER_UNPRICED_LINE,
};
use crate::money::{self, fake::FakeGateway, PaidSession};
use crate::projection;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use rusqlite::Connection;
use serde_json::json;
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
    let dir = std::env::temp_dir().join(format!("groundtruth-lo-b-{label}-{stamp}"));
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

fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn paid_session(reference: &str, amount_cents: i64, session_id: &str) -> PaidSession {
    PaidSession {
        session_id: session_id.to_string(),
        payment_intent: Some(format!("pi_{session_id}")),
        lines: Vec::new(),
        currency: "usd".to_string(),
        customer_email: None,
        paid_at: projection::handler_now(),
        created: 1,
        amount_cents,
        client_reference: Some(reference.to_string()),
        payment_link: None,
    }
}

#[test]
fn lo_b_kind_set_is_fifty_three_and_both_kinds_are_register_sale_side() {
    assert_eq!(Kind::ALL.len(), 54);
    for (s, k) in [
        ("leftover.link_minted", Kind::LeftoverLinkMinted),
        ("leftover.paid", Kind::LeftoverPaid),
    ] {
        assert_eq!(Kind::parse(s), Ok(k));
        assert_eq!(k.as_str(), s);
        assert_eq!(
            k.tier(),
            (EventDomain::Register, Some(EventClass::SaleFarmOsPath))
        );
        assert!(REGISTER_KINDS.contains(&s));
        assert!(is_farm_truth(k));
    }
    assert_eq!(event_partition::register_kinds(), REGISTER_KINDS.to_vec());
    assert_eq!(
        LEFTOVER_LINK_MINTED_PAYLOAD_FIELD_NAMES,
        &[
            "listing_id",
            "payment_link_id",
            "payment_link_url",
            "client_reference",
            "priced_total_cents",
            "currency",
            "minted_on"
        ]
    );
    assert_eq!(
        LEFTOVER_PAID_PAYLOAD_FIELD_NAMES,
        &[
            "listing_id",
            "paid_at",
            "income_event_id",
            "stripe_session_id",
            "stripe_payment_intent",
            "priced_total_cents"
        ]
    );
}

#[test]
fn lo_b_schema_v40_has_the_six_columns_the_update_guard_and_the_delete_guard() {
    let mut conn = mem();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    assert_eq!(
        table_columns(&conn, "leftover_listings"),
        LEFTOVER_LISTINGS_COLUMNS
    );
    let listing = listed_kale(&mut conn);
    let frozen = conn
        .execute(
            "UPDATE leftover_listings SET listed_oz = 9.0 WHERE listing_id = ?1",
            [&listing.listing_id],
        )
        .unwrap_err()
        .to_string();
    assert!(frozen.contains("core columns are frozen"), "{frozen}");
    conn.execute(
        "UPDATE leftover_listings SET payment_link_url = 'x' WHERE listing_id = ?1",
        [&listing.listing_id],
    )
    .unwrap();
    let deleted = conn
        .execute("DELETE FROM leftover_listings", [])
        .unwrap_err()
        .to_string();
    assert!(deleted.contains("append-only"), "{deleted}");
    for trigger in [
        "leftover_listings_before_delete",
        "leftover_listings_before_update",
    ] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                [trigger],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{trigger}");
    }
}

#[test]
fn lo_b_unpriced_mint_is_sentence_1_and_writes_nothing() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    for bad in [0, -500] {
        let err =
            leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, bad).unwrap_err();
        assert_eq!(err, LEFTOVER_UNPRICED_LINE, "{bad}");
    }
    assert_eq!(count_kind(&conn, "leftover.link_minted"), 0);
    let again = leftover::listings(&conn).unwrap();
    assert!(again[0].payment_link_id.is_none());
}

#[test]
fn lo_b_mint_freezes_link_url_reference_and_price() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    let minted =
        leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let reference = format!("{LEFTOVER_CLIENT_REFERENCE_PREFIX}{}", listing.listing_id);
    assert_eq!(
        minted.payment_link_id.as_deref(),
        Some(format!("plink_fake_{}", listing.listing_id).as_str())
    );
    assert_eq!(
        minted.payment_link_url.as_deref(),
        Some(
            format!(
                "https://buy.stripe.com/test/wo/{}?client_reference_id={reference}",
                listing.listing_id
            )
            .as_str()
        )
    );
    assert!(minted.payment_link_minted_at.is_some());
    assert_eq!(minted.priced_total_cents, Some(700));
    assert!(minted.paid_at.is_none());
    assert_eq!(count_kind(&conn, "leftover.link_minted"), 1);
    // 1a: the bill's display slots are the crop name and the harvest day.
    let bills = gw.state.lock().unwrap().order_links_created.clone();
    assert_eq!(bills.len(), 1);
    assert_eq!(bills[0].venue_name, "Kale");
    assert_eq!(bills[0].harvest_date, listing.harvested_on);
    assert_eq!(bills[0].client_reference, reference);
    assert_eq!(bills[0].amount_cents, 700);
}

#[test]
fn lo_b_second_mint_is_sentence_2_and_paid_mint_is_sentence_3() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let err =
        leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap_err();
    assert_eq!(err, LEFTOVER_ALREADY_LINKED_LINE);
    let now = projection::handler_now();
    leftover::pay_listing_from_link_session(
        &mut conn,
        &listing.listing_id,
        "cs_lo_b_paid_1",
        None,
        700,
        &now,
    )
    .unwrap();
    let err =
        leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap_err();
    assert_eq!(err, LEFTOVER_ALREADY_PAID_LINE);
    assert_eq!(count_kind(&conn, "leftover.link_minted"), 1);
    assert_eq!(count_kind(&conn, "leftover.paid"), 1);
}

#[test]
fn lo_b_stale_mint_is_sentence_4_and_gate_order_holds() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    // Undo the harvest beneath the listing: LO-A carries the row.
    let undone = trays::undo_last(&mut conn).unwrap().expect("the harvest");
    assert_eq!(undone.undone_kind, "trays.harvested");
    let gw = FakeGateway::new();
    // Sentence 1 outranks stale.
    let err = leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 0).unwrap_err();
    assert_eq!(err, LEFTOVER_UNPRICED_LINE);
    // Priced but stale: sentence 4, and nothing reached Stripe.
    let err =
        leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap_err();
    assert_eq!(err, LEFTOVER_STALE_HARVEST_LINE);
    assert!(gw.state.lock().unwrap().order_links_created.is_empty());
    assert_eq!(count_kind(&conn, "leftover.link_minted"), 0);
}

#[test]
fn lo_b_poll_close_books_leftover_paid_and_income_in_one_tx() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let reference = format!("{LEFTOVER_CLIENT_REFERENCE_PREFIX}{}", listing.listing_id);
    let session = paid_session(&reference, 700, "cs_lo_b_close");
    let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
    assert!(matches!(
        outcome,
        crate::models::AppliedOutcome::Applied { .. }
    ));
    let paid = &leftover::listings(&conn).unwrap()[0];
    assert_eq!(paid.paid_session_id.as_deref(), Some("cs_lo_b_close"));
    assert_eq!(
        paid.paid_at.as_deref(),
        Some(
            db::local_date_from_utc_rfc3339(&session.paid_at)
                .unwrap()
                .as_str()
        )
    );
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
    assert_eq!(descriptor, "Stripe payment link · cs_lo_b_close");
    // No order book moved.
    assert_eq!(count_table(&conn, "orders"), 0);
    assert_eq!(count_kind(&conn, "wholesale.paid"), 0);
}

#[test]
fn lo_b_amount_mismatch_is_refused_with_a_delete_proof_row() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let reference = format!("{LEFTOVER_CLIENT_REFERENCE_PREFIX}{}", listing.listing_id);
    let session = paid_session(&reference, 999, "cs_lo_b_wrong");
    let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
    assert!(matches!(
        outcome,
        crate::models::AppliedOutcome::Rejected { .. }
    ));
    let status: String = conn
        .query_row(
            "SELECT status FROM stripe_unapplied_facts WHERE stripe_id = 'cs_lo_b_wrong'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "leftover_amount_mismatch");
    assert!(leftover::listings(&conn).unwrap()[0].paid_at.is_none());
    assert_eq!(count_kind(&conn, "income.received"), 0);
}

#[test]
fn lo_b_second_session_is_already_paid_and_repoll_is_idempotent() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let reference = format!("{LEFTOVER_CLIENT_REFERENCE_PREFIX}{}", listing.listing_id);
    let first = paid_session(&reference, 700, "cs_lo_b_first");
    money::apply_paid_session(&mut conn, &first).unwrap();
    // The same session again: idempotent, nothing doubled.
    let again = money::apply_paid_session(&mut conn, &first).unwrap();
    assert!(matches!(
        again,
        crate::models::AppliedOutcome::AlreadyApplied
    ));
    // A different session paying the same listing: delete-proof refusal.
    let second = paid_session(&reference, 700, "cs_lo_b_second");
    let outcome = money::apply_paid_session(&mut conn, &second).unwrap();
    assert!(matches!(
        outcome,
        crate::models::AppliedOutcome::Rejected { .. }
    ));
    let status: String = conn
        .query_row(
            "SELECT status FROM stripe_unapplied_facts WHERE stripe_id = 'cs_lo_b_second'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "leftover_already_paid");
    assert_eq!(count_kind(&conn, "leftover.paid"), 1);
    assert_eq!(count_kind(&conn, "income.received"), 1);
}

#[test]
fn lo_b_wholesale_wo_path_is_unchanged_and_never_reaches_a_listing() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let venue =
        crate::marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None)
            .unwrap();
    let order = wholesale::record_order(
        &mut conn,
        &venue.venue_id,
        &today(),
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 1,
            price_cents_per_tray: Some(600),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let session = paid_session(&format!("wo-{}", order.id), 600, "cs_lo_b_wholesale");
    let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
    assert!(matches!(
        outcome,
        crate::models::AppliedOutcome::Applied { .. }
    ));
    assert_eq!(
        wholesale::get_order(&conn, &order.id).unwrap().state,
        "paid"
    );
    assert_eq!(count_kind(&conn, "wholesale.paid"), 1);
    assert_eq!(count_kind(&conn, "leftover.paid"), 0);
    assert!(leftover::listings(&conn).unwrap()[0].paid_at.is_none());
}

#[test]
fn lo_b_payloads_are_sealed_at_the_choke_point() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let good = LeftoverLinkMintedPayload {
        listing_id: listing.listing_id.clone(),
        payment_link_id: "plink_x".into(),
        payment_link_url: "https://example.test/x".into(),
        client_reference: format!("lo-{}", listing.listing_id),
        priced_total_cents: 700,
        currency: "usd".into(),
        minted_on: today(),
    };
    let mut extra = serde_json::to_value(&good).unwrap();
    extra["shopPage"] = json!(true);
    let mut bad_reference = serde_json::to_value(&good).unwrap();
    bad_reference["clientReference"] = json!("wo-somewhere-else");
    let bad = [
        (extra, "unknown field"),
        (bad_reference, "client_reference must be lo-"),
        (
            json!({
                "listingId": listing.listing_id,
                "paidAt": "yesterday",
                "incomeEventId": "inc-1",
                "stripeSessionId": "cs_x",
            }),
            "paid_at must be YYYY-MM-DD",
        ),
    ];
    for (payload, needle) in bad {
        let kind = if payload.get("paidAt").is_some() {
            Kind::LeftoverPaid
        } else {
            Kind::LeftoverLinkMinted
        };
        let ev = EventRecord::originated(
            kind,
            "leftover_listing",
            "lo-x".to_string(),
            payload,
            json!({ "op": "none" }),
            "2026-08-28T12:00:00.000Z".to_string(),
            None,
            None,
            None,
        );
        let tx = conn.transaction().unwrap();
        let err = events::write_event(&tx, &ev).unwrap_err();
        assert!(err.contains(needle), "{needle}: {err}");
        drop(tx);
    }
    assert_eq!(count_kind(&conn, "leftover.link_minted"), 0);
    assert_eq!(count_kind(&conn, "leftover.paid"), 0);
}

#[test]
fn lo_b_mint_and_close_move_no_capacity() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let before = serde_json::to_string(&trays::capacity_by_harvest_date(&conn).unwrap()).unwrap();
    let trays_before = count_table(&conn, "trays");
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let now = projection::handler_now();
    leftover::pay_listing_from_link_session(
        &mut conn,
        &listing.listing_id,
        "cs_lo_b_capacity",
        None,
        700,
        &now,
    )
    .unwrap();
    let after = serde_json::to_string(&trays::capacity_by_harvest_date(&conn).unwrap()).unwrap();
    assert_eq!(before, after);
    assert_eq!(count_table(&conn, "trays"), trays_before);
    assert_eq!(count_table(&conn, "orders"), 0);
    assert_eq!(count_table(&conn, "wholesale_orders"), 0);
}

#[test]
fn lo_b_listing_mint_and_close_replay_and_verify_passes() {
    let dir = temp_dir("verify");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let now = projection::handler_now();
    leftover::pay_listing_from_link_session(
        &mut conn,
        &listing.listing_id,
        "cs_lo_b_replay",
        None,
        700,
        &now,
    )
    .unwrap();
    assert_eq!(income::list_income(&conn).unwrap().len(), 1);
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
