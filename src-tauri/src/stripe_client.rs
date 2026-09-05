//! Real Stripe HTTP gateway behind [`crate::money::StripeGateway`].
//!
//! Network failures return plain-English `Err` strings. They never panic.
//! The restricted key and customer emails never appear in those messages.

use crate::money::{
    AccountInfo, DisputeRecord, HarvestLinkLine, MintedLink, Offer, OrderBill, PaidLine,
    PaidSession, RefundRecord, SessionPage, StripeGateway, UnparsedSession,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::Value;
use uuid::Uuid;

const API_BASE: &str = "https://api.stripe.com";
const STRIPE_VERSION: &str = "2024-11-20.acacia";

/// Minimal HTTP surface so list pagination and errors can be tested without the wire.
pub(crate) trait StripeHttp: Send + Sync {
    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, String>;
    fn post(
        &self,
        path: &str,
        form: &[(&str, &str)],
        idempotency_key: &str,
    ) -> Result<Value, String>;
}

pub struct UreqHttp {
    key: String,
}

impl UreqHttp {
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }

    fn auth_header(&self) -> String {
        format!("Basic {}", B64.encode(format!("{}:", self.key)))
    }

    fn map_err(&self, err: ureq::Error) -> String {
        let raw = match &err {
            ureq::Error::StatusCode(code) => {
                format!(
                    "Stripe returned an error (HTTP {code}). Check the key scopes and try again."
                )
            }
            _ => "Farm OS couldn't reach Stripe. Check the connection and try again.".to_string(),
        };
        redact_secrets(&raw, &self.key)
    }

    fn read_json(&self, mut response: ureq::http::Response<ureq::Body>) -> Result<Value, String> {
        response.body_mut().read_json::<Value>().map_err(|_| {
            "Stripe sent a response Farm OS couldn't read. Try again in a moment.".to_string()
        })
    }
}

impl StripeHttp for UreqHttp {
    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        let url = format!("{API_BASE}{path}");
        let mut req = ureq::get(&url)
            .header("Authorization", self.auth_header())
            .header("Stripe-Version", STRIPE_VERSION);
        for (k, v) in query {
            req = req.query(*k, *v);
        }
        match req.call() {
            Ok(resp) => self.read_json(resp),
            Err(e) => Err(self.map_err(e)),
        }
    }

    fn post(
        &self,
        path: &str,
        form: &[(&str, &str)],
        idempotency_key: &str,
    ) -> Result<Value, String> {
        let url = format!("{API_BASE}{path}");
        let req = ureq::post(&url)
            .header("Authorization", self.auth_header())
            .header("Stripe-Version", STRIPE_VERSION)
            .header("Idempotency-Key", idempotency_key);
        match req.send_form(form.iter().copied()) {
            Ok(resp) => self.read_json(resp),
            Err(e) => Err(self.map_err(e)),
        }
    }
}

pub struct StripeClient<H: StripeHttp> {
    http: H,
    mode: String,
}

impl StripeClient<UreqHttp> {
    pub fn with_key(key: &str, mode: &str) -> Self {
        Self {
            http: UreqHttp::new(key),
            mode: mode.to_string(),
        }
    }
}

impl<H: StripeHttp> StripeClient<H> {
    #[allow(dead_code)] // Used by tests and Prompt 3/4 poll.
    pub fn new(http: H, mode: impl Into<String>) -> Self {
        Self {
            http,
            mode: mode.into(),
        }
    }

    #[allow(dead_code)]
    fn list_all_pages(
        &self,
        path: &str,
        extra_query: &[(&str, &str)],
    ) -> Result<Vec<Vec<Value>>, String> {
        let mut pages = Vec::new();
        let mut starting_after: Option<String> = None;
        loop {
            let mut query: Vec<(&str, &str)> = vec![("limit", "100")];
            query.extend_from_slice(extra_query);
            let cursor;
            if let Some(ref sa) = starting_after {
                cursor = sa.clone();
                query.push(("starting_after", cursor.as_str()));
            }
            let page = self.http.get(path, &query)?;
            let data = page
                .get("data")
                .and_then(|d| d.as_array())
                .ok_or_else(|| "Stripe list response was missing data.".to_string())?;
            let mut items = Vec::new();
            for item in data {
                items.push(item.clone());
            }
            let has_more = page
                .get("has_more")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !items.is_empty() {
                let last_id = items
                    .last()
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        "Stripe list page was missing an id for pagination.".to_string()
                    })?
                    .to_string();
                starting_after = Some(last_id);
                pages.push(items);
            }
            if !has_more || data.is_empty() {
                break;
            }
        }
        Ok(pages)
    }

    fn fetch_session_line_items(&self, session_id: &str) -> Result<Vec<Value>, String> {
        let path = format!("/v1/checkout/sessions/{session_id}/line_items");
        let pages = self.list_all_pages(&path, &[])?;
        Ok(pages.into_iter().flatten().collect())
    }
}

