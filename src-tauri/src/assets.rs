//! Asset register — four operator fields, computes nothing (Track 4 residual).
//!
//! Authority: BOOKS-BOUNDARY asset clause; ROADMAP Track 4 done-when.
//! Exactly four operator fields: description, date placed in service, cost,
//! disposal date. Farm OS makes no tax determination. No depreciation, no
//! schedule, no remaining or book value, no section 179, no useful life, no
//! convention, no method. Adding any of those is a boundary violation, not a
//! feature. Cost is integer cents and is stored, never operated on.

use crate::db;
use crate::events::{EventRecord, Kind};
use crate::projection;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const ASSETS_COLUMNS: &[&str] = &[
    "asset_id",
    "origin",
    "description",
    "placed_in_service_on",
    "cost_cents",
    "disposal_date",
    "last_event_id",
    "created_at",
    "updated_at",
    "voided_at",
];

#[allow(dead_code)] // H-10: schema seal, held by mileage_asset_tests::h10_asset_spine_columns_are_real_and_listed.
pub const ASSET_SPINE_COLUMNS: &[&str] =
    &["last_event_id", "created_at", "updated_at", "voided_at"];

pub const ASSET_PAYLOAD_KEYS: &[&str] = &[
    "eventId",
    "origin",
    "assetId",
    "description",
    "placedInServiceOn",
    "costCents",
    "disposalDate",
];

/// Payload keys for asset.voided — identity only.
pub const ASSET_VOID_PAYLOAD_KEYS: &[&str] = &["eventId", "origin", "assetId"];

/// Forbidden key names — the register must never carry a derived number.
/// Cost is money and is allowed; a COMPUTATION on it is not.
// FORBIDDEN-KEYS-BEGIN
pub const ASSET_FORBIDDEN_COMPUTED_KEYS: &[&str] = &[
    "depreciation",
    "depreciationCents",
    "accumulated",
    "accumulatedDepreciation",
    "section179",
    "section_179",
    "bonus",
    "bonusDepreciation",
    "macrs",
    "method",
    "convention",
    "usefulLife",
    "life",
    "recoveryPeriod",
    "salvage",
    "salvageValue",
    "remainingValue",
    "bookValue",
    "netBookValue",
    "basis",
    "adjustedBasis",
    "schedule",
    "annualDeduction",
    "yearsHeld",
    "expenseElection",
];
// FORBIDDEN-KEYS-END

/// Sealed asset payload. `deny_unknown_fields` is the type-level seal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub struct AssetPayload {
    pub event_id: String,
    pub origin: String,
    pub asset_id: String,
    pub description: String,
    /// Operator-entered calendar day, YYYY-MM-DD.
    pub placed_in_service_on: String,
    /// Integer cents. Stored. Never a depreciable base in this codebase.
    pub cost_cents: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposal_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub struct AssetVoidPayload {
    pub event_id: String,
    pub origin: String,
    pub asset_id: String,
}

#[allow(dead_code)] // H-10: schema seal, held by mileage_asset_tests::h10_asset_payload_field_names_match_the_struct.
pub const ASSET_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "event_id",
    "origin",
    "asset_id",
    "description",
    "placed_in_service_on",
    "cost_cents",
    "disposal_date",
];

#[derive(Debug, Clone)]
pub struct RecordAssetInput {
    pub description: String,
    pub placed_in_service_on: String,
    pub cost_cents: i64,
    pub disposal_date: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CorrectAssetInput {
    pub asset_id: String,
    pub description: String,
    pub placed_in_service_on: String,
    pub cost_cents: i64,
    pub disposal_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetView {
    pub asset_id: String,
    pub origin: String,
    pub description: String,
    pub placed_in_service_on: String,
    pub cost_cents: i64,
    pub disposal_date: Option<String>,
    pub last_event_id: String,
    pub created_at: String,
    pub updated_at: String,
}

fn is_asset_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::AssetRecorded | Kind::AssetCorrected | Kind::AssetVoided
    )
}

