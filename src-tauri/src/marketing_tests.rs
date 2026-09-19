//! Wave 2 — marketing domain seals, flush, replay, export/import.

use crate::attention;
use crate::capacity_gate::{CapacitySight, CapacityState};
use crate::db;
use crate::event_file;
use crate::event_partition::{marketing_kinds, EventDomain, Kind, MARKETING_KINDS};
use crate::events::{self, EventRecord};
use crate::export;
use crate::identity::{self, enforce_cutover_lock, is_farm_truth, LOCK_ACTIVE_IN_TEST};
use crate::import;
use crate::marketing::{
    self, FollowupClearPayload, FollowupSetPayload, ReviewRequestPayload, SamplePayload,
    StandingRequestDecidedPayload, StandingRequestPayload, TouchPayload, VenueArchivePayload,
    VenuePayload, FOLLOWUP_CLEAR_PAYLOAD_FIELD_NAMES, FOLLOWUP_SET_PAYLOAD_FIELD_NAMES,
    REVIEW_REQUESTED_PAYLOAD_FIELD_NAMES, SAMPLE_PAYLOAD_FIELD_NAMES,
    STANDING_REQUEST_DECIDED_PAYLOAD_FIELD_NAMES, STANDING_REQUEST_PAYLOAD_FIELD_NAMES,
    TOUCH_PAYLOAD_FIELD_NAMES, VENUE_ARCHIVE_PAYLOAD_FIELD_NAMES, VENUE_PAYLOAD_FIELD_NAMES,
};
use crate::models::CapacityRow;
use crate::projection;
use crate::scans;
use crate::trays;
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn event_log_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap()
}

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-mkt-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn m1_marketing_kinds_tier_and_all_len() {
    // Bumped for the silent-shortfall residual (wholesale.write_off)
    // and GT-D17 (standing.requested).
    assert_eq!(Kind::ALL.len(), 54);
    for kind in MARKETING_KINDS {
        let k = Kind::parse(kind).unwrap();
        assert_eq!(k.tier(), (EventDomain::Marketing, None));
    }
    assert_eq!(marketing_kinds(), MARKETING_KINDS.to_vec());
}

#[test]
fn m2_money_keys_refused_on_every_payload_struct() {
    let mut conn = mem();
    let before = event_log_count(&conn);
    let money_keys = ["amount", "price", "cents", "value"];
    let kinds_and_bases: Vec<(Kind, serde_json::Value)> = vec![
        (
            Kind::VenueRecorded,
            json!({
                "venueId": "v1",
                "name": "Cafe",
                "venueType": "cafe",
            }),
        ),
        (
            Kind::VenueArchived,
            json!({ "venueId": "v1", "archivedAt": "2026-08-11T00:00:00.000Z" }),
        ),
        (
            Kind::SampleDropped,
            json!({
                "sampleId": "s1",
                "venueId": "v1",
                "droppedOn": "2026-08-11",
                "varieties": ["peas"],
                "packCount": 1,
            }),
        ),
        (
            Kind::TouchLogged,
            json!({
                "touchId": "t1",
                "venueId": "v1",
                "touchedOn": "2026-08-11",
                "channel": "visit",
            }),
        ),
        (
            Kind::FollowupSet,
            json!({
                "followupId": "f1",
                "venueId": "v1",
                "dueOn": "2026-08-14",
                "what": "check",
            }),
        ),
        (
            Kind::FollowupCleared,
            json!({
                "followupId": "f1",
                "clearedAt": "2026-08-11T00:00:00.000Z",
            }),
        ),
        (
            Kind::ReviewRequested,
            json!({
                "venueId": "v1",
                "decidedOn": "2026-08-14",
                "outcome": "asked",
            }),
        ),
        (
            Kind::StandingRequested,
            json!({
                "requestId": "r1",
                "token": "0123456789abcdef0123456789abcdef",
                "venueId": "v1",
                "varieties": ["peas"],
                "bagsPerCycle": 1,
                "requestedAt": "2026-08-17T14:00:00Z",
            }),
        ),
        (
            Kind::StandingRequestDecided,
            json!({
                "requestId": "r1",
                "outcome": "dismissed",
                "decidedAt": "2026-08-17T15:00:00Z",
            }),
        ),
    ];

    for (kind, mut base) in kinds_and_bases {
        for money in &money_keys {
            base.as_object_mut()
                .unwrap()
                .insert((*money).into(), json!(1));
            let event = EventRecord::originated(
                kind,
                "marketing",
                "entity".to_string(),
                base.clone(),
                json!({ "op": "none" }),
                "2026-08-11T00:00:00.000Z".to_string(),
                None,
                None,
                Some(format!("ev-{}-{money}", kind.as_str())),
            );
            let tx = conn.transaction().unwrap();
            let err = events::write_event(&tx, &event).unwrap_err();
            assert!(
                err.contains(money),
                "kind {} money {money}: err={err}",
                kind.as_str()
            );
            std::mem::drop(tx);
            assert_eq!(event_log_count(&conn), before);
            base.as_object_mut().unwrap().remove(*money);
        }
    }
}

#[test]
fn m3_field_names_match_structs() {
    // Mirror consumption T6c: lists must equal the struct fields.
    assert_eq!(
        VENUE_PAYLOAD_FIELD_NAMES,
        &[
            "venue_id",
            "name",
            "venue_type",
            "contact",
            "phone",
            "address",
            "note",
        ]
    );
    let _: VenuePayload = serde_json::from_value(json!({
        "venueId": "v",
        "name": "n",
        "venueType": "t",
    }))
    .unwrap();

    assert_eq!(
        VENUE_ARCHIVE_PAYLOAD_FIELD_NAMES,
        &["venue_id", "archived_at"]
    );
    let _: VenueArchivePayload = serde_json::from_value(json!({
        "venueId": "v",
        "archivedAt": "2026-08-11T00:00:00.000Z",
    }))
    .unwrap();

    assert_eq!(
        SAMPLE_PAYLOAD_FIELD_NAMES,
        &[
            "sample_id",
            "venue_id",
            "dropped_on",
            "varieties",
            "pack_count",
            "note",
            "token",
        ]
    );
    let _: SamplePayload = serde_json::from_value(json!({
        "sampleId": "s",
        "venueId": "v",
        "droppedOn": "2026-08-11",
        "varieties": ["a"],
        "packCount": 1,
    }))
    .unwrap();

    assert_eq!(
        TOUCH_PAYLOAD_FIELD_NAMES,
        &[
            "touch_id",
            "venue_id",
            "touched_on",
            "channel",
            "outcome",
            "note"
        ]
    );
    let _: TouchPayload = serde_json::from_value(json!({
        "touchId": "t",
        "venueId": "v",
        "touchedOn": "2026-08-11",
        "channel": "visit",
    }))
    .unwrap();

    assert_eq!(
        FOLLOWUP_SET_PAYLOAD_FIELD_NAMES,
        &["followup_id", "venue_id", "due_on", "what"]
    );
    let _: FollowupSetPayload = serde_json::from_value(json!({
        "followupId": "f",
        "venueId": "v",
        "dueOn": "2026-08-14",
        "what": "x",
    }))
    .unwrap();

    assert_eq!(
        FOLLOWUP_CLEAR_PAYLOAD_FIELD_NAMES,
        &["followup_id", "cleared_at"]
    );
    let _: FollowupClearPayload = serde_json::from_value(json!({
        "followupId": "f",
        "clearedAt": "2026-08-11T00:00:00.000Z",
    }))
    .unwrap();

    assert_eq!(
        REVIEW_REQUESTED_PAYLOAD_FIELD_NAMES,
        &["venue_id", "decided_on", "outcome"]
    );
    let _: ReviewRequestPayload = serde_json::from_value(json!({
        "venueId": "v",
        "decidedOn": "2026-08-14",
        "outcome": "asked",
    }))
    .unwrap();

    assert_eq!(
        STANDING_REQUEST_PAYLOAD_FIELD_NAMES,
        &[
            "request_id",
            "token",
            "venue_id",
            "varieties",
            "bags_per_cycle",
            "requested_at",
            "contact"
        ]
    );
    let _: StandingRequestPayload = serde_json::from_value(json!({
        "requestId": "r",
        "token": "0123456789abcdef0123456789abcdef",
        "venueId": "v",
        "varieties": ["a"],
        "bagsPerCycle": 1,
        "requestedAt": "2026-08-17T14:00:00Z",
    }))
    .unwrap();
    assert_eq!(
        STANDING_REQUEST_DECIDED_PAYLOAD_FIELD_NAMES,
        &["request_id", "outcome", "decided_at"]
    );
    let _: StandingRequestDecidedPayload = serde_json::from_value(json!({
        "requestId": "r",
        "outcome": "accepted",
        "decidedAt": "2026-08-17T15:00:00Z",
    }))
    .unwrap();
}

