//! Rack-side fence 2 (GT-D21): pair, push, pull — the desktop stays the authority.

use crate::attention;
use crate::db;
use crate::event_file;
use crate::field_devices::{self, DeviceResolution};
use crate::phone::{self, AcceptedCapture};
use crate::phone_pull;
use crate::projection::{self, EXCLUSION_LIST};
use crate::scans;
use crate::trays;
use chrono::{Duration, SecondsFormat, Utc};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const NOW: &str = "2026-08-17T20:00:00.000Z";
const TOKEN_SECRET: &str = "secret_pull_token_fixture";
const UUID_A: &str = "11111111-2222-4333-8444-555555555555";
const UUID_B: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const UUID_C: &str = "99999999-8888-4777-8666-555555555555";

fn mem() -> Connection {
    db::open_in_memory().unwrap()
}
fn configure(conn: &Connection) {
    scans::set_config(conn, Some("https://scans.example"), Some(TOKEN_SECRET)).unwrap();
}
fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}
fn token_of(link: &Option<String>) -> String {
    let link = link.as_deref().expect("a configured pair carries a link");
    let i = link.find("/a/").unwrap() + 3;
    link[i..i + 32].to_string()
}
fn captured(days_ago: i64) -> String {
    (Utc::now() - Duration::days(days_ago)).to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn row(seq: i64, id: &str, tok: &str, verb: &str, crop: &str, qty: i64, oz: &str) -> String {
    let cap = captured(1);
    format!(
        r#"{{"seq":{seq},"proposalId":"{id}","deviceToken":"{tok}","verb":"{verb}","cropId":"{crop}","quantity":{qty},"actualYieldOz":{oz},"phoneCapturedAt":"{cap}","note":null,"receivedAt":"{NOW}"}}"#
    )
}
fn body(rows: &[String], first: Option<i64>, max: i64) -> String {
    let first = first.map(|f| f.to_string()).unwrap_or("null".into());
    format!(
        r#"{{"rows":[{}],"firstAvailableSeq":{first},"maxSeq":{max},"servedAt":"{NOW}"}}"#,
        rows.join(",")
    )
}
fn cards(conn: &Connection) -> usize {
    attention::check_attention(conn)
        .unwrap()
        .iter()
        .filter(|i| i.kind == phone::PHONE_PROPOSAL_ATTENTION_KIND)
        .count()
}
fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-ppull-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn f2a_schema_v34_tables_exist_reference_and_logs_excluded_idempotent() {
    let conn = mem();
    let v: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::SCHEMA_VERSION);
    assert_eq!(db::SCHEMA_VERSION, 44);
    for t in [
        "field_devices",
        "phone_pull_observations",
        "phone_pull_refusals",
    ] {
        assert_eq!(
            count(
                &conn,
                &format!("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{t}'")
            ),
            1,
            "{t}"
        );
        let line = EXCLUSION_LIST
            .iter()
            .find(|l| l.contains(t))
            .unwrap_or_else(|| panic!("{t} on EXCLUSION_LIST"));
        assert!(
            line.contains("not derived from events")
                || line.contains("neither copied nor compared"),
            "{line}"
        );
    }
    conn.pragma_update(None, "user_version", 33).unwrap();
    db::migrate(&conn).unwrap();
    let again: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(again, db::SCHEMA_VERSION);
}

