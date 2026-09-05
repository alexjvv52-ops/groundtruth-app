//! Money state machine: confirm-then-consume orders, refunds, disputes.
//!
//! ABSOLUTE STOP: never record an order, payment, or stock movement that also
//! exists in the commercial app. Farm OS owns upstream of harvest; the
//! commercial app owns everything downstream. One crossing, one direction,
//! once per harvest.

use crate::attention;
use crate::db;
use crate::events;
use crate::events::{EventRecord, Kind};
use crate::models::{AppliedOutcome, MoneyStatus, OrderView, StripeAccountPreview};
use crate::projection;
use crate::stripe_client::{self, StripeClient};
use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Live Stripe keys are refused. Flipping this is a deliberate code change,
/// reviewed and rebuilt — never a setting, never a checkbox, never a flag file.
pub const ALLOW_LIVE_KEYS: bool = false;

/// Verify-replay compares every column (apply_stripe_fact_unapplied).
pub const STRIPE_UNAPPLIED_FACTS_COLUMNS: &[&str] = &[
    "event_id",
    "stripe_object",
    "stripe_id",
    "status",
    "amount_cents",
    "currency",
    "stripe_created",
    "observed_at",
];

// --- Gateway boundary ------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AccountInfo {
    pub account_id: String,
    pub account_name: String,
    pub mode: String,
}

#[allow(dead_code)] // Prompt 3 creates offers against this shape.
#[derive(Debug, Clone)]
pub struct Offer {
    pub id: String,
    pub harvest_date: String,
    pub crop_id: String,
    pub price_cents: i64,
    pub stripe_price_id: Option<String>,
    pub stripe_link_id: Option<String>,
    pub stripe_link_url: Option<String>,
    pub created_at: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct StripeLink {
    pub price_id: String,
    pub link_id: String,
    pub url: String,
}

/// TILL-A (GT-D22): one bill, one Payment Link. The names are for the Stripe
/// dashboard only — attribution is the baked client_reference and the link
/// id, never metadata.
#[derive(Debug, Clone)]
pub struct OrderBill {
    pub order_id: String,
    pub venue_name: String,
    pub harvest_date: String,
    pub amount_cents: i64,
    pub client_reference: String,
}

#[derive(Debug, Clone)]
pub struct MintedLink {
    pub link_id: String,
    pub url: String,
}

/// One Checkout Session line item, identified by Stripe Price id.
/// Attribution to crop/harvest happens via local `offers.stripe_price_id` — never metadata.
#[derive(Debug, Clone)]
pub struct PaidLine {
    pub price_id: String,
    pub quantity: i64,
    pub amount_cents: i64,
}

#[derive(Debug, Clone)]
pub struct PaidSession {
    pub session_id: String,
    pub payment_intent: Option<String>,
    /// Lines with quantity ≥ 1. Zeroed adjustable-quantity lines are omitted.
    pub lines: Vec<PaidLine>,
    pub currency: String,
    pub customer_email: Option<String>,
    pub paid_at: String,
    /// Unix seconds — used as the poll cursor.
    pub created: i64,
    /// Session total (for Attention copy). Sum of line amounts when known.
    pub amount_cents: i64,
    /// Browser-minted cart reference (`client_reference_id` on the Checkout Session).
    pub client_reference: Option<String>,
    /// TILL-A (GT-D22): the Payment Link the session came from (`plink_…`),
    /// when Stripe reports one. Secondary match key; the baked client_reference is primary.
    pub payment_link: Option<String>,
}

/// Legacy harvest Payment Link line — unused after cart checkout; kept for FakeGateway.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct HarvestLinkLine {
    pub price_id: String,
    pub max_quantity: i64,
}

#[derive(Debug, Clone)]
pub struct RefundRecord {
    pub refund_id: String,
    pub payment_intent: Option<String>,
    pub session_id: Option<String>,
    pub created: i64,
    /// Stripe's amount in cents. None when Stripe did not report one —
    /// unknown, never zero.
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    /// Stripe's own status. C2 (INT-002): read by the refund gate — only
    /// `succeeded` may reach the order book; `pending` / `requires_action`
    /// hold the poll cursor; `failed` / `canceled` are traced as terminal.
    /// Still not a column: stripe_unapplied_facts.status stays Groundtruth's
    /// refusal reason, and the Stripe word rides on the trace payload as
    /// `stripeStatus`.
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DisputeRecord {
    pub dispute_id: String,
    pub payment_intent: Option<String>,
    pub session_id: Option<String>,
    pub created: i64,
    /// Stripe's amount in cents. None when Stripe did not report one —
    /// unknown, never zero.
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    /// Parsed per the enrichment ruling but not yet stored: the
    /// stripe_unapplied_facts.status column is Groundtruth's refusal
    /// reason, not Stripe's, so persisting this needs its own schema arm
    /// and a ruling on staleness (a dispute status changes after we
    /// observe it). Deferred residual.
    #[allow(dead_code)]
    pub status: Option<String>,
}

/// A Checkout Session that was paid but could not be parsed into a PaidSession.
#[derive(Debug, Clone)]
pub struct UnparsedSession {
    pub session_id: String,
    pub created: i64,
    pub amount_cents: i64,
    pub currency: String,
    /// Plain English — never raw JSON.
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct SessionPage {
    pub parsed: Vec<PaidSession>,
    pub unparsed: Vec<UnparsedSession>,
}

impl SessionPage {
    pub fn from_parsed(parsed: Vec<PaidSession>) -> Self {
        Self {
            parsed,
            unparsed: vec![],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.parsed.is_empty() && self.unparsed.is_empty()
    }
}

/// Result of applying a refund or dispute fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactOutcome {
    Applied,
    AlreadyApplied,
    /// Fact arrived before its paid session — safe no-op; poll must not advance past it.
    AwaitingOrder,
    /// C2 (INT-002): Stripe reports the refund as not yet terminal
    /// (`pending` / `requires_action`). Traced, never applied; the poll holds
    /// its cursor at this refund so it is re-read until Stripe settles it.
    Deferred,
}

pub trait StripeGateway: Send + Sync {
    fn account(&self) -> Result<AccountInfo, String>;
    /// Create a Stripe Product+Price for one crop offer. Returns `price_id`.
    fn create_price(&self, offer: &Offer) -> Result<String, String>;
    /// Legacy harvest Payment Link creator — unused after cart checkout; kept for FakeGateway tests.
    #[allow(dead_code)]
    fn create_harvest_payment_link(
        &self,
        harvest_date: &str,
        lines: &[HarvestLinkLine],
    ) -> Result<(String, String), String>;
    /// Used once on shop generate to deactivate stale harvest Payment Links, then idle.
    fn deactivate_link(&self, link_id: &str) -> Result<(), String>;
    /// TILL-A (GT-D22): Product + Price + Payment Link for one wholesale bill.
    fn create_order_payment_link(&self, bill: &OrderBill) -> Result<MintedLink, String>;
    fn list_paid_sessions(&self, since: Option<&str>) -> Result<Vec<PaidSession>, String>;
    fn list_refunds(&self, since: Option<&str>) -> Result<Vec<RefundRecord>, String>;
    fn list_disputes(&self, since: Option<&str>) -> Result<Vec<DisputeRecord>, String>;

    /// Page-sized lists. Default wraps `list_paid_sessions` with no unparsed rows.
    fn list_paid_session_pages(&self, since: Option<&str>) -> Result<Vec<SessionPage>, String> {
        Ok(vec![SessionPage::from_parsed(
            self.list_paid_sessions(since)?,
        )])
    }
    fn list_refund_pages(&self, since: Option<&str>) -> Result<Vec<Vec<RefundRecord>>, String> {
        Ok(vec![self.list_refunds(since)?])
    }
    fn list_dispute_pages(&self, since: Option<&str>) -> Result<Vec<Vec<DisputeRecord>>, String> {
        Ok(vec![self.list_disputes(since)?])
    }
}

// --- Key validation and farm-file storage ----------------------------------

/// Validate key shape and test-mode lock. Returns `"test"` or `"live"`.
/// Never logs or echoes the key.
pub fn validate_restricted_key(key: &str) -> Result<&'static str, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Paste a restricted key to connect Stripe.".to_string());
    }
    if key.starts_with("sk_") {
        return Err(
            "That's a secret key. Farm OS needs a restricted key, which can do less if it ever leaks."
                .to_string(),
        );
    }
    if !key.starts_with("rk_") {
        return Err(
            "Farm OS needs a restricted key (it starts with rk_). Create one in the Stripe Dashboard."
                .to_string(),
        );
    }
    if key.starts_with("rk_live_") {
        if !ALLOW_LIVE_KEYS {
            return Err("Farm OS is in test mode. Live keys are not accepted yet.".to_string());
        }
        return Ok("live");
    }
    if key.starts_with("rk_test_") {
        return Ok("test");
    }
    Err(
        "Farm OS needs a restricted key (rk_test_… or rk_live_…). Create one in the Stripe Dashboard."
            .to_string(),
    )
}

