//! Physical consumption events — units only, never dollars (Track 4).
//!
//! Authority: BOOKS-BOUNDARY §3. Shape mirrors Track 3 cost writes:
//! EventRecord::originated → projection::apply_event + events::insert_event
//! in one transaction. Payload is sealed at the choke point for this kind only.

use crate::events::{EventRecord, Kind};
use crate::projection;
use rusqlite::{params, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Every column `consumption_events` projection writes.
pub const CONSUMPTION_EVENTS_COLUMNS: &[&str] = &[
    "event_id",
    "origin",
    "occurred_at",
    "variety_or_item",
    "unit",
    "quantity",
    "sow_event_id",
    "linked_cost_event_id",
    "notes",
];

/// Payload keys (camelCase). Total over `CONSUMPTION_EVENTS_COLUMNS`.
pub const CONSUMPTION_PAYLOAD_KEYS: &[&str] = &[
    "eventId",
    "origin",
    "occurredAt",
    "varietyOrItem",
    "unit",
    "quantity",
    "sowEventId",
    "linkedCostEventId",
    "notes",
];

/// Forbidden monetary key names — must never appear on a consumption payload.
pub const FORBIDDEN_MONETARY_KEYS: &[&str] = &[
    "dollars",
    "dollar",
    "amount",
    "price",
    "cost",
    "unit_cost",
    "unitCost",
    "value",
    "rate",
    "total",
    "extended",
    "usd",
    "cents",
    "subtotal",
    "amountCents",
    "unitPriceCents",
];

/// Sealed consumption payload. `deny_unknown_fields` is the type-level seal —
/// a monetary sibling cannot be added without failing deserialize of extras,
/// and the struct itself admits no monetary field (T6c).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub struct ConsumptionPayload {
    pub event_id: String,
    pub origin: String,
    pub occurred_at: String,
    pub variety_or_item: String,
    pub unit: String,
    pub quantity: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sow_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_cost_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Compile-time / schema-level: the payload type has exactly these fields.
/// Adding a monetary field requires editing this list — T6c asserts equality.
#[allow(dead_code)] // H-10: schema seal, held by consumption_tests::t6c_payload_type_no_monetary_keys.
pub const CONSUMPTION_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "event_id",
    "origin",
    "occurred_at",
    "variety_or_item",
    "unit",
    "quantity",
    "sow_event_id",
    "linked_cost_event_id",
    "notes",
];

pub const TRAY_VARIETY_OR_ITEM: &str = "tray";
pub const UNIT_TRAY: &str = "tray";
pub const UNIT_OZ: &str = "oz";

/// Unit for the standing planting retired by a harvest act.
/// Deliberately NOT `UNIT_TRAY`. BOOKS-BOUNDARY §3: any window measuring physical
/// quantity per tray takes its tray count from surviving consumption.physical
/// records with unit = 'tray'. Those records are the trays brought into being at
/// sow. A harvest record carrying 'tray' would double that denominator silently,
/// which §3 forbids. Do not "simplify" this back to UNIT_TRAY.
pub const UNIT_PLANTING: &str = "planting";

#[derive(Debug, Clone)]
pub struct RecordConsumptionInput {
    pub variety_or_item: String,
    pub unit: String,
    pub quantity: f64,
    pub occurred_at: String,
    pub sow_event_id: Option<String>,
    pub linked_cost_event_id: Option<String>,
    pub notes: Option<String>,
}

/// Validate BOOKS-BOUNDARY §3 quantity + sealed key set. Used by the choke
/// point for `consumption.physical` only — other kinds stay open.
pub fn validate_consumption_payload(payload: &Value) -> Result<(), String> {
    let obj = payload
        .as_object()
        .ok_or_else(|| "consumption.physical payload must be an object".to_string())?;

    for key in obj.keys() {
        if !CONSUMPTION_PAYLOAD_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "consumption.physical rejects unknown payload key: {key}"
            ));
        }
        if FORBIDDEN_MONETARY_KEYS
            .iter()
            .any(|f| f.eq_ignore_ascii_case(key))
        {
            return Err(format!(
                "consumption.physical rejects monetary payload key: {key}"
            ));
        }
    }

    for req in [
        "eventId",
        "origin",
        "occurredAt",
        "varietyOrItem",
        "unit",
        "quantity",
    ] {
        if !obj.contains_key(req) {
            return Err(format!(
                "consumption.physical payload missing required key: {req}"
            ));
        }
    }

    let origin = obj
        .get("origin")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "consumption.physical origin must be a string".to_string())?;
    if origin != "farm_os" {
        return Err(format!(
            "consumption.physical origin must be farm_os, got {origin}"
        ));
    }

    let variety = obj
        .get("varietyOrItem")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "consumption.physical varietyOrItem must be a string".to_string())?;
    if variety.trim().is_empty() {
        return Err("consumption.physical varietyOrItem must be non-empty".into());
    }

    let unit = obj
        .get("unit")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "consumption.physical unit must be a string".to_string())?;
    if unit.trim().is_empty() {
        return Err("consumption.physical unit must be non-empty".into());
    }

    let q = obj
        .get("quantity")
        .ok_or_else(|| "consumption.physical quantity is missing".to_string())?;
    let quantity = match q {
        Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| "consumption.physical quantity must be a finite number".to_string())?,
        Value::Null => {
            return Err("consumption.physical quantity is missing".into());
        }
        _ => {
            return Err("consumption.physical quantity must be a finite number".into());
        }
    };
    if !quantity.is_finite() {
        return Err("consumption.physical quantity must be finite (not NaN or Infinity)".into());
    }
    if quantity <= 0.0 {
        return Err("consumption.physical quantity must be greater than zero".into());
    }

    Ok(())
}

