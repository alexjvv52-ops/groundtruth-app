//! Phase 8 desktop — the app counts the scans (GT-D16).

use crate::db;
use crate::projection::EXCLUSION_LIST;
use crate::scans;
use rusqlite::Connection;
use serde_json::Value;
use std::fs;

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn configure(conn: &Connection) {
    scans::set_config(
        conn,
        Some("https://example.com"),
        Some("secret_token_fixture"),
    )
    .unwrap();
}

fn ok_body(new_count: i64, first: i64, max: i64, served: &str) -> String {
    format!(
        r#"{{"newCount":{new_count},"firstAvailableId":{first},"maxScanId":{max},"servedAt":"{served}"}}"#
    )
}

#[test]
fn p8d1_schema_is_current_and_scan_tables_exist() {
    let conn = mem();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, db::SCHEMA_VERSION);

    let tables: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name IN ('scan_config', 'scan_observations')
                 ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(
        tables,
        vec!["scan_config".to_string(), "scan_observations".to_string()]
    );

    let (id, n): (i64, i64) = conn
        .query_row("SELECT id, COUNT(*) FROM scan_config", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(id, 1);
    assert_eq!(n, 1);
}

#[test]
fn p8d2_both_new_lines_are_on_exclusion_list() {
    let config = EXCLUSION_LIST
        .iter()
        .find(|l| l.contains("scan_config"))
        .expect("scan_config must be on EXCLUSION_LIST");
    assert!(config.contains("neither copied nor compared"), "{config}");
    assert!(config.contains("No apply_* writes it"), "{config}");

    let observations = EXCLUSION_LIST
        .iter()
        .find(|l| l.contains("scan_observations"))
        .expect("scan_observations must be on EXCLUSION_LIST");
    assert!(
        observations.contains("not derived from events"),
        "{observations}"
    );
}

