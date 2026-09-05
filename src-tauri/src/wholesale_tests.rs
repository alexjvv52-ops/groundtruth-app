//! Phase 5 — wholesale order book (GT-D14).

use crate::attention;
use crate::db;
use crate::event_file;
use crate::events::{self, EventRecord, Kind};
use crate::export;
use crate::health::{self, CheckInputs, CheckStatus, Severity};
use crate::income::{self, RecordIncomeInput};
use crate::marketing;
use crate::offers;
use crate::projection;
use crate::reachability;
use crate::trays;
use crate::wholesale::{self, BadDebtPayload, OrderLine, OrderedPayload};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-ws-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn remaining_for(conn: &Connection, harvest_date: &str) -> i64 {
    trays::remaining_for_date(conn, harvest_date).unwrap()
}

fn venue(conn: &mut Connection) -> marketing::VenueView {
    marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap()
}

fn write_event_err(conn: &mut Connection, event: &EventRecord) -> String {
    let tx = conn.transaction().unwrap();
    let err = events::write_event(&tx, event).unwrap_err();
    drop(tx);
    err
}

/// An order that predates the upstream price gate. `record_order` refuses
/// to create one now, so this is how a farm carries one: the event is in
/// the log and replay applies it. Payload validation still accepts an
/// absent price on purpose.
pub(crate) fn inherited_unpriced_order(
    conn: &mut Connection,
    venue_id: &str,
    harvest_date: &str,
    crop_id: &str,
    trays: i64,
) -> String {
    inherit_wholesale_ordered(
        conn,
        venue_id,
        harvest_date,
        vec![OrderLine {
            crop_id: crop_id.into(),
            trays,
            price_cents_per_tray: None,
        }],
    )
}

/// Mirrors `record_order`'s OrderedPayload construction and `write_pair`
/// call exactly, minus the new upstream price gate.
pub(crate) fn inherit_wholesale_ordered(
    conn: &mut Connection,
    venue_id: &str,
    harvest_date: &str,
    lines: Vec<OrderLine>,
) -> String {
    let today = db::local_date_today();
    let order_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let payload = OrderedPayload {
        order_id: order_id.clone(),
        venue_id: venue_id.to_string(),
        harvest_date: harvest_date.to_string(),
        ordered_on: today,
        lines,
        overcommit_ack: false,
    };
    let event = EventRecord::originated(
        Kind::WholesaleOrdered,
        "wholesale_order",
        order_id.clone(),
        serde_json::to_value(&payload).unwrap(),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &event).unwrap();
    events::insert_event(&tx, &event).unwrap();
    tx.commit().unwrap();
    order_id
}

#[test]
fn phase5ws_seal_refusals() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = tray.expected_harvest_date.clone().unwrap();

    let err = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 0,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap_err();
    assert!(err.contains("trays"), "{err}");

    let err = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        "2026-13-40",
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap_err();
    assert!(
        err.to_lowercase().contains("date") || err.contains("YYYY-MM-DD"),
        "{err}"
    );

    let err = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![
            OrderLine {
                crop_id: "dun-peas".into(),
                trays: 1,
                price_cents_per_tray: Some(800),
            },
            OrderLine {
                crop_id: "dun-peas".into(),
                trays: 1,
                price_cents_per_tray: Some(800),
            },
        ],
        false,
    )
    .unwrap_err();
    assert!(err.contains("duplicate"), "{err}");

    let now = projection::handler_now();
    let paid_empty = EventRecord::originated(
        Kind::WholesalePaid,
        "wholesale_order",
        "missing-order",
        json!({
            "orderId": "missing-order",
            "paidOn": "2026-08-13",
            "incomeEventId": ""
        }),
        json!({ "op": "none" }),
        now,
        None,
        None,
        Some("ev-paid-empty".into()),
    );
    let err = write_event_err(&mut conn, &paid_empty);
    assert!(err.contains("income_event_id"), "{err}");

    let first = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &db::local_date_today(),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(500),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &first.id, None).unwrap();
    wholesale::pay_order(
        &mut conn,
        &first.id,
        500,
        &db::local_date_today(),
        None,
        true,
        false,
        None,
    )
    .unwrap();

    let err = wholesale::deliver_order(&mut conn, &first.id, None).unwrap_err();
    assert!(err.contains("delivered") && err.contains("paid"), "{err}");

    let err = wholesale::void_order(&mut conn, &first.id, None).unwrap_err();
    assert!(err.contains("voided") && err.contains("paid"), "{err}");
}

#[test]
fn phase5ws_lifecycle_and_capacity() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let _tray_id = tray_maturing_today(&mut conn, "dun-peas", 3);
    let d = db::local_date_today();
    let before = remaining_for(&conn, &d);
    assert_eq!(before, 3);

    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    assert_eq!(remaining_for(&conn, &d), 1);

    let delivered = wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    assert_eq!(delivered.state, "delivered");
    assert!(delivered.delivered_on.is_some());

    let paid = wholesale::pay_order(
        &mut conn,
        &order.id,
        1600,
        &db::local_date_today(),
        None,
        true,
        false,
        None,
    )
    .unwrap();
    assert_eq!(paid.state, "paid");
    let income_id = paid.income_event_id.clone().expect("income_event_id");
    let income_rows = income::list_income(&conn).unwrap();
    let row = income_rows
        .iter()
        .find(|r| r.income_id == income_id)
        .expect("income row");
    assert_eq!(row.source, v.name);
    let produce = crate::categories::INCOME_CATEGORIES
        .iter()
        .find(|c| c.name == "Produce you grew")
        .unwrap();
    assert_eq!(row.canonical_category, produce.id);
    assert_eq!(paid.income_event_id.as_deref(), Some(income_id.as_str()));

    for kind in [
        "wholesale.ordered",
        "wholesale.delivered",
        "wholesale.paid",
        "income.received",
    ] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE kind = ?1",
                [kind],
                |r| r.get(0),
            )
            .unwrap();
        assert!(n >= 1, "missing {kind}");
    }

    let second = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    // D2(b): delivery is not evidence. A's delivered-and-paid claim still
    // reserves until harvest, so both orders consume the 3 growing trays.
    assert_eq!(remaining_for(&conn, &d), 0);
    wholesale::void_order(&mut conn, &second.id, Some("changed mind".into())).unwrap();
    assert_eq!(remaining_for(&conn, &d), 1);
}

#[test]
fn phase5ws_mix_decomposes_and_shop_cannot_resell() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let a = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let b = trays::sow_tray(&mut conn, "kale", 1).unwrap();
    let d = a.expected_harvest_date.clone().unwrap();
    assert_eq!(b.expected_harvest_date.as_deref(), Some(d.as_str()));

    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![
            OrderLine {
                crop_id: "dun-peas".into(),
                trays: 1,
                price_cents_per_tray: Some(800),
            },
            OrderLine {
                crop_id: "kale".into(),
                trays: 1,
                price_cents_per_tray: Some(800),
            },
        ],
        false,
    )
    .unwrap();
    assert_eq!(order.lines.len(), 2);

    let offers = offers::list_offers(&conn, &d).unwrap();
    let peas = offers.iter().find(|o| o.crop_id == "dun-peas").unwrap();
    let kale = offers.iter().find(|o| o.crop_id == "kale").unwrap();
    assert_eq!(peas.sold, 1);
    assert_eq!(kale.sold, 1);

    let listings = offers::shop_listings(&conn).unwrap();
    assert!(
        listings
            .iter()
            .all(|o| o.harvest_date != d || (o.crop_id != "dun-peas" && o.crop_id != "kale")),
        "shop_listings must exclude wholesale-committed trays: {listings:?}"
    );
}

#[test]
fn phase5ws_overcommit_raises_attention() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = tray.expected_harvest_date.clone().unwrap();

    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 5,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();

    let items: Vec<(String, Option<String>, Option<String>, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT kind, entity_type, entity_id, message FROM attention
                 WHERE kind = 'wholesale.overcommitted' AND resolved_at IS NULL",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].1.as_deref(), Some("harvest_date"));
    assert_eq!(items[0].2.as_deref(), Some(d.as_str()));
    assert!(items[0].3.contains("committed"));

    trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    let n_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1 AND resolved_at IS NULL",
            [&d],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n_after, 1, "sowing more must not retro-raise a duplicate");
}

#[test]
fn phase5ws_export_carries_states() {
    let dir = temp_dir("export");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let v = venue(&mut conn);
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    let d = db::local_date_today();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(700),
        }],
        false,
    )
    .unwrap();
    let delivered_on = "2026-08-10".to_string();
    wholesale::deliver_order(&mut conn, &order.id, Some(delivered_on.clone())).unwrap();
    event_file::flush_events(&conn, &dir).unwrap();

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(result.bundle_path);
    let manifest = fs::read_to_string(bundle.join("manifest.json")).unwrap();
    assert!(manifest.contains("wholesale.csv"));
    let csv = fs::read_to_string(bundle.join("wholesale.csv")).unwrap();
    assert!(csv.contains(&order.id));
    assert!(csv.contains("delivered"));
    assert!(csv.contains(&delivered_on));
    let unpaid = csv
        .lines()
        .find(|l| l.contains(&order.id))
        .expect("order line");
    let cols: Vec<&str> = unpaid.split(',').collect();
    // paid_on is column 7 (0-based index 6)
    assert_eq!(
        cols[6], "",
        "paid_on must be empty for delivered-unpaid: {unpaid}"
    );

    let _ = fs::remove_dir_all(&dir);
}

fn days_from_today(n: i64) -> String {
    (chrono::Local::now().date_naive() + chrono::Duration::days(n))
        .format("%Y-%m-%d")
        .to_string()
}

fn open_kinds(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT kind FROM attention WHERE resolved_at IS NULL ORDER BY created_at, id")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[test]
fn b1_clean_farm_has_no_money_debts() {
    let mut conn = mem();
    trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    attention::check_attention(&conn).unwrap();
    let kinds = open_kinds(&conn);
    for k in attention::MONEY_DEBT_KINDS {
        assert!(
            !kinds.iter().any(|open| open == k),
            "clean farm must not open {k}: {kinds:?}"
        );
    }
    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.deliveries, 0);
    assert_eq!(owed.total_cents, None);
    assert_eq!(owed.oldest_days, None);
}