impl<H: StripeHttp> StripeGateway for StripeClient<H> {
    fn account(&self) -> Result<AccountInfo, String> {
        // Authenticated account for this key (not Connect account-by-id).
        let v = self.http.get("/v1/account", &[])?;
        let account_id = v
            .get("id")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "Stripe account response was missing an id.".to_string())?
            .to_string();
        let account_name = account_display_name(&v);
        Ok(AccountInfo {
            account_id,
            account_name,
            mode: self.mode.clone(),
        })
    }

    fn create_price(&self, offer: &Offer) -> Result<String, String> {
        // Metadata is for human readability in the Stripe dashboard only —
        // Farm OS never attributes lines from metadata.
        let product = self.http.post(
            "/v1/products",
            &[
                (
                    "name",
                    &format!("{} · {}", offer.crop_id, offer.harvest_date),
                ),
                ("metadata[harvest_date]", &offer.harvest_date),
                ("metadata[crop_id]", &offer.crop_id),
                ("metadata[offer_id]", &offer.id),
            ],
            &Uuid::new_v4().to_string(),
        )?;
        let product_id = product
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "Stripe did not return a product id.".to_string())?
            .to_string();

        let unit = offer.price_cents.to_string();
        let price = self.http.post(
            "/v1/prices",
            &[
                ("product", product_id.as_str()),
                ("unit_amount", unit.as_str()),
                ("currency", "cad"),
                ("metadata[harvest_date]", &offer.harvest_date),
                ("metadata[crop_id]", &offer.crop_id),
                ("metadata[offer_id]", &offer.id),
            ],
            &Uuid::new_v4().to_string(),
        )?;
        price
            .get("id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Stripe did not return a price id.".to_string())
    }

    fn create_harvest_payment_link(
        &self,
        harvest_date: &str,
        lines: &[HarvestLinkLine],
    ) -> Result<(String, String), String> {
        if lines.is_empty() {
            return Err("A harvest Payment Link needs at least one crop.".into());
        }
        // Owned strings so form field references stay valid for the post call.
        let mut owned: Vec<(String, String)> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let max = line.max_quantity.clamp(0, 99).to_string();
            owned.push((format!("line_items[{i}][price]"), line.price_id.clone()));
            owned.push((format!("line_items[{i}][quantity]"), "1".into()));
            owned.push((
                format!("line_items[{i}][adjustable_quantity][enabled]"),
                "true".into(),
            ));
            owned.push((
                format!("line_items[{i}][adjustable_quantity][minimum]"),
                "0".into(),
            ));
            owned.push((
                format!("line_items[{i}][adjustable_quantity][maximum]"),
                max,
            ));
        }
        owned.push(("metadata[harvest_date]".into(), harvest_date.to_string()));
        owned.push((
            "payment_intent_data[metadata][harvest_date]".into(),
            harvest_date.to_string(),
        ));

        let form: Vec<(&str, &str)> = owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let link = self
            .http
            .post("/v1/payment_links", &form, &Uuid::new_v4().to_string())?;
        let link_id = link
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "Stripe did not return a payment link id.".to_string())?
            .to_string();
        let url = link
            .get("url")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "Stripe did not return a payment link url.".to_string())?
            .to_string();
        Ok((link_id, url))
    }

    fn deactivate_link(&self, link_id: &str) -> Result<(), String> {
        let path = format!("/v1/payment_links/{link_id}");
        self.http
            .post(&path, &[("active", "false")], &Uuid::new_v4().to_string())?;
        Ok(())
    }

    fn create_order_payment_link(&self, bill: &OrderBill) -> Result<MintedLink, String> {
        // Metadata is for the dashboard only — Farm OS never attributes from it.
        // LO-B (GT-D24-B): a leftover bill is recognised by its lo- reference and
        // labelled leftover_listing_id on every metadata site; the wholesale wo-
        // path below is byte-identical to TILL-A.
        let leftover = bill
            .client_reference
            .starts_with(crate::leftover::LEFTOVER_CLIENT_REFERENCE_PREFIX);
        let meta_key = if leftover {
            "metadata[leftover_listing_id]"
        } else {
            "metadata[wholesale_order_id]"
        };
        let intent_meta_key = if leftover {
            "payment_intent_data[metadata][leftover_listing_id]"
        } else {
            "payment_intent_data[metadata][wholesale_order_id]"
        };
        let idem = if leftover { "lo" } else { "wo" };
        let name = format!("{} · harvest {}", bill.venue_name, bill.harvest_date);
        let product = self.http.post(
            "/v1/products",
            &[("name", name.as_str()), (meta_key, bill.order_id.as_str())],
            &format!("{idem}-product-{}", bill.order_id),
        )?;
        let product_id = product
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "Stripe did not return a product id.".to_string())?
            .to_string();
        let unit = bill.amount_cents.to_string();
        let price = self.http.post(
            "/v1/prices",
            &[
                ("product", product_id.as_str()),
                ("unit_amount", unit.as_str()),
                ("currency", "cad"),
                (meta_key, bill.order_id.as_str()),
            ],
            &format!("{idem}-price-{}", bill.order_id),
        )?;
        let price_id = price
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "Stripe did not return a price id.".to_string())?
            .to_string();
        let link = self.http.post(
            "/v1/payment_links",
            &[
                ("line_items[0][price]", price_id.as_str()),
                ("line_items[0][quantity]", "1"),
                (meta_key, bill.order_id.as_str()),
                (intent_meta_key, bill.order_id.as_str()),
            ],
            &format!("{idem}-link-{}", bill.order_id),
        )?;
        let link_id = link
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "Stripe did not return a payment link id.".to_string())?
            .to_string();
        let url = link
            .get("url")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "Stripe did not return a payment link url.".to_string())?
            .to_string();
        Ok(MintedLink { link_id, url })
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
        // Do not expand line_items on the list — fetch each session's lines explicitly.
        let mut extra: Vec<(&str, &str)> = Vec::new();
        let since_owned;
        if let Some(s) = since.filter(|s| !s.is_empty()) {
            since_owned = s.to_string();
            if looks_like_unix(&since_owned) {
                // gte: same-second sessions at the cursor boundary must re-fetch;
                // apply is idempotent on (stripe_session_id, crop_id).
                extra.push(("created[gte]", since_owned.as_str()));
            }
        }
        let raw_pages = self.list_all_pages("/v1/checkout/sessions", &extra)?;
        let mut pages = Vec::new();
        for raw_page in raw_pages {
            let mut page = SessionPage::default();
            for raw in raw_page {
                if raw.get("payment_status").and_then(|v| v.as_str()) != Some("paid") {
                    continue;
                }
                let session_id = match raw.get("id").and_then(|v| v.as_str()) {
                    Some(id) => id.to_string(),
                    None => {
                        page.unparsed
                            .push(unparsed_from_raw(&raw, "session missing id".into()));
                        continue;
                    }
                };
                match self.fetch_session_line_items(&session_id) {
                    Ok(line_items) => match build_paid_session(&raw, &line_items) {
                        Ok(session) => page.parsed.push(session),
                        Err(reason) => page.unparsed.push(unparsed_from_raw(&raw, reason)),
                    },
                    Err(reason) => page.unparsed.push(unparsed_from_raw(&raw, reason)),
                }
            }
            if !page.is_empty() {
                pages.push(page);
            }
        }
        Ok(pages)
    }

    fn list_refund_pages(&self, since: Option<&str>) -> Result<Vec<Vec<RefundRecord>>, String> {
        // TODO(stage-4): refunds and disputes still use created[gt] and per-page cursor
        // advancement. Same stranding risk as sessions had. Not in scope for the P0 pass.
        let mut extra: Vec<(&str, &str)> = Vec::new();
        let since_owned;
        if let Some(s) = since.filter(|s| !s.is_empty()) {
            since_owned = s.to_string();
            if looks_like_unix(&since_owned) {
                extra.push(("created[gt]", since_owned.as_str()));
            }
        }
        let raw_pages = self.list_all_pages("/v1/refunds", &extra)?;
        let mut pages = Vec::new();
        for raw_page in raw_pages {
            let mut page = Vec::new();
            for raw in raw_page {
                if let Some(r) = refund_from_json(&raw) {
                    page.push(r);
                }
            }
            if !page.is_empty() {
                pages.push(page);
            }
        }
        Ok(pages)
    }

    fn list_dispute_pages(&self, since: Option<&str>) -> Result<Vec<Vec<DisputeRecord>>, String> {
        let mut extra: Vec<(&str, &str)> = Vec::new();
        let since_owned;
        if let Some(s) = since.filter(|s| !s.is_empty()) {
            since_owned = s.to_string();
            if looks_like_unix(&since_owned) {
                extra.push(("created[gt]", since_owned.as_str()));
            }
        }
        let raw_pages = self.list_all_pages("/v1/disputes", &extra)?;
        let mut pages = Vec::new();
        for raw_page in raw_pages {
            let mut page = Vec::new();
            for raw in raw_page {
                if let Some(r) = dispute_from_json(&raw) {
                    page.push(r);
                }
            }
            if !page.is_empty() {
                pages.push(page);
            }
        }
        Ok(pages)
    }
}

