//! Post-cutover expense correct/void + money_corrections proofs (c1–c13).

use crate::costs::{self, CorrectExpenseInput, RecordCostInput, COST_FORBIDDEN_COMPUTED_KEYS};
use crate::db;
use crate::event_file;
use crate::event_partition::{EventClass, EventDomain, Kind};
use crate::export;
use crate::identity::LOCK_ACTIVE_IN_TEST;
use crate::import;
use crate::projection;
use chrono::Local;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "groundtruth-correction-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn open_farm(dir: &Path) -> Connection {
    db::open_and_migrate(&dir.join("farm.db")).unwrap()
}

fn basic_cost(amount_cents: i64) -> RecordCostInput {
    RecordCostInput {
        amount_cents,
        payee: "Seed Co".into(),
        category_id: "seed".into(),
        date_paid: today(),
        descriptor: None,
        receipt_source_path: None,
    }
}

fn record_one(conn: &mut Connection, dir: &Path, amount: i64) -> costs::CostEventView {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    costs::record_cost(conn, dir, basic_cost(amount)).unwrap()
}

#[test]
fn c1_new_kinds_tier_money_out_and_counts() {
    assert_eq!(
        Kind::CostMoneyOutCorrected.tier(),
        (EventDomain::Register, Some(EventClass::MoneyOut))
    );
    assert_eq!(
        Kind::CostMoneyOutVoided.tier(),
        (EventDomain::Register, Some(EventClass::MoneyOut))
    );
    // Bumped for the silent-shortfall residual (wholesale.write_off).
    assert_eq!(Kind::ALL.len(), 54);
    assert_eq!(EventClass::ALL.len(), 8);
}

