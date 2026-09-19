//! The mint posts THE FARM'S currency (GT-D26 WORLD-PAY), sealed to usd and
//! cad this chip; cad is still history at the seal; nothing mints a literal.

use crate::db;
use crate::events::{EventRecord, Kind};
use crate::invoice;
use crate::leftover::{self, LeftoverLinkMintedPayload, LeftoverListingView};
use crate::marketing;
use crate::money::fake::FakeGateway;
use crate::money::{Offer, OrderBill, StripeGateway};
use crate::shop;
use crate::stripe_client::{fake_http::FakeHttp, StripeClient};
use crate::trays;
use crate::wholesale::{self, LinkMintedPayload, OrderLine, WholesaleOrderView};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn price_form_currency(forms: &[(String, Vec<(String, String)>)]) -> String {
    let (_, form) = forms
        .iter()
        .find(|(path, _)| path == "/v1/prices")
        .expect("prices form");
    let (_, value) = form
        .iter()
        .find(|(key, _)| key == "currency")
        .expect("prices form carries a currency");
    value.clone()
}

#[test]
fn usd_order_payment_link_price_is_minted_in_usd() {
    let http = FakeHttp::new();
    http.push_post("/v1/products", json!({"id": "prod_usd"}));
    http.push_post("/v1/prices", json!({"id": "price_usd"}));
    http.push_post(
        "/v1/payment_links",
        json!({"id": "plink_usd", "url": "https://buy.stripe.com/test_usd"}),
    );
    let client = StripeClient::new(http, "test");
    let minted = client
        .create_order_payment_link(&OrderBill {
            order_id: "wo1".into(),
            venue_name: "Fixture Cafe".into(),
            harvest_date: "2026-09-08".into(),
            amount_cents: 600,
            client_reference: "wo-wo1".into(),
        })
        .unwrap();
    assert_eq!(minted.link_id, "plink_usd");
    assert_eq!(minted.currency, "usd");
    let forms = client.http().post_forms.lock().unwrap();
    assert_eq!(price_form_currency(&forms), "usd");
}

#[test]
fn cad_order_payment_link_price_is_minted_in_cad() {
    let http = FakeHttp::new();
    http.push_post("/v1/products", json!({"id": "prod_usd"}));
    http.push_post("/v1/prices", json!({"id": "price_usd"}));
    http.push_post(
        "/v1/payment_links",
        json!({"id": "plink_usd", "url": "https://buy.stripe.com/test_usd"}),
    );
    let client = StripeClient::new(http, "test").with_currency("cad");
    let minted = client
        .create_order_payment_link(&OrderBill {
            order_id: "wo1".into(),
            venue_name: "Fixture Cafe".into(),
            harvest_date: "2026-09-08".into(),
            amount_cents: 600,
            client_reference: "wo-wo1".into(),
        })
        .unwrap();
    assert_eq!(minted.link_id, "plink_usd");
    assert_eq!(minted.currency, "cad");
    let forms = client.http().post_forms.lock().unwrap();
    assert_eq!(price_form_currency(&forms), "cad");
}

#[test]
fn usd_retail_offer_price_is_minted_in_usd() {
    let http = FakeHttp::new();
    http.push_post("/v1/products", json!({"id": "prod_offer"}));
    http.push_post("/v1/prices", json!({"id": "price_offer"}));
    let client = StripeClient::new(http, "test");
    let price_id = client
        .create_price(&Offer {
            id: "offer1".into(),
            harvest_date: "2026-09-08".into(),
            crop_id: "kale".into(),
            price_cents: 1200,
            stripe_price_id: None,
            stripe_link_id: None,
            stripe_link_url: None,
            created_at: "2026-09-08T12:00:00.000Z".into(),
        })
        .unwrap();
    assert_eq!(price_id, "price_offer");
    let forms = client.http().post_forms.lock().unwrap();
    assert_eq!(price_form_currency(&forms), "usd");
}