#[test]
fn b1_delivery_due_raises_for_today_and_past_then_clears_on_deliver() {
    let mut conn = mem();
    let v = venue(&mut conn);
    trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();

    let first = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(0),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let due: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT entity_id, message FROM attention
                 WHERE kind = 'money.delivery_due' AND resolved_at IS NULL
                 ORDER BY created_at, id",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].0, first.id);
    assert!(due[0].1.contains("due today"), "{}", due[0].1);
    assert!(due[0].1.contains(&v.name), "{}", due[0].1);

    let second = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(-2),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let due: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT entity_id, message FROM attention
                 WHERE kind = 'money.delivery_due' AND resolved_at IS NULL
                 ORDER BY created_at, id",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(due.len(), 2);
    assert_eq!(due[1].0, second.id);
    assert!(due[1].1.contains("2 days ago"), "{}", due[1].1);

    wholesale::deliver_order(&mut conn, &first.id, None).unwrap();
    attention::check_attention(&conn).unwrap();
    let due_kinds: Vec<String> = open_kinds(&conn)
        .into_iter()
        .filter(|k| k == "money.delivery_due")
        .collect();
    assert_eq!(due_kinds.len(), 1);
    let unpaid: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.delivered_unpaid' AND entity_id = ?1 AND resolved_at IS NULL",
            [&first.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unpaid, 1);
    let past_still: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.delivery_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&second.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(past_still, 1);
}

#[test]
fn b1_delivered_unpaid_raises_ages_and_clears_on_pay() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    attention::check_attention(&conn).unwrap();
    let open: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, message FROM attention
                 WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(open.len(), 1);
    assert!(open[0].1.contains("$16.00"), "{}", open[0].1);
    assert!(open[0].1.contains(&v.name), "{}", open[0].1);
    let row_id = open[0].0.clone();

    conn.execute(
        "UPDATE wholesale_orders SET delivered_on = ?1, harvest_date = ?1 WHERE id = ?2",
        [days_from_today(-5), order.id.clone()],
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let again: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, message FROM attention
                 WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(again.len(), 1);
    assert_eq!(
        again[0].0, row_id,
        "raise_or_refresh must keep the same open row"
    );
    assert!(again[0].1.contains("5 days ago"), "{}", again[0].1);

    wholesale::pay_order(
        &mut conn,
        &order.id,
        1600,
        &db::local_date_today(),
        None,
        true,
        false,
        None,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let still: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(still, 0);
    let resolved_by: String = conn
        .query_row(
            "SELECT resolved_by FROM attention WHERE id = ?1",
            [&row_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(resolved_by, "condition_cleared");
}

#[test]
fn b1_capacity_short_is_live_dates_only_and_supersedes_overcommit() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = tray.expected_harvest_date.clone().unwrap();

    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 5,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    let over: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1 AND resolved_at IS NULL",
            [&d],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        over, 1,
        "record_order must still raise wholesale.overcommitted"
    );

    attention::check_attention(&conn).unwrap();
    let shorts: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT entity_id, message FROM attention
                 WHERE kind = 'money.capacity_short' AND resolved_at IS NULL",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(shorts.len(), 1);
    assert_eq!(shorts[0].0, format!("{d}|dun-peas"));
    let expected_date = chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d")
        .unwrap()
        .format("%a %b %e")
        .to_string()
        .replace("  ", " ");
    assert!(
        !expected_date.contains("  "),
        "date must render clean, got {expected_date:?}"
    );
    assert!(shorts[0].1.contains(&expected_date), "{}", shorts[0].1);
    assert!(shorts[0].1.contains("2 trays"), "{}", shorts[0].1);

    let resolved_by: String = conn
        .query_row(
            "SELECT resolved_by FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1",
            [&d],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(resolved_by, "superseded_by_money_capacity_short");
    let over_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1 AND resolved_at IS NULL",
            [&d],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(over_open, 0);

    trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    assert_eq!(remaining_for(&conn, &d), 0);
    attention::check_attention(&conn).unwrap();
    let short_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(short_open, 0);
    let cleared: String = conn
        .query_row(
            "SELECT resolved_by FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1",
            [&format!("{d}|dun-peas")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cleared, "condition_cleared");
}

#[test]
fn b1_capacity_short_ignores_past_dates() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let tray_id = tray_maturing_today(&mut conn, "dun-peas", 3);
    let d = db::local_date_today();
    let tray = trays::get_tray(&conn, &tray_id).unwrap();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 3,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    wholesale::pay_order(
        &mut conn,
        &order.id,
        2400,
        &db::local_date_today(),
        None,
        true,
        false,
        None,
    )
    .unwrap();
    trays::advance_tray(&mut conn, &tray.id).unwrap();
    trays::harvest_tray(&mut conn, &tray.id, 30.0).unwrap();

    // Harvest evidence retires the claim (D2(b)); delivery-and-pay is not
    // what settles it. A completed date settles to zero instead of reading
    // negative forever. That permanent negative was the today-boundary hole;
    // the assertion that used to live here was pinning it.
    assert_eq!(
        remaining_for(&conn, &d),
        0,
        "harvest evidence stops the order reserving trays that are no longer growing"
    );

    // The `< today` filter is still load-bearing for a date that is genuinely
    // short and genuinely past — an order left OPEN on a past date.
    let past = days_from_today(-1);
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &past,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    assert!(
        remaining_for(&conn, &past) < 0,
        "an open order on a past date is still short"
    );

    attention::check_attention(&conn).unwrap();
    let plan = reachability::cover_plan(&conn).unwrap();
    assert!(
        !plan.iter().any(|c| c.harvest_date == past),
        "the < today filter still holds: a past date is never a COVER row: {plan:?}"
    );
    // D4(b): the past promise stays promised until evidence covers it, so
    // the shortfall rolls onto the live same-crop date. That is cover
    // honesty, not a today-boundary leak — and it is a CLAIM, so it is
    // pinned here instead of described. The completed date `d` itself was
    // calm before this order (remaining 0 above).
    assert_eq!(
        trays::cover_remaining_for(&conn, &d, "dun-peas").unwrap(),
        -2,
        "the past promise rolled onto the live same-crop date, not into thin air"
    );
    // Counted once. The evidence that discharged the first order cannot
    // also discharge this one — same invariant as f4a_no_tray_discharges_twice.
    assert_eq!(
        plan.iter().map(|c| c.short_trays).sum::<i64>(),
        2,
        "the undischarged past promise is counted exactly once: {plan:?}"
    );
}

#[test]
fn b1_owed_summary_math_and_unpriced_honesty() {
    let mut conn = mem();
    let v = venue(&mut conn);
    trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();

    let a = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(-4),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    let b = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(0),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(1200),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &a.id, Some(days_from_today(-4))).unwrap();
    wholesale::deliver_order(&mut conn, &b.id, None).unwrap();

    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.deliveries, 2);
    assert_eq!(owed.total_cents, Some(2800));
    assert!(!owed.any_unpriced);
    assert_eq!(owed.oldest_days, Some(4));

    let c_id = inherited_unpriced_order(&mut conn, &v.venue_id, &days_from_today(0), "dun-peas", 1);
    wholesale::deliver_order(&mut conn, &c_id, None).unwrap();

    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.deliveries, 3);
    assert_eq!(
        owed.total_cents, None,
        "a partial sum must never be presented as a total"
    );
    assert!(owed.any_unpriced);
    assert_eq!(owed.oldest_days, Some(4));
}

#[test]
fn b2_dismiss_is_per_episode_not_forever() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    attention::check_attention(&conn).unwrap();
    let open: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, message FROM attention
                 WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(open.len(), 1);
    let id = open[0].0.clone();

    attention::dismiss_attention(&mut conn, &id).unwrap();
    attention::check_attention(&conn).unwrap();
    let still: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(still, 0, "episode must hold for today");

    let yesterday = (chrono::Local::now() - chrono::Duration::days(1)).to_rfc3339();
    conn.execute(
        "UPDATE attention SET resolved_at = ?1 WHERE id = ?2",
        [yesterday, id.clone()],
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let again: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM attention
                 WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL",
            )
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(again.len(), 1);
    assert_ne!(again[0], id, "a new day must raise a new row");
}

#[test]
fn b2_dismissal_is_scoped_to_one_fact_and_one_order() {
    let mut conn = mem();
    let v = venue(&mut conn);
    trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
    let a = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(0),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    let b = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(0),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(1200),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &a.id, None).unwrap();
    wholesale::deliver_order(&mut conn, &b.id, None).unwrap();
    attention::check_attention(&conn).unwrap();
    let open: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, entity_id FROM attention
                 WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL
                 ORDER BY created_at, id",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(open.len(), 2);
    let a_id = open
        .iter()
        .find(|(_, eid)| eid == &a.id)
        .expect("A card")
        .0
        .clone();

    attention::dismiss_attention(&mut conn, &a_id).unwrap();
    attention::check_attention(&conn).unwrap();
    let after: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT entity_id FROM attention
                 WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL",
            )
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(after.len(), 1);
    assert_eq!(after[0], b.id);

    conn.execute(
        "UPDATE wholesale_orders SET delivered_on = ?1 WHERE id = ?2",
        [days_from_today(-4), a.id.clone()],
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let again: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT entity_id FROM attention
                 WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL
                 ORDER BY created_at, id",
            )
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(again.len(), 2);
    assert!(
        again.contains(&a.id),
        "changed fact must raise A again: {again:?}"
    );
    assert!(again.contains(&b.id));
}