#[test]
fn c2_correct_leaves_original_event_and_updates_in_place() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let dir = temp_dir("c2");
    let mut conn = open_farm(&dir);
    let original = record_one(&mut conn, &dir, 500);
    let original_payload: String = conn
        .query_row(
            "SELECT payload FROM event_log WHERE id = ?1",
            [&original.event_id],
            |r| r.get(0),
        )
        .unwrap();
    let before_log: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();

    let corrected = costs::correct_expense(
        &mut conn,
        CorrectExpenseInput {
            target_event_id: original.event_id.clone(),
            amount_cents: 700,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            reason: Some("typo".into()),
        },
    )
    .unwrap();

    let after_payload: String = conn
        .query_row(
            "SELECT payload FROM event_log WHERE id = ?1",
            [&original.event_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after_payload, original_payload);
    let after_log: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after_log, before_log + 1);
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cost_events WHERE event_id = ?1",
            [&original.event_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(corrected.amount_cents, 700);
    assert_eq!(corrected.event_id, original.event_id);
    assert_ne!(corrected.last_event_id, original.event_id);

    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c3_void_sets_voided_at_and_leaves_figure_readable() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let dir = temp_dir("c3");
    let mut conn = open_farm(&dir);
    let original = record_one(&mut conn, &dir, 900);
    costs::void_expense(&mut conn, &original.event_id, Some("mistake".into())).unwrap();

    let (amount, voided): (i64, Option<String>) = conn
        .query_row(
            "SELECT amount_cents, voided_at FROM cost_events WHERE event_id = ?1",
            [&original.event_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(amount, 900);
    assert!(voided.is_some());
    let active = costs::list_expenses(&conn).unwrap();
    assert!(active.iter().all(|e| e.event_id != original.event_id));

    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c4_voided_expense_cannot_be_corrected_or_voided_again() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let dir = temp_dir("c4");
    let mut conn = open_farm(&dir);
    let original = record_one(&mut conn, &dir, 400);
    costs::void_expense(&mut conn, &original.event_id, None).unwrap();

    let correct_err = costs::correct_expense(
        &mut conn,
        CorrectExpenseInput {
            target_event_id: original.event_id.clone(),
            amount_cents: 450,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            reason: None,
        },
    )
    .unwrap_err();
    assert!(correct_err.contains("already voided"), "{correct_err}");
    let void_err = costs::void_expense(&mut conn, &original.event_id, None).unwrap_err();
    assert!(void_err.contains("already voided"), "{void_err}");

    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c5_money_corrections_is_append_only() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let dir = temp_dir("c5");
    let mut conn = open_farm(&dir);
    let original = record_one(&mut conn, &dir, 300);
    costs::correct_expense(
        &mut conn,
        CorrectExpenseInput {
            target_event_id: original.event_id.clone(),
            amount_cents: 350,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            reason: None,
        },
    )
    .unwrap();

    let id: String = conn
        .query_row(
            "SELECT correction_event_id FROM money_corrections LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let update_err = conn
        .execute(
            "UPDATE money_corrections SET reason = 'x' WHERE correction_event_id = ?1",
            [&id],
        )
        .unwrap_err()
        .to_string();
    assert!(
        update_err.contains("money_corrections is append-only"),
        "{update_err}"
    );
    let delete_err = conn
        .execute(
            "DELETE FROM money_corrections WHERE correction_event_id = ?1",
            [&id],
        )
        .unwrap_err()
        .to_string();
    assert!(
        delete_err.contains("money_corrections is append-only"),
        "{delete_err}"
    );

    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c6_trail_shows_both_sides_of_a_correction() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let dir = temp_dir("c6");
    let mut conn = open_farm(&dir);
    let original = record_one(&mut conn, &dir, 500);
    costs::correct_expense(
        &mut conn,
        CorrectExpenseInput {
            target_event_id: original.event_id.clone(),
            amount_cents: 800,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            reason: Some("fix".into()),
        },
    )
    .unwrap();
    let trail = costs::list_money_corrections(&conn).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].before_amount_cents, 500);
    assert_eq!(trail[0].after_amount_cents, Some(800));
    assert_ne!(
        trail[0].before_amount_cents,
        trail[0].after_amount_cents.unwrap()
    );

    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c7_verify_replay_passes_with_corrections_compared() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let dir = temp_dir("c7");
    let mut conn = open_farm(&dir);
    let a = record_one(&mut conn, &dir, 500);
    let b = record_one(&mut conn, &dir, 600);
    costs::correct_expense(
        &mut conn,
        CorrectExpenseInput {
            target_event_id: a.event_id.clone(),
            amount_cents: 550,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            reason: None,
        },
    )
    .unwrap();
    costs::void_expense(&mut conn, &b.event_id, Some("drop".into())).unwrap();
    event_file::flush_events(&conn, &dir).unwrap();
    drop(conn);

    let outcome = projection::farm_dir_verify(&dir).unwrap();
    assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
    let exclusions = outcome.report().exclusions.join("\n");
    assert!(
        !exclusions.contains("money_corrections"),
        "money_corrections must not be excluded:\n{exclusions}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c8_export_import_round_trips_corrections() {
    LOCK_ACTIVE_IN_TEST.with(|c| c.set(false));
    let src = temp_dir("c8-src");
    let mut conn = open_farm(&src);
    let original = record_one(&mut conn, &src, 500);
    costs::correct_expense(
        &mut conn,
        CorrectExpenseInput {
            target_event_id: original.event_id.clone(),
            amount_cents: 650,
            payee: "Seed Co".into(),
            category_id: "seed".into(),
            date_paid: today(),
            descriptor: None,
            reason: Some("round-trip".into()),
        },
    )
    .unwrap();
    event_file::flush_events(&conn, &src).unwrap();
    let bundle = PathBuf::from(export::export_bundle(&conn, &src).unwrap().bundle_path);
    assert!(bundle.join("money-corrections.csv").is_file());
    drop(conn);

    let tgt = temp_dir("c8-tgt");
    let mut tgt_conn = open_farm(&tgt);
    import::apply_import(&mut tgt_conn, &bundle).unwrap();
    let trail = costs::list_money_corrections(&tgt_conn).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].before_amount_cents, 500);
    assert_eq!(trail[0].after_amount_cents, Some(650));
    let amount: i64 = tgt_conn
        .query_row(
            "SELECT amount_cents FROM cost_events WHERE event_id = ?1",
            [&original.event_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(amount, 650);

    drop(tgt_conn);
    let _ = fs::remove_dir_all(&src);
    let _ = fs::remove_dir_all(&tgt);
}

#[test]
fn c9_non_goal_no_ledger_debit_credit_fields() {
    let dir = temp_dir("c9");
    let conn = open_farm(&dir);
    let forbidden = [
        "%ledger%",
        "%debit%",
        "%credit%",
        "%receivable%",
        "%invoice_total%",
        "%period_close%",
        "%accrual%",
    ];
    {
        let mut stmt = conn
            .prepare(
                "SELECT m.name, p.name FROM sqlite_master m
                 JOIN pragma_table_info(m.name) p
                 WHERE m.type = 'table'",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap();
        for row in rows {
            let (table, col) = row.unwrap();
            let lower = col.to_lowercase();
            for pat in forbidden {
                let needle = pat.trim_matches('%');
                assert!(
                    !lower.contains(needle),
                    "table {table} column {col} matches non-goal {pat}"
                );
            }
        }
    }
    let _ = COST_FORBIDDEN_COMPUTED_KEYS;
    // Payload field lists for new structs — sealed via allowed key sets.
    for key in costs::COST_CORRECT_PAYLOAD_KEYS
        .iter()
        .chain(costs::COST_VOID_PAYLOAD_KEYS.iter())
    {
        let lower = key.to_lowercase();
        for needle in [
            "ledger",
            "debit",
            "credit",
            "receivable",
            "invoicetotal",
            "periodclose",
            "accrual",
        ] {
            assert!(
                !lower.contains(needle),
                "payload key {key} matches non-goal {needle}"
            );
        }
    }
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c11_track_check_rejects_income() {
    let dir = temp_dir("c11");
    let conn = open_farm(&dir);
    let err = conn
        .execute(
            "INSERT INTO money_corrections
             (correction_event_id, target_event_id, track, action,
              before_json, after_json, before_amount_cents, after_amount_cents,
              before_date, after_date, before_payee, after_payee, reason, corrected_at)
             VALUES ('c11', 'target', 'income', 'corrected',
                     '{}', '{}', 100, 100,
                     '2026-08-12', '2026-08-12', 'Payee', 'Payee', NULL, '2026-08-12T00:00:00Z')",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("CHECK constraint"),
        "income track must abort on CHECK, got: {err}"
    );
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn c13_no_test_pins_the_schema_version_as_a_literal() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let n = db::SCHEMA_VERSION;
    let needles = [
        format!("assert_eq!(version, {n})"),
        format!("assert_eq!(ver, {n})"),
        format!("assert_eq!(db::SCHEMA_VERSION, {n})"),
        format!("*t == {n}"),
        format!("assert_eq!({n}, version)"),
        format!("assert_eq!({n}, ver)"),
        format!("assert_eq!({n}, db::SCHEMA_VERSION)"),
    ];
    // Deliberate tripwire, not a mirror: this pin must fail when the schema
    // is bumped, so the cost payload keys get re-checked at the new version.
    //
    // H-11(b) - keyed by the enclosing test, not by a line number. The needles
    // carry the tripwire: at the next schema version every pin stops matching
    // and all three trip however they are keyed. This list only answers WHICH
    // three sites are deliberate, and that answer does not move when a file is
    // reformatted or gains a line above a pin. The line-number form was one
    // inserted line away from red on three files, and rustfmt collected it.
    let allow: &[&str] = &[
        "cost_event_tests.rs::phase2_schema_version_10_payload_keys_unchanged",
        "marketing_tests.rs::gt17e_second_kind_in_closed_set_and_v29_triggers_refuse_until_v30",
        "phone_pull_tests.rs::f2a_schema_v34_tables_exist_reference_and_logs_excluded_idempotent",
    ];
    for ent in fs::read_dir(&src).unwrap() {
        let path = ent.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name != "lib.rs" && !name.ends_with("_tests.rs") {
            continue;
        }
        // Last `fn` seen wins. A commented-out signature cannot set it: the
        // strip runs on the trimmed line, which still begins with the slashes.
        let mut owner_fn = String::new();
        for (i, line) in fs::read_to_string(&path).unwrap().lines().enumerate() {
            let t = line.trim_start();
            if let Some(rest) = t
                .strip_prefix("fn ")
                .or_else(|| t.strip_prefix("pub fn "))
                .or_else(|| t.strip_prefix("async fn "))
                .or_else(|| t.strip_prefix("pub async fn "))
            {
                owner_fn = rest
                    .split(|c: char| c == '(' || c == '<' || c.is_whitespace())
                    .next()
                    .unwrap_or("")
                    .to_string();
            }
            if t.starts_with("//") {
                continue;
            }
            let owner = format!("{name}::{owner_fn}");
            if allow.contains(&owner.as_str()) {
                continue;
            }
            assert!(
                !needles.iter().any(|p| line.contains(p.as_str())),
                "SCHEMA_VERSION pinned as a literal in {owner} (line {}): {}",
                i + 1,
                line.trim()
            );
        }
    }
}

#[test]
fn c12_money_screen_label_matches_the_trail() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/screens/Money.tsx");
    let src = fs::read_to_string(&path).expect("read Money.tsx");
    assert!(
        src.contains("Expense corrections"),
        "Money.tsx must label the trail as expense-only"
    );
    assert!(
        !src.contains(">Corrections</h2>"),
        "Money.tsx must not use a bare Corrections heading that implies wider coverage"
    );
}
