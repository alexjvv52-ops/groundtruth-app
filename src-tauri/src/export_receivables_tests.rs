//! B8 receivables and income-corrections proofs.

use crate::attention;
use crate::costs::{self, CorrectExpenseInput, RecordCostInput};
use crate::db;
use crate::events::{EventRecord, Kind};
use crate::export;
use crate::export_tests::{flush, open_farm, parse_csv, parse_manifest, tempfile_dir, today};
use crate::income::{self, CorrectIncomeInput, RecordIncomeInput};
use crate::marketing;
use crate::projection;
use crate::trays;
use crate::wholesale::{self, DeliveredPayload, OrderLine};
use chrono::{Duration, Local};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn days_ago(n: i64) -> String {
    (Local::now() - Duration::days(n))
        .format("%Y-%m-%d")
        .to_string()
}

fn venue(conn: &mut Connection) -> marketing::VenueView {
    marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap()
}

fn harvest_date(conn: &mut Connection, crop_id: &str, trays_n: i64) -> String {
    trays::sow_tray(conn, crop_id, trays_n).unwrap();
    today()
}

fn delivered_unpaid(
    conn: &mut Connection,
    venue_id: &str,
    harvest: &str,
    lines: Vec<OrderLine>,
    delivered_on: &str,
) -> wholesale::WholesaleOrderView {
    let order = wholesale::record_order(conn, venue_id, harvest, lines, false).unwrap();
    wholesale::deliver_order(conn, &order.id, Some(delivered_on.to_string())).unwrap()
}

fn basic_income(amount_cents: i64, source: &str) -> RecordIncomeInput {
    RecordIncomeInput {
        amount_cents,
        source: source.into(),
        category_id: "produce_you_grew".into(),
        date_received: today(),
        descriptor: None,
        receipt_source_path: None,
    }
}

fn receivables_row<'a>(data: &'a [Vec<String>], order_id: &str) -> &'a [String] {
    data.iter()
        .find(|r| r[0] == order_id)
        .unwrap_or_else(|| panic!("missing receivables row for {order_id}"))
}

/// Replay / bundle-import stand-in — NOT a production door.
/// Mirrors wholesale.rs deliver_order exactly, minus the harvest gate.
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

