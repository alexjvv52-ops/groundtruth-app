//! Leftover listing — one register kind, one table, one write door.
//!
//! Authority: GT-D24 (LO-A). A leftover listing is an operator-entered ounce
//! quantity on Money, keyed by (crop_id, harvested_on), capped by harvested
//! ounces for that crop-day, and capacity-free: it consumes no tray and
//! restores none, and it books no money (income.received is LO-B). The cap
//! is a handler gate at the write door; `apply_leftover_listed` writes only
//! what the payload froze, so replay never reads the trays.
//!
//! LO-B (GT-D24-B) adds the listing's money face: a Payment Link minted at an
//! operator-typed price (leftover.link_minted) and the poll's close
//! (leftover.paid + income.received in one transaction). Six nullable columns
//! carry it; the five LO-A columns are frozen by trigger.

use crate::db;
use crate::events::{EventRecord, Kind};
use crate::projection;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Verify-replay compares every column. The first five are written only by
/// apply_leftover_listed from the leftover.listed payload + event.created_at;
/// the six LO-B columns only by apply_leftover_link_minted / apply_leftover_paid
/// from their payloads + event.created_at.
pub const LEFTOVER_LISTINGS_COLUMNS: &[&str] = &[
    "listing_id",
    "crop_id",
    "harvested_on",
    "listed_oz",
    "created_at",
    "payment_link_id",
    "payment_link_url",
    "payment_link_minted_at",
    "priced_total_cents",
    "paid_session_id",
    "paid_at",
];

/// GT-D24 sentence 1 — the gate runs 1, then 4, then 3, then 2.
pub const LEFTOVER_NEEDS_HARVEST_LINE: &str =
    "Harvested ounces are required before leftover can be listed.";
/// GT-D24 sentence 2.
pub const LEFTOVER_OVER_HARVEST_LINE: &str =
    "Leftover ounces cannot exceed harvested ounces for that crop and day.";
/// GT-D24 sentence 3.
pub const LEFTOVER_ZERO_LINE: &str = "Leftover ounces must be greater than zero.";
/// GT-D24 sentence 4.
pub const LEFTOVER_ALREADY_LISTED_LINE: &str = "Leftover for that harvest is already listed.";

/// GT-D24-B mint sentence 1 — the mint gate runs 1, 2, 3, 4, then Stripe.
pub const LEFTOVER_UNPRICED_LINE: &str =
    "Price leftover in dollars before a payment link can be minted.";
/// GT-D24-B mint sentence 2 — an open (unpaid) link already exists.
pub const LEFTOVER_ALREADY_LINKED_LINE: &str =
    "This leftover already has a payment link. Copy the one on the listing — a second link would be a second bill.";
/// GT-D24-B mint sentence 3.
pub const LEFTOVER_ALREADY_PAID_LINE: &str = "This leftover is already paid.";
/// GT-D24-B mint sentence 4 — the key's harvested tray count is zero (4a).
pub const LEFTOVER_STALE_HARVEST_LINE: &str =
    "This leftover cannot be billed because the harvest is no longer on the books.";
/// LO-B-CASH (GT-D24-B). The cash door's empty-amount refusal (signed 6).
pub const LEFTOVER_CASH_UNPRICED_LINE: &str =
    "Price leftover in dollars before it can be marked paid.";

/// LO-B (GT-D24-B). Farm-owned client_reference_id for a leftover bill.
/// Wholesale's is "wo-" (wholesale.rs); retail references are browser UUIDs /
/// Date.now()-hex and never start with either.
pub const LEFTOVER_CLIENT_REFERENCE_PREFIX: &str = "lo-";
pub fn client_reference_for_listing(listing_id: &str) -> String {
    format!("{LEFTOVER_CLIENT_REFERENCE_PREFIX}{listing_id}")
}
pub fn listing_id_from_client_reference(reference: &str) -> Option<&str> {
    reference
        .strip_prefix(LEFTOVER_CLIENT_REFERENCE_PREFIX)
        .filter(|s| !s.is_empty())
}

