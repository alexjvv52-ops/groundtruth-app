//! Rack-side fence 1 (GT-D20): proposal ledger + Confirm gate.
use crate::attention;
use crate::db;
use crate::event_file;
use crate::event_partition::{grow_kinds, EventDomain, GROW_KINDS};
use crate::events::{self, EventRecord, Kind};
use crate::identity::is_farm_truth;
use crate::phone::{
    self, AcceptedCapture, GateVerdict, IngestOutcome, PhoneProposalInput, PHONE_PROPOSALS_COLUMNS,
    PHONE_PROPOSAL_ATTENTION_KIND, PHONE_PROPOSAL_DECIDED_PAYLOAD_FIELD_NAMES,
    PHONE_PROPOSED_PAYLOAD_FIELD_NAMES,
};
use crate::projection::{self, EXCLUSION_LIST};
use crate::reachability::format_mon_d_local;
use crate::trays;
use chrono::{Duration, Local, SecondsFormat, Utc};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}
fn today() -> String {
    db::local_date_today()
}
fn now_utc() -> String {
    db::utc_now_rfc3339()
}
fn captured(days_ago: i64) -> String {
    (Utc::now() - Duration::days(days_ago)).to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn local_day(rfc: &str) -> String {
    db::local_date_from_utc_rfc3339(rfc).unwrap()
}
fn clock(rfc: &str) -> String {
    attention::format_clock(
        chrono::DateTime::parse_from_rfc3339(rfc)
            .unwrap()
            .with_timezone(&Local),
    )
}
fn input(id: &str, verb: &str, qty: i64, oz: Option<f64>, at: &str) -> PhoneProposalInput {
    PhoneProposalInput {
        proposal_id: id.into(),
        device_id: "dev-seed".into(),
        verb: verb.into(),
        crop_id: "kale".into(),
        quantity: qty,
        actual_yield_oz: oz,
        phone_captured_at: at.into(),
        note: None,
    }
}
/// One batch of `qty` kale sown `sown_back` days ago, still under cover.
fn covered(conn: &mut Connection, qty: i64, sown_back: i64) -> String {
    let t = trays::sow_tray(conn, "kale", qty).unwrap();
    if sown_back > 0 {
        trays::dev_backdate_tray(conn, &t.id, sown_back).unwrap();
    }
    t.id
}
/// One batch of `qty` kale sown `sown_back` days ago, in light for `light_back` days.
fn lit(conn: &mut Connection, qty: i64, sown_back: i64, light_back: i64) -> String {
    let id = covered(conn, qty, sown_back - light_back);
    trays::advance_trays(conn, std::slice::from_ref(&id)).unwrap();
    if light_back > 0 {
        trays::dev_backdate_tray(conn, &id, light_back).unwrap();
    }
    id
}
fn ingest(conn: &mut Connection, i: &PhoneProposalInput) {
    assert_eq!(
        phone::ingest_phone_proposal(conn, i).unwrap(),
        IngestOutcome::Written
    );
}
fn confirm(conn: &mut Connection, id: &str, qty: i64, oz: Option<f64>) -> phone::ConfirmResult {
    phone::confirm_phone_captures(
        conn,
        &[AcceptedCapture {
            proposal_id: id.into(),
            quantity: qty,
            actual_yield_oz: oz,
        }],
    )
    .unwrap()
}
fn row(conn: &Connection, id: &str) -> phone::ProposalRow {
    phone::get_proposal(conn, id).unwrap()
}
fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}
fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-pf-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn pf1_kinds_closed_set_grow_not_farm_truth_table_compared_payloads_sealed() {
    assert_eq!(Kind::ALL.len(), 54);
    for (s, k) in [
        ("phone.proposed", Kind::PhoneProposed),
        ("phone.proposal_decided", Kind::PhoneProposalDecided),
    ] {
        assert_eq!(Kind::parse(s).unwrap(), k);
        assert_eq!(k.as_str(), s);
        assert_eq!(k.tier(), (EventDomain::Grow, None));
        assert!(GROW_KINDS.contains(&s) && grow_kinds().contains(&s));
        assert!(!is_farm_truth(k), "a proposal is not farm truth (GT-D2)");
    }
    assert_eq!(
        PHONE_PROPOSED_PAYLOAD_FIELD_NAMES,
        &[
            "proposal_id",
            "device_id",
            "verb",
            "crop_id",
            "quantity",
            "actual_yield_oz",
            "phone_captured_at",
            "note"
        ]
    );
    assert_eq!(
        PHONE_PROPOSAL_DECIDED_PAYLOAD_FIELD_NAMES,
        &[
            "proposal_id",
            "outcome",
            "decided_at",
            "accepted_quantity",
            "accepted_yield_oz",
            "applied_event_ids",
            "gate_reason"
        ]
    );
    let mut conn = mem();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='phone_proposals'"
        ),
        1
    );
    assert!(
        EXCLUSION_LIST
            .iter()
            .all(|l| !l.contains("phone_proposals")),
        "compared, not excluded"
    );
    assert_eq!(PHONE_PROPOSALS_COLUMNS.len(), 15);
    // Sealed at the choke point: each refused, nothing written.
    let before = count(&conn, "SELECT COUNT(*) FROM event_log");
    let good = json!({"proposalId":"p1","deviceId":"d","verb":"move_to_light","cropId":"kale","quantity":2,"phoneCapturedAt":captured(0)});
    let bad = [
        json!({"proposalId":"p1","deviceId":"d","verb":"move_to_light","cropId":"kale","quantity":2,"phoneCapturedAt":captured(0),"extra":1}),
        json!({"proposalId":"p1","deviceId":"d","verb":"sow","cropId":"kale","quantity":2,"phoneCapturedAt":captured(0)}),
        json!({"proposalId":"p1","deviceId":"d","verb":"move_to_light","cropId":"kale","quantity":0,"phoneCapturedAt":captured(0)}),
        json!({"proposalId":"p1","deviceId":"d","verb":"harvest","cropId":"kale","quantity":2,"phoneCapturedAt":captured(0)}),
        json!({"proposalId":"p1","deviceId":"d","verb":"move_to_light","cropId":"kale","quantity":2,"actualYieldOz":3.0,"phoneCapturedAt":captured(0)}),
        json!({"proposalId":"p1","deviceId":"d","verb":"move_to_light","cropId":"kale","quantity":2,"phoneCapturedAt":"yesterday"}),
    ];
    for p in bad {
        let ev = EventRecord::originated(
            Kind::PhoneProposed,
            "phone_proposal",
            "p1".to_string(),
            p,
            json!({"op":"none"}),
            now_utc(),
            None,
            None,
            None,
        );
        let tx = conn.transaction().unwrap();
        assert!(events::write_event(&tx, &ev).is_err());
        drop(tx);
    }
    let decided_bad = [
        json!({"proposalId":"p1","outcome":"maybe","decidedAt":now_utc(),"appliedEventIds":[]}),
        json!({"proposalId":"p1","outcome":"accepted","decidedAt":now_utc(),"acceptedQuantity":2,"appliedEventIds":[]}),
        json!({"proposalId":"p1","outcome":"discarded","decidedAt":now_utc(),"appliedEventIds":[],"gateReason":"because"}),
        json!({"proposalId":"p1","outcome":"accepted","decidedAt":now_utc(),"acceptedQuantity":2,"appliedEventIds":["e"],"gateReason":"batch_mismatch"}),
    ];
    for p in decided_bad {
        let ev = EventRecord::originated(
            Kind::PhoneProposalDecided,
            "phone_proposal",
            "p1".to_string(),
            p,
            json!({"op":"none"}),
            now_utc(),
            None,
            None,
            None,
        );
        let tx = conn.transaction().unwrap();
        assert!(events::write_event(&tx, &ev).is_err());
        drop(tx);
    }
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM event_log"), before);
    let ev = EventRecord::originated(
        Kind::PhoneProposed,
        "phone_proposal",
        "p1".to_string(),
        good,
        json!({"op":"none"}),
        now_utc(),
        None,
        None,
        None,
    );
    let tx = conn.transaction().unwrap();
    projection::apply_event(&tx, &ev).unwrap();
    events::write_event(&tx, &ev).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM phone_proposals WHERE decided_at IS NULL"
        ),
        1
    );
}

