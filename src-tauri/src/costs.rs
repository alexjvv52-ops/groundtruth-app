//! Money-out cost events — capture at the act (Track 3).
//!
//! Authority: BOOKS-BOUNDARY §1–§2; ROADMAP §6 / §12 / Track 3.
//! Money is integer cents. date_paid is operator-entered, never inferred from
//! a physical event. created_at/updated_at come from the handler's single
//! clock read via event.created_at — apply_* reads no clock.
//! Receipts land under <farm_dir>/receipts/ before the cost event commits.

use crate::categories::{find_category, line_is_other};
use crate::db;
use crate::events::{self, EventRecord, Kind};
use crate::projection;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Columns filled from the event spine / correction path, never inventing a
/// second id scheme. Mirror of `INCOME_SPINE_COLUMNS`.
#[allow(dead_code)] // H-10: schema seal, held by cost_event_tests::h10_cost_spine_columns_are_real_and_listed.
pub const COST_SPINE_COLUMNS: &[&str] = &["last_event_id", "created_at", "updated_at", "voided_at"];

/// Every column `cost_events` projection writes.
pub const COST_EVENTS_COLUMNS: &[&str] = &[
    "event_id",
    "origin",
    "date_paid",
    "amount_cents",
    "payee",
    "canonical_category",
    "schedule_f_line",
    "schedule_c_line",
    "descriptor",
    "quantity",
    "unit_price_cents",
    "delivery_date",
    "invoice_reference",
    "receipt_file_ref",
    "last_event_id",
    "created_at",
    "updated_at",
    "voided_at",
];

/// Payload keys for cost.money_out (and the replacement body of a correction).
/// Spine-only columns `last_event_id` / `voided_at` are not payload keys —
/// same split income uses between INCOME_PAYLOAD_KEYS and INCOME_SPINE_COLUMNS.
pub const COST_EVENT_PAYLOAD_KEYS: &[&str] = &[
    "eventId",
    "origin",
    "datePaid",
    "amountCents",
    "payee",
    "canonicalCategory",
    "scheduleFLine",
    "scheduleCLine",
    "descriptor",
    "quantity",
    "unitPriceCents",
    "deliveryDate",
    "invoiceReference",
    "receiptFileRef",
    "createdAt",
    "updatedAt",
];

/// Payload keys for cost.money_out_corrected — full replacement plus target.
pub const COST_CORRECT_PAYLOAD_KEYS: &[&str] = &[
    "eventId",
    "origin",
    "targetEventId",
    "datePaid",
    "amountCents",
    "payee",
    "canonicalCategory",
    "scheduleFLine",
    "scheduleCLine",
    "descriptor",
    "quantity",
    "unitPriceCents",
    "deliveryDate",
    "invoiceReference",
    "receiptFileRef",
    "reason",
    "beforeJson",
    "afterJson",
    "beforeAmountCents",
    "afterAmountCents",
    "beforeDate",
    "afterDate",
    "beforePayee",
    "afterPayee",
    "correctedAt",
];

/// Payload keys for cost.money_out_voided — identity plus trail.
pub const COST_VOID_PAYLOAD_KEYS: &[&str] = &[
    "eventId",
    "origin",
    "targetEventId",
    "reason",
    "beforeJson",
    "beforeAmountCents",
    "beforeDate",
    "beforePayee",
    "correctedAt",
];

/// Forbidden key names — the register must never carry a derived number.
// FORBIDDEN-KEYS-BEGIN
pub const COST_FORBIDDEN_COMPUTED_KEYS: &[&str] = &[
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
    "ledger",
    "debit",
    "credit",
    "receivable",
    "invoiceTotal",
    "periodClose",
    "accrual",
];
// FORBIDDEN-KEYS-END

/// Columns of the permanent correction trail.
pub const MONEY_CORRECTIONS_COLUMNS: &[&str] = &[
    "correction_event_id",
    "target_event_id",
    "track",
    "action",
    "before_json",
    "after_json",
    "before_amount_cents",
    "after_amount_cents",
    "before_date",
    "after_date",
    "before_payee",
    "after_payee",
    "reason",
    "corrected_at",
];

