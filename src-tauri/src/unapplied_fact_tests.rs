//! Shape B — rejected-session persistence.

use crate::db;
use crate::event_file;
use crate::money::{self, FactOutcome};
use crate::poll;
use crate::projection;
use crate::trays;
use rusqlite::{params, Connection};
use std::fs;
use std::path::PathBuf;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "farm-os-unapplied-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn unmatched_session(session_id: &str, amount_cents: i64, created: i64) -> money::PaidSession {
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
        created,
        amount_cents,
        client_reference: None,
        payment_link: None,
    }
}

fn fact_rows(conn: &Connection) -> Vec<money::UnappliedFactView> {
    money::list_unapplied_facts(conn).unwrap()
}

fn open_attention_kind(conn: &Connection, kind: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM attention WHERE resolved_at IS NULL AND kind = ?1",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn sb1_unmatched_session_writes_one_fact_and_still_raises_attention() {
    let mut conn = mem();
    let session = unmatched_session("cs_unmatched", 2400, 800);
    let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
    assert!(matches!(
        outcome,
        crate::models::AppliedOutcome::Rejected { .. }
    ));

    assert_eq!(
        trays::count_event_kind(&conn, "stripe.fact_unapplied").unwrap(),
        1
    );
    let rows = fact_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "unmatched");
    assert_eq!(rows[0].stripe_object, "checkout_session");
    assert_eq!(rows[0].stripe_id, "cs_unmatched");
    assert_eq!(rows[0].amount_cents, Some(2400));
    assert_eq!(open_attention_kind(&conn, "stripe.unrecognised_session"), 1);
}

#[test]
fn sb2_same_session_twice_still_one_row() {
    let mut conn = mem();
    let session = unmatched_session("cs_twice", 1200, 801);
    let _ = money::apply_paid_session(&mut conn, &session).unwrap();
    let _ = money::apply_paid_session(&mut conn, &session).unwrap();
    assert_eq!(
        trays::count_event_kind(&conn, "stripe.fact_unapplied").unwrap(),
        1
    );
    assert_eq!(fact_rows(&conn).len(), 1);
}

#[test]
fn sb3_refund_against_unpaid_order_is_already_applied_and_recorded() {
    let mut conn = mem();
    conn.execute(
        "INSERT INTO orders
         (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
          quantity, amount_cents, currency, customer_email, state, capacity_consumed,
          paid_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, 1200, 'cad', NULL, 'refunded', 0, ?6, ?6, ?6)",
        params![
            "ord_unpaid",
            "cs_unpaid",
            "pi_unpaid",
            "2026-08-20",
            "dun-peas",
            "2026-08-05T12:00:00.000Z"
        ],
    )
    .unwrap();

    let outcome = money::apply_refund(
        &mut conn,
        &money::RefundRecord {
            refund_id: "re_unpaid".into(),
            payment_intent: Some("pi_unpaid".into()),
            session_id: Some("cs_unpaid".into()),
            created: 1_700_000_200,
            amount_cents: Some(1200),
            currency: Some("cad".into()),
            status: Some("succeeded".into()),
        },
    )
    .unwrap();
    assert_eq!(outcome, FactOutcome::AlreadyApplied);

    let rows = fact_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "no_paid_order");
    assert_eq!(rows[0].stripe_object, "refund");
    assert_eq!(rows[0].stripe_id, "re_unpaid");
    assert_eq!(rows[0].amount_cents, Some(1200));
    assert_eq!(rows[0].currency.as_deref(), Some("cad"));
}

#[test]
fn sb4_unparsed_session_writes_row_and_advances_cursor() {
    use money::fake::FakeGateway;

    let mut conn = mem();
    let gw = FakeGateway::new();
    {
        let mut st = gw.state.lock().unwrap();
        st.session_pages = vec![money::SessionPage {
            parsed: vec![],
            unparsed: vec![money::UnparsedSession {
                session_id: "cs_unparsed".into(),
                created: 910,
                amount_cents: 1800,
                currency: "cad".into(),
                reason: "session missing price id".into(),
            }],
        }];
    }

    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok);
    let rows = fact_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "unmatched");
    assert_eq!(rows[0].stripe_id, "cs_unparsed");
    assert_eq!(rows[0].amount_cents, Some(1800));
    assert_eq!(poll::sessions_since(&conn).unwrap().as_deref(), Some("910"));
}

#[test]
fn sb5_verify_replay_rebuilds_the_row() {
    let dir = tempfile_dir("sb5");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    let session = unmatched_session("cs_replay", 3600, 820);
    money::apply_paid_session(&mut conn, &session).unwrap();
    event_file::try_flush_after_commit(&conn, &dir);
    drop(conn);

    let jsonl = event_file::events_path(&dir);
    let text = fs::read_to_string(&jsonl).unwrap();
    assert!(
        text.contains("stripe.fact_unapplied"),
        "events.jsonl must carry the new kind"
    );

    let snap = dir.join("snapshot.db");
    let replay = dir.join("replay.db");
    {
        let live = Connection::open(&farm).unwrap();
        live.execute("VACUUM INTO ?1", params![snap.to_str().unwrap()])
            .unwrap();
    }
    let outcome = projection::verify_replay(&snap, &jsonl, &replay).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify_replay failed: {}",
        outcome.summary_line()
    );

    let replay_conn = Connection::open(&replay).unwrap();
    let n: i64 = replay_conn
        .query_row(
            "SELECT COUNT(*) FROM stripe_unapplied_facts WHERE status = 'unmatched'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
    let _ = fs::remove_dir_all(&dir);
}

