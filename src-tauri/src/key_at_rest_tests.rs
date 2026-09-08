//! KEY-AT-REST (board-signed 2026-09-07, tip 98e2e0d) — the two connection
//! secrets are sealed to this account before they touch the farm file.
//!
//! SCOPE A: `stripe_config.restricted_key` and `scan_config.pull_token`.
//! WRAP A / ENTROPY B / FORMAT A / TAG: DPAPI user scope, the application
//! id as entropy, the same TEXT columns under `dpapi1:` (a build for another
//! OS writes `plain:`). MIGRATE A + RESIDUE B: an untagged plaintext is
//! sealed on the next open, with `secure_delete` on and a WAL checkpoint
//! after. UNWRAP-FAIL A: a blob this account cannot open stays, reads as not
//! connected / no token, and raises one card. SNAPSHOT A / EXPORT A are
//! pinned where they live (snapshots.rs and export_tests are untouched here
//! beyond the live-column pins).
//!
//! No SCHEMA_VERSION literal here — H-11(b) keeps that pin in its three files.

use crate::db;
use crate::key_at_rest::{self, Opened, TAG, UNREADABLE_KIND, UNREADABLE_LINE};
use crate::money;
use crate::scans;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};

const KEY: &str = "rk_test_key_at_rest_unit_key_0001";
const PULL: &str = "pull-token-key-at-rest-unit-0001";
const URL: &str = "https://scan.example.test";

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "farm-os-key-at-rest-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn account() -> money::AccountInfo {
    money::AccountInfo {
        account_id: "acct_key_at_rest".into(),
        account_name: "Key At Rest Farm".into(),
        mode: "test".into(),
    }
}

fn store_both(conn: &Connection) {
    money::store_stripe_key(conn, KEY, &account()).unwrap();
    scans::set_config(conn, Some(URL), Some(PULL)).unwrap();
}

fn raw_key(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT restricted_key FROM stripe_config WHERE id = 1",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn raw_pull(conn: &Connection) -> Option<String> {
    conn.query_row("SELECT pull_token FROM scan_config WHERE id = 1", [], |r| {
        r.get(0)
    })
    .unwrap()
}