#[test]
fn b2_legacy_overcommit_resolves_when_the_date_is_covered() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = tray.expected_harvest_date.clone().unwrap();
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 5,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    let over_id: String = conn
        .query_row(
            "SELECT id FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1 AND resolved_at IS NULL",
            [&d],
            |r| r.get(0),
        )
        .unwrap();

    trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    assert_eq!(remaining_for(&conn, &d), 0);

    attention::check_attention(&conn).unwrap();
    let over_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'wholesale.overcommitted' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(over_open, 0);
    let resolved_by: String = conn
        .query_row(
            "SELECT resolved_by FROM attention WHERE id = ?1",
            [&over_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(resolved_by, "condition_cleared");
    let short_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(short_open, 0);
}

#[test]
fn b2_legacy_oversold_is_superseded_by_the_standing_card() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = tray.expected_harvest_date.clone().unwrap();
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 5,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    attention::raise(
        &conn,
        "order.oversold",
        Some("harvest_date"),
        Some(&d),
        "oversold on this date",
        &["dismiss"],
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let resolved_by: String = conn
        .query_row(
            "SELECT resolved_by FROM attention
             WHERE kind = 'order.oversold' AND entity_id = ?1",
            [&d],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(resolved_by, "superseded_by_per_crop_card");
    let shorts: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT entity_id FROM attention
                 WHERE kind = 'money.capacity_short' AND resolved_at IS NULL",
            )
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(shorts.len(), 1);
    assert_eq!(shorts[0], format!("{d}|dun-peas"));
}

#[test]
fn b2_legacy_cards_do_not_linger_on_a_past_date() {
    let conn = mem();
    let past = days_from_today(-3);
    attention::raise(
        &conn,
        "wholesale.overcommitted",
        Some("harvest_date"),
        Some(&past),
        "past overcommit",
        &["dismiss"],
    )
    .unwrap();
    attention::raise(
        &conn,
        "order.oversold",
        Some("harvest_date"),
        Some(&past),
        "past oversold",
        &["dismiss"],
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    for kind in ["wholesale.overcommitted", "order.oversold"] {
        let open: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attention WHERE kind = ?1 AND resolved_at IS NULL",
                [kind],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open, 0, "{kind} must not linger");
        let resolved_by: String = conn
            .query_row(
                "SELECT resolved_by FROM attention WHERE kind = ?1 AND entity_id = ?2",
                [kind, past.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved_by, "date_passed", "{kind}");
    }
    let short_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(short_open, 0);
}

#[test]
fn b2_b1_autoclear_still_works() {
    let mut conn = mem();
    let v = venue(&mut conn);
    trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();

    let due = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(0),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let due_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.delivery_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&due.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(due_open, 1);
    wholesale::deliver_order(&mut conn, &due.id, None).unwrap();
    attention::check_attention(&conn).unwrap();
    let due_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.delivery_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&due.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(due_after, 0);

    let unpaid_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.delivered_unpaid' AND entity_id = ?1 AND resolved_at IS NULL",
            [&due.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unpaid_open, 1);
    wholesale::pay_order(
        &mut conn,
        &due.id,
        800,
        &db::local_date_today(),
        None,
        true,
        false,
        None,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let unpaid_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.delivered_unpaid' AND entity_id = ?1 AND resolved_at IS NULL",
            [&due.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unpaid_after, 0);

    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = tray.expected_harvest_date.clone().unwrap();
    // First sow already put 3 on this harvest date; 3 more is 6. Order past that.
    let short_order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 8,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let short_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{d}|dun-peas")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(short_open, 1);
    wholesale::void_order(&mut conn, &short_order.id, Some("test".into())).unwrap();
    attention::check_attention(&conn).unwrap();
    let short_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{d}|dun-peas")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(short_after, 0);
}

#[test]
fn b3_sow_by_is_harvest_date_minus_growth_days() {
    let conn = mem();
    assert_eq!(reachability::sow_by("2026-08-21", 9).unwrap(), "2026-08-12");
    assert_eq!(reachability::sow_by("2026-03-01", 9).unwrap(), "2026-02-20");

    let harvest8 = days_from_today(8);
    let far = reachability::for_date_for_crop(&conn, &harvest8, "red-arrow-radish").unwrap();
    assert!(far.reachable);
    assert_eq!(far.days_until_harvest, 8);
    assert_eq!(far.crop_growth_days, Some(7));
    assert_eq!(
        far.last_sow_by.as_deref(),
        Some(days_from_today(1).as_str())
    );
    assert!(!far.must_sow_today);
    let radish = reachability::for_date_for_crop(&conn, &harvest8, "red-arrow-radish").unwrap();
    assert!(radish.reachable);
    assert_eq!(
        radish.last_sow_by.as_deref(),
        Some(reachability::sow_by(&harvest8, 7).unwrap().as_str())
    );
    let mellow = reachability::for_date_for_crop(&conn, &harvest8, "mellow-mix").unwrap();
    assert!(mellow.reachable);
    assert_eq!(
        mellow.last_sow_by.as_deref(),
        Some(reachability::sow_by(&harvest8, 8).unwrap().as_str())
    );
    let spicy = reachability::for_date_for_crop(&conn, &harvest8, "spicy-mix").unwrap();
    assert!(spicy.reachable);
    assert_eq!(
        spicy.last_sow_by.as_deref(),
        Some(reachability::sow_by(&harvest8, 8).unwrap().as_str())
    );
    let broccoli = reachability::for_date_for_crop(&conn, &harvest8, "broccoli").unwrap();
    assert!(broccoli.reachable);
    assert_eq!(
        broccoli.last_sow_by.as_deref(),
        Some(reachability::sow_by(&harvest8, 8).unwrap().as_str())
    );

    let edge =
        reachability::for_date_for_crop(&conn, &days_from_today(7), "red-arrow-radish").unwrap();
    assert!(edge.reachable);
    assert!(edge.must_sow_today, "D+7 is radish-or-nothing, today");
    assert_eq!(edge.crops.len(), 1);
    assert_eq!(edge.crops[0].crop_name, "Red arrow radish");

    let dead =
        reachability::for_date_for_crop(&conn, &days_from_today(2), "red-arrow-radish").unwrap();
    assert!(!dead.reachable, "every seeded crop needs 7-9 days");
    assert!(dead.crops.is_empty());
    assert_eq!(dead.crop_growth_days, Some(7));
    assert_eq!(
        dead.last_sow_by.as_deref(),
        Some(days_from_today(-5).as_str())
    );
    assert!(!dead.must_sow_today);
}

#[test]
fn b3_unreachable_gap_says_it_cannot_be_fixed_by_sowing() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let d = days_from_today(2);
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 4,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();

    // The record-time instant signal is honest before check_attention sweeps it.
    let over: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1",
            [&d],
            |r| r.get(0),
        )
        .unwrap();
    assert!(over.contains("cannot be fixed by sowing"), "{over}");

    attention::check_attention(&conn).unwrap();
    let msg: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{d}|kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert!(msg.contains("cannot be fixed by sowing"), "{msg}");
    assert!(msg.contains("call the venue"), "{msg}");
    assert!(msg.contains("4 trays"), "{msg}");
    assert!(
        !msg.contains("Sow "),
        "unreachable must never invite a sow: {msg}"
    );
    assert!(
        !msg.contains("sow by"),
        "unreachable must not name a deadline: {msg}"
    );

    let plan = reachability::cover_plan(&conn).unwrap();
    assert_eq!(plan.len(), 1);
    assert!(!plan[0].reachability.reachable);
    assert_eq!(plan[0].short_trays, 4);
    assert_eq!(plan[0].orders.len(), 1);
    assert_eq!(plan[0].orders[0].venue_name, "Fixture Cafe");
    assert_eq!(plan[0].orders[0].state, "ordered");
}

#[test]
fn b3_reachable_gap_invites_sow_and_shows_urgency() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let urgent = days_from_today(7);
    let slack = days_from_today(20);
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &urgent,
        vec![OrderLine {
            crop_id: "red-arrow-radish".into(),
            trays: 3,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &slack,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 2,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();

    let u: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{urgent}|red-arrow-radish")],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        u.starts_with("Sow 3 trays of Red arrow radish today to cover"),
        "{u}"
    );
    assert!(u.contains("today is the last day that reaches it"), "{u}");
    assert!(!u.contains("cannot be fixed by sowing"), "{u}");

    let s: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{slack}|kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert!(s.contains("is short 2 trays"), "{s}");
    assert!(s.contains("sow by"), "{s}");
    assert!(!s.contains("cannot be fixed by sowing"), "{s}");

    let plan = reachability::cover_plan(&conn).unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(
        plan[0].harvest_date, urgent,
        "must-sow-today ranks above slack"
    );
    assert!(plan[0].reachability.must_sow_today);
    assert_eq!(plan[1].harvest_date, slack);
    assert!(!plan[1].reachability.must_sow_today);
}

#[test]
fn b3_cover_plan_ranks_unreachable_first() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let dead = days_from_today(2);
    let urgent = days_from_today(7);
    let slack = days_from_today(20);
    for (date, trays) in [(&slack, 2), (&urgent, 3), (&dead, 4)] {
        wholesale::record_order(
            &mut conn,
            &v.venue_id,
            date,
            vec![OrderLine {
                crop_id: "kale".into(),
                trays,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
    }
    let plan = reachability::cover_plan(&conn).unwrap();
    let order: Vec<String> = plan.iter().map(|c| c.harvest_date.clone()).collect();
    assert_eq!(
        order,
        vec![dead, urgent, slack],
        "unreachable, then today-or-never, then slack"
    );
}

#[test]
fn b3_sowing_cannot_clear_or_claim_to_fix_an_unreachable_date() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let d = days_from_today(2);
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 4,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let before: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{d}|kale")],
            |r| r.get(0),
        )
        .unwrap();

    // The sow the old SowSheet invited: lands on D+9, serves a different date.
    trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
    attention::check_attention(&conn).unwrap();

    let after: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{d}|kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        after, before,
        "the gap did not move, so the sentence must not move"
    );
    assert!(after.contains("cannot be fixed by sowing"), "{after}");
    assert_eq!(
        remaining_for(&conn, &d),
        -4,
        "trays sown today cannot serve D+2"
    );

    let plan = reachability::cover_plan(&conn).unwrap();
    assert_eq!(
        plan.len(),
        1,
        "the new sow date is covered, not short: {plan:?}"
    );
    assert_eq!(plan[0].harvest_date, d);
    assert!(!plan[0].reachability.reachable);
}

#[test]
fn b3_void_clears_the_unreachable_card() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let d = days_from_today(2);
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 4,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();

    wholesale::void_order(&mut conn, &order.id, Some("venue called".into())).unwrap();
    attention::check_attention(&conn).unwrap();

    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        open, 0,
        "B2 auto-resolve still holds for an unreachable date"
    );
    let resolved_by: String = conn
        .query_row(
            "SELECT resolved_by FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1",
            [&format!("{d}|kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(resolved_by, "condition_cleared");
    assert!(reachability::cover_plan(&conn).unwrap().is_empty());
}

#[test]
fn b3_redate_forward_only_flips_the_language() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let dead = days_from_today(2);
    let live = days_from_today(9);
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &dead,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 4,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();

    // Forward-only redate: void, then re-record. No amend kind exists.
    wholesale::void_order(
        &mut conn,
        &order.id,
        Some("moved to a date we can hit".into()),
    )
    .unwrap();
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &live,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 4,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();

    let dead_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{dead}|kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dead_open, 0);
    let msg: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{live}|kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!msg.contains("cannot be fixed by sowing"), "{msg}");
    assert!(msg.contains("Sow 4 trays of Kale today to cover"), "{msg}");
}