fn insert_unpaid_order(conn: &Connection, session_id: &str, payment_intent: &str) {
    conn.execute(
        "INSERT INTO orders
         (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
          quantity, amount_cents, currency, customer_email, state, capacity_consumed,
          paid_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, 1200, 'cad', NULL, 'refunded', 0, ?6, ?6, ?6)",
        params![
            format!("ord_{session_id}"),
            session_id,
            payment_intent,
            "2026-08-20",
            "dun-peas",
            "2026-08-05T12:00:00.000Z"
        ],
    )
    .unwrap();
}

fn seed_offer(conn: &Connection, harvest_date: &str, crop_id: &str, price_id: &str) {
    conn.execute(
        "INSERT INTO offers
         (id, harvest_date, crop_id, price_cents, stripe_price_id,
          stripe_link_id, stripe_link_url, created_at)
         VALUES (?1, ?2, ?3, 1200, ?4, NULL, NULL, ?5)
         ON CONFLICT(harvest_date, crop_id) DO UPDATE SET
           stripe_price_id = excluded.stripe_price_id",
        params![
            format!("offer_{price_id}"),
            harvest_date,
            crop_id,
            price_id,
            "2026-08-05T12:00:00.000Z"
        ],
    )
    .unwrap();
}

fn paid_session(
    session_id: &str,
    harvest_date: &str,
    crop_id: &str,
    created: i64,
) -> money::PaidSession {
    let price_id = format!("price_{harvest_date}_{crop_id}");
    money::PaidSession {
        session_id: session_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        lines: vec![money::PaidLine {
            price_id,
            quantity: 1,
            amount_cents: 1200,
        }],
        currency: "cad".into(),
        customer_email: Some("buyer@example.com".into()),
        paid_at: "2026-08-05T12:00:00.000Z".into(),
        created,
        amount_cents: 1200,
        client_reference: None,
        payment_link: None,
    }
}

#[test]
fn ra1_refund_from_json_reads_amount_and_keeps_missing_unknown() {
    use crate::stripe_client::refund_from_json;
    use serde_json::json;

    let full = refund_from_json(&json!({
        "id": "re_full",
        "payment_intent": "pi_1",
        "created": 1_700_000_000,
        "amount": 2400,
        "currency": "cad",
        "status": "succeeded",
    }))
    .unwrap();
    assert_eq!(full.refund_id, "re_full");
    assert_eq!(full.amount_cents, Some(2400));
    assert_eq!(full.currency.as_deref(), Some("cad"));
    assert_eq!(full.status.as_deref(), Some("succeeded"));

    let missing = refund_from_json(&json!({
        "id": "re_no_amt",
        "created": 1,
    }))
    .unwrap();
    assert_eq!(missing.amount_cents, None);
    assert!(refund_from_json(&json!({ "created": 1 })).is_none());
}

#[test]
fn ra2_dispute_from_json_reads_amount_and_keeps_missing_unknown() {
    use crate::stripe_client::dispute_from_json;
    use serde_json::json;

    let full = dispute_from_json(&json!({
        "id": "dp_full",
        "payment_intent": "pi_1",
        "created": 1_700_000_000,
        "amount": 3600,
        "currency": "usd",
        "status": "needs_response",
    }))
    .unwrap();
    assert_eq!(full.dispute_id, "dp_full");
    assert_eq!(full.amount_cents, Some(3600));
    assert_eq!(full.currency.as_deref(), Some("usd"));
    assert_eq!(full.status.as_deref(), Some("needs_response"));

    let missing = dispute_from_json(&json!({
        "id": "dp_no_amt",
        "created": 1,
    }))
    .unwrap();
    assert_eq!(missing.amount_cents, None);
    assert!(dispute_from_json(&json!({ "created": 1 })).is_none());
}

#[test]
fn ra3_unmatched_dispute_with_amount_lands_on_the_row() {
    let mut conn = mem();
    insert_unpaid_order(&conn, "cs_dp_amt", "pi_dp_amt");
    let outcome = money::apply_dispute(
        &mut conn,
        &money::DisputeRecord {
            dispute_id: "dp_amt".into(),
            payment_intent: Some("pi_dp_amt".into()),
            session_id: Some("cs_dp_amt".into()),
            created: 1_700_000_300,
            amount_cents: Some(1800),
            currency: Some("cad".into()),
            status: Some("needs_response".into()),
        },
    )
    .unwrap();
    assert_eq!(outcome, FactOutcome::AlreadyApplied);
    let rows = fact_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "no_paid_order");
    assert_eq!(rows[0].stripe_object, "dispute");
    assert_eq!(rows[0].amount_cents, Some(1800));
    assert_eq!(rows[0].currency.as_deref(), Some("cad"));
}

