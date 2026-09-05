//! B6 — operator-defined crops, rename-safe standing demand, estimate dates.

use crate::db;
use crate::marketing;
use crate::reachability;
use crate::trays;
use rusqlite::Connection;
use std::collections::BTreeMap;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn add_days(iso: &str, n: i64) -> String {
    let d = chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d").unwrap();
    (d + chrono::Duration::days(n))
        .format("%Y-%m-%d")
        .to_string()
}

#[test]
fn b6t1_add_crop_sows_and_dates_from_its_own_growth_days() {
    let mut conn = mem();
    let basil = trays::add_crop(&conn, "Basil", 14, 4, 6.0).unwrap();
    assert_eq!(basil.id, "basil");
    assert_eq!(basil.name, "Basil");
    assert_eq!(basil.growth_days, 14);
    assert_eq!(basil.blackout_days, 4);
    assert!(basil.seed_rate_oz_per_tray.is_none());

    let tray = trays::sow_tray(&mut conn, &basil.id, 1).unwrap();
    let sown = tray.sown_on.as_deref().expect("sown_on");
    let want_h = add_days(sown, 14);
    let want_c = add_days(sown, 4);
    assert_eq!(tray.expected_harvest_date.as_deref(), Some(want_h.as_str()));
    assert_eq!(tray.cover_check_date.as_deref(), Some(want_c.as_str()));
}

#[test]
fn b6t2_growth_day_edit_moves_planning_not_history() {
    let mut conn = mem();
    let x = trays::add_crop(&conn, "X", 9, 3, 6.0).unwrap();
    let tray = trays::sow_tray(&mut conn, &x.id, 1).unwrap();
    let harvest_before = tray.expected_harvest_date.clone();
    assert!(harvest_before.is_some());

    trays::update_crop_growth_days(&conn, &x.id, 6, 3).unwrap();
    let after = trays::get_tray(&conn, &tray.id).unwrap();
    assert_eq!(
        after.expected_harvest_date, harvest_before,
        "sown tray dates must not move"
    );

    let today = db::local_date_today();
    let target = add_days(&today, 7);
    let r = reachability::for_date_for_crop_on(&conn, &target, &x.id, &today).unwrap();
    let reach = r
        .crops
        .iter()
        .find(|c| c.crop_id == x.id)
        .expect("X still listed for a date 7 days out");
    assert_eq!(reach.growth_days, 6);
    assert_eq!(reach.sow_by, add_days(&target, -6));
}

#[test]
fn b6t3_rename_keeps_the_standing_target_and_its_shortfall() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let mut targets = BTreeMap::new();
    targets.insert("Dun peas".into(), 5);
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &db::local_date_today(),
        Some(5),
        Some(vec!["Dun peas".into()]),
        Some(targets),
        None,
    )
    .unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();

    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.varieties.len(), 1);
    assert_eq!(d.varieties[0].name, "Dun peas");
    assert_eq!(d.varieties[0].trays_week, 5);
    assert_eq!(d.varieties[0].sown_last_7_days, 2);
    assert_eq!(d.varieties[0].shortfall, 3);

    trays::rename_crop(&mut conn, "dun-peas", "Green pea shoots").unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.varieties.len(), 1);
    assert_eq!(d.varieties[0].name, "Green pea shoots");
    assert_eq!(d.varieties[0].trays_week, 5);
    assert_eq!(d.varieties[0].sown_last_7_days, 2);
    assert_eq!(d.varieties[0].shortfall, 3);
    assert!(
        d.standing_varieties.iter().any(|n| n == "Green pea shoots"),
        "{:?}",
        d.standing_varieties
    );
    assert!(
        !d.standing_varieties.iter().any(|n| n == "Dun peas"),
        "{:?}",
        d.standing_varieties
    );

    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.varieties[0].name, "Green pea shoots");
    assert_eq!(d.varieties[0].sown_last_7_days, 3);
    assert_eq!(d.varieties[0].shortfall, 2);
}

#[test]
fn b6t4_a_live_crop_name_outranks_a_former_one() {
    let mut conn = mem();
    let a = trays::add_crop(&conn, "Basil", 14, 4, 6.0).unwrap();
    trays::rename_crop(&mut conn, &a.id, "Thai basil").unwrap();
    let b = trays::add_crop(&conn, "Basil", 10, 3, 5.0).unwrap();

    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM crop_aliases WHERE former_name = 'Basil'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0, "live Basil must kill the former-name alias");

    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let mut targets = BTreeMap::new();
    targets.insert("Basil".into(), 3);
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &db::local_date_today(),
        Some(3),
        Some(vec!["Basil".into()]),
        Some(targets),
        None,
    )
    .unwrap();
    trays::sow_tray(&mut conn, &b.id, 1).unwrap();
    trays::sow_tray(&mut conn, &a.id, 2).unwrap();

    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.varieties.len(), 1);
    assert_eq!(d.varieties[0].name, "Basil");
    assert_eq!(
        d.varieties[0].sown_last_7_days, 1,
        "Basil target must resolve to the live Basil (B), not Thai basil (A)"
    );
    assert_eq!(d.varieties[0].shortfall, 2);
}

#[test]
fn b6t5_sow_by_dates_say_estimate_and_the_unreachable_clause_does_not_move() {
    let conn = mem();
    let today = db::local_date_today();
    let reachable = add_days(&today, 10);
    let r = reachability::for_date_for_crop(&conn, &reachable, "dun-peas").unwrap();
    assert!(r.reachable, "10 days out must still be sowable");
    let msg = reachability::cover_message(&reachability::CoverDate {
        harvest_date: reachable.clone(),
        crop_id: r.crop_id.clone(),
        crop_name: r.crop_name.clone(),
        short_trays: 2,
        message: String::new(),
        reachability: r,
        orders: vec![],
        harvested_trays: 0,
        jar_on_hand_oz: None,
    });
    assert!(msg.contains("(est.)"), "{msg}");
    assert!(msg.contains("sow by"), "{msg}");

    let dead = add_days(&today, 2);
    let r = reachability::for_date_for_crop(&conn, &dead, "dun-peas").unwrap();
    assert!(!r.reachable, "2 days out must be unreachable");
    let msg = reachability::cover_message(&reachability::CoverDate {
        harvest_date: dead,
        crop_id: r.crop_id.clone(),
        crop_name: r.crop_name.clone(),
        short_trays: 2,
        message: String::new(),
        reachability: r,
        orders: vec![],
        harvested_trays: 0,
        jar_on_hand_oz: None,
    });
    assert!(msg.contains("cannot be fixed by sowing"), "{msg}");
    assert!(msg.contains("call the venue"), "{msg}");
    assert!(!msg.contains("sow by"), "{msg}");
    assert!(msg.contains("(est.)"), "{msg}");
}
