//! Targeted coverage for the loopback port. S1 + S4a(a) + S4b(a) + S6.

use crate::db;
use crate::dock_port::{self, DockPortView};
use crate::field_devices;
use crate::scans;
use rusqlite::Connection;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// FI-7 (Q-5) - the port document's top-level key count, in ONE place.
/// e1 and h1 both assert against it. They were two independent literals and
/// drifted at FI-1 and again at FI-5; now a fence that changes the document
/// changes this line and there is nothing else to find.
const PORT_DOC_KEYS: usize = 16;

fn configure(conn: &Connection) {
    scans::set_config(
        conn,
        Some("https://scans.example"),
        Some("secret_pull_token_fixture"),
    )
    .unwrap();
}

fn token_of(link: &Option<String>) -> String {
    let link = link.as_deref().expect("a configured pair carries a link");
    let i = link.find("/a/").unwrap() + 3;
    link[i..i + 32].to_string()
}

fn paired_db() -> (Arc<Mutex<Connection>>, String) {
    let mut conn = db::open_in_memory().unwrap();
    configure(&conn);
    let pairing = field_devices::pair_admin(&mut conn).unwrap();
    let token = token_of(&pairing.link);
    (Arc::new(Mutex::new(conn)), token)
}

fn lock_suite() -> std::sync::MutexGuard<'static, ()> {
    static SUITE: Mutex<()> = Mutex::new(());
    match SUITE.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn start_clean(db: Arc<Mutex<Connection>>) -> u16 {
    let _ = dock_port::stop();
    let root = std::env::temp_dir().join("dockdoc-port-paths");
    let _ = std::fs::create_dir_all(&root);
    let view = dock_port::start(db, root.clone(), root, "127.0.0.1:0").unwrap();
    view.port.expect("ephemeral port")
}

fn attention_row_count(db: &Arc<Mutex<Connection>>) -> i64 {
    db.lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM attention", [], |r| r.get(0))
        .unwrap()
}

fn exchange(port: u16, request: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let _ = stream.flush();
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    parse_http(&buf)
}

fn parse_http(raw: &[u8]) -> (u16, String, Vec<u8>) {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header terminator");
    let header = std::str::from_utf8(&raw[..split]).unwrap();
    let body = raw[split + 4..].to_vec();
    let status = header
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    (status, header.to_string(), body)
}

fn get(port: u16, path: &str, token: Option<&str>) -> (u16, String, Vec<u8>) {
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    if let Some(t) = token {
        req.push_str(&format!("X-Dock-Token: {t}\r\n"));
    }
    req.push_str("\r\n");
    exchange(port, &req)
}

#[test]
fn t1_bound_address_is_loopback_and_port_nonzero() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(Arc::clone(&db));
    assert_ne!(port, 0);
    let (status, _, _) = get(port, "/ready", None);
    assert_eq!(status, 401);
    let view = dock_port::status();
    assert!(view.running);
    assert_eq!(view.port, Some(port));
    let _ = dock_port::stop();
}

#[test]
fn t2_get_ready_live_admin_token_is_exactly_ready_true() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, headers, body) = get(port, "/ready", Some(&token));
    assert_eq!(status, 200);
    assert!(headers
        .to_ascii_lowercase()
        .contains("content-type: application/json"));
    assert_eq!(body, br#"{"ready":true}"#);
    let _ = dock_port::stop();
}

#[test]
fn t3_get_ready_without_token_is_401() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/ready", None);
    assert_eq!(status, 401);
    assert_eq!(body, br#"{"error":"Unauthorized."}"#);
    let _ = dock_port::stop();
}

#[test]
fn t4_get_ready_unknown_token_is_401() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/ready", Some("00000000000000000000000000000000"));
    assert_eq!(status, 401);
    assert_eq!(body, br#"{"error":"Unauthorized."}"#);
    let _ = dock_port::stop();
}

#[test]
fn t5_get_ready_after_retire_admin_is_same_401_as_t3() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(Arc::clone(&db));
    {
        let conn = db.lock().unwrap();
        field_devices::retire_admin(&conn).unwrap();
    }
    let (status, _, body) = get(port, "/ready", Some(&token));
    assert_eq!(status, 401);
    assert_eq!(body, br#"{"error":"Unauthorized."}"#);
    let _ = dock_port::stop();
}

#[test]
fn t6_post_ready_with_live_token_is_405() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let req =
        format!("POST /ready HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Dock-Token: {token}\r\n\r\n");
    let (status, _, body) = exchange(port, &req);
    assert_eq!(status, 405);
    assert_eq!(body, br#"{"error":"Read only."}"#);
    let _ = dock_port::stop();
}

#[test]
fn t7_put_and_delete_are_405() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    for method in ["PUT", "DELETE"] {
        let req = format!(
            "{method} /ready HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Dock-Token: {token}\r\n\r\n"
        );
        let (status, _, body) = exchange(port, &req);
        assert_eq!(status, 405, "{method}");
        assert_eq!(body, br#"{"error":"Read only."}"#, "{method}");
    }
    let _ = dock_port::stop();
}

#[test]
fn t8_get_nope_with_live_token_is_404() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/nope", Some(&token));
    assert_eq!(status, 404);
    assert_eq!(body, br#"{"error":"Not found."}"#);
    let _ = dock_port::stop();
}

#[test]
fn t9_get_carrying_content_length_is_400_and_body_is_not_read() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let req = format!(
        "GET /ready HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Dock-Token: {token}\r\nContent-Length: 100\r\n\r\nthis body must not be consumed"
    );
    let (status, _, body) = exchange(port, &req);
    assert_eq!(status, 400);
    assert_eq!(body, br#"{"error":"Bad request."}"#);
    let _ = dock_port::stop();
}

#[test]
fn t10_head_ready_live_token_is_200_headers_zero_length_body() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let req =
        format!("HEAD /ready HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Dock-Token: {token}\r\n\r\n");
    let (status, headers, body) = exchange(port, &req);
    assert_eq!(status, 200);
    assert!(headers
        .to_ascii_lowercase()
        .contains("content-type: application/json"));
    assert!(headers.contains("Content-Length: 14"), "{headers}");
    assert!(body.is_empty(), "HEAD must not carry a body: {body:?}");
    let _ = dock_port::stop();
}

#[test]
fn t11_get_ready_query_string_is_ignored() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/ready?x=1", Some(&token));
    assert_eq!(status, 200);
    assert_eq!(body, br#"{"ready":true}"#);
    let _ = dock_port::stop();
}

#[test]
fn t12_stop_ends_the_thread_and_connect_fails() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let stopped: DockPortView = dock_port::stop();
    assert!(!stopped.running);
    assert_eq!(stopped.port, None);
    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err(),
        "listener must not outlive stop"
    );
}

#[test]
fn d1_get_folds_live_admin_token_is_200_and_json_parses() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, headers, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    assert!(headers
        .to_ascii_lowercase()
        .contains("content-type: application/json"));
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    assert!(parsed.is_object());
    let _ = dock_port::stop();
}

#[test]
fn d2_folds_document_has_exactly_the_four_keys() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let obj = parsed.as_object().expect("object");
    assert!(obj.contains_key("overall"));
    assert!(obj.contains_key("byScope"));
    assert!(obj.contains_key("cards"));
    assert!(obj.contains_key("checks"));
    assert!(!obj.contains_key("surfaces"));
    assert!(!obj.contains_key("ranks"));
    assert!(!obj.contains_key("todayAttentionOrder"));
    let cards = obj["cards"].as_array().expect("cards array");
    assert_eq!(cards.len(), 6, "FI-3 six nodes");
    assert_eq!(cards[0]["card"], "money");
    assert_eq!(cards[1]["card"], "cover");
    assert_eq!(cards[2]["card"], "promise");
    assert_eq!(cards[3]["card"], "rack");
    assert_eq!(cards[4]["card"], "phone_queue");
    assert_eq!(cards[5]["card"], "system");
    assert!(
        cards[4]["severity"].is_null(),
        "phone_queue severity must be null (S5)"
    );
    let _ = dock_port::stop();
}