#[test]
fn b8t1_delivered_unpaid_appears_with_amount_and_age() {
    let dir = tempfile_dir("b8t1");
    let mut conn = open_farm(&dir);
    let v = venue(&mut conn);
    harvest_date(&mut conn, "dun-peas", 2);
    let n = 5i64;
    let delivered_on = days_ago(n);
    let harvest = delivered_on.clone();
    let order = delivered_unpaid(
        &mut conn,
        &v.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        &delivered_on,
    );
    flush(&conn, &dir);

    let as_of = db::local_date_today();
    let debts = attention::money_debts_on(&conn, &as_of).unwrap();
    let debt = debts
        .collect
        .iter()
        .find(|d| d.order_id == order.id)
        .expect("COLLECT card must name this order");

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let csv = fs::read_to_string(bundle.join("receivables.csv")).unwrap();
    let (header, data) = parse_csv(&csv);
    assert_eq!(
        header,
        [
            "order_id",
            "venue_name",
            "delivered_on",
            "days_outstanding",
            "amount_cents",
            "amount_is_partial",
        ]
    );
    let row = receivables_row(&data, &order.id);
    assert_eq!(row[1], v.name);
    assert_eq!(row[2], delivered_on);
    assert_eq!(row[3], n.to_string());
    assert_eq!(row[3], debt.days.to_string());
    assert_eq!(row[4], "1600");
    assert_eq!(row[4], debt.cents.to_string());
    assert_eq!(row[5], "false");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn b8t2_unpriced_lines_never_invent_a_total() {
    let dir = tempfile_dir("b8t2");
    let mut conn = open_farm(&dir);
    let v = venue(&mut conn);
    let harvest_peas = harvest_date(&mut conn, "dun-peas", 1);
    let _harvest_kale = harvest_date(&mut conn, "kale", 2);
    let delivered_on = days_ago(2);

    let mixed_id = crate::wholesale_tests::inherit_wholesale_ordered(
        &mut conn,
        &v.venue_id,
        &harvest_peas,
        vec![
            OrderLine {
                crop_id: "dun-peas".into(),
                trays: 1,
                price_cents_per_tray: Some(500),
            },
            OrderLine {
                crop_id: "kale".into(),
                trays: 1,
                price_cents_per_tray: None,
            },
        ],
    );
    let mixed =
        wholesale::deliver_order(&mut conn, &mixed_id, Some(delivered_on.to_string())).unwrap();
    let unpriced_id = crate::wholesale_tests::inherited_unpriced_order(
        &mut conn,
        &v.venue_id,
        &harvest_peas,
        "kale",
        1,
    );
    let unpriced =
        wholesale::deliver_order(&mut conn, &unpriced_id, Some(delivered_on.to_string())).unwrap();
    flush(&conn, &dir);

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let csv = fs::read_to_string(bundle.join("receivables.csv")).unwrap();
    let (_header, data) = parse_csv(&csv);
    let mixed_row = receivables_row(&data, &mixed.id);
    assert_eq!(mixed_row[5], "true");
    assert_eq!(mixed_row[4], "500");

    let unpriced_row = receivables_row(&data, &unpriced.id);
    assert_eq!(unpriced_row[5], "true");
    assert_eq!(
        unpriced_row[4], "",
        "all-unpriced amount_cents must be EMPTY, not 0"
    );
    assert_ne!(unpriced_row[4], "0");

    let manifest = parse_manifest(&bundle);
    assert_eq!(manifest.counts.receivables_orders, 2);
    assert_eq!(manifest.counts.receivables_orders_partial, 2);
    assert_eq!(
        manifest.counts.receivables_total_cents, 0,
        "partial figures must not fold into the trusted total"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn b8t3_income_void_is_a_reconstructable_row() {
    let dir = tempfile_dir("b8t3");
    let mut conn = open_farm(&dir);
    let recorded =
        income::record_income(&mut conn, &dir, basic_income(2500, "Market cash"), false).unwrap();
    income::void_income(&mut conn, &recorded.income_id).unwrap();
    flush(&conn, &dir);

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let csv = fs::read_to_string(bundle.join("income-corrections.csv")).unwrap();
    let (header, data) = parse_csv(&csv);
    assert_eq!(
        header,
        [
            "correction_event_id",
            "target_income_id",
            "track",
            "action",
            "before_amount_cents",
            "after_amount_cents",
            "before_date",
            "after_date",
            "before_source",
            "after_source",
            "reason",
            "corrected_at",
        ]
    );
    assert_eq!(data.len(), 1);
    let row = &data[0];
    assert_eq!(row[1], recorded.income_id);
    assert_eq!(row[2], "income");
    assert_eq!(row[3], "voided");
    assert_eq!(row[4], "2500");
    assert_eq!(row[5], "");
    assert_eq!(row[6], today());
    assert_eq!(row[7], "");
    assert_eq!(row[8], "Market cash");
    assert_eq!(row[9], "");
    assert_eq!(row[10], "");
    assert!(!row[11].is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn b8t4_income_correction_carries_before_and_after() {
    let dir = tempfile_dir("b8t4");
    let mut conn = open_farm(&dir);
    let recorded =
        income::record_income(&mut conn, &dir, basic_income(1000, "Market cash"), false).unwrap();
    income::correct_income(
        &mut conn,
        &dir,
        CorrectIncomeInput {
            income_id: recorded.income_id.clone(),
            amount_cents: 1750,
            source: "Insurance Co".into(),
            category_id: "crop_insurance".into(),
            date_received: today(),
            descriptor: Some("hail".into()),
            receipt_source_path: None,
        },
    )
    .unwrap();
    flush(&conn, &dir);

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let csv = fs::read_to_string(bundle.join("income-corrections.csv")).unwrap();
    let (_header, data) = parse_csv(&csv);
    assert_eq!(data.len(), 1);
    let row = &data[0];
    assert_eq!(row[1], recorded.income_id);
    assert_eq!(row[2], "income");
    assert_eq!(row[3], "corrected");
    assert_eq!(row[4], "1000");
    assert_eq!(row[5], "1750");
    assert_eq!(row[6], today());
    assert_eq!(row[7], today());
    assert_eq!(row[8], "Market cash");
    assert_eq!(row[9], "Insurance Co");
    assert_eq!(row[10], "");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn b8t5_cost_correction_export_is_untouched() {
    let dir = tempfile_dir("b8t5");
    let mut conn = open_farm(&dir);
    let kept = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 500,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    costs::correct_expense(
        &mut conn,
        CorrectExpenseInput {
            target_event_id: kept.event_id.clone(),
            amount_cents: 700,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            reason: Some("typo".into()),
        },
    )
    .unwrap();
    let voided = costs::record_cost(
        &mut conn,
        &dir,
        RecordCostInput {
            amount_cents: 250,
            payee: "Other Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            receipt_source_path: None,
        },
    )
    .unwrap();
    costs::void_expense(&mut conn, &voided.event_id, Some("entered twice".into())).unwrap();
    flush(&conn, &dir);

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let csv = fs::read_to_string(bundle.join("money-corrections.csv")).unwrap();
    let (header, data) = parse_csv(&csv);
    assert_eq!(
        header,
        [
            "correction_event_id",
            "target_event_id",
            "track",
            "action",
            "before_amount_cents",
            "after_amount_cents",
            "before_date",
            "after_date",
            "before_payee",
            "after_payee",
            "reason",
            "corrected_at",
        ]
    );

    let mut stmt = conn
        .prepare(
            "SELECT correction_event_id, target_event_id, track, action,
                    before_amount_cents, after_amount_cents, before_date, after_date,
                    before_payee, after_payee, reason, corrected_at
             FROM money_corrections
             ORDER BY corrected_at, correction_event_id",
        )
        .unwrap();
    let expected: Vec<Vec<String>> = stmt
        .query_map([], |r| {
            let after_amt: Option<i64> = r.get(5)?;
            let after_date: Option<String> = r.get(7)?;
            let after_payee: Option<String> = r.get(9)?;
            let reason: Option<String> = r.get(10)?;
            Ok(vec![
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?.to_string(),
                after_amt.map(|n| n.to_string()).unwrap_or_default(),
                r.get::<_, String>(6)?,
                after_date.unwrap_or_default(),
                r.get::<_, String>(8)?,
                after_payee.unwrap_or_default(),
                reason.unwrap_or_default(),
                r.get::<_, String>(11)?,
            ])
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(data, expected);
    assert_eq!(data.len(), 2);

    let manifest = parse_manifest(&bundle);
    assert_eq!(manifest.counts.money_corrections, 2);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn b8t6_manifest_names_both_new_files_and_its_own_counts() {
    let dir = tempfile_dir("b8t6");
    let mut conn = open_farm(&dir);
    let v = venue(&mut conn);
    let harvest = harvest_date(&mut conn, "dun-peas", 1);
    delivered_unpaid(
        &mut conn,
        &v.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(900),
        }],
        &days_ago(3),
    );
    let recorded =
        income::record_income(&mut conn, &dir, basic_income(400, "Market cash"), false).unwrap();
    income::void_income(&mut conn, &recorded.income_id).unwrap();
    flush(&conn, &dir);

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let manifest = parse_manifest(&bundle);
    let listed: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
    assert!(listed.contains(&"receivables.csv"));
    assert!(listed.contains(&"income-corrections.csv"));

    let recv = fs::read_to_string(bundle.join("receivables.csv")).unwrap();
    let (_h1, recv_rows) = parse_csv(&recv);
    let inc = fs::read_to_string(bundle.join("income-corrections.csv")).unwrap();
    let (_h2, inc_rows) = parse_csv(&inc);
    assert_eq!(manifest.counts.receivables_orders, recv_rows.len() as i64);
    assert_eq!(manifest.counts.income_corrections, inc_rows.len() as i64);

    let notes = manifest.notes.join("\n");
    assert!(
        notes.contains("income-corrections.csv"),
        "notes must name income-corrections.csv"
    );
    assert!(
        !notes.contains("excluded from income.csv; the full history remains in events.jsonl"),
        "old events.jsonl-only income sentence must be gone"
    );

    // The two exclusion classes must not share one trail claim. income-corrections.csv
    // covers income.corrected / income.voided only (export.rs:1091); it carries zero
    // rows about refunded or disputed retail orders. Their record is the
    // stripe.refunded / stripe.disputed pair in events.jsonl.
    assert!(
        !manifest
            .notes
            .iter()
            .any(|n| n.contains("refunded or disputed") && n.contains("income-corrections.csv")),
        "the refunded/disputed exclusion must not name income-corrections.csv as its trail"
    );
    let retail = manifest
        .notes
        .iter()
        .find(|n| n.contains("refunded or disputed"))
        .expect("notes must name the refunded or disputed exclusion");
    assert!(
        retail.contains("events.jsonl"),
        "the retail exclusion must name its real trail"
    );
    assert!(
        !retail.contains("income-corrections.csv"),
        "the retail note must not point at a CSV that does not carry it"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn b8_uncountable_collect_row_writes_empty_days_outstanding() {
    let dir = tempfile_dir("b8-uncountable-days");
    let mut conn = open_farm(&dir);
    let v = venue(&mut conn);
    let harvest = (Local::now() + Duration::days(5))
        .format("%Y-%m-%d")
        .to_string();
    let delivered_on = today();
    let order = wholesale::record_order(
        &mut conn,
        &v.venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    deliver_without_harvest_gate(&mut conn, &order.id, delivered_on.clone());
    flush(&conn, &dir);

    let result = export::export_bundle(&conn, &dir).unwrap();
    let bundle = PathBuf::from(&result.bundle_path);
    let csv = fs::read_to_string(bundle.join("receivables.csv")).unwrap();
    let (_header, data) = parse_csv(&csv);
    let row = receivables_row(&data, &order.id);
    assert_eq!(row[1], v.name);
    assert_eq!(row[2], delivered_on);
    assert_eq!(
        row[3], "",
        "uncountable days_outstanding must be EMPTY, not 0"
    );
    assert_eq!(row[4], "1600");
    let _ = fs::remove_dir_all(&dir);
}