#[test]
fn m4_trigger_refuses_marketing_class_and_unknown_kind() {
    let conn = mem();
    let err = conn
        .execute(
            "INSERT INTO event_log
             (id, kind, entity_type, entity_id, payload, inverse, created_at,
              origin, event_domain, event_class)
             VALUES ('e1', 'venue.recorded', 'venue', 'v1', '{}', '{}',
                     '2026-08-11T00:00:00.000Z', 'farm_os', 'marketing', 'money_out')",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("event_class must be NULL for marketing"),
        "{err}"
    );

    let err = conn
        .execute(
            "INSERT INTO event_log
             (id, kind, entity_type, entity_id, payload, inverse, created_at,
              origin, event_domain, event_class)
             VALUES ('e2', 'venue.invented', 'venue', 'v1', '{}', '{}',
                     '2026-08-11T00:00:00.000Z', 'farm_os', 'marketing', NULL)",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("kind invalid for marketing"), "{err}");
}

#[test]
fn m5_marketing_event_flushes_with_zero_lag() {
    let dir = temp_dir("m5");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    event_file::flush_events(&conn, &dir).unwrap();
    let live_max: i64 = conn
        .query_row("SELECT IFNULL(MAX(seq),0) FROM event_log", [], |r| r.get(0))
        .unwrap();
    let watermark = event_file::read_watermark(&event_file::events_path(&dir)).unwrap();
    assert_eq!(live_max - watermark, 0, "FLUSH LAG must be 0");

    // guard_row accepts the flushed marketing row (flush would have refused otherwise)
    let text = fs::read_to_string(event_file::events_path(&dir)).unwrap();
    assert!(text.contains("venue.recorded"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m6_drop_sample_writes_sample_and_followup_atomically() {
    let mut conn = mem();
    let venue = marketing::record_venue(
        &mut conn,
        "Cafe",
        "cafe",
        Some("Sam".into()),
        None,
        Some("1 Main St".into()),
        None,
    )
    .unwrap();
    let before = event_log_count(&conn);
    marketing::drop_sample(
        &mut conn,
        &venue.venue_id,
        "2026-08-11",
        vec!["peas".into()],
        2,
        None,
    )
    .unwrap();
    assert_eq!(event_log_count(&conn), before + 2);
    let samples: i64 = conn
        .query_row("SELECT COUNT(*) FROM mkt_samples", [], |r| r.get(0))
        .unwrap();
    let followups: i64 = conn
        .query_row("SELECT COUNT(*) FROM mkt_followups", [], |r| r.get(0))
        .unwrap();
    assert_eq!(samples, 1);
    assert_eq!(followups, 1);
    let due: String = conn
        .query_row("SELECT due_on FROM mkt_followups", [], |r| r.get(0))
        .unwrap();
    assert_eq!(due, "2026-08-14");

    // Rollback path: start a tx that fails after sample would land — use invalid channel via direct write.
    let before2 = event_log_count(&conn);
    let tx = conn.transaction().unwrap();
    // Force failure by writing a followup with empty what through validate.
    let bad = EventRecord::originated(
        Kind::FollowupSet,
        "followup",
        "fx".to_string(),
        json!({
            "followupId": "fx",
            "venueId": venue.venue_id,
            "dueOn": "2026-08-20",
            "what": "",
        }),
        json!({ "op": "none" }),
        "2026-08-11T00:00:00.000Z".to_string(),
        None,
        None,
        Some("ev-bad".to_string()),
    );
    assert!(events::write_event(&tx, &bad).is_err());
    std::mem::drop(tx);
    assert_eq!(event_log_count(&conn), before2);
}

#[test]
fn m7_overdue_followup_attention_and_atomic_resolve() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    // due_on in the past
    marketing::set_followup(&mut conn, &venue.venue_id, "2020-01-01", "call back").unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();
    let items = attention::check_attention(&conn).unwrap();
    let due: Vec<_> = items
        .iter()
        .filter(|i| i.kind == "marketing.followup_due")
        .collect();
    assert_eq!(due.len(), 1);
    let id = due[0].id.clone();
    marketing::raise_overdue_followups(&conn).unwrap();
    let again = attention::check_attention(&conn)
        .unwrap()
        .into_iter()
        .filter(|i| i.kind == "marketing.followup_due")
        .count();
    assert_eq!(again, 1, "raise_once is a no-op on second raise");

    let touches_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM mkt_touches", [], |r| r.get(0))
        .unwrap();
    marketing::FAIL_RESOLVE_AFTER_ATTENTION.with(|c| c.set(true));
    let err = marketing::resolve_followup(&mut conn, &id, "visit", None).unwrap_err();
    assert!(err.contains("forced failure"), "{err}");
    marketing::FAIL_RESOLVE_AFTER_ATTENTION.with(|c| c.set(false));
    let still_open = attention::get_open_item(&conn, &id).unwrap().is_some();
    assert!(still_open, "attention must roll back with forced failure");
    let touches_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM mkt_touches", [], |r| r.get(0))
        .unwrap();
    assert_eq!(touches_before, touches_after);

    marketing::resolve_followup(&mut conn, &id, "visit", None).unwrap();
    assert!(attention::get_open_item(&conn, &id).unwrap().is_none());
    let touches: i64 = conn
        .query_row("SELECT COUNT(*) FROM mkt_touches", [], |r| r.get(0))
        .unwrap();
    assert_eq!(touches, touches_before + 1);
}

#[test]
fn m8_verify_replay_passes_with_marketing_tables() {
    let dir = temp_dir("m8");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let venue = marketing::record_venue(
        &mut conn,
        "Cafe",
        "cafe",
        Some("Sam".into()),
        None,
        Some("1 Main St".into()),
        None,
    )
    .unwrap();
    marketing::drop_sample(
        &mut conn,
        &venue.venue_id,
        "2026-08-11",
        vec!["peas".into()],
        1,
        None,
    )
    .unwrap();
    marketing::log_touch(&mut conn, &venue.venue_id, "2026-08-11", "call", None, None).unwrap();
    event_file::flush_events(&conn, &dir).unwrap();
    std::mem::drop(conn);
    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    let exclusions = outcome.report().exclusions.join("\n");
    assert!(
        !exclusions.contains("mkt_"),
        "no mkt_ in exclusions:\n{exclusions}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m9_export_import_marketing_only_pre_cutover() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(true));
    let src = temp_dir("m9-src");
    let mut conn = db::open_and_migrate(&src.join("farm.db")).unwrap();
    let venue = marketing::record_venue(
        &mut conn,
        "Cafe",
        "cafe",
        Some("Sam".into()),
        None,
        Some("1 Main St".into()),
        None,
    )
    .unwrap();
    marketing::drop_sample(
        &mut conn,
        &venue.venue_id,
        "2026-08-11",
        vec!["peas".into()],
        1,
        None,
    )
    .unwrap();
    event_file::flush_events(&conn, &src).unwrap();
    let result = export::export_bundle(&conn, &src).unwrap();
    let bundle = PathBuf::from(result.bundle_path);
    assert!(bundle.join("marketing.csv").is_file());

    let target = temp_dir("m9-tgt");
    let mut tgt = db::open_and_migrate(&target.join("farm.db")).unwrap();
    import::apply_import(&mut tgt, Path::new(&bundle)).unwrap();
    let venues: i64 = tgt
        .query_row("SELECT COUNT(*) FROM mkt_venues", [], |r| r.get(0))
        .unwrap();
    let samples: i64 = tgt
        .query_row("SELECT COUNT(*) FROM mkt_samples", [], |r| r.get(0))
        .unwrap();
    assert_eq!(venues, 1);
    assert_eq!(samples, 1);
    // enforce_cutover_lock is a pure classifier: it refuses TraySown whether
    // or not the retired freeze gate ever consults it (identity::CUTOVER_LIVE).
    let sow_err = enforce_cutover_lock(Kind::TraySown).unwrap_err();
    assert_eq!(sow_err, "Farm truth lives in Farm OS until cutover.");
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let _ = fs::remove_dir_all(&src);
    let _ = fs::remove_dir_all(&target);
}

#[test]
fn m10_marketing_kinds_are_not_farm_truth() {
    for kind in [
        Kind::VenueRecorded,
        Kind::VenueCorrected,
        Kind::VenueArchived,
        Kind::SampleDropped,
        Kind::TouchLogged,
        Kind::FollowupSet,
        Kind::FollowupCleared,
        Kind::ReviewRequested,
    ] {
        assert!(!is_farm_truth(kind), "{kind:?}");
        enforce_cutover_lock(kind).unwrap();
    }
    let _ = identity::GROUNDTRUTH_APPLICATION_ID;
}

#[test]
fn phase1_resolve_clears_followup_same_tx() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let followup =
        marketing::set_followup(&mut conn, &venue.venue_id, "2020-01-01", "call back").unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();
    let attention_id: String = conn
        .query_row(
            "SELECT id FROM attention WHERE kind = 'marketing.followup_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&followup.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    let touch = marketing::resolve_followup(&mut conn, &attention_id, "visit", None).unwrap();

    let cleared_at: Option<String> = conn
        .query_row(
            "SELECT cleared_at FROM mkt_followups WHERE followup_id = ?1",
            [&followup.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        cleared_at.is_some(),
        "follow-up must be cleared in the same tx"
    );

    let visit_touches: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mkt_touches WHERE venue_id = ?1 AND channel = 'visit'",
            [&venue.venue_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(visit_touches, 1);

    let (resolved_at, resolved_by): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT resolved_at, resolved_by FROM attention WHERE id = ?1",
            [&attention_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(resolved_at.is_some());
    assert_eq!(resolved_by.as_deref(), Some("logged_touch"));

    let cleared_events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE kind = 'followup.cleared' AND entity_id = ?1",
            [&followup.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cleared_events, 1);
    let touch_events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE kind = 'touch.logged' AND entity_id = ?1",
            [&touch.touch_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(touch_events, 1);
}

#[test]
fn phase1_second_followup_same_venue_raises_again() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let followup_a =
        marketing::set_followup(&mut conn, &venue.venue_id, "2020-01-01", "call back").unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();
    let attention_a: String = conn
        .query_row(
            "SELECT id FROM attention WHERE kind = 'marketing.followup_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&followup_a.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    marketing::resolve_followup(&mut conn, &attention_a, "visit", None).unwrap();

    let followup_b =
        marketing::set_followup(&mut conn, &venue.venue_id, "2020-01-02", "check in").unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();
    let (entity_id, entity_type): (String, String) = conn
        .query_row(
            "SELECT entity_id, entity_type FROM attention
             WHERE kind = 'marketing.followup_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&followup_b.followup_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(entity_id, followup_b.followup_id);
    assert_eq!(entity_type, "followup");
}

#[test]
fn phase1_dismiss_sticks_per_followup_not_venue() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let followup_a =
        marketing::set_followup(&mut conn, &venue.venue_id, "2020-01-01", "call back").unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();
    let attention_a: String = conn
        .query_row(
            "SELECT id FROM attention WHERE kind = 'marketing.followup_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&followup_a.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    attention::dismiss_attention(&mut conn, &attention_a).unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();
    let open_a: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention WHERE kind = 'marketing.followup_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&followup_a.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(open_a, 0, "dismissal of A must stick");

    let followup_b =
        marketing::set_followup(&mut conn, &venue.venue_id, "2020-01-02", "check in").unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();
    let open_b: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention WHERE kind = 'marketing.followup_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&followup_b.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        open_b, 1,
        "follow-up B must raise independently of A's dismissal"
    );
}

#[test]
fn phase1_venue_keyed_attention_superseded_and_rekeyed() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    attention::raise(
        &conn,
        "marketing.followup_due",
        Some("venue"),
        Some(&venue.venue_id),
        "legacy",
        &["logged_touch", "dismiss"],
    )
    .unwrap();
    let followup =
        marketing::set_followup(&mut conn, &venue.venue_id, "2020-01-01", "call back").unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();

    let (resolved_at, resolved_by): (Option<String>, String) = conn
        .query_row(
            "SELECT resolved_at, resolved_by FROM attention
             WHERE kind = 'marketing.followup_due' AND entity_type = 'venue' AND entity_id = ?1",
            [&venue.venue_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(resolved_at.is_some());
    assert_eq!(resolved_by, "rekeyed_to_followup");

    let open_followup: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'marketing.followup_due' AND entity_id = ?1
               AND entity_type = 'followup' AND resolved_at IS NULL",
            [&followup.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(open_followup, 1);
}

#[test]
fn phase1_resolve_rollback_leaves_followup_open_and_attention_open() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let followup =
        marketing::set_followup(&mut conn, &venue.venue_id, "2020-01-01", "call back").unwrap();
    marketing::raise_overdue_followups(&conn).unwrap();
    let attention_id: String = conn
        .query_row(
            "SELECT id FROM attention WHERE kind = 'marketing.followup_due' AND entity_id = ?1 AND resolved_at IS NULL",
            [&followup.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    marketing::FAIL_RESOLVE_AFTER_ATTENTION.with(|c| c.set(true));
    let err = marketing::resolve_followup(&mut conn, &attention_id, "visit", None).unwrap_err();
    assert!(err.contains("forced failure"), "{err}");
    marketing::FAIL_RESOLVE_AFTER_ATTENTION.with(|c| c.set(false));

    let cleared_at: Option<String> = conn
        .query_row(
            "SELECT cleared_at FROM mkt_followups WHERE followup_id = ?1",
            [&followup.followup_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(cleared_at.is_none());
    let touches: i64 = conn
        .query_row("SELECT COUNT(*) FROM mkt_touches", [], |r| r.get(0))
        .unwrap();
    assert_eq!(touches, 0);
    let still_open = attention::get_open_item(&conn, &attention_id)
        .unwrap()
        .is_some();
    assert!(still_open, "attention must roll back with forced failure");
}

fn days_ago(n: i64) -> String {
    let today = db::local_date_today();
    let d = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d").unwrap();
    (d - chrono::Duration::days(n))
        .format("%Y-%m-%d")
        .to_string()
}

#[test]
fn phase3st_standing_demand_shortfall_math() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let mut targets = BTreeMap::new();
    targets.insert("Dun peas".into(), 3);
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &db::local_date_today(),
        Some(3),
        Some(vec!["Dun peas".into()]),
        Some(targets),
        None,
    )
    .unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.trays_week, 3);
    assert_eq!(d.sown_last_7_days, 0);
    assert_eq!(d.shortfall, 3);
    assert!(d.unallocated_venues.is_empty());
    assert_eq!(d.unallocated_trays_week, 0);

    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.shortfall, 2);

    trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.shortfall, 0);
    assert_eq!(d.sown_last_7_days, 3);

    marketing::archive_venue(&mut conn, &venue.venue_id).unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.trays_week, 0);
    assert_eq!(d.shortfall, 0);
}

#[test]
fn phase3st_standing_gone_quiet_raises_per_episode() {
    let mut conn = mem();
    let today = db::local_date_today();
    let past = days_ago(20);

    let venue_a =
        marketing::record_venue(&mut conn, "Cafe A", "cafe", None, None, None, None).unwrap();
    marketing::change_stage(
        &mut conn,
        &venue_a.venue_id,
        "standing",
        &today,
        Some(2),
        Some(vec!["peas".into()]),
        None,
        None,
    )
    .unwrap();
    marketing::log_touch(&mut conn, &venue_a.venue_id, &past, "visit", None, None).unwrap();
    marketing::raise_standing_quiet(&conn).unwrap();

    let episode_a = format!("{}:{past}", venue_a.venue_id);
    let open_a: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'marketing.standing_quiet'
               AND entity_id = ?1
               AND resolved_at IS NULL",
            [&episode_a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(open_a, 1);
    let entity_id: String = conn
        .query_row(
            "SELECT entity_id FROM attention
             WHERE kind = 'marketing.standing_quiet' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(entity_id, episode_a);

    let attention_a: String = conn
        .query_row(
            "SELECT id FROM attention
             WHERE kind = 'marketing.standing_quiet'
               AND entity_id = ?1
               AND resolved_at IS NULL",
            [&episode_a],
            |r| r.get(0),
        )
        .unwrap();
    attention::dismiss_attention(&mut conn, &attention_a).unwrap();
    marketing::raise_standing_quiet(&conn).unwrap();
    let open_after_dismiss: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'marketing.standing_quiet'
               AND entity_id = ?1
               AND resolved_at IS NULL",
            [&episode_a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(open_after_dismiss, 0, "episode dismissal sticks");

    marketing::log_touch(&mut conn, &venue_a.venue_id, &today, "visit", None, None).unwrap();
    marketing::raise_standing_quiet(&conn).unwrap();
    let open_recovered: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'marketing.standing_quiet' AND resolved_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(open_recovered, 0, "not quiet after today's touch");

    let venue_b =
        marketing::record_venue(&mut conn, "Cafe B", "cafe", None, None, None, None).unwrap();
    marketing::change_stage(
        &mut conn,
        &venue_b.venue_id,
        "standing",
        &today,
        Some(1),
        Some(vec!["peas".into()]),
        None,
        None,
    )
    .unwrap();
    marketing::raise_standing_quiet(&conn).unwrap();
    let episode_b = format!("{}:never", venue_b.venue_id);
    let open_b: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM attention
             WHERE kind = 'marketing.standing_quiet'
               AND entity_id = ?1
               AND resolved_at IS NULL",
            [&episode_b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(open_b, 1);
}

#[test]
fn phase3st_weekly_gone_quiet_includes_standing() {
    let mut conn = mem();
    let today = db::local_date_today();
    let past = days_ago(20);
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &today,
        Some(3),
        Some(vec!["peas".into()]),
        None,
        None,
    )
    .unwrap();
    marketing::log_touch(&mut conn, &venue.venue_id, &past, "visit", None, None).unwrap();

    let known = CapacityState::Known(CapacitySight {
        rows: vec![CapacityRow {
            harvest_date: "2026-08-20".into(),
            crop_id: "dun-peas".into(),
            crop_name: "Dun peas".into(),
            trays: 20,
            expected_yield_oz: 100.0,
            sold_trays: 2,
            remaining_trays: 18,
            harvested_trays: 0,
            cover_supply: 20,
            cover_promised: 2,
            cover_remaining: 18,
        }],
        taken_at: "2026-08-12T12:00:00Z".into(),
        file_name: "live".into(),
    });
    let view = marketing::weekly_actions_with_capacity(&conn, &known).unwrap();
    assert!(
        view.actions
            .iter()
            .any(|a| a.kind == "gone_quiet" && a.title.contains("(standing)")),
        "expected standing gone_quiet, got {:?}",
        view.actions
    );
}

#[test]
fn phase3pf_standing_varieties_union() {
    let mut conn = mem();
    let today = db::local_date_today();
    let venue_a =
        marketing::record_venue(&mut conn, "Cafe A", "cafe", None, None, None, None).unwrap();
    marketing::change_stage(
        &mut conn,
        &venue_a.venue_id,
        "standing",
        &today,
        Some(2),
        Some(vec!["peas".into(), "radish".into()]),
        None,
        None,
    )
    .unwrap();
    let venue_b =
        marketing::record_venue(&mut conn, "Cafe B", "cafe", None, None, None, None).unwrap();
    marketing::change_stage(
        &mut conn,
        &venue_b.venue_id,
        "standing",
        &today,
        Some(2),
        Some(vec!["radish".into(), "kale".into()]),
        None,
        None,
    )
    .unwrap();
    let venue_c =
        marketing::record_venue(&mut conn, "Cafe C", "cafe", None, None, None, None).unwrap();
    marketing::change_stage(
        &mut conn,
        &venue_c.venue_id,
        "standing",
        &today,
        Some(1),
        None,
        None,
        None,
    )
    .unwrap();

    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.standing_varieties, vec!["kale", "peas", "radish"]);
    assert_eq!(d.shortfall, 0);
    assert_eq!(d.unallocated_venues.len(), 3);
    assert_eq!(d.unallocated_trays_week, 5);

    marketing::archive_venue(&mut conn, &venue_a.venue_id).unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.standing_varieties, vec!["kale", "radish"]);
    assert_eq!(d.unallocated_trays_week, 3);
}

#[test]
fn phase3pv_seal_targets_sum_and_sync() {
    let mut conn = mem();
    let today = db::local_date_today();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let mut ok = BTreeMap::new();
    ok.insert("A".into(), 2);
    ok.insert("B".into(), 1);
    let view = marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &today,
        Some(3),
        Some(vec!["A".into(), "B".into()]),
        Some(ok.clone()),
        None,
    )
    .unwrap();
    assert_eq!(view.variety_targets, Some(ok.clone()));
    assert_eq!(view.trays_week, Some(3));

    let mut mismatch = BTreeMap::new();
    mismatch.insert("A".into(), 2);
    mismatch.insert("B".into(), 1);
    let err = marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &today,
        Some(4),
        Some(vec!["A".into(), "B".into()]),
        Some(mismatch),
        None,
    )
    .unwrap_err();
    assert!(err.contains("sum"), "{err}");

    let mut zero = BTreeMap::new();
    zero.insert("A".into(), 0);
    let err = marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &today,
        Some(1),
        Some(vec!["A".into()]),
        Some(zero),
        None,
    )
    .unwrap_err();
    assert!(err.contains("at least 1"), "{err}");

    let empty: BTreeMap<String, i64> = BTreeMap::new();
    let err = marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &today,
        Some(1),
        Some(vec!["A".into()]),
        Some(empty),
        None,
    )
    .unwrap_err();
    assert!(err.contains("empty"), "{err}");

    let mut keys = BTreeMap::new();
    keys.insert("A".into(), 2);
    keys.insert("B".into(), 1);
    let err = marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &today,
        Some(3),
        Some(vec!["A".into()]),
        Some(keys),
        None,
    )
    .unwrap_err();
    assert!(err.contains("exactly match"), "{err}");

    let mut talking = BTreeMap::new();
    talking.insert("A".into(), 1);
    let err = marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "talking",
        &today,
        None,
        None,
        Some(talking),
        None,
    )
    .unwrap_err();
    assert!(err.contains("only allowed for standing"), "{err}");

    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &today,
        Some(3),
        None,
        None,
        None,
    )
    .unwrap();
}

#[test]
fn phase3pv_demand_is_per_variety() {
    let mut conn = mem();
    let today = db::local_date_today();
    let v1 = marketing::record_venue(&mut conn, "V1", "cafe", None, None, None, None).unwrap();
    let mut t1 = BTreeMap::new();
    t1.insert("Dun peas".into(), 2);
    t1.insert("Kale".into(), 1);
    marketing::change_stage(
        &mut conn,
        &v1.venue_id,
        "standing",
        &today,
        Some(3),
        Some(vec!["Dun peas".into(), "Kale".into()]),
        Some(t1),
        None,
    )
    .unwrap();
    let v2 = marketing::record_venue(&mut conn, "V2", "cafe", None, None, None, None).unwrap();
    let mut t2 = BTreeMap::new();
    t2.insert("Dun peas".into(), 1);
    marketing::change_stage(
        &mut conn,
        &v2.venue_id,
        "standing",
        &today,
        Some(1),
        Some(vec!["Dun peas".into()]),
        Some(t2),
        None,
    )
    .unwrap();

    trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    let peas = d
        .varieties
        .iter()
        .find(|v| v.name == "Dun peas")
        .expect("peas");
    let kale = d.varieties.iter().find(|v| v.name == "Kale").expect("kale");
    assert_eq!(peas.shortfall, 1);
    assert_eq!(kale.shortfall, 1);
    assert_eq!(d.shortfall, 2);

    trays::sow_tray(&mut conn, "kale", 1).unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    let peas = d
        .varieties
        .iter()
        .find(|v| v.name == "Dun peas")
        .expect("peas");
    let kale = d.varieties.iter().find(|v| v.name == "Kale").expect("kale");
    assert_eq!(peas.shortfall, 1);
    assert_eq!(kale.shortfall, 0);
}

#[test]
fn phase3pv_legacy_totals_are_loud_not_counted() {
    let mut conn = mem();
    let venue =
        marketing::record_venue(&mut conn, "Legacy Cafe", "cafe", None, None, None, None).unwrap();
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &db::local_date_today(),
        Some(3),
        None,
        None,
        None,
    )
    .unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    assert!(d.unallocated_venues.iter().any(|n| n == "Legacy Cafe"));
    assert_eq!(d.unallocated_trays_week, 3);
    assert_eq!(d.trays_week, 3);
    assert_eq!(d.shortfall, 0);
    assert!(d.varieties.is_empty());
}