/// Call Stripe `account()`, show the grower what they connected — do not store yet.
pub fn preview_stripe_key(conn: &Connection, key: &str) -> Result<StripeAccountPreview, String> {
    let mode = validate_restricted_key(key)?;
    let key = key.trim();
    let client = StripeClient::with_key(key, mode);
    let account = client
        .account()
        .map_err(|e| stripe_client::redact_secrets(&e, key))?;
    refuse_if_account_mismatch(conn, &account)?;
    Ok(StripeAccountPreview {
        account_id: account.account_id,
        account_name: account.account_name,
        mode: account.mode,
    })
}

/// After the grower confirms the account name/id, write the key into the farm file.
pub fn confirm_stripe_key(conn: &Connection, key: &str) -> Result<MoneyStatus, String> {
    let mode = validate_restricted_key(key)?;
    let key = key.trim();
    let client = StripeClient::with_key(key, mode);
    let account = client
        .account()
        .map_err(|e| stripe_client::redact_secrets(&e, key))?;
    store_stripe_key(conn, key, &account)?;
    money_status(conn)
}

/// Testable path: validate + store an already-resolved account (no network).
pub(crate) fn store_stripe_key(
    conn: &Connection,
    key: &str,
    account: &AccountInfo,
) -> Result<(), String> {
    let mode = validate_restricted_key(key)?;
    let key = key.trim();
    if account.mode != mode {
        return Err("The key's mode did not match the Stripe account.".to_string());
    }
    refuse_if_account_mismatch(conn, account)?;

    let now = db::utc_now_rfc3339();
    conn.execute(
        "UPDATE stripe_config
         SET restricted_key = ?1,
             account_id = ?2,
             account_name = ?3,
             mode = ?4,
             configured_at = ?5
         WHERE id = 1",
        params![key, account.account_id, account.account_name, mode, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn refuse_if_account_mismatch(conn: &Connection, account: &AccountInfo) -> Result<(), String> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT account_id FROM stripe_config WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();

    if let Some(existing) = stored {
        if !existing.is_empty() && existing != account.account_id {
            let message = format!(
                "A different Stripe account ({}) was offered. Farm OS refused it so a sale is never recorded twice.",
                account.account_id
            );
            attention::raise(
                conn,
                "stripe.account_mismatch",
                Some("stripe"),
                Some(&account.account_id),
                &message,
                &["dismiss"],
            )?;
            return Err(
                "That key belongs to a different Stripe account than the one already connected. Farm OS refused it."
                    .to_string(),
            );
        }
    }
    Ok(())
}

/// GT-D23. The one sentence for a missing key, returned by every caller of
/// `gateway_from_db`; the door it names is the Connect Stripe button on Money.
pub const STRIPE_NOT_CONNECTED_LINE: &str =
    "Stripe is not connected yet. Use Connect Stripe on Money to paste a restricted key.";

#[allow(dead_code)] // Prompt 3/4 poll loop.
pub fn gateway_from_db(conn: &Connection) -> Result<StripeClient<stripe_client::UreqHttp>, String> {
    let (key, mode): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT restricted_key, mode FROM stripe_config WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or((None, None));
    let key = key
        .filter(|k| !k.is_empty())
        .ok_or_else(|| STRIPE_NOT_CONNECTED_LINE.to_string())?;
    let mode = mode.unwrap_or_else(|| "test".to_string());
    validate_restricted_key(&key)?;
    Ok(StripeClient::with_key(&key, &mode))
}

// --- Confirm-then-consume --------------------------------------------------

struct ResolvedLine {
    crop_id: String,
    quantity: i64,
    amount_cents: i64,
}

/// Look up crop_id + harvest_date from local offers by Stripe price id.
/// Never reads Stripe metadata for attribution.
fn resolve_lines(
    conn: &Connection,
    session: &PaidSession,
) -> Result<Result<(String, Vec<ResolvedLine>), String>, String> {
    let mut resolved = Vec::new();
    let mut harvest_date: Option<String> = None;
    for line in &session.lines {
        if line.quantity < 1 {
            continue;
        }
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT crop_id, harvest_date FROM offers WHERE stripe_price_id = ?1",
                [&line.price_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((crop_id, hd)) = row else {
            return Ok(Err(format!("no local offer for price {}", line.price_id)));
        };
        match &harvest_date {
            None => harvest_date = Some(hd.clone()),
            Some(existing) if existing != &hd => {
                return Ok(Err(
                    "lines in one session pointed at different harvest dates".into(),
                ));
            }
            Some(_) => {}
        }
        resolved.push(ResolvedLine {
            crop_id,
            quantity: line.quantity,
            amount_cents: line.amount_cents,
        });
    }
    if resolved.is_empty() {
        return Ok(Err("session has no line item quantity".into()));
    }
    Ok(Ok((harvest_date.unwrap(), resolved)))
}

pub(crate) fn apply_paid_session(
    conn: &mut Connection,
    session: &PaidSession,
) -> Result<AppliedOutcome, String> {
    // TILL-A (GT-D22): a session on this farm's own Payment Link belongs to
    // the wholesale order it bills — never to the retail order book.
    if let Some(order_id) = crate::wholesale::link_session_order(
        conn,
        session.client_reference.as_deref(),
        session.payment_link.as_deref(),
    )? {
        return apply_wholesale_link_session(conn, session, &order_id);
    }
    // LO-B (GT-D24-B): a session on a leftover Payment Link belongs to its
    // listing — wo- first, then lo-, then retail (B3).
    if let Some(listing_id) = crate::leftover::link_session_listing(
        conn,
        session.client_reference.as_deref(),
        session.payment_link.as_deref(),
    )? {
        return apply_leftover_link_session(conn, session, &listing_id);
    }
    let resolved = match resolve_lines(conn, session)? {
        Ok(r) => r,
        Err(reason) => {
            let amount = format_money_amount(session.amount_cents, &session.currency);
            let message = format!(
                "A payment of {amount} arrived that Farm OS couldn't match to a crop and harvest date."
            );
            let _ = reason;
            let now = projection::handler_now();
            attention::raise_once_at(
                conn,
                "stripe.unrecognised_session",
                Some("stripe_session"),
                Some(&session.session_id),
                &message,
                &["open_in_stripe", "dismiss"],
                &now,
            )?;
            record_unapplied_fact(
                conn,
                "checkout_session",
                &session.session_id,
                "unmatched",
                Some(session.amount_cents),
                Some(&session.currency),
                session.created,
            )?;
            return Ok(AppliedOutcome::Rejected {
                session_id: session.session_id.clone(),
            });
        }
    };
    let (harvest_date, lines) = resolved;

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // Idempotency gate: a session (or cart client_reference) already recorded
    // means this exact fact was already applied — never insert it twice.
    if session_already_recorded(&tx, session)? {
        drop(tx);
        return Ok(AppliedOutcome::AlreadyApplied);
    }

    let now = projection::handler_now();
    let mut order_ids = Vec::new();
    let mut line_payloads = Vec::new();
    for line in &lines {
        let order_id = projection::handler_new_id();
        line_payloads.push(json!({
            "orderId": order_id,
            "cropId": line.crop_id,
            "quantity": line.quantity,
            "amountCents": line.amount_cents,
        }));
        order_ids.push(order_id);
    }

    let payload = json!({
        "orderIds": order_ids,
        "sessionId": session.session_id,
        "paymentIntent": session.payment_intent,
        "harvestDate": harvest_date,
        "lines": line_payloads,
        "amountCents": session.amount_cents,
        "currency": session.currency,
        "customerEmail": session.customer_email,
        "paidAt": session.paid_at,
        "clientReference": session.client_reference,
    });
    let event = EventRecord::originated(
        Kind::StripeSessionPaid,
        "stripe_session",
        session.session_id.clone(),
        payload,
        json!({ "op": "none" }),
        now.clone(),
        None,
        None,
        Some(projection::handler_new_id()),
    );

    if let Err(reason) = projection::apply_event(&tx, &event) {
        drop(tx);
        let amount = format_money_amount(session.amount_cents, &session.currency);
        let message = format!("A payment for {amount} couldn't be recorded: {reason}.");
        attention::raise_once_at(
            conn,
            "order.unrecorded",
            Some("stripe_session"),
            Some(&session.session_id),
            &message,
            &["open_in_stripe", "dismiss"],
            &now,
        )?;
        record_unapplied_fact(
            conn,
            "checkout_session",
            &session.session_id,
            "unrecorded",
            Some(session.amount_cents),
            Some(&session.currency),
            session.created,
        )?;
        return Ok(AppliedOutcome::Rejected {
            session_id: session.session_id.clone(),
        });
    }

    // B5 — one formula, per crop. A surplus in one crop must not silence
    // another crop's deficit (D21a). `&tx` derefs to &Connection; if inference
    // complains, write `&*tx`.
    let caps = crate::trays::capacity_by_harvest_date(&tx)?;
    let mut seen = BTreeSet::new();
    for line in &lines {
        if !seen.insert(line.crop_id.clone()) {
            continue;
        }
        let rem = crate::trays::remaining_exact_for(&tx, &harvest_date, &line.crop_id)?;
        if rem >= 0 {
            continue;
        }
        let by = -rem;
        let oversold = oversold_word(by);
        let date_label = format_short_month_day(&harvest_date);
        let reach = crate::reachability::for_date_for_crop(&tx, &harvest_date, &line.crop_id)?;
        let harvested_trays = caps
            .iter()
            .find(|r| r.harvest_date == harvest_date && r.crop_id == line.crop_id)
            .map(|r| r.harvested_trays)
            .unwrap_or(0);
        let has_open_order =
            crate::reachability::orders_on_date_for_crop(&tx, &harvest_date, &line.crop_id)?
                .iter()
                .any(|o| o.state == "ordered");
        let crop = &reach.crop_name;
        let message = if reach.reachable {
            format!("{date_label} is oversold by {oversold} of {crop}.")
        } else if crate::reachability::settle_by_delivering_facts(
            reach.days_until_harvest,
            harvested_trays,
            has_open_order,
        ) {
            format!(
                "{date_label} is oversold by {oversold} of {crop}. \
                 It cannot be fixed by sowing - deliver or void the open order."
            )
        } else {
            format!(
                "{date_label} is oversold by {oversold} of {crop}. \
                 It cannot be fixed by sowing - call the venue."
            )
        };
        attention::raise_in_tx_at(
            &tx,
            "order.oversold",
            Some("harvest_date"),
            Some(&format!("{}|{}", harvest_date, line.crop_id)),
            &message,
            &["dismiss"],
            &now,
        )?;
    }

    events::insert_event(&tx, &event)?;

    tx.commit().map_err(|e| e.to_string())?;
    Ok(AppliedOutcome::Applied {
        order_id: order_ids[0].clone(),
    })
}

/// TILL-A (GT-D22). Never inserts an orders row. Books cash only through
/// wholesale::pay_order_from_link_session; everything else is a named fact.
fn apply_wholesale_link_session(
    conn: &mut Connection,
    session: &PaidSession,
    order_id: &str,
) -> Result<AppliedOutcome, String> {
    if crate::wholesale::link_payment_already_applied(conn, &session.session_id)? {
        return Ok(AppliedOutcome::AlreadyApplied);
    }
    let order = crate::wholesale::get_order(conn, order_id)?;
    let detail = json!({
        "wholesaleOrderId": order_id,
        "orderState": order.state,
        "venueName": order.venue_name,
        "paymentLink": session.payment_link,
        "clientReference": session.client_reference,
        "paymentIntent": session.payment_intent,
    });
    match order.state.as_str() {
        "delivered" => {}
        "ordered" => {
            return refuse_wholesale_link_session(conn, session, "wholesale_not_delivered", detail);
        }
        _ => {
            return refuse_wholesale_link_session(
                conn,
                session,
                "wholesale_already_settled",
                detail,
            );
        }
    }
    let Some(total) = order.priced_total_cents else {
        return refuse_wholesale_link_session(conn, session, "wholesale_amount_mismatch", detail);
    };
    if !session.currency.eq_ignore_ascii_case("cad") || session.amount_cents != total {
        return refuse_wholesale_link_session(conn, session, "wholesale_amount_mismatch", detail);
    }
    let paid_on = db::local_date_from_utc_rfc3339(&session.paid_at)?;
    crate::wholesale::pay_order_from_link_session(
        conn,
        order_id,
        &session.session_id,
        session.payment_intent.as_deref(),
        session.amount_cents,
        &paid_on,
    )?;
    Ok(AppliedOutcome::Applied {
        order_id: order_id.to_string(),
    })
}

fn refuse_wholesale_link_session(
    conn: &mut Connection,
    session: &PaidSession,
    status: &str,
    detail: Value,
) -> Result<AppliedOutcome, String> {
    record_unapplied_fact_detailed(
        conn,
        "checkout_session",
        &session.session_id,
        status,
        Some(session.amount_cents),
        Some(&session.currency),
        session.created,
        detail,
    )?;
    Ok(AppliedOutcome::Rejected {
        session_id: session.session_id.clone(),
    })
}

/// LO-B (GT-D24-B): close a matched leftover session — leftover.paid +
/// income.received in one transaction, or a delete-proof refusal row.
fn apply_leftover_link_session(
    conn: &mut Connection,
    session: &PaidSession,
    listing_id: &str,
) -> Result<AppliedOutcome, String> {
    if crate::leftover::leftover_payment_already_applied(conn, &session.session_id)? {
        return Ok(AppliedOutcome::AlreadyApplied);
    }
    let listing = crate::leftover::get_listing(conn, listing_id)?;
    let detail = json!({
        "leftoverListingId": listing_id,
        "cropName": listing.crop_name,
        "harvestedOn": listing.harvested_on,
        "paymentLink": session.payment_link,
        "clientReference": session.client_reference,
        "paymentIntent": session.payment_intent,
    });
    if listing.paid_at.is_some() {
        return refuse_leftover_link_session(conn, session, "leftover_already_paid", detail);
    }
    let Some(total) = listing.priced_total_cents else {
        return refuse_leftover_link_session(conn, session, "leftover_amount_mismatch", detail);
    };
    if !session.currency.eq_ignore_ascii_case("cad") || session.amount_cents != total {
        return refuse_leftover_link_session(conn, session, "leftover_amount_mismatch", detail);
    }
    crate::leftover::pay_listing_from_link_session(
        conn,
        listing_id,
        &session.session_id,
        session.payment_intent.as_deref(),
        session.amount_cents,
        &session.paid_at,
    )?;
    Ok(AppliedOutcome::Applied {
        order_id: listing_id.to_string(),
    })
}

fn refuse_leftover_link_session(
    conn: &mut Connection,
    session: &PaidSession,
    status: &str,
    detail: Value,
) -> Result<AppliedOutcome, String> {
    record_unapplied_fact_detailed(
        conn,
        "checkout_session",
        &session.session_id,
        status,
        Some(session.amount_cents),
        Some(&session.currency),
        session.created,
        detail,
    )?;
    Ok(AppliedOutcome::Rejected {
        session_id: session.session_id.clone(),
    })
}

/// True when this session's Stripe fact was already recorded — via the same
/// `stripe_session_id`, or (idempotency key 2) the same non-empty
/// `client_reference` recorded on any order. Checked before ever generating a
/// new order id, so retries never race a partial insert.
fn session_already_recorded(tx: &Transaction<'_>, session: &PaidSession) -> Result<bool, String> {
    let by_session: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM orders WHERE stripe_session_id = ?1",
            [&session.session_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if by_session > 0 {
        return Ok(true);
    }
    if let Some(cr) = session
        .client_reference
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        let by_ref: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM orders WHERE client_reference = ?1",
                [cr],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if by_ref > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// C2 (INT-002): true when a `stripe.refunded` event already names this
/// refund. The poll re-reads refunds on purpose (newest-first pages, and the
/// cursor hold for a pending refund), so an applied refund comes back — and
/// before this check it was traced as `no_paid_order` because its orders were
/// already `refunded`. The event log is the only place the refund id lives.
fn refund_already_applied(conn: &Connection, refund_id: &str) -> Result<bool, String> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM event_log
             WHERE kind = 'stripe.refunded'
               AND json_extract(payload, '$.refundId') = ?1
             LIMIT 1",
            params![refund_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(found.is_some())
}

/// C2 (INT-002): what the gate decided about a refund that must not reach
/// the order book. Each carries the refusal reason the trace row stores.
enum RefundRefusal {
    /// Stripe status `pending` / `requires_action`: hold and re-read.
    NotTerminal,
    /// Stripe status `failed` / `canceled`: never a refund of anything.
    TerminalFailed,
    /// `succeeded`, amount reported, but below what the session's paid
    /// orders were paid.
    AmountPartial,
    /// Amount, currency or status missing, or the currency is not the
    /// orders' currency — nothing to measure against.
    NotComparable,
}

impl RefundRefusal {
    fn status(&self) -> &'static str {
        match self {
            RefundRefusal::NotTerminal => "not_terminal",
            RefundRefusal::TerminalFailed => "terminal_failed",
            RefundRefusal::AmountPartial => "amount_partial",
            RefundRefusal::NotComparable => "not_comparable",
        }
    }
}

/// C2 (INT-002, D1). The one rule: a refund reaches the order book only when
/// Stripe says `succeeded` and its amount covers the sum of what the session's
/// `paid` / `disputed` orders were paid, in the same currency. Everything
/// else is a refusal with a name. Nothing here reads a default: a missing
/// amount, currency or status is `NotComparable`, never zero.
fn refund_gate(refund: &RefundRecord, paid: &[&OrderRow]) -> Result<(), RefundRefusal> {
    let status = match refund.status.as_deref().map(str::to_ascii_lowercase) {
        Some(s) => s,
        None => return Err(RefundRefusal::NotComparable),
    };
    match status.as_str() {
        "pending" | "requires_action" => return Err(RefundRefusal::NotTerminal),
        "failed" | "canceled" => return Err(RefundRefusal::TerminalFailed),
        "succeeded" => {}
        _ => return Err(RefundRefusal::NotComparable),
    }
    let amount = match refund.amount_cents {
        Some(a) => a,
        None => return Err(RefundRefusal::NotComparable),
    };
    let currency = match refund.currency.as_deref() {
        Some(c) => c.to_ascii_lowercase(),
        None => return Err(RefundRefusal::NotComparable),
    };
    if paid
        .iter()
        .any(|o| !o.currency.eq_ignore_ascii_case(&currency))
    {
        return Err(RefundRefusal::NotComparable);
    }
    let total: i64 = paid.iter().map(|o| o.amount_cents).sum();
    if amount < total {
        return Err(RefundRefusal::AmountPartial);
    }
    Ok(())
}

/// C2 (INT-002). Trace a refused refund: one `stripe.fact_unapplied` per
/// (refund, reason), idempotent on re-read, plus — for the two terminal
/// refusals only (D2) — a Today card that states the reported amount and
/// nothing the record does not carry. `not_terminal` and `not_comparable`
/// leave only the durable trace. The card is keyed like `order.refunded`:
/// entity `order`, first order of the session, so `open_in_stripe` resolves
/// to the payment the refund belongs to.
fn trace_refused_refund(
    conn: &mut Connection,
    refund: &RefundRecord,
    refusal: &RefundRefusal,
    paid: &[&OrderRow],
) -> Result<(), String> {
    let status = refusal.status();
    if unapplied_fact_recorded(conn, "refund", &refund.refund_id, status)? {
        return Ok(());
    }
    let order_ids: Vec<String> = paid.iter().map(|o| o.id.clone()).collect();
    let detail = json!({
        "paymentIntent": refund.payment_intent,
        "orderIds": order_ids,
        "stripeStatus": refund.status,
    });
    let card: Option<(&str, String)> = match refusal {
        RefundRefusal::NotTerminal | RefundRefusal::NotComparable => None,
        RefundRefusal::AmountPartial | RefundRefusal::TerminalFailed => {
            match (refund.amount_cents, refund.currency.as_deref()) {
                (Some(cents), Some(currency)) => {
                    let amount = format_money_amount(cents, currency);
                    let total_qty: i64 = paid.iter().map(|o| o.quantity).sum();
                    let trays = tray_word(total_qty);
                    let date_label = format_short_month_day(&paid[0].harvest_date);
                    Some(match refusal {
                        RefundRefusal::AmountPartial => (
                            "order.refund_partial",
                            format!(
                                "A partial refund of {amount} was issued on {trays} due {date_label}. \
                                 The order stays paid and its capacity stays sold."
                            ),
                        ),
                        _ => (
                            "order.refund_failed",
                            format!(
                                "A refund of {amount} for {trays} due {date_label} failed at Stripe. \
                                 The order stays paid."
                            ),
                        ),
                    })
                }
                // A failed refund can arrive without an amount (the gate
                // refuses on status before it reads the amount). Trace only:
                // never a card without its number.
                _ => None,
            }
        }
    };

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let now = projection::handler_now();
    let event = unapplied_fact_event(
        "refund",
        &refund.refund_id,
        status,
        refund.amount_cents,
        refund.currency.as_deref(),
        refund.created,
        detail,
        &now,
    );
    projection::apply_event(&tx, &event)?;
    if let Some((kind, message)) = card {
        attention::raise_in_tx_at(
            &tx,
            kind,
            Some("order"),
            Some(&paid[0].id),
            &message,
            &["open_in_stripe", "dismiss"],
            &now,
        )?;
    }
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn apply_refund(
    conn: &mut Connection,
    refund: &RefundRecord,
) -> Result<FactOutcome, String> {
    // C2 (INT-002): a refund the log already carries is done, whatever its
    // orders read now. Checked first, before any trace can be written.
    if refund_already_applied(conn, &refund.refund_id)? {
        return Ok(FactOutcome::AlreadyApplied);
    }
    let orders = find_orders(
        conn,
        refund.payment_intent.as_deref(),
        refund.session_id.as_deref(),
    )?;
    if orders.is_empty() {
        // TILL-A (GT-D22): a refund on a wholesale Payment Link payment is
        // named and the walk advances. The order book is not moved here —
        // Reverse payment on the order is the operator's door.
        if let Some(order_id) = crate::wholesale::paid_order_for_stripe_intent(
            conn,
            refund.payment_intent.as_deref(),
            refund.session_id.as_deref(),
        )? {
            record_unapplied_fact_detailed(
                conn,
                "refund",
                &refund.refund_id,
                "wholesale_payment",
                refund.amount_cents,
                refund.currency.as_deref(),
                refund.created,
                json!({
                    "wholesaleOrderId": order_id,
                    "paymentIntent": refund.payment_intent,
                    "stripeStatus": refund.status
                }),
            )?;
            return Ok(FactOutcome::AlreadyApplied);
        }
        // R-10 (PACK-RF-PI): a refund Stripe listed with neither a payment
        // intent nor a session id has no key to match on — find_orders reads
        // exactly these two, and refund_from_json never fills session_id — so
        // no later poll can resolve it. Left AwaitingOrder it breaks the walk
        // and strands every newer refund behind it, in silence. There is
        // nothing to await: name it once and let the walk pass. The order book
        // is not moved, and no dollar is invented — amount and currency ride
        // through exactly as Stripe reported them, None stays None.
        let intent = refund.payment_intent.as_deref().filter(|s| !s.is_empty());
        let session = refund.session_id.as_deref().filter(|s| !s.is_empty());
        if intent.is_none() && session.is_none() {
            record_unapplied_fact_detailed(
                conn,
                "refund",
                &refund.refund_id,
                "no_payment_intent",
                refund.amount_cents,
                refund.currency.as_deref(),
                refund.created,
                json!({
                    "paymentIntent": refund.payment_intent,
                    "sessionId": refund.session_id,
                    "stripeStatus": refund.status
                }),
            )?;
            return Ok(FactOutcome::AlreadyApplied);
        }
        return Ok(FactOutcome::AwaitingOrder);
    }
    // F-A: a dispute changes state without touching capacity (apply_stripe_disputed)
    // does not SET capacity_consumed), so a refund arriving afterwards found no
    // 'paid' order and left the trays consumed forever. Refund is the ONLY site
    // that releases; dispute never does. Widening what refund acts on therefore
    // adds no second release path.
    let paid: Vec<&OrderRow> = orders
        .iter()
        .filter(|o| o.state == "paid" || o.state == "disputed")
        .collect();
    if paid.is_empty() {
        record_unapplied_fact(
            conn,
            "refund",
            &refund.refund_id,
            "no_paid_order",
            refund.amount_cents,
            refund.currency.as_deref(),
            refund.created,
        )?;
        return Ok(FactOutcome::AlreadyApplied);
    }

    // C2 (INT-002, D1/D2): the gate. Before this fence every refund object
    // Stripe listed — partial, pending, failed, amount unreported — flipped
    // every paid order on the session to `refunded`, released its capacity
    // and removed the whole sale from cash. Only a succeeded refund that
    // covers the session passes; the rest are traced with their reason.
    if let Err(refusal) = refund_gate(refund, &paid) {
        trace_refused_refund(conn, refund, &refusal, &paid)?;
        return Ok(match refusal {
            RefundRefusal::NotTerminal => FactOutcome::Deferred,
            _ => FactOutcome::AlreadyApplied,
        });
    }

    let harvest_date = paid[0].harvest_date.clone();
    let now = projection::handler_now();
    let today = db::local_date_from_utc_rfc3339(&now)?;
    let release = harvest_date.as_str() > today.as_str();
    let total_qty: i64 = paid.iter().map(|o| o.quantity).sum();
    let trays = tray_word(total_qty);
    let date_label = format_short_month_day(&harvest_date);

    let (kind, message) = if release {
        (
            "order.refunded",
            format!(
                "A refund was issued for {trays} due {date_label}. That capacity is available again."
            ),
        )
    } else {
        (
            "order.refunded_after_harvest",
            format!("A refund was issued for product already harvested on {date_label}."),
        )
    };

    let updated_ids: Vec<String> = paid.iter().map(|o| o.id.clone()).collect();

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    let session_id = refund
        .session_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| paid.first().map(|o| o.stripe_session_id.clone()))
        .ok_or_else(|| "refund cannot be linked to a paid session".to_string())?;
    let paid_event_id = paid_session_event_id(&tx, &session_id)?;

    let payload = json!({
        "refundId": refund.refund_id,
        "orderIds": updated_ids,
        "capacityReleased": release,
        "amountCents": refund.amount_cents,
        "currency": refund.currency,
    });
    let event = EventRecord::originated(
        Kind::StripeRefunded,
        "order",
        updated_ids[0].clone(),
        payload,
        json!({ "op": "none" }),
        now.clone(),
        None,
        Some(&paid_event_id),
        Some(projection::handler_new_id()),
    );

    projection::apply_event(&tx, &event)?;

    attention::raise_in_tx_at(
        &tx,
        kind,
        Some("order"),
        Some(&updated_ids[0]),
        &message,
        &["open_in_stripe", "dismiss"],
        &now,
    )?;

    events::insert_event(&tx, &event)?;

    tx.commit().map_err(|e| e.to_string())?;
    Ok(FactOutcome::Applied)
}

pub(crate) fn apply_dispute(
    conn: &mut Connection,
    dispute: &DisputeRecord,
) -> Result<FactOutcome, String> {
    let orders = find_orders(
        conn,
        dispute.payment_intent.as_deref(),
        dispute.session_id.as_deref(),
    )?;
    if orders.is_empty() {
        // TILL-A (GT-D22): a refund on a wholesale Payment Link payment is
        // named and the walk advances. The order book is not moved here —
        // Reverse payment on the order is the operator's door.
        if let Some(order_id) = crate::wholesale::paid_order_for_stripe_intent(
            conn,
            dispute.payment_intent.as_deref(),
            dispute.session_id.as_deref(),
        )? {
            record_unapplied_fact_detailed(
                conn,
                "dispute",
                &dispute.dispute_id,
                "wholesale_payment",
                dispute.amount_cents,
                dispute.currency.as_deref(),
                dispute.created,
                json!({
                    "wholesaleOrderId": order_id,
                    "paymentIntent": dispute.payment_intent,
                    "stripeStatus": dispute.status
                }),
            )?;
            return Ok(FactOutcome::AlreadyApplied);
        }
        return Ok(FactOutcome::AwaitingOrder);
    }
    let paid: Vec<&OrderRow> = orders.iter().filter(|o| o.state == "paid").collect();
    if paid.is_empty() {
        record_unapplied_fact(
            conn,
            "dispute",
            &dispute.dispute_id,
            "no_paid_order",
            dispute.amount_cents,
            dispute.currency.as_deref(),
            dispute.created,
        )?;
        return Ok(FactOutcome::AlreadyApplied);
    }

    let total_qty: i64 = paid.iter().map(|o| o.quantity).sum();
    let trays = tray_word(total_qty);
    let message = format!("A card payment for {trays} is disputed.");

    let updated_ids: Vec<String> = paid.iter().map(|o| o.id.clone()).collect();

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // Status change only — no capacity release, no funds movement recorded.
    // reverses_event_id stays NULL (see docs/track-1-inventory.md Phase 2).
    let payload = json!({
        "disputeId": dispute.dispute_id,
        "orderIds": updated_ids,
        "amountCents": dispute.amount_cents,
        "currency": dispute.currency,
    });
    let now = projection::handler_now();
    let event = EventRecord::originated(
        Kind::StripeDisputed,
        "order",
        updated_ids[0].clone(),
        payload,
        json!({ "op": "none" }),
        now.clone(),
        None,
        None,
        Some(projection::handler_new_id()),
    );

    projection::apply_event(&tx, &event)?;

    attention::raise_in_tx_at(
        &tx,
        "order.disputed",
        Some("order"),
        Some(&updated_ids[0]),
        &message,
        &["open_in_stripe", "dismiss"],
        &now,
    )?;

    events::insert_event(&tx, &event)?;

    tx.commit().map_err(|e| e.to_string())?;
    Ok(FactOutcome::Applied)
}

// --- Projection (Phase 4) ---------------------------------------------------
//
// Deterministic appliers used by both the live handlers above (via
// `projection::apply_event`, dispatched from `projection::kind`) and
// verify-replay. No clock reads, no random ids — everything comes from
// `event.payload` / `event.created_at`.
//
// Ruling 5: the canonical nested payload carries `clientReference` (null when
// absent). Two historical payload shapes are recognised on replay:
//   a) flat   — single order per event: `orderId`/`cropId`/`quantity`/
//               `amountCents` at the payload top level, no `lines`.
//   b) nested — current shape: `lines[]` (one entry per order) + `orderIds[]`.
// Rows written before this Ruling never carried `clientReference`; those are
// declared known divergences (DL-001/DL-002; docs/divergence-ledger.md) and
// replay correctly leaves `client_reference` NULL for them.

fn req_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("stripe.session_paid payload missing {key}"))
}

fn opt_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| if x.is_null() { None } else { x.as_str() })
}