#[test]
fn cad_retail_offer_price_is_minted_in_cad() {
    let http = FakeHttp::new();
    http.push_post("/v1/products", json!({"id": "prod_offer"}));
    http.push_post("/v1/prices", json!({"id": "price_offer"}));
    let client = StripeClient::new(http, "test").with_currency("cad");
    let price_id = client
        .create_price(&Offer {
            id: "offer1".into(),
            harvest_date: "2026-09-08".into(),
            crop_id: "kale".into(),
            price_cents: 1200,
            stripe_price_id: None,
            stripe_link_id: None,
            stripe_link_url: None,
            created_at: "2026-09-08T12:00:00.000Z".into(),
        })
        .unwrap();
    assert_eq!(price_id, "price_offer");
    let forms = client.http().post_forms.lock().unwrap();
    assert_eq!(price_form_currency(&forms), "cad");
}

fn wholesale_minted(currency: &str) -> EventRecord {
    let payload = LinkMintedPayload {
        order_id: "wo1".into(),
        payment_link_id: "plink_1".into(),
        payment_link_url: "https://buy.stripe.com/test_1?client_reference_id=wo-wo1".into(),
        client_reference: wholesale::client_reference_for("wo1"),
        amount_cents: 600,
        currency: currency.into(),
        minted_on: "2026-09-08".into(),
    };
    EventRecord::originated(
        Kind::WholesaleLinkMinted,
        "wholesale_order",
        "wo1",
        serde_json::to_value(&payload).unwrap(),
        json!({ "op": "none" }),
        "2026-09-08T12:00:00.000Z",
        None,
        None,
        None,
    )
}

fn leftover_minted(currency: &str) -> EventRecord {
    let payload = LeftoverLinkMintedPayload {
        listing_id: "lo1".into(),
        payment_link_id: "plink_2".into(),
        payment_link_url: "https://buy.stripe.com/test_2?client_reference_id=lo-lo1".into(),
        client_reference: leftover::client_reference_for_listing("lo1"),
        priced_total_cents: 700,
        currency: currency.into(),
        minted_on: "2026-09-08".into(),
    };
    EventRecord::originated(
        Kind::LeftoverLinkMinted,
        "leftover_listing",
        "lo1",
        serde_json::to_value(&payload).unwrap(),
        json!({ "op": "none" }),
        "2026-09-08T12:00:00.000Z",
        None,
        None,
        None,
    )
}

#[test]
fn usd_seal_accepts_usd_and_cad_history_and_refuses_a_third_currency() {
    assert!(crate::currency::is_sealed("usd") && crate::currency::is_sealed("cad"));
    assert!(wholesale::validate_wholesale_event(&wholesale_minted("usd")).is_ok());
    assert!(wholesale::validate_wholesale_event(&wholesale_minted("cad")).is_ok());
    assert!(wholesale::validate_wholesale_event(&wholesale_minted("eur")).is_ok());
    assert!(leftover::validate_leftover_event(&leftover_minted("usd")).is_ok());
    assert!(leftover::validate_leftover_event(&leftover_minted("cad")).is_ok());
    assert!(leftover::validate_leftover_event(&leftover_minted("eur")).is_ok());
    for no in ["krw", "vnd", "xxx"] {
        let err = wholesale::validate_wholesale_event(&wholesale_minted(no)).unwrap_err();
        assert!(err.contains(&crate::currency::sealed_codes_line()), "{err}");
        let err = leftover::validate_leftover_event(&leftover_minted(no)).unwrap_err();
        assert!(err.contains(&crate::currency::sealed_codes_line()), "{err}");
    }
}

#[test]
fn usd_nothing_mints_cad() {
    for (rel, needle) in [
        ("src/stripe_client.rs", "(\"currency\", \"cad\")"),
        ("src/stripe_client.rs", "(\"currency\", \"usd\")"),
        ("src/wholesale.rs", "currency: \"cad\""),
        ("src/wholesale.rs", "currency: \"usd\""),
        ("src/leftover.rs", "currency: \"cad\""),
        ("src/leftover.rs", "currency: \"usd\""),
        ("src/shop.rs", "currency:'cad'"),
        ("src/shop.rs", "currency:'usd'"),
    ] {
        assert_eq!(
            read(rel).matches(needle).count(),
            0,
            "{rel} still mints a literal"
        );
    }
    let mint = read("src/stripe_client.rs");
    assert_eq!(
        mint.matches("(\"currency\", self.currency.as_str())")
            .count(),
        2,
        "two mint sites, both the farm's currency"
    );
}

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

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-shop-door-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn set_name(conn: &Connection) {
    invoice::set_farm_display_name(conn, "Groundtruth Farm").unwrap();
}

