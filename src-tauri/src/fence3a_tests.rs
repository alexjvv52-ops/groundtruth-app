//! Fence III-a — retail oversell goes per-crop. remaining_exact_for + P1-P3.

use crate::attention;
use crate::db;
use crate::marketing;
use crate::money;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use chrono::{Datelike, Duration, NaiveDate};
use rusqlite::Connection;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn today() -> String {
    db::local_date_today()
}

fn add_days(date: &str, days: i64) -> String {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
    (d + Duration::days(days)).format("%Y-%m-%d").to_string()
}

fn date_label(yyyy_mm_dd: &str) -> String {
    let d = NaiveDate::parse_from_str(yyyy_mm_dd, "%Y-%m-%d").unwrap();
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{} {}", months[d.month0() as usize], d.day())
}

fn harvest_date_for(conn: &Connection, tray_id: &str) -> String {
    trays::get_tray(conn, tray_id)
        .unwrap()
        .expected_harvest_date
        .expect("expected harvest date")
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
    paid_session_lines(conn, session_id, harvest_date, &[(crop_id, qty)])
}

fn paid_session_lines(
    conn: &Connection,
    session_id: &str,
    harvest_date: &str,
    lines: &[(&str, i64)],
) -> money::PaidSession {
    let mut paid_lines = Vec::new();
    let mut amount_cents = 0i64;
    for (crop_id, qty) in lines {
        let price_id = format!("price_{harvest_date}_{crop_id}");
        seed_offer_price(conn, harvest_date, crop_id, &price_id, 1200);
        paid_lines.push(money::PaidLine {
            price_id,
            quantity: *qty,
            amount_cents: qty * 1200,
        });
        amount_cents += qty * 1200;
    }
    money::PaidSession {
        session_id: session_id.into(),
        payment_intent: Some(format!("pi_{session_id}")),
        lines: paid_lines,
        currency: "cad".into(),
        customer_email: Some("buyer@example.com".into()),
        paid_at: "2026-08-05T12:00:00.000Z".into(),
        created: 1_700_000_000,
        amount_cents,
        client_reference: None,
        payment_link: None,
    }
}

fn venue(conn: &mut Connection) -> marketing::VenueView {
    marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap()
}

fn oversold_open(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT entity_id, message FROM attention
             WHERE kind = 'order.oversold' AND resolved_at IS NULL
             ORDER BY entity_id",
        )
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// Two crops on one date, ONE oversold: exactly one order.oversold row,
/// naming the oversold crop. The surplus crop raises nothing.
#[test]
fn fence3a_one_oversold_crop_on_a_shared_date() {
    let mut conn = mem();
    let peas = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let sun = trays::sow_tray(&mut conn, "sunflower", 5).unwrap();
    let hd = harvest_date_for(&conn, &peas.id);
    assert_eq!(harvest_date_for(&conn, &sun.id), hd, "same harvest date");

    let session = paid_session_lines(
        &conn,
        "cs_f3a_one",
        &hd,
        &[("dun-peas", 2), ("sunflower", 1)],
    );
    money::apply_paid_session(&mut conn, &session).unwrap();

    let rows = oversold_open(&conn);
    assert_eq!(rows.len(), 1, "exactly one oversold crop: {rows:?}");
    assert_eq!(rows[0].0, format!("{hd}|dun-peas"));
    assert!(
        rows[0].1.contains("of Dun peas"),
        "must name the oversold crop: {}",
        rows[0].1
    );
    assert!(
        !rows[0].1.contains("Sunflower"),
        "must not name the surplus crop: {}",
        rows[0].1
    );
    let sun_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'order.oversold' AND entity_id = ?1",
            [format!("{hd}|sunflower")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sun_rows, 0, "surplus crop raises nothing");
}

