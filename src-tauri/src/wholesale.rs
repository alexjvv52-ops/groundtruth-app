//! Wholesale order book — four register kinds, one money register, one pool.
//!
//! Authority: GT-D14. Payment is not a new money fact: `wholesale.paid`
//! points at an `income.received` row written in the same transaction.
//! Capacity subtracts alongside the retail path. Mixes decompose at entry.

use crate::attention;
use crate::categories;
use crate::db;
use crate::events::{EventRecord, Kind};
use crate::income;
use crate::projection;
use crate::trays;
use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

/// Verify-replay compares every column. Each is written only by an
/// apply_wholesale_* fn from the event payload + event.created_at.
pub const WHOLESALE_ORDERS_COLUMNS: &[&str] = &[
    "id",
    "venue_id",
    "harvest_date",
    "state",
    "ordered_on",
    "delivered_on",
    "paid_on",
    "income_event_id",
    "voided_at",
    "void_reason",
    "created_at",
    "updated_at",
    "payment_link_id",
    "payment_link_url",
    "payment_link_minted_at",
];
/// Keyed by (order_id, crop_id) — compared through compare_keyed_sql.
pub const WHOLESALE_ORDER_LINES_COLUMNS: &[&str] =
    &["order_id", "crop_id", "trays", "price_cents_per_tray"];
pub const WHOLESALE_WRITE_OFFS_COLUMNS: &[&str] = &[
    "event_id",
    "order_id",
    "income_event_id",
    "shortfall_cents",
    "category",
    "reason",
    "written_off_on",
    "created_at",
];
pub const WHOLESALE_BAD_DEBTS_COLUMNS: &[&str] = &[
    "event_id",
    "order_id",
    "amount_cents",
    "written_off_on",
    "created_at",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrderLine {
    pub crop_id: String,
    pub trays: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_cents_per_tray: Option<i64>,
}

pub const ORDER_LINE_FIELD_NAMES: &[&str] = &["crop_id", "trays", "price_cents_per_tray"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrderedPayload {
    pub order_id: String,
    pub venue_id: String,
    pub harvest_date: String,
    pub ordered_on: String,
    pub lines: Vec<OrderLine>,
    /// The operator was shown the over-commit line and recorded anyway.
    /// A fact about the promise, not about the UI -- the payload is the
    /// only place it survives.
    ///
    /// serde(default) so every wholesale.ordered event written before this
    /// field existed still deserializes, and reads false -- which is true
    /// of them. This is what keeps replay of the existing log working.
    #[serde(default)]
    pub overcommit_ack: bool,
}

pub const ORDERED_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "order_id",
    "venue_id",
    "harvest_date",
    "ordered_on",
    "lines",
    "overcommit_ack",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveredPayload {
    pub order_id: String,
    pub delivered_on: String,
}

pub const DELIVERED_PAYLOAD_FIELD_NAMES: &[&str] = &["order_id", "delivered_on"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaidPayload {
    pub order_id: String,
    pub paid_on: String,
    pub income_event_id: String,
    /// TILL-A (GT-D22): set only when the poll booked this payment from a
    /// Checkout Session on the order's own Payment Link. The event log is the
    /// only place the session id lives; a re-read session is answered by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_payment_intent: Option<String>,
}

pub const PAID_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "order_id",
    "paid_on",
    "income_event_id",
    "stripe_session_id",
    "stripe_payment_intent",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoidedPayload {
    pub order_id: String,
    pub voided_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub const VOIDED_PAYLOAD_FIELD_NAMES: &[&str] = &["order_id", "voided_at", "reason"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaymentReversedPayload {
    pub order_id: String,
    pub income_event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[allow(dead_code)] // H-10: schema seal, held by fence_bounce_tests::payment_reversed_payload_field_names_pinned.
pub const PAYMENT_REVERSED_PAYLOAD_FIELD_NAMES: &[&str] =
    &["order_id", "income_event_id", "reason"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BadDebtPayload {
    pub order_id: String,
    pub amount_cents: i64,
    pub written_off_on: String,
}
#[allow(dead_code)] // H-10: schema seal, held by fence_bad_debt_tests::bad_debt_payload_field_names_pinned.
pub const BAD_DEBT_PAYLOAD_FIELD_NAMES: &[&str] = &["order_id", "amount_cents", "written_off_on"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkMintedPayload {
    pub order_id: String,
    pub payment_link_id: String,
    /// The URL the operator shows: Stripe's link url with the baked
    /// client_reference_id query parameter (the poll's primary match key).
    pub payment_link_url: String,
    pub client_reference: String,
    pub amount_cents: i64,
    pub currency: String,
    pub minted_on: String,
}

pub const LINK_MINTED_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "order_id",
    "payment_link_id",
    "payment_link_url",
    "client_reference",
    "amount_cents",
    "currency",
    "minted_on",
];

/// The five business-standard allowance categories, hard-coded by ruling 2.
/// Stored as these exact strings; the UI owns the labels. The DB CHECK in
/// SCHEMA_V26_WRITE_OFFS_SQL carries the same five and must not drift.
pub const WRITE_OFF_CATEGORIES: [&str; 5] = [
    "sales_discount",
    "quality_spoilage",
    "pricing_or_billing_error",
    "customer_goodwill",
    "other",
];
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteOffPayload {
    pub order_id: String,
    pub income_event_id: String,
    pub shortfall_cents: i64,
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub written_off_on: String,
}
pub const WRITE_OFF_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "order_id",
    "income_event_id",
    "shortfall_cents",
    "category",
    "reason",
    "written_off_on",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WholesaleOrderLineView {
    pub crop_id: String,
    pub crop_name: String,
    pub trays: i64,
    pub price_cents_per_tray: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WholesaleOrderView {
    pub id: String,
    pub venue_id: String,
    pub venue_name: String,
    pub harvest_date: String,
    pub state: String,
    pub ordered_on: String,
    pub delivered_on: Option<String>,
    pub paid_on: Option<String>,
    pub income_event_id: Option<String>,
    pub voided_at: Option<String>,
    pub void_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub lines: Vec<WholesaleOrderLineView>,
    pub priced_total_cents: Option<i64>,
    pub delivered_age_days: Option<i64>,
    pub payment_link_id: Option<String>,
    pub payment_link_url: Option<String>,
    pub payment_link_minted_at: Option<String>,
}

/// C-2 (SOP-2): one pack per venue for a harvest date. Read-only.
/// ROUTE (ENGINE C / JOIN id): venue_id, address and phone ride along for
/// the run page, read from the venue row by venue_id -- never by venue
/// name. Both stay nullable; the paper drops an absent segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VenuePackView {
    pub venue_id: String,
    pub venue_name: String,
    pub harvest_date: String,
    pub lines: Vec<WholesaleOrderLineView>,
    pub tray_total: i64,
    pub address: Option<String>,
    pub phone: Option<String>,
}

fn is_wholesale_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::WholesaleOrdered
            | Kind::WholesaleDelivered
            | Kind::WholesalePaid
            | Kind::WholesaleVoided
            | Kind::WholesaleWriteOff
            | Kind::WholesalePaymentReversed
            | Kind::WholesaleBadDebt
            | Kind::WholesaleLinkMinted
    )
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
    NaiveDate::from_ymd_opt(y, m, d)
        .ok_or_else(|| format!("{label} must be a real calendar day"))?;
    Ok(())
}

fn format_mon_d(yyyy_mm_dd: &str) -> Result<String, String> {
    let d = NaiveDate::parse_from_str(yyyy_mm_dd, "%Y-%m-%d").map_err(|e| e.to_string())?;
    Ok(format!("{} {}", d.format("%b"), d.day()))
}

fn validate_lines(lines: &[OrderLine]) -> Result<(), String> {
    if lines.is_empty() {
        return Err("order lines must not be empty".into());
    }
    let mut seen = BTreeSet::new();
    for line in lines {
        if line.trays < 1 {
            return Err("trays must be at least 1".into());
        }
        if let Some(price) = line.price_cents_per_tray {
            if price < 1 {
                return Err("price_cents_per_tray must be at least 1 when present".into());
            }
        }
        if line.crop_id.trim().is_empty() {
            return Err("crop_id must be non-empty".into());
        }
        if !seen.insert(line.crop_id.clone()) {
            return Err(format!("duplicate crop line: {}", line.crop_id));
        }
    }
    Ok(())
}

/// Choke-point gate for wholesale order-book kinds.
pub fn validate_wholesale_event(event: &EventRecord) -> Result<(), String> {
    if !is_wholesale_kind(event.kind) {
        return Ok(());
    }
    match event.kind {
        Kind::WholesaleOrdered => {
            let p: OrderedPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("wholesale.ordered payload refused: {e}"))?;
            validate_calendar_date(&p.harvest_date, "harvest_date")?;
            validate_calendar_date(&p.ordered_on, "ordered_on")?;
            validate_lines(&p.lines)?;
        }
        Kind::WholesaleDelivered => {
            let p: DeliveredPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("wholesale.delivered payload refused: {e}"))?;
            validate_calendar_date(&p.delivered_on, "delivered_on")?;
        }
        Kind::WholesalePaid => {
            let p: PaidPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("wholesale.paid payload refused: {e}"))?;
            validate_calendar_date(&p.paid_on, "paid_on")?;
            if p.income_event_id.trim().is_empty() {
                return Err("income_event_id required".into());
            }
        }
        Kind::WholesaleVoided => {
            let _: VoidedPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("wholesale.voided payload refused: {e}"))?;
        }
        Kind::WholesaleWriteOff => {
            let p: WriteOffPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("wholesale.write_off payload refused: {e}"))?;
            validate_calendar_date(&p.written_off_on, "written_off_on")?;
            if p.shortfall_cents < 1 {
                return Err("shortfall_cents must be at least 1".into());
            }
            if !WRITE_OFF_CATEGORIES.contains(&p.category.as_str()) {
                return Err(format!("unknown write-off category: {}", p.category));
            }
            // Ruling 3 — Other without a reason is the silence this fence exists
            // to remove. Refused in the payload, not only in the UI.
            if p.category == "other" && p.reason.as_deref().map(str::trim).unwrap_or("").is_empty()
            {
                return Err("a reason is required when the category is Other".into());
            }
        }
        Kind::WholesalePaymentReversed => {
            let p: PaymentReversedPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("wholesale.payment_reversed payload refused: {e}"))?;
            if p.order_id.trim().is_empty() {
                return Err("order_id required".into());
            }
            if p.income_event_id.trim().is_empty() {
                return Err("income_event_id required".into());
            }
        }
        Kind::WholesaleBadDebt => {
            let p: BadDebtPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("wholesale.bad_debt payload refused: {e}"))?;
            if p.order_id.trim().is_empty() {
                return Err("order_id required".into());
            }
            if p.amount_cents < 1 {
                return Err("amount_cents must be at least 1".into());
            }
            validate_calendar_date(&p.written_off_on, "written_off_on")?;
        }
        Kind::WholesaleLinkMinted => {
            let p: LinkMintedPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("wholesale.link_minted payload refused: {e}"))?;
            if p.order_id.trim().is_empty() {
                return Err("order_id required".into());
            }
            if p.payment_link_id.trim().is_empty() {
                return Err("payment_link_id required".into());
            }
            if !p.payment_link_url.starts_with("https://") {
                return Err("payment_link_url must be an https:// URL".into());
            }
            if p.client_reference != client_reference_for(&p.order_id) {
                return Err("client_reference must name this order".into());
            }
            if p.amount_cents < 1 {
                return Err("amount_cents must be at least 1".into());
            }
            if !crate::currency::is_sealed(&p.currency) {
                return Err(format!(
                    "currency must be one of: {} (GT-D26 WORLD-PAY)",
                    crate::currency::sealed_codes_line()
                ));
            }
            validate_calendar_date(&p.minted_on, "minted_on")?;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn write_pair(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    projection::apply_event(tx, event)?;
    crate::events::insert_event(tx, event)?;
    Ok(())
}

