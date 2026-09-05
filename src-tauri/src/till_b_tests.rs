//! TILL-B (GT-D22-B) — the stored payment link, drawn.
//!
//! One read, no write: `payment_link_url` on the row, unchanged, becomes a
//! boolean matrix the desk draws as `<rect>`s. The fixed-URL matrix below is
//! pinned so a crate move or a changed input fails here, not on the desk.

use crate::db;
use crate::marketing;
use crate::money::fake::FakeGateway;
use crate::wholesale::{self, OrderLine, WholesaleOrderView};
use rusqlite::Connection;

/// A stored `payment_link_url` in the shape TILL-A writes: the minted link,
/// then `?client_reference_id=wo-<order id>`.
const FIXED_URL: &str = "https://buy.stripe.com/test/wo/0f2b6f8e-2c53-4d6a-9d1e-7b3c9a5e1f00?client_reference_id=wo-0f2b6f8e-2c53-4d6a-9d1e-7b3c9a5e1f00";

/// qrcode 0.14.1, error level M, for FIXED_URL: version 8, 49 modules a side.
/// `#` is a dark module, `.` a light one, row by row from the top.
const FIXED_URL_MODULES: [&str; 49] = [
    "#######..#.###...##.#...######..#.#.....#.#######",
    "#.....#....#.##.####.##.#...#.####.#.####.#.....#",
    "#.###.#.#....#.####...#.###.##.#####...##.#.###.#",
    "#.###.#.#####..####..#.#.###.##.....##.#..#.###.#",
    "#.###.#.#..###.############...#.#..###....#.###.#",
    "#.....#.#...#.#.#...#.#...#....#..##.##...#.....#",
    "#######.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#######",
    "........#.#..#.##..##.#...#.#..##....#...........",
    "#.#####...#...#..#..########.##....##.###.#####..",
    "..##...#..##..#.####.....#.#######.###...###.....",
    "#.#.#.##..##.##.##.#.###.##......##.#.#.....#...#",
    ".#####.#####.####......#...##...##....##....#..##",
    "#####.#..###....##.#.##.#..#..##.#.###.##..#.####",
    "....##....#.#..#.##..#..#..#.####....#....##...#.",
    "###.#.#.##.##...#..#...####....####.#.#.#..#..###",
    "#..###.###.##..####..#........##.#.###..#.###...#",
    "##....#######.#...###..#####.##.....#.#.##...##..",
    "..####..##.####.##.....#......##.#.##..#..##...#.",
    "###...####..#..#..#..##.#.##.##.....####...#.#..#",
    "#...##.##...#.....##.#.#....##..##....#..#.##..##",
    "#..#..#.##.##.##.###..#.##.#.#.#.#..##..#.....##.",
    "##.#.#..#.###.#....#.###....###.#..#.....####.#..",
    ".##.#########.#.###..#######.#.#.###..#######..##",
    ".####...##.####..###..#...###.#.#.##....#...##...",
    ".#.##.#.##.#.#...#..###.#.#..###....##.##.#.#.#.#",
    "#..##...##..##.#...#..#...#...####..#..##...#....",
    ".########....#.##.#.#.######.#....###.#.#########",
    "....#..###..###....#.#.#.#..###.#..#.#..........#",
    ".#.#..#.##..#..##....#.###...###.####..##.#####..",
    ".###...#..#..#.#.#.####.###.#.####..#..####..#...",
    "#..##.####...#..###.#..###.#......#######.#######",
    ".....#..###.#..#..###...#.##.#....#.#..###...#.#.",
    ".####.##...#..###...#.#.....#..#####...###..#.##.",
    "##..#....#.#..##.#.#...##..###..#.#....###.#.....",
    "##.#.###.#......####.##.#...#.####.#.###..#.##..#",
    "#...#........#.##.#.##.##########....##.#......##",
    ".###..##...#.#...####....##...#....##.##.#.##.###",
    ".#...#..#.#....#.###.#.###.####.##.#.#.####..#.#.",
    ".#...##.###...#####.#.##...#.....#######..#######",
    ".###......##...##.####.#.##.#####....##.##.....#.",
    "###...#..#.....#..###.######.##...####.#########.",
    "........#.######.#...##...#..##.#..##...#...#..#.",
    "#######..#...#.#.##.#.#.#.#......##..##.#.#.#...#",
    "#.....#.#.#..##......##...#.##..##.#.##.#...#....",
    "#.###.#.#.###..#.#.#..#####..###.#####..#####.###",
    "#.###.#.#......#..#..##..##...#.#..#.#.##.####.#.",
    "#.###.#.#...##.#####..#.#.#..#.####.###..#.#..###",
    "#.....#...#....##......#.##...##.#..#....##.....#",
    "#######.#..###.##..###.#...#.##.....#........####",
];

const FINDER: [&str; 7] = [
    "#######", "#.....#", "#.###.#", "#.###.#", "#.###.#", "#.....#", "#######",
];

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
fn till_b_fixed_url_matrix_is_pinned() {
    let modules = wholesale::qr_modules(FIXED_URL).unwrap();
    assert_eq!(modules, drawn(&FIXED_URL_MODULES));
}

#[test]
fn till_b_same_url_same_matrix_and_another_url_another_matrix() {
    let first = wholesale::qr_modules(FIXED_URL).unwrap();
    let second = wholesale::qr_modules(FIXED_URL).unwrap();
    assert_eq!(first, second);
    let other = FIXED_URL.replace("1f00", "1f01");
    assert_ne!(other, FIXED_URL);
    assert_ne!(wholesale::qr_modules(&other).unwrap(), first);
}

#[test]
fn till_b_matrix_is_a_square_qr_symbol() {
    let m = wholesale::qr_modules(FIXED_URL).unwrap();
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
fn till_b_the_row_is_drawn_unchanged_and_the_read_writes_nothing() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    let minted = wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let url = minted.payment_link_url.clone().unwrap();
    assert!(url.contains("?client_reference_id=wo-"), "{url}");
    let events_before = count_table(&conn, "event_log");
    let from_row = wholesale::payment_link_qr_modules(&conn, &order.id).unwrap();
    assert_eq!(from_row, wholesale::qr_modules(&url).unwrap());
    assert_eq!(count_table(&conn, "event_log"), events_before);
    let again = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(again.payment_link_url, minted.payment_link_url);
    assert_eq!(again.payment_link_id, minted.payment_link_id);
    assert_eq!(again.state, "delivered");
}

#[test]
fn till_b_no_link_no_picture() {
    let mut conn = mem();
    let order = priced_ordered(&mut conn);
    let err = wholesale::payment_link_qr_modules(&conn, &order.id).unwrap_err();
    assert!(err.contains("no payment link"), "{err}");
    let missing = wholesale::payment_link_qr_modules(&conn, "no-such-order").unwrap_err();
    assert!(missing.contains("not found"), "{missing}");
}

#[test]
fn till_b_too_long_is_an_error_not_a_panic() {
    let err = wholesale::qr_modules(&"a".repeat(3000)).unwrap_err();
    assert_eq!(err, "data too long");
}