#[test]
fn b6_unmatched_target_key_is_visible_not_dropped() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let mut targets = BTreeMap::new();
    targets.insert("Wheatgrass".into(), 4);
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        &db::local_date_today(),
        Some(4),
        Some(vec!["Wheatgrass".into()]),
        Some(targets),
        None,
    )
    .unwrap();
    let d = marketing::standing_demand(&conn).unwrap();
    assert_eq!(d.varieties.len(), 1);
    assert_eq!(d.varieties[0].name, "Wheatgrass");
    assert_eq!(d.varieties[0].trays_week, 4);
    assert_eq!(d.varieties[0].sown_last_7_days, 0);
    assert_eq!(d.varieties[0].shortfall, 4);
    assert!(
        d.standing_varieties.iter().any(|n| n == "Wheatgrass"),
        "{:?}",
        d.standing_varieties
    );
}

fn complete_venue(conn: &mut Connection, name: &str) -> marketing::VenueView {
    marketing::record_venue(
        conn,
        name,
        "cafe",
        Some("Sam".into()),
        None,
        Some("1 Main St".into()),
        None,
    )
    .unwrap()
}

fn drop_peas(conn: &mut Connection, venue_id: &str) -> Result<marketing::SampleView, String> {
    marketing::drop_sample(conn, venue_id, "2026-08-11", vec!["peas".into()], 1, None)
}

