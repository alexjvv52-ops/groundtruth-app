//! Wave 1 — health severity truth table (h1–h4).

use crate::attention::{CollectDebt, DeliverDebt, MoneyDebts};
use crate::db;
use crate::event_file;
use crate::events::{self, EventRecord, Kind};
use crate::health::{
    cover_invariant_holds, h4_inputs_from_evidence, money_invariant_holds,
    read_failure_invariant_holds, severity_for, shelf_invariant_holds, CheckInputs, CheckStatus,
    Evidence, H4Verify, ReadFailures, Severity, READ_FAILURE_PREFIX, REPORTED_CHECKS,
};
use crate::snapshots;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn ev(check_id: &str, ran_at: &str, ok: bool, detail: &str) -> Evidence {
    Evidence {
        check_id: check_id.into(),
        ran_at: ran_at.into(),
        ok,
        detail: detail.into(),
    }
}

const NOW: &str = "2026-08-11T12:00:00.000Z";
const FRESH: &str = "2026-08-10T12:00:00.000Z"; // 1 day old
const STALE: &str = "2026-07-01T12:00:00.000Z"; // > 7 and > 30 days
const FUTURE: &str = "2026-08-12T12:00:00.000Z";
const VERIFY_OK: &str = "2026-08-01T12:00:00.000Z"; // within 30 days of NOW
const VERIFY_OLD: &str = "2026-07-01T12:00:00.000Z"; // 41 days — outside 30

