//! Phase 6 — physical tray ceiling and growth signal.

use crate::attention;
use crate::db;
use crate::health::{self, CheckInputs, Severity};
use crate::marketing;
use crate::reachability::{self, CoverDate};
use crate::trays;
use crate::wholesale::{self, OrderLine, OrderedPayload};
use chrono::{Datelike, Duration, NaiveDate};
use rusqlite::Connection;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn add_days(iso: &str, n: i64) -> String {
    let d = NaiveDate::parse_from_str(iso, "%Y-%m-%d").unwrap();
    (d + Duration::days(n)).format("%Y-%m-%d").to_string()
}

fn today() -> String {
    db::local_date_today()
}

fn order_trays(conn: &mut Connection, harvest_date: &str, crop_id: &str, trays: i64) {
    let venue = marketing::record_venue(conn, "Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(
        conn,
        &venue.venue_id,
        harvest_date,
        vec![OrderLine {
            crop_id: crop_id.to_string(),
            trays,
            price_cents_per_tray: Some(800),
        }],
        true,
    )
    .unwrap();
}

fn cover_for<'a>(plan: &'a [CoverDate], harvest_date: &str) -> &'a CoverDate {
    plan.iter()
        .find(|c| c.harvest_date == harvest_date)
        .unwrap_or_else(|| panic!("no cover row for {harvest_date}"))
}

#[test]
fn p6t1_unknown_ceiling_is_dark_safe() {
    let mut conn = mem();
    let today = today();
    let harvest = add_days(&today, 14);
    order_trays(&mut conn, &harvest, "dun-peas", 2);

    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert!(r.shelf_slots_free.is_none());
    assert!(r.reachable);
    assert!(r.sow_can_serve);
    assert_eq!(r.overdue_on_shelf, 0);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest);
    assert!(c.reachability.shelf_slots_free.is_none());

    let d = reachability::format_mon_d_local(&harvest).unwrap();
    let by = reachability::format_est_mon_d_local(r.last_sow_by.as_deref().unwrap()).unwrap();
    let card_by =
        reachability::format_est_mon_d_local(c.reachability.last_sow_by.as_deref().unwrap())
            .unwrap();
    let expected_msg = format!("{d} is short 2 trays of Dun peas - sow by {card_by} to cover it.");
    assert_eq!(c.message, expected_msg);

    let expected_entry = format!(
        "{d} is 14 days out. Sowing Dun peas still reaches it - sow by {by} at the latest. No Dun peas is growing for that date. 2 trays of Dun peas already promised."
    );
    assert_eq!(r.entry_line, expected_entry);

    assert_eq!(
        reachability::band(c),
        3,
        "unknown ceiling is not shelf-blocked; slack rank is 3 after the new band"
    );
    assert!(
        !c.message.contains("no room") && !c.message.contains("only room"),
        "{}",
        c.message
    );
    assert!(
        !r.entry_line.contains("Room to sow") && !r.entry_line.contains("No room to sow"),
        "{}",
        r.entry_line
    );
}

#[test]
fn p6t2_full_shelf_no_room_sentence_still_reachable() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let harvest = t.expected_harvest_date.expect("harvest date");
    order_trays(&mut conn, &harvest, "dun-peas", 10);

    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert!(
        r.reachable,
        "axes stay independent: time-reachable is still true"
    );
    assert_eq!(r.shelf_slots_free, Some(0));
    assert!(!r.sow_can_serve);
    assert_eq!(r.overdue_on_shelf, 0, "p6t2 fixture has no overdue trays");

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest);
    let d = reachability::format_mon_d_local(&harvest).unwrap();
    let n = reachability::tray_word(c.short_trays);
    assert_eq!(
        c.message,
        format!("{d} is short {n} of Dun peas - sowing reaches it but there is no room. Harvest early or void the order.")
    );
    assert!(r.reachable);
}

#[test]
fn p6t3_unreachable_plus_full_shelf_keeps_call_the_venue() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let dead = add_days(&today, 2);
    order_trays(&mut conn, &dead, "dun-peas", 4);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &dead);
    assert!(!c.reachability.reachable);
    assert!(
        c.message
            .contains("cannot be fixed by sowing - call the venue"),
        "do not reorder the branches in cover_message: {}",
        c.message
    );
    assert!(
        !c.message.contains("no room"),
        "do not reorder the branches in cover_message: {}",
        c.message
    );
}