#[test]
fn f2b_pair_mints_token_link_hash_only_one_live_admin_switch_and_retire() {
    let mut conn = mem();
    // GATE A - pairing does not need a worker. With no scan endpoint the pair
    // still mints, still stores only the hash, and the only thing absent is the
    // capture URL.
    let p0 = field_devices::pair_admin(&mut conn).unwrap();
    assert_eq!(p0.link, None, "no endpoint: no link, never an empty string");
    assert_eq!(p0.token.len(), 32, "{}", p0.token);
    assert_eq!(p0.token, p0.token.to_lowercase(), "lowercase hex");
    assert!(
        p0.token.chars().all(|c| c.is_ascii_hexdigit()),
        "{}",
        p0.token
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM field_devices WHERE retired_at IS NULL"
        ),
        1
    );
    let stored0: String = conn
        .query_row(
            "SELECT token_hash FROM field_devices WHERE device_id = ?1",
            [&p0.device_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored0, field_devices::token_hash(&p0.token));
    assert_ne!(stored0, p0.token, "hash, never the token");
    // Retire it so the rest of this test reads the same empty desk it always did.
    field_devices::retire_admin(&conn).unwrap();
    configure(&conn);
    assert_eq!(
        field_devices::admin_status(&conn).unwrap().status,
        "No Admin phone is paired."
    );
    assert!(!field_devices::has_live_admin(&conn).unwrap());
    let p1 = field_devices::pair_admin(&mut conn).unwrap();
    let link1 = p1.link.as_deref().expect("configured: a link");
    assert!(link1.starts_with("https://scans.example/a/"), "{link1}");
    let tok1 = token_of(&p1.link);
    assert!(tok1.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    assert!(
        link1.contains("?c=kale:Kale") || link1.contains("&c=kale:Kale"),
        "{link1}"
    );
    assert!(
        link1.contains("c=broccoli:"),
        "every crop rides in the link"
    );
    assert_eq!(p1.pairing_text, "Open this link on the phone. It is the phone's key — pairing another phone retires this one.");
    assert!(
        p1.status.starts_with("Admin phone paired today at "),
        "{}",
        p1.status
    );
    let stored: String = conn
        .query_row(
            "SELECT token_hash FROM field_devices WHERE device_id = ?1",
            [&p1.device_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, field_devices::token_hash(&tok1));
    assert_ne!(stored, tok1, "hash, never the token");
    assert_eq!(
        field_devices::resolve_token(&conn, &tok1).unwrap(),
        DeviceResolution::Live(p1.device_id.clone())
    );
    // A second live admin cannot exist by index.
    let dup = conn.execute("INSERT INTO field_devices (device_id, label, role, token_hash, paired_at, retired_at) VALUES ('x', NULL, 'admin', 'h', '2026-08-17T00:00:00Z', NULL)", []);
    assert!(dup.is_err());
    // Switch: pairing again retires the first in the same act.
    let p2 = field_devices::pair_admin(&mut conn).unwrap();
    let tok2 = token_of(&p2.link);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM field_devices WHERE retired_at IS NULL"
        ),
        1
    );
    assert_eq!(
        field_devices::resolve_token(&conn, &tok1).unwrap(),
        DeviceResolution::Retired
    );
    assert_eq!(
        field_devices::resolve_token(&conn, &tok2).unwrap(),
        DeviceResolution::Live(p2.device_id.clone())
    );
    assert_eq!(
        field_devices::resolve_token(&conn, "00000000000000000000000000000000").unwrap(),
        DeviceResolution::Unknown
    );
    // Retire: zero live admin; final; delete-proof.
    let r = field_devices::retire_admin(&conn).unwrap();
    assert_eq!(
        r.line,
        "Retired. Captures from that phone are refused from now on."
    );
    assert_eq!(r.status, "No Admin phone is paired.");
    assert_eq!(
        field_devices::retire_admin(&conn).unwrap_err(),
        "No Admin phone is paired."
    );
    assert_eq!(
        field_devices::resolve_token(&conn, &tok2).unwrap(),
        DeviceResolution::Retired
    );
    let e = conn
        .execute(
            "UPDATE field_devices SET token_hash = 'z' WHERE device_id = ?1",
            [&p2.device_id],
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("immutable"), "{e}");
    let e = conn
        .execute(
            "UPDATE field_devices SET retired_at = NULL WHERE device_id = ?1",
            [&p2.device_id],
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("final"), "{e}");
    let e = conn
        .execute("DELETE FROM field_devices", [])
        .unwrap_err()
        .to_string();
    assert!(e.contains("append-only"), "{e}");
}

