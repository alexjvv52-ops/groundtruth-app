//! Standing-request pull (GT-D18 → desktop, customer QR fence 5). The scans.rs
//! shape: read-only GETs, one observation row per pull (success or failure),
//! one evaluator. Cursor = MAX(max_seq) over ok pulls. The desktop is the
//! authority: every row goes through marketing::ingest_standing_request; unknown
//! or malformed rows leave a delete-proof refusal row and never become a
//! candidate. Sentences signed 2026-08-17 (fence 5, item 5).

use crate::marketing::{self, IngestOutcome};
use crate::scans;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PAGE_LIMIT: usize = 200;
pub const MAX_PAGES: usize = 10;
pub const UNCONFIGURED: &str = "Standing requests unavailable — no scan endpoint is configured.";
pub const NEVER_PULLED: &str = "Standing requests unavailable — no pull has run yet.";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StandingPullView {
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
    request_id: String,
    token: String,
    bags_per_cycle: i64,
    requested_at: String,
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

pub fn cursor(conn: &Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(max_seq), 0) FROM standing_pull_observations WHERE ok = 1",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

pub fn pulled_line(rows: i64, new: i64, known: i64, refused: i64) -> String {
    let noun = if rows == 1 { "request" } else { "requests" };
    format!("Pulled {rows} standing {noun}: {new} new, {known} already known, {refused} refused.")
}

pub fn refusal_line(refused: i64) -> String {
    format!(
        "{refused} refused — no sample carries that token, or the request was malformed. Nothing was invented."
    )
}

pub fn latest_view(conn: &Connection) -> Result<StandingPullView, String> {
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
                "Standing requests unavailable — the last pull failed ({}).",
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
) -> StandingPullView {
    StandingPullView {
        message,
        last_ok_message,
        refusal_message,
        gap_message,
    }
}

/// Live pull. Refuses with the scans sentence when config is missing.
pub fn pull(conn: &mut Connection, now_utc: &str) -> Result<StandingPullView, String> {
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
pub fn pull_with<F>(
    conn: &mut Connection,
    now_utc: &str,
    get: F,
) -> Result<StandingPullView, String>
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
            "{}/standing-requests?after={}",
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
                    &StandingPullObservation {
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
            let error = crate::stripe_client::redact_secrets(
                &format!("standing pull HTTP {status}"),
                &token,
            );
            insert_observation(
                conn,
                &StandingPullObservation {
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
                let error = crate::stripe_client::redact_secrets(
                    &format!("standing pull body: {e}"),
                    &token,
                );
                insert_observation(
                    conn,
                    &StandingPullObservation {
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
            match marketing::ingest_standing_request(
                conn,
                &row.request_id,
                &row.token,
                row.bags_per_cycle,
                &row.requested_at,
            )? {
                IngestOutcome::Written => new_count += 1,
                IngestOutcome::Known => known_count += 1,
                IngestOutcome::Refused(reason) => {
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
        &StandingPullObservation {
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
        marketing::raise_standing_requests(conn)?;
    }
    latest_view(conn)
}

fn newest_observation(conn: &Connection, only_ok: bool) -> Result<Option<Observation>, String> {
    let sql = if only_ok {
        "SELECT ok, error, rows_received, new_count, known_count, refused_count
         FROM standing_pull_observations WHERE ok = 1
         ORDER BY fetched_at DESC, rowid DESC LIMIT 1"
    } else {
        "SELECT ok, error, rows_received, new_count, known_count, refused_count
         FROM standing_pull_observations
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
            "SELECT first_available_seq, max_seq FROM standing_pull_observations
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
                "{k} standing requests are missing from the endpoint and can never be pulled."
            )))
        }
        (_, Some(max), Some(prev)) if max < prev => Ok(Some(
            "The endpoint's log restarted — ids now run below ids already pulled. These counts are not comparable to earlier ones."
                .into(),
        )),
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
        "INSERT INTO standing_pull_refusals
         (id, seq, request_id, token, bags_per_cycle, requested_at, reason, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            Uuid::new_v4().to_string(),
            row.seq,
            row.request_id,
            row.token,
            row.bags_per_cycle,
            row.requested_at,
            reason,
            observed_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// H-7b Fence 1 - the row this writer inserts, named. Field order is the old
/// parameter order. Borrowed fields: write-only, never outlives its call.
///
/// phone_pull carries a structurally identical row (db.rs:546 and :679 declare
/// the same thirteen columns in the same order). Two structs on purpose - the
/// tables are separate and the modules stay separable.
struct StandingPullObservation<'a> {
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

fn insert_observation(conn: &Connection, o: &StandingPullObservation<'_>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO standing_pull_observations
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