#[test]
fn p6t4_partial_room_is_a_number_not_a_binary() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
    let harvest = t.expected_harvest_date.expect("harvest date");
    order_trays(&mut conn, &harvest, "dun-peas", 10);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest);
    assert_eq!(c.reachability.shelf_slots_free, Some(2));
    assert_eq!(
        c.reachability.overdue_on_shelf, 0,
        "p6t4 fixture has no overdue trays"
    );
    let d = reachability::format_mon_d_local(&harvest).unwrap();
    let n = reachability::tray_word(c.short_trays);
    assert_eq!(
        c.message,
        format!("{d} is short {n} of Dun peas - sowing reaches it but there is only room for 2. Harvest early or void the order.")
    );
    assert!(!c.message.contains("Sow "));
}

#[test]
fn p6t5_overdue_tray_occupies_light_from_today_forward() {
    let mut conn = mem();
    let today = today();
    let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let growth = t.growth_days_at_sow.expect("growth_days_at_sow");
    trays::dev_backdate_tray(&mut conn, &t.id, growth + 3).unwrap();

    let occ_today = trays::occupancy_on(&conn, &today, &today).unwrap();
    assert!(
        occ_today.light >= 1,
        "overdue tray must occupy light today, got {occ_today:?}"
    );

    for extra in [1, 5, 20] {
        let future = add_days(&today, extra);
        let occ = trays::occupancy_on(&conn, &future, &today).unwrap();
        assert!(
            occ.light >= 1,
            "overdue tray must occupy light on {future}, got {occ:?}"
        );
    }
    assert_eq!(trays::overdue_trays_on_shelf(&conn, &today).unwrap(), 1);
}

#[test]
fn p6t6_capacity_formula_byte_identical_across_ceiling_settings() {
    let mut conn = mem();
    trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
    let unset = serde_json::to_vec(&trays::capacity_by_harvest_date(&conn).unwrap()).unwrap();

    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let at_88 = serde_json::to_vec(&trays::capacity_by_harvest_date(&conn).unwrap()).unwrap();

    trays::set_shelf_capacity(&conn, Some(1), Some(1)).unwrap();
    let at_11 = serde_json::to_vec(&trays::capacity_by_harvest_date(&conn).unwrap()).unwrap();

    assert_eq!(at_88, unset);
    assert_eq!(at_11, unset);
}

#[test]
fn p6t7_must_sow_today_with_zero_room_never_says_sow_n_today() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    // D3: reachability is now the SHORT crop's question. Sizing this date with
    // the library's fastest crop stopped creating the state this claim observes.
    // Same repair as p6t8 — the premise moved, the claim did not.
    let peas_days: i64 = conn
        .query_row(
            "SELECT growth_days FROM crops WHERE id = 'dun-peas'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let harvest = add_days(&today, peas_days);
    order_trays(&mut conn, &harvest, "dun-peas", 12);

    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert!(r.reachable);
    assert!(r.must_sow_today);
    assert_eq!(r.shelf_slots_free, Some(0));

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest);
    assert!(
        c.message.contains("sowing reaches it but there is no room"),
        "{}",
        c.message
    );
    assert!(
        !c.message.contains("Sow "),
        "must_sow_today must not win when there is no room: {}",
        c.message
    );
}

#[test]
fn p6t8_growth_signal_fires_at_three_dates() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();

    // D3 / D4(b): peas ready on their own date cover later pea promises.
    // The growth signal is about dates the shelf cannot serve, so the
    // promises must be a different crop — a Kale tray never covers a
    // Sunflower date.
    let d1 = add_days(&today, 10);
    let d2 = add_days(&today, 12);
    order_trays(&mut conn, &d1, "sunflower", 2);
    order_trays(&mut conn, &d2, "sunflower", 2);
    let two = reachability::shelf_pressure_on(&conn, &today).unwrap();
    assert_eq!(two.dates, 2);
    assert!(!two.firing);
    assert!(reachability::shelf_pressure_line(&two).is_none());

    let d3 = add_days(&today, 14);
    order_trays(&mut conn, &d3, "sunflower", 2);
    let three = reachability::shelf_pressure_on(&conn, &today).unwrap();
    assert_eq!(three.dates, 3);
    assert!(three.firing);
    assert_eq!(
        reachability::shelf_pressure_line(&three).as_deref(),
        Some(
            "In the next 30 days, 3 harvest dates are reachable in time but do not have enough shelf room. Your shelf, not your calendar, is the limit."
        )
    );
}