/// Max receipt size. Rejected with plain language before any write.
pub const MAX_RECEIPT_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordCostInput {
    pub amount_cents: i64,
    pub payee: String,
    pub category_id: String,
    /// Operator-entered cash-basis date (YYYY-MM-DD). Defaults to today in UI.
    pub date_paid: String,
    /// Required when the category's F or C mapping is "other".
    pub descriptor: Option<String>,
    /// Absolute path to a local receipt file. Copied into receipts/ on save only.
    /// Never persisted as-is — only the content-addressed relative ref lands.
    #[serde(default)]
    pub receipt_source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostEventView {
    pub event_id: String,
    pub origin: String,
    pub date_paid: String,
    pub amount_cents: i64,
    pub payee: String,
    pub canonical_category: String,
    pub schedule_f_line: String,
    pub schedule_c_line: String,
    pub descriptor: String,
    pub receipt_file_ref: Option<String>,
    pub last_event_id: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct CorrectExpenseInput {
    pub target_event_id: String,
    pub amount_cents: i64,
    pub payee: String,
    pub category_id: String,
    pub date_paid: String,
    pub descriptor: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoneyCorrectionView {
    pub correction_event_id: String,
    pub target_event_id: String,
    pub track: String,
    pub action: String,
    pub before_json: String,
    pub after_json: Option<String>,
    pub before_amount_cents: i64,
    pub after_amount_cents: Option<i64>,
    pub before_date: String,
    pub after_date: Option<String>,
    pub before_payee: String,
    pub after_payee: Option<String>,
    pub reason: Option<String>,
    pub corrected_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptSourceInfo {
    pub file_name: String,
    pub size_bytes: u64,
}

/// Stat a picked receipt for confirmation UI. No copy. Rejects oversized files.
pub fn receipt_source_info(path: &str) -> Result<ReceiptSourceInfo, String> {
    let p = Path::new(path);
    let meta = fs::metadata(p).map_err(|_| "Could not read that receipt.".to_string())?;
    if meta.len() > MAX_RECEIPT_BYTES {
        return Err("That receipt is too large — keep it under 25 MB.".into());
    }
    let file_name = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("receipt")
        .to_string();
    Ok(ReceiptSourceInfo {
        file_name,
        size_bytes: meta.len(),
    })
}

/// `<farm_dir>/receipts/` — same root as events.jsonl.
pub fn receipts_dir(farm_dir: &Path) -> PathBuf {
    farm_dir.join("receipts")
}

/// Write receipt bytes content-addressed under receipts/. Returns relative ref
/// with forward slashes (`receipts/<sha256hex>.<ext>`).
///
/// Nothing is written until this is called (on save). Identical content dedupes.
pub fn persist_receipt(farm_dir: &Path, source_path: &Path) -> Result<String, String> {
    let meta = fs::metadata(source_path).map_err(|_| "Could not read that receipt.".to_string())?;
    if meta.len() > MAX_RECEIPT_BYTES {
        return Err("That receipt is too large — keep it under 25 MB.".into());
    }
    let bytes = fs::read(source_path).map_err(|_| "Could not read that receipt.".to_string())?;
    if (bytes.len() as u64) > MAX_RECEIPT_BYTES {
        return Err("That receipt is too large — keep it under 25 MB.".into());
    }

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let ext = sanitize_extension(source_path);
    let rel = format!("receipts/{hex}.{ext}");
    let dir = receipts_dir(farm_dir);
    fs::create_dir_all(&dir).map_err(|e| format!("Could not save receipt: {e}"))?;
    let dest = dir.join(format!("{hex}.{ext}"));
    if dest.exists() {
        return Ok(rel);
    }

    let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    {
        let mut f = File::create(&tmp).map_err(|e| format!("Could not save receipt: {e}"))?;
        f.write_all(&bytes)
            .map_err(|e| format!("Could not save receipt: {e}"))?;
        f.sync_all()
            .map_err(|e| format!("Could not save receipt: {e}"))?;
    }
    match fs::rename(&tmp, &dest) {
        Ok(()) => Ok(rel),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            // Race: another writer landed the same hash — keep existing, drop temp.
            if dest.exists() {
                Ok(rel)
            } else {
                Err(format!("Could not save receipt: {e}"))
            }
        }
    }
}

fn sanitize_extension(source_path: &Path) -> String {
    source_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            e.chars()
                .flat_map(|c| c.to_lowercase())
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        })
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "bin".into())
}