#[test]
fn ra4_unmatched_refund_without_amount_stays_null() {
    let mut conn = mem();
    insert_unpaid_order(&conn, "cs_re_none", "pi_re_none");
    let outcome = money::apply_refund(
        &mut conn,
        &money::RefundRecord {
            refund_id: "re_none".into(),
            payment_intent: Some("pi_re_none".into()),
            session_id: Some("cs_re_none".into()),
            created: 1_700_000_400,
            amount_cents: None,
            currency: None,
            status: None,
        },
    )
    .unwrap();
    assert_eq!(outcome, FactOutcome::AlreadyApplied);
    let rows = fact_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].amount_cents, None);
    assert_eq!(rows[0].currency, None);
}

#[test]
fn ra5_verify_replay_applies_historic_and_enriched_refund_payloads() {
    use crate::events::{self, EventRecord, Kind};
    use serde_json::json;

    let dir = tempfile_dir("ra5");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
    let hd = trays::get_tray(&conn, &t.id)
        .unwrap()
        .expected_harvest_date
        .expect("expected harvest date");
    seed_offer(&conn, &hd, "dun-peas", &format!("price_{hd}_dun-peas"));

    let historic = paid_session("cs_hist", &hd, "dun-peas", 830);
    let enriched = paid_session("cs_new", &hd, "dun-peas", 831);
    money::apply_paid_session(&mut conn, &historic).unwrap();
    money::apply_paid_session(&mut conn, &enriched).unwrap();

    let hist_order = money::list_orders(&conn, None)
        .unwrap()
        .into_iter()
        .find(|o| o.stripe_session_id == "cs_hist")
        .unwrap();
    let now = crate::projection::handler_now();
    let historic_event = EventRecord::originated(
        Kind::StripeRefunded,
        "order",
        hist_order.id.clone(),
        json!({
            "refundId": "re_hist",
            "orderIds": [hist_order.id],
            "capacityReleased": true,
        }),
        json!({ "op": "none" }),
        now,
        None,
        None,
        Some(crate::projection::handler_new_id()),
    );
    let tx = conn.transaction().unwrap();
    crate::projection::apply_event(&tx, &historic_event).unwrap();
    events::insert_event(&tx, &historic_event).unwrap();
    tx.commit().unwrap();

    money::apply_refund(
        &mut conn,
        &money::RefundRecord {
            refund_id: "re_new".into(),
            payment_intent: Some("pi_cs_new".into()),
            session_id: Some("cs_new".into()),
            created: 1_700_000_500,
            amount_cents: Some(1500),
            currency: Some("cad".into()),
            status: Some("succeeded".into()),
        },
    )
    .unwrap();

    event_file::try_flush_after_commit(&conn, &dir);
    drop(conn);

    let jsonl = event_file::events_path(&dir);
    let text = fs::read_to_string(&jsonl).unwrap();
    let hist_line = text
        .lines()
        .find(|line| line.contains("re_hist"))
        .expect("historic stripe.refunded event");
    assert!(
        !hist_line.contains("amountCents"),
        "historic payload must not invent amountCents"
    );
    assert!(text.contains("re_new"));
    assert!(text.contains("amountCents"));

    let snap = dir.join("snapshot.db");
    let replay = dir.join("replay.db");
    {
        let live = Connection::open(&farm).unwrap();
        live.execute("VACUUM INTO ?1", params![snap.to_str().unwrap()])
            .unwrap();
    }
    let outcome = projection::verify_replay(&snap, &jsonl, &replay).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify_replay failed: {}",
        outcome.summary_line()
    );
    let _ = fs::remove_dir_all(&dir);
}

// --- C2 REFUND-GATE (INT-002) ----------------------------------------------
// Before C2 every refund object Stripe listed - partial, pending, failed,
// amount unreported - flipped the session's paid orders to `refunded`,
// released their capacity and removed the whole sale from cash. The gate:
// only a `succeeded` refund whose amount covers the session's paid orders
// (same currency) reaches the order book. Everything else is traced by
// reason; two of the reasons also raise a card.

/// One sown tray gives a future harvest date; the 1200 cad offer sits on it.
fn int002_seed(conn: &mut Connection) -> String {
    let t = trays::sow_tray(conn, "dun-peas", 4).unwrap();
    let hd = trays::get_tray(conn, &t.id)
        .unwrap()
        .expected_harvest_date
        .expect("expected harvest date");
    seed_offer(conn, &hd, "dun-peas", &format!("price_{hd}_dun-peas"));
    hd
}

fn int002_order(conn: &Connection, session_id: &str) -> crate::models::OrderView {
    money::list_orders(conn, None)
        .unwrap()
        .into_iter()
        .find(|o| o.stripe_session_id == session_id)
        .expect("order for session")
}

/// One paid 1200 cad session (qty 1) on `hd`; returns its order id.
fn int002_pay(conn: &mut Connection, session_id: &str, hd: &str, created: i64) -> String {
    let outcome =
        money::apply_paid_session(conn, &paid_session(session_id, hd, "dun-peas", created))
            .unwrap();
    assert!(
        matches!(outcome, crate::models::AppliedOutcome::Applied { .. }),
        "session must apply: {outcome:?}"
    );
    int002_order(conn, session_id).id
}

fn int002_refund(
    session_id: &str,
    refund_id: &str,
    amount_cents: Option<i64>,
    currency: Option<&str>,
    status: Option<&str>,
    created: i64,
) -> money::RefundRecord {
    money::RefundRecord {
        refund_id: refund_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        session_id: Some(session_id.into()),
        created,
        amount_cents,
        currency: currency.map(str::to_string),
        status: status.map(str::to_string),
    }
}