#[test]
fn b3_card_and_plan_speak_the_same_sentence() {
    let mut conn = mem();
    let v = venue(&mut conn);
    for (date, trays) in [(days_from_today(2), 4), (days_from_today(9), 3)] {
        wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &date,
            vec![OrderLine {
                crop_id: "kale".into(),
                trays,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
    }
    attention::check_attention(&conn).unwrap();
    let plan = reachability::cover_plan(&conn).unwrap();
    assert_eq!(plan.len(), 2);
    for c in &plan {
        let msg: String = conn
            .query_row(
                "SELECT message FROM attention
                 WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
                [&format!("{}|{}", c.harvest_date, c.crop_id)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(msg, c.message, "card and plan must be the same bytes");
    }
}

/// Build the eight-check array the way compute_status does, without needing a
/// farm folder on disk. H1-H4 are absent-evidence Degraded here; only M1-M4 are
/// under test.
fn money_checks(conn: &Connection) -> (crate::attention::MoneyDebts, Vec<CheckStatus>) {
    let debts = attention::money_debts(conn).unwrap();
    let inputs = CheckInputs {
        money: debts.clone(),
        ..CheckInputs::default()
    };
    let now = db::utc_now_rfc3339();
    let statuses = ["M1", "M2", "M3", "M4"]
        .iter()
        .map(|id| health::severity_for(id, None, &now, &inputs))
        .collect();
    (debts, statuses)
}

fn sev(statuses: &[CheckStatus], id: &str) -> Severity {
    statuses
        .iter()
        .find(|s| s.check_id == id)
        .unwrap_or_else(|| panic!("{id} missing"))
        .severity
}

#[test]
fn b4_health_is_never_calm_while_today_would_show_a_money_card() {
    // Four independent money states, each on its own farm.
    // (a) delivered-unpaid, one day old — the case the audit's literal M1
    //     threshold would have called Healthy.
    {
        let mut conn = mem();
        let v = venue(&mut conn);
        trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let o = wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &days_from_today(0),
            vec![OrderLine {
                crop_id: "dun-peas".into(),
                trays: 2,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
        wholesale::deliver_order(&mut conn, &o.id, None).unwrap();
        attention::check_attention(&conn).unwrap();
        assert!(open_kinds(&conn)
            .iter()
            .any(|k| k == "money.delivered_unpaid"));
        let (debts, statuses) = money_checks(&conn);
        assert!(debts.has_any());
        assert!(health::money_invariant_holds(&debts, &statuses));
        assert_ne!(sev(&statuses, "M1"), Severity::Healthy);
    }
    // (b) delivery due today.
    {
        let mut conn = mem();
        let v = venue(&mut conn);
        trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &days_from_today(0),
            vec![OrderLine {
                crop_id: "dun-peas".into(),
                trays: 1,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
        attention::check_attention(&conn).unwrap();
        assert!(open_kinds(&conn).iter().any(|k| k == "money.delivery_due"));
        let (debts, statuses) = money_checks(&conn);
        assert!(health::money_invariant_holds(&debts, &statuses));
        assert_eq!(sev(&statuses, "M3"), Severity::Degraded);
    }
    // (c) negative remaining, reachable.
    {
        let mut conn = mem();
        let v = venue(&mut conn);
        wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &days_from_today(9),
            vec![OrderLine {
                crop_id: "kale".into(),
                trays: 3,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
        attention::check_attention(&conn).unwrap();
        assert!(open_kinds(&conn)
            .iter()
            .any(|k| k == "money.capacity_short"));
        let (debts, statuses) = money_checks(&conn);
        assert!(health::money_invariant_holds(&debts, &statuses));
        assert_eq!(sev(&statuses, "M2"), Severity::Degraded);
        let m2 = statuses.iter().find(|s| s.check_id == "M2").unwrap();
        assert!(m2.sentence.contains("3 trays"), "{}", m2.sentence);
        assert!(
            m2.sentence.contains(&debts.cover[0].message),
            "M2 renders the plan's sentence; do not compose a second one in health.rs. m2={} plan={}",
            m2.sentence,
            debts.cover[0].message
        );
    }
    // (d) negative remaining, unreachable — B3 language must reach Health.
    {
        let mut conn = mem();
        let v = venue(&mut conn);
        wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &days_from_today(2),
            vec![OrderLine {
                crop_id: "kale".into(),
                trays: 4,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
        attention::check_attention(&conn).unwrap();
        let (debts, statuses) = money_checks(&conn);
        assert!(health::money_invariant_holds(&debts, &statuses));
        assert_eq!(sev(&statuses, "M2"), Severity::Unhealthy);
        let m2 = statuses.iter().find(|s| s.check_id == "M2").unwrap();
        assert!(
            m2.sentence.contains("cannot be fixed by sowing"),
            "M2 must speak B3's language: {}",
            m2.sentence
        );
        assert!(m2.sentence.contains("4 trays"), "{}", m2.sentence);
    }
}

#[test]
fn b4_m_checks_track_the_today_cards_one_for_one() {
    let mut conn = mem();
    let v = venue(&mut conn);
    // Harvest-today would also raise COVER: a delivered order still consumes
    // capacity, and nothing sown today is ready today. Collect on the sown
    // date so the only COVER is the unreachable kale date.
    let _tray_id = tray_maturing_today(&mut conn, "dun-peas", 3);
    let d = db::local_date_today();
    let paid_path = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(2),
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 4,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &paid_path.id, None).unwrap();
    attention::check_attention(&conn).unwrap();

    let (debts, statuses) = money_checks(&conn);
    assert_eq!(debts.collect.len(), 1);
    assert!(
        debts.deliver.is_empty(),
        "delivered leaves the DELIVER list"
    );
    assert_eq!(debts.cover.len(), 1);
    assert_ne!(sev(&statuses, "M1"), Severity::Healthy);
    assert_eq!(sev(&statuses, "M3"), Severity::Healthy);
    assert_ne!(sev(&statuses, "M2"), Severity::Healthy);
}

#[test]
fn b4_dismissal_quiets_today_but_never_quiets_health() {
    let mut conn = mem();
    let v = venue(&mut conn);
    trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let o = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(0),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &o.id, None).unwrap();
    attention::check_attention(&conn).unwrap();
    let card_id: String = conn
        .query_row(
            "SELECT id FROM attention
             WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();

    attention::dismiss_attention(&mut conn, &card_id).unwrap();
    attention::check_attention(&conn).unwrap();
    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        open, 0,
        "B2: the card is quiet on Today for the rest of today"
    );

    let (debts, statuses) = money_checks(&conn);
    assert_eq!(debts.collect.len(), 1, "the debt did not stop existing");
    assert_ne!(
        sev(&statuses, "M1"),
        Severity::Healthy,
        "a dismissal is an episode on Today, never a silence on Health"
    );
}

#[test]
fn b4_clearing_every_debt_returns_health_to_healthy() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = tray.expected_harvest_date.clone().unwrap();

    let due = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(0),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    let short = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 5,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let (_, statuses) = money_checks(&conn);
    assert_ne!(sev(&statuses, "M2"), Severity::Healthy);
    assert_ne!(sev(&statuses, "M3"), Severity::Healthy);

    // A harvest-today order still consumes capacity after deliver/pay, and a
    // paid order cannot be voided — so today would stay permanently short.
    // Void the due order (ordered → voided) to release today's COVER + DELIVER.
    // Collect is exercised on `d`, where trays actually exist.
    wholesale::void_order(&mut conn, &due.id, Some("moved off today".into())).unwrap();
    let collect = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(-1),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &collect.id, None).unwrap();
    wholesale::pay_order(
        &mut conn,
        &collect.id,
        800,
        &db::local_date_today(),
        None,
        true,
        false,
        None,
    )
    .unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    if remaining_for(&conn, &d) < 0 {
        wholesale::void_order(&mut conn, &short.id, Some("chef cancelled".into())).unwrap();
    }
    attention::check_attention(&conn).unwrap();

    let (debts, statuses) = money_checks(&conn);
    assert!(!debts.has_any(), "no money debt should remain: {debts:?}");
    for id in ["M1", "M2", "M3"] {
        assert_eq!(sev(&statuses, id), Severity::Healthy, "{id}");
    }
    for k in attention::MONEY_DEBT_KINDS {
        assert!(
            !open_kinds(&conn).iter().any(|open| open == k),
            "Today must be calm too: {k}"
        );
    }
}

/// A tray that matures TODAY: sow it, then move its dates back by its growth
/// days. Asserts the setup rather than trusting it.
fn tray_maturing_today(conn: &mut Connection, crop_id: &str, qty: i64) -> String {
    let t = trays::sow_tray(conn, crop_id, qty).unwrap();
    let growth = t.growth_days_at_sow.expect("growth days");
    trays::dev_backdate_tray(conn, &t.id, growth).unwrap();
    let moved = trays::get_tray(conn, &t.id).unwrap();
    assert_eq!(
        moved.expected_harvest_date.as_deref(),
        Some(db::local_date_today().as_str()),
        "fixture must produce a tray that is ready today (check backdate sign)"
    );
    t.id
}

#[test]
fn bd1_the_normal_same_day_cycle_ends_calm_without_voiding_anything() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let today = db::local_date_today();
    let tray_id = tray_maturing_today(&mut conn, "dun-peas", 3);

    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &today,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 3,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    assert_eq!(
        remaining_for(&conn, &today),
        0,
        "grown exactly to the promise"
    );
    attention::check_attention(&conn).unwrap();
    assert!(
        !open_kinds(&conn)
            .iter()
            .any(|k| k == "money.capacity_short"),
        "covered date must be calm"
    );

    // Do the work: harvest what the order is for.
    trays::advance_tray(&mut conn, &tray_id).unwrap();
    trays::harvest_tray(&mut conn, &tray_id, 30.0).unwrap();
    // D2(b): harvest evidence discharges the promise. The old window
    // ("trays off the shelf, promise still open until delivered") closed.
    assert_eq!(
        remaining_for(&conn, &today),
        0,
        "harvest evidence discharges the promise — remaining is 0, not -3"
    );

    // Deliver — the ledger records the van; capacity does not move again.
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    assert_eq!(
        remaining_for(&conn, &today),
        0,
        "delivery does not change remaining after harvest evidence"
    );
    attention::check_attention(&conn).unwrap();
    assert!(
        !open_kinds(&conn)
            .iter()
            .any(|k| k == "money.capacity_short"),
        "COVER must clear on delivery: {:?}",
        open_kinds(&conn)
    );

    // Pay — still calm, and the order was never voided.
    wholesale::pay_order(&mut conn, &order.id, 2700, &today, None, true, false, None).unwrap();
    attention::check_attention(&conn).unwrap();
    assert!(reachability::cover_plan(&conn).unwrap().is_empty());
    let state: String = conn
        .query_row(
            "SELECT state FROM wholesale_orders WHERE id = ?1",
            [&order.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "paid", "the ledger records what actually happened");
}

#[test]
fn bd2_the_open_window_says_deliver_not_call_the_venue() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let today = db::local_date_today();
    let tray_id = tray_maturing_today(&mut conn, "dun-peas", 3);
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &today,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 3,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    trays::advance_tray(&mut conn, &tray_id).unwrap();
    trays::harvest_tray(&mut conn, &tray_id, 30.0).unwrap();
    attention::check_attention(&conn).unwrap();

    // D2(b): harvest evidence discharges the promise. The old window
    // (COVER short until delivered) closed. Capacity is calm; the van
    // still waiting is M3, not a capacity short.
    assert_eq!(trays::cover_shortfall_on(&conn, &today).unwrap(), 0);
    assert!(reachability::cover_plan(&conn).unwrap().is_empty());
    let short_open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(short_open, 0, "COVER must not nag a discharged date");
    let (_, statuses) = money_checks(&conn);
    let m3 = statuses.iter().find(|s| s.check_id == "M3").unwrap();
    assert_ne!(
        m3.severity,
        Severity::Healthy,
        "the open delivery is still M3's fact, got {:?}",
        m3.severity
    );
}

#[test]
fn bd3_a_true_future_unreachable_date_still_says_call_the_venue() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let d = days_from_today(2);
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 4,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    attention::check_attention(&conn).unwrap();
    let msg: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{d}|kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert!(msg.contains("cannot be fixed by sowing"), "{msg}");
    assert!(
        msg.contains("call the venue"),
        "B3 language must not soften: {msg}"
    );
    assert!(
        msg.contains("The last day a Kale sow could reach it was"),
        "{msg}"
    );
    assert!(!msg.contains("deliver or void"), "{msg}");
}

#[test]
fn bd4_a_past_date_delivery_still_drives_m3() {
    let mut conn = mem();
    let v = venue(&mut conn);
    trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &days_from_today(-1),
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    let (_, statuses) = money_checks(&conn);
    assert_eq!(sev(&statuses, "M3"), Severity::Unhealthy);
    let m3 = statuses.iter().find(|s| s.check_id == "M3").unwrap();
    assert!(m3.sentence.contains("Fixture Cafe"), "{}", m3.sentence);
    assert!(m3.sentence.contains("1 day ago"), "{}", m3.sentence);

    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    let (_, statuses) = money_checks(&conn);
    assert_eq!(sev(&statuses, "M3"), Severity::Healthy);
    assert_ne!(sev(&statuses, "M1"), Severity::Healthy, "now it is owed");
}

#[test]
fn bd5_an_open_order_still_reserves_trays() {
    // Regression guard: the fix must not switch overcommit detection off.
    let mut conn = mem();
    let v = venue(&mut conn);
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = tray.expected_harvest_date.clone().unwrap();
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 5,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    assert_eq!(
        remaining_for(&conn, &d),
        -2,
        "an open promise still reserves"
    );
    attention::check_attention(&conn).unwrap();
    assert!(open_kinds(&conn)
        .iter()
        .any(|k| k == "money.capacity_short"));
    let plan = reachability::cover_plan(&conn).unwrap();
    assert_eq!(plan.len(), 1);
    assert!(plan[0].reachability.reachable, "D+9 is still reachable");
}

#[test]
fn bd6_the_shop_and_the_capacity_view_agree_about_a_delivered_order() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let _tray_id = tray_maturing_today(&mut conn, "dun-peas", 4);
    let d = db::local_date_today();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 4,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    let peas = offers::list_offers(&conn, &d)
        .unwrap()
        .into_iter()
        .find(|o| o.crop_id == "dun-peas")
        .unwrap();
    assert_eq!(peas.remaining, 0, "an open wholesale order blocks the shop");
    assert_eq!(remaining_for(&conn, &d), 0);

    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    let peas = offers::list_offers(&conn, &d)
        .unwrap()
        .into_iter()
        .find(|o| o.crop_id == "dun-peas")
        .unwrap();
    assert_eq!(
        peas.remaining,
        remaining_for(&conn, &d),
        "two queries, one rule about what a delivered order means"
    );
}

#[test]
fn bd7_today_with_nothing_grown_still_says_call_the_venue() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let today = db::local_date_today();
    // A chef rings at nine wanting trays this afternoon. Nothing was ever sown
    // for today. "Deliver" is not an action he has.
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &today,
        vec![OrderLine {
            crop_id: "kale".into(),
            trays: 4,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    let over: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1",
            [&today],
            |r| r.get(0),
        )
        .unwrap();
    assert!(over.contains("call the venue"), "{over}");
    assert!(!over.contains("deliver or void"), "{over}");
    attention::check_attention(&conn).unwrap();
    let msg: String = conn
        .query_row(
            "SELECT message FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&format!("{today}|kale")],
            |r| r.get(0),
        )
        .unwrap();
    assert!(msg.contains("cannot be fixed by sowing"), "{msg}");
    assert!(
        msg.contains("call the venue"),
        "nothing was grown, so there is nothing to deliver: {msg}"
    );
    assert!(!msg.contains("deliver or void"), "{msg}");
    let plan = reachability::cover_plan(&conn).unwrap();
    assert_eq!(plan[0].harvested_trays, 0);
    assert!(!reachability::settle_by_delivering(&plan[0]));
    // Health gives the same instruction, not a different one.
    let (_, statuses) = money_checks(&conn);
    let m2 = statuses.iter().find(|s| s.check_id == "M2").unwrap();
    assert_eq!(m2.severity, Severity::Unhealthy);
    assert!(
        m2.sentence.contains(&plan[0].message),
        "M2 renders the plan's sentence; do not compose a second one in health.rs. m2={} plan={}",
        m2.sentence,
        plan[0].message
    );
}

#[test]
fn bd8_the_record_time_signal_and_the_standing_card_agree_on_today() {
    // The B3 invariant, restated for the today boundary: whatever the instant
    // signal says at write time, the standing card says on the next check.
    let mut conn = mem();
    let v = venue(&mut conn);
    let today = db::local_date_today();
    let tray_id = tray_maturing_today(&mut conn, "dun-peas", 3);
    trays::advance_tray(&mut conn, &tray_id).unwrap();
    trays::harvest_tray(&mut conn, &tray_id, 30.0).unwrap();
    // Trays for today were grown and taken off the shelf. Now an order lands.
    wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &today,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 3,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    // D2(b): the trays already left the shelf, so the promise is covered.
    // Record time raises nothing; the standing card stays quiet. Same
    // instruction from both surfaces: silence.
    let over: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'wholesale.overcommitted' AND entity_id = ?1 AND resolved_at IS NULL",
            [&today],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        over, 0,
        "harvest evidence covers the order; no instant signal"
    );
    attention::check_attention(&conn).unwrap();
    let short: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'money.capacity_short' AND entity_id = ?1 AND resolved_at IS NULL",
            [&today],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(short, 0, "standing card agrees: the date is covered");
}

#[test]
fn bd9_harvested_trays_are_the_evidence_not_the_date() {
    // Same date, same negative, same open order — the only difference is
    // whether anything was actually harvested. The sentence must follow that.
    let today = db::local_date_today();
    // Harvested: delivery is possible.
    let mut harvested = mem();
    let v = venue(&mut harvested);
    let t = tray_maturing_today(&mut harvested, "dun-peas", 3);
    trays::advance_tray(&mut harvested, &t).unwrap();
    trays::harvest_tray(&mut harvested, &t, 30.0).unwrap();
    wholesale::record_order(
        &mut harvested,
        &v.venue_id,
        &today,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 3,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    // D2(b) / D5(1): harvest evidence discharges the promise. COVER is
    // empty — delivery is not what retires the reservation.
    let plan = reachability::cover_plan(&harvested).unwrap();
    assert!(
        plan.is_empty(),
        "harvested trays discharged the promise: {plan:?}"
    );
    assert_eq!(
        reachability::harvested_trays_for_date(&harvested, &today).unwrap(),
        3
    );
    // Discarded: the trays are equally gone, but nobody can be handed anything.
    let mut discarded = mem();
    let v = venue(&mut discarded);
    let t = tray_maturing_today(&mut discarded, "dun-peas", 3);
    trays::discard_tray(&mut discarded, &t).unwrap();
    wholesale::record_order(
        &mut discarded,
        &v.venue_id,
        &today,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 3,
            price_cents_per_tray: Some(900),
        }],
        false,
    )
    .unwrap();
    let plan = reachability::cover_plan(&discarded).unwrap();
    assert_eq!(
        plan[0].harvested_trays, 0,
        "a discarded tray is gone, not delivered"
    );
    assert!(!reachability::settle_by_delivering(&plan[0]));
    assert!(
        plan[0].message.contains("call the venue"),
        "{}",
        plan[0].message
    );
}

#[test]
fn rb3_pay_amount_gate_warns_and_refuses_only_unacknowledged() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let today = d.clone();
    let line = OrderLine {
        crop_id: "dun-peas".into(),
        trays: 1,
        price_cents_per_tray: Some(14400),
    };

    let matching =
        wholesale::record_order(&mut conn, &v.venue_id, &d, vec![line.clone()], false).unwrap();
    wholesale::deliver_order(&mut conn, &matching.id, None).unwrap();
    assert_eq!(
        wholesale::pay_amount_line_on(&conn, &matching.id, 14400).unwrap(),
        None
    );
    let paid_match = wholesale::pay_order(
        &mut conn,
        &matching.id,
        14400,
        &today,
        None,
        false,
        false,
        None,
    )
    .unwrap();
    assert_eq!(paid_match.state, "paid");

    let mismatch = wholesale::record_order(&mut conn, &v.venue_id, &d, vec![line], false).unwrap();
    wholesale::deliver_order(&mut conn, &mismatch.id, None).unwrap();

    let under = "This order is priced at $144.00. You are recording $12.00 — $132.00 \
         less than the order. Recording it marks the order paid in full and it \
         stops being owed.";
    assert_eq!(
        wholesale::pay_amount_line_on(&conn, &mismatch.id, 1200)
            .unwrap()
            .as_deref(),
        Some(under)
    );
    assert_eq!(
        wholesale::pay_order(
            &mut conn,
            &mismatch.id,
            1200,
            &today,
            None,
            false,
            false,
            None
        )
        .unwrap_err(),
        under
    );

    let paid_short = wholesale::pay_order(
        &mut conn,
        &mismatch.id,
        1200,
        &today,
        None,
        true,
        false,
        Some(wholesale::WriteOffInput {
            category: "sales_discount".into(),
            reason: None,
        }),
    )
    .unwrap();
    assert_eq!(paid_short.state, "paid");
    let income_id = paid_short.income_event_id.clone().expect("income_event_id");
    let for_order: Vec<_> = income::list_income(&conn)
        .unwrap()
        .into_iter()
        .filter(|r| r.income_id == income_id)
        .collect();
    assert_eq!(for_order.len(), 1);
    assert_eq!(for_order[0].amount_cents, 1200);

    let over = "This order is priced at $144.00. You are recording $500.00 — $356.00 \
         more than the order. Recording it marks the order paid in full and it \
         stops being owed.";
    assert_eq!(
        wholesale::pay_amount_line_on(&conn, &mismatch.id, 50000)
            .unwrap()
            .as_deref(),
        Some(over)
    );
}

#[test]
fn rb3_pay_amount_gate_is_silent_on_an_unpriced_order_but_settlement_is_refused() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let order_id = inherited_unpriced_order(&mut conn, &v.venue_id, &d, "dun-peas", 1);
    let order = wholesale::deliver_order(&mut conn, &order_id, None).unwrap();

    // RB3a recorded this as a named residual: with no priced total there is
    // nothing to compare an amount against, so the gate stays silent for ANY
    // amount. That half is unchanged and still asserted here — `pay_amount_line_on`
    // was not touched.
    assert!(wholesale::pay_amount_line_on(&conn, &order.id, 1)
        .unwrap()
        .is_none());
    assert!(wholesale::pay_amount_line_on(&conn, &order.id, 999_999)
        .unwrap()
        .is_none());

    // What changed: the silence no longer reaches a settlement. The unpriced
    // refusal fires before the amount gate, so the amount is never the question.
    let err = wholesale::pay_order(
        &mut conn,
        &order.id,
        1_200,
        &db::local_date_today(),
        None,
        false,
        false,
        None,
    )
    .unwrap_err();
    assert_eq!(err, wholesale::UNPRICED_SETTLEMENT_LINE);

    // And it wrote nothing.
    let still = wholesale::list_orders(&conn)
        .unwrap()
        .into_iter()
        .find(|o| o.id == order.id)
        .unwrap();
    assert_eq!(still.state, "delivered");
    assert!(income::list_income(&conn).unwrap().is_empty());
}

#[test]
fn rb3b_applying_income_settles_the_order_and_writes_no_second_income_row() {
    let mut conn = mem();
    let dir = temp_dir("rb3b-apply");
    let v = venue(&mut conn);
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let today = d.clone();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(14400),
        }],
        false,
    )
    .unwrap();
    let income = income::record_income(
        &mut conn,
        &dir,
        RecordIncomeInput {
            amount_cents: 14400,
            source: v.name.clone(),
            category_id: "produce_you_grew".into(),
            date_received: today.clone(),
            descriptor: None,
            receipt_source_path: None,
        },
        false,
    )
    .unwrap();
    let before = income::list_income(&conn).unwrap().len();

    let err =
        wholesale::settle_order_with_income(&mut conn, &order.id, &income.income_id, false, None)
            .unwrap_err();
    assert_eq!(
        err,
        "wholesale.paid only from delivered, current state ordered"
    );

    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    let paid =
        wholesale::settle_order_with_income(&mut conn, &order.id, &income.income_id, false, None)
            .unwrap();
    let rows = income::list_income(&conn).unwrap();
    assert_eq!(rows.len(), before);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].income_id, income.income_id);
    assert_eq!(paid.state, "paid");
    assert_eq!(
        paid.income_event_id.as_deref(),
        Some(income.income_id.as_str())
    );
    assert_eq!(paid.paid_on.as_deref(), Some(income.date_received.as_str()));
    assert!(attention::money_debts(&conn).unwrap().collect.is_empty());
}