/// Record that money just left. Completes fully offline.
///
/// When a receipt path is supplied, the file is fully written under
/// `farm_dir/receipts/` BEFORE the cost_events insert commits.
pub fn record_cost(
    conn: &mut Connection,
    farm_dir: &Path,
    input: RecordCostInput,
) -> Result<CostEventView, String> {
    let category = find_category(&input.category_id)
        .ok_or_else(|| format!("unknown category: {}", input.category_id))?;

    let payee = input.payee.trim().to_string();
    if payee.is_empty() {
        return Err("payee is required".into());
    }
    if input.amount_cents <= 0 {
        return Err("amount must be positive".into());
    }

    let descriptor = input
        .descriptor
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

    validate_date_paid_format(&input.date_paid)?;

    // Single clock read for the whole write. date_paid future check uses this
    // stamp's local calendar day — never a second Local::now, and never a
    // physical-event date.
    let now = projection::handler_now();
    let today_local = db::local_date_from_utc_rfc3339(&now)?;
    if input.date_paid > today_local {
        return Err("date paid cannot be in the future".into());
    }

    // Receipt BEFORE any DB write. Failure here → no commit, no flush.
    let receipt_file_ref = match input.receipt_source_path.as_deref() {
        Some(p) if !p.trim().is_empty() => Some(persist_receipt(farm_dir, Path::new(p.trim()))?),
        _ => None,
    };

    let event_id = projection::handler_new_id();
    let payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "datePaid": input.date_paid,
        "amountCents": input.amount_cents,
        "payee": payee,
        "canonicalCategory": category.id,
        "scheduleFLine": category.schedule_f_line,
        "scheduleCLine": category.schedule_c_line,
        "descriptor": descriptor.clone(),
        "quantity": Value::Null,
        "unitPriceCents": Value::Null,
        "deliveryDate": Value::Null,
        "invoiceReference": Value::Null,
        "receiptFileRef": receipt_file_ref.clone(),
        "createdAt": now,
        "updatedAt": now,
    });

    let event = EventRecord::originated(
        Kind::CostMoneyOut,
        "cost_event",
        event_id.clone(),
        payload,
        json!({ "op": "none" }),
        now.clone(),
        None,
        None,
        Some(event_id.clone()),
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;

    load_cost_view(conn, &event.event_id)
}

fn validate_date_paid_format(date: &str) -> Result<(), String> {
    let parts: Vec<_> = date.split('-').collect();
    if parts.len() != 3 {
        return Err("date paid must be YYYY-MM-DD".into());
    }
    let y: i32 = parts[0]
        .parse()
        .map_err(|_| "date paid must be YYYY-MM-DD".to_string())?;
    let m: u32 = parts[1]
        .parse()
        .map_err(|_| "date paid must be YYYY-MM-DD".to_string())?;
    let d: u32 = parts[2]
        .parse()
        .map_err(|_| "date paid must be YYYY-MM-DD".to_string())?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || y < 1990 {
        return Err("date paid must be a real calendar day".into());
    }
    // Reject impossible days via chrono without reading "now".
    chrono::NaiveDate::from_ymd_opt(y, m, d)
        .ok_or_else(|| "date paid must be a real calendar day".to_string())?;
    Ok(())
}

fn is_cost_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::CostMoneyOut | Kind::CostMoneyOutCorrected | Kind::CostMoneyOutVoided
    )
}

fn allowed_keys(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::CostMoneyOutVoided => COST_VOID_PAYLOAD_KEYS,
        Kind::CostMoneyOutCorrected => COST_CORRECT_PAYLOAD_KEYS,
        _ => COST_EVENT_PAYLOAD_KEYS,
    }
}