#[test]
fn h1_severity_for_truth_table() {
    // H1 fresh-ok
    let e = ev("H1", FRESH, true, "5 of 5 varieties parsed");
    let s = severity_for(
        "H1",
        Some(&e),
        NOW,
        &CheckInputs {
            varieties_parsed: Some(5),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Healthy);

    // H1 stale-ok
    let e = ev("H1", STALE, true, "5 of 5 varieties parsed");
    let s = severity_for(
        "H1",
        Some(&e),
        NOW,
        &CheckInputs {
            varieties_parsed: Some(5),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);

    // H1 absent-evidence
    let s = severity_for("H1", None, NOW, &CheckInputs::default());
    assert_eq!(s.severity, Severity::Degraded);
    assert!(s.sentence.contains("hasn't reported"));

    // H1 failing (consecutive failures)
    let e = ev("H1", FRESH, false, "network down");
    let s = severity_for(
        "H1",
        Some(&e),
        NOW,
        &CheckInputs {
            consecutive_failures: 3,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Unhealthy);

    // H1 structural miss
    let miss = "4 of 5 varieties parsed — Purple Kohlrabi not found";
    let e = ev("H1", FRESH, false, miss);
    let s = severity_for(
        "H1",
        Some(&e),
        NOW,
        &CheckInputs {
            structural_miss: Some(miss.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Unhealthy);
    assert_eq!(s.sentence, miss);

    // H1 ran_at-in-the-future
    let e = ev("H1", FUTURE, true, "5 of 5");
    let s = severity_for(
        "H1",
        Some(&e),
        NOW,
        &CheckInputs {
            varieties_parsed: Some(5),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);
    assert!(s.sentence.contains("future"));

    // H3 fresh-ok
    let e = ev("H3", FRESH, true, "ok");
    let s = severity_for(
        "H3",
        Some(&e),
        NOW,
        &CheckInputs {
            flush_lag: 0,
            snapshot_today: true,
            retention_ran: true,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Healthy);

    // H3 stale-ok (evidence exists but no snapshot today)
    let e = ev("H3", STALE, true, "ok");
    let s = severity_for(
        "H3",
        Some(&e),
        NOW,
        &CheckInputs {
            flush_lag: 0,
            snapshot_today: false,
            retention_ran: true,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);

    // H3 absent
    let s = severity_for("H3", None, NOW, &CheckInputs::default());
    assert_eq!(s.severity, Severity::Degraded);

    // H3 failing (flush lag)
    let e = ev("H3", FRESH, false, "lag");
    let s = severity_for(
        "H3",
        Some(&e),
        NOW,
        &CheckInputs {
            flush_lag: 2,
            snapshot_today: true,
            retention_ran: true,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Unhealthy);

    // H3 future
    let e = ev("H3", FUTURE, true, "ok");
    let s = severity_for(
        "H3",
        Some(&e),
        NOW,
        &CheckInputs {
            flush_lag: 0,
            snapshot_today: true,
            retention_ran: true,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);
    assert!(s.sentence.contains("future"));

    // H4 fresh-ok
    let e = ev("H4", FRESH, true, "quick_check ok");
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: true,
            last_full_verify_ok_at: Some(VERIFY_OK.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Healthy);

    // H4 stale-ok (no verify in 30 days)
    let e = ev("H4", STALE, true, "quick_check ok");
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: true,
            last_full_verify_ok_at: Some(VERIFY_OLD.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);

    // H4 absent
    let s = severity_for("H4", None, NOW, &CheckInputs::default());
    assert_eq!(s.severity, Severity::Degraded);

    // H4 failing quick_check
    let e = ev("H4", FRESH, false, "quick_check FAIL");
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: false,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Unhealthy);

    // H4 future
    let e = ev("H4", FUTURE, true, "ok");
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: true,
            last_full_verify_ok_at: Some(VERIFY_OK.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);
    assert!(s.sentence.contains("future"));
}

#[test]
fn h2_absent_evidence_says_hasnt_reported() {
    for id in ["H1", "H2", "H3", "H4"] {
        let s = severity_for(id, None, NOW, &CheckInputs::default());
        assert_eq!(s.severity, Severity::Degraded);
        assert!(
            s.sentence.contains("hasn't reported"),
            "{}: {}",
            id,
            s.sentence
        );
    }
}

#[test]
fn h3_flush_lag_unhealthy_with_no_grace() {
    let e = ev("H3", FRESH, true, "ok");
    let s = severity_for(
        "H3",
        Some(&e),
        NOW,
        &CheckInputs {
            flush_lag: 1,
            snapshot_today: true,
            retention_ran: true,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Unhealthy);
    assert!(s.sentence.contains("FLUSH LAG"));
}

#[test]
fn h4_verify_window_thirty_days() {
    let e = ev("H4", FRESH, true, "quick_check ok");

    // 31+ days → Degraded
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: true,
            last_full_verify_ok_at: Some(VERIFY_OLD.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);
    assert!(s.sentence.contains("30 days"));

    // within 30 days → Healthy
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: true,
            last_full_verify_ok_at: Some(VERIFY_OK.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Healthy);
}

fn tempfile_dir(label: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("farm-os-health-{}-{}", label, uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn h4_verify_fail_with_quick_ok_is_not_labelled_quick_check_fail() {
    let dir = tempfile_dir("h4-verify-fail");
    let e = Evidence {
        check_id: "H4".into(),
        ran_at: FRESH.into(),
        ok: false,
        detail: "quick_check ok; verify failed: VERIFY-REPLAY: FAIL".into(),
    };
    let h4: H4Verify = h4_inputs_from_evidence(Some(&e), &dir);
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
    assert!(s.sentence.contains("did not pass"), "{}", s.sentence);
    assert!(!s.sentence.contains("quick_check FAIL"));
    assert!(!s.sentence.contains("unexplained divergences"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn h3_log_ahead_is_unhealthy() {
    let e = ev("H3", FRESH, true, "ok");
    let s = severity_for(
        "H3",
        Some(&e),
        NOW,
        &CheckInputs {
            log_ahead: true,
            log_watermark: 120,
            db_max_seq: 101,
            flush_lag: 0,
            snapshot_today: true,
            retention_ran: true,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Unhealthy);
    assert!(s.sentence.contains("AHEAD"));
}

#[test]
fn flush_refuses_across_fork() {
    let dir = tempfile_dir("flush-fork");
    let events = event_file::events_path(&dir);
    let line = r#"{"seq":120,"event_id":"fork-test","kind":"tray.sown"}"#;
    fs::write(&events, format!("{line}\n")).unwrap();
    let before = fs::read(&events).unwrap();
    let conn = db::open_in_memory().unwrap();
    let err = event_file::flush_events(&conn, &dir).unwrap_err();
    assert!(err.contains("refusing to append across a fork"));
    let after = fs::read(&events).unwrap();
    assert_eq!(before, after);
    let _ = fs::remove_dir_all(&dir);
}

fn write_n_grow_events(conn: &mut Connection, n: i64) {
    for i in 1..=n {
        let id = format!("tray-{i}");
        let event = EventRecord::originated(
            Kind::TraySown,
            "tray",
            &id,
            json!({
                "cropId": "dun-peas",
                "quantity": 1,
                "sownOn": "2026-08-06",
                "blackoutOn": "2026-08-06"
            }),
            json!({ "op": "delete_tray", "trayId": id }),
            "2026-08-06T00:00:00.000Z",
            None,
            None,
            None,
        );
        let tx = conn.transaction().unwrap();
        events::write_event(&tx, &event).unwrap();
        tx.commit().unwrap();
    }
}

fn jsonl_seqs(path: &Path) -> Vec<i64> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["seq"].as_i64().unwrap()
        })
        .collect()
}

fn pre_restore_archives(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("events.jsonl.pre-restore-") {
            out.push(entry.path());
        }
    }
    out.sort();
    out
}

#[allow(clippy::permissions_set_readonly_false)] // H-17 / H-8: Windows rollback
                                                 // path - clears the read-only attribute so the archive can be renamed back.
                                                 // The lint's hazard is the Unix 0o777 meaning; this app targets Windows.
fn clear_readonly(path: &Path) {
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
}

#[test]
fn restore_rebuild_matches_db() {
    let dir = tempfile_dir("rebuild-match");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    write_n_grow_events(&mut conn, 5);
    event_file::flush_events(&conn, &dir).unwrap();
    let events = event_file::events_path(&dir);
    assert_eq!(jsonl_seqs(&events), vec![1, 2, 3, 4, 5]);

    db::drop_event_log_triggers(&conn).unwrap();
    conn.execute("DELETE FROM event_log WHERE seq >= 4", [])
        .unwrap();

    let ok = event_file::archive_and_rebuild(&conn, &dir, "2026-08-12T23:30:45.000Z").unwrap();
    assert_eq!(ok.watermark, 3);
    assert_eq!(ok.lines_written, 3);

    let archives = pre_restore_archives(&dir);
    assert_eq!(archives.len(), 1);
    let archive = &archives[0];
    assert_eq!(jsonl_seqs(archive), vec![1, 2, 3, 4, 5]);
    assert!(
        fs::metadata(archive).unwrap().permissions().readonly(),
        "pre-restore archive must be read-only"
    );

    assert_eq!(jsonl_seqs(&events), vec![1, 2, 3]);
    assert_eq!(event_file::read_watermark(&events).unwrap(), 3);
    event_file::verify_integrity(&conn, &dir).unwrap();
    event_file::flush_events(&conn, &dir).unwrap();

    clear_readonly(archive);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn restore_rebuild_rolls_back_on_guard_failure() {
    let dir = tempfile_dir("rebuild-rollback");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    write_n_grow_events(&mut conn, 5);
    event_file::flush_events(&conn, &dir).unwrap();
    let events = event_file::events_path(&dir);
    let before = fs::read(&events).unwrap();

    db::drop_event_log_triggers(&conn).unwrap();
    conn.execute(
        "INSERT INTO event_log
         (id, kind, entity_type, entity_id, payload, inverse, created_at,
          origin, event_domain, event_class, reverses_event_id)
         VALUES ('bad-null-origin', 'tray.sown', 'tray', 't-bad', '{}', '{}',
                 '2026-08-06T00:00:00.000Z', NULL, 'grow', NULL, NULL)",
        [],
    )
    .unwrap();

    let err = event_file::archive_and_rebuild(&conn, &dir, "2026-08-12T23:30:45.000Z").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(fs::read(&events).unwrap(), before);
    assert!(pre_restore_archives(&dir).is_empty());
    assert!(!dir.join("events.jsonl.rebuilding").exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn restore_rebuild_no_prior_log() {
    let dir = tempfile_dir("rebuild-none");
    let farm = dir.join("farm.db");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    write_n_grow_events(&mut conn, 3);
    let events = event_file::events_path(&dir);
    assert!(!events.exists());

    let ok = event_file::archive_and_rebuild(&conn, &dir, "2026-08-12T23:30:45.000Z").unwrap();
    assert_eq!(ok.archived_as, None);
    assert_eq!(ok.lines_written, 3);
    event_file::verify_integrity(&conn, &dir).unwrap();

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn restore_rebuild_failure_keeps_session_on_restored_db() {
    let dir = tempfile_dir("rebuild-session");
    let farm = dir.join("farm.db");
    let snap_dir = dir.join("snapshots");
    fs::create_dir_all(&snap_dir).unwrap();

    let db = Mutex::new(db::open_and_migrate(&farm).unwrap());
    let snap_path = {
        let mut conn = db.lock().unwrap();
        write_n_grow_events(&mut conn, 3);
        let snap = snapshots::take_snapshot(&mut conn, &snap_dir).unwrap();
        snap.path
    };

    {
        let snap_conn = Connection::open(&snap_path).unwrap();
        db::drop_event_log_triggers(&snap_conn).unwrap();
        snap_conn
            .execute(
                "INSERT INTO event_log
                 (id, kind, entity_type, entity_id, payload, inverse, created_at,
                  origin, event_domain, event_class, reverses_event_id)
                 VALUES ('bad-null-origin', 'tray.sown', 'tray', 't-bad', '{}', '{}',
                         '2026-08-06T00:00:00.000Z', NULL, 'grow', NULL, NULL)",
                [],
            )
            .unwrap();
    }

    let err =
        snapshots::restore_snapshot(&db, &farm, &snap_dir, Path::new(&snap_path)).unwrap_err();
    assert!(
        err.contains("log could not be rebuilt"),
        "unexpected restore error: {err}"
    );

    let conn = db.lock().unwrap();
    let bad: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE id = 'bad-null-origin'",
            [],
            |r| r.get(0),
        )
        .expect("managed connection must be the restored farm, not the in-memory decoy");
    assert_eq!(bad, 1);
    let sown: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE kind = 'tray.sown' AND origin = 'farm_os'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sown, 3);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn h4fr_never_verified_is_calm_and_degraded() {
    let e = ev("H4", FRESH, true, "quick_check ok");
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: true,
            never_verified: true,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);
    assert!(s.sentence.contains("has not been verified yet"));
    assert!(!s.sentence.contains("30 days"));
}

#[test]
fn h4fr_stale_verify_keeps_thirty_day_wording() {
    let e = ev("H4", FRESH, true, "quick_check ok");
    let s = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: true,
            never_verified: false,
            last_full_verify_ok_at: None,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);
    assert!(s.sentence.contains("no full verify in 30 days"));
}

#[test]
fn h4fr_verified_then_healthy_and_failure_still_red() {
    let e = ev("H4", FRESH, true, "quick_check ok");
    let healthy = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: true,
            last_full_verify_ok_at: Some(NOW.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(healthy.severity, Severity::Healthy);
    assert!(!healthy.sentence.contains("has not been verified yet"));

    let failed = severity_for(
        "H4",
        Some(&e),
        NOW,
        &CheckInputs {
            quick_check_ok: true,
            last_full_verify_failed: true,
            ..CheckInputs::default()
        },
    );
    assert_eq!(failed.severity, Severity::Unhealthy);
    assert!(failed.sentence.contains("did not pass"));
}

fn collect_debt(days: i64, cents: i64, unpriced: bool) -> CollectDebt {
    CollectDebt {
        order_id: format!("order-{days}"),
        venue_name: "Harvest Table".into(),
        delivered_on: "2026-08-01".into(),
        days,
        age_countable: true,
        any_unpriced: unpriced,
        cents,
        message: "Collect".into(),
    }
}

fn collect_debt_uncountable(days: i64, cents: i64, unpriced: bool) -> CollectDebt {
    CollectDebt {
        order_id: format!("order-uncountable-{days}"),
        venue_name: "Harvest Table".into(),
        delivered_on: "2026-08-01".into(),
        days,
        age_countable: false,
        any_unpriced: unpriced,
        cents,
        message: "Collect".into(),
    }
}

fn deliver_debt(days_late: i64, trays: i64) -> DeliverDebt {
    DeliverDebt {
        order_id: format!("due-{days_late}"),
        venue_name: "Verdant".into(),
        harvest_date: "2026-08-14".into(),
        days_late,
        trays,
        message: "Deliver".into(),
    }
}

fn with_money(money: MoneyDebts) -> CheckInputs {
    CheckInputs {
        money,
        ..CheckInputs::default()
    }
}

#[test]
fn m1_owed_bands_and_the_invariant_floor() {
    // Clean.
    let s = severity_for("M1", None, NOW, &CheckInputs::default());
    assert_eq!(s.severity, Severity::Healthy);
    assert!(s.sentence.contains("nothing owed to you"), "{}", s.sentence);
    assert_eq!(
        s.ran_at.as_deref(),
        Some(NOW),
        "M1 is computed when you look"
    );

    // 3 days old. The audit's literal M1 line would call this Healthy; the
    // invariant forbids it, because Today shows a COLLECT card at day 0.
    let young = with_money(MoneyDebts {
        collect: vec![collect_debt(3, 18000, false)],
        ..MoneyDebts::default()
    });
    let s = severity_for("M1", None, NOW, &young);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert!(s.sentence.contains("$180.00"), "{}", s.sentence);
    assert!(s.sentence.contains("1 delivery"), "{}", s.sentence);
    assert!(s.sentence.contains("oldest 3 days"), "{}", s.sentence);

    // 8 days — still Degraded, louder sentence.
    let mid = with_money(MoneyDebts {
        collect: vec![collect_debt(8, 18000, false)],
        ..MoneyDebts::default()
    });
    assert_eq!(
        severity_for("M1", None, NOW, &mid).severity,
        Severity::Degraded
    );

    // 15 days — past the decision number.
    let old = with_money(MoneyDebts {
        collect: vec![collect_debt(15, 18000, false)],
        ..MoneyDebts::default()
    });
    assert_eq!(
        severity_for("M1", None, NOW, &old).severity,
        Severity::Unhealthy
    );

    // Unpriced never becomes a total.
    let unpriced = with_money(MoneyDebts {
        collect: vec![collect_debt(2, 0, true), collect_debt(9, 5000, false)],
        ..MoneyDebts::default()
    });
    let s = severity_for("M1", None, NOW, &unpriced);
    assert!(s.sentence.contains("partly unpriced"), "{}", s.sentence);
    assert!(!s.sentence.contains('$'), "no fake total: {}", s.sentence);
    assert!(s.sentence.contains("2 deliveries"), "{}", s.sentence);
}

#[test]
fn m1_uncountable_debts_drop_the_age_phrase() {
    let only = with_money(MoneyDebts {
        collect: vec![collect_debt_uncountable(5, 18000, false)],
        ..MoneyDebts::default()
    });
    let s = severity_for("M1", None, NOW, &only);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert!(s.sentence.contains("$180.00"), "{}", s.sentence);
    assert!(s.sentence.contains("1 delivery"), "{}", s.sentence);
    assert!(!s.sentence.contains("delivered today"), "{}", s.sentence);
    assert!(!s.sentence.contains(", oldest"), "{}", s.sentence);
    assert_eq!(
        s.sentence,
        "M1 Owed to you — $180.00 across 1 delivery. Collect the oldest on the Money tab."
    );
}

#[test]
fn m3_deliveries_due_names_venue_and_date() {
    let s = severity_for("M3", None, NOW, &CheckInputs::default());
    assert_eq!(s.severity, Severity::Healthy);
    assert!(s.sentence.contains("nothing due"), "{}", s.sentence);

    let today = with_money(MoneyDebts {
        deliver: vec![deliver_debt(0, 6)],
        ..MoneyDebts::default()
    });
    let s = severity_for("M3", None, NOW, &today);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert!(s.sentence.contains("6 trays"), "{}", s.sentence);
    assert!(s.sentence.contains("Verdant"), "{}", s.sentence);
    assert!(s.sentence.contains("Fri Aug 14"), "{}", s.sentence);

    let late = with_money(MoneyDebts {
        deliver: vec![deliver_debt(0, 6), deliver_debt(2, 3)],
        ..MoneyDebts::default()
    });
    let s = severity_for("M3", None, NOW, &late);
    assert_eq!(s.severity, Severity::Unhealthy, "{}", s.sentence);
    assert!(s.sentence.contains("2 days ago"), "{}", s.sentence);
    assert!(s.sentence.contains("Deliver or void it"), "{}", s.sentence);
}

#[test]
fn m4_ledger_symmetry_counts_and_defers_to_verify() {
    let clean = CheckInputs {
        quarter_label: "Q3 2026".into(),
        ..CheckInputs::default()
    };
    let s = severity_for("M4", None, NOW, &clean);
    assert_eq!(s.severity, Severity::Healthy);
    assert!(s.sentence.contains("Q3 2026"), "{}", s.sentence);

    let three = CheckInputs {
        untrailed_income_corrections: 3,
        quarter_label: "Q3 2026".into(),
        ..CheckInputs::default()
    };
    let s = severity_for("M4", None, NOW, &three);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert!(
        s.sentence.contains("detail lives only in the log"),
        "{}",
        s.sentence
    );
    assert!(
        s.sentence.contains("3 income records were"),
        "{}",
        s.sentence
    );

    let one = CheckInputs {
        untrailed_income_corrections: 1,
        quarter_label: "Q3 2026".into(),
        ..CheckInputs::default()
    };
    assert!(severity_for("M4", None, NOW, &one)
        .sentence
        .contains("1 income record was"),);

    let broken = CheckInputs {
        untrailed_income_corrections: 0,
        last_full_verify_failed: true,
        ..CheckInputs::default()
    };
    assert_eq!(
        severity_for("M4", None, NOW, &broken).severity,
        Severity::Unhealthy
    );
}

#[test]
fn money_invariant_is_vacuous_on_a_clean_farm() {
    let statuses = vec![severity_for("M1", None, NOW, &CheckInputs::default())];
    assert!(money_invariant_holds(&MoneyDebts::default(), &statuses));
}

#[test]
fn f1_trays_on_the_shelf_truth_table() {
    // Clean shelf.
    let s = severity_for("F1", None, NOW, &CheckInputs::default());
    assert_eq!(s.severity, Severity::Healthy);
    assert!(
        s.sentence.contains("nothing is past its harvest date"),
        "{}",
        s.sentence
    );

    // One tray reads as one tray.
    let one = CheckInputs {
        overdue_trays: 1,
        ..CheckInputs::default()
    };
    let s = severity_for("F1", None, NOW, &one);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert!(
        s.sentence.contains("1 tray is past its harvest date"),
        "{}",
        s.sentence
    );

    // Many trays name the count and the next action.
    let many = CheckInputs {
        overdue_trays: 4,
        ..CheckInputs::default()
    };
    let s = severity_for("F1", None, NOW, &many);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert!(
        s.sentence.contains("4 trays are past their harvest date"),
        "{}",
        s.sentence
    );
    assert!(
        s.sentence.contains("Harvest them on Today"),
        "{}",
        s.sentence
    );
}

#[test]
fn shelf_invariant_forbids_calm_while_trays_sit_past_harvest() {
    let overdue = CheckInputs {
        overdue_trays: 3,
        ..CheckInputs::default()
    };
    let raising = vec![severity_for("F1", None, NOW, &overdue)];
    assert!(shelf_invariant_holds(3, &raising));

    // A Healthy F1 while trays are overdue is exactly the calm Health is
    // forbidden to render.
    let calm = vec![severity_for("F1", None, NOW, &CheckInputs::default())];
    assert!(!shelf_invariant_holds(3, &calm));
    assert!(shelf_invariant_holds(0, &calm));
}

#[test]
fn f2_trays_under_cover_truth_table() {
    // Clean bench.
    let s = severity_for("F2", None, NOW, &CheckInputs::default());
    assert_eq!(s.severity, Severity::Healthy);
    assert!(
        s.sentence.contains("nothing is past its cover-check date"),
        "{}",
        s.sentence
    );
    // One tray reads as one tray, and names the verb.
    let one = CheckInputs {
        overdue_light_trays: 1,
        ..CheckInputs::default()
    };
    let s = severity_for("F2", None, NOW, &one);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert!(
        s.sentence.contains("1 tray has been under cover"),
        "{}",
        s.sentence
    );
    assert!(
        s.sentence.contains("Move it to light on Today."),
        "{}",
        s.sentence
    );
    // Many trays.
    let many = CheckInputs {
        overdue_light_trays: 6,
        ..CheckInputs::default()
    };
    let s = severity_for("F2", None, NOW, &many);
    assert_eq!(s.severity, Severity::Degraded, "{}", s.sentence);
    assert!(
        s.sentence.contains("6 trays have been under cover"),
        "{}",
        s.sentence
    );
    assert!(
        s.sentence.contains("Move them to light on Today."),
        "{}",
        s.sentence
    );
    // F2 never invents an Unhealthy band — no input value can reach it.
    let huge = CheckInputs {
        overdue_light_trays: 999,
        ..CheckInputs::default()
    };
    assert_eq!(
        severity_for("F2", None, NOW, &huge).severity,
        Severity::Degraded
    );
}

#[test]
fn cover_invariant_forbids_calm_while_trays_sit_under_cover() {
    let overdue = CheckInputs {
        overdue_light_trays: 6,
        ..CheckInputs::default()
    };
    let raising = vec![severity_for("F2", None, NOW, &overdue)];
    assert!(cover_invariant_holds(6, &raising));
    // The audit's R1 state: F1 honestly clean, F2 calm, trays overdue for light.
    // That is the lie RB1 closes.
    let calm = vec![
        severity_for("F1", None, NOW, &CheckInputs::default()),
        severity_for("F2", None, NOW, &CheckInputs::default()),
    ];
    assert!(!cover_invariant_holds(6, &calm));
    assert!(cover_invariant_holds(0, &calm));
}

#[test]
fn reported_checks_make_no_storefront_claim() {
    assert!(
        !REPORTED_CHECKS.contains(&"H1"),
        "H1 was retired by ruling 6.1 and must not be reported"
    );
    assert_eq!(REPORTED_CHECKS.len(), 9);
    for id in REPORTED_CHECKS {
        let s = severity_for(id, None, NOW, &CheckInputs::default());
        assert_eq!(s.check_id, id);
        assert!(
            !s.sentence.to_lowercase().contains("storefront"),
            "{} still claims a storefront: {}",
            id,
            s.sentence
        );
        if matches!(id, "H2" | "H3" | "H4") {
            // Evidence-backed: absent evidence legitimately says hasn't reported.
            assert!(
                s.sentence.contains("hasn't reported"),
                "{} should name absent evidence: {}",
                id,
                s.sentence
            );
        } else {
            assert!(
                !s.sentence.contains("Check ")
                    || !s.sentence.contains("hasn't reported since never"),
                "{} fell through severity_for's unknown-id arm: {}",
                id,
                s.sentence
            );
        }
    }
}

/// H2 arm 1 - the marketing partition is behind the flush watermark. This is
/// the only Unhealthy H2 can reach, and it reads a different field from H3's
/// `flush_lag`: a stuck marketing row must surface without a grow row being
/// stuck too. The arm also carries the evidence stamp through.
#[test]
fn h2_flush_lag_is_unhealthy() {
    let e = ev("H2", FRESH, true, "ok");
    let s = severity_for(
        "H2",
        Some(&e),
        NOW,
        &CheckInputs {
            marketing_flush_lag: 2,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Unhealthy);
    assert!(
        s.sentence
            .contains("marketing FLUSH LAG: 2 event(s) pending"),
        "{}",
        s.sentence
    );
    assert_eq!(s.ran_at.as_deref(), Some(FRESH));
}

/// H2 arm 2 - a future stamp is never extra-fresh. It is ordered after the
/// flush guard and before absent evidence, so this fixture keeps the lag at
/// zero and supplies evidence.
#[test]
fn h2_future_evidence_is_degraded() {
    let e = ev("H2", FUTURE, true, "ok");
    let s = severity_for("H2", Some(&e), NOW, &CheckInputs::default());
    assert_eq!(s.severity, Severity::Degraded);
    assert!(s.sentence.contains("future"), "{}", s.sentence);
}

/// H2 arm 4, the dated half - active venues and a marketing log that has not
/// been written in a week. Both numbers come from the inputs; STALE is 41 days
/// before NOW, and the count is computed at render time rather than stored.
///
/// The undated half of this arm is deliberately not tested here. When
/// `last_marketing_write_at` is None, or a stamp `age_days` cannot parse,
/// `days_shown` is `i64::MAX` and the operator reads a nineteen-digit number
/// of days. That is residual H2-DAYS-SHOWN. A test asserting that number would
/// freeze it, and fixing it is a sentence change needing its own signature.
#[test]
fn h2_silent_marketing_names_days_and_venues() {
    let e = ev("H2", FRESH, true, "ok");
    let s = severity_for(
        "H2",
        Some(&e),
        NOW,
        &CheckInputs {
            active_venues: 3,
            last_marketing_write_at: Some(STALE.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);
    assert!(
        s.sentence
            .contains("marketing log silent 41 days with 3 active venues."),
        "{}",
        s.sentence
    );
}

/// H2 arm 4's threshold. Six days of silence with active venues is still
/// Healthy; the amber sentence starts at seven (GT-D6). Without this the
/// boundary could move a day in either direction unnoticed.
#[test]
fn h2_six_days_of_silence_is_still_healthy() {
    let e = ev("H2", FRESH, true, "ok");
    let six_days_ago = "2026-08-05T12:00:00.000Z";
    let s = severity_for(
        "H2",
        Some(&e),
        NOW,
        &CheckInputs {
            active_venues: 3,
            last_marketing_write_at: Some(six_days_ago.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Healthy);
    assert!(
        s.sentence.contains("last write 6 days ago"),
        "{}",
        s.sentence
    );
}

/// H2 arm 5, the quiet half - no venues means there is no silence to report,
/// and the sentence says that rather than naming a zero.
#[test]
fn h2_healthy_with_no_active_venues() {
    let e = ev("H2", FRESH, true, "ok");
    let s = severity_for("H2", Some(&e), NOW, &CheckInputs::default());
    assert_eq!(s.severity, Severity::Healthy);
    assert!(
        s.sentence
            .contains("marketing flush clean, no active venues"),
        "{}",
        s.sentence
    );
}

/// H2 arm 5, the working half - venues and a recent write. FRESH is one day
/// before NOW.
#[test]
fn h2_healthy_names_days_since_last_write() {
    let e = ev("H2", FRESH, true, "ok");
    let s = severity_for(
        "H2",
        Some(&e),
        NOW,
        &CheckInputs {
            active_venues: 2,
            last_marketing_write_at: Some(FRESH.into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Healthy);
    assert!(
        s.sentence.contains("last write 1 days ago"),
        "{}",
        s.sentence
    );
}

/// H2-DAYS-SHOWN sub-case A - the marketing log has never been written.
/// Before this ruling the sentence carried i64::MAX as a day count. It now
/// carries no day count at all: the only number in it is one the code
/// actually has. assert_eq! on the whole sentence, so no digit can hide.
#[test]
fn h2_never_written_marketing_log_names_no_day_count() {
    let e = ev("H2", FRESH, true, "ok");
    let s = severity_for(
        "H2",
        Some(&e),
        NOW,
        &CheckInputs {
            active_venues: 3,
            last_marketing_write_at: None,
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);
    assert_eq!(
        s.sentence,
        "H2 Capture paths — the marketing log has never been written, with 3 active venues."
    );
}

/// H2-DAYS-SHOWN sub-case B - a stamp `age_days` cannot parse. "Never been
/// written" would be false here: there is a write, only its instant is
/// unreadable. Same refusal to invent a number, different fact, different
/// sentence. The fixture string is the one f1b uses to prove compose_when
/// refuses a non-instant.
#[test]
fn h2_unreadable_write_stamp_names_no_day_count() {
    let e = ev("H2", FRESH, true, "ok");
    let s = severity_for(
        "H2",
        Some(&e),
        NOW,
        &CheckInputs {
            active_venues: 3,
            last_marketing_write_at: Some("not an instant".into()),
            ..CheckInputs::default()
        },
    );
    assert_eq!(s.severity, Severity::Degraded);
    assert_eq!(
        s.sentence,
        "H2 Capture paths — the marketing log's last write is not readable, with 3 active venues."
    );
}

// ---------------------------------------------------------------------------
// C1 (INT-001) — a reader that failed can never render calm.
//
// Before this fence `compute_status` wrote `unwrap_or(<calm>)` over seven
// reads. An events.jsonl whose last line was not JSON rendered H3 "snapshot
// today, FLUSH LAG 0" and H2 "marketing flush clean, no active venues" while
// the flush itself was aborting on the same unreadable watermark; a failed
// `money_debts` rendered M1 "nothing owed to you". The tests below hold the
// arms, the invariant, and the two end-to-end paths.
// ---------------------------------------------------------------------------

/// Every check's read-failure arm: Unhealthy, carries READ_FAILURE_PREFIX and
/// the error text verbatim, and composes none of the calm sentence it would
/// otherwise have rendered from the defaults beside the failure. Inputs arm
/// the calm path on purpose (snapshot today, fresh evidence), so each row is a
/// proof that the failure arm beats it.
#[test]
fn int001_each_read_failure_arm_is_unhealthy_and_names_the_failure() {
    let err = "no such table: fixture_off";
    let fail = |slot: fn(&mut ReadFailures, String)| {
        let mut f = ReadFailures::default();
        slot(&mut f, err.to_string());
        f
    };
    let table: Vec<(&str, ReadFailures, &str)> = vec![
        ("H2", fail(|f, e| f.marketing = Some(e)), "no active venues"),
        ("H3", fail(|f, e| f.log = Some(e)), "FLUSH LAG 0"),
        ("M1", fail(|f, e| f.money = Some(e)), "nothing owed to you"),
        (
            "M2",
            fail(|f, e| f.money = Some(e)),
            "every live promise is covered",
        ),
        ("M3", fail(|f, e| f.money = Some(e)), "nothing due"),
        ("M4", fail(|f, e| f.trail = Some(e)), "has a visible trail"),
        (
            "F1",
            fail(|f, e| f.shelf = Some(e)),
            "nothing is past its harvest date",
        ),
        (
            "F2",
            fail(|f, e| f.cover = Some(e)),
            "nothing is past its cover-check date",
        ),
    ];
    for (id, failures, calm) in table {
        let inputs = CheckInputs {
            snapshot_today: true,
            retention_ran: true,
            read_failures: failures,
            ..CheckInputs::default()
        };
        // Fresh, ok evidence for the H checks: the failure arm must beat the
        // evidence arms, not merely the absent-evidence arm.
        let e = ev(id, FRESH, true, "ok");
        let evidence = if id.starts_with('H') { Some(&e) } else { None };
        let s = severity_for(id, evidence, NOW, &inputs);
        assert_eq!(s.severity, Severity::Unhealthy, "{id}: {}", s.sentence);
        assert!(
            s.sentence.starts_with(&format!("{id} ")),
            "{id}: {}",
            s.sentence
        );
        assert!(
            s.sentence.contains(READ_FAILURE_PREFIX),
            "{id}: {}",
            s.sentence
        );
        assert!(s.sentence.contains(err), "{id}: {}", s.sentence);
        assert!(
            !s.sentence.contains(calm),
            "{id} composed the calm sentence beside a failed read: {}",
            s.sentence
        );
        assert!(s.ran_at.is_some(), "{id}: you looked; the read failed");
    }
}

/// The invariant: a failed slot beside a calm status is a violation, beside
/// non-Healthy statuses it holds, with the check absent it is a violation
/// (absence fails, as in `money_invariant_holds`), and with nothing failed it
/// is vacuous.
#[test]
fn int001_read_failure_invariant_forbids_calm_while_a_reader_failed() {
    let failed = ReadFailures {
        money: Some("no such table: wholesale_orders".into()),
        ..ReadFailures::default()
    };
    let poisoned = CheckInputs {
        read_failures: failed.clone(),
        ..CheckInputs::default()
    };
    let raising: Vec<CheckStatus> = ["M1", "M2", "M3"]
        .iter()
        .map(|id| severity_for(id, None, NOW, &poisoned))
        .collect();
    assert!(read_failure_invariant_holds(&failed, &raising));

    let calm: Vec<CheckStatus> = ["M1", "M2", "M3"]
        .iter()
        .map(|id| severity_for(id, None, NOW, &CheckInputs::default()))
        .collect();
    assert!(calm.iter().all(|s| s.severity == Severity::Healthy));
    assert!(!read_failure_invariant_holds(&failed, &calm));
    assert!(!read_failure_invariant_holds(&failed, &[]));
    assert!(read_failure_invariant_holds(
        &ReadFailures::default(),
        &calm
    ));
}

/// compute_status end to end: an events.jsonl whose last line is not JSON.
/// The calm path is fully armed first (snapshot from today, fresh ok evidence
/// for H2 and H3), so before C1 this farm rendered H3 "snapshot today, FLUSH
/// LAG 0" and H2 "marketing flush clean, no active venues".
#[test]
fn int001_unreadable_events_jsonl_renders_h3_and_h2_unhealthy_never_flush_lag_0() {
    let dir = tempfile_dir("c1-log");
    let farm = dir.join("farm.db");
    let snapshots_dir = dir.join("snapshots");
    let mut conn = db::open_and_migrate(&farm).unwrap();
    snapshots::take_snapshot(&mut conn, &snapshots_dir).unwrap();
    let now = db::utc_now_rfc3339();
    let today = chrono::Local::now().date_naive();
    crate::health::record(&conn, "H3", &now, true, "ok").unwrap();
    crate::health::record(&conn, "H2", &now, true, "ok").unwrap();
    let events = event_file::events_path(&dir);
    fs::write(&events, "{\"seq\": 1\n").unwrap();
    assert!(event_file::read_watermark(&events).is_err());

    let statuses = crate::health::compute_status(&conn, &dir, &snapshots_dir, &now, today).unwrap();
    let h3 = statuses.iter().find(|s| s.check_id == "H3").unwrap();
    assert_eq!(h3.severity, Severity::Unhealthy, "{}", h3.sentence);
    assert!(h3.sentence.contains(READ_FAILURE_PREFIX), "{}", h3.sentence);
    assert!(
        h3.sentence.contains("Flush lag is unknown"),
        "{}",
        h3.sentence
    );
    assert!(!h3.sentence.contains("FLUSH LAG 0"), "{}", h3.sentence);
    let h2 = statuses.iter().find(|s| s.check_id == "H2").unwrap();
    assert_eq!(h2.severity, Severity::Unhealthy, "{}", h2.sentence);
    assert!(h2.sentence.contains(READ_FAILURE_PREFIX), "{}", h2.sentence);
    assert!(
        h2.sentence.contains("Silence is unknown"),
        "{}",
        h2.sentence
    );
    assert!(!h2.sentence.contains("no active venues"), "{}", h2.sentence);
    // The register was readable: the shelf checks stay on their own facts.
    for id in ["F1", "F2"] {
        let s = statuses.iter().find(|s| s.check_id == id).unwrap();
        assert_eq!(s.severity, Severity::Healthy, "{id}: {}", s.sentence);
    }
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

/// compute_status end to end: the order book cannot be read. M1, M2 and M3
/// render the failure and name the table; F1 and F2 read a different table
/// and keep their own facts — slots do not bleed.
#[test]
fn int001_unreadable_order_book_renders_m1_m2_m3_unhealthy_and_leaves_f1_f2_alone() {
    let dir = tempfile_dir("c1-money");
    let snapshots_dir = dir.join("snapshots");
    let conn = db::open_in_memory().unwrap();
    conn.execute_batch("ALTER TABLE wholesale_orders RENAME TO wholesale_orders_off;")
        .unwrap();
    let now = db::utc_now_rfc3339();
    let today = chrono::Local::now().date_naive();
    let statuses = crate::health::compute_status(&conn, &dir, &snapshots_dir, &now, today).unwrap();
    for (id, calm) in [
        ("M1", "nothing owed to you"),
        ("M2", "every live promise is covered"),
        ("M3", "nothing due"),
    ] {
        let s = statuses.iter().find(|s| s.check_id == id).unwrap();
        assert_eq!(s.severity, Severity::Unhealthy, "{id}: {}", s.sentence);
        assert!(
            s.sentence.contains(READ_FAILURE_PREFIX),
            "{id}: {}",
            s.sentence
        );
        assert!(
            s.sentence.contains("wholesale_orders"),
            "{id}: {}",
            s.sentence
        );
        assert!(!s.sentence.contains(calm), "{id}: {}", s.sentence);
    }
    for id in ["F1", "F2"] {
        let s = statuses.iter().find(|s| s.check_id == id).unwrap();
        assert_eq!(s.severity, Severity::Healthy, "{id}: {}", s.sentence);
    }
    let _ = fs::remove_dir_all(&dir);
}

/// compute_status end to end: the shelf cannot be read. F1 and F2 render the
/// failure and name the table.
#[test]
fn int001_unreadable_shelf_renders_f1_and_f2_unhealthy() {
    let dir = tempfile_dir("c1-shelf");
    let snapshots_dir = dir.join("snapshots");
    let conn = db::open_in_memory().unwrap();
    conn.execute_batch("ALTER TABLE trays RENAME TO trays_off;")
        .unwrap();
    let now = db::utc_now_rfc3339();
    let today = chrono::Local::now().date_naive();
    let statuses = crate::health::compute_status(&conn, &dir, &snapshots_dir, &now, today).unwrap();
    for (id, calm) in [
        ("F1", "nothing is past its harvest date"),
        ("F2", "nothing is past its cover-check date"),
    ] {
        let s = statuses.iter().find(|s| s.check_id == id).unwrap();
        assert_eq!(s.severity, Severity::Unhealthy, "{id}: {}", s.sentence);
        assert!(
            s.sentence.contains(READ_FAILURE_PREFIX),
            "{id}: {}",
            s.sentence
        );
        assert!(s.sentence.contains("trays"), "{id}: {}", s.sentence);
        assert!(!s.sentence.contains(calm), "{id}: {}", s.sentence);
    }
    let _ = fs::remove_dir_all(&dir);
}

/// compute_status end to end: the correction trail cannot be read. M4 renders
/// the failure and never claims every correction has a visible trail.
#[test]
fn int001_unreadable_correction_trail_renders_m4_unhealthy() {
    let dir = tempfile_dir("c1-trail");
    let snapshots_dir = dir.join("snapshots");
    let conn = db::open_in_memory().unwrap();
    conn.execute_batch("ALTER TABLE money_corrections RENAME TO money_corrections_off;")
        .unwrap();
    let now = db::utc_now_rfc3339();
    let today = chrono::Local::now().date_naive();
    let statuses = crate::health::compute_status(&conn, &dir, &snapshots_dir, &now, today).unwrap();
    let m4 = statuses.iter().find(|s| s.check_id == "M4").unwrap();
    assert_eq!(m4.severity, Severity::Unhealthy, "{}", m4.sentence);
    assert!(m4.sentence.contains(READ_FAILURE_PREFIX), "{}", m4.sentence);
    assert!(m4.sentence.contains("money_corrections"), "{}", m4.sentence);
    assert!(
        !m4.sentence.contains("has a visible trail"),
        "{}",
        m4.sentence
    );
    let _ = fs::remove_dir_all(&dir);
}