#[test]
fn d3_folds_auth_and_method_match_ready() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(Arc::clone(&db));
    let (status, _, body) = get(port, "/folds", None);
    assert_eq!(status, 401);
    assert_eq!(body, br#"{"error":"Unauthorized."}"#);
    let (status, _, body) = get(port, "/folds", Some("00000000000000000000000000000000"));
    assert_eq!(status, 401);
    assert_eq!(body, br#"{"error":"Unauthorized."}"#);
    {
        let conn = db.lock().unwrap();
        field_devices::retire_admin(&conn).unwrap();
    }
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 401);
    assert_eq!(body, br#"{"error":"Unauthorized."}"#);
    let req =
        format!("POST /folds HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Dock-Token: {token}\r\n\r\n");
    let (status, _, body) = exchange(port, &req);
    assert_eq!(status, 405);
    assert_eq!(body, br#"{"error":"Read only."}"#);
    let _ = dock_port::stop();
}

#[test]
fn d4_get_folds_does_not_change_attention_row_count() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let before = attention_row_count(&db);
    let port = start_clean(Arc::clone(&db));
    let (status, _, _) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let after = attention_row_count(&db);
    assert_eq!(
        after, before,
        "GET /folds must not write the attention table (S8(c))"
    );
    let _ = dock_port::stop();
}

#[test]
fn e1_folds_document_has_exactly_the_signed_keys() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let obj = parsed.as_object().expect("object");
    assert_eq!(obj.len(), PORT_DOC_KEYS);
    assert!(obj.contains_key("documentVersion"));
    assert!(obj.contains_key("overall"));
    assert!(obj.contains_key("byScope"));
    assert!(obj.contains_key("cards"));
    assert!(obj.contains_key("checks"));
    assert!(obj.contains_key("worstClash"));
    assert!(obj.contains_key("clashes"));
    assert!(obj.contains_key("phoneQueue"));
    assert!(obj.contains_key("diagnosis"));
    assert!(obj.contains_key("captureEndpoint"));
    assert!(obj.contains_key("pullHealth"));
    assert!(obj.contains_key("lastPullAtDisplay"));
    assert!(obj.contains_key("servedAt"));
    assert!(obj.contains_key("servedAtDisplay"));
    assert!(obj.contains_key("attentionEvaluatedAt"));
    assert!(obj.contains_key("attentionEvaluatedAtDisplay"));
    assert!(!obj.contains_key("surfaces"));
    assert!(!obj.contains_key("ranks"));
    assert!(!obj.contains_key("todayAttentionOrder"));
    assert_eq!(
        obj["documentVersion"], 9,
        "FI-10b put the pull's own age on the wire"
    );
    let _ = dock_port::stop();
}

#[test]
fn e2_served_at_parses_as_rfc3339() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let served = parsed["servedAt"].as_str().expect("servedAt");
    chrono::DateTime::parse_from_rfc3339(served).expect("servedAt is RFC3339");
    // FI-1: the second age is absent or a real instant. Never a placeholder.
    match parsed["attentionEvaluatedAt"].as_str() {
        Some(at) => {
            chrono::DateTime::parse_from_rfc3339(at).expect("attentionEvaluatedAt is RFC3339");
        }
        None => assert!(parsed["attentionEvaluatedAt"].is_null()),
    }
    let _ = dock_port::stop();
}

#[test]
fn e3_get_folds_with_non_null_worst_clash_does_not_change_attention_row_count() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    {
        let conn = db.lock().unwrap();
        let now = crate::db::utc_now_rfc3339();
        conn.execute(
            "INSERT INTO attention
             (id, kind, entity_type, entity_id, message, actions, created_at, resolved_at, resolved_by)
             VALUES ('e3-open', 'order.unrecorded', 'harvest_date', '2099-01-01',
                     'seeded open row', '[]', ?1, NULL, NULL)",
            rusqlite::params![&now],
        )
        .unwrap();
    }
    let before = attention_row_count(&db);
    let port = start_clean(Arc::clone(&db));
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    assert!(
        !parsed["worstClash"].is_null(),
        "seeded open row must produce a clash so the write-free path is exercised"
    );
    let after = attention_row_count(&db);
    assert_eq!(
        after, before,
        "GET /folds must not write the attention table when worstClash is present (S2c-2)"
    );
    let _ = dock_port::stop();
}

#[test]
fn g1_dock_bind_is_lan_fixed_port() {
    assert_eq!(dock_port::DOCK_BIND, "0.0.0.0:18765");
}

#[test]
fn g2_get_root_without_token_is_html_shell_with_the_signed_age_strings() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, headers, body) = get(port, "/", None);
    assert_eq!(status, 200);
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: text/html"),
        "{headers}"
    );
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("Read from the PC at {when}."));
    assert!(text.contains("The PC has not answered yet. These numbers are from {when}."));
    assert!(text.contains(
        "Not connected to the PC. These numbers are from {when} and are not current. Dock again to refresh."
    ));
    assert!(text.contains(
        "Not connected to the PC. This phone has no farm numbers yet. Dock to the PC to read them."
    ));
    assert!(text.contains("Farm last evaluated on the PC at {when}."));
    assert!(text.contains("Farm not evaluated on the PC since it started."));
    assert!(!text.contains("{time}"), "the time-only token must be gone");
    let _ = dock_port::stop();
}

#[test]
fn g3_get_root_has_csp_and_no_store_and_no_cors() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, headers, _) = get(port, "/", None);
    assert_eq!(status, 200);
    let lower = headers.to_ascii_lowercase();
    assert!(!lower.contains("access-control"), "{headers}");
    assert!(lower.contains("content-security-policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'"), "{headers}");
    assert!(lower.contains("cache-control: no-store"), "{headers}");
    let _ = dock_port::stop();
}

#[test]
fn g4_shell_body_has_no_farm_data() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/", None);
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    for word in ["Unhealthy", "Degraded", "Healthy"] {
        assert!(!text.contains(word), "severity word {word} in shell");
    }
    for id in ["M1", "M2", "M3", "M4", "F1", "F2", "H2", "H3", "H4"] {
        assert!(!text.contains(id), "check id {id} in shell");
    }
    assert!(!text.contains("Dun peas"), "crop name in shell");
    let _ = dock_port::stop();
}

fn get_with_host(port: u16, path: &str, host: &str) -> (u16, String, Vec<u8>) {
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n");
    exchange(port, &req)
}

#[test]
fn g5_host_allow_list_is_literal_or_localhost() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, _) = get_with_host(port, "/", "192.168.1.10");
    assert_eq!(status, 200, "IPv4 literal");
    let (status, _, body) = get_with_host(port, "/", "evil.example");
    assert_eq!(status, 403, "DNS name");
    assert_eq!(body, br#"{"error":"Forbidden."}"#);
    let (status, _, _) = get_with_host(port, "/", "localhost");
    assert_eq!(status, 200, "localhost");
    let (status, _, _) = get_with_host(port, "/", "[::1]");
    assert_eq!(status, 200, "bracketed IPv6");
    let _ = dock_port::stop();
}