#[test]
fn f2c_pull_written_known_refused_per_reason_signed_sentences_and_attention() {
    let mut conn = mem();
    configure(&conn);
    let p = field_devices::pair_admin(&mut conn).unwrap();
    let _live = token_of(&p.link);
    let retired_p = {
        let old = p.clone();
        let _ = field_devices::pair_admin(&mut conn).unwrap();
        old
    };
    // (re-pair once more so `live` below is the current admin)
    let p3 = field_devices::pair_admin(&mut conn).unwrap();
    let live = token_of(&p3.link);
    let stale = token_of(&retired_p.link);
    let page = body(
        &[
            row(1, UUID_A, &live, "move_to_light", "kale", 2, "null"), // written
            row(2, UUID_A, &live, "move_to_light", "kale", 2, "null"), // known
            row(3, UUID_B, &stale, "move_to_light", "kale", 1, "null"), // retired_device
            row(
                4,
                UUID_C,
                "0123456789abcdef0123456789abcdef",
                "harvest",
                "kale",
                1,
                "5.0",
            ), // unknown_device
            row(
                5,
                "dddddddd-1111-4222-8333-444444444444",
                &live,
                "harvest",
                "kale",
                0,
                "5.0",
            ), // invalid
            row(
                6,
                "eeeeeeee-1111-4222-8333-444444444444",
                &live,
                "move_to_light",
                "no-such-crop",
                1,
                "null",
            ), // unknown_crop
        ],
        Some(1),
        6,
    );
    let view = phone_pull::pull_with(&mut conn, NOW, |url, secret| {
        assert!(url.ends_with("/field-proposals?after=0"), "{url}");
        assert_eq!(secret, TOKEN_SECRET);
        Ok((200, page.clone()))
    })
    .unwrap();
    assert_eq!(
        view.message,
        "Pulled 6 phone captures: 1 new, 1 already known, 4 refused."
    );
    assert_eq!(view.refusal_message.as_deref(), Some("4 refused — not from the live Admin phone, or the capture was malformed. Nothing was invented."));
    assert!(view.last_ok_message.is_none() && view.gap_message.is_none());
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM phone_proposals"), 1);
    let dev: String = conn
        .query_row(
            "SELECT device_id FROM phone_proposals WHERE proposal_id = ?1",
            [UUID_A],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        dev, p3.device_id,
        "device id resolved by the desktop, never sent by the phone"
    );
    for (reason, n) in [
        ("retired_device", 1),
        ("unknown_device", 1),
        ("invalid", 1),
        ("unknown_crop", 1),
    ] {
        assert_eq!(
            count(
                &conn,
                &format!("SELECT COUNT(*) FROM phone_pull_refusals WHERE reason = '{reason}'")
            ),
            n,
            "{reason}"
        );
    }
    let stored_hash: String = conn
        .query_row(
            "SELECT device_token_hash FROM phone_pull_refusals WHERE reason = 'retired_device'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored_hash, field_devices::token_hash(&stale));
    assert_eq!(phone_pull::cursor(&conn).unwrap(), 6);
    assert_eq!(cards(&conn), 1);
    let card = attention::check_attention(&conn)
        .unwrap()
        .into_iter()
        .find(|i| i.entity_id.as_deref() == Some(UUID_A))
        .expect("card");
    assert!(
        card.message
            .starts_with("Move 2 trays of Kale to light — captured "),
        "{}",
        card.message
    );
    assert_eq!(phone::phone_captures(&conn).unwrap().len(), 1);
    let err = conn
        .execute("DELETE FROM phone_pull_refusals", [])
        .unwrap_err()
        .to_string();
    assert!(err.contains("append-only"), "{err}");
    let view2 = phone_pull::pull_with(&mut conn, NOW, |url, _| {
        assert!(url.ends_with("after=6"), "{url}");
        Ok((200, body(&[], Some(1), 6)))
    })
    .unwrap();
    assert_eq!(
        view2.message,
        "Pulled 0 phone captures: 0 new, 0 already known, 0 refused."
    );
    let _ = live;
}

#[test]
fn f2d_transport_failure_keeps_cursor_and_says_so() {
    let mut conn = mem();
    configure(&conn);
    let _p = field_devices::pair_admin(&mut conn).unwrap();
    let v =
        phone_pull::pull_with(&mut conn, NOW, |_, _| Err(format!("boom {TOKEN_SECRET}"))).unwrap();
    assert!(
        v.message
            .starts_with("Phone captures unavailable — the last pull failed ("),
        "{}",
        v.message
    );
    assert!(!v.message.contains(TOKEN_SECRET), "secret redacted");
    let ok = body(&[], Some(1), 3);
    phone_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, ok.clone()))).unwrap();
    let v = phone_pull::pull_with(&mut conn, NOW, |_, _| Ok((500, String::new()))).unwrap();
    assert!(
        v.message.starts_with("The last pull failed ("),
        "{}",
        v.message
    );
    assert!(
        v.message
            .ends_with("The counts below are from the last successful pull."),
        "{}",
        v.message
    );
    assert_eq!(
        v.last_ok_message.as_deref(),
        Some("Pulled 0 phone captures: 0 new, 0 already known, 0 refused.")
    );
    assert_eq!(phone_pull::cursor(&conn).unwrap(), 3);
}

#[test]
fn f2e_gap_and_restart_sentences() {
    let mut conn = mem();
    configure(&conn);
    let _p = field_devices::pair_admin(&mut conn).unwrap();
    phone_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, body(&[], Some(1), 3)))).unwrap();
    let v = phone_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, body(&[], Some(6), 6)))).unwrap();
    assert_eq!(
        v.gap_message.as_deref(),
        Some("2 phone captures are missing from the endpoint and can never be pulled.")
    );
    let v = phone_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, body(&[], Some(1), 2)))).unwrap();
    assert_eq!(v.gap_message.as_deref(), Some(phone_pull::LOG_RESTARTED));
}