#[test]
fn p6t9_existing_tray_uses_at_sow_candidate_uses_live() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "spicy-mix", 8).unwrap();
    let at_sow = t.growth_days_at_sow.expect("growth_days_at_sow");
    let past_old_span = add_days(&today, at_sow + 6);

    let occ_before = trays::occupancy_on(&conn, &past_old_span, &today).unwrap();
    assert_eq!(occ_before.light, 0);
    assert_eq!(occ_before.blackout, 0);

    trays::update_crop_growth_days(&conn, "spicy-mix", 20, 3).unwrap();
    let occ_after = trays::occupancy_on(&conn, &past_old_span, &today).unwrap();
    assert_eq!(
        occ_after, occ_before,
        "existing tray occupancy must keep growth_days_at_sow"
    );

    // Live growth_days is now 20. A 1-day all-light candidate sits only in
    // blackout-occupied days (light pool empty). An 8-day all-light candidate
    // reaches the existing trays' light window. The span moved.
    let free_short = trays::slots_free_for_span(&conn, &today, 1, 0, &today).unwrap();
    let free_live = trays::slots_free_for_span(&conn, &today, 20, 0, &today).unwrap();
    assert_ne!(
        free_short, free_live,
        "candidate sow span must move with live crops.growth_days: short={free_short:?} live={free_live:?}"
    );
}

#[test]
fn p6t10_set_shelf_capacity_writes_no_event() {
    let conn = mem();
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    trays::set_shelf_capacity(&conn, None, None).unwrap();
    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn p6t11_fast_crop_fits_while_slow_crop_does_not() {
    let mut conn = mem();
    let today = today();
    let fast = trays::add_crop(&conn, "Fast", 2, 0, 5.0).unwrap();
    let slow = trays::add_crop(&conn, "Slow", 9, 0, 5.0).unwrap();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    trays::sow_tray(&mut conn, "spicy-mix", 8).unwrap();

    let harvest = add_days(&today, 9);
    let fast_date = reachability::for_date_for_crop_on(&conn, &harvest, &fast.id, &today).unwrap();
    let slow_date = reachability::for_date_for_crop_on(&conn, &harvest, &slow.id, &today).unwrap();
    let fast_r = fast_date
        .crops
        .iter()
        .find(|c| c.crop_id == fast.id)
        .expect("fast crop reaches the date");
    let slow_r = slow_date
        .crops
        .iter()
        .find(|c| c.crop_id == slow.id)
        .expect("slow crop reaches the date");

    assert!(
        fast_date.sow_can_serve
            && fast_date.shelf_slots_free.is_some_and(|k| k > 0)
            && fast_r.can_sow_now
            && !slow_r.can_sow_now
            && slow_r.slots_free == Some(0),
        "the sow grid ranks crops by can_sow_now; if this diverges the grid will offer a crop the shelf cannot hold. sow_can_serve={} shelf_slots_free={:?} fast.can_sow_now={} slow.can_sow_now={} slow.slots_free={:?}",
        fast_date.sow_can_serve,
        fast_date.shelf_slots_free,
        fast_r.can_sow_now,
        slow_r.can_sow_now,
        slow_r.slots_free
    );
}

#[test]
fn p6t12_unknown_ceiling_leaves_every_crop_sowable() {
    let conn = mem();
    let today = today();
    let harvest = add_days(&today, 14);
    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert!(!r.crops.is_empty());
    assert!(
        r.crops
            .iter()
            .all(|c| c.can_sow_now && c.slots_free.is_none()),
        "unknown ceiling must leave every crop sowable: {:?}",
        r.crops
            .iter()
            .map(|c| (&c.crop_id, c.can_sow_now, c.slots_free))
            .collect::<Vec<_>>()
    );
}

fn m2_of(conn: &Connection) -> health::CheckStatus {
    let debts = attention::money_debts(conn).unwrap();
    let inputs = CheckInputs {
        money: debts,
        ..CheckInputs::default()
    };
    health::severity_for("M2", None, &db::utc_now_rfc3339(), &inputs)
}

#[test]
fn p6t13_shelf_blocked_date_makes_m2_unhealthy() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let harvest = t.expected_harvest_date.expect("harvest date");
    order_trays(&mut conn, &harvest, "dun-peas", 10);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest);
    assert!(reachability::shelf_blocked(c));
    let m2 = m2_of(&conn);
    assert_eq!(m2.severity, Severity::Unhealthy);
}