fn account_display_name(v: &Value) -> String {
    v.pointer("/settings/dashboard/display_name")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            v.pointer("/business_profile/name")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("Stripe account")
        .to_string()
}

pub(crate) fn refund_from_json(raw: &Value) -> Option<RefundRecord> {
    let refund_id = raw.get("id").and_then(|v| v.as_str())?.to_string();
    let payment_intent = raw
        .get("payment_intent")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let created = raw.get("created").and_then(|v| v.as_i64()).unwrap_or(0);
    let amount_cents = raw.get("amount").and_then(|v| v.as_i64());
    let currency = raw
        .get("currency")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let status = raw
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(RefundRecord {
        refund_id,
        payment_intent,
        session_id: None,
        created,
        amount_cents,
        currency,
        status,
    })
}

pub(crate) fn dispute_from_json(raw: &Value) -> Option<DisputeRecord> {
    let dispute_id = raw.get("id").and_then(|v| v.as_str())?.to_string();
    let payment_intent = raw
        .get("payment_intent")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let created = raw.get("created").and_then(|v| v.as_i64()).unwrap_or(0);
    let amount_cents = raw.get("amount").and_then(|v| v.as_i64());
    let currency = raw
        .get("currency")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let status = raw
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(DisputeRecord {
        dispute_id,
        payment_intent,
        session_id: None,
        created,
        amount_cents,
        currency,
        status,
    })
}

