//! Rack-side / phone capture — Fence 1 (GT-D20). The proposal ledger and the
//! Confirm gate: the only door through which a phone-originated physical fact
//! becomes farm truth. `phone.proposed` is a candidate and never changes trays;
//! on Confirm each accepted proposal is cross-referenced against live PC facts
//! and, when clean, applied through the EXISTING tray write paths on the phone
//! capture day (ruling 3); `phone.proposal_decided` carries the applied event ids
//! (provenance by join, ruling 2). Bytes signed 2026-08-17 (rack-side fence 1
//! addendum). No relay, no phone UI, no transport lives here.

use crate::attention;
use crate::db;
use crate::events::{EventRecord, Kind};
use crate::models::HarvestInput;
use crate::projection;
use crate::reachability::{format_mon_d_local, tray_word};
use crate::trays;
use chrono::{DateTime, Local};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const VERB_MOVE_TO_LIGHT: &str = "move_to_light";
pub const VERB_HARVEST: &str = "harvest";
pub const VERBS: [&str; 2] = [VERB_MOVE_TO_LIGHT, VERB_HARVEST];

/// Attention kind for an undecided proposal. Reality surface only (surfaces.ts
/// REALITY set); never a Today card (ruling 6).
pub const PHONE_PROPOSAL_ATTENTION_KIND: &str = "phone.proposal";
/// Listed actions on the attention row; both resolve only through the decide
/// path (confirm_phone_captures / discard_phone_capture).
pub const PHONE_PROPOSAL_ATTENTION_ACTIONS: [&str; 2] = ["confirm", "discard"];

/// Verify-replay compares every column (compare_keyed by proposal_id).
pub const PHONE_PROPOSALS_COLUMNS: &[&str] = &[
    "proposal_id",
    "device_id",
    "verb",
    "crop_id",
    "quantity",
    "actual_yield_oz",
    "phone_captured_at",
    "note",
    "created_at",
    "decided_at",
    "outcome",
    "accepted_quantity",
    "accepted_yield_oz",
    "applied_event_ids",
    "gate_reason",
];
#[allow(dead_code)] // H-10: schema seal, held by phone_tests::pf1_kinds_closed_set_grow_not_farm_truth_table_compared_payloads_sealed.
pub const PHONE_PROPOSED_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "proposal_id",
    "device_id",
    "verb",
    "crop_id",
    "quantity",
    "actual_yield_oz",
    "phone_captured_at",
    "note",
];
#[allow(dead_code)] // H-10: schema seal, held by phone_tests::pf1_kinds_closed_set_grow_not_farm_truth_table_compared_payloads_sealed.
pub const PHONE_PROPOSAL_DECIDED_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "proposal_id",
    "outcome",
    "decided_at",
    "accepted_quantity",
    "accepted_yield_oz",
    "applied_event_ids",
    "gate_reason",
];
/// The closed reason set (rulings 3 + 5 + batch fit). Mirrored by the
/// phone_proposals.gate_reason CHECK.
pub const GATE_REASONS: [&str; 8] = [
    "not_enough_blackout",
    "not_enough_light",
    "weight_not_positive",
    "same_day_harvest_exists",
    "capture_in_future",
    "capture_before_sow",
    "capture_before_light",
    "batch_mismatch",
];

// ---- Signed bytes (rack-side fence 1 addendum, 2026-08-17). ONE set each. ----
pub const PHONE_CAPTURE_DECIDE_ONLY: &str =
    "A phone capture is decided with Confirm or Discard on the capture itself.";
pub const PHONE_CAPTURE_ALREADY_DECIDED: &str = "This phone capture has already been decided.";
pub const WEIGHT_NOT_POSITIVE_SENTENCE: &str =
    "Harvest weight must be more than 0 oz. Nothing written.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneProposedPayload {
    pub proposal_id: String,
    pub device_id: String,
    pub verb: String,
    pub crop_id: String,
    pub quantity: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_yield_oz: Option<f64>,
    pub phone_captured_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneProposalDecidedPayload {
    pub proposal_id: String,
    pub outcome: String, // "accepted" | "discarded"
    pub decided_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_quantity: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_yield_oz: Option<f64>,
    pub applied_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_reason: Option<String>,
}