#[test]
fn p8d3_unconfigured_pull_names_what_is_missing_and_writes_no_row() {
    let conn = mem();
    let err = scans::pull(&conn, "2026-08-15T18:00:00Z").unwrap_err();
    assert!(err.contains("scan endpoint URL"), "{err}");
    assert!(err.contains("pull token"), "{err}");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM scan_observations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn p8d4_set_config_refuses_empty_or_non_https_url() {
    let conn = mem();
    let empty = scans::set_config(&conn, Some(""), Some("token")).unwrap_err();
    assert!(empty.contains("https://"), "{empty}");
    let http = scans::set_config(&conn, Some("http://example.com"), Some("token")).unwrap_err();
    assert!(http.contains("https://"), "{http}");
    let view = scans::config(&conn).unwrap();
    assert_eq!(view.endpoint_url, None);
    assert!(!view.token_set);
}

#[test]
fn p8d5_set_config_stores_without_returning_the_token() {
    let conn = mem();
    let view = scans::set_config(
        &conn,
        Some("https://example.com"),
        Some("secret_token_fixture"),
    )
    .unwrap();
    assert_eq!(view.endpoint_url.as_deref(), Some("https://example.com"));
    assert!(view.token_set);
    assert!(view.configured_at.is_some());

    let json = serde_json::to_value(&view).unwrap();
    let obj = json.as_object().unwrap();
    assert!(obj.contains_key("tokenSet"));
    assert!(!obj.contains_key("pullToken"));
    assert!(!obj.contains_key("pull_token"));
    assert!(!obj.contains_key("token"));
    for (k, v) in obj {
        if let Value::String(s) = v {
            assert!(
                !s.contains("secret_token_fixture"),
                "ScanConfigView field {k} carried the token"
            );
        }
    }
}

#[test]
fn p8d6_successful_pull_with_records_ok_and_ids() {
    let conn = mem();
    configure(&conn);
    let served = "2026-08-15T18:00:00.000Z";
    let body = ok_body(3, 1, 5, served);
    scans::pull_with(&conn, "2026-08-15T18:01:00Z", |_, _| {
        Ok((200, body.clone()))
    })
    .unwrap();
    let (ok, served_at, new_count, first, max): (i64, String, i64, i64, i64) = conn
        .query_row(
            "SELECT ok, served_at, new_count, first_available_id, max_scan_id
             FROM scan_observations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(ok, 1);
    assert_eq!(served_at, served);
    assert_eq!(new_count, 3);
    assert_eq!(first, 1);
    assert_eq!(max, 5);
}

#[test]
fn p8d7_failed_pull_with_redacts_the_token() {
    let conn = mem();
    let token = "secret_token_fixture";
    scans::set_config(&conn, Some("https://example.com"), Some(token)).unwrap();
    scans::pull_with(&conn, "2026-08-15T18:01:00Z", |_, _| {
        Err(format!("upstream said {token}"))
    })
    .unwrap();
    let (ok, error): (i64, String) = conn
        .query_row("SELECT ok, error FROM scan_observations", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(ok, 0);
    assert!(!error.contains(token), "{error}");
    assert!(error.contains("[redacted]"), "{error}");
}

#[test]
fn p8d8_total_sums_ok_rows_only() {
    let conn = mem();
    configure(&conn);
    let body = ok_body(4, 1, 4, "2026-08-15T18:00:00.000Z");
    scans::pull_with(&conn, "2026-08-15T18:01:00Z", |_, _| {
        Ok((200, body.clone()))
    })
    .unwrap();
    conn.execute(
        "INSERT INTO scan_observations
         (id, fetched_at, url, http_status, ok, served_at, new_count,
          first_available_id, max_scan_id, error)
         VALUES ('rogue', '2026-08-15T18:02:00Z', 'https://example.com/scans?after=4',
                 500, 0, NULL, 99, NULL, NULL, 'failed')",
        [],
    )
    .unwrap();
    let view = scans::latest_view(&conn).unwrap();
    let count = view.count.expect("ok pull must produce a count");
    assert_eq!(count.value, 4);
    assert_ne!(count.value, 103);
}

#[test]
fn p8d9_cursor_is_max_ok_max_scan_id_and_zero_on_empty() {
    let conn = mem();
    assert_eq!(scans::cursor(&conn).unwrap(), 0);
    configure(&conn);
    let body = ok_body(2, 1, 7, "2026-08-15T18:00:00.000Z");
    scans::pull_with(&conn, "2026-08-15T18:01:00Z", |_, _| {
        Ok((200, body.clone()))
    })
    .unwrap();
    assert_eq!(scans::cursor(&conn).unwrap(), 7);
}

#[test]
fn p8d10_gap_names_the_exact_missing_count() {
    let conn = mem();
    configure(&conn);
    let first = ok_body(5, 1, 5, "2026-08-15T18:00:00.000Z");
    scans::pull_with(&conn, "2026-08-15T18:01:00Z", |_, _| {
        Ok((200, first.clone()))
    })
    .unwrap();
    let second = ok_body(2, 8, 9, "2026-08-15T18:02:00.000Z");
    let view = scans::pull_with(&conn, "2026-08-15T18:03:00Z", |_, _| {
        Ok((200, second.clone()))
    })
    .unwrap();
    let gap = view.gap_message.expect("missing ids must name a gap");
    assert!(gap.contains("2 scan ids are missing"), "{gap}");
    assert!(gap.contains("can never be counted"), "{gap}");
}

#[test]
fn p8d11_reset_names_the_restart_and_count_is_not_zero() {
    let conn = mem();
    configure(&conn);
    let first = ok_body(5, 1, 5, "2026-08-15T18:00:00.000Z");
    scans::pull_with(&conn, "2026-08-15T18:01:00Z", |_, _| {
        Ok((200, first.clone()))
    })
    .unwrap();
    let second = ok_body(2, 1, 2, "2026-08-15T18:02:00.000Z");
    let view = scans::pull_with(&conn, "2026-08-15T18:03:00Z", |_, _| {
        Ok((200, second.clone()))
    })
    .unwrap();
    let gap = view.gap_message.expect("reset must name the restart");
    assert!(gap.contains("log restarted"), "{gap}");
    let count = view.count.expect("reset still has a count");
    assert_ne!(count.value, 0);
    assert_eq!(count.value, 7);
}

#[test]
fn p8d12_configured_never_pulled_count_is_none() {
    let conn = mem();
    configure(&conn);
    let view = scans::latest_view(&conn).unwrap();
    assert!(view.count.is_none());
    assert_eq!(
        view.message,
        "Scan count unavailable — no pull has run yet."
    );
}

#[test]
fn p8d13_failed_after_good_keeps_prior_count() {
    let conn = mem();
    configure(&conn);
    let body = ok_body(6, 1, 6, "2026-08-15T18:00:00.000Z");
    scans::pull_with(&conn, "2026-08-15T18:01:00Z", |_, _| {
        Ok((200, body.clone()))
    })
    .unwrap();
    let view = scans::pull_with(&conn, "2026-08-15T18:02:00Z", |_, _| {
        Err("cable unplugged".into())
    })
    .unwrap();
    let count = view.count.expect("prior ok pull must remain");
    assert_eq!(count.value, 6);
    assert_eq!(count.fetched_at, "2026-08-15T18:00:00.000Z");
    assert!(
        view.message.contains("The last pull failed"),
        "{}",
        view.message
    );
    assert!(view.message.contains("cable unplugged"), "{}", view.message);
}

#[test]
fn p8d14_marketing_tsx_composes_no_scan_sentence() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/screens/Marketing.tsx");
    let src = fs::read_to_string(&path).expect("read Marketing.tsx");
    for forbidden in [
        "scans since counting began",
        "Scan count unavailable",
        "are not in this number",
    ] {
        assert!(
            !src.contains(forbidden),
            "Marketing.tsx must not hardcode {forbidden:?}"
        );
    }
}