fn int002_event_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap()
}

/// Statuses in the table itself for one Stripe id - the trail, not the
/// surface (`fact_rows` goes through `list_unapplied_facts`).
fn int002_table_statuses(conn: &Connection, stripe_id: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT status FROM stripe_unapplied_facts
             WHERE stripe_object = 'refund' AND stripe_id = ?1
             ORDER BY status",
        )
        .unwrap();
    stmt.query_map([stripe_id], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// (entity_type, entity_id, message, actions) of the open cards of one kind.
fn int002_cards(conn: &Connection, kind: &str) -> Vec<(String, String, String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT entity_type, entity_id, message, actions FROM attention
             WHERE resolved_at IS NULL AND kind = ?1
             ORDER BY entity_id",
        )
        .unwrap();
    stmt.query_map([kind], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn int002_fact_payload(conn: &Connection, stripe_id: &str, status: &str) -> serde_json::Value {
    let text: String = conn
        .query_row(
            "SELECT payload FROM event_log
             WHERE kind = 'stripe.fact_unapplied'
               AND json_extract(payload, '$.stripeId') = ?1
               AND json_extract(payload, '$.status') = ?2",
            params![stripe_id, status],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn int002_cash(conn: &Connection) -> i64 {
    crate::income::cash_collected_between(conn, None, None)
        .unwrap()
        .total_cents
}

#[test]
fn int002_partial_refund_leaves_the_order_paid_and_traces_amount_partial() {
    let mut conn = mem();
    let hd = int002_seed(&mut conn);
    let order_id = int002_pay(&mut conn, "cs_part", &hd, 840);
    let cash_before = int002_cash(&conn);
    let events_before = int002_event_count(&conn);
    assert_eq!(cash_before, 1200);

    let partial = int002_refund(
        "cs_part",
        "re_part",
        Some(500),
        Some("cad"),
        Some("succeeded"),
        900,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &partial).unwrap(),
        FactOutcome::AlreadyApplied
    );

    let order = int002_order(&conn, "cs_part");
    assert_eq!(order.state, "paid");
    assert_eq!(order.capacity_consumed, 1);
    assert_eq!(int002_cash(&conn), cash_before);
    assert_eq!(int002_event_count(&conn), events_before + 1);

    let rows = fact_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].stripe_object, "refund");
    assert_eq!(rows[0].stripe_id, "re_part");
    assert_eq!(rows[0].status, "amount_partial");
    assert_eq!(rows[0].amount_cents, Some(500));
    assert_eq!(rows[0].currency.as_deref(), Some("cad"));

    let payload = int002_fact_payload(&conn, "re_part", "amount_partial");
    assert_eq!(payload["paymentIntent"], "pi_cs_part");
    assert_eq!(payload["orderIds"], serde_json::json!([order_id]));
    assert_eq!(payload["stripeStatus"], "succeeded");
    assert_eq!(payload["amountCents"], 500);

    let cards = int002_cards(&conn, "order.refund_partial");
    assert_eq!(cards.len(), 1);
    let (entity_type, entity_id, message, actions) = &cards[0];
    // Keyed like order.refunded, so open_in_stripe resolves to the payment.
    assert_eq!(entity_type, "order");
    assert_eq!(entity_id, &order_id);
    assert!(message.contains("$5.00"), "reported amount only: {message}");
    assert!(message.contains("stays paid"), "{message}");
    assert!(actions.contains("open_in_stripe") && actions.contains("dismiss"));
    assert!(
        money::stripe_dashboard_url(&conn, entity_id)
            .unwrap()
            .expect("payment url")
            .ends_with("payments/pi_cs_part"),
        "open_in_stripe must land on the payment"
    );
    assert_eq!(open_attention_kind(&conn, "order.refunded"), 0);
    assert_eq!(open_attention_kind(&conn, "order.refund_failed"), 0);

    // A second partial refund on the same order: its own trace row, but the
    // open card index (kind, entity_id) keeps one open card per order.
    let second = int002_refund(
        "cs_part",
        "re_part2",
        Some(300),
        Some("cad"),
        Some("succeeded"),
        901,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &second).unwrap(),
        FactOutcome::AlreadyApplied
    );
    assert_eq!(
        int002_table_statuses(&conn, "re_part2"),
        vec!["amount_partial"]
    );
    assert_eq!(fact_rows(&conn).len(), 2);
    assert_eq!(int002_cards(&conn, "order.refund_partial").len(), 1);
    assert_eq!(int002_order(&conn, "cs_part").state, "paid");

    // Re-read (the poll re-lists on purpose): nothing new anywhere.
    let events_now = int002_event_count(&conn);
    assert_eq!(
        money::apply_refund(&mut conn, &partial).unwrap(),
        FactOutcome::AlreadyApplied
    );
    assert_eq!(fact_rows(&conn).len(), 2);
    assert_eq!(int002_event_count(&conn), events_now);
    assert_eq!(int002_cards(&conn, "order.refund_partial").len(), 1);
    assert_eq!(int002_order(&conn, "cs_part").state, "paid");
}