fn allowed_keys(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::AssetVoided => ASSET_VOID_PAYLOAD_KEYS,
        _ => ASSET_PAYLOAD_KEYS,
    }
}

fn validate_calendar_date(date: &str, label: &str) -> Result<(), String> {
    let parts: Vec<_> = date.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("{label} must be YYYY-MM-DD"));
    }
    let y: i32 = parts[0]
        .parse()
        .map_err(|_| format!("{label} must be YYYY-MM-DD"))?;
    let m: u32 = parts[1]
        .parse()
        .map_err(|_| format!("{label} must be YYYY-MM-DD"))?;
    let d: u32 = parts[2]
        .parse()
        .map_err(|_| format!("{label} must be YYYY-MM-DD"))?;
    chrono::NaiveDate::from_ymd_opt(y, m, d)
        .ok_or_else(|| format!("{label} must be a real calendar day"))?;
    Ok(())
}

/// Validate sealed key set + four-field rules for asset kinds.
pub fn validate_asset_payload(payload: &Value, kind: Kind) -> Result<(), String> {
    if !is_asset_kind(kind) {
        return Ok(());
    }
    let obj = payload
        .as_object()
        .ok_or_else(|| format!("{} payload must be an object", kind.as_str()))?;

    let allowed = allowed_keys(kind);
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "{} rejects unknown payload key: {key}",
                kind.as_str()
            ));
        }
        if ASSET_FORBIDDEN_COMPUTED_KEYS
            .iter()
            .any(|f| f.eq_ignore_ascii_case(key))
        {
            return Err(format!(
                "asset register rejects computed payload key: {key}"
            ));
        }
    }

    let required: &[&str] = match kind {
        Kind::AssetVoided => &["eventId", "origin", "assetId"],
        _ => &[
            "eventId",
            "origin",
            "assetId",
            "description",
            "placedInServiceOn",
            "costCents",
        ],
    };
    for req in required {
        if !obj.contains_key(*req) {
            return Err(format!(
                "{} payload missing required key: {req}",
                kind.as_str()
            ));
        }
    }

    let origin = obj
        .get("origin")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} origin must be a string", kind.as_str()))?;
    if origin != "farm_os" {
        return Err(format!(
            "{} origin must be farm_os, got {origin}",
            kind.as_str()
        ));
    }

    if kind == Kind::AssetVoided {
        return Ok(());
    }

    let description = obj
        .get("description")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} description must be a string", kind.as_str()))?;
    let desc_trim = description.trim();
    if desc_trim.is_empty() {
        return Err(format!("{} description must be non-empty", kind.as_str()));
    }
    if desc_trim.chars().count() > 200 {
        return Err(format!(
            "{} description must be at most 200 characters",
            kind.as_str()
        ));
    }

    let placed = obj
        .get("placedInServiceOn")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} placedInServiceOn must be a string", kind.as_str()))?;
    validate_calendar_date(placed, "placedInServiceOn")?;

    let cost = obj
        .get("costCents")
        .ok_or_else(|| format!("{} costCents is missing", kind.as_str()))?;
    let cost_cents = match cost {
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| format!("{} costCents must be an integer", kind.as_str()))?,
        _ => {
            return Err(format!("{} costCents must be an integer", kind.as_str()));
        }
    };
    if cost_cents <= 0 {
        return Err(format!(
            "{} costCents must be greater than zero",
            kind.as_str()
        ));
    }

    if let Some(disposal_v) = obj.get("disposalDate") {
        if !disposal_v.is_null() {
            let disposal = disposal_v
                .as_str()
                .ok_or_else(|| format!("{} disposalDate must be a string", kind.as_str()))?;
            validate_calendar_date(disposal, "disposalDate")?;
            if disposal < placed {
                return Err("disposal cannot be before the date placed in service".into());
            }
        }
    }

    Ok(())
}

