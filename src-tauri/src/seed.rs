//! Seed in — one register kind, one table, one write door.
//!
//! Authority: GT-D25 (SEED-A). seed.received is the operator's record that
//! seed of one crop arrived at the farm, in ounces: add-only, keyed by the
//! crop id, never netted here. It is the IN side of the jar whose OUT side is
//! the consumption.physical oz row the sow path writes (trays.rs) — that row
//! is untouched by this module, a blank sow weight stays unknown, and an
//! undone sow still leaves its oz-out standing. Capacity-free: moves no tray,
//! books no money (a Seed purchase is dollars on the cost register). Inverse
//! none: a wrong receipt is answered by a later record, never an undo.
//! `apply_seed_received` writes only what the payload froze — the crop id,
//! never a crop name — so replay reads nothing.
//!
//! JAR-READER Job A: `seed_on_hand` is the jar's read side — what is still
//! in it, per crop, computed here and never stored. It joins the sow path's
//! oz-out rows to their crop by id only, opens the window at the crop's
//! first receipt, and prints unknown rather than a number when a sow in the
//! window went unweighed. No Kind, no table, no clock, no event.
use crate::consumption::{UNIT_OZ, UNIT_TRAY};
use crate::events::{EventRecord, Kind};
use crate::projection;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

/// Verify-replay compares every column. All four are written only by
/// apply_seed_received from the seed.received payload + event.created_at.
pub const SEED_RECEIPTS_COLUMNS: &[&str] = &["receipt_id", "crop_id", "received_oz", "created_at"];

/// GT-D25 sentence 1 — the gate runs 1, then 2.
pub const SEED_UNKNOWN_CROP_LINE: &str = "Pick a crop from the crop list before recording seed in.";
/// GT-D25 sentence 2.
pub const SEED_ZERO_LINE: &str = "Seed ounces must be greater than zero.";

/// The seed.received payload. Follows the columns; camelCase on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeedReceivedPayload {
    pub receipt_id: String,
    pub crop_id: String,
    pub received_oz: f64,
}

/// H-10: schema seal, inventoried by seed_a_tests.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub const SEED_RECEIVED_PAYLOAD_FIELD_NAMES: &[&str] = &["receipt_id", "crop_id", "received_oz"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedReceiptView {
    pub receipt_id: String,
    pub crop_id: String,
    /// Read-side join for the confirm line; never in the payload or the row.
    pub crop_name: String,
    pub received_oz: f64,
    pub created_at: String,
}