#[test]
fn pf2_ingest_written_known_refused_nothing_invented() {
    let mut conn = mem();
    let i = input("p1", "move_to_light", 2, None, &captured(0));
    assert_eq!(
        phone::ingest_phone_proposal(&mut conn, &i).unwrap(),
        IngestOutcome::Written
    );
    assert_eq!(
        phone::ingest_phone_proposal(&mut conn, &i).unwrap(),
        IngestOutcome::Known
    );
    let mut unknown = input("p2", "move_to_light", 2, None, &captured(0));
    unknown.crop_id = "no-such-crop".into();
    assert_eq!(
        phone::ingest_phone_proposal(&mut conn, &unknown).unwrap(),
        IngestOutcome::Refused("unknown_crop")
    );
    for bad in [
        input("p3", "sow", 2, None, &captured(0)),
        input("p4", "move_to_light", 0, None, &captured(0)),
        input("p5", "harvest", 2, None, &captured(0)),
        input("p6", "harvest", 2, Some(0.0), &captured(0)),
        input("p7", "move_to_light", 2, None, "not-a-time"),
        input("", "move_to_light", 2, None, &captured(0)),
    ] {
        assert_eq!(
            phone::ingest_phone_proposal(&mut conn, &bad).unwrap(),
            IngestOutcome::Refused("invalid")
        );
    }
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM phone_proposals"), 1);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM trays"),
        0,
        "a candidate never touches trays"
    );
}