fn unreadable_cards(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT message, actions FROM attention
             WHERE kind = ?1 AND resolved_at IS NULL",
        )
        .unwrap();
    stmt.query_map([UNREADABLE_KIND], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// True when any regular file directly under `dir` carries `needle`.
#[cfg(windows)]
fn dir_holds(dir: &Path, needle: &str) -> bool {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .any(|p| {
            let bytes = fs::read(&p).unwrap();
            bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
        })
}

#[test]
fn kar_seal_and_open_round_trip_both_columns() {
    let conn = mem();
    store_both(&conn);
    let key = raw_key(&conn).expect("key stored");
    let pull = raw_pull(&conn).expect("token stored");
    assert!(key.starts_with(TAG), "{key}");
    assert!(pull.starts_with(TAG), "{pull}");
    assert_eq!(
        key_at_rest::open(Some(key.as_str())),
        Opened::Secret(KEY.to_string())
    );
    assert_eq!(
        key_at_rest::open(Some(pull.as_str())),
        Opened::Secret(PULL.to_string())
    );
    // The readers open on the way to the wire.
    assert!(money::gateway_from_db(&conn).is_ok());
    assert!(money::money_status(&conn).unwrap().configured);
    assert_eq!(
        scans::endpoint_and_token(&conn).unwrap(),
        (Some(URL.to_string()), Some(PULL.to_string()))
    );
    assert!(scans::config(&conn).unwrap().token_set);
    // Empty and NULL open as Empty; an untagged value opens as Plain.
    assert_eq!(key_at_rest::open(None), Opened::Empty);
    assert_eq!(key_at_rest::open(Some("")), Opened::Empty);
    assert_eq!(key_at_rest::open(Some(KEY)), Opened::Plain(KEY.to_string()));
}

#[cfg(windows)]
#[test]
fn kar_live_farm_file_never_holds_the_plaintext_after_store() {
    let dir = tempfile_dir("store");
    let conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    store_both(&conn);
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    assert!(!dir_holds(&dir, KEY), "the key reached the farm folder");
    assert!(!dir_holds(&dir, PULL), "the token reached the farm folder");
    drop(conn);
    assert!(!dir_holds(&dir, KEY));
    assert!(!dir_holds(&dir, PULL));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn kar_untagged_plaintext_is_sealed_at_open_once_and_only_once() {
    let dir = tempfile_dir("migrate");
    let farm = dir.join("farm.db");
    {
        // A farm from before the wrap: both columns hold the plaintext.
        let conn = db::open_and_migrate(&farm).unwrap();
        conn.execute(
            "UPDATE stripe_config SET restricted_key = ?1, account_id = 'acct_old',
             account_name = 'Old Farm', mode = 'test' WHERE id = 1",
            [KEY],
        )
        .unwrap();
        conn.execute(
            "UPDATE scan_config SET endpoint_url = ?1, pull_token = ?2 WHERE id = 1",
            [URL, PULL],
        )
        .unwrap();
    }
    // The next open seals both, and the farm reads exactly as before.
    let conn = db::open_and_migrate(&farm).unwrap();
    let key = raw_key(&conn).expect("key kept");
    let pull = raw_pull(&conn).expect("token kept");
    assert!(key.starts_with(TAG), "{key}");
    assert!(pull.starts_with(TAG), "{pull}");
    assert_eq!(
        key_at_rest::open(Some(key.as_str())),
        Opened::Secret(KEY.to_string())
    );
    assert_eq!(
        key_at_rest::open(Some(pull.as_str())),
        Opened::Secret(PULL.to_string())
    );
    assert!(money::gateway_from_db(&conn).is_ok());
    assert_eq!(
        scans::endpoint_and_token(&conn).unwrap(),
        (Some(URL.to_string()), Some(PULL.to_string()))
    );
    assert!(unreadable_cards(&conn).is_empty(), "nothing was unreadable");
    // RESIDUE B: the old bytes left the live file and the WAL.
    #[cfg(windows)]
    {
        assert!(!dir_holds(&dir, KEY), "the plaintext key survived the pass");
        assert!(
            !dir_holds(&dir, PULL),
            "the plaintext token survived the pass"
        );
    }
    drop(conn);
    // Idempotent: a second open leaves the sealed bytes exactly as they are.
    let conn = db::open_and_migrate(&farm).unwrap();
    assert_eq!(raw_key(&conn).as_deref(), Some(key.as_str()));
    assert_eq!(raw_pull(&conn).as_deref(), Some(pull.as_str()));
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn kar_unreadable_blob_stays_reads_as_absent_and_raises_one_card() {
    let conn = mem();
    conn.execute(
        "UPDATE stripe_config SET restricted_key = 'dpapi1:not-a-blob!',
         account_id = 'acct_key_at_rest', account_name = 'Elsewhere', mode = 'test'
         WHERE id = 1",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE scan_config SET endpoint_url = ?1, pull_token = 'dpapi1:not-a-blob!'
         WHERE id = 1",
        [URL],
    )
    .unwrap();
    assert_eq!(
        key_at_rest::open(Some("dpapi1:not-a-blob!")),
        Opened::Unreadable
    );
    key_at_rest::seal_at_open(&conn).unwrap();
    key_at_rest::seal_at_open(&conn).unwrap();
    // The blob stays; the readers say not connected / no token.
    assert_eq!(raw_key(&conn).as_deref(), Some("dpapi1:not-a-blob!"));
    assert_eq!(raw_pull(&conn).as_deref(), Some("dpapi1:not-a-blob!"));
    assert!(!money::money_status(&conn).unwrap().configured);
    let err = money::gateway_from_db(&conn)
        .err()
        .expect("an unreadable blob is not connected");
    assert_eq!(err, money::STRIPE_NOT_CONNECTED_LINE);
    assert!(!scans::config(&conn).unwrap().token_set);
    assert_eq!(
        scans::endpoint_and_token(&conn).unwrap(),
        (Some(URL.to_string()), None)
    );
    // One card, the signed sentence, dismiss only — and only one after two passes.
    let cards = unreadable_cards(&conn);
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0].0, UNREADABLE_LINE);
    assert_eq!(cards[0].1, r#"["dismiss"]"#);
    assert_eq!(
        UNREADABLE_LINE,
        "This machine cannot read a stored secret. Connect Stripe or save the pull token again."
    );
    // The desk doors overwrite it.
    store_both(&conn);
    assert!(money::money_status(&conn).unwrap().configured);
    assert!(scans::config(&conn).unwrap().token_set);
    assert!(money::gateway_from_db(&conn).is_ok());
}

#[test]
fn kar_desk_views_still_carry_no_secret() {
    let conn = mem();
    store_both(&conn);
    let money_wire = serde_json::to_string(&money::money_status(&conn).unwrap()).unwrap();
    assert!(!money_wire.contains(KEY), "{money_wire}");
    assert!(!money_wire.contains("rk_"), "{money_wire}");
    assert!(!money_wire.contains(TAG), "{money_wire}");
    let scan_wire = serde_json::to_string(&scans::config(&conn).unwrap()).unwrap();
    assert!(!scan_wire.contains(PULL), "{scan_wire}");
    assert!(!scan_wire.contains("pull_token"), "{scan_wire}");
    assert!(!scan_wire.contains("pullToken"), "{scan_wire}");
    assert!(!scan_wire.contains(TAG), "{scan_wire}");
}

#[test]
fn kar_tag_literal_is_pinned_once_and_the_pass_sits_after_migrate() {
    #[cfg(windows)]
    assert_eq!(TAG, "dpapi1:");
    #[cfg(not(windows))]
    assert_eq!(TAG, "plain:");
    let module = read("src/key_at_rest.rs");
    assert_eq!(
        module.matches("\"dpapi1:\"").count(),
        1,
        "the tag literal lives in key_at_rest.rs once"
    );
    for rel in ["src/money.rs", "src/scans.rs", "src/db.rs"] {
        assert_eq!(
            read(rel).matches("dpapi1:").count(),
            0,
            "{rel} names the tag"
        );
    }
    assert!(
        !module.contains("safety_snapshot"),
        "the pass never takes a pre-migration snapshot"
    );
    assert!(module.contains("CRYPTPROTECT_UI_FORBIDDEN"));
    assert!(module.contains("GROUNDTRUTH_APPLICATION_ID"));
    let schema = read("src/db.rs");
    assert_eq!(
        schema
            .matches("crate::key_at_rest::seal_at_open(&conn)?;")
            .count(),
        2,
        "open_and_migrate and open_in_memory run the pass after migrate"
    );
    let open = schema
        .find("pub fn open_and_migrate(")
        .expect("open_and_migrate exists");
    let body = &schema[open..];
    let migrate_at = body.find("migrate(&conn)?;").expect("migrate call");
    let pass_at = body
        .find("crate::key_at_rest::seal_at_open(&conn)?;")
        .expect("pass call");
    assert!(migrate_at < pass_at, "the pass runs after migrate");
}
