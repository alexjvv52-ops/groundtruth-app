//! HEALTH-JAR Job A — the jar inside the cover plan's sowing sentence.
//!
//! What is pinned: a crop with no receipt changes no byte of the plan; a
//! known jar above zero grows no tail; a jar at zero grows the empty tail and
//! a jar below zero the short tail, on the three sowing arms only (E
//! must-sow-today, F sow-by, G still-covers); an unweighed sow in the window
//! leaves the figure unknown, and unknown grows no tail; arms A–D
//! (unreachable, shelf-blocked) never carry it; a must-sow-today date with an
//! empty jar sorts to the head of its band and turns Health M2 Unhealthy while
//! M2 still renders the plan's own sentence, and a slack date with the same
//! jar does not; the Sow door stays offered; the Today attention row carries
//! the same bytes as the plan.
use crate::attention;
use crate::db;
use crate::health::{self, CheckInputs, Severity};
use crate::marketing;
use crate::reachability::{self, CoverDate, CoverOrder, DateReachability};
use crate::seed;
use crate::trays;
use crate::wholesale::{self, OrderLine};
use chrono::{Duration, NaiveDate};
use rusqlite::Connection;

/// The locked tail for a jar at exactly zero.
const EMPTY_TAIL: &str = " The jar is empty — record seed in on Farm, then sow.";

/// The locked tail for a short jar; `x` is the shortfall at 0.1.
fn short_tail(x: f64) -> String {
    format!(" The jar is short {x:.1} oz — record seed in on Farm, then sow.")
}

/// The same tail in grams. Through `units::grams` and `units::unit_for` — never
/// a hand-typed gram figure.
fn short_tail_metric(oz: f64) -> String {
    format!(
        " The jar is short {} {} — record seed in on Farm, then sow.",
        crate::units::grams(oz),
        crate::units::unit_for("metric")
    )
}

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

fn growth_days(conn: &Connection, crop_id: &str) -> i64 {
    conn.query_row(
        "SELECT growth_days FROM crops WHERE id = ?1",
        [crop_id],
        |r| r.get(0),
    )
    .unwrap()
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

fn cover_for<'a>(plan: &'a [CoverDate], harvest_date: &str, crop_id: &str) -> &'a CoverDate {
    plan.iter()
        .find(|c| c.harvest_date == harvest_date && c.crop_id == crop_id)
        .unwrap_or_else(|| panic!("no cover row for {harvest_date} {crop_id}: {plan:?}"))
}

/// A receipt, then one weighed sow of the same ounces: the jar reads 0.0.
fn empty_the_jar(conn: &mut Connection, crop_id: &str) {
    seed::receive_seed(conn, crop_id, 8.0).unwrap();
    trays::sow_tray_with_seed(conn, crop_id, 1, Some(8.0)).unwrap();
}

fn m2_of(conn: &Connection) -> health::CheckStatus {
    let debts = attention::money_debts(conn).unwrap();
    let inputs = CheckInputs {
        money: debts,
        ..CheckInputs::default()
    };
    health::severity_for("M2", None, &db::utc_now_rfc3339(), &inputs)
}

/// Arm F without any tail — the p6t1 bytes, built from the row's own facts.
fn sow_by_sentence(c: &CoverDate) -> String {
    let d = reachability::format_mon_d_local(&c.harvest_date).unwrap();
    let by = reachability::format_est_mon_d_local(c.reachability.last_sow_by.as_deref().unwrap())
        .unwrap();
    format!(
        "{d} is short {} of {} - sow by {by} to cover it.",
        reachability::tray_word(c.short_trays),
        c.crop_name
    )
}

/// Arm E without any tail.
fn must_sow_today_sentence(c: &CoverDate) -> String {
    let d = reachability::format_mon_d_local(&c.harvest_date).unwrap();
    format!(
        "Sow {} of {} today to cover {d} - today is the last day that reaches it.",
        reachability::tray_word(c.short_trays),
        c.crop_name
    )
}

