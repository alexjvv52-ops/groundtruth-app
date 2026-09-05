//! Fence I — findings 01, 06, 07, 08, 09.

use crate::db;
use crate::event_file;
use crate::health::{h4_inputs_from_evidence, severity_for, CheckInputs, Evidence, Severity};
use crate::marketing;
use crate::projection::{self, EXCLUSION_LIST, REPLAY_DROPPED_TRIGGERS};
use crate::trays;
use crate::wholesale::{self, OrderLine, WriteOffInput};
use rusqlite::{params, Connection};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("groundtruth-f1-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn venue(conn: &mut Connection) -> marketing::VenueView {
    marketing::record_venue(conn, "Fixture Cafe", "cafe", None, None, None, None).unwrap()
}

fn trigger_present(conn: &Connection, name: &str) -> bool {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
            [name],
            |r| r.get(0),
        )
        .unwrap();
    n == 1
}

/// Venue, priced order delivered and paid with a shortfall + write-off,
/// plus a voided order. Caller flushes.
fn build_wholesale_book(conn: &mut Connection) -> (String, String) {
    let v = venue(conn);
    let _tray = trays::sow_tray(conn, "dun-peas", 6).unwrap();
    let d = db::local_date_today();
    let paid = wholesale::record_order(
        conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 2,
            price_cents_per_tray: Some(800),
        }],
        false,
    )
    .unwrap();
    wholesale::deliver_order(conn, &paid.id, None).unwrap();
    let today = db::local_date_today();
    wholesale::pay_order(
        conn,
        &paid.id,
        1000,
        &today,
        None,
        true,
        false,
        Some(WriteOffInput {
            category: "sales_discount".into(),
            reason: None,
        }),
    )
    .unwrap();
    let voided = wholesale::record_order(
        conn,
        &v.venue_id,
        &d,
        vec![OrderLine {
            crop_id: "dun-peas".into(),
            trays: 1,
            price_cents_per_tray: Some(700),
        }],
        false,
    )
    .unwrap();
    wholesale::void_order(conn, &voided.id, Some("changed mind".into())).unwrap();
    (paid.id, voided.id)
}

