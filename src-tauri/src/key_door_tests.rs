//! GT-D23 — the desk key door: paste → preview → confirm on Money.
//!
//! Two pins the board asked for: the missing-key sentence names the Money
//! door and no longer names Sell online; money_status never returns the key.
//! A third walks the click that found the hole: Payment link with no key.

use crate::db;
use crate::marketing;
use crate::money::{self, AccountInfo};
use crate::wholesale::{self, OrderLine};
use rusqlite::Connection;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn count_table(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn key_door_missing_key_sentence_names_the_money_door() {
    let conn = mem();
    let err = money::gateway_from_db(&conn)
        .err()
        .expect("a fresh farm has no key");
    assert_eq!(err, money::STRIPE_NOT_CONNECTED_LINE);
    assert!(err.contains("Connect Stripe"), "{err}");
    assert!(err.contains("Money"), "{err}");
    assert!(!err.contains("Sell online"), "{err}");
}

#[test]
fn key_door_payment_link_without_a_key_returns_the_sentence_and_writes_nothing() {
    let mut conn = mem();
    let v =
        marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let harvest = db::local_date_today();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 1,
            price_cents_per_tray: Some(600),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    let events_before = count_table(&conn, "event_log");
    let err = wholesale::mint_payment_link(&mut conn, &order.id).unwrap_err();
    assert_eq!(err, money::STRIPE_NOT_CONNECTED_LINE);
    assert_eq!(count_table(&conn, "event_log"), events_before);
    let again = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(again.state, "delivered");
    assert!(again.payment_link_url.is_none());
}

#[test]
fn key_door_money_status_never_returns_the_key() {
    let conn = mem();
    assert!(!money::money_status(&conn).unwrap().configured);
    let key = "rk_test_key_door_unit_key_001";
    money::store_stripe_key(
        &conn,
        key,
        &AccountInfo {
            account_id: "acct_key_door".into(),
            account_name: "Key Door Farm".into(),
            mode: "test".into(),
        },
    )
    .unwrap();
    let status = money::money_status(&conn).unwrap();
    assert!(status.configured);
    assert_eq!(status.mode.as_deref(), Some("test"));
    assert_eq!(status.account_name.as_deref(), Some("Key Door Farm"));
    let wire = serde_json::to_string(&status).unwrap();
    assert!(!wire.contains(key), "{wire}");
    assert!(!wire.contains("rk_"), "{wire}");
    assert!(money::gateway_from_db(&conn).is_ok());
}