#[test]
fn hj1_no_receipt_changes_no_byte_of_the_plan() {
    let mut conn = mem();
    let today = today();
    let harvest = add_days(&today, 14);
    order_trays(&mut conn, &harvest, "dun-peas", 3);
    // A weighed sow with no receipt behind it is not a jar (SCOPE A): no row,
    // no figure, no tail.
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest, "dun-peas");
    assert!(c.jar_on_hand_oz.is_none(), "{c:?}");
    assert!(!reachability::jar_empty(c));
    assert_eq!(c.message, sow_by_sentence(c));
    assert_eq!(reachability::band(c), 3);
    for c in &plan {
        assert!(!c.message.contains("The jar"), "{}", c.message);
    }
    // The wire shape: the new key rides beside the old ones and reads null.
    let wire = serde_json::to_value(c).unwrap();
    assert!(wire.as_object().unwrap().contains_key("jarOnHandOz"));
    assert!(wire["jarOnHandOz"].is_null());

    // SCOPE C: another crop's jar - even an empty one - is not this date's.
    // The plan's bytes do not move.
    let before = serde_json::to_vec(&plan).unwrap();
    empty_the_jar(&mut conn, "kale");
    let after = serde_json::to_vec(&reachability::cover_plan_on(&conn, &today).unwrap()).unwrap();
    assert_eq!(after, before);
}

#[test]
fn hj2_a_known_jar_above_zero_grows_no_tail() {
    let mut conn = mem();
    let today = today();
    seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    let harvest = add_days(&today, 14);
    order_trays(&mut conn, &harvest, "dun-peas", 3);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest, "dun-peas");
    assert_eq!(c.jar_on_hand_oz, Some(8.0), "{c:?}");
    assert!(!reachability::jar_empty(c));
    assert_eq!(c.message, sow_by_sentence(c));
    let wire = serde_json::to_value(c).unwrap();
    assert_eq!(wire["jarOnHandOz"], serde_json::json!(8.0));
}

#[test]
fn hj3_a_jar_at_zero_grows_the_empty_tail_on_the_three_sowing_arms() {
    let today = today();
    // E - must-sow-today. The one tray sown today lands on this very date, so
    // the promise is sized past it to keep the date short.
    {
        let mut conn = mem();
        empty_the_jar(&mut conn, "dun-peas");
        let harvest = add_days(&today, growth_days(&conn, "dun-peas"));
        order_trays(&mut conn, &harvest, "dun-peas", 3);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        assert!(c.reachability.must_sow_today, "{c:?}");
        assert_eq!(c.jar_on_hand_oz, Some(0.0), "{c:?}");
        assert!(reachability::jar_empty(c));
        assert_eq!(
            c.message,
            format!("{}{EMPTY_TAIL}", must_sow_today_sentence(c))
        );
        assert_eq!(reachability::band(c), 2);
    }
    // F - sow by, slack.
    let f = {
        let mut conn = mem();
        empty_the_jar(&mut conn, "dun-peas");
        let harvest = add_days(&today, 14);
        order_trays(&mut conn, &harvest, "dun-peas", 3);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas").clone();
        assert!(!c.reachability.must_sow_today, "{c:?}");
        assert!(c.reachability.last_sow_by.is_some(), "{c:?}");
        assert_eq!(c.jar_on_hand_oz, Some(0.0), "{c:?}");
        assert!(reachability::jar_empty(&c));
        assert_eq!(c.message, format!("{}{EMPTY_TAIL}", sow_by_sentence(&c)));
        assert_eq!(reachability::band(&c), 3);
        c
    };
    // G - still covers. Reachable with no sow-by date is not a state the plan
    // builds from the crop table, so the arm is reached the way
    // crop_library_tests reaches cover_message: from a literal.
    {
        let g = CoverDate {
            message: String::new(),
            reachability: DateReachability {
                last_sow_by: None,
                ..f.reachability.clone()
            },
            ..f.clone()
        };
        assert!(reachability::jar_empty(&g));
        let d = reachability::format_mon_d_local(&g.harvest_date).unwrap();
        assert_eq!(
            reachability::cover_message(&g),
            format!(
                "{d} is short {} of Dun peas - sowing still covers it.{EMPTY_TAIL}",
                reachability::tray_word(g.short_trays)
            )
        );
    }
}

#[test]
fn hj4_a_short_jar_names_the_ounces_at_one_decimal() {
    let today = today();
    {
        let mut conn = mem();
        seed::receive_seed(&mut conn, "dun-peas", 6.0).unwrap();
        trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
        let harvest = add_days(&today, 14);
        order_trays(&mut conn, &harvest, "dun-peas", 3);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        assert_eq!(c.jar_on_hand_oz, Some(-2.0), "{c:?}");
        assert!(reachability::jar_empty(c));
        assert_eq!(
            c.message,
            format!("{}{}", sow_by_sentence(c), short_tail(2.0))
        );
        assert!(c
            .message
            .ends_with(" The jar is short 2.0 oz — record seed in on Farm, then sow."));
    }
    // The figure is the jar's own, at 0.1: 6.0 in, 8.4 out reads short 2.4.
    {
        let mut conn = mem();
        seed::receive_seed(&mut conn, "dun-peas", 6.0).unwrap();
        trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.4)).unwrap();
        let harvest = add_days(&today, 14);
        order_trays(&mut conn, &harvest, "dun-peas", 3);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        assert_eq!(c.jar_on_hand_oz, Some(-2.4), "{c:?}");
        assert!(c
            .message
            .ends_with(" The jar is short 2.4 oz — record seed in on Farm, then sow."));
    }
}