/// The leftover.listed payload. Follows the columns; camelCase on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeftoverListedPayload {
    pub listing_id: String,
    pub crop_id: String,
    pub harvested_on: String,
    pub listed_oz: f64,
}

/// H-10: schema seal, kept reachable through wholesale::_payload_field_inventory.
pub const LEFTOVER_LISTED_PAYLOAD_FIELD_NAMES: &[&str] =
    &["listing_id", "crop_id", "harvested_on", "listed_oz"];

/// The leftover.link_minted payload. camelCase on the wire (B2/B4a signing:
/// pricedTotalCents).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeftoverLinkMintedPayload {
    pub listing_id: String,
    pub payment_link_id: String,
    pub payment_link_url: String,
    pub client_reference: String,
    pub priced_total_cents: i64,
    pub currency: String,
    pub minted_on: String,
}

/// H-10: schema seal, kept reachable through wholesale::_payload_field_inventory.
pub const LEFTOVER_LINK_MINTED_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "listing_id",
    "payment_link_id",
    "payment_link_url",
    "client_reference",
    "priced_total_cents",
    "currency",
    "minted_on",
];

/// The leftover.paid payload. paid_at is the local calendar day (3a), derived
/// exactly as wholesale's paid_on: local_date_from_utc_rfc3339 of the session's
/// paid timestamp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeftoverPaidPayload {
    pub listing_id: String,
    pub paid_at: String,
    pub income_event_id: String,
    /// LO-B-CASH (GT-D24-B): Some only when the poll booked this payment from
    /// a Checkout Session on the listing's own Payment Link; None on a desk
    /// cash close. The event log is the only place the session id lives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_payment_intent: Option<String>,
    /// LO-B-CASH: Some only on a cash close of a never-minted listing — the
    /// typed dollars become the listing total (signed 5). None everywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priced_total_cents: Option<i64>,
}

/// H-10: schema seal, kept reachable through wholesale::_payload_field_inventory.
pub const LEFTOVER_PAID_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "listing_id",
    "paid_at",
    "income_event_id",
    "stripe_session_id",
    "stripe_payment_intent",
    "priced_total_cents",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverListingView {
    pub listing_id: String,
    pub crop_id: String,
    pub crop_name: String,
    pub harvested_on: String,
    /// Computed at read from trays.actual_yield_oz for the key; never stored.
    pub harvested_oz: f64,
    pub listed_oz: f64,
    pub created_at: String,
    pub payment_link_id: Option<String>,
    pub payment_link_url: Option<String>,
    pub payment_link_minted_at: Option<String>,
    pub priced_total_cents: Option<i64>,
    pub paid_session_id: Option<String>,
    pub paid_at: Option<String>,
    /// LINK-CODE (VIEW A). The ISO code the listing's Payment Link was
    /// minted in, read at list time from the live leftover.link_minted
    /// event — computed at read like harvested_oz, never stored.
    pub minted_currency: Option<String>,
}

/// Ounces are stored and compared at 0.1 — the WeightPad's own precision.
fn round_tenth(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// The cap: harvested trays and their ounces for one crop-day. The count is
/// carried so "no harvest" is distinguishable from a harvest of zero.
fn harvested_oz(
    conn: &Connection,
    crop_id: &str,
    harvested_on: &str,
) -> Result<(i64, f64), String> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(actual_yield_oz), 0.0) FROM trays
         WHERE crop_id = ?1 AND harvested_on = ?2 AND state = 'harvested'",
        params![crop_id, harvested_on],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(|e| e.to_string())
}