#[test]
fn g6_auth_still_holds_on_folds_and_ready() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(Arc::clone(&db));
    for path in ["/folds", "/ready"] {
        let (status, _, body) = get(port, path, None);
        assert_eq!(status, 401, "{path} no token");
        assert_eq!(body, br#"{"error":"Unauthorized."}"#, "{path} no token");
        let (status, _, body) = get(port, path, Some("00000000000000000000000000000000"));
        assert_eq!(status, 401, "{path} unknown");
        assert_eq!(body, br#"{"error":"Unauthorized."}"#, "{path} unknown");
    }
    {
        let conn = db.lock().unwrap();
        field_devices::retire_admin(&conn).unwrap();
    }
    for path in ["/folds", "/ready"] {
        let (status, _, body) = get(port, path, Some(&token));
        assert_eq!(status, 401, "{path} retired");
        assert_eq!(body, br#"{"error":"Unauthorized."}"#, "{path} retired");
    }
    let _ = dock_port::stop();
}

#[test]
fn g7_post_root_is_405() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let req = format!("POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Dock-Token: {token}\r\n\r\n");
    let (status, _, body) = exchange(port, &req);
    assert_eq!(status, 405);
    assert_eq!(body, br#"{"error":"Read only."}"#);
    let _ = dock_port::stop();
}

#[test]
fn g8_shell_places_the_one_glance_fields_in_signed_order() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/", None);
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    let line = text.find("id=\"line\"").expect("status line");
    let evaluated = text.find("id=\"evaluated\"").expect("evaluation line");
    let overall = text.find("id=\"overall\"").expect("overall");
    let clash = text.find("id=\"clash\"").expect("clash");
    let cards = text.find("id=\"cards\"").expect("cards");
    let pull = text.find("id=\"pull\"").expect("pull control");
    assert!(line < evaluated, "the status line reads first");
    assert!(
        evaluated < overall,
        "the evaluation line sits under the status line"
    );
    assert!(overall < clash, "overall sits above the attention line");
    assert!(clash < cards, "the attention line sits above the cards");
    assert!(cards < pull, "the pull control sits under the cards");
    let _ = dock_port::stop();
}

/// The CSP corrective left this as a one-time grep. It is load-bearing now that
/// the painters build elements: unsafe-inline is allowed, so textContent-only is
/// the whole defence.
#[test]
fn g9_shell_never_builds_markup_from_strings() {
    for hazard in [
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
    ] {
        assert!(
            !crate::dock_shell::SHELL.contains(hazard),
            "{hazard} in the shell - the page must only ever set textContent"
        );
    }
}

#[test]
fn h1_folds_checks_carry_severity_sentence_and_ran_at() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let obj = parsed.as_object().expect("object");
    // FI-1: e1 owns the top-level key set; this second count exists so a
    // check-row change cannot quietly add a top-level key. Both now assert
    // against PORT_DOC_KEYS, so they cannot drift again - they did at FI-1
    // and at FI-5.
    assert_eq!(
        obj.len(),
        PORT_DOC_KEYS,
        "no top-level key beyond the signed set"
    );
    let checks = obj
        .get("checks")
        .and_then(|c| c.as_array())
        .expect("checks array");
    assert_eq!(checks.len(), 9, "nine reported checks");
    for c in checks {
        let o = c.as_object().expect("check object");
        assert_eq!(
            o.len(),
            7,
            "checkId title scope card severity sentence ranAt"
        );
        for key in [
            "checkId", "title", "scope", "card", "severity", "sentence", "ranAt",
        ] {
            assert!(o.contains_key(key), "check is missing {key}");
        }
        assert!(
            o["severity"].is_string(),
            "the real path fills every severity - a null means a check had no row"
        );
        assert!(
            !o["sentence"].as_str().unwrap_or("").is_empty(),
            "the real path fills every sentence"
        );
    }
    let _ = dock_port::stop();
}

/// 2-b. The phone must emit the bytes the desk emits, so this reads the desk
/// source off disk (the g13 precedent) and refuses a second format.
#[test]
fn h2_shell_has_no_formatter_and_both_callers_carry_authority() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/farm/healthScopes.ts");
    let desk = std::fs::read_to_string(&path).expect("read healthScopes.ts");
    let shell = crate::dock_shell::SHELL;
    // FI-9 - the shell stopped being a second formatter. It prints what the PC
    // composed, so there is nothing here to drift from the desk.
    assert!(
        !shell.contains("function evidenceText"),
        "the shell formatter is gone"
    );
    assert!(
        !shell.contains("(ran_at "),
        "no check line is built in the shell"
    );
    assert!(!shell.contains("EV_HEAD"));
    assert!(
        shell.contains("doc.diagnosis"),
        "the shell prints the PC's block"
    );
    // Both callers carry the authority line, and it is one string in two places.
    assert!(
        desk.contains(crate::dock_folds::AUTHORITY_LINE),
        "desk lost AUTHORITY"
    );
    assert!(
        desk.contains("lines.push(AUTHORITY_LINE);"),
        "the desk must actually emit it, not merely declare it"
    );
    assert!(
        shell.is_ascii(),
        "dock_shell.rs must stay ascii - the paste transport flattens what is not"
    );
}

/// The shell is served over plain http on the LAN, which is not a secure
/// context. Every API below works at localhost and dies at the rack.
#[test]
fn h3_shell_diagnosis_block_is_select_all_and_uses_no_secure_context_api() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/", None);
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("id=\"evidence\""),
        "the diagnosis block must exist"
    );
    assert!(
        text.contains("user-select: all"),
        "the operator selects and copies by hand"
    );
    for banned in [
        "navigator.clipboard",
        "navigator.share",
        "createObjectURL",
        "download=",
    ] {
        assert!(
            !text.contains(banned),
            "{banned} needs a secure context the shell does not have"
        );
    }
    let _ = dock_port::stop();
}

/// FI-1 - the phone reads no clock of its own. Age is a PC-composed string.
#[test]
fn f1a_shell_reads_no_clock_of_its_own() {
    for hazard in [
        "new Date(",
        "Date.now(",
        "getHours(",
        "getMinutes(",
        "toLocale",
    ] {
        assert!(
            !crate::dock_shell::SHELL.contains(hazard),
            "{hazard} in the shell - every age must arrive as a PC string"
        );
    }
    assert!(crate::dock_shell::SHELL.contains("{when}"));
}

/// FI-1 - the frozen {when} pattern: ASCII, safe in an HTML grep, and carrying
/// a date so a day-old snapshot cannot read like a six-minute-old one.
#[test]
fn f1b_when_format_is_frozen_and_carries_a_date() {
    assert_eq!(crate::dock_folds::WHEN_FORMAT, "%a %b %-d, %-I:%M %P");
    let when = crate::dock_folds::compose_when("2026-08-21T20:04:00.000Z").expect("composes");
    assert!(when.is_ascii(), "{when}");
    for banned in ['<', '&', '"'] {
        assert!(!when.contains(banned), "{when}");
    }
    let a = crate::dock_folds::compose_when("2026-08-21T20:04:00.000Z").unwrap();
    let b = crate::dock_folds::compose_when("2026-08-22T20:04:00.000Z").unwrap();
    assert_ne!(a, b, "the pattern must carry a date, not only a time");
    assert!(crate::dock_folds::compose_when("not an instant").is_none());
}

/// FI-1 (F-c) - the stamp is written in exactly one place and the port's reader
/// is not it. Source scan in the h2 / g13 style: the stamp is process global and
/// the suite runs concurrently, so a behavioural equality check would race.
#[test]
fn f1c_evaluation_stamp_is_written_only_inside_check_attention() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/attention.rs");
    let src = std::fs::read_to_string(&path).expect("read attention.rs");
    assert_eq!(
        src.matches("record_evaluation(").count(),
        2,
        "record_evaluation must be defined once and called once"
    );
    let funnel = src
        .find("pub fn check_attention(")
        .expect("check_attention exists");
    let funnel_body = &src[funnel..];
    let funnel_end = funnel_body.find("\n}").expect("check_attention body");
    assert!(
        funnel_body[..funnel_end].contains("record_evaluation("),
        "check_attention is the funnel that writes the stamp"
    );
    let open = src.find("pub fn open_items(").expect("open_items exists");
    let open_body = &src[open..];
    let open_end = open_body.find("\n}").expect("open_items body");
    assert!(
        !open_body[..open_end].contains("record_evaluation"),
        "open_items must never stamp - it is the port's reader"
    );
    let port = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dock_port.rs");
    let port_src = std::fs::read_to_string(&port).expect("read dock_port.rs");
    assert!(
        !port_src.contains("check_attention"),
        "the port must never reach the evaluating reader"
    );
}

/// FI-1 - the document reports the stamp it read. It never sets the evaluation
/// time from the moment it was served.
#[test]
fn f1d_folds_reports_the_evaluation_stamp_and_never_the_served_time() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    {
        let conn = db.lock().unwrap();
        crate::attention::check_attention(&conn).unwrap();
    }
    let port = start_clean(Arc::clone(&db));
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let evaluated = parsed["attentionEvaluatedAt"]
        .as_str()
        .expect("stamped in this process");
    chrono::DateTime::parse_from_rfc3339(evaluated).expect("RFC3339");
    assert_ne!(
        parsed["attentionEvaluatedAt"], parsed["servedAt"],
        "the evaluation stamp must be read, never set from the serve"
    );
    assert!(parsed["attentionEvaluatedAtDisplay"].is_string());
    assert!(parsed["servedAtDisplay"].is_string());
    let _ = dock_port::stop();
}

/// FI-2 - one load path. A second fetch would be a second freshness rule.
#[test]
fn f2a_shell_has_exactly_one_fetch() {
    assert_eq!(
        crate::dock_shell::SHELL.matches("fetch(").count(),
        1,
        "one door to /folds - submit, Pull now, the tick and visibility all reach it"
    );
}

/// FI-2 - the controls exist, and adding them did not put a clock back in the
/// phone. f1a must keep holding.
#[test]
fn f2b_pull_now_and_gated_auto_pull_exist_without_a_clock() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains(">Pull now<"), "the signed label");
    assert!(shell.contains("id=\"pull\""));
    assert!(shell.contains("setInterval("));
    assert!(shell.contains("visibilitychange"));
    assert!(shell.contains("visibilityState"));
    for hazard in [
        "new Date(",
        "Date.now(",
        "getHours(",
        "getMinutes(",
        "toLocale",
    ] {
        assert!(
            !shell.contains(hazard),
            "{hazard} - FI-1 f1a must keep holding"
        );
    }
}