#[test]
fn pf3_attention_raised_refreshed_reraised_after_restore_and_decide_only() {
    let mut conn = mem();
    let at = captured(0);
    let mut i = input("p1", "harvest", 2, Some(12.0), &at);
    i.note = Some("  back rack  ".into());
    ingest(&mut conn, &i);
    let items = attention::check_attention(&conn).unwrap();
    let it = items
        .iter()
        .find(|a| a.kind == PHONE_PROPOSAL_ATTENTION_KIND)
        .expect("raised");
    let age = phone::capture_age_label(&at, &now_utc()).unwrap();
    assert_eq!(
        it.message,
        format!("Harvest 2 trays of Kale, 12.0 oz — captured {age}. Note: back rack.")
    );
    assert_eq!(
        it.actions,
        vec!["confirm".to_string(), "discard".to_string()]
    );
    assert_eq!(
        attention::dismiss_attention(&mut conn, &it.id).unwrap_err(),
        phone::PHONE_CAPTURE_DECIDE_ONLY
    );
    assert_eq!(
        attention::resolve_attention(&mut conn, &it.id, "confirm").unwrap_err(),
        phone::PHONE_CAPTURE_DECIDE_ONLY
    );
    conn.execute(
        "UPDATE attention SET message = 'stale' WHERE id = ?1",
        [&it.id],
    )
    .unwrap();
    let again = attention::check_attention(&conn).unwrap();
    assert!(
        again
            .iter()
            .any(|a| a.id == it.id && a.message.starts_with("Harvest 2 trays of Kale")),
        "age never freezes"
    );
    conn.execute("DELETE FROM attention", []).unwrap(); // a restore empties attention
    let after = attention::check_attention(&conn).unwrap();
    assert_eq!(
        after
            .iter()
            .filter(|a| a.kind == PHONE_PROPOSAL_ATTENTION_KIND)
            .count(),
        1,
        "re-raised from the durable home"
    );
    let view = phone::phone_captures(&conn).unwrap();
    assert_eq!(view.len(), 1);
    assert_eq!(
        view[0].message,
        format!("Harvest 2 trays of Kale, 12.0 oz — captured {age}. Note: back rack.")
    );
}

#[test]
fn pf4_capture_age_bytes() {
    let n = now_utc();
    for (days, form) in [(0, "today at "), (1, "yesterday at ")] {
        let at = captured(days);
        assert_eq!(
            phone::capture_age_label(&at, &n).unwrap(),
            format!("{form}{}", clock(&at))
        );
    }
    let at = captured(3);
    assert_eq!(
        phone::capture_age_label(&at, &n).unwrap(),
        format!(
            "{} at {}",
            format_mon_d_local(&local_day(&at)).unwrap(),
            clock(&at)
        )
    );
}

