//! Health status is COMPUTED, never stored. `health_evidence` holds rows only.
//! A check that crashes writes no row; absent evidence renders Degraded.
//! Computing is the only path to green.

use crate::attention::{self, MoneyDebts};
use crate::event_file;
use crate::projection;
use crate::snapshots;
use crate::storefront;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckStatus {
    pub check_id: String,
    pub severity: Severity,
    pub sentence: String,
    pub ran_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Evidence {
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub check_id: String,
    pub ran_at: String,
    pub ok: bool,
    pub detail: String,
}

/// Inputs gathered outside `severity_for` so that function stays pure.
#[derive(Debug, Clone, Default)]
pub struct CheckInputs {
    pub consecutive_failures: i64,
    pub varieties_parsed: Option<usize>,
    pub structural_miss: Option<String>,
    /// Flush lag via `event_file::read_watermark` (live_max_seq - watermark).
    pub flush_lag: i64,
    pub snapshot_today: bool,
    pub retention_ran: bool,
    pub quick_check_ok: bool,
    pub last_full_verify_ok_at: Option<String>,
    pub last_full_verify_failed: bool,
    pub unknown_divergences: bool,
    /// True when no full verify has EVER run on this farm (no
    /// last-verify-replay.txt and no verify-flavored H4 evidence).
    pub never_verified: bool,
    /// The last verify could not run. Nothing was compared.
    pub last_verify_incomplete: bool,
    /// The exact VERIFY-REPLAY line the verdict came from.
    pub last_verify_outcome_line: Option<String>,
    /// Flush lag for the marketing partition of events.jsonl.
    pub marketing_flush_lag: i64,
    /// Every venue not archived, at any stage. `h2_facts` reads it as
    /// `SELECT COUNT(*) FROM mkt_venues WHERE archived_at IS NULL`, and H2's
    /// silence arm names it to the operator. The previous wording here — "at
    /// or past 'sampled' and not yet 'standing'" — described a filter this
    /// field has never carried.
    pub active_venues: i64,
    /// UTC RFC3339 of the most recent marketing event_log write.
    /// None means the marketing log has never been written.
    pub last_marketing_write_at: Option<String>,
    /// events.jsonl watermark is ahead of event_log max seq — the log and
    /// database have forked (seen after a restore). Flush is jammed.
    pub log_ahead: bool,
    pub log_watermark: i64,
    pub db_max_seq: i64,
    /// B4 — the same money debts B1 raises Today's cards from. Read from live
    /// tables via `attention::money_debts`, never from the attention table, so
    /// a dismissal can quiet Today without ever quieting Health.
    pub money: MoneyDebts,
    /// OWED-LO (audit R-1): priced-unpaid leftover listings via
    /// `leftover::owed_leftover` — the same evaluator wholesale::owed_summary
    /// reads for the Money / Today owed line. Counted into M1's owed amount.
    /// Deliberately NOT a money debt: MoneyDebts stays wholesale-only, so
    /// Today raises no leftover card (GT-D24-B scope; ruling 3 of OWED-LO).
    pub leftover_owed: crate::leftover::LeftoverOwed,
    /// B4/M4 — income voids + corrections this quarter with no visible trail row.
    pub untrailed_income_corrections: i64,
    /// "Q3 2026". Empty only in hand-built test inputs.
    pub quarter_label: String,
    /// F1 — trays past their harvest date and still on the shelf, from
    /// `trays::overdue_trays_on_shelf`. The same reader
    /// `reachability::cover_message` uses; never a second tray query.
    pub overdue_trays: i64,
    /// F2 — trays still under cover past their cover-check date, summed from
    /// `trays::overdue_light_groups` at `COVER_HEALTH_DAYS_PAST`. The card
    /// reads the same query at `COVER_CARD_DAYS_PAST`. Never the attention
    /// table — a dismissal is one episode on one local day and must never
    /// quiet Health.
    pub overdue_light_trays: i64,
    /// C1 (INT-001) — a reader that failed. Each slot names the check(s) it
    /// feeds; a check whose slot is `Some` renders the failure before any other
    /// arm, so the defaulted value beside it is never read. Before this fence
    /// every one of these reads was `unwrap_or(<calm>)` in `compute_status`,
    /// and an unreadable events.jsonl rendered "snapshot today, FLUSH LAG 0".
    pub read_failures: ReadFailures,
}

/// C1 (INT-001). One slot per reader `compute_status` consults. `Some(e)` is
/// the error text verbatim — never a count, an age or an amount in its place.
#[derive(Debug, Clone, Default)]
pub struct ReadFailures {
    /// `flush_lag` / `log_fork` (events.jsonl watermark, event_log) → H3.
    pub log: Option<String>,
    /// `h2_facts` (watermark, marketing rows, mkt_venues) → H2.
    pub marketing: Option<String>,
    /// `attention::money_debts` (order book, cover plan) and
    /// `leftover::owed_leftover` (leftover listings) → M1, M2, M3.
    pub money: Option<String>,
    /// `untrailed_income_corrections` (event_log, money_corrections) → M4.
    pub trail: Option<String>,
    /// `trays::overdue_trays_on_shelf` → F1.
    pub shelf: Option<String>,
    /// `trays::overdue_light_trays` → F2.
    pub cover: Option<String>,
}

/// The words every read-failure sentence carries. The desk keys its recovery
/// on this exact phrase (healthScopes.ts `recoveryFor`): a check whose reader
/// failed confirmed no debt, tray or log fact, so the id's own steps would
/// presume one.
pub const READ_FAILURE_PREFIX: &str = "could not read";

/// The read-failure arm, one shape for all eight checks. Unhealthy (D1): an
/// unreadable log is the same class as H3's fork arm, an unreadable register
/// the same class as H4's quick_check FAIL. The sentence carries the error
/// text and nothing invented beside it.
fn read_failure_status(
    check_id: &str,
    label: &str,
    subject: &str,
    error: &str,
    tail: &str,
    ran_at: Option<String>,
) -> CheckStatus {
    CheckStatus {
        check_id: check_id.into(),
        severity: Severity::Unhealthy,
        sentence: format!("{check_id} {label} — {READ_FAILURE_PREFIX} {subject}: {error}.{tail}"),
        ran_at,
    }
}

pub const OWED_UNHEALTHY_DAYS: i64 = 14;

/// The checks Health reports, in render order: Farm scope first, then System.
///
/// H1 is absent by ruling 6.1. The Storefront observer it read was removed from
/// the Farm surface, so a check claiming to read it would be reporting on a
/// surface that no longer exists — and on a URL that is none of Groundtruth's
/// business. `severity_h1` and its truth table stay in the tree; the check is
/// no longer reported and is no longer fed at app start.
pub const REPORTED_CHECKS: [&str; 9] = ["M1", "M2", "M3", "F1", "F2", "H2", "H3", "H4", "M4"];

pub fn record(
    conn: &Connection,
    check_id: &str,
    ran_at_utc: &str,
    ok: bool,
    detail: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO health_evidence (id, check_id, ran_at, ok, detail)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            Uuid::new_v4().to_string(),
            check_id,
            ran_at_utc,
            if ok { 1 } else { 0 },
            detail,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn latest_evidence(conn: &Connection, check_id: &str) -> Result<Option<Evidence>, String> {
    conn.query_row(
        "SELECT check_id, ran_at, ok, detail FROM health_evidence
         WHERE check_id = ?1
         ORDER BY ran_at DESC
         LIMIT 1",
        params![check_id],
        |r| {
            Ok(Evidence {
                check_id: r.get(0)?,
                ran_at: r.get(1)?,
                ok: r.get::<_, i64>(2)? == 1,
                detail: r.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Pure function of (evidence, now, inputs). No I/O.
pub fn severity_for(
    check_id: &str,
    evidence: Option<&Evidence>,
    now_utc: &str,
    extra: &CheckInputs,
) -> CheckStatus {
    match check_id {
        "H1" => severity_h1(evidence, now_utc, extra),
        "H2" => severity_h2(evidence, now_utc, extra),
        "H3" => severity_h3(evidence, now_utc, extra),
        "H4" => severity_h4(evidence, now_utc, extra),
        "M1" => severity_m1(now_utc, extra),
        "M2" => severity_m2(now_utc, extra),
        "M3" => severity_m3(now_utc, extra),
        "M4" => severity_m4(now_utc, extra),
        "F1" => severity_f1(now_utc, extra),
        "F2" => severity_f2(now_utc, extra),
        other => CheckStatus {
            check_id: other.to_string(),
            severity: Severity::Degraded,
            sentence: format!("Check {other} hasn't reported since never"),
            ran_at: None,
        },
    }
}

/// Pure H2 severity (GT-D6 silence honesty). Silence and skew are computed
/// at render time from facts + now — never from a precomputed *_days field.
fn severity_h2(evidence: Option<&Evidence>, now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = evidence.map(|e| e.ran_at.clone());

    // 0. C1 (INT-001): the marketing facts could not be read. Ahead of every
    // other arm — flush lag, silence and venue count are all unknown.
    if let Some(e) = &extra.read_failures.marketing {
        return read_failure_status(
            "H2",
            "Capture paths",
            "the marketing log",
            e,
            " Silence is unknown.",
            ran_at,
        );
    }

    // 1. Unhealthy: marketing partition fails the flush guard.
    if extra.marketing_flush_lag > 0 {
        return CheckStatus {
            check_id: "H2".into(),
            severity: Severity::Unhealthy,
            sentence: format!(
                "H2 Capture paths — marketing FLUSH LAG: {} event(s) pending",
                extra.marketing_flush_lag
            ),
            ran_at,
        };
    }

    // 2. Clock skew — never treat a future timestamp as extra-fresh.
    if let Some(ev) = evidence {
        if ran_at_in_future(&ev.ran_at, now_utc) {
            return CheckStatus {
                check_id: "H2".into(),
                severity: Severity::Degraded,
                sentence: "H2 Capture paths — evidence is timestamped in the future.".into(),
                ran_at,
            };
        }
    }

    // 3. Absent evidence.
    if evidence.is_none() {
        return CheckStatus {
            check_id: "H2".into(),
            severity: Severity::Degraded,
            sentence: "H2 Capture paths — hasn't reported since never.".into(),
            ran_at: None,
        };
    }

    // 4. Degraded: active venues AND marketing log silent ≥ 7 days (GT-D6).
    //
    // H2-DAYS-SHOWN (signed 2026-08-25). Silence is reported as a number only
    // when a number exists, and `age_days` on a real stamp is the only thing
    // that produces one. A log that was never written and a stamp this build
    // cannot parse are different facts, and each says so in its own words —
    // "never been written" would be false of the second, because there is a
    // write and only its instant is unreadable.
    //
    // Before this ruling both fell through `unwrap_or(i64::MAX)` and a
    // `.max(7)` that is a no-op on i64::MAX, and the operator read a
    // nineteen-digit day count.
    //
    // All three branches return from inside this arm. Falling through to
    // arm 5 would print "last write 0 days ago" for a log with no write —
    // the same lie in a quieter voice — because that branch's `unwrap_or(0)`
    // is unreachable only for as long as this one returns.
    if extra.active_venues > 0 {
        match extra.last_marketing_write_at.as_deref() {
            None => {
                return CheckStatus {
                    check_id: "H2".into(),
                    severity: Severity::Degraded,
                    sentence: format!(
                        "H2 Capture paths — the marketing log has never been written, \
                         with {} active venues.",
                        extra.active_venues
                    ),
                    ran_at,
                };
            }
            Some(when) => match age_days(when, now_utc) {
                None => {
                    return CheckStatus {
                        check_id: "H2".into(),
                        severity: Severity::Degraded,
                        sentence: format!(
                            "H2 Capture paths — the marketing log's last write is not readable, \
                             with {} active venues.",
                            extra.active_venues
                        ),
                        ran_at,
                    };
                }
                Some(days_shown) => {
                    if days_shown >= 7 {
                        return CheckStatus {
                            check_id: "H2".into(),
                            severity: Severity::Degraded,
                            sentence: format!(
                                "H2 Capture paths — marketing log silent {days_shown} days \
                                 with {} active venues.",
                                extra.active_venues
                            ),
                            ran_at,
                        };
                    }
                }
            },
        }
    }

    // 5. Healthy: flush clean, and either no active venues or written within 7 days.
    CheckStatus {
        check_id: "H2".into(),
        severity: Severity::Healthy,
        sentence: if extra.active_venues == 0 {
            "H2 Capture paths — marketing flush clean, no active venues".into()
        } else {
            let days = extra
                .last_marketing_write_at
                .as_deref()
                .and_then(|when| age_days(when, now_utc))
                .unwrap_or(0);
            format!("H2 Capture paths — marketing flush clean, last write {days} days ago")
        },
        ran_at,
    }
}

fn severity_h1(evidence: Option<&Evidence>, now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = evidence.map(|e| e.ran_at.clone());
    if let Some(ev) = evidence {
        if ran_at_in_future(&ev.ran_at, now_utc) {
            return CheckStatus {
                check_id: "H1".into(),
                severity: Severity::Degraded,
                sentence: "H1 evidence is timestamped in the future".into(),
                ran_at,
            };
        }
    }

    if extra.consecutive_failures >= 3 {
        return CheckStatus {
            check_id: "H1".into(),
            severity: Severity::Unhealthy,
            sentence: format!(
                "H1 Storefront readable — {} consecutive failures",
                extra.consecutive_failures
            ),
            ran_at,
        };
    }

    if let Some(miss) = &extra.structural_miss {
        return CheckStatus {
            check_id: "H1".into(),
            severity: Severity::Unhealthy,
            sentence: miss.clone(),
            ran_at,
        };
    }

    match evidence {
        None => CheckStatus {
            check_id: "H1".into(),
            severity: Severity::Degraded,
            sentence: "Check H1 hasn't reported since never".into(),
            ran_at: None,
        },
        Some(ev) => {
            let age = age_days(&ev.ran_at, now_utc);
            if !ev.ok {
                return CheckStatus {
                    check_id: "H1".into(),
                    severity: Severity::Unhealthy,
                    sentence: ev.detail.clone(),
                    ran_at,
                };
            }
            if age.map(|d| d > 7).unwrap_or(true) {
                return CheckStatus {
                    check_id: "H1".into(),
                    severity: Severity::Degraded,
                    sentence: format!("Check H1 hasn't reported since {}", ev.ran_at),
                    ran_at,
                };
            }
            if extra.varieties_parsed == Some(5) {
                CheckStatus {
                    check_id: "H1".into(),
                    severity: Severity::Healthy,
                    sentence: "H1 Storefront readable — 5 of 5 varieties".into(),
                    ran_at,
                }
            } else {
                CheckStatus {
                    check_id: "H1".into(),
                    severity: Severity::Degraded,
                    sentence: format!("Check H1 hasn't reported since {}", ev.ran_at),
                    ran_at,
                }
            }
        }
    }
}

fn severity_h3(evidence: Option<&Evidence>, now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = evidence.map(|e| e.ran_at.clone());

    // C1 (INT-001): the log could not be read. Ahead of every other arm —
    // flush lag and fork state are unknown, so neither "FLUSH LAG 0" nor the
    // fork sentence may be composed from the defaults beside the failure.
    if let Some(e) = &extra.read_failures.log {
        return read_failure_status(
            "H3",
            "Critical jobs",
            "the log",
            e,
            " Flush lag is unknown.",
            ran_at,
        );
    }

    if let Some(ev) = evidence {
        if ran_at_in_future(&ev.ran_at, now_utc) {
            return CheckStatus {
                check_id: "H3".into(),
                severity: Severity::Degraded,
                sentence: "H3 Critical jobs — evidence is timestamped in the future.".into(),
                ran_at,
            };
        }
    }

    if evidence.is_none() {
        return CheckStatus {
            check_id: "H3".into(),
            severity: Severity::Degraded,
            sentence: "H3 Critical jobs — hasn't reported since never.".into(),
            ran_at: None,
        };
    }

    if extra.log_ahead {
        return CheckStatus {
            check_id: "H3".into(),
            severity: Severity::Unhealthy,
            sentence: format!(
                "H3 Critical jobs — events.jsonl is AHEAD of the database \
                 (log seq {} > database seq {}). The log and database have \
                 forked; new events cannot reach the log.",
                extra.log_watermark, extra.db_max_seq
            ),
            ran_at,
        };
    }

    // Unhealthy: FLUSH LAG > 0 — immediately, no grace.
    if extra.flush_lag > 0 {
        return CheckStatus {
            check_id: "H3".into(),
            severity: Severity::Unhealthy,
            sentence: format!(
                "H3 Critical jobs — FLUSH LAG: {} event(s) pending",
                extra.flush_lag
            ),
            ran_at,
        };
    }

    if !extra.snapshot_today || !extra.retention_ran {
        return CheckStatus {
            check_id: "H3".into(),
            severity: Severity::Degraded,
            sentence: if !extra.snapshot_today {
                "H3 Critical jobs — no snapshot from today".into()
            } else {
                "H3 Critical jobs — retention has not run".into()
            },
            ran_at,
        };
    }

    CheckStatus {
        check_id: "H3".into(),
        severity: Severity::Healthy,
        sentence: "H3 Critical jobs — snapshot today, FLUSH LAG 0".into(),
        ran_at,
    }
}

fn severity_h4(evidence: Option<&Evidence>, now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = evidence.map(|e| e.ran_at.clone());
    if let Some(ev) = evidence {
        if ran_at_in_future(&ev.ran_at, now_utc) {
            return CheckStatus {
                check_id: "H4".into(),
                severity: Severity::Degraded,
                sentence: "H4 Core data intact — evidence is timestamped in the future.".into(),
                ran_at,
            };
        }
    }

    if evidence.is_none() {
        return CheckStatus {
            check_id: "H4".into(),
            severity: Severity::Degraded,
            sentence: "H4 Core data intact — hasn't reported since never.".into(),
            ran_at: None,
        };
    }

    if !extra.quick_check_ok {
        return CheckStatus {
            check_id: "H4".into(),
            severity: Severity::Unhealthy,
            sentence: "H4 Core data intact — PRAGMA quick_check FAIL".into(),
            ran_at,
        };
    }

    if extra.last_verify_incomplete {
        let line = extra
            .last_verify_outcome_line
            .clone()
            .unwrap_or_else(|| projection::VERIFY_INCOMPLETE_PREFIX.to_string());
        return CheckStatus {
            check_id: "H4".into(),
            severity: Severity::Unhealthy,
            sentence: format!(
                "H4 Core data intact — quick_check ok; the last verify could not \
                 complete: {line}. Nothing was compared. Read last-verify-replay.txt \
                 in the farm folder."
            ),
            ran_at,
        };
    }

    if extra.last_full_verify_failed && extra.unknown_divergences {
        return CheckStatus {
            check_id: "H4".into(),
            severity: Severity::Unhealthy,
            sentence: "H4 Core data intact — quick_check ok; verify FAILED with unexplained \
                 divergences. Read last-verify-replay.txt in the farm folder."
                .into(),
            ran_at,
        };
    }

    if extra.last_full_verify_failed && !extra.unknown_divergences {
        let line = extra
            .last_verify_outcome_line
            .clone()
            .unwrap_or_else(|| "VERIFY-REPLAY: FAIL".to_string());
        return CheckStatus {
            check_id: "H4".into(),
            severity: Severity::Unhealthy,
            sentence: format!(
                "H4 Core data intact — quick_check ok; the last verify did not pass: \
                 {line}. Read last-verify-replay.txt in the farm folder."
            ),
            ran_at,
        };
    }

    match &extra.last_full_verify_ok_at {
        Some(when) => {
            let age = age_days(when, now_utc);
            if age.map(|d| d <= 30).unwrap_or(false) {
                CheckStatus {
                    check_id: "H4".into(),
                    severity: Severity::Healthy,
                    sentence: format!("H4 Core data intact — quick_check ok, verify passed {when}"),
                    ran_at,
                }
            } else {
                CheckStatus {
                    check_id: "H4".into(),
                    severity: Severity::Degraded,
                    sentence: "H4 Core data intact — no full verify in 30 days".into(),
                    ran_at,
                }
            }
        }
        None => {
            if extra.never_verified {
                CheckStatus {
                    check_id: "H4".into(),
                    severity: Severity::Degraded,
                    sentence: "H4 Core data intact — quick_check ok. This farm \
                         hasn't been verified yet. Run Verify now on the Health \
                         page once, so the app can confirm the record is intact."
                        .into(),
                    ran_at,
                }
            } else {
                CheckStatus {
                    check_id: "H4".into(),
                    severity: Severity::Degraded,
                    sentence: "H4 Core data intact — no full verify in 30 days".into(),
                    ran_at,
                }
            }
        }
    }
}

/// M1-M4 take no Evidence row on purpose. H1-H4 report on machinery that runs
/// on a schedule, so absent evidence is itself news. A money debt is a fact in
/// the register, readable now; its timestamp is the moment you looked.
fn money_ran_at(now_utc: &str) -> Option<String> {
    Some(now_utc.to_string())
}

fn deliveries_word(n: usize) -> String {
    if n == 1 {
        "1 delivery".to_string()
    } else {
        format!("{n} deliveries")
    }
}

fn days_word(n: i64) -> String {
    if n == 1 {
        "1 day".to_string()
    } else {
        format!("{n} days")
    }
}
fn leftover_word(n: i64) -> String {
    if n == 1 {
        "1 leftover listing".to_string()
    } else {
        format!("{n} leftover listings")
    }
}
/// OWED-LO: what M1 counts as owed. Wholesale deliveries first, leftover
/// after, joined with "and". Every word is already on the desk: "delivery"
/// from this file, "leftover" and "the listing" from Money's Leftover
/// section (leftover.rs LEFTOVER_ALREADY_LINKED_LINE). With no leftover the
/// output is byte-identical to deliveries_word, so every pre-OWED-LO
/// sentence is unchanged.
fn owed_items_word(deliveries: usize, leftover: i64) -> String {
    if leftover == 0 {
        deliveries_word(deliveries)
    } else if deliveries == 0 {
        leftover_word(leftover)
    } else {
        format!(
            "{} and {}",
            deliveries_word(deliveries),
            leftover_word(leftover)
        )
    }
}

fn more_promises(n: usize) -> String {
    match n {
        0 => String::new(),
        k => format!(" {k} more promises are short."),
    }
}

fn severity_m1(now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = money_ran_at(now_utc);
    // C1 (INT-001): the order book could not be read; "nothing owed" is not
    // a fact this render holds.
    if let Some(e) = &extra.read_failures.money {
        return read_failure_status("M1", "Owed to you", "the order book", e, "", ran_at);
    }
    let debts = &extra.money.collect;
    // OWED-LO (audit R-1): priced-unpaid leftover is owed money. Same
    // evaluator as the Money / Today owed line (leftover::owed_leftover via
    // wholesale::owed_summary), so the surfaces cannot disagree. An unpriced
    // listing is not owed and never becomes a dollar (GT-D24-B).
    let leftover = extra.leftover_owed;
    if debts.is_empty() && leftover.count == 0 {
        return CheckStatus {
            check_id: "M1".into(),
            severity: Severity::Healthy,
            sentence: "M1 Owed to you — nothing owed to you.".into(),
            ran_at,
        };
    }
    let oldest = debts
        .iter()
        .filter(|d| d.age_countable)
        .map(|d| d.days)
        .max();
    let any_unpriced = debts.iter().any(|d| d.any_unpriced);
    let items = owed_items_word(debts.len(), leftover.count);
    // Never a partial sum wearing a total's clothes — same rule the owed line
    // and the Money screen already apply. Leftover cents are priced by
    // definition; they join the total whenever the wholesale side has one.
    let what = if any_unpriced {
        format!("{items}, value partly unpriced")
    } else {
        let cents: i64 = debts.iter().map(|d| d.cents).sum::<i64>() + leftover.cents;
        format!(
            "${}.{:02} across {}",
            cents / 100,
            (cents % 100).abs(),
            items
        )
    };
    let age = match oldest {
        Some(0) => Some("delivered today".to_string()),
        Some(n) => Some(format!("oldest {}", days_word(n))),
        None => None,
    };
    let severity = if oldest.unwrap_or(0) > OWED_UNHEALTHY_DAYS {
        Severity::Unhealthy
    } else {
        Severity::Degraded
    };
    let action = if any_unpriced {
        "Price them, then collect on the Money tab."
    } else {
        "Collect the oldest on the Money tab."
    };
    let sentence = match age {
        Some(a) => format!("M1 Owed to you — {what}, {a}. {action}"),
        None => format!("M1 Owed to you — {what}. {action}"),
    };
    CheckStatus {
        check_id: "M1".into(),
        severity,
        sentence,
        ran_at,
    }
}

fn severity_m2(now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = money_ran_at(now_utc);
    // C1 (INT-001): the cover plan could not be read; "every live promise is
    // covered" is not a fact this render holds.
    if let Some(e) = &extra.read_failures.money {
        return read_failure_status("M2", "Promises covered", "the cover plan", e, "", ran_at);
    }
    let cover = &extra.money.cover;
    if cover.is_empty() {
        return CheckStatus {
            check_id: "M2".into(),
            severity: Severity::Healthy,
            sentence: "M2 Promises covered — every live promise is covered.".into(),
            ran_at,
        };
    }
    // cover_plan is ranked worst-first: unreachable, shelf-blocked,
    // must-sow-today, then slack. The head is the date that deserves the
    // sentence.
    //
    // M2 RENDERS THE PLAN'S SENTENCE. It does not compose one. Composing a
    // second sentence is what let Health say "sowing can still cover it"
    // about a date the Today card had already called out of room -- two
    // surfaces, one fact, opposite instructions. Do not reintroduce a
    // format! of the facts here.
    let worst = &cover[0];
    let rest = more_promises(cover.len() - 1);
    // HEALTH-JAR (TRIGGER C): a must-sow-today date whose jar is empty is red
    // too - the last day to sow, and nothing to sow. A slack date or an
    // unknown jar stays Degraded. jar_empty is the plan's one door; the
    // sentence below is still the plan's, tail and all.
    let severity = if !worst.reachability.reachable
        || crate::reachability::shelf_blocked(worst)
        || (crate::reachability::jar_empty(worst) && worst.reachability.must_sow_today)
    {
        Severity::Unhealthy
    } else {
        Severity::Degraded
    };
    CheckStatus {
        check_id: "M2".into(),
        severity,
        sentence: format!("M2 Promises covered — {}{rest}", worst.message),
        ran_at,
    }
}

fn severity_m3(now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = money_ran_at(now_utc);
    // C1 (INT-001): the order book could not be read; "nothing due" is not a
    // fact this render holds.
    if let Some(e) = &extra.read_failures.money {
        return read_failure_status("M3", "Deliveries due", "the order book", e, "", ran_at);
    }
    let due = &extra.money.deliver;
    if due.is_empty() {
        return CheckStatus {
            check_id: "M3".into(),
            severity: Severity::Healthy,
            sentence: "M3 Deliveries due — nothing due.".into(),
            ran_at,
        };
    }
    let rest = match due.len() - 1 {
        0 => String::new(),
        1 => " 1 more is due.".to_string(),
        k => format!(" {k} more are due."),
    };
    if let Some(late) = due.iter().find(|d| d.days_late > 0) {
        let when = crate::reachability::format_mon_d_local(&late.harvest_date)
            .unwrap_or_else(|_| late.harvest_date.clone());
        return CheckStatus {
            check_id: "M3".into(),
            severity: Severity::Unhealthy,
            sentence: format!(
                "M3 Deliveries due — {} to {} were due {when}, {} ago. Deliver or void it.{rest}",
                crate::reachability::tray_word(late.trays),
                late.venue_name,
                days_word(late.days_late)
            ),
            ran_at,
        };
    }
    let d = &due[0];
    CheckStatus {
        check_id: "M3".into(),
        severity: Severity::Degraded,
        sentence: format!(
            "M3 Deliveries due — {} to {}, due {}.{rest}",
            crate::reachability::tray_word(d.trays),
            d.venue_name,
            crate::reachability::format_mon_d_local(&d.harvest_date)
                .unwrap_or_else(|_| d.harvest_date.clone())
        ),
        ran_at,
    }
}

fn severity_m4(now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = money_ran_at(now_utc);
    // C1 (INT-001): the correction trail could not be read; "every money
    // correction has a visible trail" is not a fact this render holds.
    if let Some(e) = &extra.read_failures.trail {
        return read_failure_status(
            "M4",
            "Ledger symmetry",
            "the correction trail",
            e,
            "",
            ran_at,
        );
    }
    let quarter = if extra.quarter_label.is_empty() {
        "this quarter".to_string()
    } else {
        extra.quarter_label.clone()
    };
    // The ledger cannot be called symmetric while the record itself is disputed.
    if extra.last_full_verify_failed || extra.unknown_divergences || extra.last_verify_incomplete {
        return CheckStatus {
            check_id: "M4".into(),
            severity: Severity::Unhealthy,
            sentence: "M4 Ledger symmetry — the last verify did not pass; the ledger \
                 cannot be called symmetric until it does."
                .into(),
            ran_at,
        };
    }
    let n = extra.untrailed_income_corrections;
    if n <= 0 {
        return CheckStatus {
            check_id: "M4".into(),
            severity: Severity::Healthy,
            sentence: format!(
                "M4 Ledger symmetry — every money correction in {quarter} has a visible trail."
            ),
            ran_at,
        };
    }
    let records = if n == 1 {
        "1 income record was"
    } else {
        "income records were"
    };
    let count = if n == 1 {
        String::new()
    } else {
        format!("{n} ")
    };
    CheckStatus {
        check_id: "M4".into(),
        severity: Severity::Degraded,
        sentence: format!(
            "M4 Ledger symmetry — {count}{records} voided or corrected in {quarter}; \
             detail lives only in the log."
        ),
        ran_at,
    }
}

/// F1 — trays past their harvest date, still on the shelf.
///
/// Reads `trays::overdue_trays_on_shelf` (trays.rs:2028-2039) — the same
/// function `reachability::cover_message` already uses to name overdue trays on
/// a COVER card (reachability.rs:222, 329-333). No new tray math.
///
/// Why it exists: Today raises `tray.overdue_harvest` (attention.rs:285-327)
/// and no reported check read trays at all. M2 reads `cover_plan`, which skips
/// any date that is not negative, so trays past harvest with no order attached
/// moved nothing. Health rendered calm while Today showed a shelf problem.
///
/// This reader is BROADER than the Today card: the card waits 3 days and only
/// looks at trays under light, this counts every live tray past its date. Louder
/// than Today is honest; quieter would not be.
fn severity_f1(now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = money_ran_at(now_utc);
    // C1 (INT-001): the shelf could not be read; "nothing is past its harvest
    // date" is not a fact this render holds.
    if let Some(e) = &extra.read_failures.shelf {
        return read_failure_status("F1", "Trays on the shelf", "the shelf", e, "", ran_at);
    }
    if extra.overdue_trays <= 0 {
        return CheckStatus {
            check_id: "F1".into(),
            severity: Severity::Healthy,
            sentence: "F1 Trays on the shelf — nothing is past its harvest date.".into(),
            ran_at,
        };
    }
    let trays = crate::reachability::tray_word(extra.overdue_trays);
    let verb = if extra.overdue_trays == 1 {
        "is"
    } else {
        "are"
    };
    let poss = if extra.overdue_trays == 1 {
        "its"
    } else {
        "their"
    };
    CheckStatus {
        check_id: "F1".into(),
        severity: Severity::Degraded,
        sentence: format!(
            "F1 Trays on the shelf — {trays} {verb} past {poss} harvest date and \
             still on the shelf. Harvest them on Today, or count the shelf on \
             Farm if they are already gone."
        ),
        ran_at,
    }
}

/// F2 — trays still under cover past their cover-check date.
///
/// RB1. Today has raised `tray.overdue_light` (attention.rs) since Phase 2 and
/// no reported check read that condition. F1 reads `sown_on + growth_days`, and
/// blackout days are always fewer than growth days, so a tray could be loudly
/// overdue for light on Today while Health's Farm scope read "None raising."
/// That is the state locked principle 1 forbids.
///
/// One query (`trays::overdue_light_groups`): the card reads it at
/// `COVER_CARD_DAYS_PAST` and Health at `COVER_HEALTH_DAYS_PAST`, so Health
/// raises two days earlier than the card and can never be quieter than Today
/// about a tray past its cover check. Never the attention table.
///
/// Degraded, never Unhealthy: the condition carries no severity and no age band
/// anywhere in the tree, and F1 — its sibling on the harvest half — is
/// Degraded-only. An Unhealthy threshold here would be an invented number.
///
/// F1 and F2 can both raise for one tray that is past both dates. Two checks,
/// two different facts, two different verbs. That is not double-counting.
fn severity_f2(now_utc: &str, extra: &CheckInputs) -> CheckStatus {
    let ran_at = money_ran_at(now_utc);
    // C1 (INT-001): the bench could not be read; "nothing is past its
    // cover-check date" is not a fact this render holds.
    if let Some(e) = &extra.read_failures.cover {
        return read_failure_status("F2", "Trays under cover", "the bench", e, "", ran_at);
    }
    if extra.overdue_light_trays <= 0 {
        return CheckStatus {
            check_id: "F2".into(),
            severity: Severity::Healthy,
            sentence: "F2 Trays under cover — nothing is past its cover-check date.".into(),
            ran_at,
        };
    }
    let trays = crate::reachability::tray_word(extra.overdue_light_trays);
    let one = extra.overdue_light_trays == 1;
    let verb = if one { "has" } else { "have" };
    let poss = if one { "its" } else { "their" };
    let them = if one { "it" } else { "them" };
    CheckStatus {
        check_id: "F2".into(),
        severity: Severity::Degraded,
        sentence: format!(
            "F2 Trays under cover — {trays} {verb} been under cover past {poss} \
             cover-check date. Move {them} to light on Today."
        ),
        ran_at,
    }
}

/// Compute flush lag the same way verify/export do: live_max_seq − watermark.
/// Building block: `event_file::read_watermark`.
pub fn flush_lag(conn: &Connection, farm_dir: &Path) -> Result<i64, String> {
    let events_path = event_file::events_path(farm_dir);
    let watermark = event_file::read_watermark(&events_path)?;
    let live_max_seq: i64 = conn
        .query_row("SELECT IFNULL(MAX(seq), 0) FROM event_log", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    Ok((live_max_seq - watermark).max(0))
}

pub fn log_fork(conn: &Connection, farm_dir: &Path) -> Result<(bool, i64, i64), String> {
    let events_path = event_file::events_path(farm_dir);
    let watermark = event_file::read_watermark(&events_path)?;
    let live_max_seq: i64 = conn
        .query_row("SELECT IFNULL(MAX(seq), 0) FROM event_log", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    Ok((watermark > live_max_seq, watermark, live_max_seq))
}

pub fn run_quick_check(conn: &Connection) -> Result<(bool, String), String> {
    let result: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let ok = result == "ok";
    Ok((ok, result))
}

pub fn snapshot_taken_today(snapshots_dir: &Path, today_local: NaiveDate) -> Result<bool, String> {
    let snaps = snapshots::list_snapshots(snapshots_dir)?;
    Ok(snaps.iter().any(|s| {
        DateTime::parse_from_rfc3339(&s.taken_at)
            .ok()
            .map(|dt| dt.with_timezone(&Local).date_naive() == today_local)
            .unwrap_or(false)
    }))
}

/// Retention runs inside every successful `take_snapshot`. A snapshot from
/// today is evidence retention ran as part of that take.
pub fn retention_ran_today(snapshots_dir: &Path, today_local: NaiveDate) -> Result<bool, String> {
    snapshot_taken_today(snapshots_dir, today_local)
}

pub fn compute_status(
    conn: &Connection,
    farm_dir: &Path,
    snapshots_dir: &Path,
    now_utc: &str,
    today_local: NaiveDate,
) -> Result<Vec<CheckStatus>, String> {
    let h2_ev = latest_evidence(conn, "H2")?;
    let h3_ev = latest_evidence(conn, "H3")?;
    let h4_ev = latest_evidence(conn, "H4")?;

    // C1 (INT-001). Every reader below lands either its value or its error.
    // The value written beside a failure is the type's default and is never
    // rendered: each check's read-failure arm returns before any arm that
    // would read it. No reader may be defaulted back into a calm number.
    let mut failures = ReadFailures::default();
    let lag = read_or_fail(flush_lag(conn, farm_dir), &mut failures.log);
    let (log_ahead, log_watermark, db_max_seq) =
        read_or_fail(log_fork(conn, farm_dir), &mut failures.log);
    let snap_today = snapshot_taken_today(snapshots_dir, today_local)?;
    let retention = retention_ran_today(snapshots_dir, today_local)?;

    let h4 = h4_inputs_from_evidence(h4_ev.as_ref(), farm_dir);

    let (mkt_lag, active_venues, last_mkt_write) =
        read_or_fail(h2_facts(conn, farm_dir), &mut failures.marketing);

    // B4 — the same evaluator Today's money cards come from. Read from live
    // tables, never from the attention table, so a dismissal can quiet Today
    // without ever quieting Health.
    let money = read_or_fail(attention::money_debts(conn), &mut failures.money);
    // OWED-LO (audit R-1): the one leftover-owed evaluator — the same
    // function wholesale::owed_summary reads for the Money / Today owed
    // line. Shares the money read-failure slot: an unreadable leftover
    // table must poison M1 exactly as an unreadable order book does.
    let leftover_owed = read_or_fail(crate::leftover::owed_leftover(conn), &mut failures.money);
    let (quarter_start, quarter_label) = quarter_bounds(today_local);
    let untrailed = read_or_fail(
        untrailed_income_corrections(conn, &quarter_start),
        &mut failures.trail,
    );

    // F1 — the same tray reader the COVER sentence already uses.
    let today_str = today_local.format("%Y-%m-%d").to_string();
    let overdue_trays = read_or_fail(
        crate::trays::overdue_trays_on_shelf(conn, &today_str),
        &mut failures.shelf,
    );
    // F2 — the same query as Today's card, read at COVER_HEALTH_DAYS_PAST.
    let overdue_light_trays = read_or_fail(
        crate::trays::overdue_light_trays(conn, &today_str),
        &mut failures.cover,
    );

    let inputs = CheckInputs {
        flush_lag: lag,
        snapshot_today: snap_today,
        retention_ran: retention,
        quick_check_ok: h4.quick_check_ok,
        last_full_verify_ok_at: h4.last_full_verify_ok_at,
        last_full_verify_failed: h4.last_full_verify_failed,
        unknown_divergences: h4.unknown_divergences,
        never_verified: h4.never_verified,
        last_verify_incomplete: h4.last_verify_incomplete,
        last_verify_outcome_line: h4.last_verify_outcome_line,
        marketing_flush_lag: mkt_lag,
        active_venues,
        last_marketing_write_at: last_mkt_write,
        log_ahead,
        log_watermark,
        db_max_seq,
        money,
        leftover_owed,
        untrailed_income_corrections: untrailed,
        quarter_label,
        overdue_trays,
        overdue_light_trays,
        read_failures: failures,
        // consecutive_failures / varieties_parsed / structural_miss were H1's
        // inputs only. H1 is not reported (ruling 6.1).
        ..CheckInputs::default()
    };

    Ok(REPORTED_CHECKS
        .iter()
        .map(|id| {
            let evidence = match *id {
                "H2" => h2_ev.as_ref(),
                "H3" => h3_ev.as_ref(),
                "H4" => h4_ev.as_ref(),
                // M1-M4 and F1 take no Evidence row: a debt or a tray is a fact
                // in the register, readable now.
                _ => None,
            };
            severity_for(id, evidence, now_utc, &inputs)
        })
        .collect())
}

/// C1 (INT-001). A reader's `Err` goes into its slot; the first error in a
/// slot wins (two readers share the log slot and fail on the same file). The
/// returned default is a placeholder the read-failure arms make unreachable.
fn read_or_fail<T: Default>(read: Result<T, String>, slot: &mut Option<String>) -> T {
    match read {
        Ok(value) => value,
        Err(e) => {
            if slot.is_none() {
                *slot = Some(e);
            }
            T::default()
        }
    }
}

/// Gather H2 facts for CheckInputs. Marketing flush lag uses the shared
/// watermark vs max marketing seq so a stuck marketing row surfaces
/// independently of grow. Silence days are NOT computed here — that is
/// the H2 severity arm's job at render time via age_days.
fn h2_facts(conn: &Connection, farm_dir: &Path) -> Result<(i64, i64, Option<String>), String> {
    let events_path = event_file::events_path(farm_dir);
    let watermark = event_file::read_watermark(&events_path)?;
    let mkt_max: i64 = conn
        .query_row(
            "SELECT IFNULL(MAX(seq), 0) FROM event_log WHERE event_domain = 'marketing'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    // Pending marketing rows are those with seq > watermark.
    let mkt_lag = if mkt_max > watermark {
        mkt_max - watermark
    } else {
        0
    };

    let active_venues: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mkt_venues WHERE archived_at IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    let last_write: Option<String> = conn
        .query_row(
            "SELECT MAX(created_at) FROM event_log
             WHERE event_domain = 'marketing'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();

    Ok((mkt_lag, active_venues, last_write))
}

/// Quarter start as a YYYY-MM-DD string, plus its label. Boundaries are machine
/// local (chrono::Local everywhere in this tree); event_log.created_at is UTC
/// RFC3339, which sorts lexicographically against a date prefix. A correction
/// written within hours of a quarter boundary can therefore land on either side.
/// Named, not hidden: it is the same machine-local edge the audit already
/// records under unknown unknowns, and it is not worth timezone machinery here.
fn quarter_bounds(today_local: NaiveDate) -> (String, String) {
    let q = (today_local.month0() / 3) + 1;
    let start_month = (q - 1) * 3 + 1;
    let start = NaiveDate::from_ymd_opt(today_local.year(), start_month, 1).unwrap_or(today_local);
    (
        start.format("%Y-%m-%d").to_string(),
        format!("Q{q} {}", today_local.year()),
    )
}

/// B4/M4 — income corrections whose detail never reached a visible row.
///
/// `money_corrections` accepts track='income' by schema, but only the cost path
/// writes there today, so this counts every income void/correction. The query is
/// written as a NOT EXISTS on purpose: the day B8 starts writing income rows to
/// the trail, this check goes quiet by itself with no edit to health.rs.
fn untrailed_income_corrections(conn: &Connection, quarter_start: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM event_log e
          WHERE e.kind IN ('income.voided', 'income.corrected')
            AND e.undone_at IS NULL
            AND e.created_at >= ?1
            AND NOT EXISTS (SELECT 1 FROM money_corrections m
                             WHERE m.correction_event_id = e.id)",
        params![quarter_start],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// Record H2 evidence from current inputs (status itself is never stored).
pub fn record_h2(
    conn: &Connection,
    farm_dir: &Path,
    now_utc: &str,
    _today_local: NaiveDate,
) -> Result<CheckStatus, String> {
    let (mkt_lag, active_venues, last_mkt_write) = h2_facts(conn, farm_dir)?;
    let inputs = CheckInputs {
        marketing_flush_lag: mkt_lag,
        active_venues,
        last_marketing_write_at: last_mkt_write,
        ..CheckInputs::default()
    };
    // Fresh evidence at now so record reflects flush/silence/venues facts,
    // not the absent-evidence arm.
    let ev = Evidence {
        check_id: "H2".into(),
        ran_at: now_utc.to_string(),
        ok: true,
        detail: String::new(),
    };
    let status = severity_for("H2", Some(&ev), now_utc, &inputs);
    let ok = status.severity == Severity::Healthy;
    record(conn, "H2", now_utc, ok, &status.sentence)?;
    Ok(status)
}

/// RETIRED by ruling 6.1 (2026-08-16): H1 is no longer reported, so nothing
/// gathers its inputs. Kept beside `severity_h1` and its truth table rather
/// than deleted, so the retirement is visible and reversible if the Storefront
/// ever returns as an operator-configured surface.
#[allow(dead_code)]
fn h1_inputs_from_evidence(evidence: Option<&Evidence>) -> (Option<usize>, Option<String>) {
    match evidence {
        Some(ev) if ev.ok => (Some(5), None),
        Some(ev) if !ev.ok => (None, Some(ev.detail.clone())),
        _ => (None, None),
    }
}

pub(crate) struct H4Verify {
    pub quick_check_ok: bool,
    pub last_full_verify_ok_at: Option<String>,
    pub last_full_verify_failed: bool,
    pub unknown_divergences: bool,
    pub never_verified: bool,
    /// The last verify could not run. Nothing was compared.
    pub last_verify_incomplete: bool,
    /// The exact VERIFY-REPLAY line behind the verdict, for the sentence.
    pub last_verify_outcome_line: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifyVerdict {
    Incomplete,
    Failed,
    Passed,
}

fn file_verify_verdict(text: &str) -> Option<VerifyVerdict> {
    if text.contains(projection::VERIFY_INCOMPLETE_PREFIX) {
        Some(VerifyVerdict::Incomplete)
    } else if text.contains("VERIFY-REPLAY: FAIL") {
        Some(VerifyVerdict::Failed)
    } else if text.contains("VERIFY-REPLAY: PASS") {
        Some(VerifyVerdict::Passed)
    } else {
        None
    }
}

fn evidence_verify_verdict(detail: &str) -> Option<VerifyVerdict> {
    if detail
        .to_lowercase()
        .contains(&projection::VERIFY_INCOMPLETE_PREFIX.to_lowercase())
    {
        Some(VerifyVerdict::Incomplete)
    } else if detail.contains("verify failed") || detail.contains("VERIFY-REPLAY: FAIL") {
        Some(VerifyVerdict::Failed)
    } else if detail.contains("verify passed") {
        Some(VerifyVerdict::Passed)
    } else {
        None
    }
}

fn first_verify_replay_line(text: &str) -> Option<String> {
    text.lines()
        .find(|l| l.trim_start().starts_with("VERIFY-REPLAY:"))
        .map(|l| l.trim().to_string())
}

fn verify_replay_line_in_detail(detail: &str) -> Option<String> {
    detail
        .lines()
        .find_map(|l| l.find("VERIFY-REPLAY:").map(|i| l[i..].trim().to_string()))
}

fn unknown_divergences_in(text: &str) -> bool {
    text.contains("unexplained_divergences") || text.contains("unexplained divergences")
}

pub(crate) fn h4_inputs_from_evidence(evidence: Option<&Evidence>, farm_dir: &Path) -> H4Verify {
    let quick_check_ok = match evidence {
        Some(e) => {
            let d = e.detail.to_lowercase();
            if d.contains("quick_check fail") {
                false
            } else if d.contains("quick_check ok") {
                true
            } else {
                e.ok
            }
        }
        None => false,
    };

    let verify_path = farm_dir.join("last-verify-replay.txt");
    let file_exists = verify_path.exists();
    let file_text = std::fs::read_to_string(&verify_path).ok();
    let (file_verdict, file_when, file_line, file_unknown) = match &file_text {
        Some(text) => (
            file_verify_verdict(text),
            text.lines()
                .find_map(|l| l.strip_prefix("when=").map(|s| s.trim().to_string())),
            first_verify_replay_line(text),
            unknown_divergences_in(text),
        ),
        None => (None, None, None, false),
    };

    let (ev_verdict, ev_when, ev_line, ev_unknown) = match evidence {
        Some(ev) => (
            evidence_verify_verdict(&ev.detail),
            Some(ev.ran_at.clone()),
            verify_replay_line_in_detail(&ev.detail),
            unknown_divergences_in(&ev.detail),
        ),
        None => (None, None, None, false),
    };

    let evidence_newer = match (&ev_when, &file_when) {
        (Some(ev), Some(file)) => match (parse_rfc3339(ev), parse_rfc3339(file)) {
            (Some(e), Some(f)) => e > f,
            _ => false,
        },
        _ => false,
    };

    // The newest fact wins. Evidence with no verify verdict (the app-start
    // "quick_check ok" row) must never override the file.
    let use_evidence = ev_verdict.is_some() && (file_verdict.is_none() || evidence_newer);
    let (verdict, when, line, unknown) = if use_evidence {
        (ev_verdict, ev_when, ev_line, ev_unknown)
    } else {
        (file_verdict, file_when, file_line, file_unknown)
    };

    H4Verify {
        quick_check_ok,
        last_full_verify_ok_at: if matches!(verdict, Some(VerifyVerdict::Passed)) {
            when
        } else {
            None
        },
        last_full_verify_failed: matches!(verdict, Some(VerifyVerdict::Failed)),
        unknown_divergences: unknown,
        never_verified: !file_exists && ev_verdict.is_none(),
        last_verify_incomplete: matches!(verdict, Some(VerifyVerdict::Incomplete)),
        last_verify_outcome_line: line,
    }
}

/// Record H4 quick_check evidence. Errors are returned as detail, never panic.
pub fn record_quick_check(conn: &Connection, now_utc: &str) -> Result<CheckStatus, String> {
    let (ok, detail) = match run_quick_check(conn) {
        Ok((ok, result)) => {
            if ok {
                (true, "quick_check ok".to_string())
            } else {
                (false, format!("quick_check FAIL: {result}"))
            }
        }
        Err(e) => (false, format!("quick_check FAIL: {e}")),
    };
    record(conn, "H4", now_utc, ok, &detail)?;
    let ev = Evidence {
        check_id: "H4".into(),
        ran_at: now_utc.to_string(),
        ok,
        detail: detail.clone(),
    };
    Ok(severity_for(
        "H4",
        Some(&ev),
        now_utc,
        &CheckInputs {
            quick_check_ok: ok,
            ..CheckInputs::default()
        },
    ))
}

/// "Verify now" — runs verify_replay_paths and records H4 evidence. The
/// farm folder is the scratch root (FIX A): scratch is `snapshots/verify-<uuid>/`.
pub fn run_full_verify(
    conn: &Connection,
    farm_db: &Path,
    farm_dir: &Path,
    now_utc: &str,
) -> Result<CheckStatus, String> {
    let events = event_file::events_path(farm_dir);
    let (ok, detail, unknown, incomplete) =
        match projection::verify_replay_paths(farm_db, &events, farm_dir) {
            Ok(outcome) => {
                let _ = projection::write_verify_status(farm_dir, &outcome);
                let unknown = !outcome.report().unknown_diffs.is_empty();
                let failed = outcome.exit_nonzero();
                (!failed && !unknown, outcome.summary_line(), unknown, false)
            }
            Err(e) => {
                let line = format!("{} — {e}", projection::VERIFY_INCOMPLETE_PREFIX);
                let _ = projection::write_verify_incomplete(farm_dir, &e);
                (false, line, false, true)
            }
        };

    let quick_ok = run_quick_check(conn).map(|(ok, _)| ok).unwrap_or(false);
    let combined_ok = ok && quick_ok;
    let full_detail = if combined_ok {
        format!("quick_check ok; verify passed at {now_utc}; {detail}")
    } else if !quick_ok {
        format!("quick_check FAIL; {detail}")
    } else if incomplete {
        format!("quick_check ok; {detail}")
    } else {
        format!("quick_check ok; verify failed: {detail}")
    };
    record(conn, "H4", now_utc, combined_ok, &full_detail)?;

    let inputs = CheckInputs {
        quick_check_ok: quick_ok,
        last_full_verify_ok_at: if ok { Some(now_utc.to_string()) } else { None },
        last_full_verify_failed: !ok,
        unknown_divergences: unknown,
        last_verify_incomplete: incomplete,
        last_verify_outcome_line: Some(detail.clone()),
        ..CheckInputs::default()
    };
    let ev = Evidence {
        check_id: "H4".into(),
        ran_at: now_utc.to_string(),
        ok: combined_ok,
        detail: full_detail.clone(),
    };
    let status = severity_for("H4", Some(&ev), now_utc, &inputs);
    Ok(CheckStatus {
        check_id: status.check_id,
        severity: status.severity,
        sentence: full_detail,
        ran_at: status.ran_at,
    })
}

/// Record H1 from a storefront reading (success or structural/network failure).
pub fn record_h1_from_reading(
    conn: &Connection,
    now_utc: &str,
    reading: &storefront::StorefrontReading,
) -> Result<(), String> {
    let (ok, detail) = if reading.ok {
        (
            true,
            format!("{} of 5 varieties parsed", reading.varieties.len()),
        )
    } else {
        (
            false,
            reading
                .error
                .clone()
                .unwrap_or_else(|| "storefront fetch failed".into()),
        )
    };
    record(conn, "H1", now_utc, ok, &detail)
}

/// Record H3 from current snapshot/flush state.
pub fn record_h3(
    conn: &Connection,
    farm_dir: &Path,
    snapshots_dir: &Path,
    now_utc: &str,
    today_local: NaiveDate,
) -> Result<(), String> {
    let lag = flush_lag(conn, farm_dir)?;
    let (log_ahead, _, _) = log_fork(conn, farm_dir)?;
    let snap = snapshot_taken_today(snapshots_dir, today_local)?;
    let retention = retention_ran_today(snapshots_dir, today_local)?;
    let ok = lag == 0 && !log_ahead && snap && retention;
    let detail = format!(
        "flush_lag={lag}; snapshot_today={snap}; retention_ran={retention}; log_ahead={log_ahead}"
    );
    record(conn, "H3", now_utc, ok, &detail)
}

fn ran_at_in_future(ran_at: &str, now_utc: &str) -> bool {
    match (parse_rfc3339(ran_at), parse_rfc3339(now_utc)) {
        (Some(ran), Some(now)) => ran > now,
        _ => false,
    }
}

fn age_days(then: &str, now_utc: &str) -> Option<i64> {
    let then = parse_rfc3339(then)?;
    let now = parse_rfc3339(now_utc)?;
    Some((now - then).num_days())
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[allow(dead_code)]
fn within_days(then: &str, now_utc: &str, days: i64) -> bool {
    match (parse_rfc3339(then), parse_rfc3339(now_utc)) {
        (Some(t), Some(n)) => n - t <= Duration::days(days),
        _ => false,
    }
}

/// B4's hard invariant, written down so a future edit fails a test instead of
/// shipping a green lie: Health may never read calm while Today would show a
/// money card. It holds by construction — M1/M2/M3 are Healthy exactly when
/// their debt list is empty, and B1 raises a card for every entry in those same
/// lists — but "holds by construction" is a claim, and claims get asserted.
#[allow(dead_code)] // H-2: signed invariant, exercised by health_tests.
pub fn money_invariant_holds(debts: &MoneyDebts, statuses: &[CheckStatus]) -> bool {
    if !debts.has_any() {
        return true;
    }
    statuses.iter().any(|s| {
        matches!(s.check_id.as_str(), "M1" | "M2" | "M3") && s.severity != Severity::Healthy
    })
}

/// Sibling of `money_invariant_holds` for the shelf. While trays sit past their
/// harvest date, F1 may not read Healthy. Locked principle 1: Health never
/// reads calm while Today would show a money or shelf problem.
#[allow(dead_code)] // H-2: signed invariant, exercised by health_tests.
pub fn shelf_invariant_holds(overdue_trays: i64, statuses: &[CheckStatus]) -> bool {
    if overdue_trays <= 0 {
        return true;
    }
    statuses
        .iter()
        .any(|s| s.check_id == "F1" && s.severity != Severity::Healthy)
}

/// C1 (INT-001) invariant, sibling of the three above: a reader that failed
/// may never leave the check it feeds reading Healthy. Every check named by a
/// `Some` slot must be present and non-Healthy — absence fails, exactly as
/// `money_invariant_holds` fails when no M-check raised.
#[allow(dead_code)] // H-2: signed invariant, exercised by health_tests.
pub fn read_failure_invariant_holds(failures: &ReadFailures, statuses: &[CheckStatus]) -> bool {
    let poisoned_never_healthy = |ids: &[&str]| {
        ids.iter().all(|id| {
            statuses
                .iter()
                .any(|s| s.check_id == *id && s.severity != Severity::Healthy)
        })
    };
    (failures.log.is_none() || poisoned_never_healthy(&["H3"]))
        && (failures.marketing.is_none() || poisoned_never_healthy(&["H2"]))
        && (failures.money.is_none() || poisoned_never_healthy(&["M1", "M2", "M3"]))
        && (failures.trail.is_none() || poisoned_never_healthy(&["M4"]))
        && (failures.shelf.is_none() || poisoned_never_healthy(&["F1"]))
        && (failures.cover.is_none() || poisoned_never_healthy(&["F2"]))
}

/// RB1's invariant, sibling of `shelf_invariant_holds` for the cover half.
/// While trays sit under cover past their cover-check date, F2 may not read
/// Healthy. Locked principle 1, closed for light as well as for harvest.
#[allow(dead_code)] // H-2: signed invariant, exercised by health_tests.
pub fn cover_invariant_holds(overdue_light_trays: i64, statuses: &[CheckStatus]) -> bool {
    if overdue_light_trays <= 0 {
        return true;
    }
    statuses
        .iter()
        .any(|s| s.check_id == "F2" && s.severity != Severity::Healthy)
}
