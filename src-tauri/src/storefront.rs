//! GET-only storefront reader. This module performs one GET. It contains no
//! write verb of any kind — no POST, PUT, PATCH or DELETE, and no credential.

use crate::observed::Observed;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use uuid::Uuid;

/// The operator's own shop origin. A public build ships no address here:
/// the reader stays inert until one is set. C1 — no Settings field, no
/// Kind, no schema.
pub const STOREFRONT_URL: &str = "";

/// C1 — what `fetch_and_record` says when there is no origin to read.
pub const STOREFRONT_NO_ORIGIN_LINE: &str =
    "No storefront address is set, so there is nothing to read.";

/// True only when `STOREFRONT_URL` names a real origin. Empty or an
/// `example.` placeholder means the door refuses instead of fetching.
fn origin_is_set() -> bool {
    let url = STOREFRONT_URL.trim();
    !url.is_empty() && !url.starts_with("https://example.") && !url.starts_with("http://example.")
}

pub const KNOWN_VARIETIES: [&str; 5] = [
    "Dun Peas",
    "Mellow Micro Mix",
    "Purple Kohlrabi",
    "Red Arrow Radish",
    "Spicy Micro Mix",
];

pub const SOLD_OUT_TEXT: &str = "Sold out — request it below";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variety {
    pub name: String,
    pub price: String,
    pub availability: String,
}

#[derive(Debug, Clone)]
pub struct StorefrontReading {
    pub ok: bool,
    pub varieties: Vec<Variety>,
    pub error: Option<String>,
    pub http_status: Option<i32>,
    pub content_sha256: Option<String>,
    pub fetched_at: String,
    pub url: String,
}

/// One GET via ureq (stripe_client pattern: `ureq::get(&url).call()`), with a
/// 10 second timeout so a dead network cannot hang app start. Every call writes
/// exactly one `storefront_observations` row — on success and on failure.
pub fn fetch_and_record(conn: &Connection, now_utc: &str) -> Result<StorefrontReading, String> {
    if !origin_is_set() {
        return Err(STOREFRONT_NO_ORIGIN_LINE.to_string());
    }
    // Pattern from stripe_client.rs: ureq::get(&url).call() — that file sets
    // no timeout. Wave 1 adds timeout_global(10s) so startup cannot hang.
    let result = ureq::get(STOREFRONT_URL)
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
                .map_err(|e| format!("storefront body read failed: {e}"))?;
            let sha = sha256_hex(body.as_bytes());
            match parse_storefront_html(&body) {
                Ok(varieties) => {
                    let reading = StorefrontReading {
                        ok: true,
                        varieties,
                        error: None,
                        http_status: Some(status),
                        content_sha256: Some(sha),
                        fetched_at: now_utc.to_string(),
                        url: STOREFRONT_URL.to_string(),
                    };
                    insert_observation(conn, &reading)?;
                    Ok(reading)
                }
                Err(err) => {
                    let reading = StorefrontReading {
                        ok: false,
                        varieties: Vec::new(),
                        error: Some(err),
                        http_status: Some(status),
                        content_sha256: Some(sha),
                        fetched_at: now_utc.to_string(),
                        url: STOREFRONT_URL.to_string(),
                    };
                    insert_observation(conn, &reading)?;
                    Ok(reading)
                }
            }
        }
        Err(e) => {
            let (http_status, content_sha256, err) = match &e {
                ureq::Error::StatusCode(code) => {
                    (Some(*code as i32), None, format!("storefront HTTP {code}"))
                }
                other => (None, None, format!("storefront request failed: {other}")),
            };
            let reading = StorefrontReading {
                ok: false,
                varieties: Vec::new(),
                error: Some(err),
                http_status,
                content_sha256,
                fetched_at: now_utc.to_string(),
                url: STOREFRONT_URL.to_string(),
            };
            insert_observation(conn, &reading)?;
            Ok(reading)
        }
    }
}