#[test]
fn hj5_an_unweighed_sow_leaves_the_jar_unknown_and_unknown_grows_no_tail() {
    let mut conn = mem();
    let today = today();
    seed::receive_seed(&mut conn, "dun-peas", 8.0).unwrap();
    // One blank sow in the window, then a weighed sow of every ounce received.
    // Were the blank read as zero the jar would be empty; it is unknown.
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
    let harvest = add_days(&today, 14);
    order_trays(&mut conn, &harvest, "dun-peas", 4);

    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest, "dun-peas");
    assert!(c.jar_on_hand_oz.is_none(), "{c:?}");
    assert!(!reachability::jar_empty(c));
    assert_eq!(c.message, sow_by_sentence(c));
    let wire = serde_json::to_value(c).unwrap();
    assert!(wire["jarOnHandOz"].is_null());
}

#[test]
fn hj6_arms_a_to_d_never_carry_the_tail() {
    let today = today();
    // B - unreachable. The jar is empty and the date still says call the venue.
    let b = {
        let mut conn = mem();
        empty_the_jar(&mut conn, "dun-peas");
        let dead = add_days(&today, 2);
        order_trays(&mut conn, &dead, "dun-peas", 4);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &dead, "dun-peas").clone();
        assert!(!c.reachability.reachable, "{c:?}");
        assert_eq!(c.jar_on_hand_oz, Some(0.0), "{c:?}");
        assert!(
            !reachability::jar_empty(&c),
            "unreachable is never jar-empty"
        );
        assert!(
            c.message
                .contains("cannot be fixed by sowing - call the venue."),
            "{}",
            c.message
        );
        assert!(!c.message.contains("The jar"), "{}", c.message);
        assert_eq!(reachability::band(&c), 0);
        c
    };
    // A - unreachable, settle by delivering. Reached from a literal, as the
    // plan cannot be asked for a harvested tray on a date that is today.
    {
        let a = CoverDate {
            message: String::new(),
            harvested_trays: 1,
            orders: vec![CoverOrder {
                order_id: "o1".into(),
                venue_name: "Cafe".into(),
                state: "ordered".into(),
                trays: 4,
            }],
            reachability: DateReachability {
                days_until_harvest: 0,
                ..b.reachability.clone()
            },
            ..b.clone()
        };
        assert!(reachability::settle_by_delivering(&a));
        assert!(!reachability::jar_empty(&a));
        let msg = reachability::cover_message(&a);
        assert!(
            msg.ends_with("cannot be fixed by sowing - deliver or void the open order."),
            "{msg}"
        );
        assert!(!msg.contains("The jar"), "{msg}");
    }
    // C - shelf-blocked, no room. The p6t2 bytes, jar empty behind them.
    {
        let mut conn = mem();
        trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
        seed::receive_seed(&mut conn, "dun-peas", 8.0).unwrap();
        let t = trays::sow_tray_with_seed(&mut conn, "dun-peas", 8, Some(8.0)).unwrap();
        let harvest = t.expected_harvest_date.expect("harvest date");
        order_trays(&mut conn, &harvest, "dun-peas", 10);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        assert!(reachability::shelf_blocked(c));
        assert_eq!(c.jar_on_hand_oz, Some(0.0), "{c:?}");
        assert!(
            !reachability::jar_empty(c),
            "shelf-blocked is never jar-empty"
        );
        let d = reachability::format_mon_d_local(&harvest).unwrap();
        let n = reachability::tray_word(c.short_trays);
        assert_eq!(
            c.message,
            format!("{d} is short {n} of Dun peas - sowing reaches it but there is no room. Harvest early or void the order.")
        );
        assert_eq!(reachability::band(c), 1);
    }
    // D - shelf-blocked, partial room. The p6t4 bytes, jar empty behind them.
    {
        let mut conn = mem();
        trays::set_shelf_capacity(&conn, Some(8), Some(8)).unwrap();
        seed::receive_seed(&mut conn, "dun-peas", 8.0).unwrap();
        let t = trays::sow_tray_with_seed(&mut conn, "dun-peas", 6, Some(8.0)).unwrap();
        let harvest = t.expected_harvest_date.expect("harvest date");
        order_trays(&mut conn, &harvest, "dun-peas", 10);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        assert_eq!(c.reachability.shelf_slots_free, Some(2));
        assert_eq!(c.jar_on_hand_oz, Some(0.0), "{c:?}");
        assert!(!reachability::jar_empty(c));
        let d = reachability::format_mon_d_local(&harvest).unwrap();
        let n = reachability::tray_word(c.short_trays);
        assert_eq!(
            c.message,
            format!("{d} is short {n} of Dun peas - sowing reaches it but there is only room for 2. Harvest early or void the order.")
        );
        assert_eq!(reachability::band(c), 1);
    }
}