#[test]
fn pf5_gate_move_blocks_each_reason_with_signed_bytes() {
    let mut conn = mem();
    let t = today();
    let name = "Kale";
    // no trays under cover → zero form
    ingest(
        &mut conn,
        &input("z", "move_to_light", 3, None, &captured(0)),
    );
    let r = confirm(&mut conn, "z", 3, None);
    assert_eq!(r.blocked[0].reason, "not_enough_blackout");
    assert_eq!(
        r.blocked[0].sentence,
        format!(
            "No trays of {name} are under cover on the PC; this capture names 3. Nothing written."
        )
    );
    // 2 under cover, capture names 3
    covered(&mut conn, 2, 3);
    let r = confirm(&mut conn, "z", 3, None);
    assert_eq!(r.blocked[0].sentence, format!("Only 2 trays of {name} are under cover on the PC; this capture names 3. Nothing written."));
    // future
    ingest(
        &mut conn,
        &input("f", "move_to_light", 2, None, &captured(-1)),
    );
    let r = confirm(&mut conn, "f", 2, None);
    assert_eq!(r.blocked[0].reason, "capture_in_future");
    assert_eq!(
        r.blocked[0].sentence,
        format!(
            "This capture is dated {}, after today. Nothing written.",
            format_mon_d_local(&local_day(&captured(-1))).unwrap()
        )
    );
    // before sow: batch sown today, capture yesterday
    let mut c2 = mem();
    covered(&mut c2, 2, 0);
    let y = captured(1);
    ingest(&mut c2, &input("b", "move_to_light", 2, None, &y));
    let r = confirm(&mut c2, "b", 2, None);
    assert_eq!(r.blocked[0].reason, "capture_before_sow");
    assert_eq!(r.blocked[0].sentence, format!("This capture is dated {}, before those trays of {name} were sown on the PC. Nothing written.", format_mon_d_local(&local_day(&y)).unwrap()));
    assert_eq!(
        count(&c2, "SELECT COUNT(*) FROM trays WHERE state='light'"),
        0,
        "nothing written"
    );
    let _ = t;
}

#[test]
fn pf6_gate_harvest_blocks_each_reason_with_signed_bytes() {
    let name = "Kale";
    // weight first
    let mut conn = mem();
    lit(&mut conn, 2, 5, 2);
    ingest(
        &mut conn,
        &input("w", "harvest", 2, Some(5.0), &captured(0)),
    );
    let r = confirm(&mut conn, "w", 2, Some(0.0));
    assert_eq!(r.blocked[0].reason, "weight_not_positive");
    assert_eq!(
        r.blocked[0].sentence,
        "Harvest weight must be more than 0 oz. Nothing written."
    );
    // not enough in light
    let r = confirm(&mut conn, "w", 3, Some(5.0));
    assert_eq!(r.blocked[0].reason, "not_enough_light");
    assert_eq!(
        r.blocked[0].sentence,
        format!(
            "Only 2 trays of {name} are in light on the PC; this capture names 3. Nothing written."
        )
    );
    // same-day duplicate: another batch harvested today on the PC
    let other = lit(&mut conn, 2, 5, 2);
    trays::harvest_trays(&mut conn, &[other], 6.0).unwrap();
    let r = confirm(&mut conn, "w", 2, Some(5.0));
    assert_eq!(r.blocked[0].reason, "same_day_harvest_exists");
    assert_eq!(r.blocked[0].sentence, format!("The PC already recorded a harvest of {name} on {} — this capture may be a duplicate. Nothing written.", format_mon_d_local(&today()).unwrap()));
    // before light: in light since today, capture yesterday
    let mut c2 = mem();
    lit(&mut c2, 2, 3, 0);
    let y = captured(1);
    ingest(&mut c2, &input("l", "harvest", 2, Some(5.0), &y));
    let r = confirm(&mut c2, "l", 2, Some(5.0));
    assert_eq!(r.blocked[0].reason, "capture_before_light");
    assert_eq!(r.blocked[0].sentence, format!("This capture is dated {}, before those trays of {name} went into light on the PC. Nothing written.", format_mon_d_local(&local_day(&y)).unwrap()));
    assert_eq!(
        count(&c2, "SELECT COUNT(*) FROM trays WHERE state='harvested'"),
        0
    );
}