fn req_i64(v: &Value, key: &str) -> Result<i64, String> {
    v.get(key)
        .and_then(|x| x.as_i64())
        .ok_or_else(|| format!("stripe.session_paid payload missing {key}"))
}

/// H-7b Fence 2 - the row this writer inserts, named. Field order is the old
/// parameter order, so the mapping is positional and nothing was reordered.
///
/// Fields are borrowed, not owned. This struct is write-only and never
/// outlives its call: both callers build it inside apply_stripe_session_paid
/// from borrows they already hold, so this land allocates nothing it did not
/// allocate before.
///
/// `state` is not a field: the INSERT writes the literal 'paid'. Nor is
/// `capacity_consumed`, which re-binds ?6 (quantity), nor `updated_at`, which
/// re-binds ?11 (created_at). None of the three is a caller's choice.
struct PaidOrderRow<'a> {
    order_id: &'a str,
    session_id: &'a str,
    payment_intent: Option<&'a str>,
    harvest_date: &'a str,
    crop_id: &'a str,
    quantity: i64,
    amount_cents: i64,
    currency: &'a str,
    customer_email: Option<&'a str>,
    paid_at: &'a str,
    created_at: &'a str,
    client_reference: Option<&'a str>,
}
fn insert_order_row(tx: &Transaction<'_>, row: &PaidOrderRow<'_>) -> Result<(), String> {
    tx.execute(
        "INSERT INTO orders
         (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
          quantity, amount_cents, currency, customer_email, state,
          capacity_consumed, paid_at, created_at, updated_at, client_reference)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'paid', ?6, ?10, ?11, ?11, ?12)",
        params![
            row.order_id,
            row.session_id,
            row.payment_intent,
            row.harvest_date,
            row.crop_id,
            row.quantity,
            row.amount_cents,
            row.currency,
            row.customer_email,
            row.paid_at,
            row.created_at,
            row.client_reference,
        ],
    )
    .map(|_| ())
    .map_err(|e| plain_insert_failure_reason(&e, row.crop_id))
}