#[test]
fn qr1_drop_refused_on_incomplete_venue_handler_only() {
    let mut conn = mem();
    let v = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    assert!(!v.qr_ready);
    let before = event_log_count(&conn);
    let err = drop_peas(&mut conn, &v.venue_id).unwrap_err();
    assert_eq!(err, marketing::QR_FIELDS_REFUSAL);
    assert_eq!(
        err,
        "Venue name, delivery address, and contact are required before a QR can be generated."
    );
    assert_eq!(
        event_log_count(&conn),
        before,
        "a refused drop writes nothing"
    );
    let followups: i64 = conn
        .query_row("SELECT COUNT(*) FROM mkt_followups", [], |r| r.get(0))
        .unwrap();
    assert_eq!(followups, 0);
    // Handler-only: the same fact as a historic event still applies on replay.
    let historic = EventRecord::originated(
        Kind::SampleDropped,
        "sample",
        "s-hist".to_string(),
        json!({"sampleId":"s-hist","venueId": v.venue_id,"droppedOn":"2026-08-01",
               "varieties":["peas"],"packCount":1}),
        json!({"op":"none"}),
        "2026-08-01T00:00:00.000Z".to_string(),
        None,
        None,
        Some("ev-hist".to_string()),
    );
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &historic).unwrap();
    events::write_event(&tx, &historic).unwrap();
    tx.commit().unwrap();
    let token: Option<String> = conn
        .query_row(
            "SELECT token FROM mkt_samples WHERE sample_id = 's-hist'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        token.is_none(),
        "historic drop carries no token and is not refused"
    );
}