/// Both crops oversold on one date: two rows, two distinct composite
/// entity_ids, each naming its own crop and its own count.
#[test]
fn fence3a_both_crops_oversold_two_rows() {
    let mut conn = mem();
    let peas = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let sun = trays::sow_tray(&mut conn, "sunflower", 1).unwrap();
    let hd = harvest_date_for(&conn, &peas.id);
    assert_eq!(harvest_date_for(&conn, &sun.id), hd);

    let session = paid_session_lines(
        &conn,
        "cs_f3a_both",
        &hd,
        &[("dun-peas", 2), ("sunflower", 4)],
    );
    money::apply_paid_session(&mut conn, &session).unwrap();

    let rows = oversold_open(&conn);
    assert_eq!(rows.len(), 2, "one row per oversold crop: {rows:?}");
    let peas_key = format!("{hd}|dun-peas");
    let sun_key = format!("{hd}|sunflower");
    assert_ne!(peas_key, sun_key);
    let peas_row = rows.iter().find(|(id, _)| id == &peas_key).expect("peas");
    let sun_row = rows.iter().find(|(id, _)| id == &sun_key).expect("sun");
    assert!(
        peas_row.1.contains("oversold by 1 tray of Dun peas"),
        "{}",
        peas_row.1
    );
    assert!(
        !peas_row.1.contains("Sunflower"),
        "peas row must not name sunflower: {}",
        peas_row.1
    );
    assert!(
        sun_row.1.contains("oversold by 3 trays of Sunflower"),
        "{}",
        sun_row.1
    );
    assert!(
        !sun_row.1.contains("Dun peas"),
        "sunflower row must not name peas: {}",
        sun_row.1
    );
}

/// D21a: a crop surplus on the same date must NOT net out another crop's
/// deficit. The old date-wide gate went silent here; the new one must fire.
#[test]
fn fence3a_d21a_surplus_does_not_net_a_deficit() {
    let mut conn = mem();
    let peas = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let sun = trays::sow_tray(&mut conn, "sunflower", 5).unwrap();
    let hd = harvest_date_for(&conn, &peas.id);
    assert_eq!(harvest_date_for(&conn, &sun.id), hd);

    // Date-wide remaining after selling 2 peas: 5 sunflower + (-1) peas = +4.
    // The old remaining_for_date gate saw >= 0 and said nothing.
    let session = paid_session(&conn, "cs_f3a_d21a", &hd, "dun-peas", 2);
    money::apply_paid_session(&mut conn, &session).unwrap();

    let date_wide = trays::remaining_for_date(&conn, &hd).unwrap();
    assert!(
        date_wide >= 0,
        "date-wide rollup is the silent path: {date_wide}"
    );
    let peas_rem = trays::remaining_exact_for(&conn, &hd, "dun-peas").unwrap();
    assert_eq!(peas_rem, -1, "peas are oversold");

    let rows = oversold_open(&conn);
    assert_eq!(rows.len(), 1, "per-crop gate must fire: {rows:?}");
    assert_eq!(rows[0].0, format!("{hd}|dun-peas"));
    assert!(
        rows[0].1.contains("oversold by 1 tray of Dun peas"),
        "{}",
        rows[0].1
    );
}

/// P1 — reachable: "{d} is oversold by {n} of {crop}."
#[test]
fn fence3a_p1_reachable_sentence() {
    let mut conn = mem();
    let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let hd = harvest_date_for(&conn, &t.id);
    let session = paid_session(&conn, "cs_f3a_p1", &hd, "dun-peas", 2);
    money::apply_paid_session(&mut conn, &session).unwrap();

    let rows = oversold_open(&conn);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let expected = format!("{} is oversold by 1 tray of Dun peas.", date_label(&hd));
    assert_eq!(rows[0].1, expected);
}

/// P2 — unreachable + settle_by_delivering_facts:
/// "{d} is oversold by {n} of {crop}. It cannot be fixed by sowing - deliver or void the open order."
#[test]
fn fence3a_p2_deliver_sentence() {
    let mut conn = mem();
    let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let growth = t.growth_days_at_sow.expect("growth days");
    trays::dev_backdate_tray(&mut conn, &t.id, growth).unwrap();
    let hd = harvest_date_for(&conn, &t.id);
    assert_eq!(hd, today(), "fixture must mature today");
    trays::advance_tray(&mut conn, &t.id).unwrap();
    trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();

    let v = venue(&mut conn);
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &hd,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 3,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    let session = paid_session(&conn, "cs_f3a_p2", &hd, "dun-peas", 1);
    money::apply_paid_session(&mut conn, &session).unwrap();

    let rows = oversold_open(&conn);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let expected = format!(
        "{} is oversold by 1 tray of Dun peas. \
         It cannot be fixed by sowing - deliver or void the open order.",
        date_label(&hd)
    );
    assert_eq!(rows[0].1, expected);
}

