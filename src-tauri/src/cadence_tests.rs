//! Wave 3 — GT-D11 stage/reviews, GT-D12 capacity gate, H2, weekly actions.

use crate::capacity_gate::{self, CapacitySight, CapacityState, COMMITTED_REMAINING_THRESHOLD};
use crate::db;
use crate::event_file;
use crate::event_partition::{marketing_kinds, EventDomain, Kind, MARKETING_KINDS};
use crate::events::{self, EventRecord};
use crate::export;
use crate::health::{severity_for, CheckInputs, Evidence, Severity};
use crate::import;
use crate::marketing::{
    self, ReviewsPayload, StagePayload, REVIEWS_PAYLOAD_FIELD_NAMES, STAGE_LADDER,
    STAGE_PAYLOAD_FIELD_NAMES, WEEKLY_ACTIONS_CAP,
};
use crate::models::CapacityRow;
use crate::projection;
use crate::reachability::ShelfPressure;
use crate::trays;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
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
    let dir = std::env::temp_dir().join(format!("groundtruth-cadence-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn g1_new_kinds_tier_all_len_marketing_kinds() {
    // Bumped for the silent-shortfall residual (wholesale.write_off).
    assert_eq!(Kind::ALL.len(), 54);
    for kind in ["stage.changed", "reviews.observed", "review.requested"] {
        let k = Kind::parse(kind).unwrap();
        assert_eq!(k.tier(), (EventDomain::Marketing, None));
        assert!(MARKETING_KINDS.contains(&kind));
    }
    assert_eq!(marketing_kinds(), MARKETING_KINDS.to_vec());
}

#[test]
fn g2_money_keys_refused_on_stage_and_reviews() {
    let mut conn = mem();
    let before = event_log_count(&conn);
    let money_keys = ["amount", "price", "cents", "value"];
    let bases = [
        (
            Kind::StageChanged,
            json!({
                "venueId": "v1",
                "stage": "talking",
                "changedOn": "2026-08-11",
            }),
        ),
        (
            Kind::ReviewsObserved,
            json!({
                "observationId": "r1",
                "observedOn": "2026-08-11",
                "count": 3,
                "source": "google_business_profile",
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
    ];
    for (kind, mut base) in bases {
        for key in money_keys {
            base.as_object_mut().unwrap().insert(key.into(), json!(1));
            let event = EventRecord::originated(
                kind,
                "venue",
                "v1".to_string(),
                base.clone(),
                json!({ "op": "none" }),
                "2026-08-11T00:00:00.000Z".to_string(),
                None,
                None,
                Some("corr".to_string()),
            );
            let tx = conn.transaction().unwrap();
            let err = events::write_event(&tx, &event).unwrap_err();
            assert!(
                err.to_lowercase().contains(key),
                "expected error naming {key}, got {err}"
            );
            drop(tx);
            base.as_object_mut().unwrap().remove(key);
        }
    }
    assert_eq!(event_log_count(&conn), before);
}

#[test]
fn g4_no_score_rating_index_or_money_names() {
    let conn = mem();
    let forbidden = [
        "%score%", "%rating%", "%index%", "%amount%", "%price%", "%cents%", "%value%",
    ];
    for table in [
        "mkt_venues",
        "mkt_samples",
        "mkt_touches",
        "mkt_followups",
        "mkt_stages",
        "mkt_reviews",
        "mkt_review_requests",
        "mkt_standing_requests",
    ] {
        for pat in forbidden {
            let n: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name LIKE '{pat}'"
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 0, "{table} has forbidden column matching {pat}");
        }
    }
    let field_lists = [
        STAGE_PAYLOAD_FIELD_NAMES,
        REVIEWS_PAYLOAD_FIELD_NAMES,
        marketing::REVIEW_REQUESTED_PAYLOAD_FIELD_NAMES,
        marketing::VENUE_PAYLOAD_FIELD_NAMES,
        marketing::VENUE_ARCHIVE_PAYLOAD_FIELD_NAMES,
        marketing::SAMPLE_PAYLOAD_FIELD_NAMES,
        marketing::TOUCH_PAYLOAD_FIELD_NAMES,
        marketing::FOLLOWUP_SET_PAYLOAD_FIELD_NAMES,
        marketing::FOLLOWUP_CLEAR_PAYLOAD_FIELD_NAMES,
    ];
    for list in field_lists {
        for name in list {
            let lower = name.to_lowercase();
            for bad in [
                "score", "rating", "index", "amount", "price", "cents", "value",
            ] {
                assert!(
                    !lower.contains(bad),
                    "payload field {name} contains forbidden {bad}"
                );
            }
        }
    }
    // FIELD_NAMES match struct shape (serde camelCase → snake in consts).
    let _ = StagePayload {
        venue_id: "v".into(),
        stage: "talking".into(),
        changed_on: "2026-08-11".into(),
        trays_week: None,
        varieties: None,
        variety_targets: None,
        note: None,
    };
    let _ = ReviewsPayload {
        observation_id: "r".into(),
        observed_on: "2026-08-11".into(),
        count: 0,
        source: "google_business_profile".into(),
    };
    assert_eq!(
        STAGE_PAYLOAD_FIELD_NAMES,
        &[
            "venue_id",
            "stage",
            "changed_on",
            "trays_week",
            "varieties",
            "variety_targets",
            "note"
        ]
    );
    assert_eq!(
        REVIEWS_PAYLOAD_FIELD_NAMES,
        &["observation_id", "observed_on", "count", "source"]
    );
}

#[test]
fn g5_pitch_and_hold_piles_retired() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "talking",
        "2026-08-01",
        None,
        None,
        None,
        None,
    )
    .unwrap();

    let room = CapacityState::Known(CapacitySight {
        rows: vec![CapacityRow {
            harvest_date: "2026-08-20".into(),
            crop_id: "dun-peas".into(),
            crop_name: "Dun peas".into(),
            trays: 20,
            expected_yield_oz: 100.0,
            sold_trays: 2,
            remaining_trays: COMMITTED_REMAINING_THRESHOLD + 5,
            harvested_trays: 0,
            cover_supply: 20,
            cover_promised: 2,
            cover_remaining: COMMITTED_REMAINING_THRESHOLD + 5,
        }],
        taken_at: "2026-08-11T12:00:00Z".into(),
        file_name: "farm-2026-08-11-074100.db".into(),
    });
    let view_room = marketing::weekly_actions_with_capacity(&conn, &room).unwrap();
    assert!(
        view_room.pitching_advice.is_none(),
        "C5: a sight with room says nothing, got {:?}",
        view_room.pitching_advice
    );
    assert!(
        !view_room
            .actions
            .iter()
            .any(|a| a.kind == "pitch" || a.kind == "hold_pitch"),
        "advice must not appear in the capped list, got {:?}",
        view_room.actions
    );

    let ceiling = CapacityState::Known(CapacitySight {
        rows: vec![CapacityRow {
            harvest_date: "2026-08-20".into(),
            crop_id: "dun-peas".into(),
            crop_name: "Dun peas".into(),
            trays: 10,
            expected_yield_oz: 80.0,
            sold_trays: 10,
            remaining_trays: 0,
            harvested_trays: 0,
            cover_supply: 10,
            cover_promised: 10,
            cover_remaining: 0,
        }],
        taken_at: "2026-08-11T12:00:00Z".into(),
        file_name: "farm-2026-08-11-074100.db".into(),
    });
    let view_hold = marketing::weekly_actions_with_capacity(&conn, &ceiling).unwrap();
    assert!(
        view_hold.pitching_advice.is_none(),
        "C5: a near-committed sight says nothing, got {:?}",
        view_hold.pitching_advice
    );
    assert!(
        !view_hold
            .actions
            .iter()
            .any(|a| a.kind == "pitch" || a.kind == "hold_pitch"),
        "advice must not appear in the capped list, got {:?}",
        view_hold.actions
    );
}

#[test]
fn g7_h2_truth_table() {
    const NOW: &str = "2026-08-11T12:00:00.000Z";
    const FRESH: &str = "2026-08-11T12:00:00.000Z";
    const FUTURE: &str = "2026-08-12T12:00:00.000Z";
    const WRITE_7D: &str = "2026-08-04T12:00:00.000Z";

    let fresh = Evidence {
        check_id: "H2".into(),
        ran_at: FRESH.into(),
        ok: true,
        detail: String::new(),
    };

    // Healthy: no active venues, silent log.
    let h = severity_for(
        "H2",
        Some(&fresh),
        NOW,
        &CheckInputs {
            active_venues: 0,
            last_marketing_write_at: None,
            ..CheckInputs::default()
        },
    );
    assert_eq!(h.severity, Severity::Healthy);

    // Degraded: 4 active venues, last write 7 days ago.
    let d = severity_for(
        "H2",
        Some(&fresh),
        NOW,
        &CheckInputs {
            active_venues: 4,
            last_marketing_write_at: Some(WRITE_7D.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(d.severity, Severity::Degraded);
    assert!(d.sentence.contains("7 days"));
    assert!(d.sentence.contains("4 active venues"));

    // Degraded: 4 active venues, log never written.
    let never = severity_for(
        "H2",
        Some(&fresh),
        NOW,
        &CheckInputs {
            active_venues: 4,
            last_marketing_write_at: None,
            ..CheckInputs::default()
        },
    );
    assert_eq!(never.severity, Severity::Degraded);

    // Unhealthy: marketing flush lag.
    let u = severity_for(
        "H2",
        Some(&fresh),
        NOW,
        &CheckInputs {
            marketing_flush_lag: 2,
            active_venues: 0,
            ..CheckInputs::default()
        },
    );
    assert_eq!(u.severity, Severity::Unhealthy);

    // Degraded: evidence absent.
    let absent = severity_for("H2", None, NOW, &CheckInputs::default());
    assert_eq!(absent.severity, Severity::Degraded);
    assert!(absent.sentence.contains("hasn't reported"));

    // Degraded: evidence ran_at in the future — never Healthy.
    let future_ev = Evidence {
        check_id: "H2".into(),
        ran_at: FUTURE.into(),
        ok: true,
        detail: String::new(),
    };
    let skew = severity_for(
        "H2",
        Some(&future_ev),
        NOW,
        &CheckInputs {
            active_venues: 0,
            ..CheckInputs::default()
        },
    );
    assert_eq!(skew.severity, Severity::Degraded);
    assert!(skew.sentence.contains("future"));
    assert_ne!(skew.severity, Severity::Healthy);
}

#[test]
fn g8_stage_ladder_and_standing_requires_trays() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    let err = marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        "2026-08-11",
        None,
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(err.contains("trays_week"), "{err}");

    let err = marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "not-a-stage",
        "2026-08-11",
        None,
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(err.contains("stage"), "{err}");

    // CHECK constraint also refuses unknown stage on raw insert after a valid row shape.
    let bad = conn.execute(
        "INSERT INTO mkt_stages (venue_id, stage, trays_week, varieties, changed_on, note, updated_at)
         VALUES (?1, 'bogus', NULL, NULL, '2026-08-11', NULL, '2026-08-11T00:00:00Z')",
        [&venue.venue_id],
    );
    assert!(bad.is_err());

    for stage in STAGE_LADDER {
        let trays = if *stage == "standing" { Some(2) } else { None };
        let varieties = if *stage == "standing" {
            Some(vec!["peas".into()])
        } else {
            None
        };
        marketing::change_stage(
            &mut conn,
            &venue.venue_id,
            stage,
            "2026-08-11",
            trays,
            varieties,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("stage {stage} refused: {e}"));
    }
}

#[test]
fn g9_weekly_actions_cap_reports_dropped() {
    let mut conn = mem();
    // Create more overdue follow-ups than the cap.
    for i in 0..(WEEKLY_ACTIONS_CAP + 3) {
        let v =
            marketing::record_venue(&mut conn, &format!("V{i}"), "cafe", None, None, None, None)
                .unwrap();
        marketing::set_followup(&mut conn, &v.venue_id, "2020-01-01", "overdue").unwrap();
    }
    let unknown =
        CapacityState::Unknown("Capacity unknown — last Farm OS snapshot not readable".into());
    let view = marketing::weekly_actions_with_capacity(&conn, &unknown).unwrap();
    assert_eq!(view.actions.len(), WEEKLY_ACTIONS_CAP);
    assert!(view.dropped >= 3, "dropped={}", view.dropped);
}

#[test]
fn g10_verify_and_export_import_stage_reviews() {
    let dir = temp_dir("g10");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    event_file::on_app_start(&conn, &dir);

    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    marketing::change_stage(
        &mut conn,
        &venue.venue_id,
        "standing",
        "2026-08-11",
        Some(3),
        Some(vec!["peas".into()]),
        None,
        None,
    )
    .unwrap();
    marketing::observe_reviews(&mut conn, "2026-08-11", 12, "google_business_profile").unwrap();
    event_file::flush_events(&conn, &dir).unwrap();

    let outcome =
        projection::verify_replay_paths(&farm, &event_file::events_path(&dir), &dir).unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify failed: {}",
        outcome.summary_line()
    );
    assert!(
        !projection::EXCLUSION_LIST
            .iter()
            .any(|e| e.contains("mkt_stages") || e.contains("mkt_reviews")),
        "mkt_stages/reviews must not be excluded"
    );

    let bundle = export::export_bundle(&conn, &dir).unwrap();
    let bundle_path = PathBuf::from(&bundle.bundle_path);

    let target = temp_dir("g10-tgt");
    let target_farm = target.join("farm.db");
    let mut tgt = db::open_and_migrate(&target_farm).unwrap();
    event_file::on_app_start(&tgt, &target);
    // Marketing-only import through the ordinary door (F8 / Wave 2). The
    // freeze that once gated farm truth here is retired (identity::CUTOVER_LIVE).
    import::apply_import(&mut tgt, &bundle_path).unwrap();
    let stages: i64 = tgt
        .query_row("SELECT COUNT(*) FROM mkt_stages", [], |r| r.get(0))
        .unwrap();
    let reviews: i64 = tgt
        .query_row("SELECT COUNT(*) FROM mkt_reviews", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stages, 1);
    assert_eq!(reviews, 1);

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&target);
}

#[test]
fn g11_h2_uses_the_shared_engine() {
    const NOW: &str = "2026-08-11T12:00:00.000Z";
    let fresh = Evidence {
        check_id: "H2".into(),
        ran_at: NOW.into(),
        ok: true,
        detail: String::new(),
    };
    let healthy = severity_for(
        "H2",
        Some(&fresh),
        NOW,
        &CheckInputs {
            active_venues: 0,
            ..CheckInputs::default()
        },
    );
    assert_eq!(healthy.check_id, "H2");
    assert_eq!(healthy.severity, Severity::Healthy);

    let unhealthy = severity_for(
        "H2",
        Some(&fresh),
        NOW,
        &CheckInputs {
            marketing_flush_lag: 1,
            ..CheckInputs::default()
        },
    );
    assert_eq!(unhealthy.check_id, "H2");
    assert_eq!(unhealthy.severity, Severity::Unhealthy);
    // Not the unknown-id catch-all sentence.
    assert!(!unhealthy.sentence.contains("hasn't reported since never"));
}

#[test]
fn g12_no_parallel_severity_path() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/health.rs");
    let src = std::fs::read_to_string(&path).expect("read health.rs");
    assert!(
        !src.contains("H2Inputs"),
        "H2Inputs must not survive — use CheckInputs facts only"
    );
    let def_count = src.matches("fn severity_h2").count();
    assert_eq!(
        def_count, 1,
        "expected exactly one fn severity_h2, found {def_count}"
    );
}