#[test]
fn qr2_contact_or_phone_satisfies_gate_address_always_required() {
    let mut conn = mem();
    let phone_only = marketing::record_venue(
        &mut conn,
        "Phone Cafe",
        "cafe",
        None,
        Some("555-0100".into()),
        Some("1 Main St".into()),
        None,
    )
    .unwrap();
    assert!(phone_only.qr_ready);
    drop_peas(&mut conn, &phone_only.venue_id).unwrap();
    let contact_only = marketing::record_venue(
        &mut conn,
        "Contact Cafe",
        "cafe",
        Some("Sam".into()),
        None,
        Some("2 Main St".into()),
        None,
    )
    .unwrap();
    assert!(contact_only.qr_ready);
    drop_peas(&mut conn, &contact_only.venue_id).unwrap();
    let no_address = marketing::record_venue(
        &mut conn,
        "No Address",
        "cafe",
        Some("Sam".into()),
        Some("555-0100".into()),
        None,
        None,
    )
    .unwrap();
    assert!(!no_address.qr_ready);
    assert_eq!(
        drop_peas(&mut conn, &no_address.venue_id).unwrap_err(),
        marketing::QR_FIELDS_REFUSAL
    );
    // Whitespace is not a value.
    let blank = marketing::record_venue(
        &mut conn,
        "Blank",
        "cafe",
        Some("  ".into()),
        Some(" ".into()),
        Some("3 Main St".into()),
        None,
    )
    .unwrap();
    assert!(!blank.qr_ready);
    assert_eq!(
        drop_peas(&mut conn, &blank.venue_id).unwrap_err(),
        marketing::QR_FIELDS_REFUSAL
    );
    // Completing the venue through correct_venue opens the door.
    let fixed = marketing::correct_venue(
        &mut conn,
        &no_address.venue_id,
        "No Address",
        "cafe",
        Some("Sam".into()),
        Some("555-0100".into()),
        Some("9 Main St".into()),
        None,
    )
    .unwrap();
    assert!(fixed.qr_ready);
    drop_peas(&mut conn, &no_address.venue_id).unwrap();
}

#[test]
fn qr3_token_is_derived_opaque_stored_and_frozen_in_payload() {
    let mut conn = mem();
    let v = complete_venue(&mut conn, "Cafe");
    let s = drop_peas(&mut conn, &v.venue_id).unwrap();
    let token = s.token.clone().expect("token generated at drop");
    assert_eq!(token.len(), 32);
    assert!(token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    assert_ne!(token, s.sample_id);
    assert!(!token.contains(&s.sample_id));
    let (row_token, payload): (Option<String>, String) = conn
        .query_row(
            "SELECT m.token, e.payload FROM mkt_samples m
         JOIN event_log e ON e.entity_id = m.sample_id AND e.kind = 'sample.dropped'
         WHERE m.sample_id = ?1",
            [&s.sample_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row_token.as_deref(), Some(token.as_str()));
    let p: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(
        p["token"].as_str(),
        Some(token.as_str()),
        "token frozen in the payload"
    );
    // Derivation is exactly the signed rule: sha256(sample_id ':' entropy)[..16], lowercase hex.
    let expect: String = Sha256::digest(b"s-x:e-y")[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(marketing::sample_token("s-x", "e-y"), expect);
    // Two drops never share a token; list_samples carries it.
    let s2 = drop_peas(&mut conn, &v.venue_id).unwrap();
    assert_ne!(s2.token, s.token);
    assert!(marketing::list_samples(&conn)
        .unwrap()
        .iter()
        .all(|x| x.token.is_some()));
}

#[test]
fn qr4_schema_v28_token_column_and_partial_unique_index() {
    let conn = mem();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, db::SCHEMA_VERSION);
    let cols: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('mkt_samples') WHERE name = 'token'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cols, 1);
    let idx: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_mkt_samples_token'",
        [], |r| r.get(0)).unwrap();
    assert_eq!(idx, 1);
    assert!(marketing::MKT_SAMPLES_COLUMNS.contains(&"token"));
    // Idempotent: a rewound user_version re-runs the step without error.
    conn.pragma_update(None, "user_version", 28).unwrap();
    db::migrate(&conn).unwrap();
    let again: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(again, db::SCHEMA_VERSION);
    // Uniqueness holds for real tokens; NULLs (historic drops) are free.
    let ins = "INSERT INTO mkt_samples (sample_id, venue_id, dropped_on, varieties, pack_count, note, token, created_at) VALUES (?1,'v','2026-08-01','[\"peas\"]',1,NULL,?2,'t')";
    conn.execute(ins, rusqlite::params!["a", Option::<String>::None])
        .unwrap();
    conn.execute(ins, rusqlite::params!["b", Option::<String>::None])
        .unwrap();
    conn.execute(ins, rusqlite::params!["c", Some("tok")])
        .unwrap();
    assert!(conn
        .execute(ins, rusqlite::params!["d", Some("tok")])
        .is_err());
}

