//! Fence IV-b — one COVER card per (date, crop). Signed sentences A-K.

use crate::attention;
use crate::db;
use crate::events::{EventRecord, Kind};
use crate::marketing;
use crate::models::{HarvestGroup, HarvestInput};
use crate::projection;
use crate::reachability;
use crate::trays;
use crate::wholesale::{self, DeliveredPayload, OrderLine};
use chrono::{Duration, NaiveDate};
use rusqlite::Connection;
use serde_json::json;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn add_days(date: &str, days: i64) -> String {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
    (d + Duration::days(days)).format("%Y-%m-%d").to_string()
}

fn today() -> String {
    db::local_date_today()
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

fn card_key(date: &str, crop_id: &str) -> String {
    format!("{date}|{crop_id}")
}

/// Two crops short on one date -> two cards, each naming its own crop and number.
#[test]
fn f4b_one_card_per_date_crop() {
    let mut conn = mem();
    let today = today();
    let d = add_days(&today, 14);
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "sunflower", 3);
    order_crop(&mut conn, &v.venue_id, &d, "kale", 5);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let on_d: Vec<_> = plan.iter().filter(|c| c.harvest_date == d).collect();
    assert_eq!(on_d.len(), 2, "one card per (date, crop): {plan:?}");

    let sun = on_d.iter().find(|c| c.crop_id == "sunflower").unwrap();
    let kale = on_d.iter().find(|c| c.crop_id == "kale").unwrap();
    assert_eq!(sun.short_trays, 3);
    assert_eq!(kale.short_trays, 5);
    assert!(sun.message.contains("Sunflower"), "{}", sun.message);
    assert!(sun.message.contains("3 trays"), "{}", sun.message);
    assert!(!sun.message.contains("Kale"), "{}", sun.message);
    assert!(kale.message.contains("Kale"), "{}", kale.message);
    assert!(kale.message.contains("5 trays"), "{}", kale.message);
    assert!(!kale.message.contains("Sunflower"), "{}", kale.message);
}

/// B and F quote the SHORT crop's deadline, not the library's fastest.
#[test]
fn f4b_b_and_f_quote_the_short_crops_deadline() {
    let mut conn = mem();
    let today = today();
    let v = venue(&mut conn);

    // F: kale is 9 days; radish (fastest) is 7. Date 12 days out is reachable
    // for kale. sow-by is date-9, not date-7.
    let live = add_days(&today, 12);
    order_crop(&mut conn, &v.venue_id, &live, "kale", 2);
    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let kale_f = plan
        .iter()
        .find(|c| c.harvest_date == live && c.crop_id == "kale")
        .unwrap();
    let kale_by = reachability::format_est_mon_d_local(&add_days(&live, -9)).unwrap();
    let radish_by = reachability::format_est_mon_d_local(&add_days(&live, -7)).unwrap();
    assert!(
        kale_f.message.contains(&kale_by),
        "F must quote kale's deadline {kale_by}: {}",
        kale_f.message
    );
    assert!(
        !kale_f.message.contains(&radish_by),
        "F must not quote the library fastest {radish_by}: {}",
        kale_f.message
    );

    // B: kale on a date 2 days out cannot be sown. Last kale sow was date-9,
    // not the radish date-7.
    let dead = add_days(&today, 2);
    order_crop(&mut conn, &v.venue_id, &dead, "kale", 2);
    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let kale_b = plan
        .iter()
        .find(|c| c.harvest_date == dead && c.crop_id == "kale")
        .unwrap();
    let kale_est = reachability::format_est_mon_d_local(&add_days(&dead, -9)).unwrap();
    let radish_est = reachability::format_est_mon_d_local(&add_days(&dead, -7)).unwrap();
    assert!(
        kale_b.message.contains("cannot be fixed by sowing"),
        "{}",
        kale_b.message
    );
    assert!(
        kale_b.message.contains(&kale_est),
        "B must quote kale's last sow {kale_est}: {}",
        kale_b.message
    );
    assert!(
        !kale_b.message.contains(&radish_est),
        "B must not quote the library fastest {radish_est}: {}",
        kale_b.message
    );
}