pub fn apply_stripe_session_paid(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    let p = &event.payload;
    let client_reference = opt_str(p, "clientReference");

    if let Some(lines) = p.get("lines").and_then(|v| v.as_array()) {
        // Nested shape (current writer): one order per line, session-level fields shared.
        let session_id = req_str(p, "sessionId")?;
        let payment_intent = opt_str(p, "paymentIntent");
        let harvest_date = req_str(p, "harvestDate")?;
        let currency = req_str(p, "currency")?;
        let customer_email = opt_str(p, "customerEmail");
        let paid_at = req_str(p, "paidAt")?;

        for line in lines {
            let order_id = req_str(line, "orderId")?;
            let crop_id = req_str(line, "cropId")?;
            let quantity = req_i64(line, "quantity")?;
            let amount_cents = req_i64(line, "amountCents")?;
            insert_order_row(
                tx,
                &PaidOrderRow {
                    order_id,
                    session_id,
                    payment_intent,
                    harvest_date,
                    crop_id,
                    quantity,
                    amount_cents,
                    currency,
                    customer_email,
                    paid_at,
                    created_at: &event.created_at,
                    client_reference,
                },
            )?;
        }
    } else if p.get("orderId").is_some() {
        // Flat historical shape (pre multi-item-cart redesign): single order per event.
        let order_id = req_str(p, "orderId")?;
        let session_id = req_str(p, "sessionId")?;
        let payment_intent = opt_str(p, "paymentIntent");
        let harvest_date = req_str(p, "harvestDate")?;
        let crop_id = req_str(p, "cropId")?;
        let quantity = req_i64(p, "quantity")?;
        let amount_cents = req_i64(p, "amountCents")?;
        let currency = req_str(p, "currency")?;
        let customer_email = opt_str(p, "customerEmail");
        let paid_at = req_str(p, "paidAt")?;

        insert_order_row(
            tx,
            &PaidOrderRow {
                order_id,
                session_id,
                payment_intent,
                harvest_date,
                crop_id,
                quantity,
                amount_cents,
                currency,
                customer_email,
                paid_at,
                created_at: &event.created_at,
                client_reference,
            },
        )?;
    } else {
        return Err("stripe.session_paid payload matches neither known shape".to_string());
    }
    Ok(())
}