fn candidate_event(
    request_id: &str,
    token: &str,
    venue_id: &str,
    bags: i64,
    contact: Option<&str>,
) -> EventRecord {
    let mut payload = json!({
        "requestId": request_id,
        "token": token,
        "venueId": venue_id,
        "varieties": ["peas"],
        "bagsPerCycle": bags,
        "requestedAt": "2026-08-17T14:00:00Z",
    });
    if let Some(c) = contact {
        payload
            .as_object_mut()
            .unwrap()
            .insert("contact".into(), json!(c));
    }
    EventRecord::originated(
        Kind::StandingRequested,
        "standing_request",
        request_id.to_string(),
        payload,
        json!({ "op": "none" }),
        projection::handler_now(),
        None,
        None,
        Some(projection::handler_new_id()),
    )
}

#[test]
fn gt17a_standing_requested_is_in_the_closed_set_marketing_tier() {
    assert_eq!(Kind::ALL.len(), 54);
    let k = Kind::parse("standing.requested").unwrap();
    assert_eq!(k, Kind::StandingRequested);
    assert_eq!(k.as_str(), "standing.requested");
    assert_eq!(k.tier(), (EventDomain::Marketing, None));
    assert!(MARKETING_KINDS.contains(&"standing.requested"));
    assert!(marketing_kinds().contains(&"standing.requested"));
    assert!(!is_farm_truth(k), "a candidate is not farm truth (GT-D2)");
    assert!(marketing::MKT_STANDING_REQUESTS_COLUMNS.contains(&"request_id"));
}

#[test]
fn gt17b_candidate_projects_replays_and_is_delete_proof() {
    let dir = temp_dir("gt17b");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().expect("fence 1 token");
    let ev = candidate_event("req-1", &token, &venue.venue_id, 2, Some("Sam 555-0100"));
    let before = event_log_count(&conn);
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &ev).unwrap();
    events::write_event(&tx, &ev).unwrap();
    tx.commit().unwrap();
    assert_eq!(event_log_count(&conn), before + 1);
    #[allow(clippy::type_complexity)]
    // H-7 Class E: a query_row tuple, not a signature. Nothing here to narrow.
    let row: (
        String,
        String,
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT token, venue_id, varieties, bags_per_cycle, requested_at, contact,
                    decided_at, outcome
             FROM mkt_standing_requests WHERE request_id = 'req-1'",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0, token);
    assert_eq!(row.1, venue.venue_id);
    assert_eq!(row.2, "[\"peas\"]");
    assert_eq!(row.3, 2);
    assert_eq!(row.4, "2026-08-17T14:00:00Z");
    assert_eq!(row.5.as_deref(), Some("Sam 555-0100"));
    assert!(
        row.6.is_none() && row.7.is_none(),
        "undecided until Fence 3"
    );
    // Same request_id again fails loudly — never a second row, never hidden.
    let dup = candidate_event("req-1", &token, &venue.venue_id, 3, None);
    let tx = conn.transaction().unwrap();
    assert!(projection::apply_event(&tx, &dup).is_err());
    std::mem::drop(tx);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM mkt_standing_requests", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1);
    // Delete-proof.
    let err = conn
        .execute(
            "DELETE FROM mkt_standing_requests WHERE request_id = 'req-1'",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("append-only"), "{err}");
    // Replay reproduces the row; the table is compared, not excluded.
    event_file::flush_events(&conn, &dir).unwrap();
    std::mem::drop(conn);
    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    let exclusions = outcome.report().exclusions.join("\n");
    assert!(
        !exclusions.contains("mkt_standing_requests"),
        "candidates are compared:\n{exclusions}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn gt17c_payload_seal_refuses_bad_shapes_and_money() {
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().unwrap();
    let good = candidate_event("req-ok", &token, &venue.venue_id, 1, None);
    marketing::validate_marketing_event(&good).unwrap();
    let base = good.payload.clone();
    let before = event_log_count(&conn);
    let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
    let mut v = base.clone();
    v["extra"] = json!(1);
    cases.push(("unknown field", v));
    let mut v = base.clone();
    v["token"] = json!("abc");
    cases.push(("token shape", v));
    let mut v = base.clone();
    v["token"] = json!(token.to_uppercase());
    cases.push(("token case", v));
    let mut v = base.clone();
    v["bagsPerCycle"] = json!(0);
    cases.push(("bags 0", v));
    let mut v = base.clone();
    v["varieties"] = json!([]);
    cases.push(("empty varieties", v));
    let mut v = base.clone();
    v["requestedAt"] = json!("yesterday");
    cases.push(("requested_at", v));
    let mut v = base.clone();
    v["price"] = json!(12);
    cases.push(("money key", v));
    let mut v = base.clone();
    v.as_object_mut().unwrap().remove("token");
    cases.push(("missing token", v));
    for (label, payload) in cases {
        let ev = EventRecord::originated(
            Kind::StandingRequested,
            "standing_request",
            "req-bad".to_string(),
            payload,
            json!({ "op": "none" }),
            projection::handler_now(),
            None,
            None,
            Some(projection::handler_new_id()),
        );
        assert!(
            marketing::validate_marketing_event(&ev).is_err(),
            "{label} must be refused"
        );
        let tx = conn.transaction().unwrap();
        assert!(
            events::write_event(&tx, &ev).is_err(),
            "{label} must not write"
        );
        std::mem::drop(tx);
    }
    assert_eq!(event_log_count(&conn), before);
}

#[test]
fn gt17d_v28_triggers_refuse_the_kind_until_the_v29_reinstall() {
    // The whitelist a farm carried after Fence 1 (schema v28), frozen here.
    const MARKETING_KINDS_V28: &[&str] = &[
        "venue.recorded",
        "venue.corrected",
        "venue.archived",
        "sample.dropped",
        "touch.logged",
        "followup.set",
        "followup.cleared",
        "stage.changed",
        "reviews.observed",
        "review.requested",
    ];
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().unwrap();
    // Simulate a farm that migrated to v28 before GT-D17: old whitelist, old version.
    db::drop_event_log_triggers(&conn).unwrap();
    conn.execute_batch(&crate::event_partition::schema_event_log_triggers_sql(
        &crate::event_partition::grow_kinds(),
        &crate::event_partition::register_kinds(),
        crate::event_partition::EVENT_CLASSES,
        MARKETING_KINDS_V28,
    ))
    .unwrap();
    conn.pragma_update(None, "user_version", 28).unwrap();
    let ev = candidate_event("req-trig", &token, &venue.venue_id, 1, None);
    let tx = conn.transaction().unwrap();
    let err = events::write_event(&tx, &ev).unwrap_err();
    assert!(err.contains("kind invalid for marketing"), "{err}");
    std::mem::drop(tx);
    // The v29 step reinstalls the whitelist from Kind::ALL.
    db::migrate(&conn).unwrap();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &ev).unwrap();
    events::write_event(&tx, &ev).unwrap();
    tx.commit().unwrap();
}

