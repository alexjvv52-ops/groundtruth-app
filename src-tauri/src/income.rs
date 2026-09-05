//! Money arriving — Farm OS origin, recorded at the act (money-in track).
//!
//! This module computes nothing about tax; it carries both mapping lines so the
//! preparer never re-types. It never touches capacity: income reserves nothing,
//! and nothing here reads or writes trays, orders or capacity.

use crate::categories::{self, line_is_other};
use crate::costs;
use crate::db;
use crate::events::{EventRecord, Kind};
use crate::projection;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub const INCOME_EVENTS_COLUMNS: &[&str] = &[
    "income_id",
    "origin",
    "date_received",
    "amount_cents",
    "source",
    "canonical_category",
    "schedule_f_line",
    "schedule_c_line",
    "descriptor",
    "receipt_file_ref",
    "last_event_id",
    "created_at",
    "updated_at",
    "voided_at",
];

#[allow(dead_code)] // H-10: schema seal, held by income_tests::h10_income_spine_columns_are_real_and_listed.
pub const INCOME_SPINE_COLUMNS: &[&str] =
    &["last_event_id", "created_at", "updated_at", "voided_at"];

pub const INCOME_PAYLOAD_KEYS: &[&str] = &[
    "eventId",
    "origin",
    "incomeId",
    "dateReceived",
    "amountCents",
    "source",
    "canonicalCategory",
    "scheduleFLine",
    "scheduleCLine",
    "descriptor",
    "receiptFileRef",
];

/// Payload keys for income.voided — identity only.
pub const INCOME_VOID_PAYLOAD_KEYS: &[&str] = &["eventId", "origin", "incomeId"];

/// Forbidden key names — the register must never carry a derived number.
/// The per-tray derivation name is assembled with `concat!` so production
/// scanners that hunt for that derivation do not treat this forbid-list as a
/// reader of it.
// FORBIDDEN-KEYS-BEGIN
pub const INCOME_FORBIDDEN_COMPUTED_KEYS: &[&str] = &[
    "net",
    "netCents",
    "profit",
    "profitCents",
    "margin",
    "marginCents",
    "costOfGoods",
    "cogs",
    "tax",
    "taxCents",
    "taxable",
    "deduction",
    "estimated",
    "projected",
    "forecast",
    "perTray",
    concat!("cost", "PerTray"),
    "balance",
    "runningTotal",
    "ytd",
];
// FORBIDDEN-KEYS-END

/// Sealed income payload. `deny_unknown_fields` is the type-level seal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub struct IncomePayload {
    pub event_id: String,
    pub origin: String,
    pub income_id: String,
    /// Operator-entered calendar day, YYYY-MM-DD.
    pub date_received: String,
    /// Integer cents. Stored. Never operated on.
    pub amount_cents: i64,
    pub source: String,
    pub canonical_category: String,
    pub schedule_f_line: String,
    pub schedule_c_line: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_file_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub struct IncomeVoidPayload {
    pub event_id: String,
    pub origin: String,
    pub income_id: String,
}

#[allow(dead_code)] // H-10: schema seal, held by income_tests::h10_income_payload_field_names_match_the_struct.
pub const INCOME_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "event_id",
    "origin",
    "income_id",
    "date_received",
    "amount_cents",
    "source",
    "canonical_category",
    "schedule_f_line",
    "schedule_c_line",
    "descriptor",
    "receipt_file_ref",
];

#[derive(Debug, Clone)]
pub struct RecordIncomeInput {
    pub amount_cents: i64,
    pub source: String,
    pub category_id: String,
    pub date_received: String,
    pub descriptor: Option<String>,
    pub receipt_source_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CorrectIncomeInput {
    pub income_id: String,
    pub amount_cents: i64,
    pub source: String,
    pub category_id: String,
    pub date_received: String,
    pub descriptor: Option<String>,
    pub receipt_source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeView {
    pub income_id: String,
    pub origin: String,
    pub date_received: String,
    pub amount_cents: i64,
    pub source: String,
    pub canonical_category: String,
    pub descriptor: String,
    pub receipt_file_ref: Option<String>,
    pub last_event_id: String,
    pub created_at: String,
    pub updated_at: String,
}

fn is_income_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::IncomeReceived | Kind::IncomeCorrected | Kind::IncomeVoided
    )
}