/// Validate sealed key set for cost kinds.
pub fn validate_cost_payload(payload: &Value, kind: Kind) -> Result<(), String> {
    if !is_cost_kind(kind) {
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
        if COST_FORBIDDEN_COMPUTED_KEYS
            .iter()
            .any(|f| f.eq_ignore_ascii_case(key))
        {
            return Err(format!("cost register rejects computed payload key: {key}"));
        }
    }

    let required: &[&str] = match kind {
        Kind::CostMoneyOutVoided => &["eventId", "origin", "targetEventId"],
        Kind::CostMoneyOutCorrected => &[
            "eventId",
            "origin",
            "targetEventId",
            "datePaid",
            "amountCents",
            "payee",
            "canonicalCategory",
            "scheduleFLine",
            "scheduleCLine",
            "beforeAmountCents",
            "afterAmountCents",
            "beforeDate",
            "afterDate",
            "beforePayee",
            "afterPayee",
            "beforeJson",
            "afterJson",
            "correctedAt",
        ],
        _ => &[
            "eventId",
            "origin",
            "datePaid",
            "amountCents",
            "payee",
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
    // Corrections/voids are operator acts. Original cost.money_out may be
    // commercial_app; record/payload agreement is checked in validate_cost_event.
    if matches!(kind, Kind::CostMoneyOutCorrected | Kind::CostMoneyOutVoided) && origin != "farm_os"
    {
        return Err(format!(
            "{} origin must be farm_os, got {origin}",
            kind.as_str()
        ));
    }

    if kind == Kind::CostMoneyOutVoided {
        return Ok(());
    }

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
    Ok(())
}

/// Choke-point gate for a full event record of cost kinds.
pub fn validate_cost_event(event: &EventRecord) -> Result<(), String> {
    if !is_cost_kind(event.kind) {
        return Ok(());
    }
    // Corrections/voids are operator acts and must be farm_os. Original
    // cost.money_out may be commercial_app (import).
    if matches!(
        event.kind,
        Kind::CostMoneyOutCorrected | Kind::CostMoneyOutVoided
    ) && event.origin != "farm_os"
    {
        return Err(format!(
            "{} origin must be farm_os, got {}",
            event.kind.as_str(),
            event.origin
        ));
    }
    // Identity disagreement before other payload rules — mirrors the historic
    // cost.money_out apply path that tests assert on.
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
    validate_cost_payload(&event.payload, event.kind)?;
    Ok(())
}

struct CostRow {
    event_id: String,
    origin: String,
    date_paid: String,
    amount_cents: i64,
    payee: String,
    canonical_category: String,
    schedule_f_line: String,
    schedule_c_line: String,
    descriptor: String,
    receipt_file_ref: Option<String>,
    last_event_id: String,
    created_at: String,
    updated_at: String,
    voided_at: Option<String>,
}

fn load_cost_row(conn: &Connection, event_id: &str) -> Result<CostRow, String> {
    conn.query_row(
        "SELECT event_id, origin, date_paid, amount_cents, payee, canonical_category,
                schedule_f_line, schedule_c_line, descriptor, receipt_file_ref,
                last_event_id, created_at, updated_at, voided_at
         FROM cost_events WHERE event_id = ?1",
        [event_id],
        |row| {
            Ok(CostRow {
                event_id: row.get(0)?,
                origin: row.get(1)?,
                date_paid: row.get(2)?,
                amount_cents: row.get(3)?,
                payee: row.get(4)?,
                canonical_category: row.get(5)?,
                schedule_f_line: row.get(6)?,
                schedule_c_line: row.get(7)?,
                descriptor: row.get(8)?,
                receipt_file_ref: row.get(9)?,
                last_event_id: row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
                voided_at: row.get(13)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "that expense is not in the register".to_string())
}

fn load_cost_guard(conn: &Connection, event_id: &str) -> Result<CostRow, String> {
    let row = load_cost_row(conn, event_id)?;
    if row.voided_at.is_some() {
        return Err("that expense was already voided".into());
    }
    // C3 (INT-003): "never editable" is enforced where the writes happen. A
    // record from another system stays exactly as it arrived; corrections and
    // voids are operator acts on the operator's own records only.
    if row.origin != "farm_os" {
        return Err("That expense came from another system and is not editable here.".into());
    }
    if row.last_event_id.is_empty() {
        return Err("that expense is missing its last event id".into());
    }
    Ok(row)
}

fn load_cost_view(conn: &Connection, event_id: &str) -> Result<CostEventView, String> {
    let row = load_cost_row(conn, event_id)?;
    if row.voided_at.is_some() {
        return Err("that expense was already voided".into());
    }
    Ok(CostEventView {
        event_id: row.event_id,
        origin: row.origin,
        date_paid: row.date_paid,
        amount_cents: row.amount_cents,
        payee: row.payee,
        canonical_category: row.canonical_category,
        schedule_f_line: row.schedule_f_line,
        schedule_c_line: row.schedule_c_line,
        descriptor: row.descriptor,
        receipt_file_ref: row.receipt_file_ref,
        last_event_id: row.last_event_id,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

/// V-3(a) signed 2026-08-23 — the ONE expense selection, the same treatment
/// B-1(a) gave cash. `cost_events` is the only table in the tree that records
/// money leaving the bank (event_partition.rs:30). Assets carry a service date,
/// mileage is miles and consumption is units, so none of them is cash out
/// (V-1). Corrections need no netting: apply_cost_money_out_corrected replaces
/// the row's fields in place, so the live row already carries the corrected
/// amount, and voided rows are excluded outright.
///
/// B-2: `from`/`to` are local YYYY-MM-DD, INCLUSIVE at both ends, None
/// unbounded. `date_paid` is already a local date, so unlike the cash union
/// this filters entirely in SQL.
///
/// `origin = 'farm_os'` matches write_costs_csv. list_expenses omitted that
/// filter; the sets are identical in practice because costs.rs:536 and :578
/// refuse any other origin on write, and carrying it here removes the
/// difference rather than preserving it.
pub fn expense_rows_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<CostEventView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT event_id, origin, date_paid, amount_cents, payee,
                    canonical_category, schedule_f_line, schedule_c_line,
                    descriptor, receipt_file_ref, last_event_id, created_at, updated_at
             FROM cost_events
             WHERE voided_at IS NULL
               AND origin = 'farm_os'
               AND (?1 IS NULL OR date_paid >= ?1)
               AND (?2 IS NULL OR date_paid <= ?2)
             ORDER BY date_paid DESC, created_at DESC, event_id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![from, to], |row| {
            Ok(CostEventView {
                event_id: row.get(0)?,
                origin: row.get(1)?,
                date_paid: row.get(2)?,
                amount_cents: row.get(3)?,
                payee: row.get(4)?,
                canonical_category: row.get(5)?,
                schedule_f_line: row.get(6)?,
                schedule_c_line: row.get(7)?,
                descriptor: row.get(8)?,
                receipt_file_ref: row.get(9)?,
                last_event_id: row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CashOut {
    pub total_cents: i64,
    pub count: i64,
}

/// The lens-5 headline. The fold of the rows above, so the total and the
/// "show the rows" disclosure cannot disagree.
pub fn cash_out_between(
    conn: &Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<CashOut, String> {
    let rows = expense_rows_between(conn, from, to)?;
    Ok(CashOut {
        total_cents: rows.iter().map(|r| r.amount_cents).sum(),
        count: rows.len() as i64,
    })
}

/// Active expenses only — voided rows stay in the table, out of the list.
pub fn list_expenses(conn: &Connection) -> Result<Vec<CostEventView>, String> {
    expense_rows_between(conn, None, None)
}

/// Read-only correction trail.
pub fn list_money_corrections(conn: &Connection) -> Result<Vec<MoneyCorrectionView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT correction_event_id, target_event_id, track, action,
                    before_json, after_json, before_amount_cents, after_amount_cents,
                    before_date, after_date, before_payee, after_payee, reason, corrected_at
             FROM money_corrections
             ORDER BY corrected_at DESC, correction_event_id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(MoneyCorrectionView {
                correction_event_id: row.get(0)?,
                target_event_id: row.get(1)?,
                track: row.get(2)?,
                action: row.get(3)?,
                before_json: row.get(4)?,
                after_json: row.get(5)?,
                before_amount_cents: row.get(6)?,
                after_amount_cents: row.get(7)?,
                before_date: row.get(8)?,
                after_date: row.get(9)?,
                before_payee: row.get(10)?,
                after_payee: row.get(11)?,
                reason: row.get(12)?,
                corrected_at: row.get(13)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Full replacement of operator fields on an existing expense.
pub fn correct_expense(
    conn: &mut Connection,
    input: CorrectExpenseInput,
) -> Result<CostEventView, String> {
    let prior = load_cost_guard(conn, &input.target_event_id)?;
    let category = find_category(&input.category_id)
        .ok_or_else(|| format!("unknown category: {}", input.category_id))?;
    let payee = input.payee.trim().to_string();
    if payee.is_empty() {
        return Err("payee is required".into());
    }
    if input.amount_cents <= 0 {
        return Err("amount must be positive".into());
    }
    let descriptor = input
        .descriptor
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
    validate_date_paid_format(&input.date_paid)?;
    let now = projection::handler_now();
    let today_local = db::local_date_from_utc_rfc3339(&now)?;
    if input.date_paid > today_local {
        return Err("date paid cannot be in the future".into());
    }

    let before_json = json!({
        "datePaid": prior.date_paid,
        "amountCents": prior.amount_cents,
        "payee": prior.payee,
        "canonicalCategory": prior.canonical_category,
        "scheduleFLine": prior.schedule_f_line,
        "scheduleCLine": prior.schedule_c_line,
        "descriptor": prior.descriptor,
        "receiptFileRef": prior.receipt_file_ref,
    });
    let after_json = json!({
        "datePaid": input.date_paid,
        "amountCents": input.amount_cents,
        "payee": payee,
        "canonicalCategory": category.id,
        "scheduleFLine": category.schedule_f_line,
        "scheduleCLine": category.schedule_c_line,
        "descriptor": descriptor,
        "receiptFileRef": prior.receipt_file_ref,
    });

    let event_id = projection::handler_new_id();
    let reason = input
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let mut payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "targetEventId": input.target_event_id,
        "datePaid": input.date_paid,
        "amountCents": input.amount_cents,
        "payee": payee,
        "canonicalCategory": category.id,
        "scheduleFLine": category.schedule_f_line,
        "scheduleCLine": category.schedule_c_line,
        "descriptor": descriptor,
        "quantity": Value::Null,
        "unitPriceCents": Value::Null,
        "deliveryDate": Value::Null,
        "invoiceReference": Value::Null,
        "receiptFileRef": prior.receipt_file_ref,
        "beforeJson": before_json.to_string(),
        "afterJson": after_json.to_string(),
        "beforeAmountCents": prior.amount_cents,
        "afterAmountCents": input.amount_cents,
        "beforeDate": prior.date_paid,
        "afterDate": input.date_paid,
        "beforePayee": prior.payee,
        "afterPayee": payee,
        "correctedAt": now,
    });
    if let Some(r) = &reason {
        payload
            .as_object_mut()
            .unwrap()
            .insert("reason".into(), json!(r));
    }
    validate_cost_payload(&payload, Kind::CostMoneyOutCorrected)?;

    let event = EventRecord::originated(
        Kind::CostMoneyOutCorrected,
        "cost_event",
        input.target_event_id.clone(),
        payload,
        json!({ "op": "none" }),
        now,
        None,
        Some(&prior.last_event_id),
        Some(event_id),
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    load_cost_view(conn, &input.target_event_id)
}

/// Retire an expense entered in error. Row survives, marked voided.
pub fn void_expense(
    conn: &mut Connection,
    target_event_id: &str,
    reason: Option<String>,
) -> Result<(), String> {
    let prior = load_cost_guard(conn, target_event_id)?;
    let now = projection::handler_now();
    let event_id = projection::handler_new_id();
    let reason = reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let before_json = json!({
        "datePaid": prior.date_paid,
        "amountCents": prior.amount_cents,
        "payee": prior.payee,
        "canonicalCategory": prior.canonical_category,
        "scheduleFLine": prior.schedule_f_line,
        "scheduleCLine": prior.schedule_c_line,
        "descriptor": prior.descriptor,
        "receiptFileRef": prior.receipt_file_ref,
    });
    let mut payload = json!({
        "eventId": event_id,
        "origin": "farm_os",
        "targetEventId": target_event_id,
        "beforeJson": before_json.to_string(),
        "beforeAmountCents": prior.amount_cents,
        "beforeDate": prior.date_paid,
        "beforePayee": prior.payee,
        "correctedAt": now,
    });
    if let Some(r) = &reason {
        payload
            .as_object_mut()
            .unwrap()
            .insert("reason".into(), json!(r));
    }
    validate_cost_payload(&payload, Kind::CostMoneyOutVoided)?;

    let event = EventRecord::originated(
        Kind::CostMoneyOutVoided,
        "cost_event",
        target_event_id,
        payload,
        json!({ "op": "none" }),
        now,
        None,
        Some(&prior.last_event_id),
        Some(event_id),
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Projection: identity from the event record; other fields from payload.
/// Payload still carries eventId/origin copies — disagreement is Err, never
/// silently resolved. No clock.
pub fn apply_cost_money_out(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_cost_event(event)?;
    let p = &event.payload;
    let date_paid = req_str(p, "datePaid")?;
    let amount_cents = req_i64(p, "amountCents")?;
    let payee = req_str(p, "payee")?;
    let canonical_category = req_str(p, "canonicalCategory")?;
    let schedule_f_line = req_str(p, "scheduleFLine")?;
    let schedule_c_line = req_str(p, "scheduleCLine")?;
    let descriptor = req_str(p, "descriptor")?;
    let quantity = opt_i64(p, "quantity");
    let unit_price_cents = opt_i64(p, "unitPriceCents");
    let delivery_date = opt_str(p, "deliveryDate");
    let invoice_reference = opt_str(p, "invoiceReference");
    let receipt_file_ref = opt_str(p, "receiptFileRef");
    let created_at = &event.created_at;

    tx.execute(
        "INSERT INTO cost_events
         (event_id, origin, date_paid, amount_cents, payee, canonical_category,
          schedule_f_line, schedule_c_line, descriptor, quantity, unit_price_cents,
          delivery_date, invoice_reference, receipt_file_ref, last_event_id,
          created_at, updated_at, voided_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?1, ?15, ?15, NULL)",
        params![
            event.event_id,
            event.origin,
            date_paid,
            amount_cents,
            payee,
            canonical_category,
            schedule_f_line,
            schedule_c_line,
            descriptor,
            quantity,
            unit_price_cents,
            delivery_date,
            invoice_reference,
            receipt_file_ref,
            created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Projection: update cost_events in place and append money_corrections. No clock.
pub fn apply_cost_money_out_corrected(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    validate_cost_event(event)?;
    let p = &event.payload;
    let target = req_str(p, "targetEventId")?;
    if target != event.entity_id {
        return Err("cost.money_out_corrected targetEventId must equal entity_id".into());
    }
    let date_paid = req_str(p, "datePaid")?;
    let amount_cents = req_i64(p, "amountCents")?;
    let payee = req_str(p, "payee")?;
    let canonical_category = req_str(p, "canonicalCategory")?;
    let schedule_f_line = req_str(p, "scheduleFLine")?;
    let schedule_c_line = req_str(p, "scheduleCLine")?;
    let descriptor = p.get("descriptor").and_then(|v| v.as_str()).unwrap_or("");
    let receipt = opt_str(p, "receiptFileRef");

    let n = tx
        .execute(
            "UPDATE cost_events SET date_paid = ?1, amount_cents = ?2, payee = ?3,
                    canonical_category = ?4, schedule_f_line = ?5, schedule_c_line = ?6,
                    descriptor = ?7, receipt_file_ref = ?8, last_event_id = ?9, updated_at = ?10
             WHERE event_id = ?11 AND voided_at IS NULL",
            params![
                date_paid,
                amount_cents,
                payee,
                canonical_category,
                schedule_f_line,
                schedule_c_line,
                descriptor,
                receipt,
                event.event_id,
                event.created_at,
                target,
            ],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err("cost.money_out_corrected names an expense that is not active".into());
    }

    insert_money_correction(
        tx,
        &MoneyCorrectionRow {
            correction_event_id: event.event_id.as_str(),
            target_event_id: target,
            track: "cost",
            action: "corrected",
            before_json: req_str(p, "beforeJson")?,
            after_json: Some(req_str(p, "afterJson")?),
            before_amount_cents: req_i64(p, "beforeAmountCents")?,
            after_amount_cents: Some(req_i64(p, "afterAmountCents")?),
            before_date: req_str(p, "beforeDate")?,
            after_date: Some(req_str(p, "afterDate")?),
            before_payee: req_str(p, "beforePayee")?,
            after_payee: Some(req_str(p, "afterPayee")?),
            reason: opt_str(p, "reason"),
            corrected_at: req_str(p, "correctedAt")?,
        },
    )?;
    Ok(())
}

/// Projection: mark voided and append money_corrections. No clock.
pub fn apply_cost_money_out_voided(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    validate_cost_event(event)?;
    let p = &event.payload;
    let target = req_str(p, "targetEventId")?;
    if target != event.entity_id {
        return Err("cost.money_out_voided targetEventId must equal entity_id".into());
    }
    let n = tx
        .execute(
            "UPDATE cost_events SET voided_at = ?1, last_event_id = ?2, updated_at = ?3
             WHERE event_id = ?4 AND voided_at IS NULL",
            params![event.created_at, event.event_id, event.created_at, target,],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err("cost.money_out_voided names an expense that is not active".into());
    }

    insert_money_correction(
        tx,
        &MoneyCorrectionRow {
            correction_event_id: event.event_id.as_str(),
            target_event_id: target,
            track: "cost",
            action: "voided",
            before_json: req_str(p, "beforeJson")?,
            after_json: None,
            before_amount_cents: req_i64(p, "beforeAmountCents")?,
            after_amount_cents: None,
            before_date: req_str(p, "beforeDate")?,
            after_date: None,
            before_payee: req_str(p, "beforePayee")?,
            after_payee: None,
            reason: opt_str(p, "reason"),
            corrected_at: req_str(p, "correctedAt")?,
        },
    )?;
    Ok(())
}

/// H-7b Fence 2 - the row this writer inserts, named. Field order is the old
/// parameter order, so the mapping is positional and nothing was reordered.
///
/// Fields are borrowed, not owned. This struct is write-only and never
/// outlives its call: both callers build it from borrows of the event payload
/// they already hold, so this land allocates nothing it did not allocate
/// before.
struct MoneyCorrectionRow<'a> {
    correction_event_id: &'a str,
    target_event_id: &'a str,
    track: &'a str,
    action: &'a str,
    before_json: &'a str,
    after_json: Option<&'a str>,
    before_amount_cents: i64,
    after_amount_cents: Option<i64>,
    before_date: &'a str,
    after_date: Option<&'a str>,
    before_payee: &'a str,
    after_payee: Option<&'a str>,
    reason: Option<&'a str>,
    corrected_at: &'a str,
}
fn insert_money_correction(
    tx: &Transaction<'_>,
    row: &MoneyCorrectionRow<'_>,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO money_corrections
         (correction_event_id, target_event_id, track, action,
          before_json, after_json, before_amount_cents, after_amount_cents,
          before_date, after_date, before_payee, after_payee, reason, corrected_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            row.correction_event_id,
            row.target_event_id,
            row.track,
            row.action,
            row.before_json,
            row.after_json,
            row.before_amount_cents,
            row.after_amount_cents,
            row.before_date,
            row.after_date,
            row.before_payee,
            row.after_payee,
            row.reason,
            row.corrected_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn req_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("cost.money_out payload missing {key}"))
}

fn opt_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| if x.is_null() { None } else { x.as_str() })
}

fn req_i64(v: &Value, key: &str) -> Result<i64, String> {
    v.get(key)
        .and_then(|x| x.as_i64())
        .ok_or_else(|| format!("cost.money_out payload missing {key}"))
}

fn opt_i64(v: &Value, key: &str) -> Option<i64> {
    v.get(key)
        .and_then(|x| if x.is_null() { None } else { x.as_i64() })
}

#[cfg(test)]
mod column_payload_tests {
    use super::{COST_EVENTS_COLUMNS, COST_EVENT_PAYLOAD_KEYS, COST_SPINE_COLUMNS};

    #[test]
    fn spine_columns_are_named_and_present() {
        assert_eq!(COST_SPINE_COLUMNS.len(), 4);
        for col in COST_SPINE_COLUMNS {
            assert!(
                COST_EVENTS_COLUMNS.contains(col),
                "COST_EVENTS_COLUMNS missing spine column {col}"
            );
        }
        // Original payload keys stay paired; spine-only last_event_id/voided_at
        // are the two columns beyond the historical payload key set.
        assert_eq!(COST_EVENTS_COLUMNS.len(), COST_EVENT_PAYLOAD_KEYS.len() + 2);
    }
}
