use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crop {
    pub id: String,
    pub name: String,
    pub growth_days: i64,
    pub blackout_days: i64,
    pub expected_yield_oz: f64,
    pub sort_order: i64,
    /// Oz of seed per 10×20 tray. NULL = no pre-fill proposal.
    pub seed_rate_oz_per_tray: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayView {
    pub id: String,
    pub crop_id: String,
    pub crop_name: String,
    pub state: String,
    pub quantity: i64,
    pub growth_days_at_sow: Option<i64>,
    pub blackout_days_at_sow: Option<i64>,
    pub planned_on: Option<String>,
    pub sown_on: Option<String>,
    pub blackout_on: Option<String>,
    pub light_on: Option<String>,
    pub harvested_on: Option<String>,
    pub discarded_on: Option<String>,
    pub actual_yield_oz: Option<f64>,
    pub expected_harvest_date: Option<String>,
    pub cover_check_date: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// D21a — ready-on-or-before is COVER math only. What the retail door
/// publishes keeps exact-date credit, so one tray can never be offered
/// on two dates.
/// D22a — one row per (harvest_date, crop) carries two named figures:
/// `remaining_trays` (exact-date, sale side, today's meaning per crop)
/// and `cover_remaining` (ready-on-or-before, net of promises, cover side).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityRow {
    pub harvest_date: String,
    /// D3: the pool is per (date, crop). A Kale tray never covers a Sunflower
    /// promise. Amends GT-D14's "one pool" — that ruling treated the date as
    /// a single tray pool; the pool is now keyed by crop as well.
    pub crop_id: String,
    pub crop_name: String,
    /// Unharvested trays whose OWN nominal date is this date. Unchanged meaning.
    pub trays: i64,
    pub expected_yield_oz: f64,
    /// Still claimed on THIS date after this date's own evidence: retail
    /// outstanding (time-ordered, unchanged) + wholesale outstanding.
    pub sold_trays: i64,
    /// `trays - sold_trays`. EXACT-DATE, sale side (D21a): what offers, the shop
    /// page and reconciliation may publish. Never the cover answer.
    pub remaining_trays: i64,
    /// Harvested trays whose nominal date is this date. Unchanged meaning.
    pub harvested_trays: i64,
    /// D4(b): unharvested trays of this crop ready ON OR BEFORE this date.
    pub cover_supply: i64,
    /// Promises for this crop through this date that harvest evidence has not
    /// discharged. D2(b): delivery is not evidence. D5(1): evidence counts from
    /// the day the trays left the shelf, not from their nominal date.
    pub cover_promised: i64,
    /// `cover_supply - cover_promised`. THE cover answer: COVER, Health M2, the
    /// record-time signal and (in IV-b) sow prefill read this and nothing else.
    pub cover_remaining: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfCapacity {
    /// None means unknown. The ceiling does not constrain anything until
    /// the operator supplies a real number. An empty cell beats a 0.
    pub light_slots: Option<i64>,
    pub blackout_slots: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoneyStatus {
    pub configured: bool,
    pub mode: Option<String>,
    pub account_name: Option<String>,
    pub last_poll_ok: Option<String>,
    pub last_poll_err: Option<String>,
    pub open_order_count: i64,
    /// Public Worker URL the shop page POSTs carts to. Not a secret.
    pub checkout_endpoint_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StripeAccountPreview {
    pub account_id: String,
    pub account_name: String,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferView {
    pub id: Option<String>,
    pub harvest_date: String,
    pub crop_id: String,
    pub crop_name: String,
    pub price_cents: Option<i64>,
    pub stripe_price_id: Option<String>,
    /// Legacy harvest Payment Link URL — unused after cart checkout; kept None.
    pub stripe_link_url: Option<String>,
    pub available: i64,
    pub sold: i64,
    pub remaining: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShopPage {
    pub file_path: String,
    pub size_bytes: i64,
    pub generated_at: String,
    pub harvest_dates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderView {
    pub id: String,
    pub stripe_session_id: String,
    pub stripe_payment_intent: Option<String>,
    pub harvest_date: String,
    pub crop_id: String,
    pub crop_name: String,
    pub quantity: i64,
    pub amount_cents: i64,
    pub currency: String,
    pub customer_email: Option<String>,
    pub state: String,
    pub capacity_consumed: i64,
    pub client_reference: Option<String>,
    pub paid_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppliedOutcome {
    Applied {
        order_id: String,
    },
    AlreadyApplied,
    /// Insert failed for a durable reason (e.g. unknown crop). Attention raised; poll may advance.
    Rejected {
        session_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    pub undoes_seq: i64,
    pub undone_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveToLight {
    pub tray_ids: Vec<String>,
    pub tray_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarvestGroup {
    pub crop_id: String,
    pub crop_name: String,
    pub tray_ids: Vec<String>,
    pub tray_count: i64,
    pub estimated_yield_oz: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarvestSummary {
    pub tray_count: i64,
    pub variety_count: i64,
    pub estimated_yield_oz: f64,
    pub single_crop_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarvestInput {
    pub tray_ids: Vec<String>,
    pub actual_yield_oz: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextEvent {
    pub kind: String,
    pub date: String,
    pub tray_count: i64,
    pub crop_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodayView {
    pub move_to_light: Option<MoveToLight>,
    pub harvests: Vec<HarvestGroup>,
    pub harvest_summary: Option<HarvestSummary>,
    pub next_events: Vec<NextEvent>,
    pub active_tray_count: i64,
    /// True when any active tray has sown_on == today. Distinguishes Script A
    /// idle copy (state B) from the generic next-event line (state C).
    pub sown_today: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotInfo {
    pub file_name: String,
    pub path: String,
    pub taken_at: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FarmLocation {
    pub farm_db_path: String,
    pub folder_path: String,
    pub last_snapshot_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecountCrop {
    pub crop_id: String,
    pub crop_name: String,
    pub app_quantity: i64,
    pub tray_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecountEntry {
    pub crop_id: String,
    pub counted_quantity: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecountCropChange {
    pub crop_id: String,
    pub crop_name: String,
    pub quantity: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecountResult {
    pub adjusted_down: Vec<RecountCropChange>,
    pub adjusted_up: Vec<RecountCropChange>,
    pub unchanged: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionItem {
    pub id: String,
    pub kind: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub message: String,
    pub actions: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveResult {
    pub tray_ids: Vec<String>,
    /// Set when resolving `open_in_stripe` — Stripe dashboard URL for the frontend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationOrder {
    pub id: String,
    pub crop_name: String,
    pub quantity: i64,
    pub state: String,
    pub capacity_consumed: i64,
    pub amount_cents: i64,
    pub paid_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationDate {
    pub harvest_date: String,
    pub available: i64,
    pub sold: i64,
    pub remaining: i64,
    pub orders: Vec<ReconciliationOrder>,
}