#[test]
fn phase2cap_live_sight_reads_own_database() {
    let mut conn = mem();
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    match capacity_gate::capacity_sight_live(&conn) {
        CapacityState::Known(sight) => {
            assert!(!sight.rows.is_empty());
            assert_eq!(sight.file_name, "live farm database");
            chrono::DateTime::parse_from_rfc3339(&sight.taken_at)
                .expect("taken_at must parse as RFC3339");
        }
        CapacityState::Unknown(reason) => panic!("expected Known, got Unknown: {reason}"),
    }
}

#[test]
fn phase2cap_capacity_sight_names_live_origin() {
    let mut conn = mem();
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    let observed =
        crate::capacity_gate::capacity_observed(&conn).expect("live capacity must be Known");
    assert_eq!(
        observed.origin, "live farm database",
        "origin is the one field the capacity sentence is built from; \
         the sentence itself now lives in Marketing.tsx and has no cargo pin"
    );
    chrono::DateTime::parse_from_rfc3339(&observed.fetched_at)
        .expect("fetched_at must parse as RFC3339");
}

#[test]
fn phase2cap_advice_is_never_in_the_capped_list() {
    let mut conn = mem();
    let venue = marketing::record_venue(&mut conn, "Cafe", "cafe", None, None, None, None).unwrap();
    for i in 0..13 {
        marketing::set_followup(
            &mut conn,
            &venue.venue_id,
            "2020-01-01",
            &format!("overdue {i}"),
        )
        .unwrap();
    }
    marketing::raise_overdue_followups(&conn).unwrap();
    let room = CapacityState::Known(CapacitySight {
        rows: vec![CapacityRow {
            harvest_date: "2026-08-20".into(),
            crop_id: "dun-peas".into(),
            crop_name: "Dun peas".into(),
            trays: 20,
            expected_yield_oz: 100.0,
            sold_trays: 2,
            remaining_trays: COMMITTED_REMAINING_THRESHOLD + 5,
            harvested_trays: 0,
            cover_supply: 20,
            cover_promised: 2,
            cover_remaining: COMMITTED_REMAINING_THRESHOLD + 5,
        }],
        taken_at: "2026-08-12T12:00:00Z".into(),
        file_name: capacity_gate::LIVE_ORIGIN.to_string(),
    });
    let view = marketing::weekly_actions_with_capacity(&conn, &room).unwrap();
    assert!(view.actions.len() <= 12);
    assert!(view.dropped >= 1, "dropped={}", view.dropped);
    assert!(view.pitching_advice.is_none());
    assert!(!view
        .actions
        .iter()
        .any(|a| a.kind == "pitch" || a.kind == "hold_pitch"));
}