#[test]
fn pf7_batch_fit_exact_prefix_only() {
    let name = "Kale";
    let mut conn = mem();
    covered(&mut conn, 2, 4); // oldest
    covered(&mut conn, 4, 3);
    ingest(
        &mut conn,
        &input("m3", "move_to_light", 3, None, &captured(0)),
    );
    let r = confirm(&mut conn, "m3", 3, None);
    assert_eq!(r.blocked[0].reason, "batch_mismatch");
    assert_eq!(r.blocked[0].sentence, format!("{name} is under cover on the PC in batches of 2 then 4; this capture names 3, and batches move whole. Nothing written."));
    let r = confirm(&mut conn, "m3", 4, None); // 4 is a batch but not a prefix total (2, 6)
    assert_eq!(r.blocked[0].reason, "batch_mismatch");
    let r = confirm(&mut conn, "m3", 2, None); // exact prefix: the oldest batch only
    assert_eq!(r.written.len(), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM trays WHERE state='light' AND quantity=2"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM trays WHERE state='blackout' AND quantity=4"
        ),
        1
    );
    // one batch, harvest variant
    let mut c2 = mem();
    lit(&mut c2, 4, 5, 2);
    ingest(&mut c2, &input("h3", "harvest", 3, Some(9.0), &captured(0)));
    let r = confirm(&mut c2, "h3", 3, Some(9.0));
    assert_eq!(r.blocked[0].sentence, format!("{name} is in light on the PC in one batch of 4; this capture names 3, and batches harvest whole. Nothing written."));
    let r = confirm(&mut c2, "h3", 4, Some(9.0));
    assert_eq!(r.written.len(), 1);
}

#[test]
fn pf8_confirm_move_lands_on_capture_day_with_provenance_and_line() {
    let mut conn = mem();
    let id = covered(&mut conn, 3, 3);
    let y = captured(1);
    ingest(&mut conn, &input("p1", "move_to_light", 3, None, &y));
    attention::check_attention(&conn).unwrap();
    let before = count(&conn, "SELECT COUNT(*) FROM event_log");
    let r = confirm(&mut conn, "p1", 3, None);
    assert!(r.blocked.is_empty());
    let age = phone::capture_age_label(&y, &now_utc()).unwrap();
    assert_eq!(
        r.written[0].line,
        format!("Recorded from phone capture — 3 trays of Kale to light (captured {age}).")
    );
    let t = trays::get_tray(&conn, &id).unwrap();
    assert_eq!(t.state, "light");
    assert_eq!(
        t.light_on.as_deref(),
        Some(local_day(&y).as_str()),
        "lands on the capture day, not the Confirm day"
    );
    let applied: String = conn
        .query_row(
            "SELECT id FROM event_log WHERE kind='trays.advanced' ORDER BY seq DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(r.written[0].applied_event_ids, vec![applied.clone()]);
    let (outcome, aq, ids): (String, i64, String) = conn.query_row(
        "SELECT outcome, accepted_quantity, applied_event_ids FROM phone_proposals WHERE proposal_id='p1'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((outcome.as_str(), aq), ("accepted", 3));
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&ids).unwrap(),
        vec![applied.clone()]
    );
    let payload: String = conn.query_row("SELECT payload FROM event_log WHERE kind='phone.proposal_decided' ORDER BY seq DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    assert!(
        payload.contains(&format!("\"appliedEventIds\":[\"{applied}\"]")),
        "{payload}"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM attention WHERE kind='phone.proposal' AND resolved_at IS NULL"
        ),
        0
    );
    assert!(count(&conn, "SELECT COUNT(*) FROM event_log") > before);
    assert_eq!(phone::phone_captures(&conn).unwrap().len(), 0);
}