#[test]
fn hj7_must_sow_today_with_an_empty_jar_heads_its_band_and_m2_is_unhealthy() {
    let today = today();
    // One date: red, and the sentence is the plan's, tail and all.
    {
        let mut conn = mem();
        empty_the_jar(&mut conn, "dun-peas");
        let harvest = add_days(&today, growth_days(&conn, "dun-peas"));
        order_trays(&mut conn, &harvest, "dun-peas", 3);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        assert_eq!(plan.len(), 1, "{plan:?}");
        let c = &plan[0];
        assert!(
            c.reachability.must_sow_today && reachability::jar_empty(c),
            "{c:?}"
        );
        let m2 = m2_of(&conn);
        assert_eq!(m2.severity, Severity::Unhealthy);
        assert_eq!(m2.sentence, format!("M2 Promises covered — {}", c.message));
        assert!(m2.sentence.ends_with(EMPTY_TAIL), "{}", m2.sentence);
    }
    // The same date with a jar that holds seed is the Degraded it always was:
    // must-sow-today alone does not turn red, and the sentence has no tail.
    {
        let mut conn = mem();
        seed::receive_seed(&mut conn, "dun-peas", 16.0).unwrap();
        trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(8.0)).unwrap();
        let harvest = add_days(&today, growth_days(&conn, "dun-peas"));
        order_trays(&mut conn, &harvest, "dun-peas", 3);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = &plan[0];
        assert!(
            c.reachability.must_sow_today && !reachability::jar_empty(c),
            "{c:?}"
        );
        let m2 = m2_of(&conn);
        assert_eq!(m2.severity, Severity::Degraded);
        assert_eq!(m2.sentence, format!("M2 Promises covered — {}", c.message));
        assert!(!m2.sentence.contains("The jar"), "{}", m2.sentence);
    }
    // TRIGGER C: a slack date with an empty jar carries the tail and stays
    // Degraded. Red is only for the last day.
    {
        let mut conn = mem();
        empty_the_jar(&mut conn, "dun-peas");
        let harvest = add_days(&today, 14);
        order_trays(&mut conn, &harvest, "dun-peas", 3);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = &plan[0];
        assert!(
            !c.reachability.must_sow_today && reachability::jar_empty(c),
            "{c:?}"
        );
        let m2 = m2_of(&conn);
        assert_eq!(m2.severity, Severity::Degraded);
        assert_eq!(m2.sentence, format!("M2 Promises covered — {}", c.message));
        assert!(m2.sentence.ends_with(EMPTY_TAIL), "{}", m2.sentence);
    }
    // Inside band 2 the empty jar sorts first. Broccoli (8 days) must also be
    // sown today for an earlier date and would head the band by date alone;
    // the Dun peas date, jar empty, is the head M2 renders. No new band.
    {
        let mut conn = mem();
        empty_the_jar(&mut conn, "dun-peas");
        let peas_date = add_days(&today, growth_days(&conn, "dun-peas"));
        let broccoli_date = add_days(&today, growth_days(&conn, "broccoli"));
        assert!(broccoli_date < peas_date, "{broccoli_date} {peas_date}");
        order_trays(&mut conn, &peas_date, "dun-peas", 3);
        order_trays(&mut conn, &broccoli_date, "broccoli", 2);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        assert_eq!(plan.len(), 2, "{plan:?}");
        assert_eq!(plan[0].crop_id, "dun-peas", "{plan:?}");
        assert_eq!(plan[1].crop_id, "broccoli", "{plan:?}");
        for c in &plan {
            assert!(c.reachability.must_sow_today, "{c:?}");
            assert_eq!(reachability::band(c), 2, "{c:?}");
            assert_eq!(
                reachability::band(c) == 1,
                reachability::shelf_blocked(c),
                "p6t17 holds: {c:?}"
            );
        }
        assert!(reachability::jar_empty(&plan[0]));
        assert!(!reachability::jar_empty(&plan[1]));
        let m2 = m2_of(&conn);
        assert_eq!(m2.severity, Severity::Unhealthy);
        assert!(
            m2.sentence
                .starts_with(&format!("M2 Promises covered — {}", plan[0].message)),
            "{}",
            m2.sentence
        );
    }
}

