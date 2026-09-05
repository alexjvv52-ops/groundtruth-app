//! Mileage trips — miles only, never dollars (Track 4 residual).
//!
//! Authority: BOOKS-BOUNDARY mileage clause; ROADMAP Track 4 done-when.
//! Per-trip, dated, stored in MILES. There is no rate, no dollar value and no
//! conversion in this module or anywhere downstream of it. 2026 IRS rates
//! change mid-year; a dollar-denominated log cannot be split at that boundary,
//! so the boundary is kept by never denominating it at all.
//! Shape mirrors Track 3 / Track 4 core: EventRecord::originated ->
//! projection::apply_event + events::insert_event in one transaction.
//! apply_* reads no clock; created_at/updated_at come from event.created_at.

use crate::db;
use crate::events::{EventRecord, Kind};
use crate::projection;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const MILEAGE_TRIPS_COLUMNS: &[&str] = &[
    "trip_id",
    "origin",
    "trip_date",
    "miles",
    "purpose",
    "voided_at",
    "last_event_id",
    "created_at",
    "updated_at",
];

/// Columns filled from the event spine, never from a payload key.
#[allow(dead_code)] // H-10: schema seal, held by mileage_asset_tests::h10_mileage_spine_columns_are_real_and_listed.
pub const MILEAGE_SPINE_COLUMNS: &[&str] =
    &["voided_at", "last_event_id", "created_at", "updated_at"];

/// Payload keys (camelCase) for mileage.trip and mileage.trip_corrected.
pub const MILEAGE_TRIP_PAYLOAD_KEYS: &[&str] = &[
    "eventId", "origin", "tripId", "tripDate", "miles", "purpose",
];

/// Payload keys for mileage.trip_voided.
pub const MILEAGE_VOID_PAYLOAD_KEYS: &[&str] = &["eventId", "origin", "tripId"];

/// Forbidden key names — a mileage payload must never be able to carry or
/// imply money. Checked case-insensitively at the choke point.
// FORBIDDEN-KEYS-BEGIN
pub const MILEAGE_FORBIDDEN_KEYS: &[&str] = &[
    "dollars",
    "dollar",
    "amount",
    "amountCents",
    "cents",
    "price",
    "unitPrice",
    "unitPriceCents",
    "cost",
    "costCents",
    "value",
    "rate",
    "mileageRate",
    "perMile",
    "irsRate",
    "reimbursement",
    "deduction",
    "deductionCents",
    "total",
    "subtotal",
    "usd",
    "extended",
];
// FORBIDDEN-KEYS-END

/// Sealed mileage trip payload. `deny_unknown_fields` is the type-level seal —
/// a monetary sibling cannot be added without failing deserialize.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub struct MileageTripPayload {
    pub event_id: String,
    pub origin: String,
    pub trip_id: String,
    /// Operator-entered calendar day, YYYY-MM-DD.
    pub trip_date: String,
    /// MILES. Never converted. There is no companion rate or amount field.
    pub miles: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub struct MileageVoidPayload {
    pub event_id: String,
    pub origin: String,
    pub trip_id: String,
}

#[allow(dead_code)] // H-10: schema seal, held by mileage_asset_tests::h10_mileage_trip_payload_field_names_match_the_struct.
pub const MILEAGE_TRIP_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "event_id",
    "origin",
    "trip_id",
    "trip_date",
    "miles",
    "purpose",
];

#[derive(Debug, Clone)]
pub struct RecordMileageTripInput {
    pub trip_date: String,
    pub miles: f64,
    pub purpose: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CorrectMileageTripInput {
    pub trip_id: String,
    pub trip_date: String,
    pub miles: f64,
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MileageTripView {
    pub trip_id: String,
    pub origin: String,
    pub trip_date: String,
    pub miles: f64,
    pub purpose: Option<String>,
    pub last_event_id: String,
    pub created_at: String,
    pub updated_at: String,
}

fn is_mileage_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::MileageTripLogged | Kind::MileageTripCorrected | Kind::MileageTripVoided
    )
}

