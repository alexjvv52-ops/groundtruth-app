//! Phase 7 — reputation generates work (GT-D15).

use crate::capacity_gate::CapacityState;
use crate::db;
use crate::event_partition::{marketing_kinds, EventDomain, Kind, MARKETING_KINDS};
use crate::events::{self, EventRecord};
use crate::marketing::{self, ReviewRequestPayload, REVIEW_REQUESTED_PAYLOAD_FIELD_NAMES};
use crate::projection::{self, EXCLUSION_LIST};
use crate::trays;
use crate::wholesale::{self, OrderLine};
use rusqlite::Connection;
use serde_json::json;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn standing_venue(conn: &mut Connection, name: &str) -> marketing::VenueView {
    venue_at(conn, name, "standing")
}

fn venue_at(conn: &mut Connection, name: &str, stage: &str) -> marketing::VenueView {
    let venue = marketing::record_venue(conn, name, "cafe", None, None, None, None).unwrap();
    let trays_week = if stage == "standing" { Some(2) } else { None };
    let varieties = if stage == "standing" {
        Some(vec!["peas".into()])
    } else {
        None
    };
    marketing::change_stage(
        conn,
        &venue.venue_id,
        stage,
        &db::local_date_today(),
        trays_week,
        varieties,
        None,
        None,
    )
    .unwrap();
    venue
}