pub(crate) fn apply_stripe_refunded(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    let order_ids = event
        .payload
        .get("orderIds")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "stripe.refunded payload missing orderIds".to_string())?;
    let capacity_released = event
        .payload
        .get("capacityReleased")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| "stripe.refunded payload missing capacityReleased".to_string())?;

    for id in order_ids {
        let order_id = id
            .as_str()
            .ok_or_else(|| "stripe.refunded orderIds entry not a string".to_string())?;
        let n = tx
            .execute(
                "UPDATE orders
                 SET state = 'refunded',
                     capacity_consumed = CASE WHEN ?1 THEN 0 ELSE capacity_consumed END,
                     updated_at = ?2
                 WHERE id = ?3",
                params![capacity_released, event.created_at, order_id],
            )
            .map_err(|e| e.to_string())?;
        if n != 1 {
            return Err(format!("order not found: {order_id}"));
        }
    }
    Ok(())
}

pub(crate) fn apply_stripe_disputed(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    let order_ids = event
        .payload
        .get("orderIds")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "stripe.disputed payload missing orderIds".to_string())?;

    for id in order_ids {
        let order_id = id
            .as_str()
            .ok_or_else(|| "stripe.disputed orderIds entry not a string".to_string())?;
        let n = tx
            .execute(
                "UPDATE orders SET state = 'disputed', updated_at = ?1 WHERE id = ?2",
                params![event.created_at, order_id],
            )
            .map_err(|e| e.to_string())?;
        if n != 1 {
            return Err(format!("order not found: {order_id}"));
        }
    }
    Ok(())
}

