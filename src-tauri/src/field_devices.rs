//! Rack-side fence 2 (GT-D21): the paired field terminal. Reference data (the
//! scan_config precedent): the token is minted here, shown once inside the
//! pairing link, and only its SHA-256 hash is stored — a secret never enters
//! the ledger. Exactly one live Admin (partial unique index); pairing retires
//! the previous Admin in the same transaction; explicit Retire is allowed. The
//! relay does no device auth: this table is where a pulled row's token is
//! resolved. Bytes signed 2026-08-17 (rack-side fence 2).

use crate::db;
use crate::phone;
use crate::projection;
use crate::scans;
use crate::trays;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

// ---- Signed bytes. ONE set each. ----
pub const NO_ADMIN_PAIRED: &str = "No Admin phone is paired.";
pub const PAIRING_TEXT: &str =
    "Open this link on the phone. It is the phone's key — pairing another phone retires this one.";
pub const RETIRED_LINE: &str = "Retired. Captures from that phone are refused from now on.";

pub fn paired_line(stamp: &str) -> String {
    format!("Admin phone paired {stamp}.")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminPhoneView {
    pub paired: bool,
    pub status: String,
    pub paired_at: Option<String>,
}

/// `token` is the minted secret itself, alive only inside this returned value so
/// the desk can offer Copy token beside Copy link. Nothing persists it: pairing
/// still writes `token_hash` and nothing else, and this struct is handed to the
/// front end by commands.rs and stored nowhere. Same secret the link carries.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingView {
    pub device_id: String,
    /// `None` when no scan endpoint is configured. Pairing still succeeds and
    /// `token` below is still the phone's key; there is simply no capture URL
    /// to hand it yet. Never `""` — absence is absence, the shape
    /// `dock_folds::capture_endpoint` already uses for this same fact.
    pub link: Option<String>,
    pub token: String,
    pub pairing_text: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetireView {
    pub line: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceResolution {
    Live(String),
    Retired,
    Unknown,
}

/// SHA-256 hex of the token — the only form the desktop keeps.
pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 32 lowercase hex, the customer-token shape (marketing::sample_token precedent).
fn mint_token(device_id: &str, entropy: &str) -> String {
    let d = Sha256::digest(format!("{device_id}:{entropy}").as_bytes());
    d[..16].iter().map(|b| format!("{b:02x}")).collect()
}

fn live_admin(conn: &Connection) -> Result<Option<(String, String)>, String> {
    conn.query_row(
        "SELECT device_id, paired_at FROM field_devices WHERE role = 'admin' AND retired_at IS NULL LIMIT 1",
        [], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(|e| e.to_string())
}

pub fn has_live_admin(conn: &Connection) -> Result<bool, String> {
    Ok(live_admin(conn)?.is_some())
}

pub fn admin_status(conn: &Connection) -> Result<AdminPhoneView, String> {
    match live_admin(conn)? {
        Some((_, at)) => {
            let stamp = phone::capture_age_label(&at, &db::utc_now_rfc3339())?;
            Ok(AdminPhoneView {
                paired: true,
                status: paired_line(&stamp),
                paired_at: Some(at),
            })
        }
        None => Ok(AdminPhoneView {
            paired: false,
            status: NO_ADMIN_PAIRED.into(),
            paired_at: None,
        }),
    }
}

/// The `?c=<id>:<name>&c=…` suffix of the pairing link and nothing else:
/// crop names only, percent-encoded, in `list_crops` order. No token, no
/// endpoint, no path. Empty when the farm has no crops. Job 4 put this on
/// the port document so the capture page can offer crops without the wire
/// ever carrying the phone's key.
pub fn crop_query(conn: &Connection) -> Result<String, String> {
    let mut q = String::new();
    let mut sep = '?';
    for c in trays::list_crops(conn)? {
        q.push(sep);
        q.push_str("c=");
        q.push_str(&crate::marketing::percent_encode(&c.id));
        q.push(':');
        q.push_str(&crate::marketing::percent_encode(&c.name));
        sep = '&';
    }
    Ok(q)
}

/// `{endpoint}/a/{token}?c=<id>:<name>&c=…` — crop names ride in the link and are
/// never persisted at the endpoint (GT-D21). `None` when no scan endpoint is
/// configured: the endpoint is a worker for standing requests, pack scans and
/// pulls, and pairing does not need one.
pub fn pairing_link(conn: &Connection, token: &str) -> Result<Option<String>, String> {
    let endpoint = scans::config(conn)?
        .endpoint_url
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty());
    let Some(endpoint) = endpoint else {
        return Ok(None);
    };
    let query = crop_query(conn)?;
    Ok(Some(format!("{endpoint}/a/{token}{query}")))
}

/// Pair a new Admin phone. The link is composed BEFORE the transaction so a failed
/// crop read refuses without retiring anyone. A missing scan endpoint is NOT a
/// refusal: `link` is `None` and pairing proceeds — the phone's key is the token,
/// and the Dock (this PC on the LAN) authenticates it with no worker anywhere.
/// Pairing retires the previous live Admin in the same transaction; label is NULL
/// (no label input is signed).
pub fn pair_admin(conn: &mut Connection) -> Result<PairingView, String> {
    let device_id = projection::handler_new_id();
    let token = mint_token(&device_id, &projection::handler_new_id());
    let link = pairing_link(conn, &token)?;
    let now = db::utc_now_rfc3339();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE field_devices SET retired_at = ?1 WHERE role = 'admin' AND retired_at IS NULL",
        [&now],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO field_devices (device_id, label, role, token_hash, paired_at, retired_at)
         VALUES (?1, NULL, 'admin', ?2, ?3, NULL)",
        params![device_id, token_hash(&token), now],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    let status = admin_status(conn)?.status;
    Ok(PairingView {
        device_id,
        link,
        token,
        pairing_text: PAIRING_TEXT.into(),
        status,
    })
}

pub fn retire_admin(conn: &Connection) -> Result<RetireView, String> {
    let now = db::utc_now_rfc3339();
    let n = conn
        .execute(
            "UPDATE field_devices SET retired_at = ?1 WHERE role = 'admin' AND retired_at IS NULL",
            [&now],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(NO_ADMIN_PAIRED.to_string());
    }
    Ok(RetireView {
        line: RETIRED_LINE.into(),
        status: NO_ADMIN_PAIRED.into(),
    })
}

/// The desktop's authority over device identity, applied to every pulled row.
pub fn resolve_token(conn: &Connection, token: &str) -> Result<DeviceResolution, String> {
    let h = token_hash(token);
    let hit: Option<(String, Option<String>)> = conn.query_row(
        "SELECT device_id, retired_at FROM field_devices WHERE token_hash = ?1 AND role = 'admin'",
        [&h], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(|e| e.to_string())?;
    Ok(match hit {
        Some((id, None)) => DeviceResolution::Live(id),
        Some((_, Some(_))) => DeviceResolution::Retired,
        None => DeviceResolution::Unknown,
    })
}
