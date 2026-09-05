//! Wave 1 — storefront reader proofs (s1–s5). Fixtures from disk; no network.

use crate::db;
use crate::storefront::{
    consecutive_failures, latest_ok_reading, parse_storefront_html, parse_storefront_text,
    record_observation, SOLD_OUT_TEXT, STOREFRONT_URL,
};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

#[test]
fn s1_good_fixture_parses_five_sold_out() {
    let html = fixture("storefront-2026-08-11.html");
    let varieties = parse_storefront_html(&html).expect("good fixture must parse");
    assert_eq!(varieties.len(), 5);
    for v in &varieties {
        assert!(
            v.price.starts_with('$') && v.price.ends_with(" / oz"),
            "price shape: {}",
            v.price
        );
        assert_eq!(v.availability, SOLD_OUT_TEXT);
    }
    let names: Vec<_> = varieties.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "Dun Peas",
            "Mellow Micro Mix",
            "Purple Kohlrabi",
            "Red Arrow Radish",
            "Spicy Micro Mix",
        ]
    );
}

#[test]
fn s2_broken_fixture_names_purple_kohlrabi() {
    let html = fixture("storefront-broken.html");
    let err = parse_storefront_html(&html).unwrap_err();
    assert!(
        err.contains("Purple Kohlrabi"),
        "error must name Purple Kohlrabi: {err}"
    );
    assert!(
        err.contains("not found") || err.contains("of 5"),
        "error must be a structural miss: {err}"
    );
}

#[test]
fn s3_other_availability_is_verbatim_and_ok() {
    let text = "\
Dun Peas $2.00 / oz In stock — 12 oz left
Mellow Micro Mix $2.00 / oz Low stock
Purple Kohlrabi $2.25 / oz A few left this morning
Red Arrow Radish $2.25 / oz Plenty
Spicy Micro Mix $2.50 / oz Ask at the table
";
    let varieties = parse_storefront_text(text).expect("Other wording is not a structural miss");
    assert_eq!(varieties.len(), 5);
    assert_eq!(varieties[0].availability, "In stock — 12 oz left");
    assert_eq!(varieties[1].availability, "Low stock");
    assert_ne!(varieties[0].availability, SOLD_OUT_TEXT);
}

#[test]
fn s4_fetch_failure_keeps_previous_ok_reading_and_age() {
    let conn = mem();
    let html = fixture("storefront-2026-08-11.html");
    let varieties = parse_storefront_html(&html).unwrap();
    let fetched_at = "2026-08-10T12:00:00.000Z";
    record_observation(
        &conn,
        fetched_at,
        STOREFRONT_URL,
        Some(200),
        Some("abc"),
        true,
        Some(&varieties),
        None,
    )
    .unwrap();

    let fail_at = "2026-08-11T18:00:00.000Z";
    let fail_msg = "storefront request failed: pulled cable";
    record_observation(
        &conn,
        fail_at,
        STOREFRONT_URL,
        None,
        None,
        false,
        None,
        Some(fail_msg),
    )
    .unwrap();

    let err: String = conn
        .query_row(
            "SELECT error FROM storefront_observations WHERE ok = 0 ORDER BY fetched_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(err, fail_msg);

    let latest = latest_ok_reading(&conn)
        .unwrap()
        .expect("previous good reading");
    assert_eq!(latest.fetched_at, fetched_at);
    assert_eq!(latest.origin, STOREFRONT_URL);
    assert_eq!(latest.value.len(), 5);
}

#[test]
fn s5_consecutive_failures_counts_across_sequences() {
    let conn = mem();
    let html = fixture("storefront-2026-08-11.html");
    let varieties = parse_storefront_html(&html).unwrap();

    assert_eq!(consecutive_failures(&conn).unwrap(), 0);

    record_observation(
        &conn,
        "2026-08-10T10:00:00.000Z",
        STOREFRONT_URL,
        None,
        None,
        false,
        None,
        Some("fail-1"),
    )
    .unwrap();
    assert_eq!(consecutive_failures(&conn).unwrap(), 1);

    record_observation(
        &conn,
        "2026-08-10T11:00:00.000Z",
        STOREFRONT_URL,
        Some(200),
        Some("x"),
        true,
        Some(&varieties),
        None,
    )
    .unwrap();
    assert_eq!(consecutive_failures(&conn).unwrap(), 0);

    record_observation(
        &conn,
        "2026-08-10T12:00:00.000Z",
        STOREFRONT_URL,
        None,
        None,
        false,
        None,
        Some("fail-2"),
    )
    .unwrap();
    record_observation(
        &conn,
        "2026-08-10T13:00:00.000Z",
        STOREFRONT_URL,
        None,
        None,
        false,
        None,
        Some("fail-3"),
    )
    .unwrap();
    record_observation(
        &conn,
        "2026-08-10T14:00:00.000Z",
        STOREFRONT_URL,
        None,
        None,
        false,
        None,
        Some("fail-4"),
    )
    .unwrap();
    assert_eq!(consecutive_failures(&conn).unwrap(), 3);
}