#[test]
fn p6t14_m2_sentence_contains_cover_head_message_byte_for_byte() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let harvest = t.expected_harvest_date.expect("harvest date");
    order_trays(&mut conn, &harvest, "dun-peas", 10);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    assert!(!plan.is_empty());
    let m2 = m2_of(&conn);
    assert!(
        m2.sentence.contains(&plan[0].message),
        "M2 renders the plan's sentence; do not compose a second one in health.rs. m2={} plan={}",
        m2.sentence,
        plan[0].message
    );
}

#[test]
fn p6t15_time_unreachable_m2_still_unhealthy_and_names_sowing() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let dead = add_days(&today, 2);
    order_trays(&mut conn, &dead, "dun-peas", 4);

    let m2 = m2_of(&conn);
    assert_eq!(m2.severity, Severity::Unhealthy);
    assert!(
        m2.sentence.contains("cannot be fixed by sowing"),
        "{}",
        m2.sentence
    );
}

#[test]
fn p6t16_reachable_with_room_m2_is_degraded_ordinary_sow_sentence() {
    let mut conn = mem();
    let today = today();
    let harvest = add_days(&today, 14);
    order_trays(&mut conn, &harvest, "dun-peas", 2);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest);
    assert!(c.reachability.reachable);
    assert!(!reachability::shelf_blocked(c));
    let m2 = m2_of(&conn);
    assert_eq!(m2.severity, Severity::Degraded);
    assert!(
        m2.sentence.contains(&c.message),
        "M2 renders the plan's sentence; do not compose a second one in health.rs. m2={} plan={}",
        m2.sentence,
        c.message
    );
}

#[test]
fn p6t17_band_one_exactly_when_shelf_blocked() {
    fn check_all(conn: &Connection, today: &str) {
        let plan = reachability::cover_plan_on(conn, today).unwrap();
        for c in &plan {
            assert_eq!(
                reachability::band(c) == 1,
                reachability::shelf_blocked(c),
                "the one door in Step 1 holds: band={} blocked={} date={} msg={}",
                reachability::band(c),
                reachability::shelf_blocked(c),
                c.harvest_date,
                c.message
            );
        }
    }

    let today = today();
    {
        let mut conn = mem();
        let harvest = add_days(&today, 14);
        order_trays(&mut conn, &harvest, "dun-peas", 2);
        check_all(&conn, &today);
    }
    {
        let mut conn = mem();
        trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
        let t = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
        let harvest = t.expected_harvest_date.expect("harvest date");
        order_trays(&mut conn, &harvest, "dun-peas", 10);
        check_all(&conn, &today);
    }
    {
        let mut conn = mem();
        trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
        trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
        let dead = add_days(&today, 2);
        order_trays(&mut conn, &dead, "dun-peas", 4);
        check_all(&conn, &today);
    }
}

#[test]
fn p6t18_overcommit_silent_when_remaining_already_covers() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
    let harvest = t.expected_harvest_date.expect("harvest date");
    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert!(r.remaining_trays >= 4);
    let line = reachability::overcommit_line_on(&conn, &harvest, 3, &today, "dun-peas").unwrap();
    assert_eq!(line, None);
}

#[test]
fn p6t19_overcommit_silent_when_ceiling_unknown() {
    let conn = mem();
    let today = today();
    let harvest = add_days(&today, 14);
    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert!(r.shelf_slots_free.is_none());
    let line = reachability::overcommit_line_on(&conn, &harvest, 10, &today, "dun-peas").unwrap();
    assert_eq!(line, None);
}

#[test]
fn p6t20_overcommit_names_date_and_over_count() {
    let conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let harvest = add_days(&today, 14);
    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert_eq!(r.remaining_trays, 0);
    assert_eq!(r.shelf_slots_free, Some(8));
    let line = reachability::overcommit_line_on(&conn, &harvest, 10, &today, "dun-peas")
        .unwrap()
        .expect("shortfall > room must name the over-count");
    let d = reachability::format_mon_d_local(&harvest).unwrap();
    assert_eq!(
        line,
        format!(
            "{d} has room for 8 trays. This order needs 10 trays of Dun peas sown - 2 trays more than \
             the shelf can hold."
        )
    );
}