/// FI-2 - the interval cannot stack. `arm` is the only site that starts a
/// timer and it always stops one first, so repeated Remember presses cannot
/// leave the phone pulling several times a minute.
#[test]
fn f2c_the_interval_is_armed_once_and_never_stacks() {
    let shell = crate::dock_shell::SHELL;
    assert_eq!(shell.matches("setInterval(").count(), 1, "one arming site");
    let arm = shell.find("function arm(").expect("arm exists");
    let tail = &shell[arm..];
    let end = tail.find("\n}").expect("arm body");
    let body = &tail[..end];
    assert!(
        body.contains("clearInterval(ticker)"),
        "arm disarms before it arms"
    );
    assert!(body.contains("setInterval("), "arm is the arming site");
    assert!(
        shell.contains("function disarm("),
        "forget can stop the pulling"
    );
}

/// FI-2 - one pull at a time. A slow pull must not be overtaken by a fast one
/// and repainted with an older servedAt.
#[test]
fn f2d_load_refuses_to_overlap_itself() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("if (inFlight) return;"), "re-entry guard");
    assert!(shell.contains("inFlight = true;"));
    assert!(shell.contains("inFlight = false;"));
}

/// FI-2 - the age still comes only from the document. FI-2 adds no new writer
/// of either age line and no new source for {when}.
#[test]
fn f2e_fi2_adds_no_new_writer_of_the_two_ages() {
    let shell = crate::dock_shell::SHELL;
    assert_eq!(
        shell.matches("getElementById(\"line\")").count(),
        1,
        "showLine is the only writer of the served-age line"
    );
    assert_eq!(
        shell.matches("getElementById(\"evaluated\")").count(),
        2,
        "showEvaluated and clearNumbers are the only writers of the evaluation line"
    );
    assert!(shell.contains("withWhen(S1, doc.servedAtDisplay)"));
    assert!(shell.contains("withWhen(template, held.servedAtDisplay)"));
    assert!(shell.contains("doc.attentionEvaluatedAtDisplay"));
}

/// The TITLES object literal, for the title tests. Same body-scan style as f2c.
fn titles_block() -> &'static str {
    let s = crate::dock_shell::SHELL;
    let start = s.find("var TITLES = {").expect("TITLES map");
    let rest = &s[start..];
    let end = rest.find("};").expect("TITLES map end");
    &rest[..end]
}

/// FI-4 - six display titles, one per wire key, and no seventh.
#[test]
fn f4a_title_map_has_exactly_the_six_signed_titles() {
    let block = titles_block();
    for pair in [
        "money: \"Money\"",
        "cover: \"Cover\"",
        "promise: \"Promise\"",
        "rack: \"Rack\"",
        "phone_queue: \"Phone queue\"",
        "system: \"Health\"",
    ] {
        assert!(block.contains(pair), "missing title entry {pair}");
    }
    assert_eq!(
        block.matches(": \"").count(),
        6,
        "exactly six titles - a seventh would be an unsigned label"
    );
}

/// FI-4 - the card row renders the title, not the wire key, and the severity
/// suppression still keys off the wire id.
#[test]
fn f4b_card_rows_render_the_title_not_the_key() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("label.textContent = titleFor(row.card);"));
    assert!(
        !shell.contains("label.textContent = row.card;"),
        "the raw-key render is gone"
    );
    assert!(
        shell.contains("row.card !== \"phone_queue\""),
        "severity suppression must key off the wire id, not the title"
    );
}

/// FI-4 - a title must never read as a severity word or a check id, or it would
/// defeat g4's proof that the shell carries no farm data. "Health" sits one
/// character from "Healthy"; this keeps that distance on purpose.
#[test]
fn f4c_titles_do_not_collide_with_farm_words() {
    for title in ["Money", "Cover", "Promise", "Rack", "Phone queue", "Health"] {
        for word in ["Unhealthy", "Degraded", "Healthy"] {
            assert!(
                !title.contains(word),
                "title {title} contains severity word {word}"
            );
        }
        for id in ["M1", "M2", "M3", "M4", "F1", "F2", "H2", "H3", "H4"] {
            assert!(!title.contains(id), "title {title} contains check id {id}");
        }
        assert!(title.is_ascii(), "{title}");
    }
}

/// FI-4b - the card row's key set, frozen the way h1 freezes the check row. A
/// seventh key would be an unsigned field on the face.
#[test]
fn f4d_card_rows_carry_the_signed_key_set() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let obj = parsed.as_object().expect("object");
    assert_eq!(
        obj.len(),
        PORT_DOC_KEYS,
        "no top-level key beyond the signed set"
    );
    let cards = obj
        .get("cards")
        .and_then(|c| c.as_array())
        .expect("cards array");
    assert_eq!(cards.len(), 6, "six nodes");
    for c in cards {
        let o = c.as_object().expect("card object");
        assert_eq!(
            o.len(),
            6,
            "card severity checkIds sentence oldestRanAt oldestRanAtDisplay"
        );
        for key in [
            "card",
            "severity",
            "checkIds",
            "sentence",
            "oldestRanAt",
            "oldestRanAtDisplay",
        ] {
            assert!(o.contains_key(key), "card is missing {key}");
        }
    }
    let queue = cards
        .iter()
        .find(|c| c["card"] == "phone_queue")
        .expect("phone_queue card");
    assert!(queue["severity"].is_null(), "S5 - no invented severity");
    assert!(
        queue["sentence"].is_null(),
        "Q-1b - the queue face reads phoneQueue.sentence, not a second copy"
    );
    assert!(queue["oldestRanAt"].is_null(), "no checks, so no age");
    // A-1 - an age reaches the wire only when it is older than the serve, and
    // never without the PC-composed display beside it.
    let served = obj["servedAt"].as_str().expect("servedAt");
    let s = chrono::DateTime::parse_from_rfc3339(served).expect("servedAt is RFC3339");
    for c in cards {
        match c["oldestRanAt"].as_str() {
            Some(at) => {
                let a = chrono::DateTime::parse_from_rfc3339(at).expect("oldestRanAt is RFC3339");
                assert!(a < s, "an age equal to the serve is not an age");
                assert!(
                    c["oldestRanAtDisplay"].is_string(),
                    "an instant on the wire must arrive with its display string"
                );
            }
            None => assert!(c["oldestRanAtDisplay"].is_null()),
        }
    }
    let _ = dock_port::stop();
}

/// S-1 - the card age line, frozen. FI-4b shipped `CARD_AGE` as signed operator
/// text but left it without a guard; this is that guard.
///
/// Two halves, because they can drift apart: the wording lives in the constant,
/// and the face must still read THAT constant against THAT wire field. Inlining
/// a different literal, or pointing the age at some other value, each turn this
/// red on its own.
///
/// What is deliberately NOT asserted (S-1b): that only Health carries an age.
/// The shell has no per-card rule - line 456 is gated on `oldestRanAtDisplay`
/// alone. What confines the age today is a PC fact, that M1-M4 / F1 / F2 stamp
/// the read instant by design, and f4d already freezes it. Asserting "Health
/// only" here would freeze an accident of today's data, not a signed rule.
///
/// `{when}` itself is not shell text: f1b freezes WHEN_FORMAT, f1a forbids a
/// phone clock, and f4d forbids an instant reaching the wire without its
/// PC-composed display. This test owns only the sentence around it.
#[test]
fn f4g_card_age_line_is_the_signed_string() {
    let shell = crate::dock_shell::SHELL;
    assert!(
        shell.contains("var CARD_AGE = \"Last reported {when}.\";"),
        "S-1: the signed age wording"
    );
    assert!(
        shell.contains("age.textContent = withWhen(CARD_AGE, row.oldestRanAtDisplay);"),
        "S-1: the face must read the constant against the wire field"
    );
    assert!(
        shell.is_ascii(),
        "a literal glyph would break the paste transport"
    );
    // The shell composes no age of its own - every one of these would be the
    // phone wording a fact the PC already worded.
    for owned in ["Last read", "Reported ", " ago", "minutes old", "hours old"] {
        assert!(
            !shell.contains(owned),
            "shell must not compose an age: {owned}"
        );
    }
}