#[test]
fn phase2cap_near_committed_no_longer_speaks_for_pitching() {
    let conn = mem();
    let ceiling = CapacitySight {
        rows: vec![CapacityRow {
            harvest_date: "2026-08-20".into(),
            crop_id: "dun-peas".into(),
            crop_name: "Dun peas".into(),
            trays: 10,
            expected_yield_oz: 80.0,
            sold_trays: 6,
            remaining_trays: 4,
            harvested_trays: 0,
            cover_supply: 10,
            cover_promised: 6,
            cover_remaining: 4,
        }],
        taken_at: "2026-08-12T12:00:00Z".into(),
        file_name: capacity_gate::LIVE_ORIGIN.to_string(),
    };
    // The pile still measures itself.
    assert_eq!(ceiling.remaining_trays_total(), 4);
    assert!(ceiling.near_committed());
    // C5 (INT-006): and it no longer reaches the operator.
    let known = CapacityState::Known(ceiling);
    let view = marketing::weekly_actions_with_capacity(&conn, &known).unwrap();
    assert!(view.pitching_advice.is_none());
    assert!(!view
        .actions
        .iter()
        .any(|a| a.kind == "pitch" || a.kind == "hold_pitch"));
}

#[test]
fn phase2cap_unknown_offers_no_advice() {
    let conn = mem();
    let unknown =
        CapacityState::Unknown("Capacity unknown — live farm database query failed (x)".into());
    let quiet = ShelfPressure {
        dates: 0,
        trays: 0,
        firing: false,
    };
    assert!(crate::capacity_gate::pitching_advice(&unknown, &quiet).is_none());
    let view = marketing::weekly_actions_with_capacity(&conn, &unknown).unwrap();
    assert!(view.pitching_advice.is_none());
}

