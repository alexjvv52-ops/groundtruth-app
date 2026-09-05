//! Phone-capture pull (GT-D21 → desktop, rack-side fence 2). The standing_pull shape:
//! read-only GETs, one observation row per pull (success or failure), one evaluator.
//! Cursor = MAX(max_seq) over ok pulls. The desktop is the authority for device
//! identity: each row's token is resolved against field_devices; a live Admin's
//! row goes through the ONE ingest of GT-D20 (phone::ingest_phone_proposal);
//! everything else leaves a delete-proof refusal row and never becomes a
//! proposal. Sentences signed 2026-08-17.

use crate::field_devices::{self, DeviceResolution};
use crate::phone::{self, IngestOutcome, PhoneProposalInput};
use crate::scans;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PAGE_LIMIT: usize = 200;
pub const MAX_PAGES: usize = 10;
pub const UNCONFIGURED: &str = "Phone captures unavailable — no scan endpoint is configured.";
pub const NEVER_PULLED: &str = "Phone captures unavailable — no pull has run yet.";
pub const LOG_RESTARTED: &str = "The endpoint's log restarted — ids now run below ids already pulled. These counts are not comparable to earlier ones.";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhonePullView {
    /// The one sentence for the newest pull. Rendered verbatim.
    pub message: String,
    /// The last successful pull's counts, only when the newest pull failed
    /// ("The counts below are from the last successful pull.").
    pub last_ok_message: Option<String>,
    /// Only when the newest successful pull refused any row.
    pub refusal_message: Option<String>,
    /// Only when seqs are missing or the endpoint's log restarted.
    pub gap_message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRow {
    seq: i64,
    proposal_id: String,
    device_token: String,
    verb: String,
    crop_id: String,
    quantity: i64,
    #[serde(default)]
    actual_yield_oz: Option<f64>,
    phone_captured_at: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    received_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullPayload {
    rows: Vec<PullRow>,
    first_available_seq: Option<i64>,
    max_seq: Option<i64>,
    served_at: Option<String>,
}

#[derive(Debug, Clone)]
struct Observation {
    ok: bool,
    error: Option<String>,
    rows_received: i64,
    new_count: i64,
    known_count: i64,
    refused_count: i64,
}

enum RowOutcome {
    Written,
    Known,
    Refused(&'static str),
}

pub fn cursor(conn: &Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(max_seq), 0) FROM phone_pull_observations WHERE ok = 1",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

pub fn pulled_line(rows: i64, new: i64, known: i64, refused: i64) -> String {
    let noun = if rows == 1 { "capture" } else { "captures" };
    format!("Pulled {rows} phone {noun}: {new} new, {known} already known, {refused} refused.")
}

pub fn refusal_line(refused: i64) -> String {
    format!("{refused} refused — not from the live Admin phone, or the capture was malformed. Nothing was invented.")
}

/// FI-10b - when the PC last TRIED the endpoint, ok or not. "Checked" is the
/// honest word: what the attempt found is what `message` already says, and a
/// failed attempt is still the PC having looked. Returned raw - dock_folds
/// composes the operator's words (FI-1). This file owns no display format.
pub fn last_pull_at(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT fetched_at FROM phone_pull_observations
         ORDER BY fetched_at DESC, rowid DESC LIMIT 1",
        [],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn latest_view(conn: &Connection) -> Result<PhonePullView, String> {
    let (url, token) = scans::endpoint_and_token(conn)?;
    if url.is_none() || token.is_none() {
        return Ok(view(UNCONFIGURED.to_string(), None, None, None));
    }
    let Some(newest) = newest_observation(conn, false)? else {
        return Ok(view(NEVER_PULLED.to_string(), None, None, None));
    };
    let newest_ok = newest_observation(conn, true)?;
    let ok_line = newest_ok
        .as_ref()
        .map(|o| pulled_line(o.rows_received, o.new_count, o.known_count, o.refused_count));
    let refusal = newest_ok
        .as_ref()
        .filter(|o| o.refused_count > 0)
        .map(|o| refusal_line(o.refused_count));
    let gap = gap_message(conn)?;
    if newest.ok {
        Ok(view(
            ok_line.unwrap_or_else(|| pulled_line(0, 0, 0, 0)),
            None,
            refusal,
            gap,
        ))
    } else if newest_ok.is_none() {
        Ok(view(
            format!(
                "Phone captures unavailable — the last pull failed ({}).",
                newest.error.as_deref().unwrap_or("unknown")
            ),
            None,
            None,
            None,
        ))
    } else {
        Ok(view(
            format!(
                "The last pull failed ({}). The counts below are from the last successful pull.",
                newest.error.as_deref().unwrap_or("unknown")
            ),
            ok_line,
            refusal,
            gap,
        ))
    }
}

fn view(
    message: String,
    last_ok_message: Option<String>,
    refusal_message: Option<String>,
    gap_message: Option<String>,
) -> PhonePullView {
    PhonePullView {
        message,
        last_ok_message,
        refusal_message,
        gap_message,
    }
}

/// Live pull. Refuses with the scans sentence when config is missing.
pub fn pull(conn: &mut Connection, now_utc: &str) -> Result<PhonePullView, String> {
    let (url, token) = scans::endpoint_and_token(conn)?;
    match (url, token) {
        (Some(_), Some(_)) => pull_with(conn, now_utc, scans::ureq_get),
        (url, token) => {
            let mut missing = Vec::new();
            if url.is_none() {
                missing.push("scan endpoint URL");
            }
            if token.is_none() {
                missing.push("pull token");
            }
            Err(format!(
                "Configure a {} before pulling.",
                missing.join(" and ")
            ))
        }
    }
}

/// Injected-gateway pull (tests / inversion). Pages until a page is shorter
/// than PAGE_LIMIT or MAX_PAGES is reached; every row is written, known, or
/// refused before the cursor moves. On any transport or body error the pull
/// records a failed observation and the cursor stays where it was — rows
/// already written are durable and dedupe as "already known" next time.
pub fn pull_with<F>(conn: &mut Connection, now_utc: &str, get: F) -> Result<PhonePullView, String>
where
    F: Fn(&str, &str) -> Result<(i32, String), String>,
{
    let (endpoint, token) = scans::endpoint_and_token(conn)?;
    let endpoint =
        endpoint.ok_or_else(|| "Configure a scan endpoint URL before pulling.".to_string())?;
    let token = token.ok_or_else(|| "Configure a pull token before pulling.".to_string())?;
    let start = cursor(conn)?;
    let mut after = start;
    let mut rows_received = 0i64;
    let mut new_count = 0i64;
    let mut known_count = 0i64;
    let mut refused_count = 0i64;
    let mut first_available: Option<i64> = None;
    let mut endpoint_max: Option<i64> = None;
    let mut served_at: Option<String> = None;
    let mut max_seen: Option<i64> = None;
    let mut caught_up = false;
    let mut last_url = String::new();
    for page in 0..MAX_PAGES {
        let url = format!(
            "{}/field-proposals?after={}",
            endpoint.trim_end_matches('/'),
            after
        );
        last_url = url.clone();
        let (status, body) = match get(&url, &token) {
            Ok(pair) => pair,
            Err(msg) => {
                let error = crate::stripe_client::redact_secrets(&msg, &token);
                insert_observation(
                    conn,
                    &PhonePullObservation {
                        fetched_at: now_utc,
                        url: &url,
                        http_status: None,
                        ok: false,
                        served_at: None,
                        rows_received: None,
                        new_count: None,
                        known_count: None,
                        refused_count: None,
                        first_available_seq: None,
                        max_seq: None,
                        error: Some(&error),
                    },
                )?;
                return latest_view(conn);
            }
        };
        if status != 200 {
            let error =
                crate::stripe_client::redact_secrets(&format!("phone pull HTTP {status}"), &token);
            insert_observation(
                conn,
                &PhonePullObservation {
                    fetched_at: now_utc,
                    url: &url,
                    http_status: Some(status),
                    ok: false,
                    served_at: None,
                    rows_received: None,
                    new_count: None,
                    known_count: None,
                    refused_count: None,
                    first_available_seq: None,
                    max_seq: None,
                    error: Some(&error),
                },
            )?;
            return latest_view(conn);
        }
        let payload: PullPayload = match serde_json::from_str(&body) {
            Ok(p) => p,
            Err(e) => {
                let error =
                    crate::stripe_client::redact_secrets(&format!("phone pull body: {e}"), &token);
                insert_observation(
                    conn,
                    &PhonePullObservation {
                        fetched_at: now_utc,
                        url: &url,
                        http_status: Some(status),
                        ok: false,
                        served_at: None,
                        rows_received: None,
                        new_count: None,
                        known_count: None,
                        refused_count: None,
                        first_available_seq: None,
                        max_seq: None,
                        error: Some(&error),
                    },
                )?;
                return latest_view(conn);
            }
        };
        if page == 0 {
            first_available = payload.first_available_seq;
        }
        endpoint_max = payload.max_seq;
        served_at = payload.served_at.clone();
        let n = payload.rows.len();
        for row in &payload.rows {
            let outcome = match field_devices::resolve_token(conn, &row.device_token)? {
                DeviceResolution::Unknown => RowOutcome::Refused("unknown_device"),
                DeviceResolution::Retired => RowOutcome::Refused("retired_device"),
                DeviceResolution::Live(device_id) => {
                    let input = PhoneProposalInput {
                        proposal_id: row.proposal_id.clone(),
                        device_id,
                        verb: row.verb.clone(),
                        crop_id: row.crop_id.clone(),
                        quantity: row.quantity,
                        actual_yield_oz: row.actual_yield_oz,
                        phone_captured_at: row.phone_captured_at.clone(),
                        note: row.note.clone(),
                    };
                    match phone::ingest_phone_proposal(conn, &input)? {
                        IngestOutcome::Written => RowOutcome::Written,
                        IngestOutcome::Known => RowOutcome::Known,
                        IngestOutcome::Refused(r) => RowOutcome::Refused(r),
                    }
                }
            };
            match outcome {
                RowOutcome::Written => new_count += 1,
                RowOutcome::Known => known_count += 1,
                RowOutcome::Refused(reason) => {
                    refused_count += 1;
                    insert_refusal(conn, now_utc, row, reason)?;
                }
            }
            max_seen = Some(max_seen.map_or(row.seq, |m| m.max(row.seq)));
        }
        rows_received += n as i64;
        if n < PAGE_LIMIT {
            caught_up = true;
            break;
        }
        after = max_seen.unwrap_or(after);
    }
    // The cursor never jumps past a row this pull has not handled: the
    // endpoint's max only when caught up, else the last processed seq.
    // Store the endpoint's max when caught up (not max(start, endpoint_max))
    // so a restarted log is visible; cursor() is MAX(max_seq) and will not
    // move backwards.
    let cursor_after = if caught_up {
        endpoint_max.unwrap_or(max_seen.unwrap_or(start))
    } else {
        max_seen.unwrap_or(start)
    };
    insert_observation(
        conn,
        &PhonePullObservation {
            fetched_at: now_utc,
            url: &last_url,
            http_status: Some(200),
            ok: true,
            served_at: served_at.as_deref(),
            rows_received: Some(rows_received),
            new_count: Some(new_count),
            known_count: Some(known_count),
            refused_count: Some(refused_count),
            first_available_seq: first_available,
            max_seq: Some(cursor_after),
            error: None,
        },
    )?;
    if new_count > 0 {
        phone::raise_phone_proposals(conn)?;
    }
    latest_view(conn)
}

/// Automatic pull. Ok(None) — and no request — when no live Admin exists; the
/// unconfigured sentence, without a request, when the endpoint is missing.
pub fn auto_pull(conn: &mut Connection, now_utc: &str) -> Result<Option<PhonePullView>, String> {
    auto_pull_with(conn, now_utc, scans::ureq_get)
}

pub fn auto_pull_with<F>(
    conn: &mut Connection,
    now_utc: &str,
    get: F,
) -> Result<Option<PhonePullView>, String>
where
    F: Fn(&str, &str) -> Result<(i32, String), String>,
{
    if !field_devices::has_live_admin(conn)? {
        return Ok(None);
    }
    let (url, token) = scans::endpoint_and_token(conn)?;
    if url.is_none() || token.is_none() {
        return Ok(Some(latest_view(conn)?));
    }
    Ok(Some(pull_with(conn, now_utc, get)?))
}

fn newest_observation(conn: &Connection, only_ok: bool) -> Result<Option<Observation>, String> {
    let sql = if only_ok {
        "SELECT ok, error, rows_received, new_count, known_count, refused_count
         FROM phone_pull_observations WHERE ok = 1
         ORDER BY fetched_at DESC, rowid DESC LIMIT 1"
    } else {
        "SELECT ok, error, rows_received, new_count, known_count, refused_count
         FROM phone_pull_observations
         ORDER BY fetched_at DESC, rowid DESC LIMIT 1"
    };
    conn.query_row(sql, [], |r| {
        Ok(Observation {
            ok: r.get::<_, i64>(0)? == 1,
            error: r.get(1)?,
            rows_received: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
            new_count: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            known_count: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
            refused_count: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
        })
    })
    .optional()
    .map_err(|e| e.to_string())
}

fn gap_message(conn: &Connection) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT first_available_seq, max_seq FROM phone_pull_observations
             WHERE ok = 1 ORDER BY fetched_at DESC, rowid DESC LIMIT 2",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(Option<i64>, Option<i64>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    if rows.len() < 2 {
        return Ok(None);
    }
    let (first, max) = rows[0];
    let (_prev_first, prev_max) = rows[1];
    match (first, max, prev_max) {
        (Some(first), _, Some(prev)) if first > prev + 1 => {
            let k = first - prev - 1;
            Ok(Some(format!(
                "{k} phone captures are missing from the endpoint and can never be pulled."
            )))
        }
        (_, Some(max), Some(prev)) if max < prev => Ok(Some(LOG_RESTARTED.into())),
        _ => Ok(None),
    }
}

fn insert_refusal(
    conn: &Connection,
    observed_at: &str,
    row: &PullRow,
    reason: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO phone_pull_refusals
         (id, seq, proposal_id, device_token_hash, verb, crop_id, quantity, actual_yield_oz, phone_captured_at, reason, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            Uuid::new_v4().to_string(), row.seq, row.proposal_id, field_devices::token_hash(&row.device_token),
            row.verb, row.crop_id, row.quantity, row.actual_yield_oz, row.phone_captured_at, reason, observed_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// H-7b Fence 1 - the row this writer inserts, named. Field order is the old
/// parameter order. Borrowed fields for the same reason as ScanObservation:
/// write-only, never outlives its call, allocates nothing new.
///
/// standing_pull carries a structurally identical row (db.rs:546 and :679
/// declare the same thirteen columns in the same order). They are kept as two
/// structs on purpose - the tables are separate and the modules stay
/// separable.
struct PhonePullObservation<'a> {
    fetched_at: &'a str,
    url: &'a str,
    http_status: Option<i32>,
    ok: bool,
    served_at: Option<&'a str>,
    rows_received: Option<i64>,
    new_count: Option<i64>,
    known_count: Option<i64>,
    refused_count: Option<i64>,
    first_available_seq: Option<i64>,
    max_seq: Option<i64>,
    error: Option<&'a str>,
}

fn insert_observation(conn: &Connection, o: &PhonePullObservation<'_>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO phone_pull_observations
         (id, fetched_at, url, http_status, ok, served_at, rows_received, new_count,
          known_count, refused_count, first_available_seq, max_seq, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            Uuid::new_v4().to_string(),
            o.fetched_at,
            o.url,
            o.http_status,
            if o.ok { 1 } else { 0 },
            o.served_at,
            o.rows_received,
            o.new_count,
            o.known_count,
            o.refused_count,
            o.first_available_seq,
            o.max_seq,
            o.error,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