#[test]
fn int002_pending_refund_is_deferred_then_applies_once_stripe_settles_it() {
    let mut conn = mem();
    let hd = int002_seed(&mut conn);
    int002_pay(&mut conn, "cs_hold", &hd, 841);
    let events_before = int002_event_count(&conn);

    let pending = int002_refund(
        "cs_hold",
        "re_hold",
        Some(1200),
        Some("cad"),
        Some("pending"),
        901,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &pending).unwrap(),
        FactOutcome::Deferred
    );
    assert_eq!(int002_order(&conn, "cs_hold").state, "paid");
    let rows = fact_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "not_terminal");
    assert_eq!(rows[0].amount_cents, Some(1200));
    assert_eq!(
        int002_fact_payload(&conn, "re_hold", "not_terminal")["stripeStatus"],
        "pending"
    );
    for kind in [
        "order.refunded",
        "order.refund_partial",
        "order.refund_failed",
    ] {
        assert_eq!(open_attention_kind(&conn, kind), 0, "{kind}: trace only");
    }

    // Held and re-read: still deferred, still one row, no new event.
    assert_eq!(
        money::apply_refund(&mut conn, &pending).unwrap(),
        FactOutcome::Deferred
    );
    assert_eq!(fact_rows(&conn).len(), 1);
    assert_eq!(int002_event_count(&conn), events_before + 1);

    // requires_action is the other non-terminal word.
    let action = int002_refund(
        "cs_hold",
        "re_act",
        Some(1200),
        Some("cad"),
        Some("requires_action"),
        902,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &action).unwrap(),
        FactOutcome::Deferred
    );
    assert_eq!(int002_table_statuses(&conn, "re_act"), vec!["not_terminal"]);

    // Stripe settles it: same id, now succeeded, covering amount.
    let settled = int002_refund(
        "cs_hold",
        "re_hold",
        Some(1200),
        Some("cad"),
        Some("succeeded"),
        901,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &settled).unwrap(),
        FactOutcome::Applied
    );
    let order = int002_order(&conn, "cs_hold");
    assert_eq!(order.state, "refunded");
    assert_eq!(order.capacity_consumed, 0);
    assert_eq!(open_attention_kind(&conn, "order.refunded"), 1);
    // The trail keeps the not_terminal row; the surface stops calling the
    // refund unapplied. re_act's row stays on both (never applied).
    assert_eq!(
        int002_table_statuses(&conn, "re_hold"),
        vec!["not_terminal"]
    );
    let surfaced: Vec<String> = fact_rows(&conn).into_iter().map(|r| r.stripe_id).collect();
    assert_eq!(surfaced, vec!["re_act".to_string()]);

    // Re-read of the applied refund: answered by the log, no trace written.
    let events_after = int002_event_count(&conn);
    assert_eq!(
        money::apply_refund(&mut conn, &settled).unwrap(),
        FactOutcome::AlreadyApplied
    );
    assert_eq!(int002_event_count(&conn), events_after);
    assert_eq!(
        int002_table_statuses(&conn, "re_hold"),
        vec!["not_terminal"]
    );
}

#[test]
fn int002_failed_and_canceled_refunds_trace_terminal_failed_with_a_card() {
    let mut conn = mem();
    let hd = int002_seed(&mut conn);
    let fail_order = int002_pay(&mut conn, "cs_fail", &hd, 842);
    let canc_order = int002_pay(&mut conn, "cs_canc", &hd, 843);

    let failed = int002_refund(
        "cs_fail",
        "re_fail",
        Some(1200),
        Some("cad"),
        Some("failed"),
        903,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &failed).unwrap(),
        FactOutcome::AlreadyApplied
    );
    let canceled = int002_refund(
        "cs_canc",
        "re_canc",
        Some(700),
        Some("cad"),
        Some("canceled"),
        904,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &canceled).unwrap(),
        FactOutcome::AlreadyApplied
    );

    for session in ["cs_fail", "cs_canc"] {
        let order = int002_order(&conn, session);
        assert_eq!(order.state, "paid", "{session}");
        assert_eq!(order.capacity_consumed, 1, "{session}");
    }
    assert_eq!(int002_cash(&conn), 2400);
    assert_eq!(
        int002_table_statuses(&conn, "re_fail"),
        vec!["terminal_failed"]
    );
    assert_eq!(
        int002_table_statuses(&conn, "re_canc"),
        vec!["terminal_failed"]
    );
    assert_eq!(
        int002_fact_payload(&conn, "re_canc", "terminal_failed")["stripeStatus"],
        "canceled"
    );

    let cards = int002_cards(&conn, "order.refund_failed");
    assert_eq!(cards.len(), 2);
    let by_order = |id: &str| {
        cards
            .iter()
            .find(|c| c.1 == id)
            .unwrap_or_else(|| panic!("card for {id}"))
    };
    let fail_card = by_order(&fail_order);
    assert_eq!(fail_card.0, "order");
    assert!(
        fail_card.2.contains("$12.00") && fail_card.2.contains("failed at Stripe"),
        "{}",
        fail_card.2
    );
    let canc_card = by_order(&canc_order);
    assert!(
        canc_card.2.contains("$7.00") && canc_card.2.contains("stays paid"),
        "{}",
        canc_card.2
    );
    assert_eq!(open_attention_kind(&conn, "order.refunded"), 0);
    assert_eq!(open_attention_kind(&conn, "order.refund_partial"), 0);

    // Re-read: idempotent.
    let events = int002_event_count(&conn);
    assert_eq!(
        money::apply_refund(&mut conn, &failed).unwrap(),
        FactOutcome::AlreadyApplied
    );
    assert_eq!(int002_event_count(&conn), events);
    assert_eq!(int002_cards(&conn, "order.refund_failed").len(), 2);

    // A failed refund Stripe reports without an amount: traced, no card -
    // never a card without its number.
    int002_pay(&mut conn, "cs_fail_blank", &hd, 844);
    let blank = int002_refund("cs_fail_blank", "re_blank", None, None, Some("failed"), 905);
    assert_eq!(
        money::apply_refund(&mut conn, &blank).unwrap(),
        FactOutcome::AlreadyApplied
    );
    assert_eq!(
        int002_table_statuses(&conn, "re_blank"),
        vec!["terminal_failed"]
    );
    assert_eq!(int002_cards(&conn, "order.refund_failed").len(), 2);
    assert_eq!(int002_order(&conn, "cs_fail_blank").state, "paid");
}