#[test]
fn rb3b_one_income_row_cannot_settle_two_orders() {
    let mut conn = mem();
    let dir = temp_dir("rb3b-one");
    let v = venue(&mut conn);
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let today = d.clone();
    let line = OrderLine {
        crop_id: "dun-peas".into(),
        trays: 1,
        price_cents_per_tray: Some(8000),
    };
    let first =
        wholesale::record_order(&mut conn, &v.venue_id, &d, vec![line.clone()], false).unwrap();
    let second = wholesale::record_order(&mut conn, &v.venue_id, &d, vec![line], false).unwrap();
    wholesale::deliver_order(&mut conn, &first.id, None).unwrap();
    wholesale::deliver_order(&mut conn, &second.id, None).unwrap();
    let income = income::record_income(
        &mut conn,
        &dir,
        RecordIncomeInput {
            amount_cents: 8000,
            source: v.name.clone(),
            category_id: "produce_you_grew".into(),
            date_received: today,
            descriptor: None,
            receipt_source_path: None,
        },
        false,
    )
    .unwrap();

    wholesale::settle_order_with_income(&mut conn, &first.id, &income.income_id, false, None)
        .unwrap();
    let err =
        wholesale::settle_order_with_income(&mut conn, &second.id, &income.income_id, false, None)
            .unwrap_err();
    assert!(
        err.starts_with("that money is already applied to order "),
        "{err}"
    );
    let still = wholesale::list_orders(&conn)
        .unwrap()
        .into_iter()
        .find(|o| o.id == second.id)
        .unwrap();
    assert_eq!(still.state, "delivered");
    assert_eq!(income::list_income(&conn).unwrap().len(), 1);
}

