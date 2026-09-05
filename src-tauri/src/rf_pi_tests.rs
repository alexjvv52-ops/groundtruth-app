//! PACK-RF-PI (audit R-10, signed law 2026-08-30) — a refund Stripe listed
//! with no payment intent and no session id gets a name instead of silence.
//!
//! What is pinned: the guard fires only when BOTH keys are absent or empty,
//! so a refund that still carries a key keeps the old AwaitingOrder pin
//! (fence_fa_tests) untouched; the named refund records `no_payment_intent`
//! and returns the outcome that lets the poll walk continue, so it can no
//! longer strand every newer refund behind it; amount and currency ride
//! through exactly as Stripe reported them and an unreported amount stays
//! unreported; the v42 rebuild widens the status CHECK on the v37 / v38
//! precedent, keeps existing rows and puts the indexes and append-only
//! triggers back; Money prints the signed sentence and never the raw token;
//! and the dispute twin keeps its AwaitingOrder arm — named, not opened.
//! No SCHEMA_VERSION literal here — H-11(b) keeps that pin in its three files.
use crate::db;
use crate::money::{self, FactOutcome};
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
fn refund(refund_id: &str, intent: Option<&str>, session: Option<&str>) -> money::RefundRecord {
    money::RefundRecord {
        refund_id: refund_id.into(),
        payment_intent: intent.map(str::to_string),
        session_id: session.map(str::to_string),
        created: 1_700_000_500,
        amount_cents: Some(1500),
        currency: Some("cad".into()),
        status: Some("succeeded".into()),
    }
}
#[test]
fn rf_pi_no_intent_and_no_session_is_named_and_the_walk_continues() {
    let mut conn = mem();
    let outcome = money::apply_refund(&mut conn, &refund("re_nopi", None, None)).unwrap();
    assert_eq!(
        outcome,
        FactOutcome::AlreadyApplied,
        "law 4: the poll must not break and strand the refunds behind this one"
    );
    let facts = money::list_unapplied_facts(&conn).unwrap();
    assert_eq!(facts.len(), 1, "the refund is named once");
    assert_eq!(facts[0].status, "no_payment_intent");
    assert_eq!(facts[0].stripe_object, "refund");
    assert_eq!(facts[0].stripe_id, "re_nopi");
    assert_eq!(facts[0].amount_cents, Some(1500));
    assert_eq!(facts[0].currency.as_deref(), Some("cad"));
    let again = money::apply_refund(&mut conn, &refund("re_nopi", None, None)).unwrap();
    assert_eq!(again, FactOutcome::AlreadyApplied);
    assert_eq!(
        money::list_unapplied_facts(&conn).unwrap().len(),
        1,
        "re-reading the same refund adds no second row"
    );
}
#[test]
fn rf_pi_empty_keys_read_the_same_as_absent_keys() {
    let mut conn = mem();
    let outcome = money::apply_refund(&mut conn, &refund("re_empty", Some(""), Some(""))).unwrap();
    assert_eq!(outcome, FactOutcome::AlreadyApplied);
    let facts = money::list_unapplied_facts(&conn).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].status, "no_payment_intent");
}
#[test]
fn rf_pi_an_unreported_amount_stays_unreported() {
    let mut conn = mem();
    let mut r = refund("re_noamount", None, None);
    r.amount_cents = None;
    r.currency = None;
    money::apply_refund(&mut conn, &r).unwrap();
    let facts = money::list_unapplied_facts(&conn).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].amount_cents, None, "law 3: no invented dollars");
    assert_eq!(facts[0].currency, None, "law 3: no invented currency");
}
#[test]
fn rf_pi_a_refund_that_carries_a_key_still_awaits_its_order() {
    let mut conn = mem();
    for (id, intent, session) in [
        (
            "re_keys",
            Some("pi_no_such_order"),
            Some("cs_no_such_order"),
        ),
        ("re_pi_only", Some("pi_only"), None),
        ("re_cs_only", None, Some("cs_only")),
    ] {
        let outcome = money::apply_refund(&mut conn, &refund(id, intent, session)).unwrap();
        assert_eq!(
            outcome,
            FactOutcome::AwaitingOrder,
            "law 2: one key is still a key — {id} keeps the old pin"
        );
    }
    assert!(
        money::list_unapplied_facts(&conn).unwrap().is_empty(),
        "law 2: a refund with a key writes no fact here"
    );
}
#[test]
fn rf_pi_dispute_twin_is_named_not_opened() {
    let mut conn = mem();
    let outcome = money::apply_dispute(
        &mut conn,
        &money::DisputeRecord {
            dispute_id: "dp_nopi".into(),
            payment_intent: None,
            session_id: None,
            created: 1_700_000_600,
            amount_cents: Some(1500),
            currency: Some("cad".into()),
            status: Some("needs_response".into()),
        },
    )
    .unwrap();
    assert_eq!(
        outcome,
        FactOutcome::AwaitingOrder,
        "law 7: the dispute twin is named, not opened"
    );
    assert!(money::list_unapplied_facts(&conn).unwrap().is_empty());
    let door = read("src/money.rs");
    assert_eq!(
        door.matches("return Ok(FactOutcome::AwaitingOrder);")
            .count(),
        2,
        "both AwaitingOrder arms stay: the keyed refund and the dispute twin"
    );
    assert_eq!(
        door.matches("\"no_payment_intent\"").count(),
        1,
        "one writer for the token"
    );
}
#[test]
fn rf_pi_v42_widens_the_status_check_and_keeps_rows_and_guards() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(db::SCHEMA_V27_UNAPPLIED_FACTS_SQL)
        .unwrap();
    let insert = |status: &str, id: &str| -> Result<usize, rusqlite::Error> {
        conn.execute(
            "INSERT INTO stripe_unapplied_facts
             (event_id, stripe_object, stripe_id, status, amount_cents, currency,
              stripe_created, observed_at)
             VALUES (?1, 'refund', ?2, ?3, 100, 'cad', 1, '2026-08-30T12:00:00.000Z')",
            params![format!("ev_{id}_{status}"), id, status],
        )
    };
    for (i, status) in ["unmatched", "unrecorded", "no_paid_order"]
        .iter()
        .enumerate()
    {
        insert(status, &format!("legacy_{i}")).unwrap();
    }
    let err = insert("no_payment_intent", "early")
        .unwrap_err()
        .to_string();
    assert!(err.contains("CHECK"), "v27 refuses the new word: {err}");
    conn.execute_batch(db::SCHEMA_V37_UNAPPLIED_FACTS_WIDEN_SQL)
        .unwrap();
    conn.execute_batch(db::SCHEMA_V38_UNAPPLIED_FACTS_WIDEN_SQL)
        .unwrap();
    let err = insert("no_payment_intent", "still_early")
        .unwrap_err()
        .to_string();
    assert!(err.contains("CHECK"), "v38 still refuses it: {err}");
    conn.execute_batch(db::SCHEMA_V42_UNAPPLIED_FACTS_WIDEN_SQL)
        .unwrap();
    let kept: i64 = conn
        .query_row("SELECT COUNT(*) FROM stripe_unapplied_facts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(kept, 3, "rows survive the rebuild");
    insert("no_payment_intent", "re_nopi").unwrap();
    let err = insert("bogus", "re_nopi").unwrap_err().to_string();
    assert!(err.contains("CHECK"), "closed set: {err}");
    let err = insert("no_payment_intent", "re_nopi")
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
fn rf_pi_a_fresh_farm_opens_with_the_word_admitted() {
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
         VALUES ('ev_rf_pi_fresh', 'refund', 're_fresh', 'no_payment_intent', NULL, NULL, 1,
                 '2026-08-30T12:00:00.000Z')",
        [],
    )
    .expect("the migration admits the word on a fresh farm");
}
#[test]
fn rf_pi_money_prints_the_signed_sentence_never_the_token() {
    let money_src = read("../src/screens/Money.tsx");
    let sentence = "A refund arrived that Farm OS couldn't match to a payment. Nothing was changed — use Reverse payment if the money went back.";
    assert_eq!(
        money_src.matches(sentence).count(),
        1,
        "law 5: the signed sentence, byte for byte, exactly once"
    );
    assert!(
        !sentence.contains("order"),
        "law 5: never speak of an order that does not exist"
    );
    let arm = money_src
        .find(r#"status === "no_payment_intent""#)
        .expect("Money.tsx lost the no_payment_intent arm");
    let fallback = money_src
        .find("\n  return status;")
        .expect("Money.tsx lost unappliedSentence's raw-token fallback");
    assert!(
        arm < fallback,
        "law 5: the token is named before the raw-token fallback, never as the primary line"
    );
    assert_eq!(money_src.matches("function unappliedSentence").count(), 1);
}