#[test]
fn hj8_sow_stays_offered_when_the_jar_is_empty() {
    let today = today();
    let last_day = growth_days(&mem(), "dun-peas");
    for days in [last_day, 14] {
        let mut conn = mem();
        empty_the_jar(&mut conn, "dun-peas");
        let harvest = add_days(&today, days);
        order_trays(&mut conn, &harvest, "dun-peas", 3);
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        assert!(reachability::jar_empty(c), "{c:?}");
        assert!(c.message.ends_with(EMPTY_TAIL), "{}", c.message);
        // Attention, not a gate: the offer never reads the jar.
        assert!(c.reachability.sow_can_serve, "{c:?}");
        assert!(c.reachability.crops[0].can_sow_now, "{c:?}");
        let r = reachability::for_date_for_crop_on(&conn, &harvest, "dun-peas", &today).unwrap();
        assert!(r.sow_can_serve);
        assert!(!c.message.contains("cannot be fixed"), "{}", c.message);
    }
}

#[test]
fn hj9_the_attention_row_carries_the_tail() {
    let mut conn = mem();
    let today = today();
    empty_the_jar(&mut conn, "dun-peas");
    let harvest = add_days(&today, 14);
    order_trays(&mut conn, &harvest, "dun-peas", 3);

    let items = attention::check_attention(&conn).unwrap();
    let entity_id = format!("{harvest}|dun-peas");
    let row = items
        .iter()
        .find(|a| a.kind == "money.capacity_short" && a.entity_id.as_deref() == Some(&entity_id))
        .unwrap_or_else(|| panic!("no capacity_short row for {entity_id}: {items:?}"));
    let plan = reachability::cover_plan_on(&conn, &today).unwrap();
    let c = cover_for(&plan, &harvest, "dun-peas");
    assert!(reachability::jar_empty(c), "{c:?}");
    assert_eq!(row.message, c.message);
    assert!(row.message.ends_with(EMPTY_TAIL), "{}", row.message);
}

#[test]
fn hj10_a_short_jar_prints_grams_when_the_farm_reads_metric() {
    let today = today();
    // 6.0 in, 8.0 out reads short 2.0; 6.0 in, 8.4 out reads short 2.4.
    for (sown, short) in [(8.0_f64, 2.0_f64), (8.4_f64, 2.4_f64)] {
        let mut conn = mem();
        seed::receive_seed(&mut conn, "dun-peas", 6.0).unwrap();
        trays::sow_tray_with_seed(&mut conn, "dun-peas", 1, Some(sown)).unwrap();
        let harvest = add_days(&today, 14);
        order_trays(&mut conn, &harvest, "dun-peas", 3);
        // Imperial is the shipped default: the signed bytes are unchanged.
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        assert!(c.message.ends_with(&short_tail(short)), "{}", c.message);
        // The pick moves the face and nothing else. The jar still holds ounces.
        crate::units::set_farm_units(&conn, "metric").unwrap();
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        assert_eq!(
            c.jar_on_hand_oz,
            Some(-short),
            "the file still weighs in oz"
        );
        assert!(
            c.message.ends_with(&short_tail_metric(short)),
            "{}",
            c.message
        );
        assert!(!c.message.contains(" oz"), "{}", c.message);
        assert!(reachability::jar_empty(c), "{c:?}");
        // And back: changing the pick converts nothing written.
        crate::units::set_farm_units(&conn, "imperial").unwrap();
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        assert!(c.message.ends_with(&short_tail(short)), "{}", c.message);
    }
}

#[test]
fn hj11_the_empty_tail_is_the_same_bytes_in_both_systems() {
    let mut conn = mem();
    let today = today();
    empty_the_jar(&mut conn, "dun-peas");
    let harvest = add_days(&today, 14);
    order_trays(&mut conn, &harvest, "dun-peas", 3);
    for system in ["imperial", "metric"] {
        crate::units::set_farm_units(&conn, system).unwrap();
        let plan = reachability::cover_plan_on(&conn, &today).unwrap();
        let c = cover_for(&plan, &harvest, "dun-peas");
        // No figure, no unit word: an empty jar says the same thing either way.
        assert!(c.message.ends_with(EMPTY_TAIL), "{system}: {}", c.message);
    }
}
