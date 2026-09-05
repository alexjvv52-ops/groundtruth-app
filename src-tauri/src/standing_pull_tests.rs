//! Customer QR fence 5 — the desktop pulls standing candidates and stays the authority.

use crate::attention;
use crate::db;
use crate::event_file;
use crate::marketing::{self, IngestOutcome};
use crate::projection::{self, EXCLUSION_LIST};
use crate::scans;
use crate::standing_pull;
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const NOW: &str = "2026-08-17T20:00:00.000Z";
const TOKEN_SECRET: &str = "secret_pull_token_fixture";

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn configure(conn: &Connection) {
    scans::set_config(conn, Some("https://scans.example"), Some(TOKEN_SECRET)).unwrap();
}

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-spull-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn venue_and_token(conn: &mut Connection, name: &str) -> (marketing::VenueView, String) {
    let venue = marketing::record_venue(
        conn,
        name,
        "cafe",
        Some("Sam".into()),
        None,
        Some("1 Main St".into()),
        None,
    )
    .unwrap();
    let sample = marketing::drop_sample(
        conn,
        &venue.venue_id,
        "2026-08-11",
        vec!["Dun peas".into()],
        1,
        None,
    )
    .unwrap();
    (venue, sample.token.unwrap())
}

fn row(seq: i64, id: &str, token: &str, bags: i64) -> String {
    format!(
        r#"{{"seq":{seq},"requestId":"{id}","token":"{token}","bagsPerCycle":{bags},"requestedAt":"2026-08-17T19:00:00Z"}}"#
    )
}