#[test]
fn rb3b_apply_is_gated_by_the_rb3a_amount_line() {
    let mut conn = mem();
    let dir = temp_dir("rb3b-gate");
    let v = venue(&mut conn);
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let today = d.clone();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(14400),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(&mut conn, &order.id, None).unwrap();
    let income = income::record_income(
        &mut conn,
        &dir,
        RecordIncomeInput {
            amount_cents: 1200,
            source: v.name.clone(),
            category_id: "produce_you_grew".into(),
            date_received: today,
            descriptor: None,
            receipt_source_path: None,
        },
        false,
    )
    .unwrap();

    let preview = wholesale::pay_amount_line_on(&conn, &order.id, income.amount_cents)
        .unwrap()
        .unwrap();
    assert_eq!(
        wholesale::settle_order_with_income(&mut conn, &order.id, &income.income_id, false, None,)
            .unwrap_err(),
        preview
    );

    let paid = wholesale::settle_order_with_income(
        &mut conn,
        &order.id,
        &income.income_id,
        true,
        Some(wholesale::WriteOffInput {
            category: "sales_discount".into(),
            reason: None,
        }),
    )
    .unwrap();
    assert_eq!(paid.state, "paid");
    assert!(attention::money_debts(&conn).unwrap().collect.is_empty());
}

fn event_log_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap()
}

fn write_off_row_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM wholesale_write_offs", [], |r| {
        r.get(0)
    })
    .unwrap()
}