/// A date short only in a slow crop, with a fast crop available, takes the
/// unreachable arm — it does not say "sow by <fast crop's date>".
#[test]
fn f4b_slow_crop_short_does_not_quote_fast_crops_sow_by() {
    let mut conn = mem();
    let today = today();
    // 8 days: radish (7) reaches, kale (9) does not.
    let d = add_days(&today, 8);
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "kale", 2);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = plan
        .iter()
        .find(|card| card.harvest_date == d && card.crop_id == "kale")
        .unwrap();
    assert!(!c.reachability.reachable);
    let fast_by = reachability::format_est_mon_d_local(&add_days(&d, -7)).unwrap();
    assert!(
        c.message.contains("cannot be fixed by sowing"),
        "slow-crop short must take the unreachable arm: {}",
        c.message
    );
    assert!(
        !c.message.contains(&format!("sow by {fast_by}")),
        "must not invite a sow by the fast crop's date {fast_by}: {}",
        c.message
    );
}

/// Clearing one crop resolves that card and leaves the other date-mate open.
#[test]
fn f4b_clearing_one_crop_leaves_the_date_mate_open() {
    let mut conn = mem();
    let today = today();
    let d = add_days(&today, 14);
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "sunflower", 3);
    let kale_order = order_crop(&mut conn, &v.venue_id, &d, "kale", 5);
    attention::check_attention(&conn).unwrap();

    let sun_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [card_key(&d, "sunflower")],
            |r| r.get(0),
        )
        .unwrap();
    let kale_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [card_key(&d, "kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sun_open, 1);
    assert_eq!(kale_open, 1);

    wholesale::void_order(&mut conn, &kale_order.id, Some("test".into())).unwrap();
    attention::check_attention(&conn).unwrap();

    let sun_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [card_key(&d, "sunflower")],
            |r| r.get(0),
        )
        .unwrap();
    let kale_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [card_key(&d, "kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sun_after, 1, "sunflower date-mate must still stand");
    assert_eq!(kale_after, 0, "cleared kale card must resolve");
}

/// A pre-split date-keyed row retires as superseded_by_per_crop_card, not
/// condition_cleared.
#[test]
fn f4b_presplit_date_keyed_row_retires_as_superseded() {
    let mut conn = mem();
    let today = today();
    let d = add_days(&today, 14);
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "sunflower", 2);

    conn.execute(
        "INSERT INTO attention
         (id, kind, entity_type, entity_id, message, actions, created_at, resolved_at, resolved_by)
         VALUES ('legacy-date', 'money.capacity_short', 'harvest_date', ?1,
                 'legacy date-keyed row', '[]', ?2, NULL, NULL)",
        rusqlite::params![&d, db::utc_now_rfc3339()],
    )
    .unwrap();

    attention::check_attention(&conn).unwrap();

    let (resolved_by, resolved_at): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT resolved_by, resolved_at FROM attention WHERE id = 'legacy-date'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(resolved_at.is_some(), "pre-split row must retire");
    assert_eq!(
        resolved_by.as_deref(),
        Some("superseded_by_per_crop_card"),
        "must not use condition_cleared for a date that is still short"
    );

    let new_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [card_key(&d, "sunflower")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        new_open, 1,
        "still-short date re-raises under the composite key"
    );
}

/// shelf_pressure_on counts a two-crop-short date once.
#[test]
fn f4b_shelf_pressure_counts_a_two_crop_date_once() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let d = add_days(&today, 14);
    let v = venue(&mut conn);
    for crop in ["sunflower", "kale"] {
        wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &d,
            vec![OrderLine {
                crop_id: crop.into(),
                trays: 2,
                price_cents_per_tray: Some(900),
            }],
            true,
        )
        .unwrap();
    }

    let p = reachability::shelf_pressure_on(&conn, &today).unwrap();
    assert_eq!(
        p.dates, 1,
        "two crops short on one date count as one date: dates={} trays={}",
        p.dates, p.trays
    );
    assert!(!p.firing);
}