/// Parse the storefront HTML. Structural misses return Err with a precise name.
pub fn parse_storefront_html(html: &str) -> Result<Vec<Variety>, String> {
    let text = strip_tags_to_text(html);
    let mut varieties = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    let mut no_price: Vec<&str> = Vec::new();

    for name in KNOWN_VARIETIES {
        // Card titles render as `>{name}</div>`; ingredient chips use `</li>`.
        let marker = format!(">{name}</div>");
        let Some(card_at) = html.find(&marker) else {
            missing.push(name);
            continue;
        };
        let after = &html[card_at + marker.len()..];
        let section_html = truncate_at_next_card(after);
        let section_text = strip_tags_to_text(section_html);
        let Some(price) = find_price(&section_text) else {
            no_price.push(name);
            continue;
        };
        let availability = classify_availability(&section_text);
        varieties.push(Variety {
            name: name.to_string(),
            price,
            availability,
        });
    }

    if !missing.is_empty() {
        let named = missing.join(", ");
        return Err(format!(
            "{} of 5 varieties parsed — {named} not found",
            5 - missing.len()
        ));
    }
    if !no_price.is_empty() {
        return Err(format!(
            "{} found but no '$X.XX / oz' price",
            no_price.join(", ")
        ));
    }
    if varieties.len() != 5 {
        return Err(format!(
            "{} of 5 varieties parsed — unexpected count",
            varieties.len()
        ));
    }

    // Keep the plain-text view available for unit tests that inject wording.
    let _ = text;
    Ok(varieties)
}

/// Parse a plain-text storefront body (fixtures / synthetic cases). Uses the
/// same structural rules against `KNOWN_VARIETIES` and the price shape.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn parse_storefront_text(text: &str) -> Result<Vec<Variety>, String> {
    let mut varieties = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    let mut no_price: Vec<&str> = Vec::new();

    for (i, name) in KNOWN_VARIETIES.iter().enumerate() {
        let Some(pos) = text.find(name) else {
            missing.push(name);
            continue;
        };
        let end = KNOWN_VARIETIES
            .iter()
            .enumerate()
            .filter(|(j, other)| *j > i && text[pos + name.len()..].contains(*other))
            .filter_map(|(j, other)| {
                text[pos + name.len()..]
                    .find(other)
                    .map(|rel| (j, pos + name.len() + rel))
            })
            .min_by_key(|(_, at)| *at)
            .map(|(_, at)| at)
            .unwrap_or(text.len());
        let section = &text[pos..end];
        let Some(price) = find_price(section) else {
            no_price.push(name);
            continue;
        };
        let availability = classify_availability(section);
        varieties.push(Variety {
            name: (*name).to_string(),
            price,
            availability,
        });
    }

    if !missing.is_empty() {
        let named = missing.join(", ");
        return Err(format!(
            "{} of 5 varieties parsed — {named} not found",
            5 - missing.len()
        ));
    }
    if !no_price.is_empty() {
        return Err(format!(
            "{} found but no '$X.XX / oz' price",
            no_price.join(", ")
        ));
    }
    if varieties.len() != 5 {
        return Err(format!(
            "{} of 5 varieties parsed — unexpected count",
            varieties.len()
        ));
    }
    Ok(varieties)
}

pub fn latest_ok_reading(conn: &Connection) -> Result<Option<Observed<Vec<Variety>>>, String> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT fetched_at, url, payload
             FROM storefront_observations
             WHERE ok = 1 AND payload IS NOT NULL
             ORDER BY fetched_at DESC
             LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let Some((fetched_at, url, payload)) = row else {
        return Ok(None);
    };
    let varieties: Vec<Variety> =
        serde_json::from_str(&payload).map_err(|e| format!("stored storefront payload: {e}"))?;
    Ok(Some(Observed::new(varieties, fetched_at, url)))
}

#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn consecutive_failures(conn: &Connection) -> Result<i64, String> {
    let last_ok_at: Option<String> = conn
        .query_row(
            "SELECT fetched_at FROM storefront_observations
             WHERE ok = 1
             ORDER BY fetched_at DESC
             LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let count = match last_ok_at {
        Some(at) => conn.query_row(
            "SELECT COUNT(*) FROM storefront_observations
             WHERE ok = 0 AND fetched_at > ?1",
            params![at],
            |r| r.get(0),
        ),
        None => conn.query_row(
            "SELECT COUNT(*) FROM storefront_observations WHERE ok = 0",
            [],
            |r| r.get(0),
        ),
    }
    .map_err(|e| e.to_string())?;
    Ok(count)
}

