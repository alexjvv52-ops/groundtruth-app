//! Derived cost per tray — computed on request, never stored (Track 5).
//!
//! Authority: ROADMAP Track 5 done-whens; BOOKS-BOUNDARY outranks.
//! This module READS ONLY. It takes &Connection, never &mut Connection and
//! never a Transaction. It contains no INSERT, UPDATE, DELETE, CREATE, DROP
//! or ALTER. The number it returns is not a fact in the system of record and
//! must never be written to a table, an event payload, or a config file.
//!
//! It reads exactly two tables — cost_events and consumption_events — because
//! those are the exported records. It must NOT read trays, orders, event_log
//! or crops: `trays` is current state, not an exported record, and counting it
//! would fold recounts and discards into a denominator that is supposed to be
//! "trays brought into being".
//!
//! Denominator rule is not a Track 5 choice. consumption.rs already ruled it
//! above UNIT_PLANTING: tray count comes from surviving consumption.physical
//! records with unit = 'tray'. Harvest records carry 'planting' precisely so
//! they cannot double the denominator. Do not "simplify" this.

use crate::categories;
use crate::consumption::{UNIT_OZ, UNIT_TRAY};
use crate::db;
use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncludedPayment {
    pub event_id: String,
    pub date_paid: String,
    pub payee: String,
    pub canonical_category: String,
    pub amount_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncludedTrayRecord {
    pub event_id: String,
    /// occurred_at converted to the operator's local calendar day.
    pub occurred_on: String,
    pub variety_or_item: String,
    pub quantity: f64,
    pub seed_quantity_recorded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodStatement {
    pub window_label: String,
    pub window_from: String,
    pub window_to: String,
    pub origin_filter: String, // always "farm_os"
    /// Generated at query time from the actual parameters. NOT a constant.
    pub payment_rule: String,
    pub physical_rule: String,
    pub join_rule: String,
    pub exclusion_rule: String,
    pub payments: Vec<IncludedPayment>,
    pub tray_records: Vec<IncludedTrayRecord>,
    pub payment_count: i64,
    pub tray_record_count: i64,
    pub total_paid_cents: i64,
    pub total_trays: f64,
    pub tray_records_with_seed_recorded: i64,
    pub tray_records_without_seed_recorded: i64,
    pub completeness_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostPerTrayFigure {
    pub total_paid_cents: i64,
    pub total_trays: f64,
    /// Derived at query time. Never stored. f64 because money divided by
    /// trays is not money and must not masquerade as integer cents.
    pub cents_per_tray: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CostPerTrayOutcome {
    Computed {
        figure: CostPerTrayFigure,
        method: MethodStatement,
    },
    /// A refusal still carries the full method statement — the operator must
    /// be able to see what was looked at even when no number is possible.
    Refused {
        reason: String,
        method: MethodStatement,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostPerTrayRequest {
    /// "last_30" | "last_90" | "ytd" | "all" | "custom"
    pub window: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Query-time narrowing chosen by the operator. NEVER persisted.
    pub category_ids: Option<Vec<String>>,
}

fn parse_ymd(s: &str) -> Result<NaiveDate, String> {
    let parts: Vec<_> = s.split('-').collect();
    if parts.len() != 3 || s.len() != 10 {
        return Err("start date must be YYYY-MM-DD".into());
    }
    let y: i32 = parts[0]
        .parse()
        .map_err(|_| "start date must be YYYY-MM-DD".to_string())?;
    let m: u32 = parts[1]
        .parse()
        .map_err(|_| "start date must be YYYY-MM-DD".to_string())?;
    let d: u32 = parts[2]
        .parse()
        .map_err(|_| "start date must be YYYY-MM-DD".to_string())?;
    NaiveDate::from_ymd_opt(y, m, d).ok_or_else(|| "start date must be YYYY-MM-DD".into())
}

fn format_ymd(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn earliest_tray_local_day(conn: &Connection) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT occurred_at FROM consumption_events
             WHERE origin = 'farm_os' AND unit = ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![UNIT_TRAY], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut earliest: Option<String> = None;
    for row in rows {
        let at = row.map_err(|e| e.to_string())?;
        let local = db::local_date_from_utc_rfc3339(&at)?;
        earliest = Some(match earliest {
            None => local,
            Some(prev) if local < prev => local,
            Some(prev) => prev,
        });
    }
    Ok(earliest)
}

fn resolve_window(
    conn: &Connection,
    req: &CostPerTrayRequest,
) -> Result<(String, String, String), String> {
    let now = db::utc_now_rfc3339();
    let today = db::local_date_from_utc_rfc3339(&now)?;
    let today_d = parse_ymd(&today)?;

    match req.window.as_str() {
        "last_30" => {
            let from_d = today_d - chrono::Duration::days(29);
            Ok((format_ymd(from_d), today.clone(), "the last 30 days".into()))
        }
        "last_90" => {
            let from_d = today_d - chrono::Duration::days(89);
            Ok((format_ymd(from_d), today.clone(), "the last 90 days".into()))
        }
        "ytd" => {
            let from_d = NaiveDate::from_ymd_opt(today_d.year(), 1, 1)
                .ok_or_else(|| "start date must be YYYY-MM-DD".to_string())?;
            Ok((format_ymd(from_d), today.clone(), "this year so far".into()))
        }
        "all" => {
            let min_paid: Option<String> = conn
                .query_row(
                    "SELECT MIN(date_paid) FROM cost_events WHERE origin = 'farm_os' AND voided_at IS NULL",
                    [],
                    |r| r.get::<_, Option<String>>(0),
                )
                .map_err(|e| e.to_string())?;
            let min_tray = earliest_tray_local_day(conn)?;
            let from = match (min_paid, min_tray) {
                (None, None) => today.clone(),
                (Some(p), None) => p,
                (None, Some(t)) => t,
                (Some(p), Some(t)) => {
                    if p <= t {
                        p
                    } else {
                        t
                    }
                }
            };
            Ok((from, today.clone(), "everything you have recorded".into()))
        }
        "custom" => {
            let from_s = req
                .from
                .as_deref()
                .ok_or_else(|| "start date must be YYYY-MM-DD".to_string())?;
            let to_s = req
                .to
                .as_deref()
                .ok_or_else(|| "start date must be YYYY-MM-DD".to_string())?;
            let from_d = parse_ymd(from_s)?;
            let to_d = parse_ymd(to_s)?;
            if to_d < from_d {
                return Err("end date cannot be before the start date".into());
            }
            if to_d > today_d {
                return Err("end date cannot be in the future".into());
            }
            let from = format_ymd(from_d);
            let to = format_ymd(to_d);
            Ok((from.clone(), to.clone(), format!("{from} to {to}")))
        }
        _ => Err("unknown window".into()),
    }
}

fn effective_category_ids(req: &CostPerTrayRequest) -> Option<&[String]> {
    match &req.category_ids {
        Some(ids) if !ids.is_empty() => Some(ids.as_slice()),
        _ => None,
    }
}

fn load_payments(
    conn: &Connection,
    from: &str,
    to: &str,
    category_ids: Option<&[String]>,
) -> Result<Vec<IncludedPayment>, String> {
    match category_ids {
        None => {
            let mut stmt = conn
                .prepare(
                    "SELECT event_id, date_paid, payee, canonical_category, amount_cents
                     FROM cost_events
                     WHERE origin = 'farm_os' AND voided_at IS NULL
                       AND date_paid >= ?1 AND date_paid <= ?2
                     ORDER BY date_paid, event_id",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![from, to], |r| {
                    Ok(IncludedPayment {
                        event_id: r.get(0)?,
                        date_paid: r.get(1)?,
                        payee: r.get(2)?,
                        canonical_category: r.get(3)?,
                        amount_cents: r.get(4)?,
                    })
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())
        }
        Some(ids) => {
            let placeholders: String = (0..ids.len())
                .map(|i| format!("?{}", i + 3))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT event_id, date_paid, payee, canonical_category, amount_cents
                 FROM cost_events
                 WHERE origin = 'farm_os' AND voided_at IS NULL
                   AND date_paid >= ?1 AND date_paid <= ?2
                   AND canonical_category IN ({placeholders})
                 ORDER BY date_paid, event_id"
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let mut values: Vec<rusqlite::types::Value> =
                vec![from.to_string().into(), to.to_string().into()];
            for id in ids {
                values.push(id.clone().into());
            }
            let rows = stmt
                .query_map(rusqlite::params_from_iter(values), |r| {
                    Ok(IncludedPayment {
                        event_id: r.get(0)?,
                        date_paid: r.get(1)?,
                        payee: r.get(2)?,
                        canonical_category: r.get(3)?,
                        amount_cents: r.get(4)?,
                    })
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())
        }
    }
}

struct RawTrayRow {
    event_id: String,
    occurred_at: String,
    variety_or_item: String,
    quantity: f64,
    sow_event_id: Option<String>,
}

fn load_tray_rows(
    conn: &Connection,
    from: &str,
    to: &str,
) -> Result<Vec<(RawTrayRow, String)>, String> {
    // unit = 'tray' only. 'planting' (harvest) would double the
    // denominator; 'oz' is seed, not trays. consumption.rs BOOKS-BOUNDARY §3.
    let from_d = parse_ymd(from)?;
    let to_d = parse_ymd(to)?;
    let sql_from = format_ymd(from_d - chrono::Duration::days(1));
    let sql_to = format_ymd(to_d + chrono::Duration::days(1));

    let mut stmt = conn
        .prepare(
            "SELECT event_id, occurred_at, variety_or_item, quantity, sow_event_id
             FROM consumption_events
             WHERE origin = 'farm_os' AND unit = ?1
               AND substr(occurred_at, 1, 10) >= ?2
               AND substr(occurred_at, 1, 10) <= ?3
             ORDER BY occurred_at, event_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![UNIT_TRAY, sql_from, sql_to], |r| {
            Ok(RawTrayRow {
                event_id: r.get(0)?,
                occurred_at: r.get(1)?,
                variety_or_item: r.get(2)?,
                quantity: r.get(3)?,
                sow_event_id: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut kept = Vec::new();
    for row in rows {
        let row = row.map_err(|e| e.to_string())?;
        let local = db::local_date_from_utc_rfc3339(&row.occurred_at)?;
        if local.as_str() >= from && local.as_str() <= to {
            kept.push((row, local));
        }
    }
    Ok(kept)
}

fn sow_ids_with_seed_oz(conn: &Connection) -> Result<std::collections::HashSet<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT sow_event_id FROM consumption_events
             WHERE origin = 'farm_os' AND unit = ?1 AND sow_event_id IS NOT NULL",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![UNIT_OZ], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut set = std::collections::HashSet::new();
    for row in rows {
        set.insert(row.map_err(|e| e.to_string())?);
    }
    Ok(set)
}

fn category_names(ids: &[String]) -> String {
    let cats = categories::list_categories();
    ids.iter()
        .filter_map(|id| cats.iter().find(|c| &c.id == id).map(|c| c.name.clone()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// H-7b Fence 3a - the ten values this builder needs, named. Field order is
/// the old parameter order, so the mapping is positional and nothing was
/// reordered.
///
/// Taken by value, not by reference. `payments` and `tray_records` are moved
/// into the MethodStatement this returns, so the struct has to own them.
///
/// The body unpacks it on the first line. Every operator sentence below
/// interpolates bare identifiers by implicit named capture, and `{input.from}`
/// is not format syntax, so unpacking is what keeps those sentences
/// byte-identical.
struct MethodInputs<'a> {
    from: &'a str,
    to: &'a str,
    label: &'a str,
    category_ids: Option<&'a [String]>,
    payments: Vec<IncludedPayment>,
    tray_records: Vec<IncludedTrayRecord>,
    total_paid_cents: i64,
    total_trays: f64,
    with_seed: i64,
    without_seed: i64,
}
fn build_method(input: MethodInputs<'_>) -> MethodStatement {
    let MethodInputs {
        from,
        to,
        label,
        category_ids,
        payments,
        tray_records,
        total_paid_cents,
        total_trays,
        with_seed,
        without_seed,
    } = input;

    let origin_filter = "farm_os".to_string();
    let payment_count = payments.len() as i64;
    let tray_record_count = tray_records.len() as i64;

    let payment_rule = match category_ids {
        None => format!(
            "Every payment you recorded with a date paid from {from} to {to}. \
             {payment_count} payments, {origin_filter} records only. No \
             category was excluded."
        ),
        Some(ids) => {
            let names = category_names(ids);
            format!(
                "Payments you recorded with a date paid from {from} to {to}, \
                 narrowed to: {names}. {payment_count} payments, {origin_filter} \
                 records only. You chose this narrowing for this calculation; it \
                 is not saved."
            )
        }
    };

    let physical_rule = format!(
        "Every tray you brought into being by sowing, dated {from} to \
         {to} by your local calendar day. {tray_record_count} sow records, \
         {total_trays} trays. Harvest records are not counted here — \
         counting them would count the same tray twice."
    );

    let join_rule = "No payment is matched to any particular tray. This is the total \
         you paid across the window divided by the total trays you started \
         across the same window."
        .to_string();

    let exclusion_rule = "Miles and equipment are not in this number. Miles are recorded \
         in miles and carry no dollar value. Putting equipment in would \
         mean deciding how to spread its cost over time, which this \
         software does not do."
        .to_string();

    let completeness_note = if without_seed == 0 {
        "Every sow record in this window also recorded a seed quantity.".into()
    } else {
        format!(
            "{with_seed} of {tray_record_count} sow records in this window also recorded a \
             seed quantity. The other {without_seed} did not. That seed is \
             unknown, not zero. It is not part of this number either way."
        )
    };

    MethodStatement {
        window_label: label.to_string(),
        window_from: from.to_string(),
        window_to: to.to_string(),
        origin_filter,
        payment_rule,
        physical_rule,
        join_rule,
        exclusion_rule,
        payments,
        tray_records,
        payment_count,
        tray_record_count,
        total_paid_cents,
        total_trays,
        tray_records_with_seed_recorded: with_seed,
        tray_records_without_seed_recorded: without_seed,
        completeness_note,
    }
}

pub fn cost_per_tray(
    conn: &Connection,
    req: CostPerTrayRequest,
) -> Result<CostPerTrayOutcome, String> {
    let (from, to, label) = resolve_window(conn, &req)?;
    let cats = effective_category_ids(&req);

    let payments = load_payments(conn, &from, &to, cats)?;
    let total_paid_cents: i64 = payments.iter().map(|p| p.amount_cents).sum();
    let payment_count = payments.len() as i64;

    let raw_trays = load_tray_rows(conn, &from, &to)?;
    // Completeness disclosure only. It NEVER changes the numerator or the
    // denominator.
    let seed_sows = sow_ids_with_seed_oz(conn)?;

    let mut tray_records = Vec::with_capacity(raw_trays.len());
    let mut total_trays = 0.0_f64;
    let mut with_seed: i64 = 0;
    let mut without_seed: i64 = 0;
    for (row, occurred_on) in raw_trays {
        let seed_quantity_recorded =
            matches!(&row.sow_event_id, Some(id) if seed_sows.contains(id));
        if seed_quantity_recorded {
            with_seed += 1;
        } else {
            without_seed += 1;
        }
        total_trays += row.quantity;
        tray_records.push(IncludedTrayRecord {
            event_id: row.event_id,
            occurred_on,
            variety_or_item: row.variety_or_item,
            quantity: row.quantity,
            seed_quantity_recorded,
        });
    }

    let method = build_method(MethodInputs {
        from: &from,
        to: &to,
        label: &label,
        category_ids: cats,
        payments,
        tray_records,
        total_paid_cents,
        total_trays,
        with_seed,
        without_seed,
    });

    if total_trays <= 0.0 {
        return Ok(CostPerTrayOutcome::Refused {
            reason: "No trays were recorded in this window. There is nothing \
                     to divide by."
                .into(),
            method,
        });
    }
    if payment_count == 0 {
        return Ok(CostPerTrayOutcome::Refused {
            reason: "No payments were recorded in this window. A number here \
                     would say your trays were free, and that is not something the log \
                     knows."
                .into(),
            method,
        });
    }

    let cents_per_tray = (total_paid_cents as f64) / total_trays;
    Ok(CostPerTrayOutcome::Computed {
        figure: CostPerTrayFigure {
            total_paid_cents,
            total_trays,
            cents_per_tray,
        },
        method,
    })
}