#[test]
fn c5_pitching_advice_is_shelf_pressure_only() {
    let firing = ShelfPressure {
        dates: 3,
        trays: 6,
        firing: true,
    };
    let quiet = ShelfPressure {
        dates: 0,
        trays: 0,
        firing: false,
    };
    let room = CapacityState::Known(CapacitySight {
        rows: vec![CapacityRow {
            harvest_date: "2026-08-20".into(),
            crop_id: "dun-peas".into(),
            crop_name: "Dun peas".into(),
            trays: 20,
            expected_yield_oz: 100.0,
            sold_trays: 2,
            remaining_trays: COMMITTED_REMAINING_THRESHOLD + 5,
            harvested_trays: 0,
            cover_supply: 20,
            cover_promised: 2,
            cover_remaining: COMMITTED_REMAINING_THRESHOLD + 5,
        }],
        taken_at: "2026-08-12T12:00:00Z".into(),
        file_name: capacity_gate::LIVE_ORIGIN.to_string(),
    });
    let unknown =
        CapacityState::Unknown("Capacity unknown — live farm database query failed (x)".into());
    // Firing shelf pressure is the only thing that speaks, whatever the sight says.
    assert_eq!(
        capacity_gate::pitching_advice(&room, &firing),
        Some("Hold pitching - you are out of space, not out of calendar.")
    );
    assert_eq!(
        capacity_gate::pitching_advice(&unknown, &firing),
        Some("Hold pitching - you are out of space, not out of calendar.")
    );
    // A quiet shelf says nothing, however much room the pile reports.
    assert!(capacity_gate::pitching_advice(&room, &quiet).is_none());
}

