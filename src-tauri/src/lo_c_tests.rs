//! LO-C (GT-D24-C) — the leftover payment link, drawn.
//!
//! One read, no write: `payment_link_url` on the listing, unchanged, becomes
//! a boolean matrix the desk draws as `<rect>`s. The encoder and its pinned
//! fixed-URL matrix live in till_b_tests — one encoder, pinned once. Pinned
//! here is the leftover door: row → URL bytes → the same modules, a refusal
//! when no link is stored, and that the read writes nothing.
use crate::db;
use crate::leftover;
use crate::money::fake::FakeGateway;
use crate::trays;
use crate::wholesale;
use rusqlite::Connection;
/// A stored `payment_link_url` in the shape LO-B mints: the link, then
/// `?client_reference_id=lo-<listing id>`. Same byte length as till_b's
/// fixed URL, so level M picks the same version: 8, 49 modules a side.
const FIXED_LO_URL: &str = "https://buy.stripe.com/test/lo/1c9e4a7b-5d20-4f8a-b3c6-2a81d7e94f00?client_reference_id=lo-1c9e4a7b-5d20-4f8a-b3c6-2a81d7e94f00";
const FINDER: [&str; 7] = [
    "#######", "#.....#", "#.###.#", "#.###.#", "#.###.#", "#.....#", "#######",
];
fn mem() -> Connection {
    db::open_in_memory().unwrap()
}
fn today() -> String {
    db::local_date_today()
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
fn drawn(rows: &[&str]) -> Vec<Vec<bool>> {
    rows.iter()
        .map(|row| row.chars().map(|c| c == '#').collect())
        .collect()
}
#[test]
fn lo_c_fixed_lo_url_same_matrix_and_another_url_another_matrix() {
    let first = wholesale::qr_modules(FIXED_LO_URL).unwrap();
    let second = wholesale::qr_modules(FIXED_LO_URL).unwrap();
    assert_eq!(first, second);
    let other = FIXED_LO_URL.replace("4f00", "4f01");
    assert_ne!(other, FIXED_LO_URL);
    assert_ne!(wholesale::qr_modules(&other).unwrap(), first);
}
#[test]
fn lo_c_matrix_is_a_square_qr_symbol() {
    let m = wholesale::qr_modules(FIXED_LO_URL).unwrap();
    let n = m.len();
    assert_eq!(n, 49);
    assert!(m.iter().all(|row| row.len() == n));
    assert_eq!((n - 17) % 4, 0);
    let finder = drawn(&FINDER);
    let block = |r0: usize, c0: usize| -> Vec<Vec<bool>> {
        (0..7)
            .map(|i| (0..7).map(|j| m[r0 + i][c0 + j]).collect())
            .collect()
    };
    assert_eq!(block(0, 0), finder);
    assert_eq!(block(0, n - 7), finder);
    assert_eq!(block(n - 7, 0), finder);
    let timing_row: Vec<bool> = (8..n - 8).map(|i| m[6][i]).collect();
    let timing_column: Vec<bool> = (8..n - 8).map(|i| m[i][6]).collect();
    let alternating: Vec<bool> = (8..n - 8).map(|i| i % 2 == 0).collect();
    assert_eq!(timing_row, alternating);
    assert_eq!(timing_column, alternating);
    assert!(m[n - 8][8], "the dark module beside the bottom-left finder");
}
#[test]
fn lo_c_the_row_is_drawn_unchanged_and_the_read_writes_nothing() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    let minted =
        leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let url = minted.payment_link_url.clone().unwrap();
    assert!(url.contains("?client_reference_id=lo-"), "{url}");
    let events_before = count_table(&conn, "event_log");
    let from_row = leftover::payment_link_qr_modules(&conn, &listing.listing_id).unwrap();
    assert_eq!(from_row, wholesale::qr_modules(&url).unwrap());
    assert_eq!(count_table(&conn, "event_log"), events_before);
    let again = &leftover::listings(&conn).unwrap()[0];
    assert_eq!(again.payment_link_url, minted.payment_link_url);
    assert_eq!(again.payment_link_id, minted.payment_link_id);
    assert!(again.paid_at.is_none());
}
#[test]
fn lo_c_no_link_no_picture() {
    let mut conn = mem();
    let listing = listed_kale(&mut conn);
    let err = leftover::payment_link_qr_modules(&conn, &listing.listing_id).unwrap_err();
    assert!(err.contains("no payment link"), "{err}");
    let missing = leftover::payment_link_qr_modules(&conn, "no-such-listing").unwrap_err();
    assert!(missing.contains("not found"), "{missing}");
}