/// FI-5 - worstClash is the head of clashes[], from one read. If these two ever
/// disagree the pulse line is describing a clash the set does not contain.
#[test]
fn f5d_worst_clash_is_the_head_of_clashes() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let clashes = parsed["clashes"].as_array().expect("clashes array");
    if clashes.is_empty() {
        assert!(parsed["worstClash"].is_null(), "no clashes, no worst");
    } else {
        assert_eq!(parsed["worstClash"], clashes[0], "worstClash is the head");
        let mut last = i64::MIN;
        for c in clashes {
            let rank = c["rank"].as_i64().expect("rank");
            assert!(rank >= last, "clashes are ranked");
            last = rank;
            assert!(
                !c["sentence"].as_str().unwrap_or("").is_empty(),
                "every clash carries a PC sentence"
            );
        }
    }
    let _ = dock_port::stop();
}

/// FI-5 - the shell prints the sentence and composes nothing. The `source`
/// prefix and the bare-key fallback are gone.
#[test]
fn f5e_shell_prints_the_clash_sentence_and_composes_nothing() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("clash.textContent = doc.worstClash.sentence;"));
    assert!(
        !shell.contains("src + \" \" + sent"),
        "the source prefix composition is gone"
    );
    assert!(
        !shell.contains("doc.worstClash.source"),
        "source is wire-only - it must never reach the face"
    );
}

/// FI-6 - the two signed strings, verbatim, and the arrow as an escape.
#[test]
fn f6a_edge_list_carries_the_signed_strings() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("var EDGES_HEAD = \"Pulling against each other\";"));
    assert!(shell.contains("var EDGES_NONE = \"Nothing is pulling against anything right now.\";"));
    assert!(
        shell.contains("var ARROW = \" \\u2192 \";"),
        "arrow is an escape"
    );
    assert!(
        shell.is_ascii(),
        "a literal arrow would break the paste transport"
    );
    for word in ["Unhealthy", "Degraded", "Healthy"] {
        assert!(!shell.contains(word), "no severity word - g4 owns that ban");
    }
}

/// FI-6 - both ends of every edge go through titleFor, so no raw wire key can
/// reach the face, and no title is composed here.
#[test]
fn f6b_edge_rows_render_titles_not_keys() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("titleFor(c.cards[0]) + ARROW + titleFor(c.cards[1])"));
    assert!(
        !shell.contains("c.cards[0] + ARROW"),
        "a raw key must never be concatenated into a row"
    );
}

/// FI-6 - dedup by ordered pair, and the worst pair carries the emphasis.
#[test]
fn f6c_edges_are_deduplicated_and_the_worst_is_marked() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("Object.prototype.hasOwnProperty.call(seen, key)"));
    assert!(shell.contains("key === worst ? \"edge worst\" : \"edge\""));
    assert!(
        shell.contains("if (!c.cards || c.cards.length !== 2) continue;"),
        "a clash with no pair draws nothing"
    );
    assert!(
        shell.contains("EDGES_NONE"),
        "the empty state is drawn, not blank"
    );
}

/// FI-6 (G-4) - one paint path. The edges are painted only alongside the
/// numbers, so they are always exactly as stale as the face above them.
#[test]
fn f6d_edges_are_painted_only_with_the_numbers() {
    let shell = crate::dock_shell::SHELL;
    assert_eq!(
        shell.matches("showEdges(").count(),
        3,
        "definition + paintOk + paintHeld, and nothing else"
    );
    assert!(shell.contains("showEdges(doc);"));
    assert!(shell.contains("showEdges(held);"));
    assert!(shell.contains("document.getElementById(\"edges\").textContent = \"\";"));
}

/// FI-6 - the section's place on the face, and the standing promise that the
/// six-node picture is still owed. FI-6b brings SVG; FI-6 must not.
#[test]
fn f6e_edge_section_sits_under_the_cards_and_carries_no_svg() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/", None);
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    let cards = text.find("id=\"cards\"").expect("cards");
    let head = text.find("id=\"edgesHead\"").expect("edges heading");
    let edges = text.find("id=\"edges\"").expect("edges");
    let pull = text.find("id=\"pull\"").expect("pull control");
    assert!(cards < head, "the edges sit under the cards");
    assert!(head < edges, "the heading reads before its rows");
    assert!(edges < pull, "the pull control stays last");
    // FI-6b landed the schematic. `<svg` stays banned: the picture is built
    // through the DOM, never from a markup string (g9's rule).
    assert!(
        !text.contains("<svg"),
        "<svg - the picture is built through the DOM, never from a string"
    );
    let _ = dock_port::stop();
}

/// FI-6b - the picture's place on the face, and the standing rule that it is
/// drawn through the DOM. It is not a control: the button count is unmoved.
#[test]
fn f6f_schematic_sits_between_the_heading_and_the_rows() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/", None);
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    let cards = text.find("id=\"cards\"").expect("cards");
    let head = text.find("id=\"edgesHead\"").expect("edges heading");
    let schematic = text.find("id=\"schematic\"").expect("schematic");
    let edges = text.find("id=\"edges\"").expect("edges");
    let pull = text.find("id=\"pull\"").expect("pull control");
    assert!(cards < head, "the picture still sits under the cards");
    assert!(head < schematic, "the heading reads before the picture");
    assert!(schematic < edges, "the picture reads before its text rows");
    assert!(edges < pull, "the pull control stays last");
    assert_eq!(
        text.matches("<button").count(),
        3,
        "Pull now, Remember, Forget token - the picture is not a control"
    );
    let _ = dock_port::stop();
}

/// FI-6b (G'-3) - the geometry is a constant. The shell measures no viewport,
/// so no farm number and no phone dimension can reach a coordinate. Freezing
/// the six signed positions here means a nudged node is a signed change.
#[test]
fn f6g_schematic_geometry_is_constant_and_measures_nothing() {
    let shell = crate::dock_shell::SHELL;
    for hazard in [
        "getBoundingClientRect",
        "offsetWidth",
        "clientWidth",
        "innerWidth",
        "getComputedStyle",
    ] {
        assert!(
            !shell.contains(hazard),
            "{hazard} - the picture must measure nothing"
        );
    }
    assert!(shell.contains(
        "var RING = [\"system\", \"money\", \"promise\", \"cover\", \"rack\", \"phone_queue\"];"
    ));
    for node in [
        "system: { x: 200, y: 48",
        "money: { x: 280, y: 94",
        "promise: { x: 280, y: 186",
        "cover: { x: 200, y: 232",
        "rack: { x: 120, y: 186",
        "phone_queue: { x: 120, y: 94",
    ] {
        assert!(
            shell.contains(node),
            "NODES lost the signed position {node}"
        );
    }
    assert!(
        shell.contains("svg.setAttribute(\"viewBox\", \"0 0 400 288\");"),
        "the signed viewBox"
    );
    assert!(shell.is_ascii(), "no glyph may enter the picture");
}

/// FI-6b (G'-9) - one paint, one worst. `drawSchematic` is defined once and
/// called once, from inside `showEdges`, and is handed the key that already
/// marked the worst row. Node labels go through titleFor, so no wire key can
/// reach the face. G'-6a: static emphasis, never motion.
#[test]
fn f6h_schematic_shares_one_paint_and_composes_nothing() {
    let shell = crate::dock_shell::SHELL;
    assert_eq!(
        shell.matches("drawSchematic(").count(),
        2,
        "definition + the single call inside showEdges"
    );
    assert!(shell.contains("drawSchematic(pairs, worst);"));
    assert!(
        shell.contains("label.textContent = titleFor(RING[j]);"),
        "node labels are FI-4 titles, never wire keys"
    );
    assert!(
        shell.contains("var cx = (x1 + x2) / 2 - uy * bow;"),
        "G'-5 - the consistent perpendicular that keeps a>b and b>a apart"
    );
    assert!(
        shell.contains("lead ? \"3\" : \"1.5\""),
        "G'-6a - static emphasis on the lead edge"
    );
    for banned in ["animate", "keyframes", "requestAnimationFrame"] {
        assert!(
            !shell.contains(banned),
            "{banned} - G'-6a signed static, not motion"
        );
    }
    assert!(
        shell.contains("document.getElementById(\"schematic\").textContent = \"\";"),
        "clearNumbers must wipe the picture, or it outlives the numbers"
    );
}

/// FI-7 - the signed heading, and no severity word smuggled in with it.
#[test]
fn f7a_queue_section_carries_the_signed_heading() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("var QUEUE_HEAD = \"Waiting for Confirm on the PC\";"));
    assert!(shell.is_ascii());
    for word in ["Unhealthy", "Degraded", "Healthy"] {
        assert!(!shell.contains(word), "g4 owns that ban");
    }
}