fn load_state(tx: &Transaction<'_>, order_id: &str) -> Result<Option<String>, String> {
    tx.query_row(
        "SELECT state FROM wholesale_orders WHERE id = ?1",
        [order_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn apply_wholesale_ordered(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_wholesale_event(event)?;
    let p: OrderedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("wholesale.ordered payload refused: {e}"))?;
    tx.execute(
        "INSERT INTO wholesale_orders
         (id, venue_id, harvest_date, state, ordered_on, delivered_on, paid_on,
          income_event_id, voided_at, void_reason, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'ordered', ?4, NULL, NULL, NULL, NULL, NULL, ?5, ?5)",
        params![
            p.order_id,
            p.venue_id,
            p.harvest_date,
            p.ordered_on,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    for line in &p.lines {
        tx.execute(
            "INSERT INTO wholesale_order_lines
             (order_id, crop_id, trays, price_cents_per_tray)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                p.order_id,
                line.crop_id,
                line.trays,
                line.price_cents_per_tray,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn apply_wholesale_delivered(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_wholesale_event(event)?;
    let p: DeliveredPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("wholesale.delivered payload refused: {e}"))?;
    let state = load_state(tx, &p.order_id)?
        .ok_or_else(|| format!("wholesale.delivered: order not found: {}", p.order_id))?;
    if state != "ordered" {
        return Err(format!(
            "wholesale.delivered only from ordered, current state {state}"
        ));
    }
    let n = tx
        .execute(
            "UPDATE wholesale_orders
             SET state = 'delivered', delivered_on = ?1, updated_at = ?2
             WHERE id = ?3",
            params![p.delivered_on, event.created_at, p.order_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!(
            "wholesale.delivered: order not found: {}",
            p.order_id
        ));
    }
    Ok(())
}

pub fn apply_wholesale_paid(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_wholesale_event(event)?;
    let p: PaidPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("wholesale.paid payload refused: {e}"))?;
    let state = load_state(tx, &p.order_id)?
        .ok_or_else(|| format!("wholesale.paid: order not found: {}", p.order_id))?;
    if state != "delivered" {
        return Err(format!(
            "wholesale.paid only from delivered, current state {state}"
        ));
    }
    let n = tx
        .execute(
            "UPDATE wholesale_orders
             SET state = 'paid', paid_on = ?1, income_event_id = ?2, updated_at = ?3
             WHERE id = ?4",
            params![p.paid_on, p.income_event_id, event.created_at, p.order_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!("wholesale.paid: order not found: {}", p.order_id));
    }
    Ok(())
}

pub fn apply_wholesale_voided(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_wholesale_event(event)?;
    let p: VoidedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("wholesale.voided payload refused: {e}"))?;
    let state = load_state(tx, &p.order_id)?
        .ok_or_else(|| format!("wholesale.voided: order not found: {}", p.order_id))?;
    if state != "ordered" && state != "delivered" {
        return Err(format!(
            "wholesale.voided only from ordered or delivered, current state {state}"
        ));
    }
    let n = tx
        .execute(
            "UPDATE wholesale_orders
             SET state = 'voided', voided_at = ?1, void_reason = ?2, updated_at = ?3
             WHERE id = ?4",
            params![p.voided_at, p.reason, event.created_at, p.order_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!("wholesale.voided: order not found: {}", p.order_id));
    }
    Ok(())
}

/// Replay handler. Insert-only: the table is append-only by trigger.
pub fn apply_wholesale_write_off(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    let p: WriteOffPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("wholesale.write_off payload refused: {e}"))?;
    tx.execute(
        "INSERT INTO wholesale_write_offs
         (event_id, order_id, income_event_id, shortfall_cents, category, reason,
          written_off_on, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event.event_id,
            p.order_id,
            p.income_event_id,
            p.shortfall_cents,
            p.category,
            p.reason,
            p.written_off_on,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn apply_wholesale_payment_reversed(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    validate_wholesale_event(event)?;
    let p: PaymentReversedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("wholesale.payment_reversed payload refused: {e}"))?;
    let state = load_state(tx, &p.order_id)?.ok_or_else(|| {
        format!(
            "wholesale.payment_reversed: order not found: {}",
            p.order_id
        )
    })?;
    if state != "paid" {
        return Err(format!(
            "wholesale.payment_reversed only from paid, current state {state}"
        ));
    }
    // A guard on a BRAND NEW kind is not a replay hazard: no historic event
    // of this kind exists. This is not the III-b/III-c rule about adding
    // refusals to handlers that already have history.
    let n = tx
        .execute(
            "UPDATE wholesale_orders
             SET state = 'delivered', paid_on = NULL, income_event_id = NULL, updated_at = ?1
             WHERE id = ?2",
            params![event.created_at, p.order_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!(
            "wholesale.payment_reversed: order not found: {}",
            p.order_id
        ));
    }
    Ok(())
}

/// Replay handler. delivered → written_off. The obligation ends; the
/// delivery and the tray reservation do not. Never writes paid_on or
/// income_event_id: no payment happened.
pub fn apply_wholesale_bad_debt(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_wholesale_event(event)?;
    let p: BadDebtPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("wholesale.bad_debt payload refused: {e}"))?;
    let state = load_state(tx, &p.order_id)?
        .ok_or_else(|| format!("wholesale.bad_debt: order not found: {}", p.order_id))?;
    if state != "delivered" {
        return Err(format!(
            "wholesale.bad_debt only from delivered, current state {state}"
        ));
    }
    let n = tx
        .execute(
            "UPDATE wholesale_orders SET state = 'written_off', updated_at = ?1
         WHERE id = ?2",
            params![event.created_at, p.order_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!(
            "wholesale.bad_debt: order not found: {}",
            p.order_id
        ));
    }
    tx.execute(
        "INSERT INTO wholesale_bad_debts
         (event_id, order_id, amount_cents, written_off_on, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            event.event_id,
            p.order_id,
            p.amount_cents,
            p.written_off_on,
            event.created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// TILL-A (GT-D22). Delivered only; one link per row. Writes the three
/// link columns from the payload + event.created_at. Moves no state.
pub fn apply_wholesale_link_minted(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    validate_wholesale_event(event)?;
    let p: LinkMintedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("wholesale.link_minted payload refused: {e}"))?;
    let state = load_state(tx, &p.order_id)?
        .ok_or_else(|| format!("wholesale.link_minted: order not found: {}", p.order_id))?;
    if state != "delivered" {
        return Err(format!(
            "wholesale.link_minted only from delivered, current state {state}"
        ));
    }
    let existing: Option<String> = tx
        .query_row(
            "SELECT payment_link_id FROM wholesale_orders WHERE id = ?1",
            [&p.order_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if existing.is_some() {
        return Err("wholesale.link_minted: order already carries a payment link".into());
    }
    let n = tx
        .execute(
            "UPDATE wholesale_orders
             SET payment_link_id = ?1, payment_link_url = ?2, payment_link_minted_at = ?3, updated_at = ?3
             WHERE id = ?4",
            params![p.payment_link_id, p.payment_link_url, event.created_at, p.order_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!(
            "wholesale.link_minted: order not found: {}",
            p.order_id
        ));
    }
    Ok(())
}

fn venue_exists(conn: &Connection, venue_id: &str) -> Result<bool, String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mkt_venues WHERE venue_id = ?1",
            [venue_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

fn crop_exists(conn: &Connection, crop_id: &str) -> Result<bool, String> {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM crops WHERE id = ?1", [crop_id], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

pub fn record_order(
    conn: &mut Connection,
    venue_id: &str,
    harvest_date: &str,
    lines: Vec<OrderLine>,
    overcommit_ack: bool,
) -> Result<WholesaleOrderView, String> {
    if !venue_exists(conn, venue_id)? {
        return Err(format!("venue not found: {venue_id}"));
    }
    for line in &lines {
        if !crop_exists(conn, &line.crop_id)? {
            return Err(format!("unknown crop: {}", line.crop_id));
        }
    }
    // The upstream half of the unpriced refusal. dc47376 made an unpriced order
    // impossible to settle; this stops one being created, at the door where the
    // price is one field away instead of a void away.
    //
    // Here and NOT in validate_wholesale_event: that runs on replay, and farms
    // already carry unpriced orders. An absent price stays valid in the log
    // forever — this refuses a new one, never an old one.
    //
    // The UI gate is primary. This is the backstop for a caller that bypasses it.
    for line in &lines {
        if line.price_cents_per_tray.is_none() {
            return Err("every line needs a price per tray before an order can be recorded".into());
        }
    }
    // THE GATE. GT-D14 forbids refusing an order that runs ahead of what
    // is sown -- chefs order before sowing -- and this does not refuse
    // one: any caller proceeds by passing true. What it refuses is an
    // UNACKNOWLEDGED call. GT-D14 says "never refused AND never silent";
    // the code enforced the first half and left the second to the UI.
    // This closes the second half at the same door.
    //
    // The predicate is overcommit_line_on: the shortfall that must still
    // be SOWN, measured against shelf room, off the ONE formula. It stays
    // silent on a plain ahead-of-sowing over-commit and on an unknown
    // ceiling, so neither is gated.
    //
    // The Err carries the line VERBATIM -- no prefix, no developer
    // wording. The preview command and this gate return the same bytes,
    // so they cannot drift.
    let today = db::local_date_today();
    if !overcommit_ack {
        let mut seen = BTreeSet::new();
        let mut msgs = Vec::new();
        for line in &lines {
            if !seen.insert(line.crop_id.clone()) {
                continue;
            }
            if let Some(msg) = crate::reachability::overcommit_line_on(
                conn,
                harvest_date,
                line.trays,
                &today,
                &line.crop_id,
            )? {
                msgs.push(msg);
            }
        }
        if !msgs.is_empty() {
            return Err(msgs.join("\n"));
        }
    }
    let order_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let payload = OrderedPayload {
        order_id: order_id.clone(),
        venue_id: venue_id.to_string(),
        harvest_date: harvest_date.to_string(),
        ordered_on: today.clone(),
        lines,
        overcommit_ack,
    };
    let event = EventRecord::originated(
        Kind::WholesaleOrdered,
        "wholesale_order",
        order_id.clone(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;

    let when = format_mon_d(harvest_date)?;
    // Shelf-blocked renders the plan's sentence verbatim, so the instant
    // signal, the Today card and Health M2 give ONE instruction for one
    // fact. The other arms keep their own voice: they describe the act
    // just performed, not a standing debt.
    //
    // One attention row per overcommitted crop, not one per order. A
    // surplus in Kale cannot silence a Sunflower shortfall (D3).
    let plan = crate::reachability::cover_plan_on(conn, &today)?;
    let mut seen = BTreeSet::new();
    for line in &payload.lines {
        if !seen.insert(line.crop_id.clone()) {
            continue;
        }
        let rem = trays::cover_remaining_for(conn, harvest_date, &line.crop_id)?;
        if rem >= 0 {
            continue;
        }
        let n = crate::reachability::tray_word(-rem);
        let crop = plan
            .iter()
            .find(|c| c.harvest_date == harvest_date && c.crop_id == line.crop_id)
            .map(|c| c.crop_name.clone())
            .or_else(|| {
                conn.query_row(
                    "SELECT name FROM crops WHERE id = ?1",
                    [&line.crop_id],
                    |r| r.get(0),
                )
                .ok()
            })
            .unwrap_or_else(|| line.crop_id.clone());
        let planned = plan
            .iter()
            .find(|c| c.harvest_date == harvest_date && c.crop_id == line.crop_id);
        let message = match planned {
            Some(c) if crate::reachability::shelf_blocked(c) => c.message.clone(),
            _ => {
                let reach = crate::reachability::for_date_for_crop_on(
                    conn,
                    harvest_date,
                    &line.crop_id,
                    &today,
                )?;
                // The order just written IS the open order, so the third fact holds
                // by construction here. The other two are read from the tree.
                let settle = crate::reachability::settle_by_delivering_facts(
                    reach.days_until_harvest,
                    planned.map(|c| c.harvested_trays).unwrap_or(0),
                    true,
                );
                if reach.reachable {
                    // H
                    format!("{when} is committed {n} of {crop} beyond what is sown.")
                } else if settle {
                    // I
                    format!(
                        "{when} is committed {n} of {crop} beyond what is sown. \
                         It cannot be fixed by sowing - deliver or void the open order."
                    )
                } else {
                    // J
                    format!(
                        "{when} is committed {n} of {crop} beyond what is sown. \
                         It cannot be fixed by sowing - call the venue."
                    )
                }
            }
        };
        attention::raise(
            conn,
            "wholesale.overcommitted",
            Some("harvest_date"),
            Some(harvest_date),
            &message,
            &["dismiss"],
        )?;
    }

    get_order(conn, &order_id)
}

/// D2 extra: Delivered is for trays that have left the shelf. An order
/// whose harvest_date is still in the future has none. ONE set of bytes,
/// read by the gate below and by the Money row, so the refusal the
/// operator reads and the refusal they hit cannot drift.
pub fn deliver_refusal_line(conn: &Connection, order_id: &str) -> Result<Option<String>, String> {
    let order = get_order(conn, order_id)?;
    let today = db::local_date_today();
    if order.harvest_date.as_str() <= today.as_str() {
        return Ok(None);
    }
    let harvest =
        NaiveDate::parse_from_str(&order.harvest_date, "%Y-%m-%d").map_err(|e| e.to_string())?;
    let today_d = NaiveDate::parse_from_str(&today, "%Y-%m-%d").map_err(|e| e.to_string())?;
    let k = (harvest - today_d).num_days();
    let d = crate::reachability::format_mon_d_local(&order.harvest_date)?;
    let out = if k == 1 {
        "1 day out".to_string()
    } else {
        format!("{k} days out")
    };
    Ok(Some(format!(
        "{d} is {out} - an order cannot be delivered before its harvest date. Harvest early or void the order."
    )))
}

/// D2 extra, display side — N-1(a)/T0 signed 2026-08-23. `deliver_refusal_line`
/// above states the invariant the operator reads: "an order cannot be delivered
/// before its harvest date." A delivered row can still break it: replay and
/// bundle import apply `wholesale.delivered` through `projection::apply_event`
/// (import.rs:195, projection/kind.rs:61) with no harvest gate, and
/// `deliver_order`'s `delivered_on` argument is unbounded by ruling N-2. So the
/// readers must not count an age that cannot exist. Countable = the recorded
/// delivery is on or after the harvest date and not in the future. Not
/// countable -> every surface drops the age phrase. The row, its money and its
/// place in every list are untouched: nothing is suppressed.
/// Lexicographic compare on YYYY-MM-DD is chronological — the same idiom the
/// gate above uses at `order.harvest_date.as_str() <= today.as_str()`.
pub fn delivered_age_countable(harvest_date: &str, delivered_on: &str, today: &str) -> bool {
    harvest_date <= delivered_on && delivered_on <= today
}

pub fn deliver_order(
    conn: &mut Connection,
    order_id: &str,
    delivered_on: Option<String>,
) -> Result<WholesaleOrderView, String> {
    if let Some(msg) = deliver_refusal_line(conn, order_id)? {
        return Err(msg);
    }
    let delivered_on = match delivered_on {
        Some(d) if !d.trim().is_empty() => d,
        _ => db::local_date_today(),
    };
    let created_at = projection::handler_now();
    let payload = DeliveredPayload {
        order_id: order_id.to_string(),
        delivered_on,
    };
    let event = EventRecord::originated(
        Kind::WholesaleDelivered,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_order(conn, order_id)
}

/// R — an order with any unpriced line has no total, so there is nothing for a
/// payment to be compared against: `pay_amount_line_on` stays silent, the
/// shortfall reads 0, and both doors would settle any amount with no warning,
/// no category and no trail. That is the write-off machinery's one blind spot,
/// and it is blind exactly where the operator never priced the work.
///
/// Ruling 2026-08-16 ~11:52: refuse until priced. No write-off is invented for
/// this case — a write-off records a difference, and there is no total to
/// differ from.
///
/// The sentence names the exit because order lines are write-once: there is no
/// repricing path in the tree, so voiding and re-recording is the only route to
/// a priced order. ONE set of bytes, read by the gate below and by the Money
/// screen through `commands::unpriced_settlement_line`, so the refusal the
/// operator reads and the refusal they hit cannot drift.
pub const UNPRICED_SETTLEMENT_LINE: &str =
    "This order has a line with no price, so there is no total to settle \
     against. A price is set when the order is recorded and cannot be changed \
     afterwards: void this order with a reason, then record it again with a \
     price on every line.";
/// TILL-A (GT-D22). Read by the mint gate and by nothing else yet.
pub const MINT_NOT_DELIVERED_LINE: &str =
    "A payment link is minted after delivery. Mark the order delivered first.";
/// TILL-A (GT-D22). Two payable links for one bill is the thing this refuses.
pub const MINT_ALREADY_LINKED_LINE: &str =
    "This order already has a payment link. Copy the one on the order — a second link would be a second bill.";
/// TILL-A (GT-D22). Farm-owned client_reference_id for a wholesale bill.
/// Retail references are browser UUIDs / Date.now()-hex and never start with this.
pub const LINK_CLIENT_REFERENCE_PREFIX: &str = "wo-";
pub fn client_reference_for(order_id: &str) -> String {
    format!("{LINK_CLIENT_REFERENCE_PREFIX}{order_id}")
}
pub fn order_id_from_client_reference(reference: &str) -> Option<&str> {
    reference
        .strip_prefix(LINK_CLIENT_REFERENCE_PREFIX)
        .filter(|s| !s.is_empty())
}

fn refuse_unpriced_settlement(conn: &Connection, order_id: &str) -> Result<(), String> {
    if get_order(conn, order_id)?.priced_total_cents.is_none() {
        return Err(UNPRICED_SETTLEMENT_LINE.to_string());
    }
    Ok(())
}

/// RB3 — the one sentence that names a paid amount that does not match the
/// order. The preview command and the gate inside `pay_order` both call this,
/// so the warning the operator reads and the refusal they hit are the same
/// bytes and cannot drift (the overcommit gate's rule, applied to money).
///
/// Reads `priced_total_cents` off `get_order`, which derives it through
/// `priced_total` (this file) — the ONE total formula. No second sum.
///
/// Silent when the order has any unpriced line. There is nothing to compare
/// against, and gating on "I cannot check" would refuse a legitimate payment
/// for a fact the operator already knows — the same reason the overcommit gate
/// stays silent on an unknown ceiling. That an unpriced order still accepts any
/// amount is named as a residual in the commit, not papered over.
pub fn pay_amount_line_on(
    conn: &Connection,
    order_id: &str,
    amount_cents: i64,
) -> Result<Option<String>, String> {
    let order = get_order(conn, order_id)?;
    let Some(total) = order.priced_total_cents else {
        return Ok(None);
    };
    if amount_cents == total {
        return Ok(None);
    }
    let delta = (amount_cents - total).abs();
    let direction = if amount_cents < total { "less" } else { "more" };
    Ok(Some(format!(
        "This order is priced at {}. You are recording {} — {} {} than the \
         order. Recording it marks the order paid in full and it stops being owed.",
        crate::attention::dollars(total),
        crate::attention::dollars(amount_cents),
        crate::attention::dollars(delta),
        direction
    )))
}

/// The ONE confirm sentence for a bad-debt write-off. The gate in
/// `write_off_bad_debt` and the Money screen (through
/// `commands::bad_debt_confirm_line`) read these same bytes, so the
/// sentence the operator reads and the sentence the door enforces cannot
/// drift — the rule UNPRICED_SETTLEMENT_LINE follows (:750-754).
///
/// Signed 2026-08-20. Every clause is a fact: the delivery stands (the
/// state was `delivered` and `delivered_on` is preserved); the trays stay
/// committed (the capacity join at trays.rs:1790 excludes only 'voided');
/// no payment is recorded (no income row, no paid_on, no
/// income_event_id); it cannot be undone (inert inverse plus the undo
/// policy list at events.rs:315).
///
/// None when the order is not eligible — not delivered, or unpriced. An
/// unpriced order has no total, so there is no amount to name;
/// UNPRICED_SETTLEMENT_LINE is that refusal and this returns None rather
/// than inventing a second one.
pub fn bad_debt_confirm_line(conn: &Connection, order_id: &str) -> Result<Option<String>, String> {
    let order = get_order(conn, order_id)?;
    if order.state != "delivered" {
        return Ok(None);
    }
    let Some(total) = order.priced_total_cents else {
        return Ok(None);
    };
    Ok(Some(format!(
        "Write off {} from {} as bad debt? The delivery stands and the \
         trays stay committed. No payment is recorded. This cannot be undone.",
        crate::attention::dollars(total),
        order.venue_name
    )))
}

/// The ONE trail sentence for a written-off order. Derived from
/// `wholesale_bad_debts` on every read, so it survives a relaunch — unlike
/// the bounce trail, which is built client-side and held in React state.
/// The Money screen renders these bytes through
/// `commands::bad_debt_trail_line`; nothing is rebuilt in TypeScript.
///
/// Signed 2026-08-21. Amount and date come from the row the door wrote.
/// "The trays stay committed" is true because the capacity join at
/// trays.rs:1790 excludes only 'voided'; "no payment was recorded" is true
/// because no income row, no paid_on and no income_event_id exist.
///
/// None unless the order is written_off, so the line cannot appear on a row
/// it does not belong to.
pub fn bad_debt_trail_line(conn: &Connection, order_id: &str) -> Result<Option<String>, String> {
    let order = get_order(conn, order_id)?;
    if order.state != "written_off" {
        return Ok(None);
    }
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT amount_cents, written_off_on FROM wholesale_bad_debts
             WHERE order_id = ?1",
            [order_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((amount_cents, written_off_on)) = row else {
        return Ok(None);
    };
    let when = crate::reachability::format_mon_d_local(&written_off_on)?;
    Ok(Some(format!(
        "{} written off as bad debt on {}. The trays stay committed and no \
         payment was recorded.",
        crate::attention::dollars(amount_cents),
        when
    )))
}

#[allow(clippy::too_many_arguments)] // H-7 Class D: mirrors its Tauri command's arg list; both narrow together under H-7b.
pub fn pay_order(
    conn: &mut Connection,
    order_id: &str,
    amount_cents: i64,
    date_received: &str,
    descriptor: Option<String>,
    amount_ack: bool,
    duplicate_ack: bool,
    write_off: Option<WriteOffInput>,
) -> Result<WholesaleOrderView, String> {
    let (state, venue_name): (String, String) = conn
        .query_row(
            "SELECT o.state, v.name
             FROM wholesale_orders o
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE o.id = ?1",
            [order_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("wholesale order not found: {order_id}"))?;
    if state != "delivered" {
        return Err(format!(
            "wholesale.paid only from delivered, current state {state}"
        ));
    }
    // Refused before the amount is even considered: an amount cannot be right
    // or wrong against a total that does not exist. Before the transaction
    // opens, so a refusal writes nothing — not the income row, not the event.
    refuse_unpriced_settlement(conn, order_id)?;
    // RB3 — THE AMOUNT GATE, built to the shape of the overcommit gate in
    // `record_order` above. It refuses no payment: any caller proceeds by
    // passing true. What it refuses is an UNACKNOWLEDGED mismatch. A payment
    // that does not match the invoice is a real thing — a part payment, a
    // discount, a rounding — so blocking it would make the app refuse reality.
    // Going silent about it is the hole the Defining Audit named L11 and the
    // residual audit re-named R10: pay_order took any positive amount and never
    // looked at the total.
    //
    // The Err carries the line VERBATIM — no prefix, no developer wording. The
    // preview command and this gate call the same function, so they cannot drift.
    if !amount_ack {
        if let Some(line) = pay_amount_line_on(conn, order_id, amount_cents)? {
            return Err(line);
        }
    }
    // The second door. `settle_order_with_income` has refused a silent
    // shortfall since 8877fef; this path could still mark an order paid for
    // less than its total and write nothing. Same lie, cheaper tap — and the
    // cheaper tap is the one a hurried operator takes.
    //
    // Ruling 6: the order still goes to paid IN FULL. No partial state is
    // invented. The difference is recorded as an allowance.
    //
    // Refused BEFORE the transaction opens, so a refusal writes nothing at all —
    // not even the income row.
    let shortfall = match get_order(conn, order_id)?.priced_total_cents {
        Some(total) if amount_cents < total => total - amount_cents,
        _ => 0,
    };
    if shortfall > 0 && write_off.is_none() {
        return Err("settling for less than the order total needs a write-off category".into());
    }
    if shortfall == 0 && write_off.is_some() {
        return Err("there is no shortfall to write off on this order".into());
    }
    if !duplicate_ack {
        if let Some(line) =
            income::duplicate_income_warning(conn, &venue_name, amount_cents, date_received)?
        {
            return Err(line);
        }
    }
    let category = categories::INCOME_CATEGORIES
        .iter()
        .find(|c| c.name == "Produce you grew")
        .ok_or_else(|| {
            "income category named exactly \"Produce you grew\" is absent".to_string()
        })?;
    let created_at = projection::handler_now();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let income_event_id = income::write_received_in_tx(
        &tx,
        &venue_name,
        category.id,
        amount_cents,
        date_received,
        descriptor,
        &created_at,
    )?;
    let payload = PaidPayload {
        order_id: order_id.to_string(),
        paid_on: date_received.to_string(),
        income_event_id,
        stripe_session_id: None,
        stripe_payment_intent: None,
    };
    let event = EventRecord::originated(
        Kind::WholesalePaid,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at.clone(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    write_pair(&tx, &event)?;
    if let Some(w) = write_off {
        let wo_payload = WriteOffPayload {
            order_id: order_id.to_string(),
            income_event_id: payload.income_event_id.clone(),
            shortfall_cents: shortfall,
            category: w.category,
            reason: w.reason,
            written_off_on: date_received.to_string(),
        };
        let wo = EventRecord::originated(
            Kind::WholesaleWriteOff,
            "wholesale_order",
            order_id.to_string(),
            serde_json::to_value(&wo_payload).map_err(|e| e.to_string())?,
            json!({ "op": "none" }),
            created_at.clone(),
            None,
            None,
            Some(projection::handler_new_id()),
        );
        // Same transaction as the income row and the paid event. There is no
        // interleaving in which the order reads paid and the shortfall is
        // unrecorded, and a refused payload (Other with no reason) aborts all
        // three writes together.
        write_pair(&tx, &wo)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    get_order(conn, order_id)
}

/// TILL-A (GT-D22). Stripe first, the log second: if the local write fails
/// after Stripe answered, the retry re-sends the same idempotency keys and
/// Stripe returns the same objects — never a second payable link.
pub fn mint_payment_link(
    conn: &mut Connection,
    order_id: &str,
) -> Result<WholesaleOrderView, String> {
    let gw = crate::money::gateway_from_db(conn)?;
    mint_payment_link_with(conn, &gw, order_id)
}

pub fn mint_payment_link_with<G: crate::money::StripeGateway>(
    conn: &mut Connection,
    gateway: &G,
    order_id: &str,
) -> Result<WholesaleOrderView, String> {
    let order = get_order(conn, order_id)?;
    if order.state != "delivered" {
        return Err(MINT_NOT_DELIVERED_LINE.to_string());
    }
    if order.payment_link_id.is_some() {
        return Err(MINT_ALREADY_LINKED_LINE.to_string());
    }
    let Some(total) = order.priced_total_cents else {
        return Err(UNPRICED_SETTLEMENT_LINE.to_string());
    };
    let client_reference = client_reference_for(order_id);
    let bill = crate::money::OrderBill {
        order_id: order_id.to_string(),
        venue_name: order.venue_name.clone(),
        harvest_date: order.harvest_date.clone(),
        amount_cents: total,
        client_reference,
    };
    let minted = gateway.create_order_payment_link(&bill)?;
    let payment_link_url = format!(
        "{}?client_reference_id={}",
        minted.url, bill.client_reference
    );
    let created_at = projection::handler_now();
    let minted_on = db::local_date_from_utc_rfc3339(&created_at)?;
    let payload = LinkMintedPayload {
        order_id: order_id.to_string(),
        payment_link_id: minted.link_id,
        payment_link_url,
        client_reference: bill.client_reference,
        amount_cents: total,
        currency: minted.currency,
        minted_on,
    };
    let event = EventRecord::originated(
        Kind::WholesaleLinkMinted,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_order(conn, order_id)
}

/// TILL-B (GT-D22-B). The QR modules of `text`, row by row, `true` where the
/// module is dark. Pure: one text, one matrix. The crate picks the smallest
/// version that holds the bytes at error level M and the mask the standard
/// scores best; nothing here chooses.
pub fn qr_modules(text: &str) -> Result<Vec<Vec<bool>>, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| e.to_string())?;
    let width = code.width();
    Ok(code
        .into_colors()
        .chunks(width)
        .map(|row| row.iter().map(|c| *c == qrcode::Color::Dark).collect())
        .collect())
}

/// TILL-B (GT-D22-B). The stored `payment_link_url`, unchanged, as QR modules.
/// Reads the row; writes nothing.
pub fn payment_link_qr_modules(
    conn: &Connection,
    order_id: &str,
) -> Result<Vec<Vec<bool>>, String> {
    let order = get_order(conn, order_id)?;
    let url = order
        .payment_link_url
        .ok_or_else(|| format!("wholesale order has no payment link: {order_id}"))?;
    qr_modules(&url)
}

/// Reverse a recorded wholesale payment. One money act, one transaction.
/// A5: paying a wholesale order does not consume capacity, so this path
/// releases none.
pub fn reverse_payment(
    conn: &mut Connection,
    order_id: &str,
    reason: &str,
) -> Result<WholesaleOrderView, String> {
    let (state, income_event_id): (String, Option<String>) = conn
        .query_row(
            "SELECT state, income_event_id FROM wholesale_orders WHERE id = ?1",
            [order_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("wholesale order not found: {order_id}"))?;
    if state != "paid" {
        return Err(format!(
            "This order is {state}, not paid, so its payment cannot be reversed."
        ));
    }
    let income_event_id = income_event_id
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "this paid order has no income record to reverse".to_string())?;
    let created_at = projection::handler_now();
    let payload = PaymentReversedPayload {
        order_id: order_id.to_string(),
        income_event_id: income_event_id.clone(),
        reason: {
            let t = reason.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        },
    };
    let event = EventRecord::originated(
        Kind::WholesalePaymentReversed,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    income::void_income_in_tx(&tx, &income_event_id)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_order(conn, order_id)
}

/// The one bad-debt door. Delivered + unpaid only. Clears the money
/// obligation; the tray reservation is untouched (see the confirm line).
///
/// Confirm is mandatory and is enforced by bytes, not a flag: the caller
/// must pass back exactly what `bad_debt_confirm_line` returns for this
/// order, so a call that never rendered the sentence cannot write the
/// fact. The projection guard in apply_wholesale_bad_debt remains the hard
/// state gate.
pub fn write_off_bad_debt(
    conn: &mut Connection,
    order_id: &str,
    confirm_line: &str,
) -> Result<WholesaleOrderView, String> {
    refuse_unpriced_settlement(conn, order_id)?;
    let expected = bad_debt_confirm_line(conn, order_id)?.ok_or_else(|| {
        let state = get_order(conn, order_id)
            .map(|o| o.state)
            .unwrap_or_else(|_| "unknown".to_string());
        format!("wholesale.bad_debt only from delivered, current state {state}")
    })?;
    if confirm_line != expected {
        return Err("wholesale.bad_debt: confirmation text did not match this order".to_string());
    }
    let order = get_order(conn, order_id)?;
    let amount_cents = order
        .priced_total_cents
        .ok_or_else(|| UNPRICED_SETTLEMENT_LINE.to_string())?;
    let created_at = projection::handler_now();
    let payload = BadDebtPayload {
        order_id: order_id.to_string(),
        amount_cents,
        written_off_on: db::local_date_today(),
    };
    let event = EventRecord::originated(
        Kind::WholesaleBadDebt,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_order(conn, order_id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOffInput {
    pub category: String,
    pub reason: Option<String>,
}

/// RB3b — attach an income row that ALREADY EXISTS to a delivered order.
///
/// The double-count this closes: a deposit recorded through "Money came in" is a
/// free-floating income row; marking the order Paid later writes a SECOND income
/// row for the same money and Cash in shows both. This path writes no income row
/// at all — it names the one that already exists.
///
/// The three signed rulings, all enforced here:
///  1. Only from `delivered`. `pay_order`'s state rule is unchanged and this path
///     does not widen it. Early money stays free-floating until delivery.
///  2. Attaching settles IN FULL: the order goes to `paid` and stops being owed
///     even when the attached amount is under the priced total. No partial state,
///     no remaining balance — neither concept exists in this tree and neither is
///     invented here.
///  3. The door is on the income row; this is the write behind it.
pub fn settle_order_with_income(
    conn: &mut Connection,
    order_id: &str,
    income_event_id: &str,
    amount_ack: bool,
    write_off: Option<WriteOffInput>,
) -> Result<WholesaleOrderView, String> {
    let state: String = conn
        .query_row(
            "SELECT state FROM wholesale_orders WHERE id = ?1",
            [order_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("wholesale order not found: {order_id}"))?;
    // Ruling 1 — the same rule and the same words pay_order uses.
    if state != "delivered" {
        return Err(format!(
            "wholesale.paid only from delivered, current state {state}"
        ));
    }

    let income = crate::income::load_income_view(conn, income_event_id)
        .map_err(|_| "that income record is not readable — it may have been voided".to_string())?;

    // One income row settles ONE order. Without this the same money could be
    // applied twice and the register would under-state what is still owed — the
    // mirror of the double-count this fence exists to close. It lives here and
    // not in the UI, for the same reason the RB3a gate does.
    let already: Option<String> = conn
        .query_row(
            "SELECT id FROM wholesale_orders
             WHERE income_event_id = ?1 AND state <> 'voided'",
            [income_event_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(other) = already {
        return Err(format!("that money is already applied to order {other}"));
    }

    refuse_unpriced_settlement(conn, order_id)?;

    // The RB3a gate, reused unchanged and not bypassed. Ruling 2 makes an
    // under-payment legal; it does not make it silent. Same function, same bytes.
    if !amount_ack {
        if let Some(line) = pay_amount_line_on(conn, order_id, income.amount_cents)? {
            return Err(line);
        }
    }

    // The silent shortfall. Ruling 5: the order still goes to paid IN FULL —
    // no partial state is invented — and the difference is recorded as an
    // allowance instead of vanishing. Refused here, not only in the UI, so no
    // caller can mark an order paid while leaving the shortfall unwritten.
    let total = get_order(conn, order_id)?.priced_total_cents;
    let shortfall = match total {
        Some(t) if income.amount_cents < t => t - income.amount_cents,
        _ => 0,
    };
    if shortfall > 0 && write_off.is_none() {
        return Err("settling for less than the order total needs a write-off category".into());
    }
    if shortfall == 0 && write_off.is_some() {
        return Err("there is no shortfall to write off on this order".into());
    }

    let created_at = projection::handler_now();
    let paid_payload = PaidPayload {
        order_id: order_id.to_string(),
        paid_on: income.date_received.clone(),
        income_event_id: income_event_id.to_string(),
        stripe_session_id: None,
        stripe_payment_intent: None,
    };
    let paid = EventRecord::originated(
        Kind::WholesalePaid,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&paid_payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at.clone(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &paid)?;
    if let Some(w) = write_off {
        let payload = WriteOffPayload {
            order_id: order_id.to_string(),
            income_event_id: income_event_id.to_string(),
            shortfall_cents: shortfall,
            category: w.category,
            reason: w.reason,
            written_off_on: income.date_received.clone(),
        };
        let event = EventRecord::originated(
            Kind::WholesaleWriteOff,
            "wholesale_order",
            order_id.to_string(),
            serde_json::to_value(&payload).map_err(|e| e.to_string())?,
            json!({ "op": "none" }),
            created_at,
            None,
            None,
            Some(projection::handler_new_id()),
        );
        // Same transaction as the paid event. There is no interleaving in which
        // the order reads paid and the shortfall is unrecorded.
        write_pair(&tx, &event)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    get_order(conn, order_id)
}

/// TILL-A (GT-D22). Primary key: the baked client_reference. Secondary:
/// the session's payment_link against the row's persisted plink id.
pub(crate) fn link_session_order(
    conn: &Connection,
    client_reference: Option<&str>,
    payment_link: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(id) = client_reference.and_then(order_id_from_client_reference) {
        let exists: Option<i64> = conn
            .query_row("SELECT 1 FROM wholesale_orders WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_some() {
            return Ok(Some(id.to_string()));
        }
    }
    if let Some(plink) = payment_link {
        let found: Option<String> = conn
            .query_row(
                "SELECT id FROM wholesale_orders WHERE payment_link_id = ?1",
                [plink],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

pub(crate) fn link_payment_already_applied(
    conn: &Connection,
    session_id: &str,
) -> Result<bool, String> {
    let n: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM event_log
             WHERE kind = 'wholesale.paid'
               AND json_extract(payload, '$.stripeSessionId') = ?1
             LIMIT 1",
            [session_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(n.is_some())
}

pub(crate) fn paid_order_for_stripe_intent(
    conn: &Connection,
    payment_intent: Option<&str>,
    session_id: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(intent) = payment_intent.filter(|s| !s.is_empty()) {
        let found: Option<String> = conn
            .query_row(
                "SELECT entity_id FROM event_log
                 WHERE kind = 'wholesale.paid'
                   AND json_extract(payload, '$.stripePaymentIntent') = ?1
                 LIMIT 1",
                [intent],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if found.is_some() {
            return Ok(found);
        }
    }
    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        let found: Option<String> = conn
            .query_row(
                "SELECT entity_id FROM event_log
                 WHERE kind = 'wholesale.paid'
                   AND json_extract(payload, '$.stripeSessionId') = ?1
                 LIMIT 1",
                [sid],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        return Ok(found);
    }
    Ok(None)
}

/// TILL-A (GT-D22). The signed close: the existing income.received +
/// wholesale.paid pair in one transaction, from a Checkout Session on this
/// order's Payment Link. The session id is the dedupe key, so the
/// same-day-same-amount duplicate warning (a hand-entry guard) is not
/// consulted. Amount honesty is decided by the caller: this refuses anything
/// but the priced total.
pub(crate) fn pay_order_from_link_session(
    conn: &mut Connection,
    order_id: &str,
    session_id: &str,
    payment_intent: Option<&str>,
    amount_cents: i64,
    paid_on: &str,
) -> Result<WholesaleOrderView, String> {
    let (state, venue_name): (String, String) = conn
        .query_row(
            "SELECT o.state, v.name
             FROM wholesale_orders o
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE o.id = ?1",
            [order_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("wholesale order not found: {order_id}"))?;
    if state != "delivered" {
        return Err(format!(
            "wholesale.paid only from delivered, current state {state}"
        ));
    }
    refuse_unpriced_settlement(conn, order_id)?;
    let priced = get_order(conn, order_id)?.priced_total_cents;
    if priced != Some(amount_cents) {
        return Err("link payment does not match the order total".into());
    }
    let category = categories::INCOME_CATEGORIES
        .iter()
        .find(|c| c.name == "Produce you grew")
        .ok_or_else(|| {
            "income category named exactly \"Produce you grew\" is absent".to_string()
        })?;
    let created_at = projection::handler_now();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let income_event_id = income::write_received_in_tx(
        &tx,
        &venue_name,
        category.id,
        amount_cents,
        paid_on,
        Some(format!("Stripe payment link · {session_id}")),
        &created_at,
    )?;
    let payload = PaidPayload {
        order_id: order_id.to_string(),
        paid_on: paid_on.to_string(),
        income_event_id,
        stripe_session_id: Some(session_id.to_string()),
        stripe_payment_intent: payment_intent.map(str::to_string),
    };
    let event = EventRecord::originated(
        Kind::WholesalePaid,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_order(conn, order_id)
}

pub fn void_order(
    conn: &mut Connection,
    order_id: &str,
    reason: Option<String>,
) -> Result<WholesaleOrderView, String> {
    let created_at = projection::handler_now();
    let payload = VoidedPayload {
        order_id: order_id.to_string(),
        voided_at: created_at.clone(),
        reason,
    };
    let event = EventRecord::originated(
        Kind::WholesaleVoided,
        "wholesale_order",
        order_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_order(conn, order_id)
}

/// J1 LINK-RETIRE. The order's settle or void is committed; if it carried a
/// minted Payment Link, that link is spent — deactivate it at Stripe so the
/// page stops taking money. Best-effort, same shape as
/// offers::retire_harvest_links: the Stripe Err is dropped, the local event
/// is already committed and is never rolled back. A never-minted order has
/// nothing to retire. A late session on the same link still lands as
/// wholesale_already_settled (money.rs) — paid money is never deleted.
pub(crate) fn retire_order_link(
    gateway: &dyn crate::money::StripeGateway,
    order: &WholesaleOrderView,
) {
    if let Some(link_id) = order.payment_link_id.as_deref() {
        let _ = gateway.deactivate_link(link_id);
    }
}

/// The desk doors (Paid…, Apply income, Void): the key on file, if any. No key means no
/// Stripe to talk to and nothing to retire against; the local event stands.
pub(crate) fn retire_order_link_from_db(conn: &Connection, order: &WholesaleOrderView) {
    if order.payment_link_id.is_none() {
        return;
    }
    if let Ok(gw) = crate::money::gateway_from_db(conn) {
        retire_order_link(&gw, order);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwedSummary {
    pub deliveries: i64,
    /// Some only when EVERY delivered-unpaid order is fully priced. Never a partial sum
    /// wearing a total's clothes (same rule the Money screen already applies).
    pub total_cents: Option<i64>,
    pub any_unpriced: bool,
    pub oldest_days: Option<i64>,
    /// OWED-LO (audit R-2): priced-unpaid leftover listings, read from
    /// leftover::owed_leftover — the same evaluator Health M1 reads, so the
    /// two surfaces cannot disagree. Kept beside the wholesale figures and
    /// never folded into total_cents: total_cents keeps its delivered-order
    /// meaning, and Books' "Still owed to us" wholesale figure is unchanged.
    pub leftover_count: i64,
    pub leftover_cents: i64,
}

/// Delivered-but-unpaid, computed on demand. Nothing stored, no age frozen.
/// OWED-LO: plus the priced-unpaid leftover cents (one evaluator, above).
pub fn owed_summary(conn: &Connection) -> Result<OwedSummary, String> {
    let today = db::local_date_today();
    let mut stmt = conn
        .prepare(
            "SELECT o.id,
                    CAST(julianday(?1) - julianday(o.delivered_on) AS INTEGER) AS days,
                    (SELECT COUNT(*) FROM wholesale_order_lines l
                      WHERE l.order_id = o.id AND l.price_cents_per_tray IS NULL) AS unpriced,
                    (SELECT COALESCE(SUM(l.trays * l.price_cents_per_tray), 0)
                       FROM wholesale_order_lines l WHERE l.order_id = o.id) AS cents,
                    o.harvest_date, o.delivered_on
             FROM wholesale_orders o
             WHERE o.state = 'delivered' AND o.delivered_on IS NOT NULL",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, i64, i64, String, String)> = stmt
        .query_map([&today], |r| {
            Ok((r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    let deliveries = rows.len() as i64;
    let any_unpriced = rows.iter().any(|(_, unpriced, _, _, _)| *unpriced > 0);
    let total_cents = if deliveries == 0 || any_unpriced {
        None
    } else {
        Some(rows.iter().map(|(_, _, cents, _, _)| cents).sum())
    };
    let oldest_days = rows
        .iter()
        .filter(|(_, _, _, h, d)| delivered_age_countable(h, d, &today))
        .map(|(days, _, _, _, _)| *days)
        .max();
    let leftover = crate::leftover::owed_leftover(conn)?;
    Ok(OwedSummary {
        deliveries,
        total_cents,
        any_unpriced,
        oldest_days,
        leftover_count: leftover.count,
        leftover_cents: leftover.cents,
    })
}

pub fn list_orders(conn: &Connection) -> Result<Vec<WholesaleOrderView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT o.id, o.venue_id, v.name, o.harvest_date, o.state, o.ordered_on,
                    o.delivered_on, o.paid_on, o.income_event_id, o.voided_at,
                    o.void_reason, o.created_at, o.updated_at,
                    o.payment_link_id, o.payment_link_url, o.payment_link_minted_at
             FROM wholesale_orders o
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             ORDER BY o.ordered_on DESC, o.created_at DESC, o.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, String>(11)?,
                r.get::<_, String>(12)?,
                r.get::<_, Option<String>>(13)?,
                r.get::<_, Option<String>>(14)?,
                r.get::<_, Option<String>>(15)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let today = db::local_date_today();
    let mut out = Vec::new();
    for row in rows {
        let (
            id,
            venue_id,
            venue_name,
            harvest_date,
            state,
            ordered_on,
            delivered_on,
            paid_on,
            income_event_id,
            voided_at,
            void_reason,
            created_at,
            updated_at,
            payment_link_id,
            payment_link_url,
            payment_link_minted_at,
        ) = row.map_err(|e| e.to_string())?;
        let lines = load_lines(conn, &id)?;
        let priced_total_cents = priced_total(&lines);
        let delivered_age_days = match delivered_on.as_deref() {
            Some(d) if delivered_age_countable(&harvest_date, d, &today) => {
                match (
                    NaiveDate::parse_from_str(d, "%Y-%m-%d"),
                    NaiveDate::parse_from_str(&today, "%Y-%m-%d"),
                ) {
                    (Ok(a), Ok(b)) => Some((b - a).num_days()),
                    _ => None,
                }
            }
            _ => None,
        };
        out.push(WholesaleOrderView {
            id,
            venue_id,
            venue_name,
            harvest_date,
            state,
            ordered_on,
            delivered_on,
            paid_on,
            income_event_id,
            voided_at,
            void_reason,
            created_at,
            updated_at,
            lines,
            priced_total_cents,
            delivered_age_days,
            payment_link_id,
            payment_link_url,
            payment_link_minted_at,
        });
    }
    Ok(out)
}

/// C-2 (SOP-2): read-only. One pack per venue for today's harvest
/// date, never one per order id. Voided orders carry state 'voided'
/// and are already excluded by the state filter.
/// CUT-DATE (COMMAND B): the today pin is `pack_by_customer_on` at
/// `local_date_today()` -- one body, two doors.
pub fn pack_by_customer(conn: &Connection) -> Result<Vec<VenuePackView>, String> {
    pack_by_customer_on(conn, &db::local_date_today())
}

/// CUT-DATE (COMMAND B): the same pack, pointed at a harvest date. The
/// date passes the one calendar gate (`validate_calendar_date`, the
/// order-book's own sentences) and is echoed on every pack. Read-only:
/// no clock, no write, no new command name.
pub fn pack_by_customer_on(
    conn: &Connection,
    harvest_date: &str,
) -> Result<Vec<VenuePackView>, String> {
    validate_calendar_date(harvest_date, "harvest_date")?;
    let mut packs: BTreeMap<String, VenuePackView> = BTreeMap::new();
    for order in list_orders(conn)? {
        if order.state != "ordered" || order.harvest_date.as_str() != harvest_date {
            continue;
        }
        let pack = match packs.entry(order.venue_name.clone()) {
            Entry::Occupied(slot) => slot.into_mut(),
            Entry::Vacant(slot) => {
                let (address, phone) = venue_route_fields(conn, &order.venue_id)?;
                slot.insert(VenuePackView {
                    venue_id: order.venue_id.clone(),
                    venue_name: order.venue_name.clone(),
                    harvest_date: harvest_date.to_string(),
                    lines: Vec::new(),
                    tray_total: 0,
                    address,
                    phone,
                })
            }
        };
        for line in order.lines {
            pack.tray_total += line.trays;
            pack.lines.push(line);
        }
    }
    Ok(packs.into_values().collect())
}

/// ROUTE (ENGINE C / JOIN id): the run page's address and phone, read from
/// the venue row by venue_id -- never by venue name -- once per pack, when
/// its first order opens it. Both columns stay nullable; the paper drops an
/// absent segment. A row list_orders just joined cannot be missing, but a
/// missing row still reads as no address and no phone, never as an error.
fn venue_route_fields(
    conn: &Connection,
    venue_id: &str,
) -> Result<(Option<String>, Option<String>), String> {
    conn.query_row(
        "SELECT address, phone FROM mkt_venues WHERE venue_id = ?1",
        [venue_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())
    .map(|row| row.unwrap_or((None, None)))
}

fn load_lines(conn: &Connection, order_id: &str) -> Result<Vec<WholesaleOrderLineView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT l.crop_id, c.name, l.trays, l.price_cents_per_tray
             FROM wholesale_order_lines l
             JOIN crops c ON c.id = l.crop_id
             WHERE l.order_id = ?1
             ORDER BY c.sort_order ASC, l.crop_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([order_id], |r| {
            Ok(WholesaleOrderLineView {
                crop_id: r.get(0)?,
                crop_name: r.get(1)?,
                trays: r.get(2)?,
                price_cents_per_tray: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn priced_total(lines: &[WholesaleOrderLineView]) -> Option<i64> {
    if lines.is_empty() {
        return None;
    }
    let mut sum = 0i64;
    for line in lines {
        let price = line.price_cents_per_tray?;
        sum += price.checked_mul(line.trays)?;
    }
    Some(sum)
}

pub(crate) fn get_order(conn: &Connection, order_id: &str) -> Result<WholesaleOrderView, String> {
    list_orders(conn)?
        .into_iter()
        .find(|o| o.id == order_id)
        .ok_or_else(|| format!("wholesale order not found: {order_id}"))
}

/// Keep payload field-name consts reachable so tripwires can inventory them.
#[allow(dead_code)]
fn _payload_field_inventory() -> &'static [&'static [&'static str]] {
    &[
        ORDER_LINE_FIELD_NAMES,
        ORDERED_PAYLOAD_FIELD_NAMES,
        DELIVERED_PAYLOAD_FIELD_NAMES,
        PAID_PAYLOAD_FIELD_NAMES,
        VOIDED_PAYLOAD_FIELD_NAMES,
        WRITE_OFF_PAYLOAD_FIELD_NAMES,
        LINK_MINTED_PAYLOAD_FIELD_NAMES,
        crate::leftover::LEFTOVER_LISTED_PAYLOAD_FIELD_NAMES,
        crate::leftover::LEFTOVER_LINK_MINTED_PAYLOAD_FIELD_NAMES,
        crate::leftover::LEFTOVER_PAID_PAYLOAD_FIELD_NAMES,
    ]
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOffCategoryTotal {
    pub category: String,
    pub count: i64,
    pub total_cents: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOffSummary {
    pub total_shortfall_cents: i64,
    pub by_category: Vec<WriteOffCategoryTotal>,
}

/// W-a signed 2026-08-23 — read side only. A write-off is an allowance against
/// an order that was SETTLED for less than its priced total. When the payment
/// is reversed (`reverse_payment`, wholesale.rs:1101) the order returns to
/// `delivered` and is owed IN FULL again through `owed_summary`
/// (wholesale.rs:1390), so an allowance still counted would say the same cents
/// were both owed and forgiven.
///
/// History is untouched: the row stays (the table is append-only,
/// db.rs:472-477) and the `wholesale.write_off` event is never retired. Only
/// the two READERS skip it — the same shape as the F-B retirement at
/// money.rs:1203-1205. ONE set of bytes, used by both readers, so the summary
/// and the exported trail cannot drift apart.
///
/// The kept states are the signed pair. `written_off` is reachable for an order
/// that carries an allowance only through reverse → bad debt; the bad debt
/// itself lives in a different table (`wholesale_bad_debts`, wholesale.rs:533)
/// and no reader sums the two together.
///
/// Requires the query to alias `wholesale_orders` as `o`.
pub const WRITE_OFF_ORDER_STATE_SQL: &str = "o.state IN ('paid', 'written_off')";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOffRow {
    pub event_id: String,
    pub order_id: String,
    pub venue_name: String,
    pub shortfall_cents: i64,
    pub category: String,
    pub reason: Option<String>,
    pub written_off_on: String,
}

/// Rows behind the Write-offs lens. Same W-a predicate as the summary and the
/// export trail: a reversed payment's allowance is never counted (see the const
/// above). B-2: `written_off_on` range, inclusive at both ends, None unbounded.
pub fn write_off_rows_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<WriteOffRow>, String> {
    let sql = format!(
        "SELECT w.event_id, w.order_id, v.name, w.shortfall_cents, w.category,
                w.reason, w.written_off_on
         FROM wholesale_write_offs w
         JOIN wholesale_orders o ON o.id = w.order_id
         JOIN mkt_venues v ON v.venue_id = o.venue_id
         WHERE {WRITE_OFF_ORDER_STATE_SQL}
           AND (?1 IS NULL OR w.written_off_on >= ?1)
           AND (?2 IS NULL OR w.written_off_on <= ?2)
         ORDER BY w.written_off_on, w.order_id"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![from, to], |r| {
            Ok(WriteOffRow {
                event_id: r.get(0)?,
                order_id: r.get(1)?,
                venue_name: r.get(2)?,
                shortfall_cents: r.get(3)?,
                category: r.get(4)?,
                reason: r.get(5)?,
                written_off_on: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// The summary, folded from exactly those rows so the total and the rows cannot
/// disagree. All five WRITE_OFF_CATEGORIES are still always present and zeroed
/// when unused — the card never has to decide what a missing row means.
pub fn write_off_summary_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<WriteOffSummary, String> {
    let rows = write_off_rows_between(conn, from, to)?;
    let mut by_category = Vec::new();
    let mut total = 0i64;
    for category in WRITE_OFF_CATEGORIES {
        let matching: Vec<&WriteOffRow> = rows.iter().filter(|r| r.category == category).collect();
        let count = matching.len() as i64;
        let cents: i64 = matching.iter().map(|r| r.shortfall_cents).sum();
        total += cents;
        by_category.push(WriteOffCategoryTotal {
            category: category.to_string(),
            count,
            total_cents: cents,
        });
    }
    Ok(WriteOffSummary {
        total_shortfall_cents: total,
        by_category,
    })
}

/// Ruling 4 — the shape a future Settings card reads. A READER: it writes
/// nothing. The arithmetic now folds `write_off_rows_between`. All five
/// categories are always present, zeroed when unused, so the card never has to
/// decide what a missing row means.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn write_off_summary(conn: &Connection) -> Result<WriteOffSummary, String> {
    write_off_summary_between(conn, None, None)
}

/// B-3 signed 2026-08-23: COUNT ONLY, states `ordered` and `delivered`. No tray
/// volume in v1. An unpriced order cannot reach `paid` —
/// `refuse_unpriced_settlement` (wholesale.rs:837) blocks both settle doors —
/// so this is the whole live exposure. Voided and written-off orders are past
/// work and are not exposure.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnpricedOrderRow {
    pub id: String,
    pub venue_name: String,
    pub harvest_date: String,
    pub state: String,
}

pub fn unpriced_open_orders(conn: &Connection) -> Result<Vec<UnpricedOrderRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT o.id, v.name, o.harvest_date, o.state
             FROM wholesale_orders o
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE o.state IN ('ordered', 'delivered')
               AND EXISTS (
                   SELECT 1 FROM wholesale_order_lines l
                   WHERE l.order_id = o.id AND l.price_cents_per_tray IS NULL
               )
             ORDER BY o.harvest_date, o.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(UnpricedOrderRow {
                id: r.get(0)?,
                venue_name: r.get(1)?,
                harvest_date: r.get(2)?,
                state: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnpricedExposure {
    pub count: i64,
}

pub fn unpriced_exposure(conn: &Connection) -> Result<UnpricedExposure, String> {
    Ok(UnpricedExposure {
        count: unpriced_open_orders(conn)?.len() as i64,
    })
}

/// Lens 7. Bad debts live in `wholesale_bad_debts` (db.rs:715), a DIFFERENT
/// table from the settlement allowances in `wholesale_write_offs`. Signed
/// constraint 6: they are never summed together, and nothing in this reader
/// touches the allowance table.
///
/// There is no exclusion predicate and none is missing: a bad debt is written
/// once from `delivered` (wholesale.rs:519) and no kind un-writes it, so unlike
/// an allowance it can never be stranded by a later reversal.
///
/// B-2: `written_off_on` range, inclusive at both ends, None unbounded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BadDebtRow {
    pub event_id: String,
    pub order_id: String,
    pub venue_name: String,
    pub amount_cents: i64,
    pub written_off_on: String,
}

pub fn bad_debt_rows_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<BadDebtRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT b.event_id, b.order_id, v.name, b.amount_cents, b.written_off_on
             FROM wholesale_bad_debts b
             JOIN wholesale_orders o ON o.id = b.order_id
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE (?1 IS NULL OR b.written_off_on >= ?1)
               AND (?2 IS NULL OR b.written_off_on <= ?2)
             ORDER BY b.written_off_on, b.order_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![from, to], |r| {
            Ok(BadDebtRow {
                event_id: r.get(0)?,
                order_id: r.get(1)?,
                venue_name: r.get(2)?,
                amount_cents: r.get(3)?,
                written_off_on: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BadDebtSummary {
    pub total_cents: i64,
    pub count: i64,
}

pub fn bad_debt_summary_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<BadDebtSummary, String> {
    let rows = bad_debt_rows_between(conn, from, to)?;
    Ok(BadDebtSummary {
        total_cents: rows.iter().map(|r| r.amount_cents).sum(),
        count: rows.len() as i64,
    })
}