/// P3 — otherwise:
/// "{d} is oversold by {n} of {crop}. It cannot be fixed by sowing - call the venue."
#[test]
fn fence3a_p3_call_the_venue_sentence() {
    let mut conn = mem();
    let hd = add_days(&today(), 2);
    let session = paid_session(&conn, "cs_f3a_p3", &hd, "dun-peas", 1);
    money::apply_paid_session(&mut conn, &session).unwrap();

    let rows = oversold_open(&conn);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let expected = format!(
        "{} is oversold by 1 tray of Dun peas. \
         It cannot be fixed by sowing - call the venue.",
        date_label(&hd)
    );
    assert_eq!(rows[0].1, expected);
}

/// Ordering pin: live pre-split → superseded_by_per_crop_card and a composite
/// row re-raises; past pre-split → date_passed and nothing re-raises.
#[test]
fn fence3a_presplit_live_vs_past_reasons() {
    let mut conn = mem();
    let today = today();
    let live = add_days(&today, 14);
    let past = add_days(&today, -3);
    let v = venue(&mut conn);
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &live,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();

    conn.execute(
        "INSERT INTO attention
         (id, kind, entity_type, entity_id, message, actions, created_at, resolved_at, resolved_by)
         VALUES ('legacy-live', 'order.oversold', 'harvest_date', ?1,
                 'legacy live date-keyed row', '[]', ?2, NULL, NULL)",
        rusqlite::params![&live, db::utc_now_rfc3339()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attention
         (id, kind, entity_type, entity_id, message, actions, created_at, resolved_at, resolved_by)
         VALUES ('legacy-past', 'order.oversold', 'harvest_date', ?1,
                 'legacy past date-keyed row', '[]', ?2, NULL, NULL)",
        rusqlite::params![&past, db::utc_now_rfc3339()],
    )
    .unwrap();

    attention::check_attention(&conn).unwrap();

    let live_by: String = conn
        .query_row(
            "SELECT resolved_by FROM attention WHERE id = 'legacy-live'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(live_by, "superseded_by_per_crop_card");
    let composite: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1
               AND resolved_at IS NULL",
            [format!("{live}|dun-peas")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        composite, 1,
        "still-oversold live date must re-raise composite"
    );

    let past_by: String = conn
        .query_row(
            "SELECT resolved_by FROM attention WHERE id = 'legacy-past'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(past_by, "date_passed");
    let past_reopen: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind IN ('order.oversold', 'money.capacity_short')
               AND resolved_at IS NULL
               AND (entity_id = ?1 OR entity_id LIKE ?2)",
            rusqlite::params![&past, format!("{past}|%")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(past_reopen, 0, "past date must not re-raise");
}

/// A pre-split date-only order.oversold row retires as
/// superseded_by_per_crop_card, not condition_cleared.
#[test]
fn fence3a_presplit_date_only_retires_as_superseded() {
    let conn = mem();
    let d = add_days(&today(), 14);
    conn.execute(
        "INSERT INTO attention
         (id, kind, entity_type, entity_id, message, actions, created_at, resolved_at, resolved_by)
         VALUES ('legacy-oversold', 'order.oversold', 'harvest_date', ?1,
                 'legacy date-keyed row', '[]', ?2, NULL, NULL)",
        rusqlite::params![&d, db::utc_now_rfc3339()],
    )
    .unwrap();

    attention::check_attention(&conn).unwrap();

    let (resolved_by, resolved_at): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT resolved_by, resolved_at FROM attention WHERE id = 'legacy-oversold'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(resolved_at.is_some(), "pre-split row must retire");
    assert_eq!(
        resolved_by.as_deref(),
        Some("superseded_by_per_crop_card"),
        "must not use condition_cleared"
    );
    assert_ne!(
        resolved_by.as_deref(),
        Some("condition_cleared"),
        "must not use condition_cleared"
    );
}