/// FI-7 - the phone prints; it never counts, pluralises or composes.
#[test]
fn f7b_queue_text_is_printed_not_composed() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("count.textContent = q && q.sentence ? q.sentence : \"\";"));
    assert!(shell.contains("row.textContent = rows[i].sentence;"));
    assert!(
        !shell.contains("pendingCount"),
        "the count reaches the face as a PC sentence, never as a number the phone words"
    );
    assert!(
        !shell.contains("rows.length +"),
        "no arithmetic into operator text"
    );
}

/// FI-7 - one paint path, so the queue is exactly as stale as the face.
#[test]
fn f7c_queue_is_painted_only_with_the_numbers() {
    let shell = crate::dock_shell::SHELL;
    assert_eq!(
        shell.matches("showQueue(").count(),
        3,
        "definition + paintOk + paintHeld, and nothing else"
    );
    assert!(shell.contains("document.getElementById(\"queue\").textContent = \"\";"));
}

/// FI-7 - the wire shape, and what is deliberately absent from it.
#[test]
fn f7d_phone_queue_is_read_only_on_the_wire() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let q = parsed["phoneQueue"].as_object().expect("phoneQueue object");
    assert!(q.contains_key("pendingCount"));
    assert!(q.contains_key("sentence"));
    assert!(q.contains_key("rows"));
    assert_eq!(q.len(), 3, "count, sentence, rows - nothing else");
    let n = q["pendingCount"].as_i64().expect("pendingCount");
    let rows = q["rows"].as_array().expect("rows");
    assert_eq!(
        n as usize,
        rows.len(),
        "the count is the rows, not a second fact"
    );
    assert!(
        !q["sentence"].as_str().unwrap_or("").is_empty(),
        "the empty queue says so in words rather than vanishing"
    );
    for r in rows {
        let o = r.as_object().expect("row object");
        assert_eq!(o.len(), 3, "proposalId, capturedAt, sentence");
        assert!(!o["sentence"].as_str().unwrap_or("").is_empty());
        for banned in ["gateReason", "outcome", "decidedAt", "verdict"] {
            assert!(
                !o.contains_key(banned),
                "{banned} - no decision on this wire"
            );
        }
    }
    // S5 stands.
    let cards = parsed["cards"].as_array().expect("cards");
    let pq = cards
        .iter()
        .find(|c| c["card"] == "phone_queue")
        .expect("phone_queue card");
    assert!(pq["severity"].is_null(), "S5 - no invented severity");
    let _ = dock_port::stop();
}

/// FI-7 - the section's place, and the standing rule that Confirm is not here.
#[test]
fn f7e_queue_sits_under_the_edges_and_offers_no_control() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/", None);
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    let edges = text.find("id=\"edges\"").expect("edges");
    let head = text.find("id=\"queueHead\"").expect("queue heading");
    let queue = text.find("id=\"queue\"").expect("queue");
    let pull = text.find("id=\"pull\"").expect("pull control");
    assert!(edges < head, "the queue sits under the edge list");
    assert!(head < queue);
    assert!(queue < pull, "the pull control stays last");
    assert_eq!(
        text.matches("<button").count(),
        3,
        "Pull now, Remember, Forget token - and nothing that could decide a capture"
    );
    for banned in ["<form id=\"queue", "onclick", "<svg"] {
        assert!(!text.contains(banned), "{banned}");
    }
    let _ = dock_port::stop();
}

/// FI-8 - the two signed strings, and no severity word smuggled in with them.
#[test]
fn f8a_task_mirror_carries_the_signed_strings() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("var TASKS_HEAD = \"What Today is asking for\";"));
    assert!(shell.contains("var TASKS_NONE = \"Today's queue is clear.\";"));
    assert!(shell.is_ascii());
    for word in ["Unhealthy", "Degraded", "Healthy"] {
        assert!(!shell.contains(word), "g4 owns that ban");
    }
}

/// FI-8 - the phone prints PC sentences in the PC's order. It does not rank,
/// sort, filter by rank, or compose.
#[test]
fn f8b_tasks_are_printed_in_wire_order() {
    let shell = crate::dock_shell::SHELL;
    // FI-8b numbered the rows, so the render expression moved. What this test
    // is named for is unchanged and still asserted below: the phone reads only
    // `.sentence` and never sorts. The ordinal is the array position.
    assert!(shell.contains("row.textContent = ordinal + \". \" + said[j];"));
    assert!(shell.contains("var s = list[i].sentence;"));
    let start = shell.find("function showTasks(").expect("showTasks");
    let tail = &shell[start..];
    let end = tail.find("\n}").expect("showTasks body");
    let body = &tail[..end];
    for hazard in [".sort(", ".rank", "today_rank", ".reverse("] {
        assert!(
            !body.contains(hazard),
            "{hazard} - the PC ranks, the phone prints"
        );
    }
}

/// FI-8 - one paint path, so the mirror is exactly as stale as the face.
#[test]
fn f8c_tasks_are_painted_only_with_the_numbers() {
    let shell = crate::dock_shell::SHELL;
    assert_eq!(
        shell.matches("showTasks(").count(),
        3,
        "definition + paintOk + paintHeld, and nothing else"
    );
    assert!(shell.contains("document.getElementById(\"tasks\").textContent = \"\";"));
}

/// FI-8 - the section's place, and the standing rule that nothing here is a
/// control. Completing work is a PC action; the phone shows what is asked.
#[test]
fn f8d_tasks_sit_between_the_cards_and_the_edges_with_no_control() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/", None);
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    let cards = text.find("id=\"cards\"").expect("cards");
    let head = text.find("id=\"tasksHead\"").expect("tasks heading");
    let tasks = text.find("id=\"tasks\"").expect("tasks");
    let edges = text.find("id=\"edgesHead\"").expect("edges heading");
    assert!(cards < head, "the tasks sit under the cards");
    assert!(head < tasks);
    assert!(tasks < edges, "the edge list stays below the tasks");
    assert_eq!(
        text.matches("<button").count(),
        3,
        "Pull now, Remember, Forget token - nothing that could complete a task"
    );
    for banned in [
        "<input type=\"checkbox",
        "mark done",
        "Mark done",
        "onclick",
    ] {
        assert!(!text.contains(banned), "{banned}");
    }
    let _ = dock_port::stop();
}

/// FI-8b - the ordering language, and the line it must not cross.
///
/// The mirror reads exactly one field off each clash: `sentence`. `rank` is on
/// the wire and decides the order, and it is never printed - the operator needs
/// to know what comes first, not what number the PC gave it. The ordinal is the
/// array position, so the only arithmetic on this path is over a list the PC
/// already sorted.
///
/// "Start here." is the single new string F8b-1 allowed, on the finding that
/// the sentences already name the work and what was missing was ordering.
#[test]
fn f8e_task_mirror_numbers_the_rows_and_prints_no_rank() {
    let shell = crate::dock_shell::SHELL;
    assert!(
        shell.contains("var TASKS_FIRST = \"Start here.\";"),
        "F8b-1: the one signed string"
    );
    assert!(
        shell.contains("var ordinal = j + 1;") && shell.contains("ordinal + \". \" + said[j]"),
        "the number is the array position, not a wire value"
    );
    // Only `sentence` reaches the face. Nothing else on a clash may.
    assert!(shell.contains("var s = list[i].sentence;"));
    for field in [
        "list[i].rank",
        "list[i].kind",
        "list[i].source",
        "list[i].cards",
    ] {
        assert!(!shell.contains(field), "the mirror must not print {field}");
    }
    // A clear queue gets no instruction: the empty state returns before
    // TASKS_FIRST is ever appended.
    let none_at = shell
        .find("none.textContent = TASKS_NONE;")
        .expect("empty state");
    let first_at = shell
        .find("first.textContent = TASKS_FIRST;")
        .expect("first line");
    assert!(
        none_at < first_at,
        "the empty state must return before the instruction"
    );
    assert!(shell.is_ascii());
}

/// FI-9 - one authority string, two callers, no second wording.
#[test]
fn f9a_authority_line_is_identical_on_both_callers() {
    assert_eq!(
        crate::dock_folds::AUTHORITY_LINE,
        "AUTHORITY: PC sole writer. Snapshot only. Do not invent numbers."
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/farm/healthScopes.ts");
    let desk = std::fs::read_to_string(&path).expect("read healthScopes.ts");
    assert!(desk.contains(crate::dock_folds::AUTHORITY_LINE));
}

/// FI-9 - the export control did not change. Select-all, no secure-context API.
#[test]
fn f9b_export_control_is_unchanged_select_all() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/", None);
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("id=\"evidence\""));
    assert!(text.contains("user-select: all"));
    assert_eq!(text.matches("<button").count(), 3, "no new control");
    for banned in [
        "navigator.clipboard",
        "navigator.share",
        "createObjectURL",
        "download=",
    ] {
        assert!(
            !text.contains(banned),
            "{banned} needs a secure context we do not have"
        );
    }
    let _ = dock_port::stop();
}