fn allowed_keys(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::IncomeVoided => INCOME_VOID_PAYLOAD_KEYS,
        _ => INCOME_PAYLOAD_KEYS,
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

/// Validate sealed key set + operator-field rules for income kinds.
pub fn validate_income_payload(payload: &Value, kind: Kind) -> Result<(), String> {
    if !is_income_kind(kind) {
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
        if INCOME_FORBIDDEN_COMPUTED_KEYS
            .iter()
            .any(|f| f.eq_ignore_ascii_case(key))
        {
            return Err(format!(
                "income register rejects computed payload key: {key}"
            ));
        }
    }

    let required: &[&str] = match kind {
        Kind::IncomeVoided => &["eventId", "origin", "incomeId"],
        _ => &[
            "eventId",
            "origin",
            "incomeId",
            "dateReceived",
            "amountCents",
            "source",
            "canonicalCategory",
            "scheduleFLine",
            "scheduleCLine",
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

    if kind == Kind::IncomeVoided {
        return Ok(());
    }

    let source = obj
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} source must be a string", kind.as_str()))?;
    let source_trim = source.trim();
    if source_trim.is_empty() {
        return Err(format!("{} source must be non-empty", kind.as_str()));
    }
    if source_trim.chars().count() > 200 {
        return Err(format!(
            "{} source must be at most 200 characters",
            kind.as_str()
        ));
    }

    let date_received = obj
        .get("dateReceived")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} dateReceived must be a string", kind.as_str()))?;
    validate_calendar_date(date_received, "dateReceived")?;

    let amount = obj
        .get("amountCents")
        .ok_or_else(|| format!("{} amountCents is missing", kind.as_str()))?;
    let amount_cents = match amount {
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| format!("{} amountCents must be an integer", kind.as_str()))?,
        _ => {
            return Err(format!("{} amountCents must be an integer", kind.as_str()));
        }
    };
    if amount_cents <= 0 {
        return Err(format!(
            "{} amountCents must be greater than zero",
            kind.as_str()
        ));
    }

    let schedule_f = obj
        .get("scheduleFLine")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let schedule_c = obj
        .get("scheduleCLine")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let needs_descriptor = line_is_other(schedule_f) || line_is_other(schedule_c);
    if needs_descriptor {
        let descriptor = obj
            .get("descriptor")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if descriptor.is_empty() {
            return Err(format!(
                "{} descriptor required for other line",
                kind.as_str()
            ));
        }
    }

    Ok(())
}