/// Ounces are stored and compared at 0.1 — the same precision as the
/// WeightPad and the leftover door.
fn round_tenth(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// The write door. Gate order (GT-D25): sentence 1, then 2. Writes one
/// seed.received event and its projection row in one transaction. Touches no
/// tray, no consumption row, no order, no capacity figure.
pub fn receive_seed(
    conn: &mut Connection,
    crop_id: &str,
    received_oz: f64,
) -> Result<SeedReceiptView, String> {
    let known: Option<i64> = conn
        .query_row("SELECT 1 FROM crops WHERE id = ?1", [crop_id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if known.is_none() {
        return Err(SEED_UNKNOWN_CROP_LINE.to_string());
    }
    if !received_oz.is_finite() || round_tenth(received_oz) <= 0.0 {
        return Err(SEED_ZERO_LINE.to_string());
    }
    let received = round_tenth(received_oz);
    let receipt_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let payload = SeedReceivedPayload {
        receipt_id: receipt_id.clone(),
        crop_id: crop_id.to_string(),
        received_oz: received,
    };
    let event = EventRecord::originated(
        Kind::SeedReceived,
        "seed_receipt",
        receipt_id.clone(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    crate::events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_receipt(conn, &receipt_id)
}

/// The choke-point seal (events.rs): shape only. The crop check is the write
/// door's gate, never a read at this choke point.
pub fn validate_seed_event(event: &EventRecord) -> Result<(), String> {
    if event.kind != Kind::SeedReceived {
        return Ok(());
    }
    let p: SeedReceivedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("seed.received payload refused: {e}"))?;
    if p.receipt_id.trim().is_empty() || p.crop_id.trim().is_empty() {
        return Err("seed.received requires receipt_id and crop_id".into());
    }
    if !p.received_oz.is_finite() || p.received_oz <= 0.0 {
        return Err("seed.received received_oz must be > 0".into());
    }
    Ok(())
}

/// Projection. Lookup-free: writes the payload fields and event.created_at.
pub fn apply_seed_received(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_seed_event(event)?;
    let p: SeedReceivedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("seed.received payload refused: {e}"))?;
    tx.execute(
        "INSERT INTO seed_receipts (receipt_id, crop_id, received_oz, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![p.receipt_id, p.crop_id, p.received_oz, event.created_at],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Read side. The crop name is joined at read, never stored.
pub fn receipts(conn: &Connection) -> Result<Vec<SeedReceiptView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.receipt_id, s.crop_id, c.name, s.received_oz, s.created_at
             FROM seed_receipts s
             JOIN crops c ON c.id = s.crop_id
             ORDER BY s.created_at DESC, c.sort_order ASC, s.receipt_id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SeedReceiptView {
                receipt_id: r.get(0)?,
                crop_id: r.get(1)?,
                crop_name: r.get(2)?,
                received_oz: round_tenth(r.get(3)?),
                created_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub(crate) fn get_receipt(conn: &Connection, receipt_id: &str) -> Result<SeedReceiptView, String> {
    receipts(conn)?
        .into_iter()
        .find(|s| s.receipt_id == receipt_id)
        .ok_or_else(|| format!("seed receipt not found: {receipt_id}"))
}

/// JAR-READER Job A. One crop's jar, computed at read and never stored.
/// SCOPE A: per crop — a crop with no receipt has no row. MATH A / A2: every
/// ounce out is placed on its crop by id (consumption_events.sow_event_id →
/// event_log.id, kind tray.sown → payload cropId), never by the oz row's
/// variety_or_item name and never through crop_aliases. OPENING A: the
/// window opens at the crop's first receipt. BLANK A: one unweighed sow in
/// the window and the figure is unknown. NEGATIVE A: short is printed, never
/// clamped. UNDONE A: an undone sow's oz-out row stands and counts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SeedOnHandRow {
    pub crop_id: String,
    /// Read-side join from crops, as in receipts(); never the oz row's name.
    pub crop_name: String,
    /// Σ received_oz over the crop's receipts, at 0.1.
    pub received_oz: f64,
    /// MIN(created_at) over the crop's receipts: the window opens here.
    pub since: String,
    /// Σ oz-out rows dated on or after `since` whose tray.sown names this
    /// crop, undone sows included, at 0.1.
    pub sown_oz: f64,
    /// Sows in the window with a tray row and no oz sibling on the same
    /// sow_event_id: how many, and how many trays they sowed.
    pub unweighed_sows: i64,
    pub unweighed_trays: i64,
    /// The part of sown_oz whose tray.sown carries undone_at. Reported; it
    /// stays inside sown_oz.
    pub undone_sown_oz: f64,
    /// Oz-out of this crop dated before `since`. Reported, not subtracted.
    pub before_first_receipt_oz: f64,
    /// Farm-wide, the same figure on every row: oz-out rows the id join
    /// cannot place on any crop (NULL or dangling sow_event_id, or a
    /// tray.sown without a cropId). Reported, never subtracted, never
    /// resolved by name.
    pub unattributed_oz: f64,
    /// received_oz − sown_oz at 0.1, negative when the jar is short; None
    /// while any sow in the window is unweighed.
    pub on_hand_oz: Option<f64>,
}

/// The jar, per crop, computed at read. Reads no clock, writes no event,
/// flushes nothing. The oz-out rows are the sow path's (trays.rs); this
/// reader joins them to their crop by id only, so a renamed crop keeps every
/// ounce it sowed and a pre-v12 row (NULL sow_event_id) is reported as
/// unattributed rather than guessed from its name.
pub fn seed_on_hand(conn: &Connection) -> Result<Vec<SeedOnHandRow>, String> {
    // 1. The IN side: one row per crop with a receipt, in crop-list order.
    let mut rows = {
        let mut stmt = conn
            .prepare(
                "SELECT s.crop_id, c.name, SUM(s.received_oz), MIN(s.created_at)
                 FROM seed_receipts s
                 JOIN crops c ON c.id = s.crop_id
                 GROUP BY s.crop_id
                 ORDER BY c.sort_order ASC, s.crop_id ASC",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| {
                Ok(SeedOnHandRow {
                    crop_id: r.get(0)?,
                    crop_name: r.get(1)?,
                    received_oz: round_tenth(r.get(2)?),
                    since: r.get(3)?,
                    sown_oz: 0.0,
                    unweighed_sows: 0,
                    unweighed_trays: 0,
                    undone_sown_oz: 0.0,
                    before_first_receipt_oz: 0.0,
                    unattributed_oz: 0.0,
                    on_hand_oz: None,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in mapped {
            out.push(r.map_err(|e| e.to_string())?);
        }
        out
    };
    let index: HashMap<String, usize> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (r.crop_id.clone(), i))
        .collect();
    // 2. The OUT side, by id. Raw sums here; each figure is rounded once.
    let mut sown = vec![0.0_f64; rows.len()];
    let mut undone = vec![0.0_f64; rows.len()];
    let mut before = vec![0.0_f64; rows.len()];
    let mut unattributed = 0.0_f64;
    {
        let mut stmt = conn
            .prepare(
                "SELECT json_extract(e.payload, '$.cropId'), c.occurred_at, c.quantity,
                        e.undone_at
                 FROM consumption_events c
                 LEFT JOIN event_log e ON e.id = c.sow_event_id AND e.kind = 'tray.sown'
                 WHERE c.origin = 'farm_os' AND c.unit = ?1",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(params![UNIT_OZ], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for r in mapped {
            let (crop_id, occurred_at, oz, undone_at) = r.map_err(|e| e.to_string())?;
            // No tray.sown by id, or one without a crop: nobody's jar (MATH A2).
            let Some(crop_id) = crop_id else {
                unattributed += oz;
                continue;
            };
            // A crop with no receipt has no jar to report on (SCOPE A).
            let Some(&i) = index.get(&crop_id) else {
                continue;
            };
            if occurred_at < rows[i].since {
                before[i] += oz;
            } else {
                sown[i] += oz;
                if undone_at.is_some() {
                    undone[i] += oz;
                }
            }
        }
    }
    // 3. BLANK A: a sow in the window whose oz sibling never came.
    {
        let mut stmt = conn
            .prepare(
                "SELECT json_extract(e.payload, '$.cropId'), c.occurred_at, c.quantity
                 FROM consumption_events c
                 JOIN event_log e ON e.id = c.sow_event_id AND e.kind = 'tray.sown'
                 WHERE c.origin = 'farm_os' AND c.unit = ?1
                   AND NOT EXISTS (
                     SELECT 1 FROM consumption_events o
                     WHERE o.sow_event_id = c.sow_event_id
                       AND o.origin = 'farm_os' AND o.unit = ?2)",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(params![UNIT_TRAY, UNIT_OZ], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for r in mapped {
            let (crop_id, occurred_at, trays) = r.map_err(|e| e.to_string())?;
            let Some(&i) = crop_id.as_deref().and_then(|id| index.get(id)) else {
                continue;
            };
            if occurred_at >= rows[i].since {
                rows[i].unweighed_sows += 1;
                rows[i].unweighed_trays += trays.round() as i64;
            }
        }
    }
    // 4. Fold, at 0.1. The figure is unknown while a sow in the window is
    // unweighed; otherwise received − sown, negative when short.
    let unattributed = round_tenth(unattributed);
    for (i, row) in rows.iter_mut().enumerate() {
        row.sown_oz = round_tenth(sown[i]);
        row.undone_sown_oz = round_tenth(undone[i]);
        row.before_first_receipt_oz = round_tenth(before[i]);
        row.unattributed_oz = unattributed;
        row.on_hand_oz = if row.unweighed_sows > 0 {
            None
        } else {
            Some(round_tenth(row.received_oz - row.sown_oz))
        };
    }
    Ok(rows)
}