#[test]
fn pf9_confirm_harvest_lands_on_capture_day_splits_weight_and_consumption() {
    let mut conn = mem();
    let id = lit(&mut conn, 2, 5, 2);
    let y = captured(1);
    ingest(&mut conn, &input("h1", "harvest", 2, Some(12.0), &y));
    let r = confirm(&mut conn, "h1", 2, Some(12.0));
    assert!(r.blocked.is_empty(), "{:?}", r.blocked);
    let age = phone::capture_age_label(&y, &now_utc()).unwrap();
    assert_eq!(
        r.written[0].line,
        format!("Recorded from phone capture — 2 trays of Kale, 12.0 oz (captured {age}).")
    );
    let t = trays::get_tray(&conn, &id).unwrap();
    assert_eq!(t.state, "harvested");
    assert_eq!(t.harvested_on.as_deref(), Some(local_day(&y).as_str()));
    assert_eq!(t.actual_yield_oz, Some(12.0));
    assert_eq!(
        count(
            &conn,
            &format!("SELECT COUNT(*) FROM consumption_events WHERE occurred_at = '{y}'")
        ),
        1,
        "one fact, one day"
    );
    let (aq, aoz): (i64, f64) = conn.query_row("SELECT accepted_quantity, accepted_yield_oz FROM phone_proposals WHERE proposal_id='h1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((aq, aoz), (2, 12.0));
    let _ = crate::marketing::harvest_commitments(&conn, "kale").unwrap(); // reader still answers
}

#[test]
fn pf10_confirm_partial_clean_written_blocked_pending_sequential_gate() {
    let mut conn = mem();
    covered(&mut conn, 3, 3);
    ingest(
        &mut conn,
        &input("a", "move_to_light", 3, None, &captured(0)),
    );
    ingest(
        &mut conn,
        &input("b", "move_to_light", 3, None, &captured(0)),
    );
    let r = phone::confirm_phone_captures(
        &mut conn,
        &[
            AcceptedCapture {
                proposal_id: "a".into(),
                quantity: 3,
                actual_yield_oz: None,
            },
            AcceptedCapture {
                proposal_id: "b".into(),
                quantity: 3,
                actual_yield_oz: None,
            },
        ],
    )
    .unwrap();
    assert_eq!(r.written.len(), 1);
    assert_eq!(r.blocked.len(), 1);
    assert_eq!(r.blocked[0].proposal_id, "b");
    assert_eq!(
        r.blocked[0].sentence,
        "No trays of Kale are under cover on the PC; this capture names 3. Nothing written."
    );
    assert!(
        row(&conn, "b").decided_at.is_none(),
        "blocked stays pending"
    );
    assert_eq!(row(&conn, "a").outcome.as_deref(), Some("accepted"));
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM event_log WHERE kind='trays.advanced'"
        ),
        1
    );
}