fn unparsed_from_raw(raw: &Value, reason: String) -> UnparsedSession {
    let created = raw.get("created").and_then(|v| v.as_i64()).unwrap_or(0);
    let session_id = raw
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("unknown-{created}"));
    let amount_cents = raw
        .get("amount_total")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let currency = raw
        .get("currency")
        .and_then(|v| v.as_str())
        .unwrap_or("cad")
        .to_string();
    UnparsedSession {
        session_id,
        created,
        amount_cents,
        currency,
        reason,
    }
}

/// Build a PaidSession from the session object and explicitly fetched line items.
/// Attribution uses `price.id` only — never session/line metadata.
fn build_paid_session(raw: &Value, line_items: &[Value]) -> Result<PaidSession, String> {
    let session_id = raw
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "session missing id".to_string())?
        .to_string();

    let mut lines = Vec::new();
    for li in line_items {
        let quantity = li.get("quantity").and_then(|q| q.as_i64()).unwrap_or(0);
        if quantity < 1 {
            continue;
        }
        let price_id = li
            .pointer("/price/id")
            .and_then(|v| v.as_str())
            .or_else(|| li.get("price").and_then(|v| v.as_str()))
            .ok_or_else(|| "line item missing price id".to_string())?
            .to_string();
        let amount_cents = li
            .get("amount_total")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| {
                let unit = li
                    .pointer("/price/unit_amount")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                unit * quantity
            });
        lines.push(PaidLine {
            price_id,
            quantity,
            amount_cents,
        });
    }
    if lines.is_empty() {
        return Err("session has no line item quantity".to_string());
    }

    let amount_cents = raw
        .get("amount_total")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| lines.iter().map(|l| l.amount_cents).sum());
    let currency = raw
        .get("currency")
        .and_then(|v| v.as_str())
        .unwrap_or("cad")
        .to_string();
    let payment_intent = match raw.get("payment_intent") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Object(obj)) => obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };
    let customer_email = raw
        .pointer("/customer_details/email")
        .and_then(|v| v.as_str())
        .or_else(|| raw.get("customer_email").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    let created = raw.get("created").and_then(|v| v.as_i64()).unwrap_or(0);
    let paid_at = chrono::DateTime::from_timestamp(created, 0)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(crate::db::utc_now_rfc3339);

    let client_reference = raw
        .get("client_reference_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let payment_link = match raw.get("payment_link") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Object(obj)) => obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };

    Ok(PaidSession {
        session_id,
        payment_intent,
        lines,
        currency,
        customer_email,
        paid_at,
        created,
        amount_cents,
        client_reference,
        payment_link,
    })
}

fn looks_like_unix(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

pub(crate) fn redact_secrets(message: &str, key: &str) -> String {
    let mut out = message.to_string();
    if !key.is_empty() {
        out = out.replace(key, "[redacted]");
    }
    // Belt-and-suspenders: never echo email addresses from Stripe error bodies.
    let re_chars: Vec<char> = out.chars().collect();
    // Simple pass: if we ever interpolated an email, scrub common pattern.
    if out.contains('@') {
        out = scrub_emails(&out);
    }
    let _ = re_chars;
    out
}

fn scrub_emails(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '%' || c == '+' || c == '-' {
            let mut token = String::new();
            token.push(c);
            while let Some(&n) = chars.peek() {
                if n.is_ascii_alphanumeric()
                    || n == '.'
                    || n == '_'
                    || n == '%'
                    || n == '+'
                    || n == '-'
                    || n == '@'
                {
                    token.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            if token.contains('@') && token.split('@').count() == 2 {
                out.push_str("[redacted]");
            } else {
                out.push_str(&token);
            }
        } else {
            out.push(c);
        }
    }
    out
}

// --- Test transport --------------------------------------------------------

#[cfg(test)]
pub mod fake_http {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeHttp {
        /// path -> queue of page JSON responses (or Err strings stored as `{"__err":"..."}`).
        pub get_pages: Mutex<HashMap<String, VecDeque<Result<Value, String>>>>,
        pub posts: Mutex<Vec<(String, String)>>,
    }

    impl FakeHttp {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn push_get(&self, path: &str, page: Value) {
            self.get_pages
                .lock()
                .unwrap()
                .entry(path.to_string())
                .or_default()
                .push_back(Ok(page));
        }

        pub fn push_get_err(&self, path: &str, err: impl Into<String>) {
            self.get_pages
                .lock()
                .unwrap()
                .entry(path.to_string())
                .or_default()
                .push_back(Err(err.into()));
        }
    }

    impl StripeHttp for FakeHttp {
        fn get(&self, path: &str, _query: &[(&str, &str)]) -> Result<Value, String> {
            let mut map = self.get_pages.lock().unwrap();
            let q = map
                .get_mut(path)
                .ok_or_else(|| format!("fake http: no stub for GET {path}"))?;
            q.pop_front()
                .ok_or_else(|| format!("fake http: no more pages for GET {path}"))?
        }

        fn post(
            &self,
            path: &str,
            _form: &[(&str, &str)],
            idempotency_key: &str,
        ) -> Result<Value, String> {
            self.posts
                .lock()
                .unwrap()
                .push((path.to_string(), idempotency_key.to_string()));
            Err("fake http: post not stubbed".into())
        }
    }
}