pub(crate) fn apply_stripe_fact_unapplied(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    let stripe_object = event
        .payload
        .get("stripeObject")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "stripe.fact_unapplied payload missing stripeObject".to_string())?;
    let stripe_id = event
        .payload
        .get("stripeId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "stripe.fact_unapplied payload missing stripeId".to_string())?;
    let status = event
        .payload
        .get("status")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "stripe.fact_unapplied payload missing status".to_string())?;
    let amount_cents = event.payload.get("amountCents").and_then(|v| v.as_i64());
    let currency = event
        .payload
        .get("currency")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let stripe_created = event
        .payload
        .get("stripeCreated")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "stripe.fact_unapplied payload missing stripeCreated".to_string())?;

    tx.execute(
        "INSERT INTO stripe_unapplied_facts
         (event_id, stripe_object, stripe_id, status, amount_cents, currency,
          stripe_created, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event.event_id,
            stripe_object,
            stripe_id,
            status,
            amount_cents,
            currency,
            stripe_created,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn unapplied_fact_recorded(
    conn: &Connection,
    object: &str,
    stripe_id: &str,
    status: &str,
) -> Result<bool, String> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM stripe_unapplied_facts
             WHERE stripe_object = ?1 AND stripe_id = ?2 AND status = ?3
             LIMIT 1",
            params![object, stripe_id, status],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(found.is_some())
}

pub(crate) fn record_unapplied_fact(
    conn: &mut Connection,
    object: &str,
    stripe_id: &str,
    status: &str,
    amount_cents: Option<i64>,
    currency: Option<&str>,
    stripe_created: i64,
) -> Result<(), String> {
    record_unapplied_fact_detailed(
        conn,
        object,
        stripe_id,
        status,
        amount_cents,
        currency,
        stripe_created,
        json!({}),
    )
}

/// C2 (INT-002): the same trace with extra payload keys (`paymentIntent`,
/// `orderIds`, `stripeStatus`) for the log. The projection reads only the six
/// keys it always read; the detail is trail, not a column.
#[allow(clippy::too_many_arguments)] // H-7 Class B: permanent. The GT-D9 write choke point - every writer in the tree passes through this signature.
fn record_unapplied_fact_detailed(
    conn: &mut Connection,
    object: &str,
    stripe_id: &str,
    status: &str,
    amount_cents: Option<i64>,
    currency: Option<&str>,
    stripe_created: i64,
    detail: Value,
) -> Result<(), String> {
    if unapplied_fact_recorded(conn, object, stripe_id, status)? {
        return Ok(());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let now = projection::handler_now();
    let event = unapplied_fact_event(
        object,
        stripe_id,
        status,
        amount_cents,
        currency,
        stripe_created,
        detail,
        &now,
    );
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// The one `stripe.fact_unapplied` record builder. `detail` keys are merged
/// beside the six projection keys and never replace them.
#[allow(clippy::too_many_arguments)] // H-7 Class B: permanent. The GT-D9 write choke point - every writer in the tree passes through this signature.
fn unapplied_fact_event(
    object: &str,
    stripe_id: &str,
    status: &str,
    amount_cents: Option<i64>,
    currency: Option<&str>,
    stripe_created: i64,
    detail: Value,
    now: &str,
) -> EventRecord {
    let mut payload = json!({
        "stripeObject": object,
        "stripeId": stripe_id,
        "status": status,
        "amountCents": amount_cents,
        "currency": currency,
        "stripeCreated": stripe_created,
    });
    if let (Some(base), Some(extra)) = (payload.as_object_mut(), detail.as_object()) {
        for (k, v) in extra {
            base.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    EventRecord::originated(
        Kind::StripeFactUnapplied,
        "stripe_fact",
        stripe_id,
        payload,
        json!({ "op": "none" }),
        now.to_string(),
        None,
        None,
        Some(projection::handler_new_id()),
    )
}

// --- Queries ---------------------------------------------------------------

pub fn money_status(conn: &Connection) -> Result<MoneyStatus, String> {
    let (restricted_key, account_name, mode, checkout_endpoint_url): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT restricted_key, account_name, mode, checkout_endpoint_url
             FROM stripe_config WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or((None, None, None, None));

    let (last_poll_ok, last_poll_err): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT last_poll_ok, last_poll_err FROM stripe_cursor WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or((None, None));

    let open_order_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM orders WHERE state = 'paid'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    let configured = restricted_key.as_ref().is_some_and(|k| !k.is_empty());

    Ok(MoneyStatus {
        configured,
        mode,
        account_name,
        last_poll_ok,
        last_poll_err,
        open_order_count,
        checkout_endpoint_url: checkout_endpoint_url.filter(|s| !s.is_empty()),
    })
}

/// Validate and store the public checkout Worker URL (https only).
pub fn set_checkout_endpoint_url(conn: &Connection, url: &str) -> Result<MoneyStatus, String> {
    let url = validate_checkout_endpoint_url(url)?;
    conn.execute(
        "UPDATE stripe_config SET checkout_endpoint_url = ?1 WHERE id = 1",
        [&url],
    )
    .map_err(|e| e.to_string())?;
    money_status(conn)
}

pub fn checkout_endpoint_url(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT checkout_endpoint_url FROM stripe_config WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
    .map(|v| v.flatten().filter(|s: &String| !s.is_empty()))
}

pub fn validate_checkout_endpoint_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Paste the checkout address from your Worker deploy.".into());
    }
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("https://") {
        return Err("Checkout address must be an https:// URL.".into());
    }
    if lower.contains(' ') || url.contains('\n') || url.contains('\r') {
        return Err("Checkout address is not a valid URL.".into());
    }
    Ok(url.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnappliedFactView {
    pub event_id: String,
    pub stripe_object: String,
    pub stripe_id: String,
    pub status: String,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    pub stripe_created: i64,
    pub observed_at: String,
}

pub fn list_unapplied_facts(conn: &Connection) -> Result<Vec<UnappliedFactView>, String> {
    // F-B: the table is append-only on purpose — it is the trail. What was
    // wrong is the SURFACE, which showed a fact forever after it was applied.
    // Retirement is therefore a read-side exclusion, not a mutation.
    let mut stmt = conn
        .prepare(
            "SELECT event_id, stripe_object, stripe_id, status,
                    amount_cents, currency, stripe_created, observed_at
             FROM stripe_unapplied_facts f
             WHERE NOT EXISTS (
                 SELECT 1 FROM orders o
                 WHERE o.stripe_session_id = f.stripe_id
                    OR (o.stripe_payment_intent IS NOT NULL
                        AND o.stripe_payment_intent = f.stripe_id)
             )
               -- C2 (INT-002): a refund fact retires the same way once the
               -- refund itself was applied (a held `not_terminal` that Stripe
               -- later settled). The row and the event stay; the surface stops
               -- calling an applied refund unapplied.
               AND NOT EXISTS (
                 SELECT 1 FROM event_log e
                 WHERE e.kind = 'stripe.refunded'
                   AND json_extract(e.payload, '$.refundId') = f.stripe_id
               )
             ORDER BY observed_at DESC, event_id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(UnappliedFactView {
                event_id: row.get(0)?,
                stripe_object: row.get(1)?,
                stripe_id: row.get(2)?,
                status: row.get(3)?,
                amount_cents: row.get(4)?,
                currency: row.get(5)?,
                stripe_created: row.get(6)?,
                observed_at: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn list_orders(
    conn: &Connection,
    harvest_date: Option<&str>,
) -> Result<Vec<OrderView>, String> {
    let sql = if harvest_date.is_some() {
        r#"
        SELECT o.id, o.stripe_session_id, o.stripe_payment_intent, o.harvest_date,
               o.crop_id, c.name, o.quantity, o.amount_cents, o.currency,
               o.customer_email, o.state, o.capacity_consumed, o.client_reference,
               o.paid_at, o.created_at, o.updated_at
        FROM orders o
        JOIN crops c ON c.id = o.crop_id
        WHERE o.harvest_date = ?1
        ORDER BY o.paid_at ASC, o.id ASC
        "#
    } else {
        r#"
        SELECT o.id, o.stripe_session_id, o.stripe_payment_intent, o.harvest_date,
               o.crop_id, c.name, o.quantity, o.amount_cents, o.currency,
               o.customer_email, o.state, o.capacity_consumed, o.client_reference,
               o.paid_at, o.created_at, o.updated_at
        FROM orders o
        JOIN crops c ON c.id = o.crop_id
        ORDER BY o.harvest_date ASC, o.paid_at ASC, o.id ASC
        "#
    };

    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let map = |row: &rusqlite::Row<'_>| -> rusqlite::Result<OrderView> {
        Ok(OrderView {
            id: row.get(0)?,
            stripe_session_id: row.get(1)?,
            stripe_payment_intent: row.get(2)?,
            harvest_date: row.get(3)?,
            crop_id: row.get(4)?,
            crop_name: row.get(5)?,
            quantity: row.get(6)?,
            amount_cents: row.get(7)?,
            currency: row.get(8)?,
            customer_email: row.get(9)?,
            state: row.get(10)?,
            capacity_consumed: row.get(11)?,
            client_reference: row.get(12)?,
            paid_at: row.get(13)?,
            created_at: row.get(14)?,
            updated_at: row.get(15)?,
        })
    };

    let rows = if let Some(date) = harvest_date {
        stmt.query_map([date], map).map_err(|e| e.to_string())?
    } else {
        stmt.query_map([], map).map_err(|e| e.to_string())?
    };

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn stripe_dashboard_url(conn: &Connection, order_id: &str) -> Result<Option<String>, String> {
    let row: Option<(Option<String>, String)> = conn
        .query_row(
            "SELECT stripe_payment_intent, stripe_session_id FROM orders WHERE id = ?1",
            [order_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let Some((pi, session_id)) = row else {
        return Ok(None);
    };

    let test_prefix = dashboard_test_prefix(conn)?;
    let path = if let Some(pi) = pi.filter(|s| !s.is_empty()) {
        format!("payments/{pi}")
    } else {
        format!("checkout/sessions/{session_id}")
    };

    Ok(Some(format!(
        "https://dashboard.stripe.com/{test_prefix}{path}"
    )))
}

/// Dashboard URL for a Checkout Session that may have no local order row.
pub fn stripe_session_dashboard_url(conn: &Connection, session_id: &str) -> Result<String, String> {
    let test_prefix = dashboard_test_prefix(conn)?;
    Ok(format!(
        "https://dashboard.stripe.com/{test_prefix}checkout/sessions/{session_id}"
    ))
}

fn dashboard_test_prefix(conn: &Connection) -> Result<&'static str, String> {
    let mode: Option<String> = conn
        .query_row("SELECT mode FROM stripe_config WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    Ok(if mode.as_deref() == Some("live") {
        ""
    } else {
        "test/"
    })
}

// --- Internals -------------------------------------------------------------

struct OrderRow {
    id: String,
    stripe_session_id: String,
    harvest_date: String,
    quantity: i64,
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    capacity_consumed: i64,
    state: String,
    /// C2 (INT-002): what this order line was paid, so a refund can be
    /// measured against the session before it may flip anything.
    amount_cents: i64,
    currency: String,
}

fn find_orders(
    conn: &Connection,
    payment_intent: Option<&str>,
    session_id: Option<&str>,
) -> Result<Vec<OrderRow>, String> {
    if let Some(pi) = payment_intent.filter(|s| !s.is_empty()) {
        let mut stmt = conn
            .prepare(
                "SELECT id, stripe_session_id, harvest_date, quantity, capacity_consumed, state,
                        amount_cents, currency
                 FROM orders WHERE stripe_payment_intent = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([pi], map_order_row)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        if !out.is_empty() {
            return Ok(out);
        }
    }
    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        let mut stmt = conn
            .prepare(
                "SELECT id, stripe_session_id, harvest_date, quantity, capacity_consumed, state,
                        amount_cents, currency
                 FROM orders WHERE stripe_session_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([sid], map_order_row)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        return Ok(out);
    }
    Ok(Vec::new())
}

fn map_order_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OrderRow> {
    Ok(OrderRow {
        id: row.get(0)?,
        stripe_session_id: row.get(1)?,
        harvest_date: row.get(2)?,
        quantity: row.get(3)?,
        capacity_consumed: row.get(4)?,
        state: row.get(5)?,
        amount_cents: row.get(6)?,
        currency: row.get(7)?,
    })
}

/// event_log.id of the originating `stripe.session_paid` for this Checkout session.
fn paid_session_event_id(tx: &Transaction<'_>, session_id: &str) -> Result<String, String> {
    tx.query_row(
        "SELECT id FROM event_log
         WHERE kind = 'stripe.session_paid' AND entity_id = ?1
         ORDER BY seq ASC
         LIMIT 1",
        [session_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| {
        format!("refund cannot be linked to paid session event for session {session_id}")
    })
}

/// True for a UNIQUE failure on either idempotency key:
/// `(orders.stripe_session_id, …)` or `(orders.client_reference, …)`.
/// FK / CHECK / NOT NULL must not be treated as AlreadyApplied.
/// `apply_paid_session` gates on `session_already_recorded` instead of this
/// (single INSERT surface via `apply_event`); kept for direct classification.
#[allow(dead_code)]
pub(crate) fn is_already_applied_violation(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(e, Some(msg)) => {
            e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                && (msg.contains("orders.stripe_session_id")
                    || msg.contains("orders.client_reference"))
        }
        _ => false,
    }
}

fn plain_insert_failure_reason(err: &rusqlite::Error, crop_id: &str) -> String {
    let msg = match err {
        rusqlite::Error::SqliteFailure(_, Some(m)) => m.as_str(),
        _ => "",
    };
    let lower = msg.to_ascii_lowercase();
    if lower.contains("foreign key") {
        format!("Farm OS doesn't have a crop called \"{crop_id}\"")
    } else if lower.contains("not null") {
        "a required field was missing".to_string()
    } else if lower.contains("check") {
        "the payment data failed a farm rule".to_string()
    } else {
        "the payment data couldn't be saved".to_string()
    }
}

/// Legacy harvest-link signature helper — unused after cart checkout.
#[allow(dead_code)]
pub fn line_signature(lines: &[HarvestLinkLine]) -> String {
    let mut parts: Vec<String> = lines
        .iter()
        .map(|l| format!("{}:{}", l.price_id, l.max_quantity))
        .collect();
    parts.sort();
    parts.join("|")
}

fn format_money_amount(cents: i64, currency: &str) -> String {
    let dollars = (cents as f64) / 100.0;
    if currency.eq_ignore_ascii_case("cad") || currency.eq_ignore_ascii_case("usd") {
        format!("${dollars:.2}")
    } else {
        format!("{dollars:.2} {}", currency.to_ascii_uppercase())
    }
}

fn tray_word(n: i64) -> String {
    if n == 1 {
        "1 tray".to_string()
    } else {
        format!("{n} trays")
    }
}

fn oversold_word(n: i64) -> String {
    if n == 1 {
        "1 tray".to_string()
    } else {
        format!("{n} trays")
    }
}

fn format_short_month_day(yyyy_mm_dd: &str) -> String {
    let Ok(d) = NaiveDate::parse_from_str(yyyy_mm_dd, "%Y-%m-%d") else {
        return yyyy_mm_dd.to_string();
    };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{} {}", months[d.month0() as usize], d.day())
}

// --- Fake gateway (tests only) ---------------------------------------------

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeState {
        pub account: Option<AccountInfo>,
        pub paid_sessions: Vec<PaidSession>,
        pub refunds: Vec<RefundRecord>,
        pub disputes: Vec<DisputeRecord>,
        /// When non-empty, `list_paid_session_pages` returns these pages.
        pub session_pages: Vec<SessionPage>,
        pub refund_pages: Vec<Vec<RefundRecord>>,
        pub dispute_pages: Vec<Vec<DisputeRecord>>,
        pub list_sessions_err: Option<String>,
        pub list_refunds_err: Option<String>,
        pub list_disputes_err: Option<String>,
        pub create_link_err: Option<String>,
        /// Offers passed to create_price.
        pub prices_created: Vec<Offer>,
        /// Harvest-date Payment Links created: (harvest_date, lines).
        pub harvest_links_created: Vec<(String, Vec<HarvestLinkLine>)>,
        pub deactivated_links: Vec<String>,
        pub order_links_created: Vec<OrderBill>,
    }

    pub struct FakeGateway {
        pub state: Mutex<FakeState>,
    }

    impl FakeGateway {
        pub fn new() -> Self {
            Self {
                state: Mutex::new(FakeState::default()),
            }
        }

        pub fn with_account(self, account: AccountInfo) -> Self {
            self.state.lock().unwrap().account = Some(account);
            self
        }

        pub fn push_session(&self, session: PaidSession) {
            self.state.lock().unwrap().paid_sessions.push(session);
        }

        #[allow(dead_code)]
        pub fn push_refund(&self, refund: RefundRecord) {
            self.state.lock().unwrap().refunds.push(refund);
        }

        #[allow(dead_code)]
        pub fn push_dispute(&self, dispute: DisputeRecord) {
            self.state.lock().unwrap().disputes.push(dispute);
        }

        #[allow(dead_code)]
        pub fn fail_sessions(&self, err: impl Into<String>) {
            self.state.lock().unwrap().list_sessions_err = Some(err.into());
        }

        pub fn clear_session_fail(&self) {
            self.state.lock().unwrap().list_sessions_err = None;
        }
    }

    impl Default for FakeGateway {
        fn default() -> Self {
            Self::new()
        }
    }

    impl StripeGateway for FakeGateway {
        fn account(&self) -> Result<AccountInfo, String> {
            self.state
                .lock()
                .unwrap()
                .account
                .clone()
                .ok_or_else(|| "fake gateway: no account".to_string())
        }

        fn create_price(&self, offer: &Offer) -> Result<String, String> {
            let mut st = self.state.lock().unwrap();
            if let Some(err) = st.create_link_err.clone() {
                return Err(err);
            }
            let price_id = format!("price_fake_{}", offer.id);
            st.prices_created.push(offer.clone());
            Ok(price_id)
        }

        fn create_harvest_payment_link(
            &self,
            harvest_date: &str,
            lines: &[HarvestLinkLine],
        ) -> Result<(String, String), String> {
            let mut st = self.state.lock().unwrap();
            if let Some(err) = st.create_link_err.clone() {
                return Err(err);
            }
            let n = st.harvest_links_created.len();
            st.harvest_links_created
                .push((harvest_date.to_string(), lines.to_vec()));
            Ok((
                format!("link_fake_{harvest_date}_{n}"),
                format!("https://buy.stripe.com/test/{harvest_date}_{n}"),
            ))
        }

        fn deactivate_link(&self, link_id: &str) -> Result<(), String> {
            self.state
                .lock()
                .unwrap()
                .deactivated_links
                .push(link_id.to_string());
            Ok(())
        }

        fn create_order_payment_link(&self, bill: &OrderBill) -> Result<MintedLink, String> {
            let mut st = self.state.lock().unwrap();
            if let Some(err) = st.create_link_err.clone() {
                return Err(err);
            }
            st.order_links_created.push(bill.clone());
            Ok(MintedLink {
                link_id: format!("plink_fake_{}", bill.order_id),
                url: format!("https://buy.stripe.com/test/wo/{}", bill.order_id),
            })
        }

        fn list_paid_sessions(&self, since: Option<&str>) -> Result<Vec<PaidSession>, String> {
            Ok(self
                .list_paid_session_pages(since)?
                .into_iter()
                .flat_map(|p| p.parsed)
                .collect())
        }

        fn list_refunds(&self, since: Option<&str>) -> Result<Vec<RefundRecord>, String> {
            Ok(self
                .list_refund_pages(since)?
                .into_iter()
                .flatten()
                .collect())
        }

        fn list_disputes(&self, since: Option<&str>) -> Result<Vec<DisputeRecord>, String> {
            Ok(self
                .list_dispute_pages(since)?
                .into_iter()
                .flatten()
                .collect())
        }

        fn list_paid_session_pages(&self, since: Option<&str>) -> Result<Vec<SessionPage>, String> {
            let st = self.state.lock().unwrap();
            if let Some(err) = &st.list_sessions_err {
                return Err(err.clone());
            }
            let pages = if st.session_pages.is_empty() {
                if st.paid_sessions.is_empty() {
                    vec![]
                } else {
                    vec![SessionPage::from_parsed(st.paid_sessions.clone())]
                }
            } else {
                st.session_pages.clone()
            };
            Ok(filter_session_pages(pages, since))
        }

        fn list_refund_pages(&self, since: Option<&str>) -> Result<Vec<Vec<RefundRecord>>, String> {
            let st = self.state.lock().unwrap();
            if let Some(err) = &st.list_refunds_err {
                return Err(err.clone());
            }
            let pages = if st.refund_pages.is_empty() {
                if st.refunds.is_empty() {
                    vec![]
                } else {
                    vec![st.refunds.clone()]
                }
            } else {
                st.refund_pages.clone()
            };
            Ok(filter_refund_pages(pages, since))
        }

        fn list_dispute_pages(
            &self,
            since: Option<&str>,
        ) -> Result<Vec<Vec<DisputeRecord>>, String> {
            let st = self.state.lock().unwrap();
            if let Some(err) = &st.list_disputes_err {
                return Err(err.clone());
            }
            let pages = if st.dispute_pages.is_empty() {
                if st.disputes.is_empty() {
                    vec![]
                } else {
                    vec![st.disputes.clone()]
                }
            } else {
                st.dispute_pages.clone()
            };
            Ok(filter_dispute_pages(pages, since))
        }
    }

    fn filter_session_pages(pages: Vec<SessionPage>, since: Option<&str>) -> Vec<SessionPage> {
        let Some(s) = since.filter(|s| !s.is_empty()) else {
            return pages;
        };
        let Ok(min) = s.parse::<i64>() else {
            return pages;
        };
        // gte: same-second sessions must not be stranded at the cursor boundary.
        pages
            .into_iter()
            .map(|p| SessionPage {
                parsed: p.parsed.into_iter().filter(|x| x.created >= min).collect(),
                unparsed: p
                    .unparsed
                    .into_iter()
                    .filter(|x| x.created >= min)
                    .collect(),
            })
            .filter(|p| !p.is_empty())
            .collect()
    }

    fn filter_refund_pages(
        pages: Vec<Vec<RefundRecord>>,
        since: Option<&str>,
    ) -> Vec<Vec<RefundRecord>> {
        let Some(s) = since.filter(|s| !s.is_empty()) else {
            return pages;
        };
        let Ok(min) = s.parse::<i64>() else {
            return pages;
        };
        pages
            .into_iter()
            .map(|p| p.into_iter().filter(|x| x.created > min).collect())
            .filter(|p: &Vec<_>| !p.is_empty())
            .collect()
    }

    fn filter_dispute_pages(
        pages: Vec<Vec<DisputeRecord>>,
        since: Option<&str>,
    ) -> Vec<Vec<DisputeRecord>> {
        let Some(s) = since.filter(|s| !s.is_empty()) else {
            return pages;
        };
        let Ok(min) = s.parse::<i64>() else {
            return pages;
        };
        pages
            .into_iter()
            .map(|p| p.into_iter().filter(|x| x.created > min).collect())
            .filter(|p: &Vec<_>| !p.is_empty())
            .collect()
    }
}