#[test]
fn int002_not_comparable_refunds_leave_only_the_trace() {
    let mut conn = mem();
    let hd = int002_seed(&mut conn);
    int002_pay(&mut conn, "cs_nc", &hd, 843);

    let cases = [
        ("re_nc_amount", None, Some("cad"), Some("succeeded")),
        ("re_nc_status", Some(1200), Some("cad"), None),
        ("re_nc_currency", Some(1200), Some("usd"), Some("succeeded")),
        ("re_nc_word", Some(1200), Some("cad"), Some("something_new")),
        ("re_nc_nocur", Some(1200), None, Some("succeeded")),
    ];
    for (i, (id, amount, currency, status)) in cases.iter().enumerate() {
        let refund = int002_refund("cs_nc", id, *amount, *currency, *status, 910 + i as i64);
        assert_eq!(
            money::apply_refund(&mut conn, &refund).unwrap(),
            FactOutcome::AlreadyApplied,
            "{id}"
        );
        assert_eq!(
            int002_table_statuses(&conn, id),
            vec!["not_comparable"],
            "{id}"
        );
    }
    let order = int002_order(&conn, "cs_nc");
    assert_eq!(order.state, "paid");
    assert_eq!(order.capacity_consumed, 1);
    assert_eq!(fact_rows(&conn).len(), cases.len());
    for kind in [
        "order.refunded",
        "order.refund_partial",
        "order.refund_failed",
    ] {
        assert_eq!(open_attention_kind(&conn, kind), 0, "{kind}: trace only");
    }
    // Nothing invented: a missing amount stays NULL on the row.
    let missing = fact_rows(&conn)
        .into_iter()
        .find(|r| r.stripe_id == "re_nc_amount")
        .unwrap();
    assert_eq!(missing.amount_cents, None);
}

#[test]
fn int002_covering_refund_applies_across_lines_and_above_the_total() {
    let mut conn = mem();
    let hd = int002_seed(&mut conn);
    seed_offer(&conn, &hd, "sunflower", &format!("price_{hd}_sunflower"));

    // Two lines, one session: 1200 + 2000.
    let mut cart = paid_session("cs_cart", &hd, "dun-peas", 844);
    cart.lines.push(money::PaidLine {
        price_id: format!("price_{hd}_sunflower"),
        quantity: 1,
        amount_cents: 2000,
    });
    cart.amount_cents = 3200;
    money::apply_paid_session(&mut conn, &cart).unwrap();
    let cart_orders: Vec<crate::models::OrderView> = money::list_orders(&conn, None)
        .unwrap()
        .into_iter()
        .filter(|o| o.stripe_session_id == "cs_cart")
        .collect();
    assert_eq!(cart_orders.len(), 2);

    // 3199 is partial against the session, not against a line.
    let short = int002_refund(
        "cs_cart",
        "re_short",
        Some(3199),
        Some("cad"),
        Some("succeeded"),
        920,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &short).unwrap(),
        FactOutcome::AlreadyApplied
    );
    assert_eq!(
        int002_table_statuses(&conn, "re_short"),
        vec!["amount_partial"]
    );
    assert!(money::list_orders(&conn, None)
        .unwrap()
        .iter()
        .filter(|o| o.stripe_session_id == "cs_cart")
        .all(|o| o.state == "paid"));

    // 3200 covers both lines: both flip, both release.
    let exact = int002_refund(
        "cs_cart",
        "re_exact",
        Some(3200),
        Some("CAD"),
        Some("succeeded"),
        921,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &exact).unwrap(),
        FactOutcome::Applied
    );
    assert!(money::list_orders(&conn, None)
        .unwrap()
        .iter()
        .filter(|o| o.stripe_session_id == "cs_cart")
        .all(|o| o.state == "refunded" && o.capacity_consumed == 0));

    // Above the total still covers (>=, D1).
    int002_pay(&mut conn, "cs_over", &hd, 845);
    let over = int002_refund(
        "cs_over",
        "re_over",
        Some(1300),
        Some("cad"),
        Some("succeeded"),
        922,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &over).unwrap(),
        FactOutcome::Applied
    );
    assert_eq!(int002_order(&conn, "cs_over").state, "refunded");
    assert_eq!(open_attention_kind(&conn, "order.refunded"), 2);
}