#[test]
fn pf11_discard_records_gate_reason_closes_attention_refuses_second() {
    let mut conn = mem();
    ingest(
        &mut conn,
        &input("b", "move_to_light", 3, None, &captured(0)),
    );
    ingest(
        &mut conn,
        &input("c", "move_to_light", 2, None, &captured(0)),
    );
    covered(&mut conn, 2, 3);
    attention::check_attention(&conn).unwrap();
    let d = phone::discard_phone_capture(&mut conn, "b").unwrap();
    assert_eq!(d.gate_reason.as_deref(), Some("not_enough_blackout"));
    let r = row(&conn, "b");
    assert_eq!(
        (r.outcome.as_deref(), r.gate_reason.as_deref()),
        (Some("discarded"), Some("not_enough_blackout"))
    );
    let ids: String = conn
        .query_row(
            "SELECT applied_event_ids FROM phone_proposals WHERE proposal_id='b'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ids, "[]");
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM attention WHERE kind='phone.proposal' AND entity_id='b' AND resolved_at IS NULL"), 0);
    assert_eq!(
        phone::discard_phone_capture(&mut conn, "b").unwrap_err(),
        phone::PHONE_CAPTURE_ALREADY_DECIDED
    );
    let d2 = phone::discard_phone_capture(&mut conn, "c").unwrap();
    assert_eq!(
        d2.gate_reason, None,
        "a clean row discarded records no reason"
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM trays WHERE state='light'"),
        0
    );
}

#[test]
fn pf12_edited_values_recorded_frozen_fields_immutable_delete_proof_decision_final() {
    let mut conn = mem();
    covered(&mut conn, 2, 3);
    covered(&mut conn, 1, 2);
    ingest(
        &mut conn,
        &input("e", "move_to_light", 3, None, &captured(0)),
    );
    let r = confirm(&mut conn, "e", 2, None); // operator edited 3 → 2 (the oldest batch)
    assert_eq!(r.written.len(), 1);
    let (q, aq): (i64, i64) = conn
        .query_row(
            "SELECT quantity, accepted_quantity FROM phone_proposals WHERE proposal_id='e'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (q, aq),
        (3, 2),
        "frozen quantity kept; applied value recorded"
    );
    let e = conn
        .execute(
            "UPDATE phone_proposals SET quantity = 9 WHERE proposal_id='e'",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("immutable"), "{e}");
    let e = conn
        .execute(
            "UPDATE phone_proposals SET outcome = 'discarded' WHERE proposal_id='e'",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("final"), "{e}");
    let e = conn
        .execute("DELETE FROM phone_proposals", [])
        .unwrap_err()
        .to_string();
    assert!(e.contains("append-only"), "{e}");
    assert_eq!(
        phone::confirm_phone_captures(
            &mut conn,
            &[AcceptedCapture {
                proposal_id: "e".into(),
                quantity: 2,
                actual_yield_oz: None
            }]
        )
        .unwrap_err(),
        phone::PHONE_CAPTURE_ALREADY_DECIDED
    );
}

#[test]
fn pf13_public_paths_still_stamp_today() {
    let mut conn = mem();
    let a = trays::sow_tray(&mut conn, "kale", 1).unwrap();
    trays::advance_trays(&mut conn, std::slice::from_ref(&a.id)).unwrap();
    assert_eq!(
        trays::get_tray(&conn, &a.id).unwrap().light_on.as_deref(),
        Some(today().as_str())
    );
    trays::harvest_trays(&mut conn, std::slice::from_ref(&a.id), 4.0).unwrap();
    assert_eq!(
        trays::get_tray(&conn, &a.id)
            .unwrap()
            .harvested_on
            .as_deref(),
        Some(today().as_str())
    );
    assert!(
        trays::advance_trays(&mut conn, &[]).is_ok(),
        "empty stays a no-op"
    );
}

#[test]
fn pf14_replay_rebuilds_phone_proposals_and_verify_passes() {
    let dir = temp_dir("pf14");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    covered(&mut conn, 2, 3);
    ingest(
        &mut conn,
        &input("p1", "move_to_light", 2, None, &captured(1)),
    );
    ingest(
        &mut conn,
        &input("p2", "move_to_light", 5, None, &captured(0)),
    );
    ingest(
        &mut conn,
        &input("p3", "harvest", 1, Some(2.0), &captured(0)),
    );
    let r = confirm(&mut conn, "p1", 2, None);
    assert_eq!(r.written.len(), 1);
    phone::discard_phone_capture(&mut conn, "p2").unwrap(); // gate_reason not_enough_blackout
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);
    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pf15_v32_triggers_refuse_the_kinds_until_the_v33_reinstall() {
    const GROW_KINDS_V32: &[&str] = &[
        "tray.sown",
        "trays.advanced",
        "trays.harvested",
        "tray.discarded",
        "trays.discarded",
        "recount.applied",
        "undo",
        "dev.backdated",
        "attention.resolved",
    ];
    let conn = mem();
    db::drop_event_log_triggers(&conn).unwrap();
    conn.execute_batch(&crate::event_partition::schema_event_log_triggers_sql(
        GROW_KINDS_V32,
        &crate::event_partition::register_kinds(),
        crate::event_partition::EVENT_CLASSES,
        &crate::event_partition::marketing_kinds(),
    ))
    .unwrap();
    conn.pragma_update(None, "user_version", 32).unwrap();
    let ev = EventRecord::originated(
        Kind::PhoneProposed,
        "phone_proposal",
        "p1".to_string(),
        json!({"proposalId":"p1","deviceId":"d","verb":"move_to_light","cropId":"kale","quantity":1,"phoneCapturedAt":"2026-08-17T20:00:00.000Z"}),
        json!({"op":"none"}),
        "2026-08-17T20:00:00.000Z".to_string(),
        None,
        None,
        Some("ev-p1".to_string()),
    );
    let mut conn = conn;
    let tx = conn.transaction().unwrap();
    let err = events::write_event(&tx, &ev).unwrap_err();
    assert!(err.contains("kind invalid for grow"), "{err}");
    drop(tx);
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

#[cfg(debug_assertions)]
#[test]
fn pf16_dev_seed_states_its_origin_and_raises() {
    let mut conn = mem();
    covered(&mut conn, 2, 3);
    let v = phone::dev_seed_phone_proposal(&mut conn, "move_to_light", "kale", 2, None, 0, None)
        .unwrap();
    assert!(v.proposal_id.starts_with("dev-"));
    assert_eq!(v.device_id, "dev-seed");
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM attention WHERE kind='phone.proposal' AND resolved_at IS NULL"
        ),
        1
    );
    let f = phone::dev_seed_phone_proposal(&mut conn, "harvest", "kale", 1, Some(3.0), -1, None)
        .unwrap();
    assert!(
        matches!(phone::gate(&conn, &row(&conn, &f.proposal_id), 1, Some(3.0), "Kale", &today()).unwrap(),
        GateVerdict::Blocked(b) if b.reason == "capture_in_future")
    );
}