/// FI-9 - the block on the wire: summary first, authority, then the packet, and
/// the summary's severity word is the one the JSON carries.
#[test]
fn f9c_diagnosis_block_is_summary_then_authority_then_packet() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let d = parsed["diagnosis"].as_str().expect("diagnosis");
    assert!(
        d.starts_with(crate::dock_folds::SUMMARY_HEAD),
        "summary first"
    );
    let auth = d
        .find(crate::dock_folds::AUTHORITY_LINE)
        .expect("authority line");
    let head = d
        .find(crate::dock_folds::DIAGNOSIS_HEAD)
        .expect("evidence header");
    let summary_end = d.find('\n').expect("more than one line");
    assert!(summary_end < head, "the summary reads before the packet");
    assert!(head < auth, "authority sits with the packet it governs");
    if let Some(word) = parsed["overall"].as_str() {
        assert!(
            d.contains(&format!("Overall: {word}")),
            "the summary word must be the word the JSON carries"
        );
    }
    for id in ["M1", "M2", "M3", "M4", "F1", "F2", "H2", "H3", "H4"] {
        assert!(d.contains(&format!("{id} [")), "packet is missing {id}");
    }
    let _ = dock_port::stop();
}

/// FI-9 - the phone reports what it was given. No wording is built there.
#[test]
fn f9d_shell_prints_the_block_and_builds_none_of_it() {
    let shell = crate::dock_shell::SHELL;
    assert!(shell.contains("doc.diagnosis ? doc.diagnosis : \"\""));
    for hazard in ["AUTHORITY", "field summary", "health evidence", "Overall: "] {
        assert!(
            !shell.contains(hazard),
            "{hazard} - the PC owns that wording"
        );
    }
}

/// FI-9b - the packet's shared vocabulary, frozen across the language line.
///
/// There are two composers by design (F9b-2b): `dock_folds::diagnosis_text` for
/// the phone, `healthScopes::evidenceText` for the desk. They are NOT the same
/// packet - the desk carries scan lines the wire does not have, and the phone
/// carries a field summary and pull health the desk does not show. Merging them
/// would cost a scan field on the document to serve a symmetry neither audience
/// asked for.
///
/// What they DO share is four strings, and dock_folds.rs claims one of them is
/// copied "byte for byte". f9a already holds AUTHORITY_LINE. This holds the
/// other three, and holds the citation itself: if healthScopes.ts is renamed or
/// evidenceText moves, the claim goes red instead of going quietly stale - the
/// D-5 defect, where a comment named an address that no longer held the logic.
///
/// Same mechanism as f9a and h2: Rust reads the TypeScript file. No npm.
#[test]
fn f9e_packet_vocabulary_is_identical_on_both_callers() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/farm/healthScopes.ts");
    let desk = std::fs::read_to_string(&path).expect("read healthScopes.ts");
    let rust = include_str!("dock_folds.rs");

    // 1. The packet header is one string in two places.
    assert!(
        desk.contains(crate::dock_folds::DIAGNOSIS_HEAD),
        "the desk's evidence header drifted from DIAGNOSIS_HEAD"
    );

    // 2. The check line: same three fields in the same order, on both sides.
    assert!(
        desk.contains("${s.checkId} [${s.severity}] ${s.sentence} (ran_at "),
        "the desk's check line changed shape"
    );
    assert!(
        rust.contains("\"{} [{}] {} (ran_at {})\""),
        "the PC's check line changed shape"
    );

    // 3. The absent-evidence fallback, both sides.
    assert!(
        desk.contains("${s.ranAt ?? \"never reported\"}"),
        "the desk's ran_at fallback moved"
    );
    assert!(
        rust.contains("\"never reported\""),
        "the PC's ran_at fallback moved"
    );

    // 4. The citation. A claim of byte-for-byte parity has to keep naming the
    //    file it claims parity with, or it is only a comment.
    assert!(
        rust.contains("The desk's check line, byte for byte (healthScopes.ts evidenceText)."),
        "the parity citation moved - re-point it at the composer it now copies"
    );
}

/// FI-10 - a link, not a control. The button count is the invariant.
#[test]
fn f10a_capture_link_is_an_anchor_not_a_button() {
    let _suite = lock_suite();
    let (db, _) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/", None);
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("<a id=\"capture\" rel=\"noreferrer\">Capture something</a>"));
    assert_eq!(text.matches("<button").count(), 3, "still three buttons");
    assert!(
        text.contains("#capture:not([href])"),
        "hidden until it has a target"
    );
    let _ = dock_port::stop();
}

/// FI-10 - the wire carries an origin, never a URL and never a token.
#[test]
fn f10b_capture_endpoint_is_an_origin_with_no_token() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let v = &parsed["captureEndpoint"];
    assert!(v.is_string() || v.is_null(), "origin or nothing");
    if let Some(e) = v.as_str() {
        assert!(
            !e.contains("/a/"),
            "that is the pairing path, not an origin"
        );
        assert!(
            !e.contains('?'),
            "no query - crop names ride the pairing link only"
        );
        assert!(!e.contains(&token), "the token must never reach the wire");
        assert!(
            !e.ends_with('/'),
            "trailing slash is trimmed so the join is exact"
        );
    }
    let _ = dock_port::stop();
}

/// FI-10 - the export block never learns the endpoint. FI-9 made this document
/// pasteable; the nav target stays out of what gets pasted.
#[test]
fn f10c_diagnosis_never_carries_the_endpoint_or_the_token() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let d = parsed["diagnosis"].as_str().expect("diagnosis");
    assert!(
        !d.contains(&token),
        "the token must never enter the export block"
    );
    assert!(!d.contains("/a/"), "no capture path in the export block");
    if let Some(e) = parsed["captureEndpoint"].as_str() {
        assert!(
            !d.contains(e),
            "the endpoint must not travel in the export block"
        );
    }
    let _ = dock_port::stop();
}

/// FI-10 - the phone assembles the URL from what it already holds, on the one
/// paint path, and nothing else builds it.
#[test]
fn f10d_shell_joins_the_origin_with_its_own_token() {
    let shell = crate::dock_shell::SHELL;
    assert_eq!(
        shell.matches("showCapture(").count(),
        3,
        "definition + paintOk + paintHeld, and nothing else"
    );
    assert!(shell.contains("var token = localStorage.getItem(KEY) || \"\";"));
    assert!(shell.contains("endpoint + \"/a/\" + encodeURIComponent(token)"));
    assert!(shell.contains("a.removeAttribute(\"href\");"));
    assert!(
        !shell.contains("target=\"_blank\""),
        "plain navigation only"
    );
}

/// FI-10b - the pull's own age, the same shape as every other age on this
/// surface: the PC composes the words, the phone substitutes them. The ban
/// list is f4g's, repeated here because a new line is a new chance to compose.
#[test]
fn f10f_pull_age_line_is_the_signed_string() {
    let _suite = lock_suite();
    let (db, token) = paired_db();
    let port = start_clean(db);
    let (status, _, body) = get(port, "/folds", Some(&token));
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("folds JSON");
    let v = &parsed["lastPullAtDisplay"];
    assert!(
        v.is_string() || v.is_null(),
        "a PC-composed instant, or nothing"
    );
    let shell = crate::dock_shell::SHELL;
    assert!(
        shell.contains("var PULL_AGE = \"Last checked {when}.\";"),
        "F10b-1(b-i): the signed wording"
    );
    assert!(
        shell.contains("var when = doc.lastPullAtDisplay;"),
        "the line must read the wire field, not a clock"
    );
    assert!(
        shell.contains("w.textContent = withWhen(PULL_AGE, when);"),
        "the constant is substituted, never assembled"
    );
    for owned in ["Last read", "Reported ", " ago", "minutes old", "hours old"] {
        assert!(
            !shell.contains(owned),
            "shell must not compose an age: {owned}"
        );
    }
    assert!(shell.is_ascii(), "shell stays ASCII");
    let _ = dock_port::stop();
}