fn n1_line(harvest_date: &str, today: &str) -> String {
    let d = reachability::format_mon_d_local(harvest_date).unwrap();
    let harvest = NaiveDate::parse_from_str(harvest_date, "%Y-%m-%d").unwrap();
    let today_d = NaiveDate::parse_from_str(today, "%Y-%m-%d").unwrap();
    let k = (harvest - today_d).num_days();
    let out = if k == 1 {
        "1 day out".to_string()
    } else {
        format!("{k} days out")
    };
    format!(
        "{d} is {out} - an order cannot be delivered before its harvest date. Harvest early or void the order."
    )
}

fn set_expected_harvest(conn: &Connection, tray_id: &str, ehd: &str) {
    let gd: i64 = conn
        .query_row(
            "SELECT growth_days_at_sow FROM trays WHERE id = ?1",
            [tray_id],
            |r| r.get(0),
        )
        .unwrap();
    let e = NaiveDate::parse_from_str(ehd, "%Y-%m-%d").unwrap();
    let sown = (e - Duration::days(gd)).format("%Y-%m-%d").to_string();
    conn.execute(
        "UPDATE trays SET sown_on = ?1 WHERE id = ?2",
        rusqlite::params![sown, tray_id],
    )
    .unwrap();
}

fn harvest_inputs(groups: &[HarvestGroup]) -> Vec<HarvestInput> {
    groups
        .iter()
        .map(|g| HarvestInput {
            tray_ids: g.tray_ids.clone(),
            actual_yield_oz: 10.0,
        })
        .collect()
}

/// D2 extra / N1: deliver_order refuses when harvest_date is still in the future.
#[test]
fn f4b_deliver_order_refuses_future_harvest_date_with_n1() {
    let mut conn = mem();
    let today = today();
    let v = venue(&mut conn);

    let d5 = add_days(&today, 5);
    let o5 = order_crop(&mut conn, &v.venue_id, &d5, "sunflower", 2);
    let err = wholesale::deliver_order(&mut conn, &o5.id, None).unwrap_err();
    assert_eq!(err, n1_line(&d5, &today));

    let d1 = add_days(&today, 1);
    let o1 = order_crop(&mut conn, &v.venue_id, &d1, "kale", 1);
    let err1 = wholesale::deliver_order(&mut conn, &o1.id, None).unwrap_err();
    assert_eq!(err1, n1_line(&d1, &today));
}

/// D2 extra: harvest_date == today and harvest_date < today still deliver.
#[test]
fn f4b_deliver_order_succeeds_on_or_before_today() {
    let mut conn = mem();
    let today = today();
    let v = venue(&mut conn);

    let on = order_crop(&mut conn, &v.venue_id, &today, "sunflower", 1);
    let delivered = wholesale::deliver_order(&mut conn, &on.id, None).unwrap();
    assert_eq!(delivered.state, "delivered");

    let past = add_days(&today, -2);
    let before = order_crop(&mut conn, &v.venue_id, &past, "kale", 1);
    let delivered_past = wholesale::deliver_order(&mut conn, &before.id, None).unwrap();
    assert_eq!(delivered_past.state, "delivered");
}

/// Replay / bundle-import stand-in — NOT a production door.
/// The only production routes that create an impossible delivered row are
/// replay and bundle import (import.rs:195). They apply `wholesale.delivered`
/// through `projection::apply_event` with no harvest gate. This helper
/// mirrors wholesale.rs deliver_order exactly, minus the gate.
fn deliver_without_harvest_gate(conn: &mut Connection, order_id: &str, delivered_on: String) {
    let created_at = projection::handler_now();
    let payload = DeliveredPayload {
        order_id: order_id.to_string(),
        delivered_on,
    };
    let event = EventRecord::originated(
        Kind::WholesaleDelivered,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&payload).unwrap(),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().unwrap();
    wholesale::write_pair(&tx, &event).unwrap();
    tx.commit().unwrap();
}

