//! Fence IV-a — capacity honesty, engine only.
//! Findings 02, 03, 04, 05. Signed readings D2(b), D3, D4(b), D5(1), D21a, D22a.

use crate::db;
use crate::marketing;
use crate::money;
use crate::reachability;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use chrono::{Duration, NaiveDate};
use rusqlite::Connection;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn add_days(date: &str, days: i64) -> String {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
    (d + Duration::days(days)).format("%Y-%m-%d").to_string()
}

fn venue(conn: &mut Connection) -> marketing::VenueView {
    marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap()
}

fn order_crop(
    conn: &mut Connection,
    venue_id: &str,
    harvest_date: &str,
    crop_id: &str,
    trays: i64,
) -> wholesale::WholesaleOrderView {
    wholesale::record_order(
        conn,
        venue_id,
        harvest_date,
        vec![OrderLine {
            crop_id: crop_id.into(),
            trays,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap()
}

fn cap_row<'a>(
    caps: &'a [crate::models::CapacityRow],
    date: &str,
    crop_id: &str,
) -> &'a crate::models::CapacityRow {
    caps.iter()
        .find(|r| r.harvest_date == date && r.crop_id == crop_id)
        .unwrap_or_else(|| panic!("missing capacity row ({date}, {crop_id})"))
}

fn seed_offer_price(
    conn: &Connection,
    harvest_date: &str,
    crop_id: &str,
    price_id: &str,
    cents: i64,
) {
    conn.execute(
        "INSERT INTO offers
         (id, harvest_date, crop_id, price_cents, stripe_price_id,
          stripe_link_id, stripe_link_url, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6)
         ON CONFLICT(harvest_date, crop_id) DO UPDATE SET
           stripe_price_id = excluded.stripe_price_id,
           price_cents = excluded.price_cents",
        rusqlite::params![
            format!("offer_{price_id}"),
            harvest_date,
            crop_id,
            cents,
            price_id,
            "2026-08-05T12:00:00.000Z"
        ],
    )
    .unwrap();
}

fn paid_session(
    conn: &Connection,
    session_id: &str,
    harvest_date: &str,
    crop_id: &str,
    qty: i64,
) -> money::PaidSession {
    let price_id = format!("price_{harvest_date}_{crop_id}");
    seed_offer_price(conn, harvest_date, crop_id, &price_id, 1200);
    money::PaidSession {
        session_id: session_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        lines: vec![money::PaidLine {
            price_id,
            quantity: qty,
            amount_cents: qty * 1200,
        }],
        currency: "cad".into(),
        customer_email: Some("buyer@example.com".into()),
        paid_at: "2026-08-05T12:00:00.000Z".into(),
        created: 1_700_000_000,
        amount_cents: qty * 1200,
        client_reference: None,
        payment_link: None,
    }
}

/// Finding 03. Same date, order N Sunflower, sow N Kale ready that date:
/// before this fence the date read covered. D3: a Kale tray never covers
/// a Sunflower promise.
#[test]
fn f4a_03_a_kale_tray_does_not_cover_a_sunflower_promise() {
    let mut conn = mem();
    let n = 3i64;
    let kale = trays::sow_tray(&mut conn, "kale", n).unwrap();
    let d = kale.expected_harvest_date.clone().unwrap();
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "sunflower", n);

    assert_eq!(trays::cover_shortfall_on(&conn, &d).unwrap(), n);
    assert_eq!(
        trays::cover_remaining_for(&conn, &d, "sunflower").unwrap(),
        -n
    );
    assert!(
        trays::cover_remaining_for(&conn, &d, "kale").unwrap() >= 0,
        "Kale has no promise; its cover remaining is 0 or better"
    );
    let plan = reachability::cover_plan(&conn).unwrap();
    assert!(
        plan.iter()
            .any(|c| c.harvest_date == d && c.short_trays == n),
        "COVER must list the date: {plan:?}"
    );
}

/// Finding 02. D2(b) in one test: delivery does not free capacity;
/// harvest evidence does.
#[test]
fn f4a_02_delivered_does_not_free_capacity() {
    let mut conn = mem();
    let n = 3i64;
    let t = trays::sow_tray(&mut conn, "sunflower", n).unwrap();
    let d = db::local_date_today();
    let v = venue(&mut conn);
    let order = order_crop(&mut conn, &v.venue_id, &d, "sunflower", n);

    let cover_before = trays::cover_remaining_for(&conn, &d, "sunflower").unwrap();
    let short_before = trays::cover_shortfall_on(&conn, &d).unwrap();
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    assert_eq!(
        trays::cover_remaining_for(&conn, &d, "sunflower").unwrap(),
        cover_before,
        "delivery is not evidence"
    );
    assert_eq!(trays::cover_shortfall_on(&conn, &d).unwrap(), short_before);

    trays::advance_tray(&mut conn, &t.id).unwrap();
    trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();
    assert_eq!(
        trays::cover_remaining_for(&conn, &d, "sunflower").unwrap(),
        0
    );
    assert_eq!(trays::cover_shortfall_on(&conn, &d).unwrap(), 0);
}

