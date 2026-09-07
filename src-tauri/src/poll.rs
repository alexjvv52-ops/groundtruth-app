//! Stripe poll loop — feeds Prompt 1's apply_* helpers.
//!
//! Sessions: apply oldest-first, advance the cursor one record at a time after
//! each record is fully handled. A cursor moved early is a permanently lost order.
//!
//! ABSOLUTE STOP: never record an order, payment, or stock movement that also
//! exists in the commercial app. Farm OS owns upstream of harvest; the
//! commercial app owns everything downstream. One crossing, one direction,
//! once per harvest.

use crate::attention;
use crate::db;
use crate::models::{AppliedOutcome, OrderView, ReconciliationDate, ReconciliationOrder};
use crate::money::{
    self, apply_dispute, apply_refund, FactOutcome, PaidSession, StripeGateway, UnparsedSession,
};
use crate::trays;
use chrono::{Local, Timelike};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollResult {
    pub ok: bool,
    pub sessions_applied: i64,
    pub sessions_rejected: i64,
    pub refunds_applied: i64,
    pub disputes_applied: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPaidOrders {
    pub count: i64,
}

struct Cursors {
    sessions_since: Option<String>,
    refunds_since: Option<String>,
    disputes_since: Option<String>,
}

pub fn run_poll_from_db(conn: &mut Connection) -> Result<PollResult, String> {
    // Not configured yet — quiet success, nothing to do.
    let configured = money::money_status(conn)?.configured;
    if !configured {
        return Ok(PollResult {
            ok: true,
            sessions_applied: 0,
            sessions_rejected: 0,
            refunds_applied: 0,
            disputes_applied: 0,
            error: None,
        });
    }
    let gw = money::gateway_from_db(conn)?;
    run_poll(conn, &gw)
}

pub fn run_poll<G: StripeGateway>(conn: &mut Connection, gw: &G) -> Result<PollResult, String> {
    match run_poll_inner(conn, gw) {
        Ok(r) => {
            mark_poll_success(conn)?;
            Ok(r)
        }
        Err(e) => {
            // Never panic / never take the window down — record and return.
            let _ = mark_poll_failure(conn, &e);
            Ok(PollResult {
                ok: false,
                sessions_applied: 0,
                sessions_rejected: 0,
                refunds_applied: 0,
                disputes_applied: 0,
                error: Some(e),
            })
        }
    }
}

enum SessionWork {
    Parsed(PaidSession),
    Unparsed(UnparsedSession),
}

impl SessionWork {
    fn created(&self) -> i64 {
        match self {
            SessionWork::Parsed(s) => s.created,
            SessionWork::Unparsed(u) => u.created,
        }
    }

    fn session_id(&self) -> &str {
        match self {
            SessionWork::Parsed(s) => &s.session_id,
            SessionWork::Unparsed(u) => &u.session_id,
        }
    }
}

fn run_poll_inner<G: StripeGateway>(conn: &mut Connection, gw: &G) -> Result<PollResult, String> {
    let cursors = read_cursors(conn)?;
    let mut sessions_applied = 0i64;
    let mut sessions_rejected = 0i64;
    let mut refunds_applied = 0i64;
    let mut disputes_applied = 0i64;

    // 1. Paid sessions — collect all pages, oldest-first, advance per record.
    let session_pages = gw.list_paid_session_pages(cursors.sessions_since.as_deref())?;
    let mut work: Vec<SessionWork> = Vec::new();
    for page in session_pages {
        for s in page.parsed {
            work.push(SessionWork::Parsed(s));
        }
        for u in page.unparsed {
            work.push(SessionWork::Unparsed(u));
        }
    }
    work.sort_by(|a, b| {
        a.created()
            .cmp(&b.created())
            .then_with(|| a.session_id().cmp(b.session_id()))
    });

    for record in work {
        match record {
            SessionWork::Parsed(session) => {
                match money::apply_paid_session_with(conn, &session, gw)? {
                    AppliedOutcome::Applied { .. } => sessions_applied += 1,
                    AppliedOutcome::AlreadyApplied => {}
                    AppliedOutcome::Rejected { .. } => sessions_rejected += 1,
                }
                set_cursor(conn, "sessions_since", &session.created.to_string())?;
            }
            SessionWork::Unparsed(u) => {
                raise_unrecognised_session(conn, &u)?;
                money::record_unapplied_fact(
                    conn,
                    "checkout_session",
                    &u.session_id,
                    "unmatched",
                    Some(u.amount_cents),
                    Some(&u.currency),
                    u.created,
                )?;
                set_cursor(conn, "sessions_since", &u.created.to_string())?;
            }
        }
    }

    // TODO(stage-4): disputes still use created[gt] and per-page cursor
    // advancement. Same stranding risk as sessions had. Not in scope for the P0 pass.

    // 2. Refunds — C2 (INT-002). Oldest-first like sessions (Stripe lists
    // newest-first, and the old per-page walk left the cursor on the OLDEST
    // refund of a page, re-listing the newer ones on the next poll). The
    // cursor advances to the newest handled refund, and is HELD one second
    // below the oldest refund Stripe still calls non-terminal, so that refund
    // is re-read every poll until it settles (`created[gt]` is exclusive).
    // Every re-read is idempotent: apply_refund answers an applied refund by
    // its id in the log and a traced one by its (refund, reason) row. An
    // unmatched (out-of-order) refund still stops the walk.
    let refunds_cursor = cursors.refunds_since.clone();
    let mut refunds: Vec<money::RefundRecord> = gw
        .list_refund_pages(refunds_cursor.as_deref())?
        .into_iter()
        .flatten()
        .collect();
    refunds.sort_by(|a, b| {
        a.created
            .cmp(&b.created)
            .then_with(|| a.refund_id.cmp(&b.refund_id))
    });
    let mut newest_handled: Option<i64> = None;
    let mut hold_at: Option<i64> = None;
    for refund in &refunds {
        match apply_refund(conn, refund)? {
            FactOutcome::Applied => {
                refunds_applied += 1;
                newest_handled = Some(refund.created);
            }
            FactOutcome::AlreadyApplied => {
                newest_handled = Some(refund.created);
            }
            FactOutcome::Deferred => {
                hold_at = Some(hold_at.map_or(refund.created, |h| h.min(refund.created)));
                newest_handled = Some(refund.created);
            }
            FactOutcome::AwaitingOrder => break,
        }
    }
    if let Some(top) = newest_handled {
        let ts = match hold_at {
            Some(h) => top.min(h - 1),
            None => top,
        };
        let prior = refunds_cursor
            .as_ref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(-1);
        if ts > prior {
            set_cursor(conn, "refunds_since", &ts.to_string())?;
        }
    }

    // 3. Disputes — same out-of-order rule.
    let mut disputes_cursor = cursors.disputes_since.clone();
    let dispute_pages = gw.list_dispute_pages(disputes_cursor.as_deref())?;
    for page in dispute_pages {
        if page.is_empty() {
            continue;
        }
        let mut advanced_through: Option<i64> = None;
        let mut blocked = false;
        for dispute in &page {
            match apply_dispute(conn, dispute)? {
                FactOutcome::Applied => {
                    disputes_applied += 1;
                    advanced_through = Some(dispute.created);
                }
                // Deferred is a refund outcome; apply_dispute never returns it
                // (C2 left the dispute path untouched). Named, not wildcarded.
                FactOutcome::AlreadyApplied | FactOutcome::Deferred => {
                    advanced_through = Some(dispute.created);
                }
                FactOutcome::AwaitingOrder => {
                    blocked = true;
                    break;
                }
            }
        }
        if let Some(ts) = advanced_through {
            let prior = disputes_cursor
                .as_ref()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(-1);
            if ts > prior {
                set_cursor(conn, "disputes_since", &ts.to_string())?;
                disputes_cursor = Some(ts.to_string());
            }
        }
        if blocked {
            break;
        }
    }

    Ok(PollResult {
        ok: true,
        sessions_applied,
        sessions_rejected,
        refunds_applied,
        disputes_applied,
        error: None,
    })
}

fn raise_unrecognised_session(conn: &Connection, u: &UnparsedSession) -> Result<(), String> {
    let amount = format_money_amount(u.amount_cents, &u.currency);
    let message = format!(
        "A payment of {amount} arrived that Farm OS couldn't match to a crop and harvest date."
    );
    attention::raise_once(
        conn,
        "stripe.unrecognised_session",
        Some("stripe_session"),
        Some(&u.session_id),
        &message,
        &["open_in_stripe", "dismiss"],
    )?;
    let _ = &u.reason; // retained for diagnostics; never echoed into Attention.
    Ok(())
}

fn format_money_amount(cents: i64, currency: &str) -> String {
    let dollars = (cents as f64) / 100.0;
    if currency.eq_ignore_ascii_case("cad") || currency.eq_ignore_ascii_case("usd") {
        format!("${dollars:.2}")
    } else {
        format!("{dollars:.2} {}", currency.to_ascii_uppercase())
    }
}

/// Apply a sessions page without advancing the cursor — hard-kill simulation aid.
#[cfg(test)]
pub fn apply_sessions_page_no_cursor(
    conn: &mut Connection,
    page: &[money::PaidSession],
) -> Result<(), String> {
    for session in page {
        let _ = money::apply_paid_session(conn, session)?;
    }
    Ok(())
}

fn read_cursors(conn: &Connection) -> Result<Cursors, String> {
    conn.query_row(
        "SELECT sessions_since, refunds_since, disputes_since
         FROM stripe_cursor WHERE id = 1",
        [],
        |row| {
            Ok(Cursors {
                sessions_since: row.get(0)?,
                refunds_since: row.get(1)?,
                disputes_since: row.get(2)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

fn set_cursor(conn: &Connection, column: &str, value: &str) -> Result<(), String> {
    // Whitelist column names — never interpolate untrusted input.
    let sql = match column {
        "sessions_since" => "UPDATE stripe_cursor SET sessions_since = ?1 WHERE id = 1",
        "refunds_since" => "UPDATE stripe_cursor SET refunds_since = ?1 WHERE id = 1",
        "disputes_since" => "UPDATE stripe_cursor SET disputes_since = ?1 WHERE id = 1",
        _ => return Err("unknown cursor column".into()),
    };
    conn.execute(sql, params![value])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn mark_poll_success(conn: &Connection) -> Result<(), String> {
    let now = db::utc_now_rfc3339();
    conn.execute(
        "UPDATE stripe_cursor
         SET last_poll_ok = ?1, last_poll_err = NULL, poll_fail_count = 0
         WHERE id = 1",
        params![now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn mark_poll_failure(conn: &Connection, err: &str) -> Result<(), String> {
    // Cursors untouched.
    conn.execute(
        "UPDATE stripe_cursor
         SET last_poll_err = ?1,
             poll_fail_count = COALESCE(poll_fail_count, 0) + 1
         WHERE id = 1",
        params![err],
    )
    .map_err(|e| e.to_string())?;

    let fail_count: i64 = conn
        .query_row(
            "SELECT COALESCE(poll_fail_count, 0) FROM stripe_cursor WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    if fail_count >= 3 {
        let since = format_fail_since(Local::now());
        let message = format!("Farm OS hasn't been able to reach Stripe since {since}.");
        // Idempotent: one open poll.failed item (entity_id = stripe).
        attention::raise(
            conn,
            "poll.failed",
            Some("stripe"),
            Some("stripe"),
            &message,
            &["try_now", "dismiss"],
        )?;
    }
    Ok(())
}

fn format_fail_since(now: chrono::DateTime<Local>) -> String {
    let h24 = now.hour();
    let (h12, ampm) = match h24 {
        0 => (12, "am"),
        1..=11 => (h24, "am"),
        12 => (12, "pm"),
        _ => (h24 - 12, "pm"),
    };
    format!("{h12} {ampm}")
}

/// Count orders created since the previous app open, then stamp this open.
/// Call after a poll cycle so newly arrived paid sessions are included.
pub fn take_new_paid_orders(conn: &Connection) -> Result<NewPaidOrders, String> {
    let last_open: Option<String> = conn
        .query_row(
            "SELECT last_app_open FROM stripe_cursor WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();

    let count: i64 = if let Some(ref since) = last_open {
        conn.query_row(
            "SELECT COUNT(*) FROM orders WHERE created_at > ?1",
            [since],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?
    } else {
        // First open: surface every order that arrived before the grower looked.
        conn.query_row("SELECT COUNT(*) FROM orders", [], |r| r.get(0))
            .map_err(|e| e.to_string())?
    };

    let now = db::utc_now_rfc3339();
    conn.execute(
        "UPDATE stripe_cursor SET last_app_open = ?1 WHERE id = 1",
        params![now],
    )
    .map_err(|e| e.to_string())?;

    Ok(NewPaidOrders { count })
}

#[cfg(test)]
pub fn sessions_since(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT sessions_since FROM stripe_cursor WHERE id = 1",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

pub fn reconciliation(conn: &Connection) -> Result<Vec<ReconciliationDate>, String> {
    let caps = trays::capacity_by_harvest_date(conn)?;
    // Sale side: sum per date over the crop rows. Output shape unchanged.
    let mut by_date: BTreeMap<String, (i64, i64, i64)> = BTreeMap::new();
    for cap in &caps {
        let e = by_date.entry(cap.harvest_date.clone()).or_insert((0, 0, 0));
        e.0 += cap.trays;
        e.1 += cap.sold_trays;
        e.2 += cap.remaining_trays;
    }
    let mut out = Vec::new();
    for (harvest_date, (available, sold, remaining)) in by_date {
        let orders = money::list_orders(conn, Some(&harvest_date))?;
        let order_views: Vec<ReconciliationOrder> = orders
            .into_iter()
            .map(|o: OrderView| ReconciliationOrder {
                id: o.id,
                crop_name: o.crop_name,
                quantity: o.quantity,
                state: o.state,
                capacity_consumed: o.capacity_consumed,
                amount_cents: o.amount_cents,
                paid_at: o.paid_at,
            })
            .collect();
        out.push(ReconciliationDate {
            harvest_date,
            available,
            sold,
            remaining,
            orders: order_views,
        });
    }
    Ok(out)
}