fn body(rows: &[String], first: Option<i64>, max: i64) -> String {
    let first = first.map(|f| f.to_string()).unwrap_or("null".into());
    format!(
        r#"{{"rows":[{}],"firstAvailableSeq":{first},"maxSeq":{max},"servedAt":"{NOW}"}}"#,
        rows.join(",")
    )
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn cards(conn: &Connection) -> usize {
    attention::check_attention(conn)
        .unwrap()
        .iter()
        .filter(|i| i.kind == marketing::STANDING_REQUEST_ATTENTION_KIND)
        .count()
}

#[test]
fn f5d1_schema_v31_tables_exist_and_are_excluded_from_replay() {
    let conn = mem();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    for t in ["standing_pull_observations", "standing_pull_refusals"] {
        let n = count(
            &conn,
            &format!("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{t}'"),
        );
        assert_eq!(n, 1, "{t}");
        let line = EXCLUSION_LIST
            .iter()
            .find(|l| l.contains(t))
            .unwrap_or_else(|| panic!("{t} on EXCLUSION_LIST"));
        assert!(line.contains("not derived from events"), "{line}");
    }
    // Idempotent: a rewound version re-runs v31 without error.
    conn.pragma_update(None, "user_version", 30).unwrap();
    db::migrate(&conn).unwrap();
    let again: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(again, db::SCHEMA_VERSION);
}

#[test]
fn f5d2_ingest_rules_written_known_invalid_unknown() {
    let mut conn = mem();
    let (venue, token) = venue_and_token(&mut conn, "Cafe");
    assert_eq!(
        marketing::ingest_standing_request(&mut conn, "r1", &token, 2, "2026-08-17T19:00:00Z")
            .unwrap(),
        IngestOutcome::Written
    );
    assert_eq!(
        marketing::ingest_standing_request(&mut conn, "r1", &token, 9, "2026-08-17T19:00:00Z")
            .unwrap(),
        IngestOutcome::Known
    );
    assert_eq!(
        marketing::ingest_standing_request(&mut conn, "r2", "abc", 1, "2026-08-17T19:00:00Z")
            .unwrap(),
        IngestOutcome::Refused("invalid")
    );
    assert_eq!(
        marketing::ingest_standing_request(&mut conn, "r3", &token, 0, "2026-08-17T19:00:00Z")
            .unwrap(),
        IngestOutcome::Refused("invalid")
    );
    assert_eq!(
        marketing::ingest_standing_request(&mut conn, "r4", &token, 1, "yesterday").unwrap(),
        IngestOutcome::Refused("invalid")
    );
    assert_eq!(
        marketing::ingest_standing_request(
            &mut conn,
            "r5",
            "0000000000000000ffffffffffffffff",
            1,
            "2026-08-17T19:00:00Z"
        )
        .unwrap(),
        IngestOutcome::Refused("unknown_token")
    );
    let (vid, vars, bags): (String, String, i64) = conn.query_row(
        "SELECT venue_id, varieties, bags_per_cycle FROM mkt_standing_requests WHERE request_id = 'r1'",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!(vid, venue.venue_id);
    assert_eq!(vars, "[\"Dun peas\"]");
    assert_eq!(bags, 2);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM mkt_standing_requests"),
        1,
        "refusals never become candidates"
    );
}

#[test]
fn f5d3_pull_writes_known_refuses_with_trace_moves_cursor_and_raises_cards() {
    let mut conn = mem();
    configure(&conn);
    let (venue, token) = venue_and_token(&mut conn, "Blue Door Bistro");
    marketing::ingest_standing_request(&mut conn, "req-known", &token, 1, "2026-08-17T18:00:00Z")
        .unwrap();
    let page = body(
        &[
            row(1, "req-known", &token, 1),
            row(2, "req-new", &token, 3),
            row(3, "req-forged", "0000000000000000ffffffffffffffff", 1),
            row(4, "req-bad", "abc", 1),
            row(5, "req-zero", &token, 0),
        ],
        Some(1),
        5,
    );
    let view = standing_pull::pull_with(&mut conn, NOW, |url, secret| {
        assert!(url.ends_with("/standing-requests?after=0"), "{url}");
        assert_eq!(secret, TOKEN_SECRET);
        Ok((200, page.clone()))
    })
    .unwrap();
    assert_eq!(
        view.message,
        "Pulled 5 standing requests: 1 new, 1 already known, 3 refused."
    );
    assert_eq!(view.refusal_message.as_deref(),
        Some("3 refused — no sample carries that token, or the request was malformed. Nothing was invented."));
    assert!(view.last_ok_message.is_none() && view.gap_message.is_none());
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM mkt_standing_requests"),
        2
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM standing_pull_refusals WHERE reason = 'unknown_token'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM standing_pull_refusals WHERE reason = 'invalid'"
        ),
        2
    );
    assert_eq!(standing_pull::cursor(&conn).unwrap(), 5);
    let card = attention::check_attention(&conn)
        .unwrap()
        .into_iter()
        .find(|i| i.entity_id.as_deref() == Some("req-new"))
        .expect("card for the new candidate");
    assert_eq!(
        card.message,
        "Blue Door Bistro asks to put Dun peas on standing — 3 bags per cycle."
    );
    assert_eq!(cards(&conn), 2);
    let _ = venue;
    // Delete-proof trace.
    let err = conn
        .execute("DELETE FROM standing_pull_refusals", [])
        .unwrap_err()
        .to_string();
    assert!(err.contains("append-only"), "{err}");
    // Second pull asks after=5 and brings nothing.
    let view2 = standing_pull::pull_with(&mut conn, NOW, |url, _| {
        assert!(url.ends_with("after=5"), "{url}");
        Ok((200, body(&[], Some(1), 5)))
    })
    .unwrap();
    assert_eq!(
        view2.message,
        "Pulled 0 standing requests: 0 new, 0 already known, 0 refused."
    );
    assert!(view2.refusal_message.is_none());
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM mkt_standing_requests"),
        2,
        "no duplicates"
    );
    assert_eq!(cards(&conn), 2);
}

#[test]
fn f5d4_failed_pull_keeps_cursor_redacts_secret_and_reports_last_ok() {
    let mut conn = mem();
    configure(&conn);
    assert_eq!(
        standing_pull::latest_view(&conn).unwrap().message,
        standing_pull::NEVER_PULLED
    );
    let v = standing_pull::pull_with(&mut conn, NOW, |_, _| Err(format!("boom {TOKEN_SECRET}")))
        .unwrap();
    assert!(
        v.message
            .starts_with("Standing requests unavailable — the last pull failed ("),
        "{}",
        v.message
    );
    assert!(
        !v.message.contains(TOKEN_SECRET),
        "secret redacted: {}",
        v.message
    );
    assert_eq!(standing_pull::cursor(&conn).unwrap(), 0);
    let (_venue, token) = venue_and_token(&mut conn, "Cafe");
    let ok = body(&[row(1, "req-1", &token, 2)], Some(1), 1);
    standing_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, ok.clone()))).unwrap();
    assert_eq!(standing_pull::cursor(&conn).unwrap(), 1);
    let v = standing_pull::pull_with(&mut conn, NOW, |_, _| Ok((500, String::new()))).unwrap();
    assert!(v.message.starts_with("The last pull failed (standing pull HTTP 500). The counts below are from the last successful pull."), "{}", v.message);
    assert_eq!(
        v.last_ok_message.as_deref(),
        Some("Pulled 1 standing request: 1 new, 0 already known, 0 refused.")
    );
    assert_eq!(
        standing_pull::cursor(&conn).unwrap(),
        1,
        "a failed pull never moves the cursor"
    );
    let v = standing_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, "not json".into()))).unwrap();
    assert!(v.message.contains("standing pull body:"), "{}", v.message);
}

