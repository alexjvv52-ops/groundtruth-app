//! INV-A — a Farm OS bill the operator can show. Render-only: both doors read
//! a priced parent plus the farm display name and write nothing — no Kind, no
//! table of invoices, no counter. The number IS the parent id. GT-D22 holds:
//! the only Stripe object on a bill is the stored payment_link_url, printed
//! as text.
use crate::leftover;
use crate::wholesale;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
/// INV-A, signed 5 — the environment gate, verbatim.
pub const INVOICE_NEEDS_FARM_NAME_LINE: &str =
    "Set the farm name in Settings before an invoice can be shown.";
/// INV-A, signed 4 — an unpriced parent has no total to bill.
pub const INVOICE_UNPRICED_LINE: &str = "There is no priced total to invoice yet.";
/// INV-A, signed 4 — a voided order has no bill.
pub const INVOICE_VOIDED_LINE: &str = "A voided order cannot be invoiced.";
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceVenueView {
    pub name: String,
    pub contact: Option<String>,
    pub phone: Option<String>,
    pub address: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceOrderLineView {
    pub crop_name: String,
    pub trays: i64,
    pub price_cents_per_tray: i64,
    pub line_total_cents: i64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceLeftoverLineView {
    pub crop_name: String,
    pub harvested_on: String,
    pub listed_oz: f64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceBillView {
    pub farm_name: String,
    pub number: String,
    pub parent: String,
    pub venue: Option<InvoiceVenueView>,
    pub order_lines: Vec<InvoiceOrderLineView>,
    pub leftover_line: Option<InvoiceLeftoverLineView>,
    pub harvest_date: String,
    pub delivered_on: Option<String>,
    pub total_cents: i64,
    pub paid_on: Option<String>,
    pub payment_link_url: Option<String>,
}
/// The farm's display name. Config on farm_config (id = 1), not an event.
pub fn farm_display_name(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT display_name FROM farm_config WHERE id = 1",
        [],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
    .map(|row: Option<Option<String>>| row.flatten())
}
/// Trimmed; an empty save clears the name back to NULL.
pub fn set_farm_display_name(conn: &Connection, name: &str) -> Result<Option<String>, String> {
    let trimmed = name.trim();
    let value = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    conn.execute(
        "UPDATE farm_config SET display_name = ?1 WHERE id = 1",
        rusqlite::params![value],
    )
    .map_err(|e| e.to_string())?;
    farm_display_name(conn)
}
fn required_farm_name(conn: &Connection) -> Result<String, String> {
    farm_display_name(conn)?
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| INVOICE_NEEDS_FARM_NAME_LINE.to_string())
}
/// INV-A. Gate order: not found, voided, unpriced, then the farm name.
pub fn wholesale_bill(conn: &Connection, order_id: &str) -> Result<InvoiceBillView, String> {
    let order = wholesale::get_order(conn, order_id)?;
    if order.voided_at.is_some() {
        return Err(INVOICE_VOIDED_LINE.to_string());
    }
    let Some(total) = order.priced_total_cents else {
        return Err(INVOICE_UNPRICED_LINE.to_string());
    };
    let farm_name = required_farm_name(conn)?;
    let venue = conn
        .query_row(
            "SELECT name, contact, phone, address FROM mkt_venues WHERE venue_id = ?1",
            [&order.venue_id],
            |r| {
                Ok(InvoiceVenueView {
                    name: r.get(0)?,
                    contact: r.get(1)?,
                    phone: r.get(2)?,
                    address: r.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let order_lines = order
        .lines
        .iter()
        .map(|l| {
            let unit = l.price_cents_per_tray.unwrap_or(0);
            InvoiceOrderLineView {
                crop_name: l.crop_name.clone(),
                trays: l.trays,
                price_cents_per_tray: unit,
                line_total_cents: unit * l.trays,
            }
        })
        .collect();
    Ok(InvoiceBillView {
        farm_name,
        number: order.id.clone(),
        parent: "wholesale".to_string(),
        venue,
        order_lines,
        leftover_line: None,
        harvest_date: order.harvest_date.clone(),
        delivered_on: order.delivered_on.clone(),
        total_cents: total,
        paid_on: order.paid_on.clone(),
        payment_link_url: order.payment_link_url.clone(),
    })
}
/// INV-A. Gate order: not found, unpriced, then the farm name. A leftover
/// listing has no void; a paid one still bills — the stamp says Paid.
pub fn leftover_bill(conn: &Connection, listing_id: &str) -> Result<InvoiceBillView, String> {
    let listing = leftover::get_listing(conn, listing_id)?;
    let Some(total) = listing.priced_total_cents else {
        return Err(INVOICE_UNPRICED_LINE.to_string());
    };
    let farm_name = required_farm_name(conn)?;
    Ok(InvoiceBillView {
        farm_name,
        number: listing.listing_id.clone(),
        parent: "leftover".to_string(),
        venue: None,
        order_lines: Vec::new(),
        leftover_line: Some(InvoiceLeftoverLineView {
            crop_name: listing.crop_name.clone(),
            harvested_on: listing.harvested_on.clone(),
            listed_oz: listing.listed_oz,
        }),
        harvest_date: listing.harvested_on.clone(),
        delivered_on: None,
        total_cents: total,
        paid_on: listing.paid_at.clone(),
        payment_link_url: listing.payment_link_url.clone(),
    })
}
