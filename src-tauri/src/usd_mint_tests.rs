//! USD-OR-CAD (GT-D13-USD) — the mint bills US dollars; cad is history.
//!
//! What is pinned: both Stripe mint sites post `currency=usd` (the Price
//! under a wholesale / leftover Payment Link and the retail offer Price); the
//! seal on `wholesale.link_minted` and `leftover.link_minted` accepts `usd`
//! and `cad` (SEAL CAD-OR-USD — every link minted before the flip replays and
//! imports unchanged) and refuses a third currency; nothing mints `cad`.

use crate::events::{EventRecord, Kind};
use crate::leftover::{self, LeftoverLinkMintedPayload};
use crate::money::{Offer, OrderBill, StripeGateway};
use crate::stripe_client::{fake_http::FakeHttp, StripeClient};
use crate::wholesale::{self, LinkMintedPayload};
use serde_json::json;
use std::fs;
use std::path::Path;

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
    let forms = client.http().post_forms.lock().unwrap();
    assert_eq!(price_form_currency(&forms), "usd");
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
    assert!(wholesale::validate_wholesale_event(&wholesale_minted("usd")).is_ok());
    assert!(wholesale::validate_wholesale_event(&wholesale_minted("cad")).is_ok());
    let err = wholesale::validate_wholesale_event(&wholesale_minted("eur")).unwrap_err();
    assert!(err.contains("usd or cad"), "{err}");
    assert!(leftover::validate_leftover_event(&leftover_minted("usd")).is_ok());
    assert!(leftover::validate_leftover_event(&leftover_minted("cad")).is_ok());
    let err = leftover::validate_leftover_event(&leftover_minted("eur")).unwrap_err();
    assert!(err.contains("usd or cad"), "{err}");
}

#[test]
fn usd_nothing_mints_cad() {
    for (rel, needle) in [
        ("src/stripe_client.rs", "(\"currency\", \"cad\")"),
        ("src/wholesale.rs", "currency: \"cad\""),
        ("src/leftover.rs", "currency: \"cad\""),
        ("src/shop.rs", "currency:'cad'"),
    ] {
        assert_eq!(
            read(rel).matches(needle).count(),
            0,
            "{rel} still mints cad"
        );
    }
    let mint = read("src/stripe_client.rs");
    assert_eq!(
        mint.matches("(\"currency\", \"usd\")").count(),
        2,
        "two mint sites, both usd"
    );
}