/// D2 extra, display side: an impossible delivered row stays in the money
/// totals and drops the age clock.
#[test]
fn f4b_owed_summary_drops_age_for_impossible_delivered_row() {
    let mut conn = mem();
    let today = today();
    let v = venue(&mut conn);
    let harvest = add_days(&today, 5);
    let o = order_crop(&mut conn, &v.venue_id, &harvest, "sunflower", 2);
    deliver_without_harvest_gate(&mut conn, &o.id, today.clone());

    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.oldest_days, None);
    assert_eq!(owed.deliveries, 1);
    assert_eq!(owed.total_cents, Some(1800));
}

/// Honest age is kept; the impossible row still counts as a delivery.
#[test]
fn f4b_owed_summary_counts_only_the_honest_row() {
    let mut conn = mem();
    let today = today();
    let v = venue(&mut conn);

    let honest_harvest = add_days(&today, -4);
    let honest = order_crop(&mut conn, &v.venue_id, &honest_harvest, "kale", 1);
    wholesale::deliver_order(&mut conn, &honest.id, Some(honest_harvest.clone())).unwrap();

    let harvest = add_days(&today, 5);
    let impossible = order_crop(&mut conn, &v.venue_id, &harvest, "sunflower", 2);
    deliver_without_harvest_gate(&mut conn, &impossible.id, today.clone());

    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.oldest_days, Some(4));
    assert_eq!(owed.deliveries, 2);
}

/// COLLECT membership is untouched; only the "delivered" clause is dropped.
#[test]
fn f4b_collect_sentence_drops_age_for_impossible_row() {
    let mut conn = mem();
    let today = today();
    let v = venue(&mut conn);
    let harvest = add_days(&today, 5);
    let o = order_crop(&mut conn, &v.venue_id, &harvest, "sunflower", 2);
    deliver_without_harvest_gate(&mut conn, &o.id, today.clone());

    let debts = attention::money_debts_on(&conn, &today).unwrap();
    let row = debts
        .collect
        .iter()
        .find(|d| d.order_id == o.id)
        .expect("impossible delivered row must stay in COLLECT");
    let expected = format!(
        "Collect {} — {}.",
        crate::currency::code_amount("usd", 1800),
        v.name
    );
    assert_eq!(row.message, expected);
    assert!(!row.message.contains("delivered"), "{}", row.message);
}

/// N-2 proved display-side: delivered_on after today is not countable.
#[test]
fn f4b_delivered_on_after_today_is_not_countable() {
    let mut conn = mem();
    let today = today();
    let v = venue(&mut conn);
    let o = order_crop(&mut conn, &v.venue_id, &today, "sunflower", 1);
    deliver_without_harvest_gate(&mut conn, &o.id, add_days(&today, 3));

    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.oldest_days, None);

    let listed = wholesale::list_orders(&conn).unwrap();
    let row = listed.iter().find(|r| r.id == o.id).expect("row kept");
    assert_eq!(row.delivered_age_days, None);
}

/// A refused future delivery writes nothing: cover_remaining_for is identical.
#[test]
fn f4b_refused_deliver_does_not_change_capacity() {
    let mut conn = mem();
    let today = today();
    let d = add_days(&today, 9);
    let v = venue(&mut conn);
    let order = order_crop(&mut conn, &v.venue_id, &d, "sunflower", 3);

    let before = trays::cover_remaining_for(&conn, &d, "sunflower").unwrap();
    let err = wholesale::deliver_order(&mut conn, &order.id, None).unwrap_err();
    assert_eq!(err, n1_line(&d, &today));
    assert_eq!(
        trays::cover_remaining_for(&conn, &d, "sunflower").unwrap(),
        before
    );
}