fn write_candidate(conn: &mut Connection, ev: &EventRecord) {
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, ev).unwrap();
    events::write_event(&tx, ev).unwrap();
    tx.commit().unwrap();
    marketing::raise_standing_requests(conn).unwrap();
}

fn open_request_card(conn: &Connection, request_id: &str) -> Option<crate::models::AttentionItem> {
    attention::check_attention(conn)
        .unwrap()
        .into_iter()
        .find(|i| {
            i.kind == marketing::STANDING_REQUEST_ATTENTION_KIND
                && i.entity_id.as_deref() == Some(request_id)
        })
}

fn request_row(conn: &Connection, request_id: &str) -> (Option<String>, Option<String>) {
    conn.query_row(
        "SELECT decided_at, outcome FROM mkt_standing_requests WHERE request_id = ?1",
        [request_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

#[test]
fn gt17e_second_kind_in_closed_set_and_v29_triggers_refuse_until_v30() {
    const MARKETING_KINDS_V29: &[&str] = &[
        "venue.recorded",
        "venue.corrected",
        "venue.archived",
        "sample.dropped",
        "touch.logged",
        "followup.set",
        "followup.cleared",
        "stage.changed",
        "reviews.observed",
        "review.requested",
        "standing.requested",
    ];
    assert_eq!(Kind::ALL.len(), 54);
    assert_eq!(db::SCHEMA_VERSION, 46);
    let k = Kind::parse("standing.request_decided").unwrap();
    assert_eq!(k, Kind::StandingRequestDecided);
    assert_eq!(k.as_str(), "standing.request_decided");
    assert_eq!(k.tier(), (EventDomain::Marketing, None));
    assert!(MARKETING_KINDS.contains(&"standing.request_decided"));
    assert!(marketing_kinds().contains(&"standing.request_decided"));
    assert!(!is_farm_truth(k));

    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().unwrap();
    write_candidate(
        &mut conn,
        &candidate_event("req-e", &token, &venue.venue_id, 1, None),
    );
    db::drop_event_log_triggers(&conn).unwrap();
    conn.execute_batch(&crate::event_partition::schema_event_log_triggers_sql(
        &crate::event_partition::grow_kinds(),
        &crate::event_partition::register_kinds(),
        crate::event_partition::EVENT_CLASSES,
        MARKETING_KINDS_V29,
    ))
    .unwrap();
    conn.pragma_update(None, "user_version", 29).unwrap();
    let decided = EventRecord::originated(
        Kind::StandingRequestDecided,
        "standing_request",
        "req-e".to_string(),
        json!({ "requestId": "req-e", "outcome": "dismissed", "decidedAt": "2026-08-17T15:00:00Z" }),
        json!({ "op": "none" }),
        "2026-08-17T15:00:00Z".to_string(),
        None,
        None,
        Some("ev-decided-e".to_string()),
    );
    let tx = conn.transaction().unwrap();
    let err = events::write_event(&tx, &decided).unwrap_err();
    assert!(err.contains("kind invalid for marketing"), "{err}");
    std::mem::drop(tx);
    db::migrate(&conn).unwrap();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &decided).unwrap();
    events::write_event(&tx, &decided).unwrap();
    tx.commit().unwrap();
    assert_eq!(request_row(&conn, "req-e").1.as_deref(), Some("dismissed"));
}

#[test]
fn gt17f_undecided_candidate_raises_marketing_card_with_exact_message() {
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Blue Door Bistro");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().unwrap();
    write_candidate(
        &mut conn,
        &candidate_event("req-f", &token, &venue.venue_id, 3, Some("Sam 555-0100")),
    );
    let card = open_request_card(&conn, "req-f").expect("card raised on check");
    assert_eq!(
        card.message,
        "Blue Door Bistro asks to put peas on standing — 3 bags per cycle. Contact given: Sam 555-0100."
    );
    assert_eq!(
        card.actions,
        vec!["accept".to_string(), "dismiss".to_string()]
    );
    assert_eq!(card.entity_type.as_deref(), Some("standing_request"));
    // Idempotent while open.
    let n = attention::check_attention(&conn)
        .unwrap()
        .iter()
        .filter(|i| i.kind == marketing::STANDING_REQUEST_ATTENTION_KIND)
        .count();
    assert_eq!(n, 1);
    // Message rules: singular bag, several varieties, no contact.
    assert_eq!(
        marketing::standing_request_message("Cafe", &["peas".into(), "sunflower".into()], 1, None),
        "Cafe asks to put peas, sunflower on standing — 1 bag per cycle."
    );
    assert_eq!(
        marketing::standing_request_message("Cafe", &["peas".into()], 2, Some("  ")),
        "Cafe asks to put peas on standing — 2 bags per cycle."
    );
}

#[test]
fn gt17g_dismiss_writes_decided_resolves_card_and_never_re_raises() {
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().unwrap();
    write_candidate(
        &mut conn,
        &candidate_event("req-g", &token, &venue.venue_id, 2, None),
    );
    assert!(open_request_card(&conn, "req-g").is_some());
    let before = event_log_count(&conn);
    let d = marketing::decide_standing_request(&mut conn, "req-g", "dismissed").unwrap();
    assert_eq!(d.outcome, "dismissed");
    assert!(d.stage.is_none());
    // decided event + attention.resolved, nothing else.
    assert_eq!(event_log_count(&conn), before + 2);
    let (decided_at, outcome) = request_row(&conn, "req-g");
    assert_eq!(decided_at.as_deref(), Some(d.decided_at.as_str()));
    assert_eq!(outcome.as_deref(), Some("dismissed"));
    assert!(open_request_card(&conn, "req-g").is_none(), "card resolved");
    assert!(
        open_request_card(&conn, "req-g").is_none(),
        "decided rows never re-raise"
    );
    let stages = marketing::list_stages(&conn).unwrap();
    assert!(
        stages.iter().all(|s| s.stage != "standing"),
        "dismiss never touches standing"
    );
    let err = marketing::decide_standing_request(&mut conn, "req-g", "accepted").unwrap_err();
    assert_eq!(err, marketing::STANDING_REQUEST_ALREADY_DECIDED);
    let err = marketing::decide_standing_request(&mut conn, "req-g", "maybe").unwrap_err();
    assert!(err.contains("accepted or dismissed"));
}

#[test]
fn gt17h_accept_single_variety_lands_standing_with_target() {
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().unwrap();
    write_candidate(
        &mut conn,
        &candidate_event("req-h", &token, &venue.venue_id, 3, None),
    );
    let before = event_log_count(&conn);
    let d = marketing::decide_standing_request(&mut conn, "req-h", "accepted").unwrap();
    // attention.resolved + standing.request_decided + stage.changed
    assert_eq!(event_log_count(&conn), before + 3);
    let stage = d.stage.expect("accept returns the standing row");
    assert_eq!(stage.stage, "standing");
    assert_eq!(stage.trays_week, Some(3));
    assert_eq!(stage.varieties, Some(vec!["peas".to_string()]));
    let mut want = BTreeMap::new();
    want.insert("peas".to_string(), 3);
    assert_eq!(stage.variety_targets, Some(want));
    assert_eq!(request_row(&conn, "req-h").1.as_deref(), Some("accepted"));
    assert!(open_request_card(&conn, "req-h").is_none());
    let demand = marketing::standing_demand(&conn).unwrap();
    assert!(demand
        .varieties
        .iter()
        .any(|v| v.name == "peas" && v.trays_week == 3));
    assert!(demand.unallocated_venues.is_empty());
}

#[test]
fn gt17i_accept_several_varieties_lands_trays_week_only() {
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = marketing::drop_sample(
        &mut conn,
        &venue.venue_id,
        "2026-08-11",
        vec!["peas".into(), "sunflower".into()],
        1,
        None,
    )
    .unwrap();
    let token = sample.token.clone().unwrap();
    let mut ev = candidate_event("req-i", &token, &venue.venue_id, 4, None);
    ev.payload["varieties"] = json!(["peas", "sunflower"]);
    write_candidate(&mut conn, &ev);
    let d = marketing::decide_standing_request(&mut conn, "req-i", "accepted").unwrap();
    let stage = d.stage.unwrap();
    assert_eq!(stage.trays_week, Some(4));
    assert_eq!(
        stage.varieties,
        Some(vec!["peas".to_string(), "sunflower".to_string()])
    );
    assert!(
        stage.variety_targets.is_none(),
        "several varieties: re-state by variety later"
    );
    let demand = marketing::standing_demand(&conn).unwrap();
    assert_eq!(demand.unallocated_venues, vec!["Cafe".to_string()]);
    assert_eq!(demand.unallocated_trays_week, 4);
}

#[test]
fn gt17j_accept_refused_on_incomplete_venue_and_generic_doors_refuse_the_kind() {
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().unwrap();
    write_candidate(
        &mut conn,
        &candidate_event("req-j", &token, &venue.venue_id, 2, None),
    );
    let card = open_request_card(&conn, "req-j").unwrap();
    // The venue loses its address after the drop.
    marketing::correct_venue(
        &mut conn,
        &venue.venue_id,
        "Cafe",
        "cafe",
        Some("Sam".into()),
        None,
        None,
        None,
    )
    .unwrap();
    let before = event_log_count(&conn);
    let err = marketing::decide_standing_request(&mut conn, "req-j", "accepted").unwrap_err();
    assert_eq!(err, marketing::STANDING_ACCEPT_REFUSAL);
    assert_eq!(
        err,
        "Standing cannot go live while venue, address, contact, or quantity is incomplete."
    );
    assert_eq!(
        event_log_count(&conn),
        before,
        "a refused accept writes nothing"
    );
    assert!(request_row(&conn, "req-j").0.is_none(), "still undecided");
    assert!(
        open_request_card(&conn, "req-j").is_some(),
        "card stays open"
    );
    // Generic doors cannot quiet a candidate.
    let err = attention::dismiss_attention(&mut conn, &card.id).unwrap_err();
    assert_eq!(err, marketing::STANDING_REQUEST_DECIDE_ONLY);
    let err = attention::resolve_attention(&mut conn, &card.id, "accept").unwrap_err();
    assert_eq!(err, marketing::STANDING_REQUEST_DECIDE_ONLY);
    assert!(open_request_card(&conn, "req-j").is_some());
    // Complete the venue: accept opens.
    marketing::correct_venue(
        &mut conn,
        &venue.venue_id,
        "Cafe",
        "cafe",
        Some("Sam".into()),
        None,
        Some("1 Main St".into()),
        None,
    )
    .unwrap();
    marketing::decide_standing_request(&mut conn, "req-j", "accepted").unwrap();
    assert!(open_request_card(&conn, "req-j").is_none());
}

#[test]
fn gt17k_accept_replaces_existing_standing_and_accepts_from_passed() {
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let mut old = BTreeMap::new();
    old.insert("peas".to_string(), 5);
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        "2026-08-11",
        Some(5),
        Some(vec!["peas".into()]),
        Some(old),
        None,
    )
    .unwrap();
    let sample = marketing::drop_sample(
        &mut conn,
        &venue.venue_id,
        "2026-08-11",
        vec!["sunflower".into()],
        1,
        None,
    )
    .unwrap();
    let token = sample.token.clone().unwrap();
    let mut ev = candidate_event("req-k1", &token, &venue.venue_id, 2, None);
    ev.payload["varieties"] = json!(["sunflower"]);
    write_candidate(&mut conn, &ev);
    let stage = marketing::decide_standing_request(&mut conn, "req-k1", "accepted")
        .unwrap()
        .stage
        .unwrap();
    assert_eq!(stage.trays_week, Some(2));
    let mut want = BTreeMap::new();
    want.insert("sunflower".to_string(), 2);
    assert_eq!(
        stage.variety_targets,
        Some(want),
        "already-standing venue is replaced"
    );
    // From passed: no extra refusal.
    let venue2 = complete_venue(&mut conn, "Gone Cafe");
    marketing::change_stage(
        &mut conn,
        &venue2.venue_id,
        "passed",
        "2026-08-11",
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let sample2 = drop_peas(&mut conn, &venue2.venue_id).unwrap();
    let token2 = sample2.token.clone().unwrap();
    write_candidate(
        &mut conn,
        &candidate_event("req-k2", &token2, &venue2.venue_id, 1, None),
    );
    let stage2 = marketing::decide_standing_request(&mut conn, "req-k2", "accepted")
        .unwrap()
        .stage
        .unwrap();
    assert_eq!(stage2.stage, "standing");
    assert_eq!(stage2.trays_week, Some(1));
}

#[test]
fn gt17l_decisions_replay_and_double_decision_fails_loudly() {
    let dir = temp_dir("gt17l");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().unwrap();
    write_candidate(
        &mut conn,
        &candidate_event("req-l1", &token, &venue.venue_id, 2, None),
    );
    write_candidate(
        &mut conn,
        &candidate_event("req-l2", &token, &venue.venue_id, 1, None),
    );
    marketing::decide_standing_request(&mut conn, "req-l1", "accepted").unwrap();
    marketing::decide_standing_request(&mut conn, "req-l2", "dismissed").unwrap();
    // A second decision for a decided row is a loud apply failure, never a rewrite.
    let dup = EventRecord::originated(
        Kind::StandingRequestDecided,
        "standing_request",
        "req-l2".to_string(),
        json!({ "requestId": "req-l2", "outcome": "accepted", "decidedAt": "2026-08-17T16:00:00Z" }),
        json!({ "op": "none" }),
        "2026-08-17T16:00:00Z".to_string(),
        None,
        None,
        Some("ev-dup".to_string()),
    );
    let tx = conn.transaction().unwrap();
    assert!(projection::apply_event(&tx, &dup).is_err());
    std::mem::drop(tx);
    assert_eq!(request_row(&conn, "req-l2").1.as_deref(), Some("dismissed"));
    event_file::flush_events(&conn, &dir).unwrap();
    std::mem::drop(conn);
    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn gt17m_dev_seed_writes_a_candidate_from_the_token_and_raises_it() {
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = drop_peas(&mut conn, &venue.venue_id).unwrap();
    let token = sample.token.clone().unwrap();
    let seeded = marketing::dev_seed_standing_request(&mut conn, &token, 2).unwrap();
    assert!(seeded.request_id.starts_with("dev-"));
    assert_eq!(seeded.contact.as_deref(), Some("dev seed"));
    assert_eq!(seeded.venue_id, venue.venue_id);
    assert_eq!(seeded.varieties, vec!["peas".to_string()]);
    assert_eq!(seeded.bags_per_cycle, 2);
    let card = open_request_card(&conn, &seeded.request_id).expect("seeded candidate raised");
    assert_eq!(
        card.message,
        "Cafe asks to put peas on standing — 2 bags per cycle. Contact given: dev seed."
    );
    assert!(
        marketing::dev_seed_standing_request(&mut conn, "0000000000000000ffffffffffffffff", 1)
            .is_err()
    );
}

#[test]
fn gt18a_qr_link_composes_from_endpoint_sample_and_crops_and_never_carries_sample_id() {
    let mut conn = mem();
    let venue = complete_venue(&mut conn, "Cafe");
    let sample = marketing::drop_sample(
        &mut conn,
        &venue.venue_id,
        "2026-08-11",
        vec!["Dun peas".into(), "Red arrow radish".into()],
        1,
        None,
    )
    .unwrap();
    let token = sample.token.clone().unwrap();
    let err = marketing::qr_link_for_token(&conn, &token).unwrap_err();
    assert_eq!(err, marketing::QR_LINK_NEEDS_ENDPOINT);
    scans::set_config(&conn, Some("https://scans.example/"), None).unwrap();
    let url = marketing::qr_link_for_token(&conn, &token).unwrap();
    assert_eq!(
        url,
        format!("https://scans.example/s/{token}?v=Dun%20peas&v=Red%20arrow%20radish&g=9&g=7")
    );
    assert!(
        !url.contains(&sample.sample_id),
        "raw sample id never leaves the farm"
    );
    assert!(marketing::qr_link_for_token(&conn, "0000000000000000ffffffffffffffff").is_err());
}