fn rfc3339_ok(s: &str) -> bool {
    DateTime::parse_from_rfc3339(s).is_ok()
}

fn validate_proposed(p: &PhoneProposedPayload) -> Result<(), String> {
    if p.proposal_id.trim().is_empty() {
        return Err("proposal_id must be non-empty".into());
    }
    if p.device_id.trim().is_empty() {
        return Err("device_id must be non-empty".into());
    }
    if p.crop_id.trim().is_empty() {
        return Err("crop_id must be non-empty".into());
    }
    if !VERBS.contains(&p.verb.as_str()) {
        return Err(format!(
            "verb must be move_to_light or harvest, got {}",
            p.verb
        ));
    }
    if p.quantity < 1 {
        return Err("quantity must be at least 1".into());
    }
    if !rfc3339_ok(&p.phone_captured_at) {
        return Err("phone_captured_at must be RFC3339".into());
    }
    match (p.verb.as_str(), p.actual_yield_oz) {
        (VERB_HARVEST, Some(oz)) if oz.is_finite() && oz > 0.0 => Ok(()),
        (VERB_HARVEST, _) => Err("harvest requires actual_yield_oz > 0".into()),
        (_, None) => Ok(()),
        (_, Some(_)) => Err("actual_yield_oz is only for harvest".into()),
    }
}