#[test]
fn p6t21_already_oversold_date_makes_shortfall_larger() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let harvest = add_days(&today, 14);
    order_trays(&mut conn, &harvest, "dun-peas", 5);
    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert!(
        r.remaining_trays < 0,
        "date is already oversold: remaining={}",
        r.remaining_trays
    );
    let trays = 6i64;
    let shortfall = trays - r.remaining_trays;
    assert_eq!(
        shortfall,
        trays + r.remaining_trays.abs(),
        "remaining_trays negative -> shortfall = trays + |remaining|"
    );
    let line = reachability::overcommit_line_on(&conn, &harvest, trays, &today, "dun-peas")
        .unwrap()
        .expect("oversold shortfall exceeds room");
    let d = reachability::format_mon_d_local(&harvest).unwrap();
    let room = r.shelf_slots_free.expect("ceiling set");
    let over = shortfall - room;
    assert_eq!(
        line,
        format!(
            "{d} has room for {}. This order needs {} of Dun peas sown - {} more than \
             the shelf can hold.",
            reachability::tray_word(room),
            reachability::tray_word(shortfall),
            reachability::tray_word(over)
        )
    );
    assert!(
        line.contains(&reachability::tray_word(shortfall)),
        "must name the larger shortfall, not the raw tray count: {line}"
    );
    assert!(
        !line.contains("needs 6 trays sown"),
        "raw tray count must not replace trays + |remaining|: {line}"
    );
}

#[test]
fn p6t22_overdue_trays_take_harvest_them_arm() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let growth = t.growth_days_at_sow.expect("growth_days_at_sow");
    trays::dev_backdate_tray(&mut conn, &t.id, growth + 3).unwrap();
    let harvest = add_days(&today, 14);
    // D4(b): the 8 overdue trays are ready on or before `harvest` and
    // would cover a 4-tray promise. Order more than they can cover so
    // the date stays short and the overdue-arm sentence can fire.
    order_trays(&mut conn, &harvest, "dun-peas", 12);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest);
    assert!(reachability::shelf_blocked(c));
    assert!(
        c.reachability.overdue_on_shelf > 0,
        "fixture must have overdue trays, got {}",
        c.reachability.overdue_on_shelf
    );
    let tail = format!(
        " {} past their harvest date and still on the shelf.",
        reachability::tray_word(c.reachability.overdue_on_shelf)
    );
    assert!(
        c.message.contains(&tail),
        "overdue tail must precede the action: {}",
        c.message
    );
    assert!(
        c.message.ends_with(" Harvest them or void the order."),
        "{}",
        c.message
    );
    let tail_at = c.message.find(&tail).expect("tail present");
    let action_at = c
        .message
        .find(" Harvest them or void the order.")
        .expect("action present");
    assert!(
        tail_at < action_at,
        "overdue tail must precede the action: {}",
        c.message
    );
}

#[test]
fn p6t23_no_overdue_trays_take_harvest_early_arm() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let harvest = t.expected_harvest_date.expect("harvest date");
    order_trays(&mut conn, &harvest, "dun-peas", 10);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest);
    assert!(reachability::shelf_blocked(c));
    assert_eq!(c.reachability.overdue_on_shelf, 0);
    assert!(
        c.message.ends_with(" Harvest early or void the order."),
        "{}",
        c.message
    );
}

#[test]
fn p6t24_m2_inherits_the_action_clause() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let harvest = t.expected_harvest_date.expect("harvest date");
    order_trays(&mut conn, &harvest, "dun-peas", 10);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let m2 = m2_of(&conn);
    assert!(
        m2.sentence.contains("Harvest early or void the order.")
            || m2.sentence.contains("Harvest them or void the order."),
        "Health inherits the action by rendering the plan's sentence; do not add one in health.rs. m2={} plan={}",
        m2.sentence,
        plan[0].message
    );
    assert!(
        m2.sentence.contains(&plan[0].message),
        "Health inherits the action by rendering the plan's sentence; do not add one in health.rs. m2={} plan={}",
        m2.sentence,
        plan[0].message
    );
}

#[test]
fn p6t25_unreachable_sentence_stays_distinct() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let dead = add_days(&today, 2);
    order_trays(&mut conn, &dead, "dun-peas", 4);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &dead);
    assert!(!c.reachability.reachable);
    assert!(
        c.message.contains("cannot be fixed by sowing"),
        "{}",
        c.message
    );
    assert!(
        !c.message.contains("Harvest them") && !c.message.contains("Harvest early"),
        "unreachable shape must stay distinct: {}",
        c.message
    );
}