#[test]
fn f2f_unconfigured_never_pulled_and_auto_pull_only_with_live_admin() {
    let mut conn = mem();
    assert_eq!(
        phone_pull::latest_view(&conn).unwrap().message,
        "Phone captures unavailable — no scan endpoint is configured."
    );
    assert!(phone_pull::auto_pull_with(&mut conn, NOW, |_, _| panic!(
        "no request without a live Admin"
    ))
    .unwrap()
    .is_none());
    configure(&conn);
    assert_eq!(
        phone_pull::latest_view(&conn).unwrap().message,
        "Phone captures unavailable — no pull has run yet."
    );
    assert!(
        phone_pull::auto_pull_with(&mut conn, NOW, |_, _| panic!("still no live Admin"))
            .unwrap()
            .is_none()
    );
    let _p = field_devices::pair_admin(&mut conn).unwrap();
    let v = phone_pull::auto_pull_with(&mut conn, NOW, |_, _| Ok((200, body(&[], None, 0))))
        .unwrap()
        .expect("pulls with a live Admin");
    assert_eq!(
        v.message,
        "Pulled 0 phone captures: 0 new, 0 already known, 0 refused."
    );
    field_devices::retire_admin(&conn).unwrap();
    assert!(
        phone_pull::auto_pull_with(&mut conn, NOW, |_, _| panic!("retired: no request"))
            .unwrap()
            .is_none()
    );
    // Unconfigured endpoint with a live Admin: the sentence, no request.
    let mut c2 = mem();
    configure(&c2);
    let _ = field_devices::pair_admin(&mut c2).unwrap();
    c2.execute(
        "UPDATE scan_config SET endpoint_url = NULL, pull_token = NULL WHERE id = 1",
        [],
    )
    .unwrap();
    let v = phone_pull::auto_pull_with(&mut c2, NOW, |_, _| panic!("unconfigured: no request"))
        .unwrap()
        .unwrap();
    assert_eq!(
        v.message,
        "Phone captures unavailable — no scan endpoint is configured."
    );
}

#[test]
fn f2g_pulled_capture_confirms_through_the_fence_1_gate_and_replay_passes() {
    let dir = temp_dir("f2g");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    configure(&conn);
    let p = field_devices::pair_admin(&mut conn).unwrap();
    let live = token_of(&p.link);
    let t = trays::sow_tray(&mut conn, "kale", 2).unwrap();
    trays::dev_backdate_tray(&mut conn, &t.id, 3).unwrap();
    let page = body(
        &[row(1, UUID_A, &live, "move_to_light", "kale", 2, "null")],
        Some(1),
        1,
    );
    phone_pull::pull_with(&mut conn, NOW, |_, _| Ok((200, page.clone()))).unwrap();
    let r = phone::confirm_phone_captures(
        &mut conn,
        &[AcceptedCapture {
            proposal_id: UUID_A.into(),
            quantity: 2,
            actual_yield_oz: None,
        }],
    )
    .unwrap();
    assert_eq!(r.written.len(), 1, "{:?}", r.blocked);
    assert_eq!(trays::get_tray(&conn, &t.id).unwrap().state, "light");
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);
    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    let _ = fs::remove_dir_all(&dir);
}

/// Option A. The pairing view now carries the token it was already carrying
/// inside the link. The law at rest is unchanged, and this test says so in the
/// two places that matter: no column of field_devices holds the plaintext, and
/// the status line an already-paired phone shows does not hold it either.
#[test]
fn gt21p1_pairing_view_carries_the_same_token_the_link_carries() {
    let mut conn = mem();
    configure(&conn);
    let p = field_devices::pair_admin(&mut conn).unwrap();
    assert_eq!(p.token.len(), 32, "{}", p.token);
    assert!(
        p.token.chars().all(|c| c.is_ascii_hexdigit()),
        "{}",
        p.token
    );
    assert_eq!(p.token, p.token.to_lowercase(), "lowercase hex");
    assert_eq!(
        p.token,
        token_of(&p.link),
        "the bare token is the token in the link"
    );
    assert!(
        p.link.as_deref().unwrap().contains(&p.token),
        "one secret, not two"
    );
    let (dev, lab, role, hash, at, ret): (String, Option<String>, String, String, String, Option<String>) = conn
        .query_row(
            "SELECT device_id, label, role, token_hash, paired_at, retired_at FROM field_devices WHERE device_id = ?1",
            [&p.device_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(hash, field_devices::token_hash(&p.token));
    assert_ne!(hash, p.token, "hash at rest, never the token");
    let row_text = format!(
        "{}{}{}{}{}{}",
        dev,
        lab.unwrap_or_default(),
        role,
        hash,
        at,
        ret.unwrap_or_default()
    );
    assert!(
        !row_text.contains(&p.token),
        "the plaintext token must appear in no column of field_devices"
    );
    let status = field_devices::admin_status(&conn).unwrap();
    assert!(
        !status.status.contains(&p.token),
        "nothing honest to show for a phone paired earlier - the desk never kept it"
    );
}