/// Finding 04. D4(b): a tray ready at D-2 covers a promise on D.
/// D21a: the sale side still shows those trays on their own date.
#[test]
fn f4a_04_an_early_batch_covers_the_later_date() {
    let mut conn = mem();
    let n = 3i64;
    let t = trays::sow_tray(&mut conn, "sunflower", n).unwrap();
    let d_minus_2 = t.expected_harvest_date.clone().unwrap();
    let d = add_days(&d_minus_2, 2);
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "sunflower", n);

    assert_eq!(
        trays::cover_remaining_for(&conn, &d, "sunflower").unwrap(),
        0
    );
    let plan = reachability::cover_plan(&conn).unwrap();
    assert!(
        !plan.iter().any(|c| c.harvest_date == d),
        "COVER must not list D when earlier trays cover it: {plan:?}"
    );

    let caps = trays::capacity_by_harvest_date(&conn).unwrap();
    let early = cap_row(&caps, &d_minus_2, "sunflower");
    assert_eq!(
        early.remaining_trays, n,
        "sale side did not move (D21a): the same tray is never publishable for two dates"
    );
}

/// Finding 05. D5(1): an early harvest discharges the promise it was cut for.
/// Before this fence COVER read "short N — cannot be fixed by sowing — call the venue".
#[test]
fn f4a_05_an_early_harvest_discharges_its_promise() {
    let mut conn = mem();
    let n = 3i64;
    let t = trays::sow_tray(&mut conn, "sunflower", n).unwrap();
    let d = t.expected_harvest_date.clone().unwrap();
    let harvest_day = add_days(&d, -2);
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "sunflower", n);

    trays::advance_tray(&mut conn, &t.id).unwrap();
    trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();
    conn.execute(
        "UPDATE trays SET harvested_on = ?1 WHERE id = ?2",
        rusqlite::params![&harvest_day, &t.id],
    )
    .unwrap();

    assert_eq!(trays::cover_shortfall_on(&conn, &d).unwrap(), 0);
    let start = NaiveDate::parse_from_str(&harvest_day, "%Y-%m-%d").unwrap();
    let end = NaiveDate::parse_from_str(&d, "%Y-%m-%d").unwrap();
    let mut day = start;
    while day <= end {
        let today = day.format("%Y-%m-%d").to_string();
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        assert!(
            !plan.iter().any(|c| c.harvest_date == d),
            "COVER must not list D on {today}: {plan:?}"
        );
        day += Duration::days(1);
    }
}

/// One retail claim and one wholesale order, only enough harvest evidence
/// for one: exactly one is discharged, the other still counts. Ambiguity
/// leaves the alarm on.
#[test]
fn f4a_no_tray_discharges_twice() {
    let mut conn = mem();
    let t = trays::sow_tray(&mut conn, "sunflower", 1).unwrap();
    let d = t.expected_harvest_date.clone().unwrap();
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "sunflower", 1);

    let session = paid_session(&conn, "cs_f4a_once", &d, "sunflower", 1);
    money::apply_paid_session(&mut conn, &session).unwrap();
    conn.execute(
        "UPDATE orders SET paid_at = '2000-01-01T00:00:00Z' WHERE harvest_date = ?1",
        [&d],
    )
    .unwrap();

    trays::advance_tray(&mut conn, &t.id).unwrap();
    trays::harvest_tray(&mut conn, &t.id, 10.0).unwrap();

    let caps = trays::capacity_by_harvest_date(&conn).unwrap();
    let row = cap_row(&caps, &d, "sunflower");
    // Retail's time-order rule takes the one harvested tray. Wholesale still
    // counts. Cover shortfall names the remainder.
    assert_eq!(row.sold_trays, 1, "exactly one promise still stands");
    assert_eq!(trays::cover_shortfall_on(&conn, &d).unwrap(), 1);
    let plan = reachability::cover_plan(&conn).unwrap();
    assert!(
        plan.iter()
            .any(|c| c.harvest_date == d && c.short_trays == 1),
        "alarm stays on: {plan:?}"
    );
}