#[test]
fn f5d5_paging_until_a_short_page_and_cursor_never_skips_unhandled_rows() {
    let mut conn = mem();
    configure(&conn);
    let (_venue, token) = venue_and_token(&mut conn, "Cafe");
    let first: Vec<String> = (1..=200)
        .map(|i| row(i, &format!("r-{i}"), &token, 1))
        .collect();
    let second: Vec<String> = (201..=203)
        .map(|i| row(i, &format!("r-{i}"), &token, 1))
        .collect();
    let calls = std::cell::Cell::new(0);
    let view = standing_pull::pull_with(&mut conn, NOW, |url, _| {
        calls.set(calls.get() + 1);
        if url.ends_with("after=0") {
            Ok((200, body(&first, Some(1), 203)))
        } else if url.ends_with("after=200") {
            Ok((200, body(&second, Some(1), 203)))
        } else {
            panic!("unexpected url {url}")
        }
    })
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(
        view.message,
        "Pulled 203 standing requests: 203 new, 0 already known, 0 refused."
    );
    assert_eq!(standing_pull::cursor(&conn).unwrap(), 203);
    assert_eq!(cards(&conn), 203);
}

#[test]
fn f5d6_gap_and_restart_messages_mirror_scans() {
    let mut conn = mem();
    configure(&conn);
    let (_venue, token) = venue_and_token(&mut conn, "Cafe");
    let p1 = body(
        &[row(1, "a", &token, 1), row(2, "b", &token, 1)],
        Some(1),
        2,
    );
    standing_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, p1.clone()))).unwrap();
    let p2 = body(&[row(5, "e", &token, 1)], Some(5), 5);
    let v = standing_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, p2.clone()))).unwrap();
    assert_eq!(
        v.gap_message.as_deref(),
        Some("2 standing requests are missing from the endpoint and can never be pulled.")
    );
    let p3 = body(&[], Some(1), 1);
    let v = standing_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, p3.clone()))).unwrap();
    assert_eq!(v.gap_message.as_deref(),
        Some("The endpoint's log restarted — ids now run below ids already pulled. These counts are not comparable to earlier ones."));
}

#[test]
fn f5d7_unconfigured_view_and_pull_refusals_use_the_scans_sentences() {
    let mut conn = mem();
    assert_eq!(
        standing_pull::latest_view(&conn).unwrap().message,
        standing_pull::UNCONFIGURED
    );
    let err = standing_pull::pull(&mut conn, NOW).unwrap_err();
    assert_eq!(
        err,
        "Configure a scan endpoint URL and pull token before pulling."
    );
    scans::set_config(&conn, Some("https://scans.example"), None).unwrap();
    assert_eq!(
        standing_pull::pull(&mut conn, NOW).unwrap_err(),
        "Configure a pull token before pulling."
    );
}

#[test]
fn f5d8_pulled_candidates_replay_and_the_pull_log_is_excluded() {
    let dir = temp_dir("f5d8");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    configure(&conn);
    let (_venue, token) = venue_and_token(&mut conn, "Cafe");
    let page = body(
        &[
            row(1, "req-1", &token, 2),
            row(2, "req-x", "0000000000000000ffffffffffffffff", 1),
        ],
        Some(1),
        2,
    );
    standing_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, page.clone()))).unwrap();
    marketing::decide_standing_request(&mut conn, "req-1", "accepted").unwrap();
    event_file::flush_events(&conn, &dir).unwrap();
    std::mem::drop(conn);
    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    let _ = fs::remove_dir_all(&dir);
}