#[test]
fn int002_applied_refund_re_read_is_answered_by_the_log_not_a_no_paid_order_trace() {
    let mut conn = mem();
    let hd = int002_seed(&mut conn);
    int002_pay(&mut conn, "cs_again", &hd, 846);
    let refund = int002_refund(
        "cs_again",
        "re_again",
        Some(1200),
        Some("cad"),
        Some("succeeded"),
        930,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &refund).unwrap(),
        FactOutcome::Applied
    );
    assert_eq!(int002_order(&conn, "cs_again").state, "refunded");
    let events = int002_event_count(&conn);

    // Before C2 this re-read found no paid order and wrote a no_paid_order
    // fact for a refund the log already carried.
    assert_eq!(
        money::apply_refund(&mut conn, &refund).unwrap(),
        FactOutcome::AlreadyApplied
    );
    assert_eq!(int002_event_count(&conn), events);
    assert!(int002_table_statuses(&conn, "re_again").is_empty());
    assert!(fact_rows(&conn).is_empty());
}

#[test]
fn int002_poll_holds_the_refund_cursor_below_the_oldest_pending_refund() {
    use money::fake::FakeGateway;

    let mut conn = mem();
    let hd = int002_seed(&mut conn);
    let gw = FakeGateway::new();
    gw.push_session(paid_session("cs_g1", &hd, "dun-peas", 700));
    gw.push_session(paid_session("cs_g2", &hd, "dun-peas", 701));
    let pending = int002_refund(
        "cs_g1",
        "re_g1",
        Some(1200),
        Some("cad"),
        Some("pending"),
        800,
    );
    let newer = int002_refund(
        "cs_g2",
        "re_g2",
        Some(1200),
        Some("cad"),
        Some("succeeded"),
        900,
    );
    // Stripe lists newest-first.
    gw.state.lock().unwrap().refund_pages = vec![vec![newer.clone(), pending.clone()]];

    let cursor = |conn: &Connection| -> Option<String> {
        conn.query_row(
            "SELECT refunds_since FROM stripe_cursor WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };

    // Poll 1: the newer refund applies; the pending one holds the cursor one
    // second below itself (created[gt] is exclusive) so it is re-read.
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.sessions_applied, 2);
    assert_eq!(r.refunds_applied, 1);
    assert_eq!(int002_order(&conn, "cs_g2").state, "refunded");
    assert_eq!(int002_order(&conn, "cs_g1").state, "paid");
    assert_eq!(cursor(&conn).as_deref(), Some("799"));
    assert_eq!(int002_table_statuses(&conn, "re_g1"), vec!["not_terminal"]);
    assert!(int002_table_statuses(&conn, "re_g2").is_empty());

    // Poll 2, nothing changed at Stripe: both re-listed, nothing new written.
    let events = int002_event_count(&conn);
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.refunds_applied, 0);
    assert_eq!(cursor(&conn).as_deref(), Some("799"));
    assert_eq!(int002_event_count(&conn), events);
    assert!(
        int002_table_statuses(&conn, "re_g2").is_empty(),
        "no no_paid_order for an applied refund"
    );

    // Stripe settles re_g1: same id, same created, now succeeded.
    let settled = int002_refund(
        "cs_g1",
        "re_g1",
        Some(1200),
        Some("cad"),
        Some("succeeded"),
        800,
    );
    gw.state.lock().unwrap().refund_pages = vec![vec![newer.clone(), settled]];
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.refunds_applied, 1);
    assert_eq!(int002_order(&conn, "cs_g1").state, "refunded");
    assert_eq!(cursor(&conn).as_deref(), Some("900"));
    assert!(
        fact_rows(&conn).is_empty(),
        "settled refund retires from the surface"
    );
    assert_eq!(int002_table_statuses(&conn, "re_g1"), vec!["not_terminal"]);

    // Poll 3: past the cursor, nothing listed, nothing written.
    let events = int002_event_count(&conn);
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.refunds_applied, 0);
    assert_eq!(cursor(&conn).as_deref(), Some("900"));
    assert_eq!(int002_event_count(&conn), events);
}