fn latest_ordered_payload(conn: &Connection) -> (String, serde_json::Value, OrderedPayload) {
    let raw: String = conn
        .query_row(
            "SELECT payload FROM event_log
             WHERE kind = 'wholesale.ordered'
             ORDER BY seq DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let payload: OrderedPayload = serde_json::from_str(&raw).unwrap();
    (raw, value, payload)
}

#[test]
fn p6t26_unacked_overcommit_err_equals_overcommit_line() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let harvest = add_days(&today, 14);
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let expected = reachability::overcommit_line_on(&conn, &harvest, 10, &today, "dun-peas")
        .unwrap()
        .expect("shortfall > room");
    let err = wholesale::record_order(
        &mut conn,
        &venue.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".to_string(),
            trays: 10,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap_err();
    assert_eq!(err, expected);
}

#[test]
fn p6t27_ack_true_records_and_payload_round_trips() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let harvest = add_days(&today, 14);
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(
        &mut conn,
        &venue.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".to_string(),
            trays: 10,
            price_cents_per_tray: Some(800),
        }],
        true,
    )
    .unwrap();
    let (_raw, _value, payload) = latest_ordered_payload(&conn);
    assert!(payload.overcommit_ack);
}

#[test]
fn p6t28_normal_order_writes_overcommit_ack_false() {
    let mut conn = mem();
    let today = today();
    let harvest = add_days(&today, 14);
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(
        &mut conn,
        &venue.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".to_string(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    let (raw, value, payload) = latest_ordered_payload(&conn);
    assert!(
        value
            .as_object()
            .is_some_and(|o| o.contains_key("overcommitAck")),
        "the field is written, not omitted: {raw}"
    );
    assert_eq!(value["overcommitAck"], serde_json::json!(false));
    assert!(!payload.overcommit_ack);
}

#[test]
fn p6t29_unknown_ceiling_is_not_gated() {
    let mut conn = mem();
    let today = today();
    let harvest = add_days(&today, 14);
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert!(r.shelf_slots_free.is_none());
    wholesale::record_order(
        &mut conn,
        &venue.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".to_string(),
            trays: 10,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
}

#[test]
fn p6t30_missing_overcommit_ack_deserializes_false() {
    let json = serde_json::json!({
        "orderId": "hist-1",
        "venueId": "v1",
        "harvestDate": "2026-09-01",
        "orderedOn": "2026-08-14",
        "lines": [{"cropId": "dun-peas", "trays": 1}]
    });
    assert!(
        json.get("overcommitAck").is_none(),
        "every event in the existing log lacks this field; serde(default) is what keeps replay working."
    );
    let payload: OrderedPayload = serde_json::from_value(json).expect(
        "every event in the existing log lacks this field; serde(default) is what keeps replay working.",
    );
    assert!(
        !payload.overcommit_ack,
        "every event in the existing log lacks this field; serde(default) is what keeps replay working."
    );
}

#[test]
fn p6t31_confirmed_shelf_blocked_attention_equals_cover_plan() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let t = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let harvest = t.expected_harvest_date.expect("harvest date");
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(
        &mut conn,
        &venue.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".to_string(),
            trays: 10,
            price_cents_per_tray: Some(800),
        }],
        true,
    )
    .unwrap();
    let attn: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1",
            [&harvest],
            |r| r.get(0),
        )
        .unwrap();
    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest);
    assert!(reachability::shelf_blocked(c));
    assert_eq!(attn, c.message);
}

#[test]
fn p6t32_time_unreachable_keeps_existing_sentence() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let dead = add_days(&today, 2);
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(
        &mut conn,
        &venue.venue_id,
        &dead,
        vec![OrderLine {
            crop_id: "dun-peas".to_string(),
            trays: 4,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    let attn: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1",
            [&dead],
            |r| r.get(0),
        )
        .unwrap();
    let when_date = NaiveDate::parse_from_str(&dead, "%Y-%m-%d").unwrap();
    let when = format!("{} {}", when_date.format("%b"), when_date.day());
    assert_eq!(
        attn,
        format!(
            "{when} is committed 4 trays of Dun peas beyond what is sown. \
             It cannot be fixed by sowing - call the venue."
        )
    );
}

#[test]
fn p6t33_ahead_of_sowing_within_shelf_is_not_gated() {
    let mut conn = mem();
    let today = today();
    trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
    let harvest = add_days(&today, 14);
    let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
    assert_eq!(r.remaining_trays, 0);
    assert_eq!(r.shelf_slots_free, Some(8));
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(
        &mut conn,
        &venue.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".to_string(),
            trays: 5,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .expect("chefs order before sowing; that is never gated.");
}