#[test]
fn f1_01_wholesale_book_is_compared_not_excluded() {
    for t in [
        "wholesale_orders",
        "wholesale_order_lines",
        "wholesale_write_offs",
        "wholesale_bad_debts",
        "stripe_unapplied_facts",
    ] {
        assert!(
            EXCLUSION_LIST.iter().all(|l| !l.contains(t)),
            "compared, not excluded: {t} in {EXCLUSION_LIST:?}"
        );
    }

    let dir = temp_dir("book");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    build_wholesale_book(&mut conn);
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);

    let outcome =
        projection::verify_replay_paths(&dir.join("farm.db"), &event_file::events_path(&dir), &dir)
            .unwrap();
    assert!(
        !outcome.exit_nonzero(),
        "verify failed: {}",
        outcome.summary_line()
    );
    assert_eq!(
        outcome.report().tables_compared,
        26,
        "five money-book tables added to the compare list (19 + 5), then leftover_listings, then seed_receipts"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn f1_01_a_tampered_order_row_fails_verify() {
    let dir = temp_dir("tamper-order");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let (paid_id, _) = build_wholesale_book(&mut conn);
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);

    {
        let conn = Connection::open(dir.join("farm.db")).unwrap();
        conn.execute(
            "UPDATE wholesale_orders SET state = 'delivered' WHERE id = ?1",
            [&paid_id],
        )
        .unwrap();
    }
    let outcome =
        projection::verify_replay_paths(&dir.join("farm.db"), &event_file::events_path(&dir), &dir)
            .unwrap();
    assert!(outcome.exit_nonzero(), "{}", outcome.summary_line());
    assert!(
        outcome
            .report()
            .unknown_diffs
            .iter()
            .any(|d| d.table == "wholesale_orders"),
        "expected wholesale_orders unknown_diff: {:?}",
        outcome.report().unknown_diffs
    );
    let _ = fs::remove_dir_all(&dir);

    let dir = temp_dir("tamper-line");
    let mut conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
    let (paid_id, _) = build_wholesale_book(&mut conn);
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);

    {
        let conn = Connection::open(dir.join("farm.db")).unwrap();
        conn.execute(
            "UPDATE wholesale_order_lines SET trays = trays + 1 WHERE order_id = ?1",
            [&paid_id],
        )
        .unwrap();
    }
    let outcome =
        projection::verify_replay_paths(&dir.join("farm.db"), &event_file::events_path(&dir), &dir)
            .unwrap();
    assert!(outcome.exit_nonzero(), "{}", outcome.summary_line());
    assert!(
        outcome
            .report()
            .unknown_diffs
            .iter()
            .any(|d| d.table == "wholesale_order_lines"),
        "expected wholesale_order_lines unknown_diff: {:?}",
        outcome.report().unknown_diffs
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn f1_07_replay_does_not_read_the_clock() {
    let dir = temp_dir("clock");
    let farm = dir.join("farm.db");
    let replay = dir.join("replay.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);

    projection::verify_replay(&farm, &event_file::events_path(&dir), &replay).unwrap();
    let replay_conn = Connection::open(&replay).unwrap();
    let live = db::open_in_memory().unwrap();
    for t in REPLAY_DROPPED_TRIGGERS {
        assert!(!trigger_present(&replay_conn, t), "replay must drop {t}");
        assert!(trigger_present(&live, t), "live migrate must keep {t}");
    }

    let tomorrow = (chrono::Local::now().date_naive() + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let insert = "INSERT INTO cost_events
         (event_id, origin, date_paid, amount_cents, payee,
          canonical_category, schedule_f_line, schedule_c_line,
          created_at, updated_at)
         VALUES (?1, 'farm_os', ?2, 100, 'Test Payee',
                 'seeds', 'Seeds and plants', 'Cost of goods',
                 '2026-08-19T00:00:00.000Z', '2026-08-19T00:00:00.000Z')";
    replay_conn
        .execute(insert, params!["future-replay", tomorrow])
        .expect("replay-shaped insert of a tomorrow-dated cost must succeed");
    let err = live
        .execute(insert, params!["future-live", tomorrow])
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("cannot be future"),
        "live protection unchanged: {err}"
    );
    let _ = fs::remove_dir_all(&dir);
}

fn ev(ran_at: &str, ok: bool, detail: &str) -> Evidence {
    Evidence {
        check_id: "H4".into(),
        ran_at: ran_at.into(),
        ok,
        detail: detail.into(),
    }
}

const NOW: &str = "2026-08-11T12:00:00.000Z";
const OLD: &str = "2026-07-01T12:00:00.000Z";
const RECENT: &str = "2026-08-10T12:00:00.000Z";

#[test]
fn f1_06_a_stale_pass_file_loses_to_a_newer_failed_run() {
    let dir = temp_dir("stale-pass");
    fs::write(
        dir.join("last-verify-replay.txt"),
        format!("when={OLD}\nVERIFY-REPLAY: PASS\noutcome=VERIFY-REPLAY: PASS\n"),
    )
    .unwrap();
    let e = ev(
        NOW,
        false,
        "quick_check ok; VERIFY-REPLAY: COULD NOT COMPLETE — events.jsonl line 12: expected a JSON event object",
    );
    let h4 = h4_inputs_from_evidence(Some(&e), &dir);
    assert!(h4.last_verify_incomplete);
    assert_eq!(h4.last_full_verify_ok_at, None);
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: h4.quick_check_ok,
            last_full_verify_ok_at: h4.last_full_verify_ok_at,
            last_full_verify_failed: h4.last_full_verify_failed,
            unknown_divergences: h4.unknown_divergences,
            last_verify_incomplete: h4.last_verify_incomplete,
            last_verify_outcome_line: h4.last_verify_outcome_line,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Unhealthy);
    assert!(s.sentence.contains("could not complete"), "{}", s.sentence);
    assert!(!s.sentence.contains("verify passed"), "{}", s.sentence);
    let m4 = severity_for(
        "M4",
        None,
        NOW,
        &CheckInputs {
            last_full_verify_failed: h4.last_full_verify_failed,
            unknown_divergences: h4.unknown_divergences,
            last_verify_incomplete: h4.last_verify_incomplete,
            ..CheckInputs::default()
        },
    );
    assert_ne!(m4.severity, Severity::Healthy, "{}", m4.sentence);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn f1_06_quick_check_evidence_does_not_clobber_a_real_pass() {
    let dir = temp_dir("quick-ok");
    fs::write(
        dir.join("last-verify-replay.txt"),
        format!("when={RECENT}\nVERIFY-REPLAY: PASS\noutcome=VERIFY-REPLAY: PASS\n"),
    )
    .unwrap();
    let e = ev(NOW, true, "quick_check ok");
    let h4 = h4_inputs_from_evidence(Some(&e), &dir);
    assert!(!h4.last_verify_incomplete);
    assert!(!h4.last_full_verify_failed);
    assert_eq!(h4.last_full_verify_ok_at.as_deref(), Some(RECENT));
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: h4.quick_check_ok,
            last_full_verify_ok_at: h4.last_full_verify_ok_at,
            last_full_verify_failed: h4.last_full_verify_failed,
            unknown_divergences: h4.unknown_divergences,
            last_verify_incomplete: h4.last_verify_incomplete,
            last_verify_outcome_line: h4.last_verify_outcome_line,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Healthy);
    assert!(s.sentence.contains("verify passed"), "{}", s.sentence);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn f1_08_failure_sentence_names_the_real_outcome() {
    let e = ev(NOW, false, "quick_check ok; verify failed");
    let flush = CheckInputs {
        quick_check_ok: true,
        last_full_verify_failed: true,
        unknown_divergences: false,
        last_verify_outcome_line: Some("VERIFY-REPLAY: FAIL — 2 event(s) pending flush.".into()),
        ..CheckInputs::default()
    };
    let s = severity_for("H4", Some(&e), NOW, &flush);
    assert!(s.sentence.contains("did not pass"), "{}", s.sentence);
    assert!(
        s.sentence
            .contains("VERIFY-REPLAY: FAIL — 2 event(s) pending flush."),
        "{}",
        s.sentence
    );
    assert!(
        !s.sentence.contains("unexplained divergences"),
        "{}",
        s.sentence
    );

    let unknown = CheckInputs {
        quick_check_ok: true,
        last_full_verify_failed: true,
        unknown_divergences: true,
        ..CheckInputs::default()
    };
    let s = severity_for("H4", Some(&e), NOW, &unknown);
    assert_eq!(
        s.sentence,
        "H4 Core data intact — quick_check ok; verify FAILED with unexplained \
         divergences. Read last-verify-replay.txt in the farm folder."
    );
}