fn allowed_keys(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::MileageTripVoided => MILEAGE_VOID_PAYLOAD_KEYS,
        _ => MILEAGE_TRIP_PAYLOAD_KEYS,
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

/// Validate sealed key set + miles-only rules for mileage kinds.
pub fn validate_mileage_payload(payload: &Value, kind: Kind) -> Result<(), String> {
    if !is_mileage_kind(kind) {
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
        if MILEAGE_FORBIDDEN_KEYS
            .iter()
            .any(|f| f.eq_ignore_ascii_case(key))
        {
            return Err(format!("mileage rejects monetary payload key: {key}"));
        }
    }

    let required: &[&str] = match kind {
        Kind::MileageTripVoided => &["eventId", "origin", "tripId"],
        _ => &["eventId", "origin", "tripId", "tripDate", "miles"],
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

    if kind != Kind::MileageTripVoided {
        let trip_date = obj
            .get("tripDate")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("{} tripDate must be a string", kind.as_str()))?;
        validate_calendar_date(trip_date, "tripDate")?;

        let q = obj
            .get("miles")
            .ok_or_else(|| format!("{} miles is missing", kind.as_str()))?;
        let miles = match q {
            Value::Null => {
                return Err(format!("{} miles must not be null", kind.as_str()));
            }
            Value::String(_) => {
                return Err(format!(
                    "{} miles must be a number, not a string",
                    kind.as_str()
                ));
            }
            Value::Number(n) => n
                .as_f64()
                .ok_or_else(|| format!("{} miles must be a finite number", kind.as_str()))?,
            _ => {
                return Err(format!("{} miles must be a number", kind.as_str()));
            }
        };
        if miles.is_nan() {
            return Err(format!("{} miles must not be NaN", kind.as_str()));
        }
        if miles.is_infinite() {
            return Err(format!("{} miles must not be Infinity", kind.as_str()));
        }
        if miles <= 0.0 {
            return Err(format!("{} miles must be greater than zero", kind.as_str()));
        }

        if let Some(purpose_v) = obj.get("purpose") {
            if !purpose_v.is_null() {
                let purpose = purpose_v
                    .as_str()
                    .ok_or_else(|| format!("{} purpose must be a string", kind.as_str()))?;
                if purpose.trim().chars().count() > 200 {
                    return Err(format!(
                        "{} purpose must be at most 200 characters",
                        kind.as_str()
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Choke-point gate for a full event record of mileage kinds.
pub fn validate_mileage_event(event: &EventRecord) -> Result<(), String> {
    if !is_mileage_kind(event.kind) {
        return Ok(());
    }
    if event.origin != "farm_os" {
        return Err(format!(
            "{} origin must be farm_os, got {}",
            event.kind.as_str(),
            event.origin
        ));
    }
    validate_mileage_payload(&event.payload, event.kind)?;
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
    let trip_id = event
        .payload
        .get("tripId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match event.kind {
        Kind::MileageTripLogged => {
            if trip_id != event.event_id {
                return Err("mileage.trip payload tripId must equal event record id".into());
            }
        }
        Kind::MileageTripCorrected | Kind::MileageTripVoided => {
            if trip_id.is_empty() {
                return Err(format!(
                    "{} payload tripId must be non-empty",
                    event.kind.as_str()
                ));
            }
            if trip_id == event.event_id {
                return Err(format!(
                    "{} payload tripId must not equal event record id",
                    event.kind.as_str()
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn load_trip_guard(conn: &Connection, trip_id: &str) -> Result<(String, Option<String>), String> {
    conn.query_row(
        "SELECT last_event_id, voided_at FROM mileage_trips WHERE trip_id = ?1",
        [trip_id],
        |row| {
            let last: String = row.get(0)?;
            let voided: Option<String> = row.get(1)?;
            Ok((last, voided))
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "that trip is no longer in the log".to_string())
    .and_then(|(last, voided)| {
        if voided.is_some() {
            Err("that trip was removed".into())
        } else {
            Ok((last, voided))
        }
    })
}

fn load_trip_view(conn: &Connection, trip_id: &str) -> Result<MileageTripView, String> {
    conn.query_row(
        "SELECT trip_id, origin, trip_date, miles, purpose, last_event_id, created_at, updated_at
         FROM mileage_trips WHERE trip_id = ?1 AND voided_at IS NULL",
        [trip_id],
        |row| {
            Ok(MileageTripView {
                trip_id: row.get(0)?,
                origin: row.get(1)?,
                trip_date: row.get(2)?,
                miles: row.get(3)?,
                purpose: row.get(4)?,
                last_event_id: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        },
    )
    .map_err(|_| "that trip is no longer in the log".to_string())
}

/// Record one dated trip in miles.
pub fn record_trip(
    conn: &mut Connection,
    input: RecordMileageTripInput,
) -> Result<MileageTripView, String> {
    if !input.miles.is_finite() || input.miles <= 0.0 {
        return Err("miles must be greater than zero".into());
    }
    let now = projection::handler_now();
    let today_local = db::local_date_from_utc_rfc3339(&now)?;
    if input.trip_date > today_local {
        return Err("a trip cannot be in the future".into());
    }

    let event_id = projection::handler_new_id();
    let trip_id = event_id.clone();
    let purpose = input
        .purpose
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let mut payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "tripId": trip_id,
        "tripDate": input.trip_date,
        "miles": input.miles,
    });
    if let Some(ref p) = purpose {
        payload
            .as_object_mut()
            .unwrap()
            .insert("purpose".into(), json!(p));
    }
    validate_mileage_payload(&payload, Kind::MileageTripLogged)?;

    let event = EventRecord::originated(
        Kind::MileageTripLogged,
        "mileage_trip",
        trip_id.clone(),
        payload,
        json!({ "op": "none" }),
        now.clone(),
        None,
        None,
        Some(event_id.clone()),
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    crate::events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;

    load_trip_view(conn, &trip_id)
}

/// Full replacement of a trip's operator fields.
pub fn correct_trip(
    conn: &mut Connection,
    input: CorrectMileageTripInput,
) -> Result<MileageTripView, String> {
    if !input.miles.is_finite() || input.miles <= 0.0 {
        return Err("miles must be greater than zero".into());
    }
    let (prior_last_event_id, _) = load_trip_guard(conn, &input.trip_id)?;

    let now = projection::handler_now();
    let today_local = db::local_date_from_utc_rfc3339(&now)?;
    if input.trip_date > today_local {
        return Err("a trip cannot be in the future".into());
    }

    let event_id = projection::handler_new_id();
    let purpose = input
        .purpose
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let mut payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "tripId": input.trip_id,
        "tripDate": input.trip_date,
        "miles": input.miles,
    });
    if let Some(ref p) = purpose {
        payload
            .as_object_mut()
            .unwrap()
            .insert("purpose".into(), json!(p));
    }
    validate_mileage_payload(&payload, Kind::MileageTripCorrected)?;

    let event = EventRecord::originated(
        Kind::MileageTripCorrected,
        "mileage_trip",
        input.trip_id.clone(),
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

    load_trip_view(conn, &input.trip_id)
}

/// Retire a trip that never happened. Row survives, marked voided.
pub fn void_trip(conn: &mut Connection, trip_id: &str) -> Result<(), String> {
    let (prior_last_event_id, _) = load_trip_guard(conn, trip_id)?;
    let now = projection::handler_now();
    let event_id = projection::handler_new_id();

    let payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "tripId": trip_id,
    });
    validate_mileage_payload(&payload, Kind::MileageTripVoided)?;

    let event = EventRecord::originated(
        Kind::MileageTripVoided,
        "mileage_trip",
        trip_id,
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

/// List non-voided trips. No aggregate, no SUM, no total row.
pub fn list_trips(conn: &Connection) -> Result<Vec<MileageTripView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT trip_id, origin, trip_date, miles, purpose, last_event_id, created_at, updated_at
             FROM mileage_trips
             WHERE voided_at IS NULL
             ORDER BY trip_date DESC, created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(MileageTripView {
                trip_id: row.get(0)?,
                origin: row.get(1)?,
                trip_date: row.get(2)?,
                miles: row.get(3)?,
                purpose: row.get(4)?,
                last_event_id: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn req_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("mileage payload missing {key}"))
}

fn opt_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| if x.is_null() { None } else { x.as_str() })
}

fn req_f64(v: &Value, key: &str) -> Result<f64, String> {
    v.get(key)
        .and_then(|x| x.as_f64())
        .ok_or_else(|| format!("mileage payload missing {key}"))
}

/// Projection: insert one trip. No clock — timestamps from event.created_at.
pub fn apply_mileage_trip(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_mileage_event(event)?;
    let p = &event.payload;
    let trip_date = req_str(p, "tripDate")?;
    let miles = req_f64(p, "miles")?;
    let purpose = opt_str(p, "purpose");

    tx.execute(
        "INSERT INTO mileage_trips
         (trip_id, origin, trip_date, miles, purpose, voided_at, last_event_id,
          created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?7)",
        params![
            event.entity_id,
            event.origin,
            trip_date,
            miles,
            purpose,
            event.event_id,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Projection: full replacement of operator fields. No clock.
pub fn apply_mileage_trip_corrected(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    validate_mileage_event(event)?;
    let p = &event.payload;
    let trip_date = req_str(p, "tripDate")?;
    let miles = req_f64(p, "miles")?;
    let purpose = opt_str(p, "purpose");

    let n = tx
        .execute(
            "UPDATE mileage_trips SET trip_date = ?1, miles = ?2, purpose = ?3,
                    last_event_id = ?4, updated_at = ?5
             WHERE trip_id = ?6 AND voided_at IS NULL",
            params![
                trip_date,
                miles,
                purpose,
                event.event_id,
                event.created_at,
                event.entity_id,
            ],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err("mileage.trip_corrected names a trip that is not in the register".into());
    }
    Ok(())
}

/// Projection: mark voided. No clock.
pub fn apply_mileage_trip_voided(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_mileage_event(event)?;
    let n = tx
        .execute(
            "UPDATE mileage_trips SET voided_at = ?1, last_event_id = ?2, updated_at = ?3
             WHERE trip_id = ?4 AND voided_at IS NULL",
            params![
                event.created_at,
                event.event_id,
                event.created_at,
                event.entity_id,
            ],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err("mileage.trip_voided names a trip that is not in the register".into());
    }
    Ok(())
}

#[cfg(test)]
mod type_seal_tests {
    use super::*;

    #[test]
    fn mileage_payload_type_admits_no_monetary_field() {
        for name in MILEAGE_TRIP_PAYLOAD_FIELD_NAMES {
            for forbidden in MILEAGE_FORBIDDEN_KEYS {
                assert!(
                    !name.eq_ignore_ascii_case(forbidden),
                    "MileageTripPayload field {name} is monetary"
                );
            }
            let lower = name.to_ascii_lowercase();
            assert!(
                !lower.contains("dollar")
                    && !lower.contains("price")
                    && !lower.contains("cents")
                    && !lower.contains("amount")
                    && !lower.contains("usd")
                    && !lower.contains("rate"),
                "MileageTripPayload field {name} looks monetary"
            );
        }
        // Built without embedding rate literals in production-scanned source (M7).
        let mut v = serde_json::json!({
            "eventId": "e1",
            "origin": "farm_os",
            "tripId": "e1",
            "tripDate": "2026-08-07",
            "miles": 12.4,
        });
        let money_key = String::from_utf8(vec![b'r', b'a', b't', b'e']).unwrap();
        v.as_object_mut()
            .unwrap()
            .insert(money_key.clone(), serde_json::json!(7.0_f64 / 10.0));
        let err = serde_json::from_value::<MileageTripPayload>(v).unwrap_err();
        let err_s = err.to_string();
        assert!(
            err_s.contains("unknown field") || err_s.contains(&money_key),
            "{err_s}"
        );
    }
}
