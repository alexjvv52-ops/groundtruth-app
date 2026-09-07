//! J4 REF-DUP-FACT (board-signed 2026-09-07, tip 0284480) — a second Stripe
//! session under a cart reference an order already carries is named, never
//! booked.
//!
//! MATCH A: the same session id stays a silent AlreadyApplied; a different
//! session id with the same client_reference whose lines still resolve is a
//! fact. RETURN A: Rejected under the new session id. FACT A: the token is
//! `duplicate_reference`. DETAIL A: clientReference, paymentIntent and the
//! first twin's session id ride on the trace; the row is keyed by the new
//! session id. ATTENTION A: fact only. CAPACITY A: one order, one
//! stripe.session_paid, capacity consumed once. SCHEMA A: v44 widens the
//! status CHECK, rows kept, guards kept. SENTENCE A prints on Money exactly
//! once, above the raw-token fallback.
//!
//! No SCHEMA_VERSION literal here — H-11(b) keeps that pin in its three files.
use crate::db;
use crate::money;
use crate::poll;
use crate::trays;
use rusqlite::{params, Connection};
use std::fs;
use std::path::Path;
fn mem() -> Connection {
    db::open_in_memory().unwrap()
}
fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}
fn price_for(hd: &str) -> String {
    format!("price_j4_{hd}")
}
/// One sown tray of dun-peas with a retail offer on its harvest date.
fn seeded_harvest(conn: &mut Connection) -> String {
    let t = trays::sow_tray(conn, "dun-peas", 6).unwrap();
    let hd = trays::get_tray(conn, &t.id)
        .unwrap()
        .expected_harvest_date
        .expect("expected harvest date");
    conn.execute(
        "INSERT INTO offers
         (id, harvest_date, crop_id, price_cents, stripe_price_id,
          stripe_link_id, stripe_link_url, created_at)
         VALUES (?1, ?2, 'dun-peas', 1200, ?3, NULL, NULL, ?4)",
        params![
            format!("offer_j4_{hd}"),
            hd,
            price_for(&hd),
            "2026-08-05T12:00:00.000Z"
        ],
    )
    .unwrap();
    hd
}
/// A paid retail session of 2 × dun-peas under `reference`.
fn session(session_id: &str, hd: &str, reference: &str, created: i64) -> money::PaidSession {
    money::PaidSession {
        session_id: session_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        lines: vec![money::PaidLine {
            price_id: price_for(hd),
            quantity: 2,
            amount_cents: 2400,
        }],
        currency: "cad".into(),
        customer_email: Some("buyer@example.com".into()),
        paid_at: "2026-08-05T12:00:00.000Z".into(),
        created,
        amount_cents: 2400,
        client_reference: Some(reference.into()),
        payment_link: None,
    }
}
fn facts(conn: &Connection, stripe_id: &str, status: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM stripe_unapplied_facts
         WHERE stripe_object = 'checkout_session' AND stripe_id = ?1 AND status = ?2",
        params![stripe_id, status],
        |r| r.get(0),
    )
    .unwrap()
}
fn fact_payload(conn: &Connection, stripe_id: &str) -> serde_json::Value {
    let text: String = conn
        .query_row(
            "SELECT payload FROM event_log
             WHERE kind = 'stripe.fact_unapplied'
               AND json_extract(payload, '$.stripeId') = ?1
               AND json_extract(payload, '$.status') = 'duplicate_reference'",
            [stripe_id],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&text).unwrap()
}
fn open_attention(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM attention WHERE resolved_at IS NULL",
        [],
        |r| r.get(0),
    )
    .unwrap()
}
#[test]
fn j4_second_session_under_a_booked_reference_is_named_never_booked() {
    let mut conn = mem();
    let hd = seeded_harvest(&mut conn);
    let before = trays::remaining_for_date(&conn, &hd).unwrap();
    let first = session("cs_j4_first", &hd, "cart_j4_shared", 1200);
    let second = session("cs_j4_second", &hd, "cart_j4_shared", 1201);
    let a = money::apply_paid_session(&mut conn, &first).unwrap();
    assert!(matches!(a, crate::models::AppliedOutcome::Applied { .. }));
    let attention_before = open_attention(&conn);
    // RETURN A: the twin is Rejected under its own session id.
    match money::apply_paid_session(&mut conn, &second).unwrap() {
        crate::models::AppliedOutcome::Rejected { session_id } => {
            assert_eq!(session_id, "cs_j4_second");
        }
        other => panic!("the twin must be Rejected, got {other:?}"),
    }
    // CAPACITY A: one order, one event, capacity consumed once.
    let orders = money::list_orders(&conn, None).unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].stripe_session_id, "cs_j4_first");
    assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), before - 2);
    assert_eq!(
        trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
        1
    );
    // FACT A, keyed by the new session id — never the first twin's.
    assert_eq!(facts(&conn, "cs_j4_second", "duplicate_reference"), 1);
    assert_eq!(facts(&conn, "cs_j4_first", "duplicate_reference"), 0);
    assert_eq!(
        trays::count_event_kind(&conn, "stripe.fact_unapplied").unwrap(),
        1
    );
    // DETAIL A: the trace names the reference, the twin's intent, and the
    // session whose order stands.
    let payload = fact_payload(&conn, "cs_j4_second");
    assert_eq!(payload["clientReference"], "cart_j4_shared");
    assert_eq!(payload["paymentIntent"], "pi_cs_j4_second");
    assert_eq!(payload["firstSessionId"], "cs_j4_first");
    assert_eq!(payload["amountCents"], 2400);
    // ATTENTION A: fact only.
    assert_eq!(open_attention(&conn), attention_before);
    // The desk sees it: the surface never retires a fact keyed by a session
    // no order carries.
    let shown = money::list_unapplied_facts(&conn).unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].stripe_id, "cs_j4_second");
    assert_eq!(shown[0].status, "duplicate_reference");
    assert_eq!(shown[0].amount_cents, Some(2400));
}
#[test]
fn j4_same_session_id_stays_silent_already_applied_and_writes_no_fact() {
    let mut conn = mem();
    let hd = seeded_harvest(&mut conn);
    let first = session("cs_j4_replay", &hd, "cart_j4_replay", 1300);
    assert!(matches!(
        money::apply_paid_session(&mut conn, &first).unwrap(),
        crate::models::AppliedOutcome::Applied { .. }
    ));
    assert!(matches!(
        money::apply_paid_session(&mut conn, &first).unwrap(),
        crate::models::AppliedOutcome::AlreadyApplied
    ));
    assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
    assert_eq!(
        trays::count_event_kind(&conn, "stripe.fact_unapplied").unwrap(),
        0
    );
    assert_eq!(facts(&conn, "cs_j4_replay", "duplicate_reference"), 0);
}
#[test]
fn j4_poll_names_the_twin_once_and_a_replayed_page_writes_no_second_fact() {
    use money::fake::FakeGateway;
    let mut conn = mem();
    let hd = seeded_harvest(&mut conn);
    let before = trays::remaining_for_date(&conn, &hd).unwrap();
    let a = session("cs_j4_poll_a", &hd, "cart_j4_poll", 1400);
    let b = session("cs_j4_poll_b", &hd, "cart_j4_poll", 1401);
    let gw = FakeGateway::new();
    {
        let mut st = gw.state.lock().unwrap();
        st.session_pages = vec![money::SessionPage::from_parsed(vec![a, b])];
    }
    let r = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(r.ok);
    assert_eq!(r.sessions_applied, 1);
    assert_eq!(r.sessions_rejected, 1);
    assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
    assert_eq!(facts(&conn, "cs_j4_poll_b", "duplicate_reference"), 1);
    // The page comes back with the cursor not moved (hard kill between apply
    // and cursor). The order is silent; the twin is already named.
    conn.execute(
        "UPDATE stripe_cursor SET sessions_since = NULL WHERE id = 1",
        [],
    )
    .unwrap();
    let again = poll::run_poll(&mut conn, &gw).unwrap();
    assert!(again.ok);
    assert_eq!(again.sessions_applied, 0);
    assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
    assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), before - 2);
    assert_eq!(
        trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
        1
    );
    assert_eq!(facts(&conn, "cs_j4_poll_b", "duplicate_reference"), 1);
    assert_eq!(
        trays::count_event_kind(&conn, "stripe.fact_unapplied").unwrap(),
        1
    );
}
#[test]
fn j4_v44_widens_the_status_check_and_keeps_rows_and_guards() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(db::SCHEMA_V27_UNAPPLIED_FACTS_SQL)
        .unwrap();
    let insert = |status: &str, id: &str| -> Result<usize, rusqlite::Error> {
        conn.execute(
            "INSERT INTO stripe_unapplied_facts
             (event_id, stripe_object, stripe_id, status, amount_cents, currency,
              stripe_created, observed_at)
             VALUES (?1, 'checkout_session', ?2, ?3, 2400, 'cad', 1, '2026-09-07T12:00:00.000Z')",
            params![format!("ev_{id}_{status}"), id, status],
        )
    };
    for (i, status) in ["unmatched", "unrecorded", "no_paid_order"]
        .iter()
        .enumerate()
    {
        insert(status, &format!("legacy_{i}")).unwrap();
    }
    let err = insert("duplicate_reference", "early")
        .unwrap_err()
        .to_string();
    assert!(err.contains("CHECK"), "v27 refuses the new word: {err}");
    conn.execute_batch(db::SCHEMA_V37_UNAPPLIED_FACTS_WIDEN_SQL)
        .unwrap();
    conn.execute_batch(db::SCHEMA_V38_UNAPPLIED_FACTS_WIDEN_SQL)
        .unwrap();
    conn.execute_batch(db::SCHEMA_V42_UNAPPLIED_FACTS_WIDEN_SQL)
        .unwrap();
    let err = insert("duplicate_reference", "still_early")
        .unwrap_err()
        .to_string();
    assert!(err.contains("CHECK"), "v42 still refuses it: {err}");
    conn.execute_batch(db::SCHEMA_V44_UNAPPLIED_FACTS_WIDEN_SQL)
        .unwrap();
    let kept: i64 = conn
        .query_row("SELECT COUNT(*) FROM stripe_unapplied_facts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(kept, 3, "rows survive the rebuild");
    insert("duplicate_reference", "cs_twin").unwrap();
    insert("no_payment_intent", "re_still_admitted").unwrap();
    let err = insert("bogus", "cs_twin").unwrap_err().to_string();
    assert!(err.contains("CHECK"), "closed set: {err}");
    let err = insert("duplicate_reference", "cs_twin")
        .unwrap_err()
        .to_string();
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
        .prepare(
            "SELECT name FROM sqlite_master WHERE tbl_name = 'stripe_unapplied_facts' ORDER BY name",
        )
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
}
#[test]
fn j4_a_fresh_farm_opens_with_the_word_admitted() {
    let conn = mem();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        version,
        db::SCHEMA_VERSION,
        "a fresh farm opens at the app's schema"
    );
    conn.execute(
        "INSERT INTO stripe_unapplied_facts
         (event_id, stripe_object, stripe_id, status, amount_cents, currency,
          stripe_created, observed_at)
         VALUES ('ev_j4_fresh', 'checkout_session', 'cs_j4_fresh', 'duplicate_reference',
                 2400, 'cad', 1, '2026-09-07T12:00:00.000Z')",
        [],
    )
    .expect("the migration admits the word on a fresh farm");
}
#[test]
fn j4_money_prints_the_signed_sentence_once_above_the_fallback() {
    let money_src = read("../src/screens/Money.tsx");
    let sentence = "A payment arrived for a cart Farm OS had already recorded. The first order stands; nothing else was recorded, and the money is at Stripe.";
    assert_eq!(
        money_src.matches(sentence).count(),
        1,
        "SENTENCE A, byte for byte, exactly once"
    );
    let arm = money_src
        .find(r#"status === "duplicate_reference""#)
        .expect("Money.tsx lost the duplicate_reference arm");
    let fallback = money_src
        .find("\n  return status;")
        .expect("Money.tsx lost unappliedSentence's raw-token fallback");
    assert!(
        arm < fallback,
        "the token is named before the raw-token fallback, never as the primary line"
    );
    assert_eq!(money_src.matches("function unappliedSentence").count(), 1);
}
#[test]
fn j4_one_writer_for_the_token_and_one_schema_home() {
    let door = read("src/money.rs");
    assert_eq!(
        door.matches("\"duplicate_reference\"").count(),
        1,
        "one writer for the token"
    );
    let schema = read("src/db.rs");
    assert_eq!(
        schema.matches("'duplicate_reference'").count(),
        1,
        "the token lives in the v44 CHECK and nowhere else in db.rs"
    );
}