/// Strips `//` line comments and `/* */` block comments from TypeScript / TSX
/// source, leaving string and template literals intact. Newlines are preserved so
/// the result still lines up with the file.
///
/// Board ruling 2026-08-21: g13 forbids hardcoded check text that can reach the
/// operator; a documentation comment citing a check id is outside that scope, so the
/// scan runs on what survives this strip. Only the line and block states remove
/// characters - string state always copies through - so a misread quote can make the
/// scan stricter, never blind.
fn strip_ts_comments(src: &str) -> String {
    #[derive(Clone, Copy)]
    enum S {
        Code,
        Line,
        Block,
        Str(char),
    }
    let mut out = String::with_capacity(src.len());
    let mut state = S::Code;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        match state {
            S::Code => match c {
                '/' if next == Some('/') => {
                    chars.next();
                    state = S::Line;
                }
                '/' if next == Some('*') => {
                    chars.next();
                    state = S::Block;
                }
                '"' | '\'' | '`' => {
                    out.push(c);
                    state = S::Str(c);
                }
                _ => out.push(c),
            },
            S::Line => {
                if c == '\n' {
                    out.push(c);
                    state = S::Code;
                }
            }
            S::Block => {
                if c == '\n' {
                    out.push(c);
                }
                if c == '*' && next == Some('/') {
                    chars.next();
                    state = S::Code;
                }
            }
            S::Str(q) => {
                out.push(c);
                if c == '\\' {
                    if let Some(escaped) = chars.next() {
                        out.push(escaped);
                    }
                } else if c == q {
                    state = S::Code;
                }
            }
        }
    }
    out
}
#[test]
fn g13_health_page_has_no_hardcoded_check_text() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/screens/Health.tsx");
    let src = std::fs::read_to_string(&path).expect("read Health.tsx");
    let rendered = strip_ts_comments(&src);
    // Blindness guards. A strip that ate the file would make the scan below pass
    // while proving nothing, so the scan is trusted only if the file survived it.
    assert!(
        rendered.contains("</main>"),
        "comment strip destroyed Health.tsx structure - the scan would be blind"
    );
    let kept_pct = rendered.len() * 100 / src.len().max(1);
    assert!(
        kept_pct >= 60,
        "comment strip kept only {kept_pct}% of Health.tsx - the scan would be blind"
    );
    for forbidden in ["H1", "H2", "H3", "H4", "not yet reporting", "Capture paths"] {
        assert!(
            !rendered.contains(forbidden),
            "Health.tsx must not hardcode {forbidden:?} outside a comment"
        );
    }
}
#[test]
fn g13_comment_strip_hides_documentation_not_rendered_text() {
    // Documentation forms disappear.
    let documented = "const a = 1; // H1 not reported per ruling 6.1\n/* H2 */\n{/* H3 */}\n";
    let stripped = strip_ts_comments(documented);
    assert!(!stripped.contains("H1"), "line comment must be stripped");
    assert!(!stripped.contains("H2"), "block comment must be stripped");
    assert!(!stripped.contains("H3"), "jsx comment must be stripped");
    // Anything that can reach the operator survives, so the scan still catches it.
    let shown = "<p>H4</p>\n<span title=\"not yet reporting\">x</span>\n<b>Capture paths</b>\n";
    let kept = strip_ts_comments(shown);
    assert!(kept.contains("H4"), "jsx text must survive the strip");
    assert!(
        kept.contains("not yet reporting"),
        "attribute string must survive the strip"
    );
    assert!(
        kept.contains("Capture paths"),
        "jsx text must survive the strip"
    );
    // A // inside a string literal is not a comment.
    assert_eq!(
        strip_ts_comments("const u = \"https://x//y\"; // gone\n"),
        "const u = \"https://x//y\"; \n"
    );
}