/// Choke-point seal for both kinds (called from events::write_event_inner and
/// from both apply_* fns, the marketing precedent).
pub fn validate_phone_event(event: &EventRecord) -> Result<(), String> {
    match event.kind {
        Kind::PhoneProposed => {
            let p: PhoneProposedPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("phone.proposed payload refused: {e}"))?;
            validate_proposed(&p)
        }
        Kind::PhoneProposalDecided => {
            let p: PhoneProposalDecidedPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("phone.proposal_decided payload refused: {e}"))?;
            if p.proposal_id.trim().is_empty() {
                return Err("proposal_id must be non-empty".into());
            }
            if !rfc3339_ok(&p.decided_at) {
                return Err("decided_at must be RFC3339".into());
            }
            match p.outcome.as_str() {
                "accepted" => {
                    if p.accepted_quantity.is_none_or(|q| q < 1) {
                        return Err("accepted requires accepted_quantity >= 1".into());
                    }
                    if let Some(oz) = p.accepted_yield_oz {
                        if !(oz.is_finite() && oz > 0.0) {
                            return Err("accepted_yield_oz must be > 0".into());
                        }
                    }
                    if p.applied_event_ids.is_empty() {
                        return Err("accepted requires applied_event_ids".into());
                    }
                    if p.gate_reason.is_some() {
                        return Err("accepted carries no gate_reason".into());
                    }
                }
                "discarded" => {
                    if !p.applied_event_ids.is_empty() {
                        return Err("discarded applies nothing".into());
                    }
                    if let Some(r) = &p.gate_reason {
                        if !GATE_REASONS.contains(&r.as_str()) {
                            return Err(format!("unknown gate_reason {r}"));
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "outcome must be accepted or discarded, got {other}"
                    ))
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// GT-D20. Insert-only, lookup-free; PK conflict fails replay loudly (the
/// standing.requested precedent). Decided columns stay NULL here.
pub fn apply_phone_proposed(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_phone_event(event)?;
    let p: PhoneProposedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("phone.proposed payload refused: {e}"))?;
    tx.execute(
        "INSERT INTO phone_proposals
         (proposal_id, device_id, verb, crop_id, quantity, actual_yield_oz, phone_captured_at, note,
          created_at, decided_at, outcome, accepted_quantity, accepted_yield_oz, applied_event_ids, gate_reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL, NULL, NULL, NULL, NULL)",
        params![p.proposal_id, p.device_id, p.verb, p.crop_id, p.quantity, p.actual_yield_oz,
                p.phone_captured_at, p.note, event.created_at],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// GT-D20. Writes the decision onto the open row only; 0 rows = missing or
/// already decided, which fails replay loudly (the standing.request_decided precedent).
pub fn apply_phone_proposal_decided(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    validate_phone_event(event)?;
    let p: PhoneProposalDecidedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("phone.proposal_decided payload refused: {e}"))?;
    let ids_json = serde_json::to_string(&p.applied_event_ids).map_err(|e| e.to_string())?;
    let n = tx
        .execute(
            "UPDATE phone_proposals
         SET decided_at = ?1, outcome = ?2, accepted_quantity = ?3, accepted_yield_oz = ?4,
             applied_event_ids = ?5, gate_reason = ?6
         WHERE proposal_id = ?7 AND decided_at IS NULL",
            params![
                p.decided_at,
                p.outcome,
                p.accepted_quantity,
                p.accepted_yield_oz,
                ids_json,
                p.gate_reason,
                p.proposal_id
            ],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!(
            "phone.proposal_decided: proposal not open: {}",
            p.proposal_id
        ));
    }
    Ok(())
}

// ---- rows ----
#[derive(Debug, Clone)]
pub struct ProposalRow {
    pub proposal_id: String,
    pub device_id: String,
    pub verb: String,
    pub crop_id: String,
    pub quantity: i64,
    pub actual_yield_oz: Option<f64>,
    pub phone_captured_at: String,
    pub note: Option<String>,
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub created_at: String,
    pub decided_at: Option<String>,
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub outcome: Option<String>,
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub gate_reason: Option<String>,
}
const ROW_SELECT: &str = "SELECT proposal_id, device_id, verb, crop_id, quantity, actual_yield_oz,
    phone_captured_at, note, created_at, decided_at, outcome, gate_reason FROM phone_proposals";
fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProposalRow> {
    Ok(ProposalRow {
        proposal_id: r.get(0)?,
        device_id: r.get(1)?,
        verb: r.get(2)?,
        crop_id: r.get(3)?,
        quantity: r.get(4)?,
        actual_yield_oz: r.get(5)?,
        phone_captured_at: r.get(6)?,
        note: r.get(7)?,
        created_at: r.get(8)?,
        decided_at: r.get(9)?,
        outcome: r.get(10)?,
        gate_reason: r.get(11)?,
    })
}
pub fn get_proposal(conn: &Connection, proposal_id: &str) -> Result<ProposalRow, String> {
    conn.query_row(
        &format!("{ROW_SELECT} WHERE proposal_id = ?1"),
        [proposal_id],
        row_from,
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("unknown phone capture: {proposal_id}"))
}
pub fn open_proposals(conn: &Connection) -> Result<Vec<ProposalRow>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "{ROW_SELECT} WHERE decided_at IS NULL ORDER BY created_at, proposal_id"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], row_from).map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}
fn crop_name(conn: &Connection, crop_id: &str) -> Result<String, String> {
    conn.query_row("SELECT name FROM crops WHERE id = ?1", [crop_id], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .map_err(|e| e.to_string())
    .map(|n| n.unwrap_or_else(|| crop_id.to_string()))
}
fn write_pair(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    projection::apply_event(tx, event)?;
    crate::events::write_event(tx, event)?;
    Ok(())
}

// ---- ingest (standing_pull outcome shape) ----
#[derive(Debug, Clone)]
pub struct PhoneProposalInput {
    pub proposal_id: String,
    pub device_id: String,
    pub verb: String,
    pub crop_id: String,
    pub quantity: i64,
    pub actual_yield_oz: Option<f64>,
    pub phone_captured_at: String,
    pub note: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    Written,
    Known,
    Refused(&'static str),
} // "invalid" | "unknown_crop"

/// The ONE way a proposal enters the farm. Known by proposal_id (a re-delivery
/// cannot mint a second one); malformed or unknown-crop rows are refused and
/// nothing is invented. In Fence 1 the only caller is the debug seed.
pub fn ingest_phone_proposal(
    conn: &mut Connection,
    input: &PhoneProposalInput,
) -> Result<IngestOutcome, String> {
    if input.proposal_id.trim().is_empty() {
        return Ok(IngestOutcome::Refused("invalid"));
    }
    let known: bool = conn
        .query_row(
            "SELECT 1 FROM phone_proposals WHERE proposal_id = ?1",
            [&input.proposal_id],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    if known {
        return Ok(IngestOutcome::Known);
    }
    let payload = PhoneProposedPayload {
        proposal_id: input.proposal_id.clone(),
        device_id: input.device_id.clone(),
        verb: input.verb.clone(),
        crop_id: input.crop_id.clone(),
        quantity: input.quantity,
        actual_yield_oz: input.actual_yield_oz,
        phone_captured_at: input.phone_captured_at.clone(),
        note: input
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string),
    };
    if validate_proposed(&payload).is_err() {
        return Ok(IngestOutcome::Refused("invalid"));
    }
    let crop_known: bool = conn
        .query_row(
            "SELECT 1 FROM crops WHERE id = ?1",
            [&payload.crop_id],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    if !crop_known {
        return Ok(IngestOutcome::Refused("unknown_crop"));
    }
    let event = EventRecord::originated(
        Kind::PhoneProposed,
        "phone_proposal",
        payload.proposal_id.clone(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        projection::handler_now(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(IngestOutcome::Written)
}

// ---- signed byte composers ----
/// Capture age: "today at 6:12 pm" / "yesterday at 6:12 pm" / "Fri Aug 14 at 6:12 pm".
pub fn capture_age_label(phone_captured_at: &str, now_utc: &str) -> Result<String, String> {
    let captured = DateTime::parse_from_rfc3339(phone_captured_at)
        .map_err(|_| "phone_captured_at must be RFC3339".to_string())?
        .with_timezone(&Local);
    let now = DateTime::parse_from_rfc3339(now_utc)
        .map_err(|_| "now must be RFC3339".to_string())?
        .with_timezone(&Local);
    let time = attention::format_clock(captured);
    match (now.date_naive() - captured.date_naive()).num_days() {
        0 => Ok(format!("today at {time}")),
        1 => Ok(format!("yesterday at {time}")),
        _ => Ok(format!(
            "{} at {time}",
            format_mon_d_local(&captured.format("%Y-%m-%d").to_string())?
        )),
    }
}
/// Twin of mass.ts massFigure.
fn mass_figure(oz: f64, units_system: &str) -> String {
    if units_system == "metric" {
        crate::units::grams(oz).to_string()
    } else {
        format!("{oz:.1}")
    }
}
/// Pending-capture card. Also the attention row's message.
pub fn capture_message(
    verb: &str,
    trays: i64,
    crop_name: &str,
    oz: Option<f64>,
    age: &str,
    note: Option<&str>,
    units_system: &str,
) -> String {
    let t = tray_word(trays);
    let mut m = if verb == VERB_HARVEST {
        format!(
            "Harvest {t} of {crop_name}, {figure} {unit} — captured {age}.",
            figure = mass_figure(oz.unwrap_or(0.0), units_system),
            unit = crate::units::unit_for(units_system)
        )
    } else {
        format!("Move {t} of {crop_name} to light — captured {age}.")
    };
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        m.push_str(&format!(" Note: {n}."));
    }
    m
}
/// Confirmation line, one per written row.
pub fn confirmation_line(
    verb: &str,
    trays: i64,
    crop_name: &str,
    oz: Option<f64>,
    age: &str,
    units_system: &str,
) -> String {
    let t = tray_word(trays);
    if verb == VERB_HARVEST {
        format!(
            "Recorded from phone capture — {t} of {crop_name}, {figure} {unit} (captured {age}).",
            figure = mass_figure(oz.unwrap_or(0.0), units_system),
            unit = crate::units::unit_for(units_system)
        )
    } else {
        format!("Recorded from phone capture — {t} of {crop_name} to light (captured {age}).")
    }
}

// ---- the gate ----
#[derive(Debug, Clone)]
struct BatchRow {
    id: String,
    quantity: i64,
    sown_on: Option<String>,
    light_on: Option<String>,
}
/// The oldest-first rule Today uses (attention::tray_ids_for_crop: sown_on ASC,
/// id ASC), carrying the batch quantities the fit needs.
fn batches_for_crop_in_state(
    conn: &Connection,
    crop_id: &str,
    state: &str,
) -> Result<Vec<BatchRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, quantity, sown_on, light_on FROM trays
         WHERE crop_id = ?1 AND state = ?2 ORDER BY sown_on ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![crop_id, state], |r| {
            Ok(BatchRow {
                id: r.get(0)?,
                quantity: r.get(1)?,
                sown_on: r.get(2)?,
                light_on: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}
#[derive(Debug, Clone)]
pub struct GateBlock {
    pub reason: &'static str,
    pub sentence: String,
}
#[derive(Debug, Clone)]
pub enum GateVerdict {
    Clean {
        tray_ids: Vec<String>,
        capture_day: String,
    },
    Blocked(GateBlock),
}
fn blocked(reason: &'static str, sentence: String) -> GateVerdict {
    GateVerdict::Blocked(GateBlock { reason, sentence })
}

/// Cross-reference against live PC facts. Deterministic order; the first failing
/// check is the one shown: capture_in_future → (harvest) weight_not_positive →
/// not_enough_{blackout|light} → (harvest) same_day_harvest_exists →
/// batch_mismatch → capture_before_{sow|light}. Reads only.
pub fn gate(
    conn: &Connection,
    row: &ProposalRow,
    quantity: i64,
    actual_yield_oz: Option<f64>,
    crop_name: &str,
    today: &str,
) -> Result<GateVerdict, String> {
    let capture_day = db::local_date_from_utc_rfc3339(&row.phone_captured_at)?;
    let cap = format_mon_d_local(&capture_day)?;
    if capture_day.as_str() > today {
        return Ok(blocked(
            "capture_in_future",
            format!("This capture is dated {cap}, after today. Nothing written."),
        ));
    }
    let harvest = row.verb == VERB_HARVEST;
    if harvest && !actual_yield_oz.is_some_and(|oz| oz.is_finite() && oz > 0.0) {
        return Ok(blocked(
            "weight_not_positive",
            WEIGHT_NOT_POSITIVE_SENTENCE.to_string(),
        ));
    }
    let (state, place, act) = if harvest {
        ("light", "in light", "harvest")
    } else {
        ("blackout", "under cover", "move")
    };
    let batches = batches_for_crop_in_state(conn, &row.crop_id, state)?;
    let available: i64 = batches.iter().map(|b| b.quantity).sum();
    if available < quantity {
        let reason = if harvest {
            "not_enough_light"
        } else {
            "not_enough_blackout"
        };
        let sentence = if available == 0 {
            format!("No trays of {crop_name} are {place} on the PC; this capture names {quantity}. Nothing written.")
        } else {
            format!("Only {} of {crop_name} are {place} on the PC; this capture names {quantity}. Nothing written.", tray_word(available))
        };
        return Ok(blocked(reason, sentence));
    }
    if harvest {
        let dup: i64 = conn.query_row(
            "SELECT COUNT(*) FROM trays WHERE crop_id = ?1 AND state = 'harvested' AND harvested_on = ?2",
            params![row.crop_id, capture_day], |r| r.get(0)).map_err(|e| e.to_string())?;
        if dup > 0 {
            return Ok(blocked("same_day_harvest_exists",
                format!("The PC already recorded a harvest of {crop_name} on {cap} — this capture may be a duplicate. Nothing written.")));
        }
    }
    // Batch fit (signed option 1): whole rows, oldest first, running total must equal N exactly.
    let mut chosen: Vec<&BatchRow> = Vec::new();
    let mut running = 0i64;
    for b in &batches {
        if running >= quantity {
            break;
        }
        running += b.quantity;
        chosen.push(b);
    }
    if running != quantity {
        let sizes: Vec<String> = batches.iter().map(|b| b.quantity.to_string()).collect();
        let sentence = if sizes.len() == 1 {
            format!("{crop_name} is {place} on the PC in one batch of {}; this capture names {quantity}, and batches {act} whole. Nothing written.", sizes[0])
        } else {
            format!("{crop_name} is {place} on the PC in batches of {}; this capture names {quantity}, and batches {act} whole. Nothing written.", sizes.join(" then "))
        };
        return Ok(blocked("batch_mismatch", sentence));
    }
    for b in &chosen {
        let (col, reason, phrase) = if harvest {
            (&b.light_on, "capture_before_light", "went into light")
        } else {
            (&b.sown_on, "capture_before_sow", "were sown")
        };
        if let Some(d) = col {
            if capture_day.as_str() < d.as_str() {
                return Ok(blocked(reason,
                    format!("This capture is dated {cap}, before those trays of {crop_name} {phrase} on the PC. Nothing written.")));
            }
        }
    }
    Ok(GateVerdict::Clean {
        tray_ids: chosen.iter().map(|b| b.id.clone()).collect(),
        capture_day,
    })
}

// ---- decide paths ----
fn decided_event(
    proposal_id: &str,
    outcome: &str,
    decided_at: &str,
    accepted_quantity: Option<i64>,
    accepted_yield_oz: Option<f64>,
    applied_event_ids: Vec<String>,
    gate_reason: Option<String>,
) -> EventRecord {
    let payload = PhoneProposalDecidedPayload {
        proposal_id: proposal_id.to_string(),
        outcome: outcome.to_string(),
        decided_at: decided_at.to_string(),
        accepted_quantity,
        accepted_yield_oz,
        applied_event_ids,
        gate_reason,
    };
    EventRecord::originated(
        Kind::PhoneProposalDecided,
        "phone_proposal",
        proposal_id.to_string(),
        serde_json::to_value(&payload).expect("decided payload serializes"),
        json!({ "op": "none" }),
        decided_at.to_string(),
        None,
        None,
        Some(projection::handler_new_id()),
    )
}
fn close_attention_in_tx(
    tx: &Transaction<'_>,
    proposal_id: &str,
    action: &str,
    created_at: &str,
) -> Result<(), String> {
    let open: Option<String> = tx.query_row(
        "SELECT id FROM attention WHERE kind = ?1 AND entity_id = ?2 AND resolved_at IS NULL LIMIT 1",
        params![PHONE_PROPOSAL_ATTENTION_KIND, proposal_id], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    if let Some(id) = open {
        attention::resolve_open_in_tx(tx, &id, action, created_at)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedCapture {
    pub proposal_id: String,
    pub quantity: i64,
    #[serde(default)]
    pub actual_yield_oz: Option<f64>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WrittenCapture {
    pub proposal_id: String,
    pub line: String,
    pub applied_event_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedCapture {
    pub proposal_id: String,
    pub reason: String,
    pub sentence: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmResult {
    pub written: Vec<WrittenCapture>,
    pub blocked: Vec<BlockedCapture>,
}

/// Confirm (rulings 4-5). One transaction: rows are gated in the order given,
/// against live facts INCLUDING what earlier rows in this Confirm wrote; a clean
/// row is applied on its capture day through the existing tray write path and
/// its decision is written beside it; a blocked row is not written and comes
/// back with its reason. PC facts win.
pub fn confirm_phone_captures(
    conn: &mut Connection,
    accepted: &[AcceptedCapture],
) -> Result<ConfirmResult, String> {
    let now = projection::handler_now();
    let today = db::local_date_from_utc_rfc3339(&now)?;
    let mut written = Vec::new();
    let mut blocked_rows = Vec::new();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let units_system = crate::units::farm_units(&tx)?;
    for a in accepted {
        let row = get_proposal(&tx, &a.proposal_id)?;
        if row.decided_at.is_some() {
            return Err(PHONE_CAPTURE_ALREADY_DECIDED.to_string());
        }
        if a.quantity < 1 {
            return Err("quantity must be at least 1".into());
        }
        let name = crop_name(&tx, &row.crop_id)?;
        let oz = if row.verb == VERB_HARVEST {
            a.actual_yield_oz
        } else {
            None
        };
        match gate(&tx, &row, a.quantity, oz, &name, &today)? {
            GateVerdict::Blocked(b) => blocked_rows.push(BlockedCapture {
                proposal_id: row.proposal_id.clone(),
                reason: b.reason.to_string(),
                sentence: b.sentence,
            }),
            GateVerdict::Clean {
                tray_ids,
                capture_day,
            } => {
                let applied_id = if row.verb == VERB_HARVEST {
                    trays::harvest_groups_in_tx(
                        &tx,
                        &[HarvestInput {
                            tray_ids,
                            actual_yield_oz: oz.unwrap_or(0.0),
                        }],
                        &capture_day,
                        &row.phone_captured_at,
                        &now,
                    )?
                } else {
                    trays::advance_trays_in_tx(&tx, &tray_ids, &capture_day, &now)?
                };
                let decided = decided_event(
                    &row.proposal_id,
                    "accepted",
                    &now,
                    Some(a.quantity),
                    oz,
                    vec![applied_id.clone()],
                    None,
                );
                close_attention_in_tx(&tx, &row.proposal_id, "confirm", &now)?;
                write_pair(&tx, &decided)?;
                let age = capture_age_label(&row.phone_captured_at, &now)?;
                written.push(WrittenCapture {
                    proposal_id: row.proposal_id.clone(),
                    line: confirmation_line(&row.verb, a.quantity, &name, oz, &age, &units_system),
                    applied_event_ids: vec![applied_id],
                });
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ConfirmResult {
        written,
        blocked: blocked_rows,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneCaptureDecision {
    pub proposal_id: String,
    pub outcome: String,
    pub decided_at: String,
    pub gate_reason: Option<String>,
}

/// Discard (ruling 5). The gate reason is computed here from live facts against
/// the proposal as captured — never taken from the client; None when the row
/// would have passed.
pub fn discard_phone_capture(
    conn: &mut Connection,
    proposal_id: &str,
) -> Result<PhoneCaptureDecision, String> {
    let row = get_proposal(conn, proposal_id)?;
    if row.decided_at.is_some() {
        return Err(PHONE_CAPTURE_ALREADY_DECIDED.to_string());
    }
    let now = projection::handler_now();
    let today = db::local_date_from_utc_rfc3339(&now)?;
    let name = crop_name(conn, &row.crop_id)?;
    let reason = match gate(conn, &row, row.quantity, row.actual_yield_oz, &name, &today)? {
        GateVerdict::Blocked(b) => Some(b.reason.to_string()),
        GateVerdict::Clean { .. } => None,
    };
    let decided = decided_event(
        proposal_id,
        "discarded",
        &now,
        None,
        None,
        Vec::new(),
        reason.clone(),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    close_attention_in_tx(&tx, proposal_id, "discard", &now)?;
    write_pair(&tx, &decided)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(PhoneCaptureDecision {
        proposal_id: proposal_id.to_string(),
        outcome: "discarded".into(),
        decided_at: now,
        gate_reason: reason,
    })
}

// ---- attention (structural) ----
/// Every undecided proposal is a phone.proposal row on Reality, keyed per
/// proposal_id; raise is idempotent while open; the only closers are the decide
/// paths. After a restore the attention table is empty and undecided proposals
/// re-raise from phone_proposals — the durable home. The age is part of the
/// sentence, so an open row's message is refreshed (raise_or_refresh precedent).
pub fn raise_phone_proposals(conn: &Connection) -> Result<(), String> {
    let now = db::utc_now_rfc3339();
    let units_system = crate::units::farm_units(conn)?;
    for row in open_proposals(conn)? {
        let name = crop_name(conn, &row.crop_id)?;
        let age = capture_age_label(&row.phone_captured_at, &now)?;
        let message = capture_message(
            &row.verb,
            row.quantity,
            &name,
            row.actual_yield_oz,
            &age,
            row.note.as_deref(),
            &units_system,
        );
        attention::raise(
            conn,
            PHONE_PROPOSAL_ATTENTION_KIND,
            Some("phone_proposal"),
            Some(&row.proposal_id),
            &message,
            &PHONE_PROPOSAL_ATTENTION_ACTIONS,
        )?;
        conn.execute(
            "UPDATE attention SET message = ?1
                      WHERE kind = ?2 AND entity_id = ?3 AND resolved_at IS NULL AND message <> ?1",
            params![message, PHONE_PROPOSAL_ATTENTION_KIND, row.proposal_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---- view ----
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneCaptureView {
    pub proposal_id: String,
    pub device_id: String,
    pub verb: String,
    pub crop_id: String,
    pub crop_name: String,
    pub quantity: i64,
    pub actual_yield_oz: Option<f64>,
    pub phone_captured_at: String,
    pub captured_label: String,
    pub note: Option<String>,
    pub message: String,
}
pub fn phone_captures(conn: &Connection) -> Result<Vec<PhoneCaptureView>, String> {
    let now = db::utc_now_rfc3339();
    let units_system = crate::units::farm_units(conn)?;
    open_proposals(conn)?
        .into_iter()
        .map(|row| {
            let name = crop_name(conn, &row.crop_id)?;
            let age = capture_age_label(&row.phone_captured_at, &now)?;
            let message = capture_message(
                &row.verb,
                row.quantity,
                &name,
                row.actual_yield_oz,
                &age,
                row.note.as_deref(),
                &units_system,
            );
            Ok(PhoneCaptureView {
                proposal_id: row.proposal_id,
                device_id: row.device_id,
                verb: row.verb,
                crop_id: row.crop_id,
                crop_name: name,
                quantity: row.quantity,
                actual_yield_oz: row.actual_yield_oz,
                phone_captured_at: row.phone_captured_at,
                captured_label: age,
                note: row.note,
                message,
            })
        })
        .collect()
}

/// Ruling 7. Debug builds only (the dev_seed_standing_request precedent):
/// writes a real phone.proposed through the normal ingest so the gate can be
/// keyboard-proven before any relay exists. The ledger states its origin:
/// proposal_id "dev-…", device_id "dev-seed". captured_days_ago may be negative
/// (a future capture) so the capture_in_future refusal is provable.
#[cfg(debug_assertions)]
pub fn dev_seed_phone_proposal(
    conn: &mut Connection,
    verb: &str,
    crop_id: &str,
    quantity: i64,
    actual_yield_oz: Option<f64>,
    captured_days_ago: i64,
    note: Option<String>,
) -> Result<PhoneCaptureView, String> {
    let now =
        DateTime::parse_from_rfc3339(&projection::handler_now()).map_err(|e| e.to_string())?;
    let captured = (now - chrono::Duration::days(captured_days_ago))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let proposal_id = format!("dev-{}", projection::handler_new_id());
    let input = PhoneProposalInput {
        proposal_id: proposal_id.clone(),
        device_id: "dev-seed".into(),
        verb: verb.into(),
        crop_id: crop_id.into(),
        quantity,
        actual_yield_oz,
        phone_captured_at: captured,
        note,
    };
    match ingest_phone_proposal(conn, &input)? {
        IngestOutcome::Written => {}
        IngestOutcome::Known => return Err("dev seed: proposal id collision".into()),
        IngestOutcome::Refused(reason) => return Err(format!("dev seed refused: {reason}")),
    }
    raise_phone_proposals(conn)?;
    phone_captures(conn)?
        .into_iter()
        .find(|v| v.proposal_id == proposal_id)
        .ok_or_else(|| "dev seed: proposal not found after ingest".to_string())
}