/// The write door. Gate order (GT-D24): sentence 1, then 4, then 3, then 2.
/// Writes one leftover.listed event and its projection row in one
/// transaction. Touches no tray, no order, no capacity figure.
pub fn list_leftover(
    conn: &mut Connection,
    crop_id: &str,
    harvested_on: &str,
    listed_oz: f64,
) -> Result<LeftoverListingView, String> {
    let (harvested_trays, cap) = harvested_oz(conn, crop_id, harvested_on)?;
    if harvested_trays == 0 {
        return Err(LEFTOVER_NEEDS_HARVEST_LINE.to_string());
    }
    let existing: Option<String> = conn
        .query_row(
            "SELECT listing_id FROM leftover_listings
             WHERE crop_id = ?1 AND harvested_on = ?2",
            params![crop_id, harvested_on],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if existing.is_some() {
        return Err(LEFTOVER_ALREADY_LISTED_LINE.to_string());
    }
    if !listed_oz.is_finite() || round_tenth(listed_oz) <= 0.0 {
        return Err(LEFTOVER_ZERO_LINE.to_string());
    }
    let listed = round_tenth(listed_oz);
    if listed > round_tenth(cap) {
        return Err(LEFTOVER_OVER_HARVEST_LINE.to_string());
    }
    let listing_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let payload = LeftoverListedPayload {
        listing_id: listing_id.clone(),
        crop_id: crop_id.to_string(),
        harvested_on: harvested_on.to_string(),
        listed_oz: listed,
    };
    let event = EventRecord::originated(
        Kind::LeftoverListed,
        "leftover_listing",
        listing_id.clone(),
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
    get_listing(conn, &listing_id)
}

/// The choke-point seal (events.rs): shape only, for all three leftover kinds.
/// The harvested cap and the mint gates are the handlers'.
pub fn validate_leftover_event(event: &EventRecord) -> Result<(), String> {
    match event.kind {
        Kind::LeftoverListed => {
            let p: LeftoverListedPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("leftover.listed payload refused: {e}"))?;
            if p.listing_id.trim().is_empty() || p.crop_id.trim().is_empty() {
                return Err("leftover.listed requires listing_id and crop_id".into());
            }
            if chrono::NaiveDate::parse_from_str(&p.harvested_on, "%Y-%m-%d").is_err() {
                return Err(format!(
                    "leftover.listed harvested_on must be YYYY-MM-DD, got {}",
                    p.harvested_on
                ));
            }
            if !p.listed_oz.is_finite() || p.listed_oz <= 0.0 {
                return Err("leftover.listed listed_oz must be > 0".into());
            }
            Ok(())
        }
        Kind::LeftoverLinkMinted => {
            let p: LeftoverLinkMintedPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("leftover.link_minted payload refused: {e}"))?;
            if p.listing_id.trim().is_empty()
                || p.payment_link_id.trim().is_empty()
                || p.payment_link_url.trim().is_empty()
            {
                return Err(
                    "leftover.link_minted requires listing_id, payment_link_id and payment_link_url"
                        .into(),
                );
            }
            if p.client_reference != client_reference_for_listing(&p.listing_id) {
                return Err(
                    "leftover.link_minted client_reference must be lo- + listing_id".into(),
                );
            }
            if p.priced_total_cents <= 0 {
                return Err("leftover.link_minted priced_total_cents must be > 0".into());
            }
            if !crate::currency::is_sealed(&p.currency) {
                return Err(format!(
                    "leftover.link_minted currency must be one of: {}, got {} (GT-D26 WORLD-PAY)",
                    crate::currency::sealed_codes_line(),
                    p.currency
                ));
            }
            if chrono::NaiveDate::parse_from_str(&p.minted_on, "%Y-%m-%d").is_err() {
                return Err(format!(
                    "leftover.link_minted minted_on must be YYYY-MM-DD, got {}",
                    p.minted_on
                ));
            }
            Ok(())
        }
        Kind::LeftoverPaid => {
            let p: LeftoverPaidPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("leftover.paid payload refused: {e}"))?;
            if p.listing_id.trim().is_empty() || p.income_event_id.trim().is_empty() {
                return Err("leftover.paid requires listing_id and income_event_id".into());
            }
            if p.stripe_session_id
                .as_deref()
                .is_some_and(|s| s.trim().is_empty())
            {
                return Err(
                    "leftover.paid stripe_session_id, when present, must not be empty".into(),
                );
            }
            if p.priced_total_cents.is_some_and(|c| c <= 0) {
                return Err("leftover.paid priced_total_cents, when present, must be > 0".into());
            }
            if chrono::NaiveDate::parse_from_str(&p.paid_at, "%Y-%m-%d").is_err() {
                return Err(format!(
                    "leftover.paid paid_at must be YYYY-MM-DD, got {}",
                    p.paid_at
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Projection. Lookup-free: writes the payload fields and event.created_at.
pub fn apply_leftover_listed(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_leftover_event(event)?;
    let p: LeftoverListedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("leftover.listed payload refused: {e}"))?;
    tx.execute(
        "INSERT INTO leftover_listings (listing_id, crop_id, harvested_on, listed_oz, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            p.listing_id,
            p.crop_id,
            p.harvested_on,
            p.listed_oz,
            event.created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Projection. Lookup-free beyond the row's own guard columns: writes the
/// payload fields and event.created_at (the minted-at stamp, as wholesale).
pub fn apply_leftover_link_minted(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_leftover_event(event)?;
    let p: LeftoverLinkMintedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("leftover.link_minted payload refused: {e}"))?;
    let existing: Option<Option<String>> = tx
        .query_row(
            "SELECT payment_link_id FROM leftover_listings WHERE listing_id = ?1",
            [&p.listing_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(link) = existing else {
        return Err(format!(
            "leftover.link_minted: listing not found: {}",
            p.listing_id
        ));
    };
    if link.is_some() {
        return Err("leftover.link_minted: listing already carries a payment link".into());
    }
    tx.execute(
        "UPDATE leftover_listings
         SET payment_link_id = ?1, payment_link_url = ?2, payment_link_minted_at = ?3,
             priced_total_cents = ?4
         WHERE listing_id = ?5",
        params![
            p.payment_link_id,
            p.payment_link_url,
            event.created_at,
            p.priced_total_cents,
            p.listing_id
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Projection. Lookup-free: writes the payload fields.
pub fn apply_leftover_paid(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_leftover_event(event)?;
    let p: LeftoverPaidPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("leftover.paid payload refused: {e}"))?;
    let existing: Option<Option<String>> = tx
        .query_row(
            "SELECT paid_at FROM leftover_listings WHERE listing_id = ?1",
            [&p.listing_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(paid) = existing else {
        return Err(format!(
            "leftover.paid: listing not found: {}",
            p.listing_id
        ));
    };
    if paid.is_some() {
        return Err("leftover.paid: listing is already paid".into());
    }
    // LO-B-CASH: a cash close has no session id, so "already paid" keys on
    // paid_at (the SELECT above), and the typed total lands only when the
    // listing never carried one.
    tx.execute(
        "UPDATE leftover_listings
         SET paid_session_id = ?1, paid_at = ?2,
             priced_total_cents = COALESCE(?4, priced_total_cents)
         WHERE listing_id = ?3",
        params![
            p.stripe_session_id,
            p.paid_at,
            p.listing_id,
            p.priced_total_cents
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The mint door. Gate order (GT-D24-B): unpriced, already linked (an open,
/// unpaid link), already paid, stale harvest (4a: the key's harvested tray
/// count is zero), then Stripe. The bill's display slots carry the crop name
/// and the harvest day (1a): the checkout page reads
/// "{crop_name} · harvest {harvested_on}".
pub fn mint_payment_link(
    conn: &mut Connection,
    listing_id: &str,
    price_cents: i64,
) -> Result<LeftoverListingView, String> {
    let gw = crate::money::gateway_from_db(conn)?;
    mint_payment_link_with(conn, &gw, listing_id, price_cents)
}

pub fn mint_payment_link_with<G: crate::money::StripeGateway>(
    conn: &mut Connection,
    gateway: &G,
    listing_id: &str,
    price_cents: i64,
) -> Result<LeftoverListingView, String> {
    let listing = get_listing(conn, listing_id)?;
    if price_cents <= 0 {
        return Err(LEFTOVER_UNPRICED_LINE.to_string());
    }
    if listing.payment_link_id.is_some() && listing.paid_at.is_none() {
        return Err(LEFTOVER_ALREADY_LINKED_LINE.to_string());
    }
    if listing.paid_at.is_some() {
        return Err(LEFTOVER_ALREADY_PAID_LINE.to_string());
    }
    let (harvested_trays, _) = harvested_oz(conn, &listing.crop_id, &listing.harvested_on)?;
    if harvested_trays == 0 {
        return Err(LEFTOVER_STALE_HARVEST_LINE.to_string());
    }
    let client_reference = client_reference_for_listing(listing_id);
    let bill = crate::money::OrderBill {
        order_id: listing_id.to_string(),
        venue_name: listing.crop_name.clone(),
        harvest_date: listing.harvested_on.clone(),
        amount_cents: price_cents,
        client_reference,
    };
    let minted = gateway.create_order_payment_link(&bill)?;
    let payment_link_url = format!(
        "{}?client_reference_id={}",
        minted.url, bill.client_reference
    );
    let created_at = projection::handler_now();
    let minted_on = db::local_date_from_utc_rfc3339(&created_at)?;
    let payload = LeftoverLinkMintedPayload {
        listing_id: listing_id.to_string(),
        payment_link_id: minted.link_id,
        payment_link_url,
        client_reference: bill.client_reference,
        priced_total_cents: price_cents,
        currency: minted.currency,
        minted_on,
    };
    let event = EventRecord::originated(
        Kind::LeftoverLinkMinted,
        "leftover_listing",
        listing_id.to_string(),
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
    get_listing(conn, listing_id)
}

/// The poll close (B3): leftover.paid + income.received in one transaction.
/// Source "Leftover {cropName} · {harvestedOn}", category Produce you grew,
/// descriptor "Stripe payment link · {session_id}". paid_at is the local
/// calendar day of the session's paid timestamp (3a).
pub(crate) fn pay_listing_from_link_session(
    conn: &mut Connection,
    listing_id: &str,
    session_id: &str,
    payment_intent: Option<&str>,
    amount_cents: i64,
    paid_at_utc: &str,
) -> Result<LeftoverListingView, String> {
    let listing = get_listing(conn, listing_id)?;
    if listing.paid_at.is_some() {
        return Err(LEFTOVER_ALREADY_PAID_LINE.to_string());
    }
    let Some(total) = listing.priced_total_cents else {
        return Err("leftover.paid needs a minted, priced listing".into());
    };
    if total != amount_cents {
        return Err("link payment does not match the listing total".into());
    }
    let paid_at = db::local_date_from_utc_rfc3339(paid_at_utc)?;
    let category = crate::categories::INCOME_CATEGORIES
        .iter()
        .find(|c| c.name == "Produce you grew")
        .ok_or_else(|| {
            "income category named exactly \"Produce you grew\" is absent".to_string()
        })?;
    let source = format!("Leftover {} · {}", listing.crop_name, listing.harvested_on);
    let created_at = projection::handler_now();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let income_event_id = crate::income::write_received_in_tx(
        &tx,
        &source,
        category.id,
        amount_cents,
        &paid_at,
        Some(format!("Stripe payment link · {session_id}")),
        &created_at,
    )?;
    let payload = LeftoverPaidPayload {
        listing_id: listing_id.to_string(),
        paid_at: paid_at.clone(),
        income_event_id,
        stripe_session_id: Some(session_id.to_string()),
        stripe_payment_intent: payment_intent.map(str::to_string),
        priced_total_cents: None,
    };
    let event = EventRecord::originated(
        Kind::LeftoverPaid,
        "leftover_listing",
        listing_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    projection::apply_event(&tx, &event)?;
    crate::events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_listing(conn, listing_id)
}

/// LO-B-CASH (GT-D24-B). The desk close: cash, Venmo, a check — money that
/// arrived outside Stripe. Writes income.received + leftover.paid in ONE
/// transaction; never an orders row; never a session id. Gate order: not
/// found, already paid, empty amount, then (if minted) the total must match —
/// no write-off on leftover. A never-minted listing takes the typed dollars
/// as its total (signed 5). Cash on a minted-but-unpaid link is allowed
/// (signed 7); a late session then lands as leftover_already_paid.
pub fn pay_listing_cash(
    conn: &mut Connection,
    listing_id: &str,
    amount_cents: i64,
    date_received: &str,
    descriptor: Option<String>,
) -> Result<LeftoverListingView, String> {
    let listing = get_listing(conn, listing_id)?;
    if listing.paid_at.is_some() {
        return Err(LEFTOVER_ALREADY_PAID_LINE.to_string());
    }
    if amount_cents <= 0 {
        return Err(LEFTOVER_CASH_UNPRICED_LINE.to_string());
    }
    if let Some(total) = listing.priced_total_cents {
        if total != amount_cents {
            return Err("cash payment does not match the listing total".into());
        }
    }
    let category = crate::categories::INCOME_CATEGORIES
        .iter()
        .find(|c| c.name == "Produce you grew")
        .ok_or_else(|| {
            "income category named exactly \"Produce you grew\" is absent".to_string()
        })?;
    let source = format!("Leftover {} · {}", listing.crop_name, listing.harvested_on);
    let created_at = projection::handler_now();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let income_event_id = crate::income::write_received_in_tx(
        &tx,
        &source,
        category.id,
        amount_cents,
        date_received,
        descriptor,
        &created_at,
    )?;
    let payload = LeftoverPaidPayload {
        listing_id: listing_id.to_string(),
        paid_at: date_received.to_string(),
        income_event_id,
        stripe_session_id: None,
        stripe_payment_intent: None,
        priced_total_cents: if listing.priced_total_cents.is_none() {
            Some(amount_cents)
        } else {
            None
        },
    };
    let event = EventRecord::originated(
        Kind::LeftoverPaid,
        "leftover_listing",
        listing_id.to_string(),
        serde_json::to_value(&payload).map_err(|e| e.to_string())?,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    projection::apply_event(&tx, &event)?;
    crate::events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_listing(conn, listing_id)
}

/// J1 LINK-RETIRE. The listing's cash close is committed; if it carried a
/// minted Payment Link, that link is spent — deactivate it at Stripe so the
/// page stops taking money. Best-effort, same shape as
/// offers::retire_harvest_links: the Stripe Err is dropped, the local event
/// is already committed and is never rolled back. A never-minted listing has
/// nothing to retire. A late session on the same link still lands as
/// leftover_already_paid (money.rs) — paid money is never deleted.
pub(crate) fn retire_listing_link(
    gateway: &dyn crate::money::StripeGateway,
    listing: &LeftoverListingView,
) {
    if let Some(link_id) = listing.payment_link_id.as_deref() {
        let _ = gateway.deactivate_link(link_id);
    }
}

/// The desk door (Paid…): the key on file, if any. No key means no
/// Stripe to talk to and nothing to retire against; the local event stands.
pub(crate) fn retire_listing_link_from_db(conn: &Connection, listing: &LeftoverListingView) {
    if listing.payment_link_id.is_none() {
        return;
    }
    if let Ok(gw) = crate::money::gateway_from_db(conn) {
        retire_listing_link(&gw, listing);
    }
}

/// B3: a session on a leftover Payment Link belongs to its listing — matched
/// by the lo- reference first, then the stored payment_link_id.
pub(crate) fn link_session_listing(
    conn: &Connection,
    client_reference: Option<&str>,
    payment_link: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(id) = client_reference.and_then(listing_id_from_client_reference) {
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM leftover_listings WHERE listing_id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_some() {
            return Ok(Some(id.to_string()));
        }
    }
    if let Some(plink) = payment_link {
        let found: Option<String> = conn
            .query_row(
                "SELECT listing_id FROM leftover_listings WHERE payment_link_id = ?1",
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

/// Idempotency: the event log is the only place the session id lives.
pub(crate) fn leftover_payment_already_applied(
    conn: &Connection,
    session_id: &str,
) -> Result<bool, String> {
    let n: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM event_log
             WHERE kind = 'leftover.paid'
               AND json_extract(payload, '$.stripeSessionId') = ?1
             LIMIT 1",
            [session_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(n.is_some())
}

/// LO-C (GT-D24-C). The stored `payment_link_url`, unchanged, as QR modules.
/// Reads the row; writes nothing. The encoder is wholesale::qr_modules — one
/// encoder, one pinned matrix (till_b_tests); this door only fetches the row.
pub fn payment_link_qr_modules(
    conn: &Connection,
    listing_id: &str,
) -> Result<Vec<Vec<bool>>, String> {
    let listing = get_listing(conn, listing_id)?;
    let url = listing
        .payment_link_url
        .ok_or_else(|| format!("leftover listing has no payment link: {listing_id}"))?;
    crate::wholesale::qr_modules(&url)
}

/// OWED-LO (audit R-1/R-2). The one leftover-owed evaluator: listings that
/// are priced (priced_total_cents IS NOT NULL) and unpaid (paid_at IS NULL),
/// counted and summed. An unpriced listing has no dollar and is not owed —
/// no figure is invented from ounces (GT-D24-B). Read-only. Both owed
/// surfaces read through here — Health M1 (health.rs compute_status) and
/// the Money / Today owed line (wholesale::owed_summary) — so the desk and
/// Health cannot disagree about leftover money.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeftoverOwed {
    pub count: i64,
    pub cents: i64,
}
pub fn owed_leftover(conn: &Connection) -> Result<LeftoverOwed, String> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(priced_total_cents), 0) FROM leftover_listings
         WHERE priced_total_cents IS NOT NULL AND paid_at IS NULL",
        [],
        |r| {
            Ok(LeftoverOwed {
                count: r.get(0)?,
                cents: r.get(1)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}
/// Read side. harvested_oz is computed here from the trays, never stored, so
/// an undone harvest shows as it is: harvested 0 oz beside the listed ounces.
pub fn listings(conn: &Connection) -> Result<Vec<LeftoverListingView>, String> {
    let minted = crate::events::minted_codes(conn, Kind::LeftoverLinkMinted)?;
    let mut stmt = conn
        .prepare(
            "SELECT l.listing_id, l.crop_id, c.name, l.harvested_on, l.listed_oz, l.created_at,
                    l.payment_link_id, l.payment_link_url, l.payment_link_minted_at,
                    l.priced_total_cents, l.paid_session_id, l.paid_at,
                    (SELECT COALESCE(SUM(t.actual_yield_oz), 0.0) FROM trays t
                      WHERE t.crop_id = l.crop_id AND t.harvested_on = l.harvested_on
                        AND t.state = 'harvested') AS harvested_oz
             FROM leftover_listings l
             JOIN crops c ON c.id = l.crop_id
             ORDER BY l.harvested_on DESC, c.sort_order ASC, l.created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            let listing_id: String = r.get(0)?;
            let minted_currency = minted.get(&listing_id).cloned();
            Ok(LeftoverListingView {
                listing_id,
                crop_id: r.get(1)?,
                crop_name: r.get(2)?,
                harvested_on: r.get(3)?,
                listed_oz: round_tenth(r.get(4)?),
                created_at: r.get(5)?,
                payment_link_id: r.get(6)?,
                payment_link_url: r.get(7)?,
                payment_link_minted_at: r.get(8)?,
                priced_total_cents: r.get(9)?,
                paid_session_id: r.get(10)?,
                paid_at: r.get(11)?,
                harvested_oz: round_tenth(r.get(12)?),
                minted_currency,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub(crate) fn get_listing(
    conn: &Connection,
    listing_id: &str,
) -> Result<LeftoverListingView, String> {
    listings(conn)?
        .into_iter()
        .find(|l| l.listing_id == listing_id)
        .ok_or_else(|| format!("leftover listing not found: {listing_id}"))
}
