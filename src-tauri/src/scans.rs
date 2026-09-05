//! Scan pull (GT-D16, Phase 8). One read-only GET, one observation row,
//! one evaluator.
//!
//! Precedents, cited so the shape is not invented:
//!
//! - [`crate::storefront::fetch_and_record`]: one call, one row, on success
//!   AND on failure.
//! - [`crate::poll::run_poll_from_db`] / [`crate::poll::run_poll`]: the
//!   injected-gateway split that makes the network path testable
//!   (`pull` / `pull_with` here).

use crate::db;
use crate::observed::Observed;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanConfigView {
    pub endpoint_url: Option<String>,
    pub token_set: bool,
    pub configured_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanView {
    /// None when unconfigured, never pulled, or failed with no prior
    /// good pull. NEVER Some(0) standing in for "unknown".
    pub count: Option<Observed<i64>>,
    /// The one sentence. Composed here and rendered verbatim.
    pub message: String,
    /// Only when ids are actually missing.
    pub gap_message: Option<String>,
    /// Permanent, always present.
    pub printed_note: String,
}

const PRINTED_NOTE: &str =
    "Packs printed before counting began carry no code and are not in this number.";

const URL_REFUSAL: &str = "The scan endpoint URL must start with https://.";

#[derive(Debug, Clone)]
struct RawConfig {
    endpoint_url: Option<String>,
    pull_token: Option<String>,
    configured_at: Option<String>,
}

#[derive(Debug, Clone)]
struct Observation {
    ok: bool,
    error: Option<String>,
    served_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullPayload {
    new_count: i64,
    first_available_id: Option<i64>,
    max_scan_id: Option<i64>,
    served_at: Option<String>,
}

pub fn config(conn: &Connection) -> Result<ScanConfigView, String> {
    let raw = read_raw(conn)?;
    Ok(ScanConfigView {
        endpoint_url: raw.endpoint_url,
        token_set: raw
            .pull_token
            .as_ref()
            .map(|t| !t.is_empty())
            .unwrap_or(false),
        configured_at: raw.configured_at,
    })
}

pub fn set_config(
    conn: &Connection,
    endpoint_url: Option<&str>,
    pull_token: Option<&str>,
) -> Result<ScanConfigView, String> {
    let current = read_raw(conn)?;
    let new_url = match endpoint_url {
        Some(u) => {
            let trimmed = u.trim();
            if trimmed.is_empty() || !trimmed.starts_with("https://") {
                return Err(URL_REFUSAL.to_string());
            }
            Some(trimmed.to_string())
        }
        None => current.endpoint_url.clone(),
    };
    let new_token = match pull_token {
        Some(t) => {
            let trimmed = t.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        None => current.pull_token.clone(),
    };
    let changed = new_url != current.endpoint_url || new_token != current.pull_token;
    let configured_at = if changed {
        Some(db::utc_now_rfc3339())
    } else {
        current.configured_at.clone()
    };
    conn.execute(
        "UPDATE scan_config
         SET endpoint_url = ?1, pull_token = ?2, configured_at = ?3
         WHERE id = 1",
        params![new_url, new_token, configured_at],
    )
    .map_err(|e| e.to_string())?;
    config(conn)
}

pub fn cursor(conn: &Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(max_scan_id), 0) FROM scan_observations WHERE ok = 1",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// Endpoint URL and pull token, trimmed, empty → None. Shared with
/// standing_pull (customer QR fence 5): one config, two read-only pulls.
pub(crate) fn endpoint_and_token(
    conn: &Connection,
) -> Result<(Option<String>, Option<String>), String> {
    let raw = read_raw(conn)?;
    let url = raw
        .endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let token = raw
        .pull_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok((url, token))
}

pub fn latest_view(conn: &Connection) -> Result<ScanView, String> {
    let raw = read_raw(conn)?;
    if !is_configured(&raw) {
        return Ok(ScanView {
            count: None,
            message: "Scan count unavailable — no scan endpoint is configured.".into(),
            gap_message: None,
            printed_note: PRINTED_NOTE.into(),
        });
    }

    let newest = newest_observation(conn)?;
    let Some(newest) = newest else {
        return Ok(ScanView {
            count: None,
            message: "Scan count unavailable — no pull has run yet.".into(),
            gap_message: None,
            printed_note: PRINTED_NOTE.into(),
        });
    };

    let newest_ok = newest_ok_observation(conn)?;
    let count = match &newest_ok {
        Some(ok_row) => {
            let total = sum_ok_new_count(conn)?;
            let served = ok_row
                .served_at
                .clone()
                .ok_or_else(|| "newest ok pull has no served_at".to_string())?;
            let origin = raw
                .endpoint_url
                .clone()
                .ok_or_else(|| "configured endpoint missing".to_string())?;
            Some(Observed::new(total, served, origin))
        }
        None => None,
    };
    let gap_message = gap_message(conn)?;

    let message = if newest.ok {
        let n = count.as_ref().map(|c| c.value).unwrap_or(0);
        format!("{n} scans since counting began.")
    } else if newest_ok.is_none() {
        format!(
            "Scan count unavailable — the last pull failed ({}).",
            newest.error.as_deref().unwrap_or("unknown")
        )
    } else {
        format!(
            "The last pull failed ({}). The count below is from the last successful pull.",
            newest.error.as_deref().unwrap_or("unknown")
        )
    };

    Ok(ScanView {
        count,
        message,
        gap_message,
        printed_note: PRINTED_NOTE.into(),
    })
}

pub fn pull(conn: &Connection, now_utc: &str) -> Result<ScanView, String> {
    let raw = read_raw(conn)?;
    let url = raw
        .endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let token = raw
        .pull_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (url, token) {
        (Some(_), Some(_)) => {
            pull_with(conn, now_utc, ureq_get)?;
            latest_view(conn)
        }
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

pub fn pull_with<F>(conn: &Connection, now_utc: &str, get: F) -> Result<ScanView, String>
where
    F: Fn(&str, &str) -> Result<(i32, String), String>,
{
    let raw = read_raw(conn)?;
    let endpoint = raw
        .endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Configure a scan endpoint URL before pulling.".to_string())?;
    let token = raw
        .pull_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Configure a pull token before pulling.".to_string())?;

    let after = cursor(conn)?;
    let url = format!("{}/scans?after={}", endpoint.trim_end_matches('/'), after);

    match get(&url, token) {
        Ok((status, body)) if status == 200 => match serde_json::from_str::<PullPayload>(&body) {
            Ok(payload) => {
                insert_observation(
                    conn,
                    &ScanObservation {
                        fetched_at: now_utc,
                        url: &url,
                        http_status: Some(status),
                        ok: true,
                        served_at: payload.served_at.as_deref(),
                        new_count: Some(payload.new_count),
                        first_available_id: payload.first_available_id,
                        max_scan_id: payload.max_scan_id,
                        error: None,
                    },
                )?;
            }
            Err(e) => {
                let error =
                    crate::stripe_client::redact_secrets(&format!("scan pull body: {e}"), token);
                insert_observation(
                    conn,
                    &ScanObservation {
                        fetched_at: now_utc,
                        url: &url,
                        http_status: Some(status),
                        ok: false,
                        served_at: None,
                        new_count: None,
                        first_available_id: None,
                        max_scan_id: None,
                        error: Some(&error),
                    },
                )?;
            }
        },
        Ok((status, _)) => {
            let error =
                crate::stripe_client::redact_secrets(&format!("scan pull HTTP {status}"), token);
            insert_observation(
                conn,
                &ScanObservation {
                    fetched_at: now_utc,
                    url: &url,
                    http_status: Some(status),
                    ok: false,
                    served_at: None,
                    new_count: None,
                    first_available_id: None,
                    max_scan_id: None,
                    error: Some(&error),
                },
            )?;
        }
        Err(msg) => {
            let error = crate::stripe_client::redact_secrets(&msg, token);
            insert_observation(
                conn,
                &ScanObservation {
                    fetched_at: now_utc,
                    url: &url,
                    http_status: None,
                    ok: false,
                    served_at: None,
                    new_count: None,
                    first_available_id: None,
                    max_scan_id: None,
                    error: Some(&error),
                },
            )?;
        }
    }
    latest_view(conn)
}

pub(crate) fn ureq_get(url: &str, token: &str) -> Result<(i32, String), String> {
    let result = ureq::get(url)
        .header("X-Pull-Token", token)
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .call();
    match result {
        Ok(mut resp) => {
            let status = resp.status().as_u16() as i32;
            let body = resp
                .body_mut()
                .read_to_string()
                .map_err(|e| format!("scan pull body read failed: {e}"))?;
            Ok((status, body))
        }
        Err(ureq::Error::StatusCode(code)) => Err(format!("scan pull HTTP {code}")),
        Err(other) => Err(format!("scan pull failed: {other}")),
    }
}

fn read_raw(conn: &Connection) -> Result<RawConfig, String> {
    conn.query_row(
        "SELECT endpoint_url, pull_token, configured_at FROM scan_config WHERE id = 1",
        [],
        |r| {
            Ok(RawConfig {
                endpoint_url: r.get(0)?,
                pull_token: r.get(1)?,
                configured_at: r.get(2)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

fn is_configured(raw: &RawConfig) -> bool {
    raw.endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some()
        && raw
            .pull_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_some()
}

fn newest_observation(conn: &Connection) -> Result<Option<Observation>, String> {
    conn.query_row(
        "SELECT ok, error, served_at
         FROM scan_observations
         ORDER BY fetched_at DESC, rowid DESC
         LIMIT 1",
        [],
        |r| {
            Ok(Observation {
                ok: r.get::<_, i64>(0)? == 1,
                error: r.get(1)?,
                served_at: r.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn newest_ok_observation(conn: &Connection) -> Result<Option<Observation>, String> {
    conn.query_row(
        "SELECT ok, error, served_at
         FROM scan_observations
         WHERE ok = 1
         ORDER BY fetched_at DESC, rowid DESC
         LIMIT 1",
        [],
        |r| {
            Ok(Observation {
                ok: true,
                error: r.get(1)?,
                served_at: r.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn sum_ok_new_count(conn: &Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(SUM(new_count), 0) FROM scan_observations WHERE ok = 1",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn gap_message(conn: &Connection) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT first_available_id, max_scan_id
             FROM scan_observations
             WHERE ok = 1
             ORDER BY fetched_at DESC, rowid DESC
             LIMIT 2",
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
    let (first_available_id, max_scan_id) = rows[0];
    let (_prev_first, prev_max) = rows[1];
    match (first_available_id, max_scan_id, prev_max) {
        (Some(first), _, Some(prev)) if first > prev + 1 => {
            let k = first - prev - 1;
            Ok(Some(format!(
                "{k} scan ids are missing from the endpoint and can never be counted."
            )))
        }
        (_, Some(max), Some(prev)) if max < prev => Ok(Some(
            "The endpoint's log restarted — ids now run below ids already counted. This total is not comparable to earlier ones."
                .into(),
        )),
        _ => Ok(None),
    }
}

/// H-7b Fence 1 - the row this writer inserts, named. Field order is the old
/// parameter order, so the mapping is positional and nothing was reordered.
///
/// Fields are borrowed, not owned. StorefrontReading owns its fields because
/// `fetch_and_record` returns it; this struct is write-only and never outlives
/// its call, so every caller keeps passing the `&str` it already had and this
/// land allocates nothing it did not allocate before.
struct ScanObservation<'a> {
    fetched_at: &'a str,
    url: &'a str,
    http_status: Option<i32>,
    ok: bool,
    served_at: Option<&'a str>,
    new_count: Option<i64>,
    first_available_id: Option<i64>,
    max_scan_id: Option<i64>,
    error: Option<&'a str>,
}

fn insert_observation(conn: &Connection, o: &ScanObservation<'_>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO scan_observations
         (id, fetched_at, url, http_status, ok, served_at, new_count,
          first_available_id, max_scan_id, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            Uuid::new_v4().to_string(),
            o.fetched_at,
            o.url,
            o.http_status,
            if o.ok { 1 } else { 0 },
            o.served_at,
            o.new_count,
            o.first_available_id,
            o.max_scan_id,
            o.error,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
