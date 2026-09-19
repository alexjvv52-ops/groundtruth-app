//! Bad debt — `wholesale.bad_debt`. Engine only; no public door.

use crate::attention;
use crate::db;
use crate::event_file;
use crate::events::{self, EventRecord, Kind};
use crate::marketing;
use crate::projection;
use crate::trays;
use crate::wholesale::{self, BadDebtPayload, OrderLine};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn today() -> String {
    db::local_date_today()
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "farm-os-bad-debt-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn delivered_order(conn: &mut Connection) -> (String, String, i64) {
    let v = marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(conn, "dun-peas", 6).unwrap();
    let d = today();
    let order = wholesale::record_order(
        conn,
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
    wholesale::deliver_order(conn, &order.id, None).unwrap();
    (order.id, d, 1600)
}

fn commit_bad_debt(conn: &mut Connection, order_id: &str, amount_cents: i64) -> Result<(), String> {
    let payload = BadDebtPayload {
        order_id: order_id.to_string(),
        amount_cents,
        written_off_on: today(),
    };
    let event = EventRecord::originated(
        Kind::WholesaleBadDebt,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        projection::handler_now(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    wholesale::write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

fn order_state(conn: &Connection, order_id: &str) -> String {
    conn.query_row(
        "SELECT state FROM wholesale_orders WHERE id = ?1",
        [order_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn master_sql(conn: &Connection, name: &str) -> String {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE name = ?1",
        [name],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .unwrap_or_default()
}

fn object_names(conn: &Connection, typ: &str, tbl_name: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = ?1 AND tbl_name = ?2
             ORDER BY name",
        )
        .unwrap();
    stmt.query_map([typ, tbl_name], |r| r.get(0))
        .unwrap()
        .map(|x| x.unwrap())
        .collect()
}

#[test]
fn bad_debt_from_delivered_leaves_collect() {
    let mut conn = mem();
    let (order_id, _, amount) = delivered_order(&mut conn);
    commit_bad_debt(&mut conn, &order_id, amount).unwrap();

    let debts = attention::money_debts_on(&conn, &today()).unwrap();
    assert!(
        debts.collect.is_empty(),
        "collect must be empty after written_off: {:?}",
        debts.collect
    );
    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.deliveries, 0);
    assert_eq!(order_state(&conn, &order_id), "written_off");
}

#[test]
fn bad_debt_does_not_release_capacity() {
    let mut conn = mem();
    let (order_id, harvest_date, amount) = delivered_order(&mut conn);
    let remaining_before = trays::remaining_for_date(&conn, &harvest_date).unwrap();
    let shortfall_before = trays::cover_shortfall_on(&conn, &harvest_date).unwrap();

    commit_bad_debt(&mut conn, &order_id, amount).unwrap();

    let remaining_after = trays::remaining_for_date(&conn, &harvest_date).unwrap();
    let shortfall_after = trays::cover_shortfall_on(&conn, &harvest_date).unwrap();
    assert_eq!(
        remaining_before, remaining_after,
        "remaining_for_date moved: before={remaining_before} after={remaining_after}"
    );
    assert_eq!(
        shortfall_before, shortfall_after,
        "cover_shortfall_on moved: before={shortfall_before} after={shortfall_after}"
    );
}

#[test]
fn bad_debt_refused_from_every_other_state() {
    let mut conn = mem();
    let v =
        marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
    let d = today();

    let ordered = wholesale::record_order(
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
    let err = commit_bad_debt(&mut conn, &ordered.id, 800).unwrap_err();
    assert_eq!(
        err,
        "wholesale.bad_debt only from delivered, current state ordered"
    );

    let paid = wholesale::record_order(
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
    wholesale::deliver_order(&mut conn, &paid.id, None).unwrap();
    wholesale::pay_order(&mut conn, &paid.id, 800, &d, None, true, false, None).unwrap();
    let err = commit_bad_debt(&mut conn, &paid.id, 800).unwrap_err();
    assert_eq!(
        err,
        "wholesale.bad_debt only from delivered, current state paid"
    );

    let voided = wholesale::record_order(
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
    wholesale::void_order(&mut conn, &voided.id, Some("changed mind".into())).unwrap();
    let err = commit_bad_debt(&mut conn, &voided.id, 800).unwrap_err();
    assert_eq!(
        err,
        "wholesale.bad_debt only from delivered, current state voided"
    );

    let written = wholesale::record_order(
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
    wholesale::deliver_order(&mut conn, &written.id, None).unwrap();
    commit_bad_debt(&mut conn, &written.id, 800).unwrap();
    let err = commit_bad_debt(&mut conn, &written.id, 800).unwrap_err();
    assert_eq!(
        err,
        "wholesale.bad_debt only from delivered, current state written_off"
    );
}

#[test]
fn bad_debt_writes_no_income_row() {
    let mut conn = mem();
    let (order_id, _, amount) = delivered_order(&mut conn);
    commit_bad_debt(&mut conn, &order_id, amount).unwrap();

    let live: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM income_events WHERE voided_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(live, 0);

    let (paid_on, income_event_id): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT paid_on, income_event_id FROM wholesale_orders WHERE id = ?1",
            [&order_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(paid_on.is_none());
    assert!(income_event_id.is_none());
}

#[test]
fn bad_debt_replays_byte_identically() {
    let dir = tempfile_dir("replay");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    let (order_id, _, amount) = delivered_order(&mut conn);
    commit_bad_debt(&mut conn, &order_id, amount).unwrap();
    event_file::try_flush_after_commit(&conn, &dir);
    drop(conn);
    let outcome = projection::verify_replay_paths(&farm, &dir.join("events.jsonl"), &dir).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify-replay failed: {}",
        outcome.summary_line()
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bad_debt_is_not_undoable() {
    let mut conn = mem();
    let (order_id, _, amount) = delivered_order(&mut conn);
    commit_bad_debt(&mut conn, &order_id, amount).unwrap();
    let newest = events::newest_undoable(&conn).unwrap();
    if let Some(e) = &newest {
        assert_ne!(e.kind, "wholesale.bad_debt");
    }
}

#[test]
fn bad_debt_row_is_append_only() {
    let mut conn = mem();
    let (order_id, _, amount) = delivered_order(&mut conn);
    commit_bad_debt(&mut conn, &order_id, amount).unwrap();

    let update_err = conn
        .execute(
            "UPDATE wholesale_bad_debts SET amount_cents = 2 WHERE order_id = ?1",
            [&order_id],
        )
        .unwrap_err()
        .to_string();
    assert!(
        update_err.contains("wholesale_bad_debts is append-only"),
        "{update_err}"
    );

    let delete_err = conn
        .execute(
            "DELETE FROM wholesale_bad_debts WHERE order_id = ?1",
            [&order_id],
        )
        .unwrap_err()
        .to_string();
    assert!(
        delete_err.contains("wholesale_bad_debts is append-only"),
        "{delete_err}"
    );
}

#[test]
fn fresh_and_migrated_wholesale_orders_schema_match() {
    let fresh = db::open_in_memory().unwrap();
    let migrated = db::open_v1_in_memory().unwrap();
    db::migrate(&migrated).unwrap();

    let fresh_sql = master_sql(&fresh, "wholesale_orders");
    let migrated_sql = master_sql(&migrated, "wholesale_orders");
    assert_eq!(
        fresh_sql, migrated_sql,
        "wholesale_orders sqlite_master sql diverged\nmigrated:\n{migrated_sql}\nfresh:\n{fresh_sql}"
    );
    assert!(
        fresh_sql.contains("written_off"),
        "converged schema must include written_off:\n{fresh_sql}"
    );

    assert_eq!(
        object_names(&fresh, "index", "wholesale_orders"),
        object_names(&migrated, "index", "wholesale_orders")
    );
    assert_eq!(
        object_names(&fresh, "trigger", "wholesale_orders"),
        object_names(&migrated, "trigger", "wholesale_orders")
    );
}

#[test]
fn written_off_order_stays_visible_in_the_list() {
    let mut conn = mem();
    let (order_id, _, amount) = delivered_order(&mut conn);
    commit_bad_debt(&mut conn, &order_id, amount).unwrap();
    let listed = wholesale::list_orders(&conn).unwrap();
    let found = listed.iter().find(|o| o.id == order_id);
    assert!(
        found.is_some(),
        "written_off order vanished from list_orders"
    );
    assert_eq!(found.unwrap().state, "written_off");
}

fn kind_count(conn: &Connection, kind: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM event_log WHERE kind = ?1",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

fn bad_debt_rows(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM wholesale_bad_debts", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn bad_debt_confirm_line_is_the_signed_sentence() {
    let mut conn = mem();
    let (order_id, _, _) = delivered_order(&mut conn);
    crate::currency::set_farm_currency(&conn, "zar").unwrap();
    let line = wholesale::bad_debt_confirm_line(&conn, &order_id)
        .unwrap()
        .expect("delivered priced order must offer a confirm line");
    assert_eq!(
        line,
        "Write off ZAR 16.00 from Fixture Cafe as bad debt? The delivery stands \
         and the trays stay committed. No payment is recorded. This cannot be \
         undone."
    );
}

#[test]
fn bad_debt_confirm_line_is_none_when_not_eligible() {
    let mut conn = mem();
    let v =
        marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let d = today();
    let line = |conn: &Connection, id: &str| wholesale::bad_debt_confirm_line(conn, id).unwrap();

    let ordered = wholesale::record_order(
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
    assert_eq!(line(&conn, &ordered.id), None);

    let paid = wholesale::record_order(
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
    wholesale::deliver_order(&mut conn, &paid.id, None).unwrap();
    wholesale::pay_order(&mut conn, &paid.id, 800, &d, None, true, false, None).unwrap();
    assert_eq!(line(&conn, &paid.id), None);

    let voided = wholesale::record_order(
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
    wholesale::void_order(&mut conn, &voided.id, Some("changed mind".into())).unwrap();
    assert_eq!(line(&conn, &voided.id), None);

    let written = wholesale::record_order(
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
    wholesale::deliver_order(&mut conn, &written.id, None).unwrap();
    let confirm = wholesale::bad_debt_confirm_line(&conn, &written.id)
        .unwrap()
        .unwrap();
    wholesale::write_off_bad_debt(&mut conn, &written.id, &confirm).unwrap();
    assert_eq!(line(&conn, &written.id), None);

    let unpriced_id =
        crate::wholesale_tests::inherited_unpriced_order(&mut conn, &v.venue_id, &d, "dun-peas", 1);
    wholesale::deliver_order(&mut conn, &unpriced_id, None).unwrap();
    assert_eq!(line(&conn, &unpriced_id), None);
}

#[test]
fn write_off_door_from_delivered_leaves_collect() {
    let mut conn = mem();
    let (order_id, _, amount) = delivered_order(&mut conn);
    let line = wholesale::bad_debt_confirm_line(&conn, &order_id)
        .unwrap()
        .unwrap();
    let view = wholesale::write_off_bad_debt(&mut conn, &order_id, &line).unwrap();
    assert_eq!(view.state, "written_off");

    let debts = attention::money_debts_on(&conn, &today()).unwrap();
    assert!(
        debts.collect.is_empty(),
        "collect must be empty after written_off: {:?}",
        debts.collect
    );
    let owed = wholesale::owed_summary(&conn).unwrap();
    assert_eq!(owed.deliveries, 0);
    assert_eq!(bad_debt_rows(&conn), 1);
    let row_amount: i64 = conn
        .query_row(
            "SELECT amount_cents FROM wholesale_bad_debts WHERE order_id = ?1",
            [&order_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(row_amount, amount);
}

#[test]
fn write_off_door_refuses_wrong_confirm_text() {
    let mut conn = mem();
    let (order_id, _, _) = delivered_order(&mut conn);
    let line = wholesale::bad_debt_confirm_line(&conn, &order_id)
        .unwrap()
        .unwrap();
    let mutated = line.replacen('W', "w", 1);
    assert_ne!(mutated, line);
    let err = wholesale::write_off_bad_debt(&mut conn, &order_id, &mutated).unwrap_err();
    assert_eq!(
        err,
        "wholesale.bad_debt: confirmation text did not match this order"
    );
    assert_eq!(order_state(&conn, &order_id), "delivered");
    assert_eq!(bad_debt_rows(&conn), 0);
    assert_eq!(kind_count(&conn, "wholesale.bad_debt"), 0);
}

#[test]
fn write_off_door_refuses_unpriced_with_the_signed_line() {
    let mut conn = mem();
    let v =
        marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
    let d = today();
    let unpriced_id =
        crate::wholesale_tests::inherited_unpriced_order(&mut conn, &v.venue_id, &d, "dun-peas", 1);
    wholesale::deliver_order(&mut conn, &unpriced_id, None).unwrap();
    let err = wholesale::write_off_bad_debt(&mut conn, &unpriced_id, "unused").unwrap_err();
    assert_eq!(err, crate::wholesale::UNPRICED_SETTLEMENT_LINE);
    assert_eq!(order_state(&conn, &unpriced_id), "delivered");
    assert_eq!(bad_debt_rows(&conn), 0);
    assert_eq!(kind_count(&conn, "wholesale.bad_debt"), 0);
}

#[test]
fn write_off_door_refuses_from_every_other_state() {
    let mut conn = mem();
    let v =
        marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let d = today();

    let ordered = wholesale::record_order(
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
    let err = wholesale::write_off_bad_debt(&mut conn, &ordered.id, "unused").unwrap_err();
    assert_eq!(
        err,
        "wholesale.bad_debt only from delivered, current state ordered"
    );
    assert_eq!(kind_count(&conn, "wholesale.bad_debt"), 0);

    let paid = wholesale::record_order(
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
    wholesale::deliver_order(&mut conn, &paid.id, None).unwrap();
    wholesale::pay_order(&mut conn, &paid.id, 800, &d, None, true, false, None).unwrap();
    let err = wholesale::write_off_bad_debt(&mut conn, &paid.id, "unused").unwrap_err();
    assert_eq!(
        err,
        "wholesale.bad_debt only from delivered, current state paid"
    );

    let voided = wholesale::record_order(
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
    wholesale::void_order(&mut conn, &voided.id, Some("changed mind".into())).unwrap();
    let err = wholesale::write_off_bad_debt(&mut conn, &voided.id, "unused").unwrap_err();
    assert_eq!(
        err,
        "wholesale.bad_debt only from delivered, current state voided"
    );

    let written = wholesale::record_order(
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
    wholesale::deliver_order(&mut conn, &written.id, None).unwrap();
    let confirm = wholesale::bad_debt_confirm_line(&conn, &written.id)
        .unwrap()
        .unwrap();
    wholesale::write_off_bad_debt(&mut conn, &written.id, &confirm).unwrap();
    let err = wholesale::write_off_bad_debt(&mut conn, &written.id, &confirm).unwrap_err();
    assert_eq!(
        err,
        "wholesale.bad_debt only from delivered, current state written_off"
    );
    assert_eq!(kind_count(&conn, "wholesale.bad_debt"), 1);
    assert_eq!(bad_debt_rows(&conn), 1);
}

#[test]
fn bad_debt_payload_field_names_pinned() {
    assert_eq!(
        crate::wholesale::BAD_DEBT_PAYLOAD_FIELD_NAMES,
        &["order_id", "amount_cents", "written_off_on"]
    );
}

#[test]
fn bad_debt_trail_line_is_the_signed_sentence() {
    let mut conn = mem();
    let (order_id, _, _) = delivered_order(&mut conn);
    let confirm = wholesale::bad_debt_confirm_line(&conn, &order_id)
        .unwrap()
        .expect("delivered priced order must offer a confirm line");
    wholesale::write_off_bad_debt(&mut conn, &order_id, &confirm).unwrap();
    let line = wholesale::bad_debt_trail_line(&conn, &order_id)
        .unwrap()
        .expect("written_off order must have a trail line");
    assert_eq!(
        line,
        format!(
            "{} written off as bad debt on {}. The trays stay committed and no payment was recorded.",
            crate::currency::code_amount("usd", 1600),
            crate::reachability::format_mon_d_local(&db::local_date_today()).unwrap()
        )
    );
}

#[test]
fn bad_debt_trail_line_is_none_for_every_other_state() {
    let mut conn = mem();
    let v =
        marketing::record_venue(&mut conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap();
    let _tray = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
    let d = today();
    let line = |conn: &Connection, id: &str| wholesale::bad_debt_trail_line(conn, id).unwrap();

    let ordered = wholesale::record_order(
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
    assert_eq!(line(&conn, &ordered.id), None);

    let delivered = wholesale::record_order(
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
    wholesale::deliver_order(&mut conn, &delivered.id, None).unwrap();
    assert_eq!(line(&conn, &delivered.id), None);

    let paid = wholesale::record_order(
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
    wholesale::deliver_order(&mut conn, &paid.id, None).unwrap();
    wholesale::pay_order(&mut conn, &paid.id, 800, &d, None, true, false, None).unwrap();
    assert_eq!(line(&conn, &paid.id), None);

    let voided = wholesale::record_order(
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
    wholesale::void_order(&mut conn, &voided.id, Some("changed mind".into())).unwrap();
    assert_eq!(line(&conn, &voided.id), None);
}

#[test]
fn bad_debt_trail_line_is_read_only() {
    let mut conn = mem();
    let (order_id, harvest_date, _) = delivered_order(&mut conn);
    let confirm = wholesale::bad_debt_confirm_line(&conn, &order_id)
        .unwrap()
        .unwrap();
    wholesale::write_off_bad_debt(&mut conn, &order_id, &confirm).unwrap();

    let collect_before = attention::money_debts_on(&conn, &today())
        .unwrap()
        .collect
        .len();
    let deliveries_before = wholesale::owed_summary(&conn).unwrap().deliveries;
    let remaining_before = trays::remaining_for_date(&conn, &harvest_date).unwrap();
    let shortfall_before = trays::cover_shortfall_on(&conn, &harvest_date).unwrap();

    let _ = wholesale::bad_debt_trail_line(&conn, &order_id)
        .unwrap()
        .expect("written_off order must have a trail line");

    let collect_after = attention::money_debts_on(&conn, &today())
        .unwrap()
        .collect
        .len();
    let deliveries_after = wholesale::owed_summary(&conn).unwrap().deliveries;
    let remaining_after = trays::remaining_for_date(&conn, &harvest_date).unwrap();
    let shortfall_after = trays::cover_shortfall_on(&conn, &harvest_date).unwrap();

    assert_eq!(
        collect_before, collect_after,
        "collect moved: before={collect_before} after={collect_after}"
    );
    assert_eq!(
        deliveries_before, deliveries_after,
        "owed deliveries moved: before={deliveries_before} after={deliveries_after}"
    );
    assert_eq!(
        remaining_before, remaining_after,
        "remaining_for_date moved: before={remaining_before} after={remaining_after}"
    );
    assert_eq!(
        shortfall_before, shortfall_after,
        "cover_shortfall_on moved: before={shortfall_before} after={shortfall_after}"
    );
}