/// D24a reader: only this crop, only not-yet-due, only nominal date <= the card.
#[test]
fn f4b_early_harvest_groups_for_filters() {
    let mut conn = mem();
    let today = today();
    let card = add_days(&today, 10);

    let sun_ok = trays::sow_tray(&mut conn, "sunflower", 2).unwrap();
    trays::advance_tray(&mut conn, &sun_ok.id).unwrap();
    set_expected_harvest(&conn, &sun_ok.id, &add_days(&today, 8));

    let kale = trays::sow_tray(&mut conn, "kale", 2).unwrap();
    trays::advance_tray(&mut conn, &kale.id).unwrap();
    set_expected_harvest(&conn, &kale.id, &add_days(&today, 8));

    let sun_late = trays::sow_tray(&mut conn, "sunflower", 1).unwrap();
    trays::advance_tray(&mut conn, &sun_late.id).unwrap();
    set_expected_harvest(&conn, &sun_late.id, &add_days(&today, 11));

    let sun_due = trays::sow_tray(&mut conn, "sunflower", 1).unwrap();
    trays::advance_tray(&mut conn, &sun_due.id).unwrap();
    set_expected_harvest(&conn, &sun_due.id, &today);

    let groups = trays::early_harvest_groups_for(&conn, &card, "sunflower", &today).unwrap();
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0].crop_id, "sunflower");
    assert_eq!(groups[0].tray_ids, vec![sun_ok.id.clone()]);
    assert_eq!(groups[0].tray_count, 2);

    let kale_groups = trays::early_harvest_groups_for(&conn, &card, "kale", &today).unwrap();
    assert_eq!(kale_groups.len(), 1);
    assert_eq!(kale_groups[0].crop_id, "kale");
    assert_eq!(kale_groups[0].tray_ids, vec![kale.id.clone()]);
}