fn delivered_order_with_income(
    conn: &mut Connection,
    dir: &std::path::Path,
    price_cents: i64,
    income_cents: i64,
) -> (wholesale::WholesaleOrderView, income::IncomeView) {
    let v = venue(conn);
    let _tray = trays::sow_tray(conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let today = d.clone();
    let order = wholesale::record_order(
        conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(price_cents),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(conn, &order.id, None).unwrap();
    let income = income::record_income(
        conn,
        dir,
        RecordIncomeInput {
            amount_cents: income_cents,
            source: v.name.clone(),
            category_id: "produce_you_grew".into(),
            date_received: today,
            descriptor: None,
            receipt_source_path: None,
        },
        false,
    )
    .unwrap();
    (order, income)
}

#[test]
fn write_off_short_settle_records_allowance_and_summary() {
    let mut conn = mem();
    let dir = temp_dir("wo-short");
    let (order, income) = delivered_order_with_income(&mut conn, &dir, 4400, 900);

    let paid = wholesale::settle_order_with_income(
        &mut conn,
        &order.id,
        &income.income_id,
        true,
        Some(wholesale::WriteOffInput {
            category: "sales_discount".into(),
            reason: None,
        }),
    )
    .unwrap();
    assert_eq!(paid.state, "paid");
    assert_eq!(income::list_income(&conn).unwrap().len(), 1);
    assert_eq!(write_off_row_count(&conn), 1);
    let shortfall: i64 = conn
        .query_row(
            "SELECT shortfall_cents FROM wholesale_write_offs WHERE order_id = ?1",
            [&order.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(shortfall, 3500);

    let summary = wholesale::write_off_summary(&conn).unwrap();
    assert_eq!(summary.total_shortfall_cents, 3500);
    assert_eq!(summary.by_category.len(), 5);
    for row in &summary.by_category {
        if row.category == "sales_discount" {
            assert_eq!(row.count, 1);
            assert_eq!(row.total_cents, 3500);
        } else {
            assert_eq!(row.count, 0);
            assert_eq!(row.total_cents, 0);
        }
    }
}

#[test]
fn write_off_short_settle_without_category_is_refused_and_writes_nothing() {
    let mut conn = mem();
    let dir = temp_dir("wo-none");
    let (order, income) = delivered_order_with_income(&mut conn, &dir, 4400, 900);
    let before = event_log_count(&conn);

    let err =
        wholesale::settle_order_with_income(&mut conn, &order.id, &income.income_id, true, None)
            .unwrap_err();
    assert_eq!(
        err,
        "settling for less than the order total needs a write-off category"
    );
    let still = wholesale::list_orders(&conn)
        .unwrap()
        .into_iter()
        .find(|o| o.id == order.id)
        .unwrap();
    assert_eq!(still.state, "delivered");
    assert_eq!(event_log_count(&conn), before);
    assert_eq!(write_off_row_count(&conn), 0);
}

#[test]
fn write_off_other_without_reason_is_refused() {
    let mut conn = mem();
    let dir = temp_dir("wo-other");
    let (order, income) = delivered_order_with_income(&mut conn, &dir, 4400, 900);
    let before = event_log_count(&conn);

    let err = wholesale::settle_order_with_income(
        &mut conn,
        &order.id,
        &income.income_id,
        true,
        Some(wholesale::WriteOffInput {
            category: "other".into(),
            reason: Some("".into()),
        }),
    )
    .unwrap_err();
    assert_eq!(err, "a reason is required when the category is Other");
    let still = wholesale::list_orders(&conn)
        .unwrap()
        .into_iter()
        .find(|o| o.id == order.id)
        .unwrap();
    assert_eq!(still.state, "delivered");
    assert_eq!(event_log_count(&conn), before);
    assert_eq!(write_off_row_count(&conn), 0);
}

#[test]
fn write_off_refused_when_there_is_no_shortfall() {
    let mut conn = mem();
    let dir = temp_dir("wo-full-some");
    let (order, income) = delivered_order_with_income(&mut conn, &dir, 4400, 4400);

    let err = wholesale::settle_order_with_income(
        &mut conn,
        &order.id,
        &income.income_id,
        false,
        Some(wholesale::WriteOffInput {
            category: "sales_discount".into(),
            reason: None,
        }),
    )
    .unwrap_err();
    assert_eq!(err, "there is no shortfall to write off on this order");
    let still = wholesale::list_orders(&conn)
        .unwrap()
        .into_iter()
        .find(|o| o.id == order.id)
        .unwrap();
    assert_eq!(still.state, "delivered");
}

#[test]
fn write_off_full_amount_with_none_writes_no_row() {
    let mut conn = mem();
    let dir = temp_dir("wo-full-none");
    let (order, income) = delivered_order_with_income(&mut conn, &dir, 4400, 4400);

    let paid =
        wholesale::settle_order_with_income(&mut conn, &order.id, &income.income_id, false, None)
            .unwrap();
    assert_eq!(paid.state, "paid");
    assert_eq!(write_off_row_count(&conn), 0);
}

fn delivered_priced_order(
    conn: &mut Connection,
    price_cents: i64,
) -> wholesale::WholesaleOrderView {
    let v = venue(conn);
    let _tray = trays::sow_tray(conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let order = wholesale::record_order(
        conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(price_cents),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(conn, &order.id, None).unwrap()
}

#[test]
fn pay_order_short_without_a_category_writes_nothing() {
    let mut conn = mem();
    let order = delivered_priced_order(&mut conn, 4400);
    let today = db::local_date_today();

    let err = wholesale::pay_order(&mut conn, &order.id, 900, &today, None, true, false, None)
        .unwrap_err();
    assert_eq!(
        err,
        "settling for less than the order total needs a write-off category"
    );
    let still = wholesale::list_orders(&conn)
        .unwrap()
        .into_iter()
        .find(|o| o.id == order.id)
        .unwrap();
    assert_eq!(still.state, "delivered");
    assert!(income::list_income(&conn).unwrap().is_empty());
    assert_eq!(write_off_row_count(&conn), 0);
}

#[test]
fn pay_order_short_with_a_category_records_the_shortfall() {
    let mut conn = mem();
    let order = delivered_priced_order(&mut conn, 4400);
    let today = db::local_date_today();

    let paid = wholesale::pay_order(
        &mut conn,
        &order.id,
        900,
        &today,
        None,
        true,
        false,
        Some(wholesale::WriteOffInput {
            category: "sales_discount".into(),
            reason: None,
        }),
    )
    .unwrap();
    assert_eq!(paid.state, "paid");
    let income_rows = income::list_income(&conn).unwrap();
    assert_eq!(income_rows.len(), 1);
    assert_eq!(income_rows[0].amount_cents, 900);
    assert_eq!(write_off_row_count(&conn), 1);
    let (shortfall, category): (i64, String) = conn
        .query_row(
            "SELECT shortfall_cents, category FROM wholesale_write_offs WHERE order_id = ?1",
            [&order.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(shortfall, 3500);
    assert_eq!(category, "sales_discount");

    let summary = wholesale::write_off_summary(&conn).unwrap();
    assert_eq!(summary.total_shortfall_cents, 3500);
    assert_eq!(summary.by_category.len(), 5);
    for row in &summary.by_category {
        if row.category == "sales_discount" {
            assert_eq!(row.count, 1);
            assert_eq!(row.total_cents, 3500);
        } else {
            assert_eq!(row.count, 0);
            assert_eq!(row.total_cents, 0);
        }
    }
    assert!(attention::money_debts(&conn).unwrap().collect.is_empty());
}

#[test]
fn pay_order_other_without_a_reason_rolls_back_every_write() {
    let mut conn = mem();
    let order = delivered_priced_order(&mut conn, 4400);
    let today = db::local_date_today();

    let err = wholesale::pay_order(
        &mut conn,
        &order.id,
        900,
        &today,
        None,
        true,
        false,
        Some(wholesale::WriteOffInput {
            category: "other".into(),
            reason: None,
        }),
    )
    .unwrap_err();
    assert_eq!(err, "a reason is required when the category is Other");
    let still = wholesale::list_orders(&conn)
        .unwrap()
        .into_iter()
        .find(|o| o.id == order.id)
        .unwrap();
    assert_eq!(still.state, "delivered");
    assert!(income::list_income(&conn).unwrap().is_empty());
    assert_eq!(write_off_row_count(&conn), 0);
}

#[test]
fn pay_order_at_full_amount_is_unchanged() {
    let mut conn = mem();
    let today = db::local_date_today();

    let full = delivered_priced_order(&mut conn, 4400);
    let paid =
        wholesale::pay_order(&mut conn, &full.id, 4400, &today, None, true, false, None).unwrap();
    assert_eq!(paid.state, "paid");
    assert_eq!(write_off_row_count(&conn), 0);

    let no_shortfall = delivered_priced_order(&mut conn, 4400);
    let err = wholesale::pay_order(
        &mut conn,
        &no_shortfall.id,
        4400,
        &today,
        None,
        true,
        false,
        Some(wholesale::WriteOffInput {
            category: "sales_discount".into(),
            reason: None,
        }),
    )
    .unwrap_err();
    assert_eq!(err, "there is no shortfall to write off on this order");
}

fn delivered_unpriced_order(conn: &mut Connection) -> wholesale::WholesaleOrderView {
    let v = venue(conn);
    let _tray = trays::sow_tray(conn, "dun-peas", 3).unwrap();
    let d = db::local_date_today();
    let order_id = inherited_unpriced_order(conn, &v.venue_id, &d, "dun-peas", 1);
    wholesale::deliver_order(conn, &order_id, None).unwrap()
}

#[test]
fn an_unpriced_order_cannot_be_paid() {
    let mut conn = mem();
    let order = delivered_unpriced_order(&mut conn);
    let today = db::local_date_today();

    let err = wholesale::pay_order(&mut conn, &order.id, 1234, &today, None, true, false, None)
        .unwrap_err();
    assert_eq!(err, wholesale::UNPRICED_SETTLEMENT_LINE);

    let err_wo = wholesale::pay_order(
        &mut conn,
        &order.id,
        1234,
        &today,
        None,
        true,
        false,
        Some(wholesale::WriteOffInput {
            category: "sales_discount".into(),
            reason: None,
        }),
    )
    .unwrap_err();
    assert_eq!(err_wo, wholesale::UNPRICED_SETTLEMENT_LINE);

    let still = wholesale::list_orders(&conn)
        .unwrap()
        .into_iter()
        .find(|o| o.id == order.id)
        .unwrap();
    assert_eq!(still.state, "delivered");
    assert!(income::list_income(&conn).unwrap().is_empty());
    assert_eq!(write_off_row_count(&conn), 0);
}

#[test]
fn an_unpriced_order_cannot_be_settled_from_income() {
    let mut conn = mem();
    let dir = temp_dir("unpriced-settle");
    let order = delivered_unpriced_order(&mut conn);
    let today = db::local_date_today();
    let v_name: String = conn
        .query_row(
            "SELECT v.name FROM wholesale_orders o
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE o.id = ?1",
            [&order.id],
            |r| r.get(0),
        )
        .unwrap();
    let income = income::record_income(
        &mut conn,
        &dir,
        RecordIncomeInput {
            amount_cents: 900,
            source: v_name,
            category_id: "produce_you_grew".into(),
            date_received: today,
            descriptor: None,
            receipt_source_path: None,
        },
        false,
    )
    .unwrap();

    let err =
        wholesale::settle_order_with_income(&mut conn, &order.id, &income.income_id, true, None)
            .unwrap_err();
    assert_eq!(err, wholesale::UNPRICED_SETTLEMENT_LINE);

    let still = wholesale::list_orders(&conn)
        .unwrap()
        .into_iter()
        .find(|o| o.id == order.id)
        .unwrap();
    assert_eq!(still.state, "delivered");
    assert!(still.income_event_id.is_none());
    let applied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wholesale_orders
             WHERE income_event_id = ?1 AND state <> 'voided'",
            [&income.income_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(applied, 0);
    assert_eq!(write_off_row_count(&conn), 0);
}

#[test]
fn a_fully_priced_order_still_settles() {
    let mut conn = mem();
    let order = delivered_priced_order(&mut conn, 4400);
    let today = db::local_date_today();

    let paid =
        wholesale::pay_order(&mut conn, &order.id, 4400, &today, None, true, false, None).unwrap();
    assert_eq!(paid.state, "paid");
    assert_eq!(income::list_income(&conn).unwrap().len(), 1);
    assert_eq!(write_off_row_count(&conn), 0);
}

#[test]
fn record_order_refuses_a_line_with_no_price() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let tray = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
    let d = tray.expected_harvest_date.clone().unwrap();
    let lines = vec![OrderLine {
        crop_id: "dun-peas".into(),
        trays: 1,
        price_cents_per_tray: None,
    }];

    let err = wholesale::record_order(&mut conn, &v.venue_id, &d, lines, false).unwrap_err();
    assert_eq!(
        err,
        "every line needs a price per tray before an order can be recorded"
    );

    let orders: i64 = conn
        .query_row("SELECT COUNT(*) FROM wholesale_orders", [], |r| r.get(0))
        .unwrap();
    assert_eq!(orders, 0);
    let events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE kind = 'wholesale.ordered'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(events, 0);

    let recorded = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    assert_eq!(recorded.lines[0].price_cents_per_tray, Some(800));
}

fn short_settle_on(
    conn: &mut Connection,
    date: &str,
    category: &str,
    price_cents: i64,
    paid_cents: i64,
) -> String {
    let v = venue(conn);
    let _tray = trays::sow_tray(conn, "dun-peas", 3).unwrap();
    let harvest = db::local_date_today();
    let order = wholesale::record_order(
        conn,
        &v.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(price_cents),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(conn, &order.id, None).unwrap();
    wholesale::pay_order(
        conn,
        &order.id,
        paid_cents,
        date,
        None,
        true,
        false,
        Some(wholesale::WriteOffInput {
            category: category.into(),
            reason: None,
        }),
    )
    .unwrap();
    order.id
}

#[test]
fn booksa_write_off_range_ends_are_inclusive() {
    let mut conn = mem();
    let from = "2026-06-02";
    let to = "2026-06-10";
    let before = "2026-06-01";
    let after = "2026-06-11";
    let before_id = short_settle_on(&mut conn, before, "sales_discount", 1000, 400);
    let from_id = short_settle_on(&mut conn, from, "sales_discount", 1000, 500);
    let to_id = short_settle_on(&mut conn, to, "quality_spoilage", 1000, 600);
    let after_id = short_settle_on(&mut conn, after, "sales_discount", 1000, 700);

    let rows = wholesale::write_off_rows_between(&conn, Some(from), Some(to)).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.order_id.as_str()).collect();
    assert!(ids.contains(&from_id.as_str()));
    assert!(ids.contains(&to_id.as_str()));
    assert!(!ids.contains(&before_id.as_str()));
    assert!(!ids.contains(&after_id.as_str()));
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|r| r.written_off_on.as_str() >= from && r.written_off_on.as_str() <= to));

    let summary = wholesale::write_off_summary_between(&conn, Some(from), Some(to)).unwrap();
    assert_eq!(
        summary.total_shortfall_cents,
        rows.iter().map(|r| r.shortfall_cents).sum::<i64>()
    );
}

#[test]
fn booksa_write_off_range_still_excludes_reversed_allowances() {
    let mut conn = mem();
    let on = "2026-06-05";
    let order_id = short_settle_on(&mut conn, on, "sales_discount", 1000, 400);
    wholesale::reverse_payment(&mut conn, &order_id, "").unwrap();

    let rows = wholesale::write_off_rows_between(&conn, Some(on), Some(on)).unwrap();
    assert!(rows.is_empty());
    let summary = wholesale::write_off_summary_between(&conn, Some(on), Some(on)).unwrap();
    assert_eq!(summary.total_shortfall_cents, 0);
    for row in &summary.by_category {
        assert_eq!(row.count, 0);
        assert_eq!(row.total_cents, 0);
    }
}

#[test]
fn booksa_write_off_summary_is_the_fold_of_its_rows() {
    let mut conn = mem();
    let from = "2026-06-02";
    let to = "2026-06-10";
    short_settle_on(&mut conn, from, "sales_discount", 1000, 400);
    short_settle_on(&mut conn, to, "quality_spoilage", 1200, 500);
    short_settle_on(&mut conn, "2026-06-01", "sales_discount", 800, 200);

    let rows = wholesale::write_off_rows_between(&conn, Some(from), Some(to)).unwrap();
    let summary = wholesale::write_off_summary_between(&conn, Some(from), Some(to)).unwrap();
    assert_eq!(
        summary.total_shortfall_cents,
        rows.iter().map(|r| r.shortfall_cents).sum::<i64>()
    );
    assert_eq!(summary.by_category.len(), 5);
    for cat in &summary.by_category {
        let matching: Vec<_> = rows.iter().filter(|r| r.category == cat.category).collect();
        assert_eq!(cat.count, matching.len() as i64);
        assert_eq!(
            cat.total_cents,
            matching.iter().map(|r| r.shortfall_cents).sum::<i64>()
        );
    }
}

#[test]
fn booksa_unpriced_exposure_counts_open_orders_only() {
    let mut conn = mem();
    let v = venue(&mut conn);
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 10).unwrap();
    let d = db::local_date_today();

    let ordered_id = inherited_unpriced_order(&mut conn, &v.venue_id, &d, "dun-peas", 1);
    let delivered_id = inherited_unpriced_order(&mut conn, &v.venue_id, &d, "dun-peas", 1);
    wholesale::deliver_order(&mut conn, &delivered_id, None).unwrap();

    let _priced = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();

    let voided_id = inherited_unpriced_order(&mut conn, &v.venue_id, &d, "dun-peas", 1);
    wholesale::void_order(&mut conn, &voided_id, Some("reprice".into())).unwrap();

    let rows = wholesale::unpriced_open_orders(&conn).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(rows.len(), 2);
    assert!(ids.contains(&ordered_id.as_str()));
    assert!(ids.contains(&delivered_id.as_str()));
    assert!(!ids.contains(&voided_id.as_str()));
    let states: Vec<&str> = rows.iter().map(|r| r.state.as_str()).collect();
    assert!(states.contains(&"ordered"));
    assert!(states.contains(&"delivered"));

    let exposure = wholesale::unpriced_exposure(&conn).unwrap();
    assert_eq!(exposure.count, rows.len() as i64);
    assert_eq!(exposure.count, 2);
}

fn bad_debt_on(conn: &mut Connection, written_off_on: &str, amount_cents: i64) -> String {
    let order = delivered_priced_order(conn, amount_cents);
    let payload = BadDebtPayload {
        order_id: order.id.clone(),
        amount_cents,
        written_off_on: written_off_on.to_string(),
    };
    let event = EventRecord::originated(
        Kind::WholesaleBadDebt,
        "wholesale_order",
        order.id.clone(),
        serde_json::to_value(&payload).unwrap(),
        json!({ "op": "none" }),
        projection::handler_now(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().unwrap();
    wholesale::write_pair(&tx, &event).unwrap();
    tx.commit().unwrap();
    order.id
}

#[test]
fn booksc_bad_debt_range_ends_are_inclusive() {
    let mut conn = mem();
    let from = "2026-06-02";
    let to = "2026-06-10";
    let before = "2026-06-01";
    let after = "2026-06-11";
    let before_id = bad_debt_on(&mut conn, before, 800);
    let from_id = bad_debt_on(&mut conn, from, 900);
    let to_id = bad_debt_on(&mut conn, to, 1000);
    let after_id = bad_debt_on(&mut conn, after, 1100);

    let rows = wholesale::bad_debt_rows_between(&conn, Some(from), Some(to)).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.order_id.as_str()).collect();
    assert!(ids.contains(&from_id.as_str()));
    assert!(ids.contains(&to_id.as_str()));
    assert!(!ids.contains(&before_id.as_str()));
    assert!(!ids.contains(&after_id.as_str()));
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|r| r.written_off_on.as_str() >= from && r.written_off_on.as_str() <= to));
}

#[test]
fn booksc_bad_debts_never_include_write_off_allowances() {
    let mut conn = mem();
    let allowance_id = short_settle_on(&mut conn, "2026-06-05", "sales_discount", 1000, 400);
    let debt_order = delivered_priced_order(&mut conn, 1500);
    let confirm = wholesale::bad_debt_confirm_line(&conn, &debt_order.id)
        .unwrap()
        .expect("delivered priced order has a confirm line");
    wholesale::write_off_bad_debt(&mut conn, &debt_order.id, &confirm).unwrap();

    let debts = wholesale::bad_debt_summary_between(&conn, None, None).unwrap();
    let allowances = wholesale::write_off_summary_between(&conn, None, None).unwrap();
    assert_eq!(debts.count, 1);
    assert_eq!(debts.total_cents, 1500);
    assert_eq!(allowances.total_shortfall_cents, 600);
    assert_ne!(debts.total_cents, allowances.total_shortfall_cents);
    assert!(!wholesale::bad_debt_rows_between(&conn, None, None)
        .unwrap()
        .iter()
        .any(|r| r.order_id == allowance_id));
    assert!(!wholesale::write_off_rows_between(&conn, None, None)
        .unwrap()
        .iter()
        .any(|r| r.order_id == debt_order.id));
}

// ---- C-2 (SOP-2): pack by customer. Read-only. ----
fn c2_line(trays: i64) -> OrderLine {
    OrderLine {
        crop_id: "dun-peas".into(),
        trays,
        price_cents_per_tray: Some(800),
    }
}

#[test]
fn c2_pack_empty_farm_returns_empty() {
    let conn = mem();
    assert!(wholesale::pack_by_customer(&conn).unwrap().is_empty());
}

#[test]
fn c2_pack_two_venues_same_date_two_packs() {
    let mut conn = mem();
    let today = db::local_date_today();
    let a =
        marketing::record_venue(&mut conn, "Alpha Cafe", "cafe", None, None, None, None).unwrap();
    let b =
        marketing::record_venue(&mut conn, "Beta Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(&mut conn, &a.venue_id, &today, vec![c2_line(2)], true).unwrap();
    wholesale::record_order(&mut conn, &b.venue_id, &today, vec![c2_line(3)], true).unwrap();
    let packs = wholesale::pack_by_customer(&conn).unwrap();
    assert_eq!(packs.len(), 2, "one pack per venue, not per order");
    assert_eq!(packs[0].venue_name, "Alpha Cafe");
    assert_eq!(packs[0].harvest_date, today);
    assert_eq!(packs[0].tray_total, 2);
    assert_eq!(packs[1].venue_name, "Beta Cafe");
    assert_eq!(packs[1].tray_total, 3);
}

#[test]
fn c2_pack_one_venue_two_orders_one_pack_sums() {
    let mut conn = mem();
    let today = db::local_date_today();
    let v =
        marketing::record_venue(&mut conn, "Alpha Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(&mut conn, &v.venue_id, &today, vec![c2_line(2)], true).unwrap();
    wholesale::record_order(&mut conn, &v.venue_id, &today, vec![c2_line(3)], true).unwrap();
    let packs = wholesale::pack_by_customer(&conn).unwrap();
    assert_eq!(packs.len(), 1, "two orders for one venue is ONE pack");
    assert_eq!(packs[0].venue_name, "Alpha Cafe");
    assert_eq!(packs[0].tray_total, 5);
    assert_eq!(
        packs[0].lines.len(),
        2,
        "lines concatenated, not merged by crop"
    );
}

#[test]
fn c2_pack_delivered_today_excluded() {
    let mut conn = mem();
    let today = db::local_date_today();
    let v =
        marketing::record_venue(&mut conn, "Alpha Cafe", "cafe", None, None, None, None).unwrap();
    let keep =
        wholesale::record_order(&mut conn, &v.venue_id, &today, vec![c2_line(2)], true).unwrap();
    let gone =
        wholesale::record_order(&mut conn, &v.venue_id, &today, vec![c2_line(7)], true).unwrap();
    wholesale::deliver_order(&mut conn, &gone.id, None).unwrap();
    let packs = wholesale::pack_by_customer(&conn).unwrap();
    assert_eq!(packs.len(), 1);
    assert_eq!(packs[0].tray_total, 2, "delivered row must not be packed");
    assert_eq!(packs[0].lines.len(), 1);
    assert!(!keep.id.is_empty());
}

#[test]
fn c2_pack_other_date_excluded() {
    let mut conn = mem();
    let today = db::local_date_today();
    let v =
        marketing::record_venue(&mut conn, "Alpha Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(&mut conn, &v.venue_id, &today, vec![c2_line(2)], true).unwrap();
    wholesale::record_order(&mut conn, &v.venue_id, "2999-01-01", vec![c2_line(9)], true).unwrap();
    let packs = wholesale::pack_by_customer(&conn).unwrap();
    assert_eq!(packs.len(), 1);
    assert_eq!(packs[0].harvest_date, today);
    assert_eq!(
        packs[0].tray_total, 2,
        "an ordered row for another date is not today's pack"
    );
}

// ---- CUT-DATE (COMMAND B): the same pack, pointed at a harvest date. ----
#[test]
fn c2_pack_on_other_date_returns_that_date_with_harvest_date_echoed() {
    let mut conn = mem();
    let today = db::local_date_today();
    let v =
        marketing::record_venue(&mut conn, "Alpha Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(&mut conn, &v.venue_id, &today, vec![c2_line(2)], true).unwrap();
    wholesale::record_order(&mut conn, &v.venue_id, "2999-01-01", vec![c2_line(9)], true).unwrap();
    let packs = wholesale::pack_by_customer_on(&conn, "2999-01-01").unwrap();
    assert_eq!(packs.len(), 1);
    assert_eq!(packs[0].venue_name, "Alpha Cafe");
    assert_eq!(
        packs[0].harvest_date, "2999-01-01",
        "the pack echoes the date asked for"
    );
    assert_eq!(
        packs[0].tray_total, 9,
        "today's row is not that date's pack"
    );
}

#[test]
fn c2_pack_on_today_equals_the_today_pin() {
    let mut conn = mem();
    let today = db::local_date_today();
    let a =
        marketing::record_venue(&mut conn, "Alpha Cafe", "cafe", None, None, None, None).unwrap();
    let b =
        marketing::record_venue(&mut conn, "Beta Cafe", "cafe", None, None, None, None).unwrap();
    wholesale::record_order(&mut conn, &a.venue_id, &today, vec![c2_line(2)], true).unwrap();
    wholesale::record_order(&mut conn, &b.venue_id, &today, vec![c2_line(3)], true).unwrap();
    wholesale::record_order(&mut conn, &b.venue_id, "2999-01-01", vec![c2_line(9)], true).unwrap();
    let pinned = serde_json::to_value(wholesale::pack_by_customer(&conn).unwrap()).unwrap();
    let dated =
        serde_json::to_value(wholesale::pack_by_customer_on(&conn, &today).unwrap()).unwrap();
    assert_eq!(
        pinned, dated,
        "a null harvest date is today: one body, two doors"
    );
    assert_eq!(pinned.as_array().map(|p| p.len()), Some(2));
}

#[test]
fn c2_pack_on_bad_date_refuses_with_the_calendar_sentence() {
    let conn = mem();
    let err = wholesale::pack_by_customer_on(&conn, "tomorrow").unwrap_err();
    assert_eq!(err, "harvest_date must be YYYY-MM-DD");
    let err = wholesale::pack_by_customer_on(&conn, "2026-02-30").unwrap_err();
    assert_eq!(err, "harvest_date must be a real calendar day");
}