/// Choke-point gate for a full event record of this kind.
pub fn validate_consumption_event(event: &EventRecord) -> Result<(), String> {
    if event.kind != Kind::ConsumptionPhysical {
        return Ok(());
    }
    if event.origin != "farm_os" {
        return Err(format!(
            "consumption.physical origin must be farm_os, got {}",
            event.origin
        ));
    }
    validate_consumption_payload(&event.payload)?;
    let payload_origin = event
        .payload
        .get("origin")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if payload_origin != event.origin {
        return Err("consumption.physical payload origin disagrees with event record".into());
    }
    let payload_id = event
        .payload
        .get("eventId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if payload_id != event.event_id {
        return Err("consumption.physical payload eventId disagrees with event record".into());
    }
    Ok(())
}

/// Build a sealed consumption event (not yet written).
pub fn build_consumption_event(input: RecordConsumptionInput) -> Result<EventRecord, String> {
    if !input.quantity.is_finite() || input.quantity <= 0.0 {
        return Err("consumption quantity must be a finite number greater than zero".into());
    }
    let variety = input.variety_or_item.trim().to_string();
    if variety.is_empty() {
        return Err("variety_or_item is required".into());
    }
    let unit = input.unit.trim().to_string();
    if unit.is_empty() {
        return Err("unit is required".into());
    }

    let event_id = projection::handler_new_id();
    let mut payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "occurredAt": input.occurred_at,
        "varietyOrItem": variety,
        "unit": unit,
        "quantity": input.quantity,
    });
    let obj = payload.as_object_mut().unwrap();
    if let Some(sow_event_id) = input.sow_event_id {
        obj.insert("sowEventId".into(), json!(sow_event_id));
    }
    if let Some(linked) = input.linked_cost_event_id {
        obj.insert("linkedCostEventId".into(), json!(linked));
    }
    if let Some(notes) = input.notes {
        obj.insert("notes".into(), json!(notes));
    }

    validate_consumption_payload(&payload)?;

    Ok(EventRecord::originated(
        Kind::ConsumptionPhysical,
        "consumption",
        event_id.clone(),
        payload,
        json!({ "op": "none" }),
        input.occurred_at,
        None,
        None,
        Some(event_id),
    ))
}

/// Write one consumption record inside an open transaction (Track 3 shape).
pub fn insert_consumption_in_tx(
    tx: &Transaction<'_>,
    input: RecordConsumptionInput,
) -> Result<EventRecord, String> {
    let event = build_consumption_event(input)?;
    projection::apply_event(tx, &event)?;
    crate::events::insert_event(tx, &event)?;
    Ok(event)
}

/// Projection: flat append-only mirror of event fields. No computation.
pub fn apply_consumption_physical(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_consumption_event(event)?;
    let p = &event.payload;
    let occurred_at = req_str(p, "occurredAt")?;
    let variety_or_item = req_str(p, "varietyOrItem")?;
    let unit = req_str(p, "unit")?;
    let quantity = req_f64(p, "quantity")?;
    let sow_event_id = opt_str(p, "sowEventId");
    let linked = opt_str(p, "linkedCostEventId");
    let notes = opt_str(p, "notes");

    tx.execute(
        "INSERT INTO consumption_events
         (event_id, origin, occurred_at, variety_or_item, unit, quantity,
          sow_event_id, linked_cost_event_id, notes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            event.event_id,
            event.origin,
            occurred_at,
            variety_or_item,
            unit,
            quantity,
            sow_event_id,
            linked,
            notes,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn req_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("consumption.physical payload missing {key}"))
}

fn opt_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| if x.is_null() { None } else { x.as_str() })
}

fn req_f64(v: &Value, key: &str) -> Result<f64, String> {
    v.get(key)
        .and_then(|x| x.as_f64())
        .ok_or_else(|| format!("consumption.physical payload missing {key}"))
}

#[cfg(test)]
mod type_seal_tests {
    use super::*;

    #[test]
    fn consumption_payload_type_admits_no_monetary_field() {
        assert_eq!(
            CONSUMPTION_PAYLOAD_KEYS.len(),
            CONSUMPTION_EVENTS_COLUMNS.len()
        );
        assert_eq!(
            CONSUMPTION_PAYLOAD_FIELD_NAMES.len(),
            CONSUMPTION_EVENTS_COLUMNS.len()
        );
        for name in CONSUMPTION_PAYLOAD_FIELD_NAMES {
            for forbidden in FORBIDDEN_MONETARY_KEYS {
                assert!(
                    !name.eq_ignore_ascii_case(forbidden),
                    "ConsumptionPayload field {name} is monetary"
                );
            }
            let lower = name.to_ascii_lowercase();
            // linked_cost_event_id is the BOOKS-BOUNDARY optional join key, not money.
            if *name == "linked_cost_event_id" {
                continue;
            }
            assert!(
                !lower.contains("dollar")
                    && !lower.contains("price")
                    && !lower.contains("cents")
                    && !lower.contains("amount")
                    && !lower.contains("usd"),
                "ConsumptionPayload field {name} looks monetary"
            );
        }
        // deny_unknown_fields: extra monetary key fails deserialize.
        let with_money = r#"{
            "eventId": "e1",
            "origin": "farm_os",
            "occurredAt": "2026-08-07T00:00:00.000Z",
            "varietyOrItem": "tray",
            "unit": "tray",
            "quantity": 1.0,
            "dollars": 3.50
        }"#;
        let err = serde_json::from_str::<ConsumptionPayload>(with_money).unwrap_err();
        assert!(
            err.to_string().contains("unknown field") || err.to_string().contains("dollars"),
            "{err}"
        );
    }
}