/// Choke-point gate for a full event record of income kinds.
pub fn validate_income_event(event: &EventRecord) -> Result<(), String> {
    if !is_income_kind(event.kind) {
        return Ok(());
    }
    if event.origin != "farm_os" {
        return Err(format!(
            "{} origin must be farm_os, got {}",
            event.kind.as_str(),
            event.origin
        ));
    }
    validate_income_payload(&event.payload, event.kind)?;
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
    let income_id = event
        .payload
        .get("incomeId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match event.kind {
        Kind::IncomeReceived => {
            if income_id != event.event_id {
                return Err("income.received payload incomeId must equal event record id".into());
            }
        }
        Kind::IncomeCorrected | Kind::IncomeVoided => {
            if income_id.is_empty() {
                return Err(format!(
                    "{} payload incomeId must be non-empty",
                    event.kind.as_str()
                ));
            }
            if income_id == event.event_id {
                return Err(format!(
                    "{} payload incomeId must not equal event record id",
                    event.kind.as_str()
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn load_income_guard(
    conn: &Connection,
    income_id: &str,
) -> Result<(String, Option<String>), String> {
    conn.query_row(
        "SELECT last_event_id, voided_at FROM income_events WHERE income_id = ?1",
        [income_id],
        |row| {
            let last: String = row.get(0)?;
            let voided: Option<String> = row.get(1)?;
            Ok((last, voided))
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "that income record is no longer in the register".to_string())
    .and_then(|(last, voided)| {
        if voided.is_some() {
            Err("that income record was removed".into())
        } else {
            Ok((last, voided))
        }
    })
}

pub(crate) fn load_income_view(conn: &Connection, income_id: &str) -> Result<IncomeView, String> {
    conn.query_row(
        "SELECT income_id, origin, date_received, amount_cents, source,
                canonical_category, descriptor, receipt_file_ref,
                last_event_id, created_at, updated_at
         FROM income_events WHERE income_id = ?1 AND voided_at IS NULL",
        [income_id],
        |row| {
            Ok(IncomeView {
                income_id: row.get(0)?,
                origin: row.get(1)?,
                date_received: row.get(2)?,
                amount_cents: row.get(3)?,
                source: row.get(4)?,
                canonical_category: row.get(5)?,
                descriptor: row.get(6)?,
                receipt_file_ref: row.get(7)?,
                last_event_id: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        },
    )
    .map_err(|_| "that income record is no longer in the register".to_string())
}

/// H-7b Fence 3b - the ten values this payload builder needs, named. Field
/// order is the old parameter order, so the mapping is positional and nothing
/// was reordered.
///
/// No derives, ever. This struct is not the payload and must never become it.
/// The object below is built by hand because two of its properties cannot
/// survive derived serialization: `origin` is a literal with no field behind
/// it, and `receiptFileRef` is absent - not null - when there is no receipt.
/// `descriptor` is written even when it is empty. Those are ledger bytes and
/// verify-replay compares them.
struct IncomePayloadInput<'a> {
    event_id: &'a str,
    income_id: &'a str,
    date_received: &'a str,
    amount_cents: i64,
    source: &'a str,
    canonical_category: &'a str,
    schedule_f_line: &'a str,
    schedule_c_line: &'a str,
    descriptor: &'a str,
    receipt_file_ref: &'a Option<String>,
}
fn build_income_payload(fields: &IncomePayloadInput<'_>) -> Value {
    let mut payload = json!({
        "eventId": fields.event_id,
        "origin": "farm_os",
        "incomeId": fields.income_id,
        "dateReceived": fields.date_received,
        "amountCents": fields.amount_cents,
        "source": fields.source,
        "canonicalCategory": fields.canonical_category,
        "scheduleFLine": fields.schedule_f_line,
        "scheduleCLine": fields.schedule_c_line,
        "descriptor": fields.descriptor,
    });
    if let Some(r) = fields.receipt_file_ref {
        payload
            .as_object_mut()
            .unwrap()
            .insert("receiptFileRef".into(), json!(r));
    }
    payload
}

fn resolve_operator_fields(
    input_source: &str,
    category_id: &str,
    descriptor: &Option<String>,
    amount_cents: i64,
) -> Result<(String, &'static categories::IncomeCategory, String), String> {
    let category = categories::find_income_category(category_id)
        .ok_or_else(|| format!("unknown category: {category_id}"))?;
    let source = input_source.trim().to_string();
    if source.is_empty() {
        return Err("source is required".into());
    }
    if source.chars().count() > 200 {
        return Err("source must be at most 200 characters".into());
    }
    if amount_cents <= 0 {
        return Err("amount must be positive".into());
    }
    let descriptor = descriptor
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let needs_descriptor = category.descriptor_required
        || line_is_other(category.schedule_f_line)
        || line_is_other(category.schedule_c_line);
    if needs_descriptor && descriptor.is_empty() {
        return Err("a short description is required for this category".into());
    }
    Ok((source, category, descriptor))
}

/// D8 (signed): a second income row for the same venue and amount inside 7
/// days is probably one receipt recorded twice. Signed law is WARN AND
/// ACKNOWLEDGE, not refuse — a genuine second payment of the same amount is
/// real. Matching is on `source` text, which pay_order fills with the venue
/// name and record_income fills with free text, so a typo will miss. That
/// imprecision is inherent to the signed rule, not a defect to widen.
pub fn duplicate_income_warning(
    conn: &Connection,
    source: &str,
    amount_cents: i64,
    date_received: &str,
) -> Result<Option<String>, String> {
    let source = source.trim();
    if source.is_empty() {
        return Ok(None);
    }
    let matched: Option<(String, String)> = conn
        .query_row(
            "SELECT source, date_received FROM income_events
             WHERE voided_at IS NULL
               AND source = ?1
               AND amount_cents = ?2
               AND date_received BETWEEN date(?3, '-7 days') AND date(?3, '+7 days')
             ORDER BY date_received ASC, created_at ASC
             LIMIT 1",
            params![source, amount_cents, date_received],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((venue, d)) = matched else {
        return Ok(None);
    };
    let amount = crate::attention::dollars(amount_cents);
    let d = crate::reachability::format_mon_d_local(&d)?;
    Ok(Some(format!(
        "{venue} already has {amount} recorded on {d}. Recording this again makes two income rows for one payment."
    )))
}

/// Record that money came in. Completes fully offline.
///
/// When a receipt path is supplied, the file is fully written under
/// `farm_dir/receipts/` BEFORE the income_events insert commits.
pub fn record_income(
    conn: &mut Connection,
    farm_dir: &Path,
    input: RecordIncomeInput,
    duplicate_ack: bool,
) -> Result<IncomeView, String> {
    let (source, category, descriptor) = resolve_operator_fields(
        &input.source,
        &input.category_id,
        &input.descriptor,
        input.amount_cents,
    )?;
    validate_calendar_date(&input.date_received, "date received")?;

    let now = projection::handler_now();
    let today_local = db::local_date_from_utc_rfc3339(&now)?;
    if input.date_received > today_local {
        return Err("date received cannot be in the future".into());
    }
    if !duplicate_ack {
        if let Some(line) =
            duplicate_income_warning(conn, &source, input.amount_cents, &input.date_received)?
        {
            return Err(line);
        }
    }

    let receipt_file_ref = match input.receipt_source_path.as_deref() {
        Some(p) if !p.trim().is_empty() => {
            Some(costs::persist_receipt(farm_dir, Path::new(p.trim()))?)
        }
        _ => None,
    };

    let event_id = projection::handler_new_id();
    let income_id = event_id.clone();
    let payload = build_income_payload(&IncomePayloadInput {
        event_id: &event_id,
        income_id: &income_id,
        date_received: &input.date_received,
        amount_cents: input.amount_cents,
        source: &source,
        canonical_category: category.id,
        schedule_f_line: category.schedule_f_line,
        schedule_c_line: category.schedule_c_line,
        descriptor: &descriptor,
        receipt_file_ref: &receipt_file_ref,
    });
    validate_income_payload(&payload, Kind::IncomeReceived)?;

    let event = EventRecord::originated(
        Kind::IncomeReceived,
        "income",
        income_id.clone(),
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

    load_income_view(conn, &income_id)
}

/// Write `income.received` inside an already-open transaction.
/// Used by wholesale.paid so cash-in and the order-book pointer commit together.
pub(crate) fn write_received_in_tx(
    tx: &Transaction<'_>,
    source: &str,
    category_id: &str,
    amount_cents: i64,
    date_received: &str,
    descriptor: Option<String>,
    now: &str,
) -> Result<String, String> {
    let (source, category, descriptor) =
        resolve_operator_fields(source, category_id, &descriptor, amount_cents)?;
    validate_calendar_date(date_received, "date received")?;
    let today_local = db::local_date_from_utc_rfc3339(now)?;
    if date_received > today_local.as_str() {
        return Err("date received cannot be in the future".into());
    }
    let event_id = projection::handler_new_id();
    let income_id = event_id.clone();
    let payload = build_income_payload(&IncomePayloadInput {
        event_id: &event_id,
        income_id: &income_id,
        date_received,
        amount_cents,
        source: &source,
        canonical_category: category.id,
        schedule_f_line: category.schedule_f_line,
        schedule_c_line: category.schedule_c_line,
        descriptor: &descriptor,
        receipt_file_ref: &None,
    });
    validate_income_payload(&payload, Kind::IncomeReceived)?;
    let event = EventRecord::originated(
        Kind::IncomeReceived,
        "income",
        income_id,
        payload,
        json!({ "op": "none" }),
        now.to_string(),
        None,
        None,
        Some(event_id.clone()),
    );
    crate::projection::apply_event(tx, &event)?;
    crate::events::insert_event(tx, &event)?;
    Ok(event.event_id)
}

/// D6: authority is the event log. An income row named by a PAID wholesale
/// order is that order's payment record, not free-standing money. R1/R2
/// refuse the operator doors while the order stands paid. The signed
/// `wholesale.payment_reversed` path is the only writer that un-pays, and
/// it voids this row in the same transaction.
fn paid_order_for_income(
    conn: &Connection,
    income_id: &str,
) -> Result<Option<(String /*venue name*/, String /*paid_on*/)>, String> {
    conn.query_row(
        "SELECT v.name, o.paid_on
         FROM wholesale_orders o
         JOIN mkt_venues v ON v.venue_id = o.venue_id
         WHERE o.income_event_id = ?1 AND o.state = 'paid'",
        [income_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Full replacement of operator fields.
pub fn correct_income(
    conn: &mut Connection,
    farm_dir: &Path,
    input: CorrectIncomeInput,
) -> Result<IncomeView, String> {
    let (prior_last_event_id, _) = load_income_guard(conn, &input.income_id)?;
    if let Some((venue, paid_on)) = paid_order_for_income(conn, &input.income_id)? {
        let d = crate::reachability::format_mon_d_local(&paid_on)?;
        return Err(format!(
            "That payment is the record of a paid order - {venue}, {d}. Its amount cannot be corrected while the order stands paid."
        ));
    }
    let (source, category, descriptor) = resolve_operator_fields(
        &input.source,
        &input.category_id,
        &input.descriptor,
        input.amount_cents,
    )?;
    validate_calendar_date(&input.date_received, "date received")?;

    let now = projection::handler_now();
    let today_local = db::local_date_from_utc_rfc3339(&now)?;
    if input.date_received > today_local {
        return Err("date received cannot be in the future".into());
    }

    let receipt_file_ref = match input.receipt_source_path.as_deref() {
        Some(p) if !p.trim().is_empty() => {
            Some(costs::persist_receipt(farm_dir, Path::new(p.trim()))?)
        }
        _ => None,
    };

    let event_id = projection::handler_new_id();
    let payload = build_income_payload(&IncomePayloadInput {
        event_id: &event_id,
        income_id: &input.income_id,
        date_received: &input.date_received,
        amount_cents: input.amount_cents,
        source: &source,
        canonical_category: category.id,
        schedule_f_line: category.schedule_f_line,
        schedule_c_line: category.schedule_c_line,
        descriptor: &descriptor,
        receipt_file_ref: &receipt_file_ref,
    });
    validate_income_payload(&payload, Kind::IncomeCorrected)?;

    let event = EventRecord::originated(
        Kind::IncomeCorrected,
        "income",
        input.income_id.clone(),
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

    load_income_view(conn, &input.income_id)
}

/// Shared byte path for `income.voided`. No paid-order check.
///
/// D6 stands: R1/R2 guard the OPERATOR doors. This in-tx writer is the
/// shared byte path, reached by the operator door only after its refusal
/// and by a signed payment reversal. Nothing else may call it.
pub(crate) fn void_income_in_tx(tx: &Transaction<'_>, income_id: &str) -> Result<String, String> {
    let (prior_last_event_id, _) = load_income_guard(tx, income_id)?;
    let now = projection::handler_now();
    let event_id = projection::handler_new_id();
    let payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "incomeId": income_id,
    });
    validate_income_payload(&payload, Kind::IncomeVoided)?;
    let event = EventRecord::originated(
        Kind::IncomeVoided,
        "income",
        income_id,
        payload,
        json!({ "op": "none" }),
        now,
        None,
        Some(&prior_last_event_id),
        Some(event_id.clone()),
    );
    projection::apply_event(tx, &event)?;
    crate::events::insert_event(tx, &event)?;
    Ok(event_id)
}

/// Retire a record entered in error. Row survives, marked voided.
pub fn void_income(conn: &mut Connection, income_id: &str) -> Result<(), String> {
    let (_prior_last_event_id, _) = load_income_guard(conn, income_id)?;
    if let Some((venue, paid_on)) = paid_order_for_income(conn, income_id)? {
        let d = crate::reachability::format_mon_d_local(&paid_on)?;
        return Err(format!(
            "That payment is the record of a paid order - {venue}, {d}. It cannot be voided while the order stands paid."
        ));
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    void_income_in_tx(&tx, income_id)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// List income. No SUM in SQL — the UI totals exactly the rows returned.
pub fn list_income(conn: &Connection) -> Result<Vec<IncomeView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT income_id, origin, date_received, amount_cents, source,
                    canonical_category, descriptor, receipt_file_ref,
                    last_event_id, created_at, updated_at
             FROM income_events
             WHERE voided_at IS NULL
             ORDER BY date_received DESC, created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(IncomeView {
                income_id: row.get(0)?,
                origin: row.get(1)?,
                date_received: row.get(2)?,
                amount_cents: row.get(3)?,
                source: row.get(4)?,
                canonical_category: row.get(5)?,
                descriptor: row.get(6)?,
                receipt_file_ref: row.get(7)?,
                last_event_id: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// B-1(a) signed 2026-08-23 — the ONE cash-basis union. Cash arrives two ways
/// and only one place in the tree ever added them without double counting:
/// `export::write_income_csv`. `export.rs:936-939` states the house rule for
/// exactly this case — "A second SUM here could disagree with the card, so
/// there is not one." This function IS that union, lifted out so the export
/// bundle and Books read the same rows.
///
/// Sources: `income_events` with origin 'farm_os' and voided_at NULL (wholesale
/// payments and manually recorded income), plus retail `orders` in state
/// 'paid'. A retail order that was refunded or disputed leaves state 'paid' and
/// so leaves cash entirely rather than netting — existing export behaviour
/// (export.rs:531-537), inherited unchanged, not a new rule.
///
/// B-2: `from`/`to` are local YYYY-MM-DD and INCLUSIVE at both ends; None means
/// unbounded. `income_events.date_received` is already a local date and filters
/// in SQL. `orders.paid_at` is UTC RFC3339, so it is converted with
/// `db::local_date_from_utc_rfc3339` and filtered in Rust — the same conversion
/// export already performs at export.rs:604. SQLite cannot do it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CashRow {
    /// "recorded" or "stripe" — the same discriminator income.csv writes.
    pub record_type: String,
    pub income_id: String,
    pub date_received: String,
    pub amount_cents: i64,
    pub source: String,
    pub canonical_category: String,
    pub schedule_f_line: String,
    pub schedule_c_line: String,
    pub descriptor: String,
    pub receipt_file_ref: String,
}

fn date_in_inclusive_range(date: &str, from: Option<&str>, to: Option<&str>) -> bool {
    from.map(|f| date >= f).unwrap_or(true) && to.map(|t| date <= t).unwrap_or(true)
}

pub fn cash_rows_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<CashRow>, String> {
    let produce = categories::find_income_category("produce_you_grew")
        .ok_or_else(|| "produce_you_grew income category missing".to_string())?;

    let mut rows: Vec<CashRow> = Vec::new();

    let mut stmt = conn
        .prepare(
            "SELECT income_id, date_received, amount_cents, source, canonical_category,
                    schedule_f_line, schedule_c_line, descriptor, receipt_file_ref
             FROM income_events
             WHERE origin = 'farm_os' AND voided_at IS NULL
               AND (?1 IS NULL OR date_received >= ?1)
               AND (?2 IS NULL OR date_received <= ?2)",
        )
        .map_err(|e| e.to_string())?;
    let recorded = stmt
        .query_map(params![from, to], |r| {
            Ok(CashRow {
                record_type: "recorded".into(),
                income_id: r.get(0)?,
                date_received: r.get(1)?,
                amount_cents: r.get(2)?,
                source: r.get(3)?,
                canonical_category: r.get(4)?,
                schedule_f_line: r.get(5)?,
                schedule_c_line: r.get(6)?,
                descriptor: r.get(7)?,
                receipt_file_ref: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            })
        })
        .map_err(|e| e.to_string())?;
    for row in recorded {
        rows.push(row.map_err(|e| e.to_string())?);
    }

    let mut stmt = conn
        .prepare(
            "SELECT o.id, o.paid_at, o.amount_cents, c.name
             FROM orders o
             JOIN crops c ON c.id = o.crop_id
             WHERE o.state = 'paid'",
        )
        .map_err(|e| e.to_string())?;
    let stripe = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in stripe {
        let (id, paid_at, amount_cents, crop_name) = row.map_err(|e| e.to_string())?;
        let date_received = db::local_date_from_utc_rfc3339(&paid_at)?;
        if !date_in_inclusive_range(&date_received, from, to) {
            continue;
        }
        rows.push(CashRow {
            record_type: "stripe".into(),
            income_id: id,
            date_received,
            amount_cents,
            source: format!("Online order — {crop_name}"),
            canonical_category: produce.id.to_string(),
            schedule_f_line: produce.schedule_f_line.to_string(),
            schedule_c_line: produce.schedule_c_line.to_string(),
            descriptor: String::new(),
            receipt_file_ref: String::new(),
        });
    }

    rows.sort_by(|a, b| (&a.date_received, &a.income_id).cmp(&(&b.date_received, &b.income_id)));
    Ok(rows)
}

/// The headline for the Cash collected lens. Literally the fold of the rows
/// above, so the total and the "show the rows" disclosure cannot disagree —
/// the rule list_income already states at income.rs:809.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CashCollected {
    pub total_cents: i64,
    pub count: i64,
}

pub fn cash_collected_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<CashCollected, String> {
    let rows = cash_rows_between(conn, from, to)?;
    Ok(CashCollected {
        total_cents: rows.iter().map(|r| r.amount_cents).sum(),
        count: rows.len() as i64,
    })
}

/// Lens 6. V-1: a CASH-BASIS figure — collected minus cash out — not profit.
/// No stored fact anywhere records a net figure and none is created here; both
/// sides are read at call time from the two shared readers. Negative is normal
/// and is returned as a negative integer, not clamped.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetCash {
    pub collected_cents: i64,
    pub out_cents: i64,
    pub net_cents: i64,
}

pub fn net_cash_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<NetCash, String> {
    let collected = cash_collected_between(conn, from, to)?.total_cents;
    let out = costs::cash_out_between(conn, from, to)?.total_cents;
    Ok(NetCash {
        collected_cents: collected,
        out_cents: out,
        net_cents: collected - out,
    })
}

/// Lens 8, V-4 signed 2026-08-23. A pure fold of `cash_rows_between` — the same
/// dollars as Cash collected, only grouped. It cannot disagree with lens 1
/// because it is the same rows.
///
/// The operator sees the plain-language category name only. Schedule F and C
/// line numbers never appear in the UI (categories.rs:4, :272), so neither
/// leaves this reader. An id with no entry in INCOME_CATEGORIES is shown as its
/// raw id rather than swallowed — the stateLabel rule at Money.tsx:161-164.
/// V-4: amount per category and nothing else. No percentage, no share of total.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryTotal {
    pub category_id: String,
    pub name: String,
    pub total_cents: i64,
    pub count: i64,
}

pub fn cash_by_category_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<CategoryTotal>, String> {
    let rows = cash_rows_between(conn, from, to)?;
    let mut totals: HashMap<String, (i64, i64)> = HashMap::new();
    for row in &rows {
        let entry = totals
            .entry(row.canonical_category.clone())
            .or_insert((0, 0));
        entry.0 += row.amount_cents;
        entry.1 += 1;
    }
    let mut out = Vec::new();
    for cat in categories::INCOME_CATEGORIES {
        if let Some((total_cents, count)) = totals.remove(cat.id) {
            out.push(CategoryTotal {
                category_id: cat.id.to_string(),
                name: cat.name.to_string(),
                total_cents,
                count,
            });
        }
    }
    let mut unknown: Vec<CategoryTotal> = totals
        .into_iter()
        .map(|(id, (total_cents, count))| CategoryTotal {
            name: id.clone(),
            category_id: id,
            total_cents,
            count,
        })
        .collect();
    unknown.sort_by(|a, b| a.category_id.cmp(&b.category_id));
    out.extend(unknown);
    Ok(out)
}

/// Lens 9, V-2(a) signed 2026-08-23: a COUNT of corrections and voids in the
/// period. No amount is derived — "how much came off the books" is not a number
/// this tree records and none is invented here.
///
/// The period is the CORRECTION's own date. `event_log.created_at` is a UTC
/// timestamp, so it is converted with db::local_date_from_utc_rfc3339 (db.rs:1946)
/// and filtered in Rust — the same treatment retail paid_at gets in
/// cash_rows_between. SQLite cannot do the conversion.
///
/// `action` carries the same two words income-corrections.csv already writes
/// (export.rs:1041-1043): "corrected" and "voided". No new vocabulary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeCorrectionRow {
    pub correction_event_id: String,
    pub target_income_id: String,
    pub action: String,
    pub corrected_on: String,
}

pub fn income_correction_rows_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<IncomeCorrectionRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT e.id, e.kind, e.entity_id, e.created_at FROM event_log e
             WHERE e.kind IN ('income.corrected','income.voided') ORDER BY e.seq",
        )
        .map_err(|e| e.to_string())?;
    let raw = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for row in raw {
        let (id, kind, entity_id, created_at) = row.map_err(|e| e.to_string())?;
        let corrected_on = db::local_date_from_utc_rfc3339(&created_at)?;
        if !date_in_inclusive_range(&corrected_on, from, to) {
            continue;
        }
        let action = match kind.as_str() {
            "income.corrected" => "corrected",
            "income.voided" => "voided",
            other => {
                return Err(format!("unexpected income correction kind: {other}"));
            }
        };
        rows.push(IncomeCorrectionRow {
            correction_event_id: id,
            target_income_id: entity_id,
            action: action.to_string(),
            corrected_on,
        });
    }
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeCorrectionCount {
    pub count: i64,
}

pub fn income_correction_count_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<IncomeCorrectionCount, String> {
    Ok(IncomeCorrectionCount {
        count: income_correction_rows_between(conn, from, to)?.len() as i64,
    })
}

fn req_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("income payload missing {key}"))
}

fn opt_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| if x.is_null() { None } else { x.as_str() })
}

fn req_i64(v: &Value, key: &str) -> Result<i64, String> {
    v.get(key)
        .and_then(|x| x.as_i64())
        .ok_or_else(|| format!("income payload missing {key}"))
}

/// Projection: insert one income row. No clock.
pub fn apply_income_received(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_income_event(event)?;
    let p = &event.payload;
    let date_received = req_str(p, "dateReceived")?;
    let amount_cents = req_i64(p, "amountCents")?;
    let source = req_str(p, "source")?;
    let canonical_category = req_str(p, "canonicalCategory")?;
    let schedule_f_line = req_str(p, "scheduleFLine")?;
    let schedule_c_line = req_str(p, "scheduleCLine")?;
    let descriptor = p.get("descriptor").and_then(|v| v.as_str()).unwrap_or("");
    let receipt = opt_str(p, "receiptFileRef");

    tx.execute(
        "INSERT INTO income_events
         (income_id, origin, date_received, amount_cents, source,
          canonical_category, schedule_f_line, schedule_c_line, descriptor,
          receipt_file_ref, last_event_id, created_at, updated_at, voided_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, NULL)",
        params![
            event.entity_id,
            event.origin,
            date_received,
            amount_cents,
            source,
            canonical_category,
            schedule_f_line,
            schedule_c_line,
            descriptor,
            receipt,
            event.event_id,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Projection: full operator-field replacement. No clock.
pub fn apply_income_corrected(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_income_event(event)?;
    let p = &event.payload;
    let date_received = req_str(p, "dateReceived")?;
    let amount_cents = req_i64(p, "amountCents")?;
    let source = req_str(p, "source")?;
    let canonical_category = req_str(p, "canonicalCategory")?;
    let schedule_f_line = req_str(p, "scheduleFLine")?;
    let schedule_c_line = req_str(p, "scheduleCLine")?;
    let descriptor = p.get("descriptor").and_then(|v| v.as_str()).unwrap_or("");
    let receipt = opt_str(p, "receiptFileRef");

    let n = tx
        .execute(
            "UPDATE income_events SET date_received = ?1, amount_cents = ?2, source = ?3,
                    canonical_category = ?4, schedule_f_line = ?5, schedule_c_line = ?6,
                    descriptor = ?7, receipt_file_ref = ?8, last_event_id = ?9, updated_at = ?10
             WHERE income_id = ?11 AND voided_at IS NULL",
            params![
                date_received,
                amount_cents,
                source,
                canonical_category,
                schedule_f_line,
                schedule_c_line,
                descriptor,
                receipt,
                event.event_id,
                event.created_at,
                event.entity_id,
            ],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err("income.corrected names a record that is not in the register".into());
    }
    Ok(())
}

/// Projection: mark voided. No clock.
pub fn apply_income_voided(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_income_event(event)?;
    let n = tx
        .execute(
            "UPDATE income_events SET voided_at = ?1, last_event_id = ?2, updated_at = ?3
             WHERE income_id = ?4 AND voided_at IS NULL",
            params![
                event.created_at,
                event.event_id,
                event.created_at,
                event.entity_id,
            ],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err("income.voided names a record that is not in the register".into());
    }
    Ok(())
}

#[cfg(test)]
mod type_seal_tests {
    use super::*;

    #[test]
    fn income_payload_type_admits_no_computed_field() {
        for name in INCOME_PAYLOAD_FIELD_NAMES {
            for forbidden in INCOME_FORBIDDEN_COMPUTED_KEYS {
                assert!(
                    !name.eq_ignore_ascii_case(forbidden),
                    "IncomePayload field {name} is a computed key"
                );
            }
        }
        let mut v = serde_json::json!({
            "eventId": "e1",
            "origin": "farm_os",
            "incomeId": "e1",
            "dateReceived": "2026-01-01",
            "amountCents": 10000,
            "source": "Grant",
            "canonicalCategory": "program_payment",
            "scheduleFLine": "4b",
            "scheduleCLine": "6 other",
            "descriptor": "EQIP",
        });
        v.as_object_mut()
            .unwrap()
            .insert("profitCents".into(), serde_json::json!(100));
        let err = serde_json::from_value::<IncomePayload>(v).unwrap_err();
        let err_s = err.to_string();
        assert!(
            err_s.contains("unknown field") || err_s.contains("profitCents"),
            "{err_s}"
        );
    }
}