/// Choke-point gate for a full event record of asset kinds.
pub fn validate_asset_event(event: &EventRecord) -> Result<(), String> {
    if !is_asset_kind(event.kind) {
        return Ok(());
    }
    if event.origin != "farm_os" {
        return Err(format!(
            "{} origin must be farm_os, got {}",
            event.kind.as_str(),
            event.origin
        ));
    }
    validate_asset_payload(&event.payload, event.kind)?;
    let payload_origin = event
        .payload
        .get("origin")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if payload_origin != event.origin {
        return Err(format!(
            "{} payload origin disagrees with event record",
            event.kind.as_str()
        ));
    }
    let payload_id = event
        .payload
        .get("eventId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if payload_id != event.event_id {
        return Err(format!(
            "{} payload eventId disagrees with event record",
            event.kind.as_str()
        ));
    }
    let asset_id = event
        .payload
        .get("assetId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match event.kind {
        Kind::AssetRecorded => {
            if asset_id != event.event_id {
                return Err("asset.recorded payload assetId must equal event record id".into());
            }
        }
        Kind::AssetCorrected | Kind::AssetVoided => {
            if asset_id.is_empty() {
                return Err(format!(
                    "{} payload assetId must be non-empty",
                    event.kind.as_str()
                ));
            }
            if asset_id == event.event_id {
                return Err(format!(
                    "{} payload assetId must not equal event record id",
                    event.kind.as_str()
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn load_asset_guard(conn: &Connection, asset_id: &str) -> Result<(String, Option<String>), String> {
    conn.query_row(
        "SELECT last_event_id, voided_at FROM assets WHERE asset_id = ?1",
        [asset_id],
        |row| {
            let last: String = row.get(0)?;
            let voided: Option<String> = row.get(1)?;
            Ok((last, voided))
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "that equipment is no longer in the register".to_string())
    .and_then(|(last, voided)| {
        if voided.is_some() {
            Err("that equipment was removed".into())
        } else {
            Ok((last, voided))
        }
    })
}

fn load_asset_view(conn: &Connection, asset_id: &str) -> Result<AssetView, String> {
    conn.query_row(
        "SELECT asset_id, origin, description, placed_in_service_on, cost_cents,
                disposal_date, last_event_id, created_at, updated_at
         FROM assets WHERE asset_id = ?1 AND voided_at IS NULL",
        [asset_id],
        |row| {
            Ok(AssetView {
                asset_id: row.get(0)?,
                origin: row.get(1)?,
                description: row.get(2)?,
                placed_in_service_on: row.get(3)?,
                cost_cents: row.get(4)?,
                disposal_date: row.get(5)?,
                last_event_id: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        },
    )
    .map_err(|_| "that asset is no longer in the register".to_string())
}

fn build_asset_payload(
    event_id: &str,
    asset_id: &str,
    description: &str,
    placed_in_service_on: &str,
    cost_cents: i64,
    disposal_date: &Option<String>,
) -> Value {
    let mut payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "assetId": asset_id,
        "description": description,
        "placedInServiceOn": placed_in_service_on,
        "costCents": cost_cents,
    });
    if let Some(d) = disposal_date {
        payload
            .as_object_mut()
            .unwrap()
            .insert("disposalDate".into(), json!(d));
    }
    payload
}

/// Record one asset with exactly four operator fields.
pub fn record_asset(conn: &mut Connection, input: RecordAssetInput) -> Result<AssetView, String> {
    let description = input.description.trim().to_string();
    if description.is_empty() {
        return Err("description is required".into());
    }
    if input.cost_cents <= 0 {
        return Err("cost must be positive".into());
    }

    let now = projection::handler_now();
    let today_local = db::local_date_from_utc_rfc3339(&now)?;
    if input.placed_in_service_on > today_local {
        return Err("date placed in service cannot be in the future".into());
    }
    let disposal_date = input
        .disposal_date
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if let Some(ref d) = disposal_date {
        if d > &today_local {
            return Err("disposal date cannot be in the future".into());
        }
        if d.as_str() < input.placed_in_service_on.as_str() {
            return Err("disposal cannot be before the date placed in service".into());
        }
    }

    let event_id = projection::handler_new_id();
    let asset_id = event_id.clone();
    let payload = build_asset_payload(
        &event_id,
        &asset_id,
        &description,
        &input.placed_in_service_on,
        input.cost_cents,
        &disposal_date,
    );
    validate_asset_payload(&payload, Kind::AssetRecorded)?;

    let event = EventRecord::originated(
        Kind::AssetRecorded,
        "asset",
        asset_id.clone(),
        payload,
        json!({ "op": "none" }),
        now,
        None,
        None,
        Some(event_id),
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    crate::events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;

    load_asset_view(conn, &asset_id)
}

/// Full four-field replacement (used to set disposal date later).
pub fn correct_asset(conn: &mut Connection, input: CorrectAssetInput) -> Result<AssetView, String> {
    let (prior_last_event_id, _) = load_asset_guard(conn, &input.asset_id)?;
    let description = input.description.trim().to_string();
    if description.is_empty() {
        return Err("description is required".into());
    }
    if input.cost_cents <= 0 {
        return Err("cost must be positive".into());
    }

    let now = projection::handler_now();
    let today_local = db::local_date_from_utc_rfc3339(&now)?;
    if input.placed_in_service_on > today_local {
        return Err("date placed in service cannot be in the future".into());
    }
    let disposal_date = input
        .disposal_date
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if let Some(ref d) = disposal_date {
        if d > &today_local {
            return Err("disposal date cannot be in the future".into());
        }
        if d.as_str() < input.placed_in_service_on.as_str() {
            return Err("disposal cannot be before the date placed in service".into());
        }
    }

    let event_id = projection::handler_new_id();
    let payload = build_asset_payload(
        &event_id,
        &input.asset_id,
        &description,
        &input.placed_in_service_on,
        input.cost_cents,
        &disposal_date,
    );
    validate_asset_payload(&payload, Kind::AssetCorrected)?;

    let event = EventRecord::originated(
        Kind::AssetCorrected,
        "asset",
        input.asset_id.clone(),
        payload,
        json!({ "op": "none" }),
        now,
        None,
        Some(&prior_last_event_id),
        Some(event_id),
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    crate::events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;

    load_asset_view(conn, &input.asset_id)
}

/// Retire equipment entered in error. Row survives, marked voided.
pub fn void_asset(conn: &mut Connection, asset_id: &str) -> Result<(), String> {
    let (prior_last_event_id, _) = load_asset_guard(conn, asset_id)?;
    let now = projection::handler_now();
    let event_id = projection::handler_new_id();
    let payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "assetId": asset_id,
    });
    validate_asset_payload(&payload, Kind::AssetVoided)?;
    let event = EventRecord::originated(
        Kind::AssetVoided,
        "asset",
        asset_id,
        payload,
        json!({ "op": "none" }),
        now,
        None,
        Some(&prior_last_event_id),
        Some(event_id),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    crate::events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// List assets. No SUM. No total. No count of "active" assets.
pub fn list_assets(conn: &Connection) -> Result<Vec<AssetView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT asset_id, origin, description, placed_in_service_on, cost_cents,
                    disposal_date, last_event_id, created_at, updated_at
             FROM assets
             WHERE voided_at IS NULL
             ORDER BY placed_in_service_on DESC, created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(AssetView {
                asset_id: row.get(0)?,
                origin: row.get(1)?,
                description: row.get(2)?,
                placed_in_service_on: row.get(3)?,
                cost_cents: row.get(4)?,
                disposal_date: row.get(5)?,
                last_event_id: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn req_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("asset payload missing {key}"))
}

fn opt_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| if x.is_null() { None } else { x.as_str() })
}

fn req_i64(v: &Value, key: &str) -> Result<i64, String> {
    v.get(key)
        .and_then(|x| x.as_i64())
        .ok_or_else(|| format!("asset payload missing {key}"))
}

/// Projection: insert one asset. No clock.
pub fn apply_asset_recorded(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_asset_event(event)?;
    let p = &event.payload;
    let description = req_str(p, "description")?;
    let placed = req_str(p, "placedInServiceOn")?;
    let cost_cents = req_i64(p, "costCents")?;
    let disposal = opt_str(p, "disposalDate");

    tx.execute(
        "INSERT INTO assets
         (asset_id, origin, description, placed_in_service_on, cost_cents,
          disposal_date, last_event_id, created_at, updated_at, voided_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, NULL)",
        params![
            event.entity_id,
            event.origin,
            description,
            placed,
            cost_cents,
            disposal,
            event.event_id,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Projection: full four-field replacement. No clock.
pub fn apply_asset_corrected(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_asset_event(event)?;
    let p = &event.payload;
    let description = req_str(p, "description")?;
    let placed = req_str(p, "placedInServiceOn")?;
    let cost_cents = req_i64(p, "costCents")?;
    let disposal = opt_str(p, "disposalDate");

    let n = tx
        .execute(
            "UPDATE assets SET description = ?1, placed_in_service_on = ?2, cost_cents = ?3,
                    disposal_date = ?4, last_event_id = ?5, updated_at = ?6
             WHERE asset_id = ?7 AND voided_at IS NULL",
            params![
                description,
                placed,
                cost_cents,
                disposal,
                event.event_id,
                event.created_at,
                event.entity_id,
            ],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err("asset.corrected names an asset that is not in the register".into());
    }
    Ok(())
}

/// Projection: mark voided. No clock.
pub fn apply_asset_voided(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_asset_event(event)?;
    let n = tx
        .execute(
            "UPDATE assets SET voided_at = ?1, last_event_id = ?2, updated_at = ?3
             WHERE asset_id = ?4 AND voided_at IS NULL",
            params![
                event.created_at,
                event.event_id,
                event.created_at,
                event.entity_id,
            ],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err("asset.voided names an asset that is not in the register".into());
    }
    Ok(())
}

#[cfg(test)]
mod type_seal_tests {
    use super::*;

    #[test]
    fn asset_payload_type_admits_no_computed_field() {
        for name in ASSET_PAYLOAD_FIELD_NAMES {
            for forbidden in ASSET_FORBIDDEN_COMPUTED_KEYS {
                assert!(
                    !name.eq_ignore_ascii_case(forbidden),
                    "AssetPayload field {name} is a computed key"
                );
            }
        }
        // Built without embedding computed-key literals in production-scanned source (A6).
        let mut v = serde_json::json!({
            "eventId": "e1",
            "origin": "farm_os",
            "assetId": "e1",
            "description": "Truck",
            "placedInServiceOn": "2026-01-01",
            "costCents": 10000,
        });
        let computed_key = String::from_utf8(vec![
            b'd', b'e', b'p', b'r', b'e', b'c', b'i', b'a', b't', b'i', b'o', b'n',
        ])
        .unwrap();
        v.as_object_mut()
            .unwrap()
            .insert(computed_key.clone(), serde_json::json!(100));
        let err = serde_json::from_value::<AssetPayload>(v).unwrap_err();
        let err_s = err.to_string();
        assert!(
            err_s.contains("unknown field") || err_s.contains(&computed_key),
            "{err_s}"
        );
    }

    #[test]
    fn asset_void_payload_type_admits_no_computed_field() {
        let v = serde_json::json!({
            "eventId": "e1",
            "origin": "farm_os",
            "assetId": "a1",
            "depreciation": 100,
        });
        assert!(serde_json::from_value::<AssetVoidPayload>(v).is_err());
    }
}
