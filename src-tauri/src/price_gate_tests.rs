//! J3 PRICE-GATE — a dropped Price id is archived after the row write,
//! best-effort like J1; attribution still never reads price/session metadata.

use crate::db;
use crate::money::{self, fake::FakeGateway, StripeGateway};
use crate::offers;
use crate::stripe_client::{fake_http::FakeHttp, StripeClient};
use crate::trays;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::Path;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn harvest_for(conn: &mut Connection, crop: &str) -> String {
    let t = trays::sow_tray(conn, crop, 2).unwrap();
    trays::get_tray(conn, &t.id)
        .unwrap()
        .expected_harvest_date
        .expect("expected harvest date")
}

fn price_in_row(conn: &Connection, offer_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT stripe_price_id FROM offers WHERE id = ?1",
        [offer_id],
        |r| r.get(0),
    )
    .ok()
    .flatten()
}

fn archived(gw: &FakeGateway) -> Vec<String> {
    gw.state.lock().unwrap().archived_prices.clone()
}

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

#[test]
fn j3_first_offer_archives_nothing() {
    let mut conn = mem();
    let gw = FakeGateway::new();
    let hd = harvest_for(&mut conn, "dun-peas");
    let view = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();
    assert!(archived(&gw).is_empty());
    let offer_id = view.id.as_ref().expect("offer id");
    let price = price_in_row(&conn, offer_id);
    assert!(price.is_some());
    assert_eq!(price, view.stripe_price_id);
}

#[test]
fn j3_set_offer_overwrite_archives_the_old_price_after_the_row_write() {
    let mut conn = mem();
    let gw = FakeGateway::new();
    let hd = harvest_for(&mut conn, "dun-peas");
    let first = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();
    let offer_id = first.id.as_ref().expect("offer id").clone();
    let a = price_in_row(&conn, &offer_id).expect("first price");
    let second = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1500).unwrap();
    let b = second.stripe_price_id.clone().expect("second price");
    assert_ne!(a, b);
    assert_eq!(price_in_row(&conn, &offer_id), Some(b));
    assert_eq!(archived(&gw), vec![a]);
    assert_eq!(gw.state.lock().unwrap().prices_created.len(), 2);
}

#[test]
fn j3_remove_offer_archives_the_price_it_held_then_nulls_the_row() {
    let mut conn = mem();
    let gw = FakeGateway::new();
    let hd = harvest_for(&mut conn, "dun-peas");
    let view = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();
    let offer_id = view.id.as_ref().expect("offer id").clone();
    let a = view.stripe_price_id.clone().expect("price");
    offers::remove_offer_with(&mut conn, &gw, &offer_id).unwrap();
    assert_eq!(archived(&gw), vec![a]);
    assert_eq!(price_in_row(&conn, &offer_id), None);
    assert!(offers::shop_listings(&conn).unwrap().is_empty());
}

#[test]
fn j3_remove_offer_twice_archives_once() {
    let mut conn = mem();
    let gw = FakeGateway::new();
    let hd = harvest_for(&mut conn, "dun-peas");
    let view = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();
    let offer_id = view.id.as_ref().expect("offer id").clone();
    offers::remove_offer_with(&mut conn, &gw, &offer_id).unwrap();
    offers::remove_offer_with(&mut conn, &gw, &offer_id).unwrap();
    assert_eq!(archived(&gw).len(), 1);
}

#[test]
fn j3_archive_error_never_rolls_back_set_offer_or_remove_offer() {
    let mut conn = mem();
    let gw = FakeGateway::new();
    gw.fail_archive("stripe down");
    let hd = harvest_for(&mut conn, "dun-peas");
    let first = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();
    let offer_id = first.id.as_ref().expect("offer id").clone();
    let second = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1500).unwrap();
    let b = second.stripe_price_id.clone().expect("second price");
    assert_eq!(price_in_row(&conn, &offer_id), Some(b));
    assert!(archived(&gw).is_empty());
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM offers WHERE harvest_date = ?1 AND crop_id = ?2",
            [hd.as_str(), "dun-peas"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
    offers::remove_offer_with(&mut conn, &gw, &offer_id).unwrap();
    assert_eq!(price_in_row(&conn, &offer_id), None);
    assert!(archived(&gw).is_empty());
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM offers WHERE harvest_date = ?1 AND crop_id = ?2",
            [hd.as_str(), "dun-peas"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn j3_retire_harvest_links_archives_no_price() {
    let mut conn = mem();
    let gw = FakeGateway::new();
    let hd = harvest_for(&mut conn, "dun-peas");
    let view = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();
    let offer_id = view.id.as_ref().expect("offer id").clone();
    let a = view.stripe_price_id.clone().expect("price");
    offers::retire_harvest_links(&mut conn, &gw).unwrap();
    assert!(archived(&gw).is_empty());
    assert_eq!(price_in_row(&conn, &offer_id), Some(a));
    assert!(gw.state.lock().unwrap().deactivated_links.is_empty());
}

#[test]
fn j3_stripe_client_archive_price_posts_active_false() {
    let http = FakeHttp::new();
    http.push_post(
        "/v1/prices/price_j3",
        json!({"id": "price_j3", "active": false}),
    );
    let client = StripeClient::new(http, "test");
    client.archive_price("price_j3").unwrap();
    let forms = client.http().post_forms.lock().unwrap();
    let want: (String, Vec<(String, String)>) = (
        "/v1/prices/price_j3".into(),
        vec![("active".into(), "false".into())],
    );
    assert!(forms.contains(&want));
    drop(forms);
    assert_eq!(client.http().posts.lock().unwrap().len(), 1);
}

#[test]
fn j3_attribution_never_reads_price_or_session_metadata() {
    for file in ["src/money.rs", "src/stripe_client.rs", "src/poll.rs"] {
        let src = read(file);
        for banned in [
            "get(\"metadata\")",
            "/metadata",
            "[\"metadata\"]",
            "metadata.get(",
            ".metadata",
            "pub metadata",
            "metadata:",
        ] {
            assert_eq!(
                src.matches(banned).count(),
                0,
                "{file} still names {banned}"
            );
        }
    }
    let src = read("src/money.rs");
    assert_eq!(src.matches("WHERE stripe_price_id = ?1").count(), 1);
    assert!(src.contains("pub struct PaidSession"));
}