/// Two trays of one crop harvested today at 6.0 oz, listed at 2.0 oz.
fn listed(conn: &mut Connection, crop_id: &str) -> LeftoverListingView {
    let mut ids = Vec::new();
    for _ in 0..2 {
        let t = trays::sow_tray(conn, crop_id, 1).unwrap();
        trays::advance_tray(conn, &t.id).unwrap();
        ids.push(t.id);
    }
    trays::harvest_trays(conn, &ids, 6.0).unwrap();
    leftover::list_leftover(conn, crop_id, &db::local_date_today(), 2.0).unwrap()
}

fn minted_lot(conn: &mut Connection, crop: &str, cents: i64) -> LeftoverListingView {
    let lot = listed(conn, crop);
    leftover::mint_payment_link_with(conn, &FakeGateway::new(), &lot.listing_id, cents).unwrap()
}

fn page_html(conn: &Connection, dir: &Path) -> String {
    let written = shop::write_leftover_shop_page(conn, dir).unwrap();
    fs::read_to_string(&written.file_path).unwrap()
}

#[test]
fn link_code_wholesale_view_carries_the_minted_code_not_the_farm_pick() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    crate::currency::set_farm_currency(&conn, "zar").unwrap();
    assert_eq!(crate::currency::farm_currency(&conn).unwrap(), "zar");
    let again = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(
        again.minted_currency.as_deref(),
        Some(crate::currency::DEFAULT_CURRENCY)
    );
    assert!(again.payment_link_url.is_some());
}

#[test]
fn link_code_wholesale_view_is_none_without_a_mint() {
    let mut conn = mem();
    let order = priced_delivered(&mut conn);
    let gw = FakeGateway::new();
    let again = wholesale::get_order(&conn, &order.id).unwrap();
    assert_eq!(again.minted_currency, None);
    assert!(gw.state.lock().unwrap().order_links_created.is_empty());
}

#[test]
fn link_code_leftover_view_carries_the_minted_code_not_the_farm_pick() {
    let mut conn = mem();
    let lot = minted_lot(&mut conn, "kale", 700);
    crate::currency::set_farm_currency(&conn, "zar").unwrap();
    assert_eq!(
        leftover::get_listing(&conn, &lot.listing_id)
            .unwrap()
            .minted_currency
            .as_deref(),
        Some(crate::currency::DEFAULT_CURRENCY)
    );
    let unminted = listed(&mut conn, "broccoli");
    let found = leftover::listings(&conn)
        .unwrap()
        .into_iter()
        .find(|l| l.listing_id == unminted.listing_id)
        .unwrap();
    assert_eq!(found.minted_currency, None);
}

#[test]
fn link_code_shop_lot_names_the_minted_code_never_the_settings_pick() {
    let mut conn = mem();
    set_name(&conn);
    minted_lot(&mut conn, "kale", 700);
    crate::currency::set_farm_currency(&conn, "zar").unwrap();
    let html = page_html(&conn, &temp_dir("link-code"));
    let row = format!(
        "Kale · harvested {} · 2.0 oz · R7.00 · USD",
        db::local_date_today()
    );
    assert_eq!(html.matches(&row).count(), 1, "{html}");
    assert!(!html.contains("ZAR"), "{html}");
    assert_eq!(html.matches(" · USD").count(), 1);
    assert_eq!(html.matches("Pay online").count(), 1);
}

#[test]
fn link_code_shop_word_prints_nothing_without_a_sealed_minted_code() {
    assert_eq!(crate::shop::lot_iso_word(None), "");
    assert_eq!(crate::shop::lot_iso_word(Some("xxx")), "");
    assert_eq!(crate::shop::lot_iso_word(Some("")), "");
    assert_eq!(crate::shop::lot_iso_word(Some("usd")), " · USD");
    assert_eq!(crate::shop::lot_iso_word(Some("ZAR")), " · ZAR");
}