/// FI-10b - the way back. Returning from the capture page can restore this
/// document from the browser's cache, and only pageshow fires on that path.
/// Both doors call `tick`, so the signed FI-2 backoff still decides whether a
/// pull happens: F10b-2(c) was declined and Back is not a fourth control.
#[test]
fn f10g_the_return_from_capture_repaints_through_tick() {
    let shell = crate::dock_shell::SHELL;
    assert!(
        shell.contains("window.addEventListener(\"pageshow\", tick);"),
        "F10b-2(b): the cache-restore path"
    );
    assert!(
        shell.contains("document.addEventListener(\"visibilitychange\", tick);"),
        "FI-2's listener still stands beside it"
    );
    assert_eq!(
        shell.matches(", tick);").count(),
        2,
        "exactly two return doors, and both go through tick"
    );
    assert!(
        shell.contains("if (skipTicks > 0) {"),
        "the return still spends a backoff tick - Back is not Pull now"
    );
    assert_eq!(
        shell.matches("<button").count(),
        3,
        "F10b-3: still exactly three buttons"
    );
}

#[test]
fn f11a_pull_head_is_one_signed_string_in_both_callers() {
    let shell = include_str!("dock_shell.rs");
    let head = crate::dock_folds::PULL_HEAD;
    assert_eq!(head, "Captures reaching the PC");
    assert!(head.is_ascii(), "PULL_HEAD must be ASCII");
    assert!(
        shell.contains(&format!("var PULL_HEAD = \"{}\";", head)),
        "shell must carry the same signed heading"
    );
    for banned in ["Healthy", "Degraded", "Unhealthy"] {
        assert!(
            !head.contains(banned),
            "heading must carry no severity word"
        );
    }
}

#[test]
fn f11b_pull_health_is_on_the_wire_whole_and_carries_no_token() {
    let (db, _) = paired_db();
    let conn = db.lock().unwrap();
    let now = crate::db::utc_now_rfc3339();
    let today = crate::dock_folds::local_date_of(&now).expect("today from now");
    let doc = crate::dock_folds::port_document(
        &conn,
        std::path::Path::new("."),
        std::path::Path::new("."),
        &now,
        today,
    )
    .unwrap();
    let v = serde_json::to_value(&doc).unwrap();
    let p = v.get("pullHealth").expect("pullHealth on the wire");
    assert!(p.is_object());
    for k in ["message", "lastOkMessage", "refusalMessage", "gapMessage"] {
        assert!(p.get(k).is_some(), "pullHealth must carry {}", k);
    }
    let msg = p.get("message").unwrap().as_str().unwrap();
    assert!(!msg.trim().is_empty(), "message must never be empty");
    let whole = serde_json::to_string(p).unwrap();
    assert!(!whole.contains("sk_"), "no token on the wire");
    assert!(!whole.contains("/a/"), "no capture path on the wire");
}

#[test]
fn f11c_shell_prints_pull_sentences_and_builds_none() {
    let shell = include_str!("dock_shell.rs");
    assert_eq!(
        shell.matches("showPull(").count(),
        4,
        "one definition, three calls"
    );
    assert!(shell.contains("showPull(null);"));
    assert!(shell.contains("showPull(doc);"));
    for field in [
        "p.message",
        "p.lastOkMessage",
        "p.refusalMessage",
        "p.gapMessage",
    ] {
        assert!(
            shell.contains(field),
            "shell must read {} off the wire",
            field
        );
    }
    // The shell owns no pull sentence of its own.
    for owned in [
        "Pulled ",
        " refused ",
        "already known",
        "can never be pulled",
    ] {
        assert!(!shell.contains(owned), "shell must not compose: {}", owned);
    }
}

#[test]
fn f11d_pull_section_sits_below_the_queue_and_above_pull_now() {
    let shell = include_str!("dock_shell.rs");
    let queue = shell.find("<div id=\"queue\">").expect("queue div");
    let head = shell.find("<h2 id=\"pullHead\">").expect("pullHead");
    let lines = shell.find("<div id=\"pullLines\">").expect("pullLines");
    let button = shell
        .find("<button type=\"button\" id=\"pull\">")
        .expect("pull button");
    assert!(queue < head, "pull head below the queue");
    assert!(head < lines, "head above its lines");
    assert!(lines < button, "pull section above Pull now");
    assert_eq!(
        shell.matches("<button").count(),
        3,
        "still exactly three buttons"
    );
    assert!(shell.is_ascii(), "shell stays ASCII");
}

#[test]
fn f11e_diagnosis_carries_the_pull_section() {
    let (db, _) = paired_db();
    let conn = db.lock().unwrap();
    let now = crate::db::utc_now_rfc3339();
    let today = crate::dock_folds::local_date_of(&now).expect("today from now");
    let doc = crate::dock_folds::port_document(
        &conn,
        std::path::Path::new("."),
        std::path::Path::new("."),
        &now,
        today,
    )
    .unwrap();
    let text = &doc.diagnosis;
    assert!(
        text.contains(crate::dock_folds::PULL_HEAD),
        "diagnosis must carry the signed heading"
    );
    assert!(
        text.contains(&doc.pull_health.message),
        "diagnosis must carry the pull message"
    );
    assert!(text.is_ascii() || !text.is_empty());
}

/// DESK-LABEL-1 - the desk can never print a raw action key.
///
/// Three places have to agree and none can see the other two at compile time:
/// the raise sites in attention.rs and poll.rs, the closed set in
/// attention::KNOWN_ACTIONS, and the label switch in Today.tsx. tsc holds the
/// union against the switch; nothing but this test holds the Rust raise sites
/// against either. Same shape as h2 and f1c, which already read another file
/// off disk to assert a cross-surface invariant.
#[test]
fn dl1_every_raised_action_is_known_and_labelled_on_the_desk() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // Every actions slice at every raise site. `&["` occurs nowhere else in
    // these two files, so an unrelated slice literal fails this test closed
    // rather than slipping past it.
    let mut raised: Vec<String> = Vec::new();
    for rel in ["src/attention.rs", "src/poll.rs"] {
        let src = std::fs::read_to_string(root.join(rel)).expect("read raise-site source");
        for line in src.lines() {
            let Some((_, after)) = line.split_once("&[\"") else {
                continue;
            };
            let Some((slice, _)) = after.split_once(']') else {
                continue;
            };
            for (i, part) in slice.split('"').enumerate() {
                if i % 2 == 0 {
                    raised.push(part.to_string());
                }
            }
        }
    }
    assert!(
        raised.len() >= 11,
        "the raise-site scan found {} keys - the pattern moved",
        raised.len()
    );
    for key in &raised {
        assert!(
            crate::attention::KNOWN_ACTIONS.contains(&key.as_str()),
            "a raise site emits '{key}', which is not in attention::KNOWN_ACTIONS"
        );
    }
    let today =
        std::fs::read_to_string(root.join("../src/screens/Today.tsx")).expect("read Today.tsx");
    let list_start = today
        .find("const ATTENTION_ACTIONS = [")
        .expect("ATTENTION_ACTIONS exists");
    let list_len = today[list_start..]
        .find("] as const;")
        .expect("ATTENTION_ACTIONS closes");
    let list = &today[list_start..list_start + list_len];
    let fn_start = today
        .find("function attentionActionLabel(")
        .expect("attentionActionLabel exists");
    let body_all = &today[fn_start..];
    let body_len = body_all.find("\n  }").expect("attentionActionLabel body");
    let body = &body_all[..body_len];
    for key in crate::attention::KNOWN_ACTIONS {
        assert!(
            list.contains(&format!("\"{key}\"")),
            "{key} is missing from ATTENTION_ACTIONS in Today.tsx"
        );
        assert!(
            body.contains(&format!("case \"{key}\":")),
            "{key} has no label arm in Today.tsx"
        );
    }
    // No orphan arm: a label for a key nothing can raise is dead vocabulary.
    for part in body.split("case \"").skip(1) {
        let key = part.split('"').next().expect("case arm key");
        assert!(
            crate::attention::KNOWN_ACTIONS.contains(&key),
            "Today.tsx labels '{key}', which is not in attention::KNOWN_ACTIONS"
        );
    }
    assert!(
        !body.contains("return action;"),
        "the raw-key fallthrough is back"
    );
    assert!(
        body.contains("const unreachable: never = action;"),
        "the exhaustiveness guard must stay - it is what makes tsc the gate"
    );
    assert_eq!(
        today.matches("isKnownAction(a)").count(),
        2,
        "both render sites must drop an unrecognised action"
    );
}