/// Harvest-early converts already-counted cover_supply into cooler evidence.
/// The card's number does not move.
#[test]
fn f4b_harvest_early_is_shortfall_neutral_and_converts_supply_to_evidence() {
    let mut conn = mem();
    let today = today();
    let d = add_days(&today, 14);
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "sunflower", 7);

    let short_before = trays::cover_shortfall_on(&conn, &d).unwrap();
    assert!(
        short_before > 0,
        "card must stand before the early group is cut: {short_before}"
    );
    assert!(
        reachability::cover_plan_on(&conn, &today)
            .unwrap()
            .iter()
            .any(|c| c.harvest_date == d && c.crop_id == "sunflower"),
        "COVER must list the card before harvest"
    );

    let t = trays::sow_tray(&mut conn, "sunflower", 3).unwrap();
    trays::advance_tray(&mut conn, &t.id).unwrap();
    let nominal = t
        .expected_harvest_date
        .clone()
        .expect("sown tray has expected_harvest_date");
    // promise 7, supply 3 → short 4; the card must survive the cut
    let short_after_sow = trays::cover_shortfall_on(&conn, &d).unwrap();
    assert_eq!(short_after_sow, 4, "short after sow (promise 7, supply 3)");

    let groups = trays::early_harvest_groups_for(&conn, &d, "sunflower", &today).unwrap();
    assert!(
        !groups.is_empty(),
        "reader must name the not-yet-due sunflower group"
    );

    let caps_before = trays::capacity_by_harvest_date(&conn).unwrap();
    let row_before = caps_before
        .iter()
        .find(|r| r.harvest_date == d && r.crop_id == "sunflower")
        .unwrap_or_else(|| panic!("missing capacity row ({d}, sunflower) before harvest"));
    let harvested_before = row_before.harvested_trays;
    let supply_before = row_before.cover_supply;
    let promised_before = row_before.cover_promised;
    let harvested_nominal_before = caps_before
        .iter()
        .find(|r| r.harvest_date == nominal && r.crop_id == "sunflower")
        .map(|r| r.harvested_trays)
        .unwrap_or_else(|| panic!("missing capacity row ({nominal}, sunflower) before harvest"));
    let message_before = reachability::cover_plan_on(&conn, &today)
        .unwrap()
        .iter()
        .find(|c| c.harvest_date == d && c.crop_id == "sunflower")
        .map(|c| c.message.clone())
        .expect("card must exist after sow");

    trays::harvest_groups(&mut conn, &harvest_inputs(&groups)).unwrap();

    let short_after_harvest = trays::cover_shortfall_on(&conn, &d).unwrap();
    let caps_after = trays::capacity_by_harvest_date(&conn).unwrap();
    let row_after = caps_after
        .iter()
        .find(|r| r.harvest_date == d && r.crop_id == "sunflower")
        .unwrap_or_else(|| panic!("missing capacity row ({d}, sunflower) after harvest"));
    let row_nominal_after = caps_after
        .iter()
        .find(|r| r.harvest_date == nominal && r.crop_id == "sunflower")
        .unwrap_or_else(|| panic!("missing capacity row ({nominal}, sunflower) after harvest"));

    // D4(b)/D5(1): the door's filter is nominal-date-on-or-before the card's
    // date, so these trays were ALREADY cover_supply. Cutting them moves 3 out
    // of supply and 3 out of undischarged promise at once. Supply-to-evidence
    // conversion is shortfall-NEUTRAL. The card keeps its number; what changes
    // is that the promise is now backed by trays in the cooler.
    assert_eq!(
        short_after_harvest, short_after_sow,
        "shortfall-neutral, not smaller: sow={short_after_sow} harvest={short_after_harvest}"
    );
    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let card = plan
        .iter()
        .find(|c| c.harvest_date == d && c.crop_id == "sunflower");
    assert!(
        card.is_some(),
        "cover_plan_on still lists ({d}, sunflower): {plan:?}"
    );
    assert_eq!(
        card.unwrap().message,
        message_before,
        "card message must be byte-identical"
    );
    // Two notions of harvest live in one row. harvested_trays is EXACT-DATE on
    // the tray's nominal date (models.rs: "whose nominal date is this date").
    // Discharge is CUMULATIVE and keyed on harvested_on (D5(1)). An early cut
    // therefore lands on the nominal row, while the CARD's cover ladder moves
    // via cover_supply / cover_promised. Asserting harvested_trays on the card's
    // own date is wrong and this claim exists partly to stop that mistake.
    assert_eq!(
        row_after.harvested_trays, harvested_before,
        "4a card harvested_trays unchanged: {harvested_before} -> {}",
        row_after.harvested_trays
    );
    assert_eq!(
        row_nominal_after.harvested_trays,
        harvested_nominal_before + 3,
        "4b nominal harvested_trays {harvested_nominal_before} -> {}",
        row_nominal_after.harvested_trays
    );
    assert_eq!(
        row_after.cover_supply,
        supply_before - 3,
        "cover_supply {supply_before} -> {}",
        row_after.cover_supply
    );
    assert_eq!(
        row_after.cover_promised,
        promised_before - 3,
        "cover_promised {promised_before} -> {}",
        row_after.cover_promised
    );
}

/// D24a end-to-end: reader's groups fed to harvest_groups, no harvested_on edit.
#[test]
fn f4b_d24a_reader_to_harvest_groups_without_hand_edit() {
    let mut conn = mem();
    let today = today();
    let d = add_days(&today, 14);
    let v = venue(&mut conn);
    order_crop(&mut conn, &v.venue_id, &d, "sunflower", 3);

    let t = trays::sow_tray(&mut conn, "sunflower", 3).unwrap();
    trays::advance_tray(&mut conn, &t.id).unwrap();
    let groups = trays::early_harvest_groups_for(&conn, &d, "sunflower", &today).unwrap();
    assert!(!groups.is_empty(), "{groups:?}");
    trays::harvest_groups(&mut conn, &harvest_inputs(&groups)).unwrap();

    let harvested = trays::get_tray(&conn, &t.id).unwrap();
    assert_eq!(harvested.state, "harvested");
    assert_eq!(
        harvested.harvested_on.as_deref(),
        Some(today.as_str()),
        "harvest_groups stamps today; no fixture hand-edit of harvested_on"
    );

    assert_eq!(trays::cover_shortfall_on(&conn, &d).unwrap(), 0);
    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    assert!(
        !plan
            .iter()
            .any(|c| c.harvest_date == d && c.crop_id == "sunflower"),
        "engine reader honours the harvest without a harvested_on edit: {plan:?}"
    );
}