/// Insert a synthetic observation row (tests / pulled-cable simulation).
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
#[allow(clippy::too_many_arguments)] // H-7 Class C: the arg list mirrors the table's column list. Narrowing is H-7b (StorefrontReading precedent).
pub fn record_observation(
    conn: &Connection,
    fetched_at: &str,
    url: &str,
    http_status: Option<i32>,
    content_sha256: Option<&str>,
    ok: bool,
    varieties: Option<&[Variety]>,
    error: Option<&str>,
) -> Result<(), String> {
    let reading = StorefrontReading {
        ok,
        varieties: varieties.unwrap_or(&[]).to_vec(),
        error: error.map(|s| s.to_string()),
        http_status,
        content_sha256: content_sha256.map(|s| s.to_string()),
        fetched_at: fetched_at.to_string(),
        url: url.to_string(),
    };
    insert_observation(conn, &reading)
}

fn insert_observation(conn: &Connection, reading: &StorefrontReading) -> Result<(), String> {
    let payload = if reading.ok {
        Some(serde_json::to_string(&reading.varieties).map_err(|e| e.to_string())?)
    } else {
        None
    };
    conn.execute(
        "INSERT INTO storefront_observations
         (id, fetched_at, url, http_status, content_sha256, ok, payload, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            Uuid::new_v4().to_string(),
            reading.fetched_at,
            reading.url,
            reading.http_status,
            reading.content_sha256,
            if reading.ok { 1 } else { 0 },
            payload,
            reading.error,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn classify_availability(section: &str) -> String {
    if section.contains(SOLD_OUT_TEXT) {
        SOLD_OUT_TEXT.to_string()
    } else {
        extract_other_availability(section)
    }
}

fn extract_other_availability(section: &str) -> String {
    // Capture the wording after the price, verbatim, until a CTA or end.
    let Some(price_at) = find_price_span(section) else {
        return String::new();
    };
    let after = section[price_at.1..].trim_start();
    let end = ["Request this variety", "Order this week's harvest", "\n"]
        .iter()
        .filter_map(|m| after.find(m))
        .min()
        .unwrap_or(after.len());
    after[..end].trim().to_string()
}

fn truncate_at_next_card(after: &str) -> &str {
    let mut earliest: Option<usize> = None;
    for name in KNOWN_VARIETIES {
        let marker = format!(">{name}</div>");
        if let Some(at) = after.find(&marker) {
            earliest = Some(earliest.map_or(at, |e| e.min(at)));
        }
    }
    match earliest {
        Some(at) => &after[..at],
        None => after,
    }
}

fn strip_tags_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if i + 3 < bytes.len() && &html[i..i + 4] == "<!--" {
                if let Some(end) = html[i + 4..].find("-->") {
                    i = i + 4 + end + 3;
                    continue;
                }
            }
            if let Some(end) = html[i + 1..].find('>') {
                i = i + 1 + end + 1;
                continue;
            }
            i += 1;
            continue;
        }
        out.push(html[i..].chars().next().unwrap());
        i += html[i..].chars().next().unwrap().len_utf8();
    }
    decode_basic_entities(&out)
}

fn decode_basic_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

/// Find `$X.XX / oz` without the regex crate (not a direct dependency).
fn find_price(text: &str) -> Option<String> {
    find_price_span(text).map(|(start, end)| text[start..end].to_string())
}

fn find_price_span(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let mut j = i + 1;
            let mut digits = 0;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                digits += 1;
                j += 1;
            }
            if digits > 0
                && j + 3 < bytes.len()
                && bytes[j] == b'.'
                && bytes[j + 1].is_ascii_digit()
                && bytes[j + 2].is_ascii_digit()
                && text.get(j + 3..j + 8) == Some(" / oz")
            {
                return Some((i, j + 8));
            }
        }
        i += 1;
    }
    None
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