fn deliver_to(conn: &mut Connection, venue_id: &str) {
    let _tray = trays::sow_tray(conn, "dun-peas", 1).unwrap();
    let harvest = db::local_date_today();
    let order = wholesale::record_order(
        conn,
        venue_id,
        &harvest,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(conn, &order.id, None).unwrap();
}

fn days_ago(n: i64) -> String {
    let today = chrono::NaiveDate::parse_from_str(&db::local_date_today(), "%Y-%m-%d").unwrap();
    (today - chrono::Duration::days(n))
        .format("%Y-%m-%d")
        .to_string()
}
fn actions_of(conn: &Connection, kind: &str) -> Vec<marketing::WeeklyAction> {
    marketing::weekly_actions_with_capacity(conn, &CapacityState::Unknown("x".into()))
        .unwrap()
        .actions
        .into_iter()
        .filter(|a| a.kind == kind)
        .collect()
}
fn review_asks(conn: &Connection) -> Vec<marketing::WeeklyAction> {
    marketing::weekly_actions_with_capacity(conn, &CapacityState::Unknown("x".into()))
        .unwrap()
        .actions
        .into_iter()
        .filter(|a| a.kind == "review_ask")
        .collect()
}

#[test]
fn p7t1_kind_admitted_and_lists_agree() {
    // Bumped for the silent-shortfall residual (wholesale.write_off).
    assert_eq!(Kind::ALL.len(), 54);
    let k = Kind::parse("review.requested").unwrap();
    assert_eq!(k, Kind::ReviewRequested);
    assert_eq!(k.as_str(), "review.requested");
    assert_eq!(k.tier(), (EventDomain::Marketing, None));
    assert!(MARKETING_KINDS.contains(&"review.requested"));
    assert_eq!(marketing_kinds(), MARKETING_KINDS.to_vec());
}

#[test]
fn p7t2_schema_v24_tables_and_once_per_venue_pk() {
    let conn = mem();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, db::SCHEMA_VERSION);
    let pk: String = conn
        .query_row(
            "SELECT name FROM pragma_table_info('mkt_review_requests') WHERE pk = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pk, "venue_id");
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'business_profile'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn p7t3_money_keys_refused_on_review_requested() {
    let mut conn = mem();
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();
    let mut base = json!({
        "venueId": "v1",
        "decidedOn": "2026-08-14",
        "outcome": "asked",
        "amount": 1,
    });
    let event = EventRecord::originated(
        Kind::ReviewRequested,
        "venue",
        "v1".to_string(),
        base.clone(),
        json!({ "op": "none" }),
        "2026-08-14T00:00:00.000Z".to_string(),
        None,
        None,
        Some("ev-money".to_string()),
    );
    let tx = conn.transaction().unwrap();
    let err = events::write_event(&tx, &event).unwrap_err();
    assert!(err.contains("amount"), "{err}");
    drop(tx);
    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
    base.as_object_mut().unwrap().remove("amount");
    let _ = base;
}

#[test]
fn p7t4_outcome_outside_asked_skipped_is_refused() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let err = marketing::record_review_request(&mut conn, &venue.venue_id, "maybe").unwrap_err();
    assert!(
        err.contains("asked") || err.contains("skipped") || err.contains("outcome"),
        "{err}"
    );
}

#[test]
fn p7t5_second_ask_is_an_error_not_an_overwrite() {
    let mut conn = mem();
    let venue = standing_venue(&mut conn, "Cafe");
    marketing::record_review_request(&mut conn, &venue.venue_id, "asked").unwrap();
    let err = marketing::record_review_request(&mut conn, &venue.venue_id, "skipped").unwrap_err();
    assert!(err.contains("already decided"), "{err}");
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mkt_review_requests WHERE venue_id = ?1",
            [&venue.venue_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
    let outcome: String = conn
        .query_row(
            "SELECT outcome FROM mkt_review_requests WHERE venue_id = ?1",
            [&venue.venue_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(outcome, "asked");
}

#[test]
fn p7t6_skip_records_and_payload_round_trips() {
    let mut conn = mem();
    let venue = standing_venue(&mut conn, "Cafe");
    let view = marketing::record_review_request(&mut conn, &venue.venue_id, "skipped").unwrap();
    assert_eq!(view.outcome, "skipped");
    let raw: String = conn
        .query_row(
            "SELECT payload FROM event_log WHERE kind = 'review.requested'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let payload: ReviewRequestPayload = serde_json::from_str(&raw).unwrap();
    assert_eq!(payload.venue_id, venue.venue_id);
    assert_eq!(payload.outcome, "skipped");
    assert_eq!(payload.decided_on, db::local_date_today());
    assert_eq!(
        REVIEW_REQUESTED_PAYLOAD_FIELD_NAMES,
        &["venue_id", "decided_on", "outcome"]
    );
}

#[test]
fn p7t7_unknown_venue_is_refused() {
    let mut conn = mem();
    let err = marketing::record_review_request(&mut conn, "missing", "asked").unwrap_err();
    assert!(err.contains("venue not found"), "{err}");
}

#[test]
fn p7t8_apply_is_idempotent_on_replay() {
    let mut conn = mem();
    let venue = standing_venue(&mut conn, "Cafe");
    marketing::record_review_request(&mut conn, &venue.venue_id, "asked").unwrap();
    let payload: String = conn
        .query_row(
            "SELECT payload FROM event_log WHERE kind = 'review.requested'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let created_at: String = conn
        .query_row(
            "SELECT created_at FROM event_log WHERE kind = 'review.requested'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let event = EventRecord::originated(
        Kind::ReviewRequested,
        "venue",
        venue.venue_id.clone(),
        serde_json::from_str(&payload).unwrap(),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some("replay-again".into()),
    );
    let tx = conn.transaction().unwrap();
    marketing::apply_review_requested(&tx, &event).unwrap();
    tx.commit().unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM mkt_review_requests", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn p7t9_gbp_verified_on_is_a_date_not_a_bool() {
    let conn = mem();
    assert_eq!(marketing::gbp_verified_on(&conn).unwrap(), None);
    let set = marketing::set_gbp_verified(&conn, Some("2026-08-14")).unwrap();
    assert_eq!(set.as_deref(), Some("2026-08-14"));
    assert_eq!(
        marketing::gbp_verified_on(&conn).unwrap().as_deref(),
        Some("2026-08-14")
    );
    let err = marketing::set_gbp_verified(&conn, Some("2026-13-40")).unwrap_err();
    assert!(
        err.contains("YYYY-MM-DD") || err.contains("calendar"),
        "{err}"
    );
}

#[test]
fn p7t10_gbp_unverified_does_not_suppress_ask() {
    let mut conn = mem();
    let venue = standing_venue(&mut conn, "Cafe");
    deliver_to(&mut conn, &venue.venue_id);
    assert_eq!(marketing::gbp_verified_on(&conn).unwrap(), None);
    let asks = review_asks(&conn);
    assert_eq!(
        asks.len(),
        1,
        "GBP verification is reference data, never a gate. \
         Its absence costs its own amber sentence and nothing more. {:?}",
        asks
    );
    assert_eq!(asks[0].venue_id.as_deref(), Some(venue.venue_id.as_str()));
    assert_eq!(asks[0].title, "Ask Cafe for a review");
}

#[test]
fn p7t11_standing_delivered_raises_one_ask() {
    let mut conn = mem();
    let venue = standing_venue(&mut conn, "Cafe");
    deliver_to(&mut conn, &venue.venue_id);
    let asks = review_asks(&conn);
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert_eq!(asks[0].venue_id.as_deref(), Some(venue.venue_id.as_str()));
    assert_eq!(asks[0].title, "Ask Cafe for a review");
}

#[test]
fn p7t12_ask_or_skip_retires_the_work() {
    let mut conn = mem();
    let asked = standing_venue(&mut conn, "Asked Cafe");
    deliver_to(&mut conn, &asked.venue_id);
    marketing::record_review_request(&mut conn, &asked.venue_id, "asked").unwrap();
    let skipped = standing_venue(&mut conn, "Skipped Cafe");
    deliver_to(&mut conn, &skipped.venue_id);
    marketing::record_review_request(&mut conn, &skipped.venue_id, "skipped").unwrap();
    let asks = review_asks(&conn);
    assert!(asks.is_empty(), "once per venue, permanently: {:?}", asks);
}

#[test]
fn p7t13_mkt_review_requests_is_compared_not_excluded() {
    assert!(
        !EXCLUSION_LIST
            .iter()
            .any(|l| l.contains("mkt_review_requests")),
        "projected table must be compared: {EXCLUSION_LIST:?}"
    );
}

#[test]
fn p7t14_business_profile_is_excluded_like_shelf_capacity() {
    let line = EXCLUSION_LIST
        .iter()
        .find(|l| l.contains("business_profile"))
        .expect("business_profile must be on EXCLUSION_LIST");
    assert!(line.contains("neither copied nor compared"), "{line}");
    assert!(line.contains("No apply_* writes it"), "{line}");
}

#[test]
fn p7t15_projection_dispatch_round_trips() {
    let mut conn = mem();
    let venue = standing_venue(&mut conn, "Cafe");
    marketing::record_review_request(&mut conn, &venue.venue_id, "asked").unwrap();
    let tx = conn.transaction().unwrap();
    let payload: serde_json::Value = tx
        .query_row(
            "SELECT payload FROM event_log WHERE kind = 'review.requested'",
            [],
            |r| r.get::<_, String>(0),
        )
        .map(|s| serde_json::from_str(&s).unwrap())
        .unwrap();
    let created_at: String = tx
        .query_row(
            "SELECT created_at FROM event_log WHERE kind = 'review.requested'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let event = EventRecord::originated(
        Kind::ReviewRequested,
        "venue",
        venue.venue_id.clone(),
        payload,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some("dispatch".into()),
    );
    projection::apply_event(&tx, &event).unwrap();
    drop(tx);
}

#[test]
fn p7t16_trial_delivered_raises_one_ask() {
    let mut conn = mem();
    let venue = venue_at(&mut conn, "Trial Cafe", "trial");
    deliver_to(&mut conn, &venue.venue_id);
    let asks = review_asks(&conn);
    assert_eq!(
        asks.len(),
        1,
        "GT-D15 signed \"standing or trial\"; trial is not optional. {asks:?}"
    );
    assert_eq!(asks[0].venue_id.as_deref(), Some(venue.venue_id.as_str()));
    assert_eq!(asks[0].title, "Ask Trial Cafe for a review");
}

#[test]
fn p7t17_standing_without_delivery_raises_no_ask() {
    let mut conn = mem();
    standing_venue(&mut conn, "Cafe");
    let asks = review_asks(&conn);
    assert!(
        asks.is_empty(),
        "ruling C makes a delivery the single trigger; \
         stage alone must never raise an ask. {asks:?}"
    );
}

#[test]
fn p7t18_trial_without_delivery_raises_no_ask() {
    let mut conn = mem();
    venue_at(&mut conn, "Trial Cafe", "trial");
    let asks = review_asks(&conn);
    assert!(
        asks.is_empty(),
        "ruling C makes a delivery the single trigger; \
         stage alone must never raise an ask. {asks:?}"
    );
}

#[test]
fn p7t19_archived_with_delivery_raises_no_ask() {
    let mut conn = mem();
    let venue = standing_venue(&mut conn, "Old Cafe");
    deliver_to(&mut conn, &venue.venue_id);
    marketing::archive_venue(&mut conn, &venue.venue_id).unwrap();
    let asks = review_asks(&conn);
    assert!(
        asks.is_empty(),
        "archived venue must not raise an ask: {asks:?}"
    );
}

#[test]
fn p7t20_talking_with_delivery_raises_no_ask() {
    let mut conn = mem();
    let venue = venue_at(&mut conn, "Talking Cafe", "talking");
    deliver_to(&mut conn, &venue.venue_id);
    let asks = review_asks(&conn);
    assert!(
        asks.is_empty(),
        "talking is not standing or trial: {asks:?}"
    );
}

/// P7-CLOSE - a review observed today is not stale.
#[test]
fn p7t21_fresh_review_raises_no_stale_line() {
    let mut conn = mem();
    standing_venue(&mut conn, "Fresh Cafe");
    marketing::observe_reviews(
        &mut conn,
        &db::local_date_today(),
        12,
        "google_business_profile",
    )
    .unwrap();
    assert!(
        actions_of(&conn, "review_stale").is_empty(),
        "a review observed today is not stale"
    );
}
/// P7-CLOSE - exactly thirty days old is stale, and the line is the farm's,
/// not a venue's.
#[test]
fn p7t22_review_thirty_days_old_with_active_venue_raises_one_line() {
    let mut conn = mem();
    standing_venue(&mut conn, "Standing Cafe");
    marketing::observe_reviews(&mut conn, &days_ago(30), 12, "google_business_profile").unwrap();
    let lines = actions_of(&conn, "review_stale");
    assert_eq!(lines.len(), 1, "exactly one line: {lines:?}");
    assert_eq!(lines[0].title, marketing::REVIEW_STALE_TITLE);
    assert_eq!(lines[0].venue_id, None, "the count is the farm's");
    assert_eq!(lines[0].attention_id, None, "not an attention row");
}
/// P7-CLOSE - ruling B. The venue set is the review-ask query's: standing or
/// trial, not archived. A talking venue is not one of them.
#[test]
fn p7t23_stale_review_without_active_venue_raises_nothing() {
    let mut conn = mem();
    venue_at(&mut conn, "Quiet Cafe", "talking");
    marketing::observe_reviews(&mut conn, &days_ago(60), 12, "google_business_profile").unwrap();
    assert!(
        actions_of(&conn, "review_stale").is_empty(),
        "ruling B: standing or trial only"
    );
}
/// P7-CLOSE - the line retires itself. There is no dismiss door and none is
/// wanted: observing again is the work.
#[test]
fn p7t24_fresher_observation_retires_the_stale_line() {
    let mut conn = mem();
    standing_venue(&mut conn, "Standing Cafe");
    marketing::observe_reviews(&mut conn, &days_ago(45), 12, "google_business_profile").unwrap();
    assert_eq!(actions_of(&conn, "review_stale").len(), 1);
    marketing::observe_reviews(
        &mut conn,
        &db::local_date_today(),
        13,
        "google_business_profile",
    )
    .unwrap();
    assert!(
        actions_of(&conn, "review_stale").is_empty(),
        "a fresher observation retires the line"
    );
}
/// P7-CLOSE - the one-time item, and the date that ends it.
#[test]
fn p7t25_unset_gbp_raises_one_line_and_setting_it_retires() {
    let conn = mem();
    let lines = actions_of(&conn, "gbp_verify");
    assert_eq!(lines.len(), 1, "one line while unverified: {lines:?}");
    assert_eq!(lines[0].title, marketing::GBP_VERIFY_TITLE);
    assert_eq!(lines[0].venue_id, None);
    assert_eq!(lines[0].attention_id, None);
    marketing::set_gbp_verified(&conn, Some(&db::local_date_today())).unwrap();
    assert!(
        actions_of(&conn, "gbp_verify").is_empty(),
        "a recorded date retires the one-time item"
    );
}
/// P7-CLOSE - the two signed sentences, byte for byte.
#[test]
fn p7t26_close_out_titles_are_the_signed_bytes() {
    assert_eq!(
        marketing::REVIEW_STALE_TITLE,
        "Update Google review count — last observed over 30 days ago"
    );
    assert_eq!(
        marketing::GBP_VERIFY_TITLE,
        "Record Google Business Profile verification date"
    );
}
/// P7-CLOSE - neither new line gates or suppresses the ask. GT-D15 and p7t10
/// stand: GBP is reference data, never a precondition.
#[test]
fn p7t27_close_out_lines_do_not_gate_the_review_ask() {
    let mut conn = mem();
    let venue = standing_venue(&mut conn, "Standing Cafe");
    deliver_to(&mut conn, &venue.venue_id);
    marketing::observe_reviews(&mut conn, &days_ago(90), 3, "google_business_profile").unwrap();
    assert_eq!(review_asks(&conn).len(), 1, "the ask is untouched");
    assert_eq!(actions_of(&conn, "review_stale").len(), 1);
    assert_eq!(actions_of(&conn, "gbp_verify").len(), 1);
}
