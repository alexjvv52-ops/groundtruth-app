//! INV-A — the render-only bill. What is pinned: the farm-name gate fires the
//! signed sentence; unpriced and voided refuse with no bill; both parents
//! render complete bill bodies from their existing view fields; the paid
//! stamp carries the parent's paid date; the read writes nothing. No literal
//! SCHEMA_VERSION here — H-11(b) keeps that pin in its three files.
use crate::db;
use crate::invoice::{
    self, INVOICE_NEEDS_FARM_NAME_LINE, INVOICE_UNPRICED_LINE, INVOICE_VOIDED_LINE,
};
use crate::leftover;
use crate::marketing;
use crate::money::fake::FakeGateway;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use rusqlite::Connection;
fn mem() -> Connection {
    db::open_in_memory().unwrap()
}
fn today() -> String {
    db::local_date_today()
}
fn set_name(conn: &Connection) {
    invoice::set_farm_display_name(conn, "Groundtruth Farm").unwrap();
}
fn venue(conn: &mut Connection) -> marketing::VenueView {
    marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap()
}
fn priced_order(conn: &mut Connection) -> wholesale::WholesaleOrderView {
    let v = venue(conn);
    wholesale::record_order(
        conn,
        &v.venue_id,
        &today(),
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 1,
            price_cents_per_tray: Some(600),
        }],
        false,
    )
    .unwrap()
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
fn count_events(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap()
}
#[test]
fn inv_a_empty_farm_name_refuses_with_the_signed_sentence() {
    let mut conn = mem();
    assert_eq!(invoice::farm_display_name(&conn).unwrap(), None);
    let order = priced_order(&mut conn);
    let err = invoice::wholesale_bill(&conn, &order.id).unwrap_err();
    assert_eq!(err, INVOICE_NEEDS_FARM_NAME_LINE);
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let err = invoice::leftover_bill(&conn, &listing.listing_id).unwrap_err();
    assert_eq!(err, INVOICE_NEEDS_FARM_NAME_LINE);
    invoice::set_farm_display_name(&conn, "   ").unwrap();
    let err = invoice::wholesale_bill(&conn, &order.id).unwrap_err();
    assert_eq!(err, INVOICE_NEEDS_FARM_NAME_LINE);
    set_name(&conn);
    assert!(invoice::wholesale_bill(&conn, &order.id).is_ok());
}
#[test]
fn inv_a_unpriced_refuses() {
    let mut conn = mem();
    set_name(&conn);
    let listing = listed_kale(&mut conn);
    let err = invoice::leftover_bill(&conn, &listing.listing_id).unwrap_err();
    assert_eq!(err, INVOICE_UNPRICED_LINE);
}
#[test]
fn inv_a_voided_refuses() {
    let mut conn = mem();
    set_name(&conn);
    let order = priced_order(&mut conn);
    wholesale::void_order(&mut conn, &order.id, Some("wrong".to_string())).unwrap();
    let err = invoice::wholesale_bill(&conn, &order.id).unwrap_err();
    assert_eq!(err, INVOICE_VOIDED_LINE);
    let missing = invoice::wholesale_bill(&conn, "no-such-order").unwrap_err();
    assert!(missing.contains("not found"), "{missing}");
}
#[test]
fn inv_a_wholesale_bill_fields_and_the_read_writes_nothing() {
    let mut conn = mem();
    set_name(&conn);
    let order = priced_order(&mut conn);
    let order = wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    let gw = FakeGateway::new();
    wholesale::mint_payment_link_with(&mut conn, &gw, &order.id).unwrap();
    let before = count_events(&conn);
    let bill = invoice::wholesale_bill(&conn, &order.id).unwrap();
    assert_eq!(count_events(&conn), before);
    assert_eq!(bill.farm_name, "Groundtruth Farm");
    assert_eq!(bill.number, order.id);
    assert_eq!(bill.parent, "wholesale");
    assert_eq!(bill.venue.as_ref().unwrap().name, "Fixture Cafe");
    assert_eq!(bill.order_lines.len(), 1);
    assert_eq!(bill.order_lines[0].crop_name, "Kale");
    assert_eq!(bill.order_lines[0].trays, 1);
    assert_eq!(bill.order_lines[0].price_cents_per_tray, 600);
    assert_eq!(bill.order_lines[0].line_total_cents, 600);
    assert_eq!(bill.total_cents, 600);
    assert_eq!(bill.harvest_date, order.harvest_date);
    assert!(bill.delivered_on.is_some());
    assert!(bill.paid_on.is_none());
    assert!(bill.leftover_line.is_none());
    let url = bill.payment_link_url.unwrap();
    assert!(url.contains("wo-"), "{url}");
}
#[test]
fn inv_a_leftover_bill_fields() {
    let mut conn = mem();
    set_name(&conn);
    let listing = listed_kale(&mut conn);
    let gw = FakeGateway::new();
    leftover::mint_payment_link_with(&mut conn, &gw, &listing.listing_id, 700).unwrap();
    let bill = invoice::leftover_bill(&conn, &listing.listing_id).unwrap();
    assert_eq!(bill.number, listing.listing_id);
    assert_eq!(bill.parent, "leftover");
    assert!(bill.venue.is_none());
    assert!(bill.order_lines.is_empty());
    let line = bill.leftover_line.as_ref().unwrap();
    assert_eq!(line.crop_name, "Kale");
    assert_eq!(line.harvested_on, listing.harvested_on);
    assert!((line.listed_oz - 2.0).abs() < 1e-9);
    assert_eq!(bill.total_cents, 700);
    assert!(bill.paid_on.is_none());
    let url = bill.payment_link_url.unwrap();
    assert!(url.contains("lo-"), "{url}");
}
#[test]
fn inv_a_paid_parent_still_bills_with_the_paid_stamp() {
    let mut conn = mem();
    set_name(&conn);
    let listing = listed_kale(&mut conn);
    leftover::pay_listing_cash(&mut conn, &listing.listing_id, 500, &today(), None).unwrap();
    let bill = invoice::leftover_bill(&conn, &listing.listing_id).unwrap();
    assert_eq!(bill.total_cents, 500);
    assert_eq!(bill.paid_on.as_deref(), Some(today().as_str()));
    let order = priced_order(&mut conn);
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    wholesale::pay_order(
        &mut conn,
        &order.id,
        600,
        &today(),
        None,
        false,
        false,
        None,
    )
    .unwrap();
    let bill = invoice::wholesale_bill(&conn, &order.id).unwrap();
    assert_eq!(bill.paid_on.as_deref(), Some(today().as_str()));
    assert_eq!(bill.total_cents, 600);
}