#[test]
fn int002_v37_widens_the_status_check_and_keeps_rows_and_guards() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(db::SCHEMA_V27_UNAPPLIED_FACTS_SQL)
        .unwrap();
    let insert = |status: &str, id: &str| -> Result<usize, rusqlite::Error> {
        conn.execute(
            "INSERT INTO stripe_unapplied_facts
             (event_id, stripe_object, stripe_id, status, amount_cents, currency,
              stripe_created, observed_at)
             VALUES (?1, 'refund', ?2, ?3, 100, 'cad', 1, '2026-08-05T12:00:00.000Z')",
            params![format!("ev_{id}_{status}"), id, status],
        )
    };
    for (i, status) in ["unmatched", "unrecorded", "no_paid_order"]
        .iter()
        .enumerate()
    {
        insert(status, &format!("legacy_{i}")).unwrap();
    }
    let err = insert("amount_partial", "early").unwrap_err().to_string();
    assert!(err.contains("CHECK"), "v27 refuses the new word: {err}");

    conn.execute_batch(db::SCHEMA_V37_UNAPPLIED_FACTS_WIDEN_SQL)
        .unwrap();

    let kept: i64 = conn
        .query_row("SELECT COUNT(*) FROM stripe_unapplied_facts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(kept, 3, "rows survive the rebuild");
    for status in [
        "amount_partial",
        "not_terminal",
        "terminal_failed",
        "not_comparable",
    ] {
        insert(status, "re_new").unwrap();
    }
    let err = insert("bogus", "re_new").unwrap_err().to_string();
    assert!(err.contains("CHECK"), "closed set: {err}");
    let err = insert("amount_partial", "re_new").unwrap_err().to_string();
    assert!(
        err.contains("UNIQUE"),
        "one row per (object, id, reason): {err}"
    );
    let err = conn
        .execute("UPDATE stripe_unapplied_facts SET status = 'unmatched'", [])
        .unwrap_err()
        .to_string();
    assert!(err.contains("append-only"), "{err}");
    let err = conn
        .execute("DELETE FROM stripe_unapplied_facts", [])
        .unwrap_err()
        .to_string();
    assert!(err.contains("append-only"), "{err}");

    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE tbl_name = 'stripe_unapplied_facts' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for expected in [
        "idx_stripe_unapplied_facts_identity",
        "idx_stripe_unapplied_facts_observed",
        "stripe_unapplied_facts",
        "stripe_unapplied_facts_before_delete",
        "stripe_unapplied_facts_before_update",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "{expected} missing from {names:?}"
        );
    }
    let scratch: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%_new'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(scratch, 0);

    // The real migration lands on the same shape from v1 and from fresh.
    let fresh = mem();
    let v: i32 = fresh
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    let from_v1 = db::open_v1_in_memory().unwrap();
    db::migrate(&from_v1).unwrap();
    for c in [&fresh, &from_v1] {
        c.execute(
            "INSERT INTO stripe_unapplied_facts
             (event_id, stripe_object, stripe_id, status, amount_cents, currency,
              stripe_created, observed_at)
             VALUES ('ev_x', 'refund', 're_x', 'not_terminal', NULL, NULL, 1, '2026-08-05T12:00:00.000Z')",
            [],
        )
        .unwrap();
    }
}

#[test]
fn int002_verify_replay_passes_with_the_new_trace_statuses() {
    let dir = tempfile_dir("int002");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    let hd = int002_seed(&mut conn);
    int002_pay(&mut conn, "cs_v1", &hd, 850);
    int002_pay(&mut conn, "cs_v2", &hd, 851);
    int002_pay(&mut conn, "cs_v3", &hd, 852);

    let partial = int002_refund(
        "cs_v1",
        "re_v1",
        Some(400),
        Some("cad"),
        Some("succeeded"),
        940,
    );
    let pending = int002_refund(
        "cs_v2",
        "re_v2",
        Some(1200),
        Some("cad"),
        Some("pending"),
        941,
    );
    let failed = int002_refund(
        "cs_v3",
        "re_v3",
        Some(1200),
        Some("cad"),
        Some("failed"),
        942,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &partial).unwrap(),
        FactOutcome::AlreadyApplied
    );
    assert_eq!(
        money::apply_refund(&mut conn, &pending).unwrap(),
        FactOutcome::Deferred
    );
    assert_eq!(
        money::apply_refund(&mut conn, &failed).unwrap(),
        FactOutcome::AlreadyApplied
    );
    let settled = int002_refund(
        "cs_v2",
        "re_v2",
        Some(1200),
        Some("cad"),
        Some("succeeded"),
        941,
    );
    assert_eq!(
        money::apply_refund(&mut conn, &settled).unwrap(),
        FactOutcome::Applied
    );

    event_file::try_flush_after_commit(&conn, &dir);
    drop(conn);

    let jsonl = event_file::events_path(&dir);
    let text = fs::read_to_string(&jsonl).unwrap();
    for word in [
        "amount_partial",
        "not_terminal",
        "terminal_failed",
        "stripeStatus",
    ] {
        assert!(text.contains(word), "events.jsonl must carry {word}");
    }

    let snap = dir.join("snapshot.db");
    let replay = dir.join("replay.db");
    {
        let live = Connection::open(&farm).unwrap();
        live.execute("VACUUM INTO ?1", params![snap.to_str().unwrap()])
            .unwrap();
    }
    let outcome = projection::verify_replay(&snap, &jsonl, &replay).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify_replay failed: {}",
        outcome.summary_line()
    );

    let replay_conn = Connection::open(&replay).unwrap();
    let statuses: Vec<String> = replay_conn
        .prepare("SELECT status FROM stripe_unapplied_facts ORDER BY status")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        statuses,
        vec!["amount_partial", "not_terminal", "terminal_failed"]
    );
    let states: Vec<(String, String)> = replay_conn
        .prepare("SELECT stripe_session_id, state FROM orders ORDER BY stripe_session_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        states,
        vec![
            ("cs_v1".to_string(), "paid".to_string()),
            ("cs_v2".to_string(), "refunded".to_string()),
            ("cs_v3".to_string(), "paid".to_string()),
        ]
    );
    let _ = fs::remove_dir_all(&dir);
}
