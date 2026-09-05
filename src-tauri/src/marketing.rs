//! Marketing domain — venues, samples, touches, follow-ups, stages, reviews,
//! review asks (GT-D15).
//!
//! Money is unrepresentable: payloads reuse consumption::FORBIDDEN_MONETARY_KEYS
//! and are sealed with deny_unknown_fields. No trays/capacity/register writes.
//! A standing order is trays per week and varieties — physical only.

use crate::attention;
use crate::capacity_gate::{self, CapacityState};
use crate::consumption::FORBIDDEN_MONETARY_KEYS;
use crate::db;
use crate::events::{EventRecord, Kind};
use crate::projection;
use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Re-export the consumption forbidden list so the two seals cannot drift.
pub const MARKETING_FORBIDDEN_MONETARY_KEYS: &[&str] = FORBIDDEN_MONETARY_KEYS;

pub const MKT_VENUES_COLUMNS: &[&str] = &[
    "venue_id",
    "name",
    "venue_type",
    "contact",
    "phone",
    "address",
    "note",
    "archived_at",
    "created_at",
    "updated_at",
];

pub const MKT_SAMPLES_COLUMNS: &[&str] = &[
    "sample_id",
    "venue_id",
    "dropped_on",
    "varieties",
    "pack_count",
    "note",
    "created_at",
    "token",
];

pub const MKT_TOUCHES_COLUMNS: &[&str] = &[
    "touch_id",
    "venue_id",
    "touched_on",
    "channel",
    "outcome",
    "note",
    "created_at",
];

pub const MKT_FOLLOWUPS_COLUMNS: &[&str] = &[
    "followup_id",
    "venue_id",
    "due_on",
    "what",
    "cleared_at",
    "created_at",
    "updated_at",
];

pub const MKT_STAGES_COLUMNS: &[&str] = &[
    "venue_id",
    "stage",
    "trays_week",
    "varieties",
    "variety_targets",
    "changed_on",
    "note",
    "updated_at",
];

pub const MKT_REVIEWS_COLUMNS: &[&str] = &[
    "observation_id",
    "observed_on",
    "count",
    "source",
    "created_at",
];

pub const MKT_REVIEW_REQUESTS_COLUMNS: &[&str] =
    &["venue_id", "decided_on", "outcome", "created_at"];

/// GT-D17. Compared by verify-replay (projection/verify.rs) like every mkt_ table.
pub const MKT_STANDING_REQUESTS_COLUMNS: &[&str] = &[
    "request_id",
    "token",
    "venue_id",
    "varieties",
    "bags_per_cycle",
    "requested_at",
    "contact",
    "created_at",
    "decided_at",
    "outcome",
];

pub const TOUCH_CHANNELS: &[&str] = &["visit", "call", "text", "email", "ig"];

/// Closed pipeline ladder (also enforced by DB CHECK).
pub const STAGE_LADDER: &[&str] = &[
    "scouted", "sampled", "talking", "trial", "standing", "dormant", "passed",
];

/// Cap on weekly action lines shown; dropped count is always reported.
pub const WEEKLY_ACTIONS_CAP: usize = 12;
/// P7-CLOSE (signed 2026-08-25) - the two Phase 7 close-out lines. One set of
/// bytes each: the builder pushes them and p7t26 asserts them, so the sentence
/// the operator reads cannot drift from the sentence under test. Same class as
/// QR_FIELDS_REFUSAL below.
pub const REVIEW_STALE_TITLE: &str = "Update Google review count — last observed over 30 days ago";
pub const GBP_VERIFY_TITLE: &str = "Record Google Business Profile verification date";

/// Customer QR fence 1 (brief 2026-08-17, ruling 9). ONE set of bytes: read by
/// the drop_sample gate and by the Marketing screen through
/// `commands::sample_gate_line` (the unpriced_settlement_line precedent), so the
/// refusal the operator reads and the refusal they hit cannot drift.
/// Handler-only — never inside validate_marketing_event, which runs on replay.
pub const QR_FIELDS_REFUSAL: &str =
    "Venue name, delivery address, and contact are required before a QR can be generated.";

fn has_text(v: Option<&str>) -> bool {
    v.map(str::trim).map(|s| !s.is_empty()).unwrap_or(false)
}

/// Signed 2026-08-17 item 4: contact = (contact OR phone) non-empty after trim.
/// Name and delivery address are each required on their own.
pub fn venue_ready_for_qr(v: &VenueView) -> bool {
    !v.name.trim().is_empty()
        && has_text(v.address.as_deref())
        && (has_text(v.contact.as_deref()) || has_text(v.phone.as_deref()))
}

/// Signed 2026-08-17 item 3: token = lowercase hex of the first 16 bytes of
/// SHA-256(sample_id ‖ ":" ‖ fresh UUIDv4). 32 chars, URL-safe, unguessable, not
/// reversible to sample_id. Handler-only: the entropy comes from handler_new_id,
/// which apply_* forbids, and the result is frozen in the payload so replay
/// never recomputes it. Raw sample_id never leaves the farm on a pack — only
/// this token will.
pub fn sample_token(sample_id: &str, entropy: &str) -> String {
    let digest = Sha256::digest(format!("{sample_id}:{entropy}").as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VenuePayload {
    pub venue_id: String,
    pub name: String,
    pub venue_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[allow(dead_code)] // H-10: schema seal, held by marketing_tests::m3_field_names_match_structs.
pub const VENUE_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "venue_id",
    "name",
    "venue_type",
    "contact",
    "phone",
    "address",
    "note",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VenueArchivePayload {
    pub venue_id: String,
    pub archived_at: String,
}

#[allow(dead_code)] // H-10: schema seal, held by marketing_tests::m3_field_names_match_structs.
pub const VENUE_ARCHIVE_PAYLOAD_FIELD_NAMES: &[&str] = &["venue_id", "archived_at"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SamplePayload {
    pub sample_id: String,
    pub venue_id: String,
    pub dropped_on: String,
    pub varieties: Vec<String>,
    pub pack_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Customer QR fence 1 (brief 2026-08-17, ruling 2). Opaque per-drop token,
    /// derived in the handler and frozen here so replay reproduces it. Optional:
    /// drops recorded before this fence carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[allow(dead_code)] // H-10: schema seal, held by marketing_tests::m3_field_names_match_structs.
pub const SAMPLE_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "sample_id",
    "venue_id",
    "dropped_on",
    "varieties",
    "pack_count",
    "note",
    "token",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TouchPayload {
    pub touch_id: String,
    pub venue_id: String,
    pub touched_on: String,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[allow(dead_code)] // H-10: schema seal, held by marketing_tests::m3_field_names_match_structs.
pub const TOUCH_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "touch_id",
    "venue_id",
    "touched_on",
    "channel",
    "outcome",
    "note",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FollowupSetPayload {
    pub followup_id: String,
    pub venue_id: String,
    pub due_on: String,
    pub what: String,
}

#[allow(dead_code)] // H-10: schema seal, held by marketing_tests::m3_field_names_match_structs.
pub const FOLLOWUP_SET_PAYLOAD_FIELD_NAMES: &[&str] =
    &["followup_id", "venue_id", "due_on", "what"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FollowupClearPayload {
    pub followup_id: String,
    pub cleared_at: String,
}

#[allow(dead_code)] // H-10: schema seal, held by marketing_tests::m3_field_names_match_structs.
pub const FOLLOWUP_CLEAR_PAYLOAD_FIELD_NAMES: &[&str] = &["followup_id", "cleared_at"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagePayload {
    pub venue_id: String,
    pub stage: String,
    pub changed_on: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trays_week: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub varieties: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variety_targets: Option<std::collections::BTreeMap<String, i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[allow(dead_code)] // H-10: schema seal, held by cadence_tests::g4_no_score_rating_index_or_money_names.
pub const STAGE_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "venue_id",
    "stage",
    "changed_on",
    "trays_week",
    "varieties",
    "variety_targets",
    "note",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewsPayload {
    pub observation_id: String,
    pub observed_on: String,
    pub count: i64,
    pub source: String,
}

#[allow(dead_code)] // H-10: schema seal, held by cadence_tests::g4_no_score_rating_index_or_money_names.
pub const REVIEWS_PAYLOAD_FIELD_NAMES: &[&str] =
    &["observation_id", "observed_on", "count", "source"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewRequestPayload {
    pub venue_id: String,
    pub decided_on: String,
    pub outcome: String, // "asked" | "skipped"
}

#[allow(dead_code)] // H-10: schema seal, held by marketing_tests::m3_field_names_match_structs.
pub const REVIEW_REQUESTED_PAYLOAD_FIELD_NAMES: &[&str] = &["venue_id", "decided_on", "outcome"];

/// GT-D17 — a standing-request candidate as observed from the scan endpoint.
/// Frozen at ingest: venue_id and varieties are copied from the token's sample
/// so a later correction cannot change what the chef saw. `request_id` is the
/// endpoint's stable submission id (a re-pull cannot mint a second candidate).
/// `requested_at` is the endpoint's clock; the event's created_at is ours.
/// `contact` is the single missing piece the customer page may ask for.
/// No price key can exist here (marketing tier, GT-D1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StandingRequestPayload {
    pub request_id: String,
    pub token: String,
    pub venue_id: String,
    pub varieties: Vec<String>,
    pub bags_per_cycle: i64,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
}

#[allow(dead_code)] // H-10: schema seal, held by marketing_tests::m3_field_names_match_structs.
pub const STANDING_REQUEST_PAYLOAD_FIELD_NAMES: &[&str] = &[
    "request_id",
    "token",
    "venue_id",
    "varieties",
    "bags_per_cycle",
    "requested_at",
    "contact",
];

/// GT-D17, second kind — the operator's decision on a candidate. decided_at is
/// the event's own created_at, carried in the payload so apply stays
/// lookup-free (the followup.cleared / clearedAt precedent).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StandingRequestDecidedPayload {
    pub request_id: String,
    pub outcome: String, // "accepted" | "dismissed"
    pub decided_at: String,
}

#[allow(dead_code)] // H-10: schema seal, held by marketing_tests::m3_field_names_match_structs.
pub const STANDING_REQUEST_DECIDED_PAYLOAD_FIELD_NAMES: &[&str] =
    &["request_id", "outcome", "decided_at"];

// ---- R3 harvest ↔ order link (GT-D19) ----
/// One commitment a harvest may be marked as covering: a standing venue
/// (`kind` = "standing", `id` = venue_id) or a wholesale order
/// (`kind` = "wholesale", `id` = order id). Per commitment line, never per
/// order line (signed item 6).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageRef {
    pub kind: String,
    pub id: String,
}
/// GT-D19. Keys by (crop_id, harvested_on) — the day's harvest of that crop,
/// not an event id (signed item 3). `covers` may be empty: "Undo mark" writes
/// the reduced list as a new event; the newest per key is the truth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarvestCoveredPayload {
    pub coverage_id: String,
    pub crop_id: String,
    pub harvested_on: String,
    pub covers: Vec<CoverageRef>,
}
#[allow(dead_code)] // H-10: schema seal, held by r3_tests::r3a_kind_in_closed_set_table_compared_payload_sealed.
pub const HARVEST_COVERED_PAYLOAD_FIELD_NAMES: &[&str] =
    &["coverage_id", "crop_id", "harvested_on", "covers"];
pub const HARVEST_COVERAGE_COLUMNS: &[&str] = &[
    "coverage_id",
    "crop_id",
    "harvested_on",
    "covers",
    "created_at",
];
pub const COVERAGE_KINDS: &[&str] = &["standing", "wholesale"];
/// Signed item 4: wholesale orders in state `ordered` with harvest_date up to
/// this many days ahead (overdue included, flagged).
pub const NEARBY_WHOLESALE_DAYS: i64 = 3;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitmentLine {
    pub kind: String,
    pub id: String,
    /// The line bytes (signed item 2). "Covering — " is the UI's prefix.
    pub text: String,
    pub covered: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarvestCommitmentsView {
    pub crop_id: String,
    pub crop_name: String,
    pub harvested_on: String,
    pub header: String,
    pub empty_line: String,
    pub lines: Vec<CommitmentLine>,
}

/// The attention kind for an undecided candidate. Marketing surface only
/// (surfaces.ts MARKETING set); never a Today card.
pub const STANDING_REQUEST_ATTENTION_KIND: &str = "marketing.standing_request";

/// Signed 2026-08-17 (fence 3, ruling 8): the accept gate. Handler-only in
/// decide_standing_request; never inside validate_marketing_event.
pub const STANDING_ACCEPT_REFUSAL: &str =
    "Standing cannot go live while venue, address, contact, or quantity is incomplete.";

/// Signed 2026-08-17 (fence 3, item 4): handler backstop for a double decision.
pub const STANDING_REQUEST_ALREADY_DECIDED: &str =
    "This standing request has already been decided.";

/// Signed 2026-08-17 (fence 3, item 4): the generic dismiss / resolve doors
/// refuse this kind, so no path closes the card without a durable outcome.
pub const STANDING_REQUEST_DECIDE_ONLY: &str =
    "A standing request is decided with Accept or Dismiss on the request itself.";

/// GT-D18: the operator-side refusal when no endpoint is configured.
pub const QR_LINK_NEEDS_ENDPOINT: &str = "Configure a scan endpoint URL before making a QR link.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandingRequestView {
    pub request_id: String,
    pub token: String,
    pub venue_id: String,
    pub venue_name: String,
    pub varieties: Vec<String>,
    pub bags_per_cycle: i64,
    pub requested_at: String,
    pub contact: Option<String>,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandingRequestDecision {
    pub request_id: String,
    pub outcome: String,
    pub decided_at: String,
    /// Present only when accepted: the standing row as change_stage left it.
    pub stage: Option<StageView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VenueView {
    pub venue_id: String,
    pub name: String,
    pub venue_type: String,
    pub contact: Option<String>,
    pub phone: Option<String>,
    pub address: Option<String>,
    pub note: Option<String>,
    pub archived_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Customer QR fence 1: name + delivery address + (contact OR phone) present.
    /// ONE evaluator (venue_ready_for_qr) read by the drop gate and by the
    /// Marketing completion door, so they cannot drift.
    pub qr_ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleView {
    pub sample_id: String,
    pub venue_id: String,
    pub venue_name: String,
    pub dropped_on: String,
    pub varieties: Vec<String>,
    pub pack_count: i64,
    pub note: Option<String>,
    pub created_at: String,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TouchView {
    pub touch_id: String,
    pub venue_id: String,
    pub venue_name: String,
    pub touched_on: String,
    pub channel: String,
    pub outcome: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowupView {
    pub followup_id: String,
    pub venue_id: String,
    pub venue_name: String,
    pub due_on: String,
    pub what: String,
    pub cleared_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub attention_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageView {
    pub venue_id: String,
    pub venue_name: String,
    pub stage: String,
    pub trays_week: Option<i64>,
    pub varieties: Option<Vec<String>>,
    pub variety_targets: Option<std::collections::BTreeMap<String, i64>>,
    pub changed_on: String,
    pub note: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewObservationView {
    pub observation_id: String,
    pub observed_on: String,
    pub count: i64,
    pub source: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRequestView {
    pub venue_id: String,
    pub venue_name: String,
    pub decided_on: String,
    pub outcome: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketingSummary {
    /// Exact TtFSO line for the Marketing page first element.
    pub time_to_first_standing_order: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VarietyDemand {
    pub name: String,
    pub trays_week: i64,
    pub sown_last_7_days: i64,
    pub shortfall: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandingDemandView {
    pub standing_venues: i64,
    pub trays_week: i64,       // named targets + legacy totals (display)
    pub sown_last_7_days: i64, // all crops, window, non-discarded
    pub shortfall: i64,        // SUM of per-variety shortfalls (named only)
    pub standing_varieties: Vec<String>,
    pub varieties: Vec<VarietyDemand>,
    pub unallocated_venues: Vec<String>,
    pub unallocated_trays_week: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeeklyAction {
    pub kind: String,
    pub title: String,
    pub venue_id: Option<String>,
    pub attention_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeeklyActionsView {
    pub actions: Vec<WeeklyAction>,
    pub dropped: usize,
    pub pitching_advice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReputationCounts {
    pub active_warm_venues: i64,
    pub samples_outstanding: i64,
    pub standing_orders: i64,
    pub standing_trays_week: i64,
    pub overdue_followups: i64,
    pub google_review_count: Option<i64>,
    pub google_review_observed_on: Option<String>,
}

fn is_marketing_kind(kind: Kind) -> bool {
    matches!(
        kind.tier().0,
        crate::event_partition::EventDomain::Marketing
    )
}

fn reject_forbidden_keys(payload: &Value) -> Result<(), String> {
    let obj = payload
        .as_object()
        .ok_or_else(|| "marketing payload must be an object".to_string())?;
    for key in obj.keys() {
        if MARKETING_FORBIDDEN_MONETARY_KEYS
            .iter()
            .any(|f| f.eq_ignore_ascii_case(key))
        {
            return Err(format!("marketing rejects monetary payload key: {key}"));
        }
    }
    Ok(())
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

fn add_days(yyyy_mm_dd: &str, days: i64) -> Result<String, String> {
    validate_calendar_date(yyyy_mm_dd, "date")?;
    let d = NaiveDate::parse_from_str(yyyy_mm_dd, "%Y-%m-%d").map_err(|e| e.to_string())?;
    Ok((d + chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string())
}

/// Choke-point gate for a full marketing event record.
pub fn validate_marketing_event(event: &EventRecord) -> Result<(), String> {
    if !is_marketing_kind(event.kind) {
        return Ok(());
    }
    reject_forbidden_keys(&event.payload)?;
    match event.kind {
        Kind::VenueRecorded | Kind::VenueCorrected => {
            let p: VenuePayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("venue payload refused: {e}"))?;
            if p.name.trim().is_empty() {
                return Err("venue name must be non-empty".into());
            }
            if p.venue_type.trim().is_empty() {
                return Err("venue_type must be non-empty".into());
            }
        }
        Kind::VenueArchived => {
            let _: VenueArchivePayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("venue.archived payload refused: {e}"))?;
        }
        Kind::SampleDropped => {
            let p: SamplePayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("sample.dropped payload refused: {e}"))?;
            validate_calendar_date(&p.dropped_on, "dropped_on")?;
            if p.pack_count <= 0 {
                return Err("pack_count must be greater than zero".into());
            }
            if p.varieties.is_empty() {
                return Err("varieties must not be empty".into());
            }
        }
        Kind::TouchLogged => {
            let p: TouchPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("touch.logged payload refused: {e}"))?;
            validate_calendar_date(&p.touched_on, "touched_on")?;
            if !TOUCH_CHANNELS.contains(&p.channel.as_str()) {
                return Err(format!(
                    "channel must be one of visit|call|text|email|ig, got {}",
                    p.channel
                ));
            }
        }
        Kind::FollowupSet => {
            let p: FollowupSetPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("followup.set payload refused: {e}"))?;
            validate_calendar_date(&p.due_on, "due_on")?;
            if p.what.trim().is_empty() {
                return Err("follow-up what must be non-empty".into());
            }
        }
        Kind::FollowupCleared => {
            let _: FollowupClearPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("followup.cleared payload refused: {e}"))?;
        }
        Kind::StageChanged => {
            let p: StagePayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("stage.changed payload refused: {e}"))?;
            validate_stage_fields(&p)?;
        }
        Kind::ReviewsObserved => {
            let p: ReviewsPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("reviews.observed payload refused: {e}"))?;
            validate_calendar_date(&p.observed_on, "observed_on")?;
            if p.count < 0 {
                return Err("review count must be >= 0".into());
            }
            if p.source.trim().is_empty() {
                return Err("review source must be non-empty".into());
            }
        }
        Kind::ReviewRequested => {
            let p: ReviewRequestPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("review.requested payload refused: {e}"))?;
            validate_calendar_date(&p.decided_on, "decided_on")?;
            if p.outcome != "asked" && p.outcome != "skipped" {
                return Err(format!(
                    "outcome must be asked or skipped, got {}",
                    p.outcome
                ));
            }
        }
        Kind::StandingRequested => {
            let p: StandingRequestPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("standing.requested payload refused: {e}"))?;
            if p.request_id.trim().is_empty() {
                return Err("request_id must be non-empty".into());
            }
            if p.token.len() != 32 || !p.token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
                return Err("token must be 32 lowercase hex characters".into());
            }
            if p.venue_id.trim().is_empty() {
                return Err("venue_id must be non-empty".into());
            }
            if p.varieties.is_empty() {
                return Err("varieties must not be empty".into());
            }
            if p.bags_per_cycle < 1 {
                return Err("bags_per_cycle must be at least 1".into());
            }
            chrono::DateTime::parse_from_rfc3339(&p.requested_at)
                .map_err(|_| "requested_at must be RFC3339".to_string())?;
        }
        Kind::StandingRequestDecided => {
            let p: StandingRequestDecidedPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("standing.request_decided payload refused: {e}"))?;
            if p.request_id.trim().is_empty() {
                return Err("request_id must be non-empty".into());
            }
            if p.outcome != "accepted" && p.outcome != "dismissed" {
                return Err(format!(
                    "outcome must be accepted or dismissed, got {}",
                    p.outcome
                ));
            }
            chrono::DateTime::parse_from_rfc3339(&p.decided_at)
                .map_err(|_| "decided_at must be RFC3339".to_string())?;
        }
        Kind::HarvestCovered => {
            let p: HarvestCoveredPayload = serde_json::from_value(event.payload.clone())
                .map_err(|e| format!("harvest.covered payload refused: {e}"))?;
            if p.crop_id.trim().is_empty() {
                return Err("crop_id must be non-empty".into());
            }
            validate_calendar_date(&p.harvested_on, "harvested_on")?;
            for c in &p.covers {
                if !COVERAGE_KINDS.contains(&c.kind.as_str()) {
                    return Err(format!(
                        "coverage kind must be standing or wholesale, got {}",
                        c.kind
                    ));
                }
                if c.id.trim().is_empty() {
                    return Err("coverage id must be non-empty".into());
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_stage_fields(p: &StagePayload) -> Result<(), String> {
    if !STAGE_LADDER.contains(&p.stage.as_str()) {
        return Err(format!(
            "stage must be one of scouted|sampled|talking|trial|standing|dormant|passed, got {}",
            p.stage
        ));
    }
    validate_calendar_date(&p.changed_on, "changed_on")?;
    if p.stage == "standing" {
        match p.trays_week {
            Some(n) if n > 0 => {}
            Some(_) => return Err("standing stage requires trays_week > 0".into()),
            None => return Err("standing stage requires trays_week".into()),
        }
        if let Some(vt) = &p.variety_targets {
            if vt.is_empty() {
                return Err("variety_targets must not be empty".into());
            }
            if vt.values().any(|n| *n < 1) {
                return Err("every variety target must be at least 1 tray/week".into());
            }
            let sum: i64 = vt.values().sum();
            if Some(sum) != p.trays_week {
                return Err("variety targets must sum to trays_week".into());
            }
            if let Some(vs) = &p.varieties {
                let keys: std::collections::BTreeSet<_> = vt.keys().cloned().collect();
                let listed: std::collections::BTreeSet<_> = vs.iter().cloned().collect();
                if keys != listed {
                    return Err("varieties must exactly match the variety target keys".into());
                }
            }
        }
    } else if p.trays_week.is_some() || p.varieties.is_some() || p.variety_targets.is_some() {
        return Err(
            "trays_week, varieties and variety_targets are only allowed for standing".into(),
        );
    }
    Ok(())
}

fn write_pair(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    projection::apply_event(tx, event)?;
    crate::events::write_event(tx, event)?;
    Ok(())
}

pub fn record_venue(
    conn: &mut Connection,
    name: &str,
    venue_type: &str,
    contact: Option<String>,
    phone: Option<String>,
    address: Option<String>,
    note: Option<String>,
) -> Result<VenueView, String> {
    let venue_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let mut payload = json!({
        "venueId": venue_id,
        "name": name.trim(),
        "venueType": venue_type.trim(),
    });
    let obj = payload.as_object_mut().unwrap();
    if let Some(c) = contact {
        obj.insert("contact".into(), json!(c));
    }
    if let Some(p) = phone {
        obj.insert("phone".into(), json!(p));
    }
    if let Some(a) = address {
        obj.insert("address".into(), json!(a));
    }
    if let Some(n) = note {
        obj.insert("note".into(), json!(n));
    }
    let event = EventRecord::originated(
        Kind::VenueRecorded,
        "venue",
        venue_id.clone(),
        payload,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_venue(conn, &venue_id)
}

#[allow(clippy::too_many_arguments)] // H-7 Class D: mirrors its Tauri command's arg list; both narrow together under H-7b.
pub fn correct_venue(
    conn: &mut Connection,
    venue_id: &str,
    name: &str,
    venue_type: &str,
    contact: Option<String>,
    phone: Option<String>,
    address: Option<String>,
    note: Option<String>,
) -> Result<VenueView, String> {
    let _ = get_venue(conn, venue_id)?;
    let created_at = projection::handler_now();
    let mut payload = json!({
        "venueId": venue_id,
        "name": name.trim(),
        "venueType": venue_type.trim(),
    });
    let obj = payload.as_object_mut().unwrap();
    if let Some(c) = contact {
        obj.insert("contact".into(), json!(c));
    }
    if let Some(p) = phone {
        obj.insert("phone".into(), json!(p));
    }
    if let Some(a) = address {
        obj.insert("address".into(), json!(a));
    }
    if let Some(n) = note {
        obj.insert("note".into(), json!(n));
    }
    let event = EventRecord::originated(
        Kind::VenueCorrected,
        "venue",
        venue_id.to_string(),
        payload,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_venue(conn, venue_id)
}

#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn archive_venue(conn: &mut Connection, venue_id: &str) -> Result<VenueView, String> {
    let _ = get_venue(conn, venue_id)?;
    let archived_at = projection::handler_now();
    let event = EventRecord::originated(
        Kind::VenueArchived,
        "venue",
        venue_id.to_string(),
        json!({ "venueId": venue_id, "archivedAt": archived_at }),
        json!({ "op": "none" }),
        archived_at.clone(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_venue(conn, venue_id)
}

/// Drop a sample and schedule a follow-up at dropped_on + 3 days (GT-D6) in one tx.
pub fn drop_sample(
    conn: &mut Connection,
    venue_id: &str,
    dropped_on: &str,
    varieties: Vec<String>,
    pack_count: i64,
    note: Option<String>,
) -> Result<SampleView, String> {
    let venue = get_venue(conn, venue_id)?;
    // Customer QR fence 1 (brief 2026-08-17, rulings 5/9). Handler-only, the
    // record_order price-gate pattern (wholesale.rs:446-461): refuse a NEW
    // drop on an incomplete venue; never inside validate_marketing_event,
    // which runs on replay and must keep applying every historic drop.
    if !venue_ready_for_qr(&venue) {
        return Err(QR_FIELDS_REFUSAL.to_string());
    }
    let sample_id = projection::handler_new_id();
    let token = sample_token(&sample_id, &projection::handler_new_id());
    let created_at = projection::handler_now();
    let due_on = add_days(dropped_on, 3)?;
    let mut sample_payload = json!({
        "sampleId": sample_id,
        "venueId": venue_id,
        "droppedOn": dropped_on,
        "varieties": varieties,
        "packCount": pack_count,
        "token": token,
    });
    if let Some(n) = note {
        sample_payload
            .as_object_mut()
            .unwrap()
            .insert("note".into(), json!(n));
    }
    let sample_event = EventRecord::originated(
        Kind::SampleDropped,
        "sample",
        sample_id.clone(),
        sample_payload,
        json!({ "op": "none" }),
        created_at.clone(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let followup_id = projection::handler_new_id();
    let followup_event = EventRecord::originated(
        Kind::FollowupSet,
        "followup",
        followup_id.clone(),
        json!({
            "followupId": followup_id,
            "venueId": venue_id,
            "dueOn": due_on,
            "what": "Check back after sample drop",
        }),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &sample_event)?;
    write_pair(&tx, &followup_event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_sample(conn, &sample_id)
}

pub fn log_touch(
    conn: &mut Connection,
    venue_id: &str,
    touched_on: &str,
    channel: &str,
    outcome: Option<String>,
    note: Option<String>,
) -> Result<TouchView, String> {
    let _ = get_venue(conn, venue_id)?;
    if !TOUCH_CHANNELS.contains(&channel) {
        return Err(format!(
            "channel must be one of visit|call|text|email|ig, got {channel}"
        ));
    }
    let touch_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let mut payload = json!({
        "touchId": touch_id,
        "venueId": venue_id,
        "touchedOn": touched_on,
        "channel": channel,
    });
    let obj = payload.as_object_mut().unwrap();
    if let Some(o) = outcome {
        obj.insert("outcome".into(), json!(o));
    }
    if let Some(n) = note {
        obj.insert("note".into(), json!(n));
    }
    let event = EventRecord::originated(
        Kind::TouchLogged,
        "touch",
        touch_id.clone(),
        payload,
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_touch(conn, &touch_id)
}

#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn set_followup(
    conn: &mut Connection,
    venue_id: &str,
    due_on: &str,
    what: &str,
) -> Result<FollowupView, String> {
    let _ = get_venue(conn, venue_id)?;
    let followup_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let event = EventRecord::originated(
        Kind::FollowupSet,
        "followup",
        followup_id.clone(),
        json!({
            "followupId": followup_id,
            "venueId": venue_id,
            "dueOn": due_on,
            "what": what.trim(),
        }),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_followup(conn, &followup_id)
}

#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn clear_followup(conn: &mut Connection, followup_id: &str) -> Result<FollowupView, String> {
    let _ = get_followup(conn, followup_id)?;
    let cleared_at = projection::handler_now();
    let event = EventRecord::originated(
        Kind::FollowupCleared,
        "followup",
        followup_id.to_string(),
        json!({ "followupId": followup_id, "clearedAt": cleared_at }),
        json!({ "op": "none" }),
        cleared_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_followup(conn, followup_id)
}

#[cfg(test)]
thread_local! {
    pub static FAIL_RESOLVE_AFTER_ATTENTION: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Resolve a marketing.followup_due attention item: log a touch AND clear
/// the follow-up in one tx. Attention entity_id is the followup_id (Phase 1).
pub fn resolve_followup(
    conn: &mut Connection,
    attention_id: &str,
    channel: &str,
    note: Option<String>,
) -> Result<TouchView, String> {
    if !TOUCH_CHANNELS.contains(&channel) {
        return Err(format!(
            "channel must be one of visit|call|text|email|ig, got {channel}"
        ));
    }
    let item = attention::get_open_item(conn, attention_id)?
        .ok_or_else(|| format!("attention item not open: {attention_id}"))?;
    if item.kind != "marketing.followup_due" {
        return Err(format!(
            "resolve_followup only handles marketing.followup_due, got {}",
            item.kind
        ));
    }
    let followup_id = item
        .entity_id
        .clone()
        .ok_or_else(|| "marketing.followup_due missing followup entity_id".to_string())?;
    let followup = get_followup(conn, &followup_id)?;
    let venue_id = followup.venue_id.clone();
    let touch_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let touched_on = db::local_date_today();

    let mut touch_payload = json!({
        "touchId": touch_id,
        "venueId": venue_id,
        "touchedOn": touched_on,
        "channel": channel,
    });
    if let Some(n) = note {
        touch_payload
            .as_object_mut()
            .unwrap()
            .insert("note".into(), json!(n));
    }
    let touch_event = EventRecord::originated(
        Kind::TouchLogged,
        "touch",
        touch_id.clone(),
        touch_payload,
        json!({ "op": "none" }),
        created_at.clone(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let cleared_event = EventRecord::originated(
        Kind::FollowupCleared,
        "followup",
        followup_id.clone(),
        json!({ "followupId": followup_id, "clearedAt": created_at }),
        json!({ "op": "none" }),
        created_at.clone(),
        None,
        None,
        Some(projection::handler_new_id()),
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    attention::resolve_open_in_tx(&tx, attention_id, "logged_touch", &created_at)?;
    #[cfg(test)]
    {
        if FAIL_RESOLVE_AFTER_ATTENTION.with(|c| c.get()) {
            return Err("forced failure after attention resolve".into());
        }
    }
    write_pair(&tx, &touch_event)?;
    write_pair(&tx, &cleared_event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_touch(conn, &touch_id)
}

/// Raise overdue follow-ups into the shared Farm attention queue, keyed per
/// followup_id (Phase 1): each follow-up alerts once; a dismissal sticks per
/// follow-up, never per venue.
pub fn raise_overdue_followups(conn: &Connection) -> Result<(), String> {
    let today = db::local_date_today();
    // Supersede pre-Phase-1 venue-keyed items so every currently-overdue
    // follow-up re-raises under its own id. Idempotent: matches zero rows
    // after the first pass. Bare UPDATE is the established attention
    // pattern (see attention::reopen_attention).
    conn.execute(
        "UPDATE attention
         SET resolved_at = ?1, resolved_by = 'rekeyed_to_followup'
         WHERE kind = 'marketing.followup_due'
           AND entity_type = 'venue'
           AND resolved_at IS NULL",
        params![db::utc_now_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT f.followup_id, v.name, f.what, f.due_on
             FROM mkt_followups f
             JOIN mkt_venues v ON v.venue_id = f.venue_id
             WHERE f.cleared_at IS NULL AND f.due_on <= ?1
             ORDER BY f.due_on, f.followup_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&today], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (followup_id, venue_name, what, due_on) = row.map_err(|e| e.to_string())?;
        let message = format!("Follow-up due at {venue_name} ({due_on}): {what}");
        attention::raise_once(
            conn,
            "marketing.followup_due",
            Some("followup"),
            Some(&followup_id),
            &message,
            &["logged_touch", "dismiss"],
        )?;
    }
    Ok(())
}

/// Phase 3: a standing venue gone quiet interrupts. Keyed per quiet episode
/// (venue_id + last-touch date, or "never") so a dismissal sticks for that
/// episode and a later relapse raises again — the Phase 1 keying lesson.
pub fn raise_standing_quiet(conn: &Connection) -> Result<(), String> {
    let today = db::local_date_today();
    let quiet_before = add_days(&today, -14)?;
    let mut stmt = conn
        .prepare(
            "SELECT s.venue_id, v.name, s.trays_week,
                    (SELECT MAX(t.touched_on) FROM mkt_touches t
                     WHERE t.venue_id = s.venue_id) AS last_touch
             FROM mkt_stages s
             JOIN mkt_venues v ON v.venue_id = s.venue_id
             WHERE v.archived_at IS NULL AND s.stage = 'standing'",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (venue_id, name, trays_week, last_touch) = row.map_err(|e| e.to_string())?;
        let quiet = match &last_touch {
            Some(d) => d.as_str() <= quiet_before.as_str(),
            None => true,
        };
        if !quiet {
            continue;
        }
        let episode = format!("{venue_id}:{}", last_touch.as_deref().unwrap_or("never"));
        let tw = trays_week.unwrap_or(0);
        let message = format!("{name} (standing, {tw} trays/week) — no touch in 14 days.");
        attention::raise_once(
            conn,
            "marketing.standing_quiet",
            Some("venue"),
            Some(&episode),
            &message,
            &["dismiss"],
        )?;
    }
    Ok(())
}

/// Signed 2026-08-17 (fence 3, item 1). Frozen on the attention row at first raise.
pub fn standing_request_message(
    venue: &str,
    varieties: &[String],
    bags_per_cycle: i64,
    contact: Option<&str>,
) -> String {
    let unit = if bags_per_cycle == 1 { "bag" } else { "bags" };
    let mut m = format!(
        "{venue} asks to put {} on standing — {bags_per_cycle} {unit} per cycle.",
        varieties.join(", ")
    );
    if let Some(c) = contact.map(str::trim).filter(|c| !c.is_empty()) {
        m.push_str(&format!(" Contact given: {c}."));
    }
    m
}

/// GT-D17 / fence 3: every undecided candidate is a card on Marketing.
/// Keyed per request_id; `raise` is idempotent while the row is open, and the
/// only closers are the decide paths, so a dismissal is always a durable
/// standing.request_decided. After a restore the attention table is empty and
/// undecided candidates re-raise from mkt_standing_requests — the durable home.
pub fn raise_standing_requests(conn: &Connection) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "SELECT r.request_id, v.name, r.varieties, r.bags_per_cycle, r.contact
             FROM mkt_standing_requests r
             JOIN mkt_venues v ON v.venue_id = r.venue_id
             WHERE r.decided_at IS NULL
             ORDER BY r.created_at, r.request_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (request_id, venue_name, varieties_json, bags, contact) =
            row.map_err(|e| e.to_string())?;
        let varieties: Vec<String> = serde_json::from_str(&varieties_json).unwrap_or_default();
        let message = standing_request_message(&venue_name, &varieties, bags, contact.as_deref());
        attention::raise(
            conn,
            STANDING_REQUEST_ATTENTION_KIND,
            Some("standing_request"),
            Some(&request_id),
            &message,
            &["accept", "dismiss"],
        )?;
    }
    Ok(())
}

/// Customer QR fence 5, signed item 4. What one pulled row became.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    Written,
    Known,
    Refused(&'static str), // "unknown_token" | "invalid"
}

/// The ONE way a pulled candidate enters the farm. Resolves the token against
/// mkt_samples (the desktop is the authority), freezes venue_id + varieties
/// from that sample, and writes standing.requested through the same choke
/// point the dev seed uses — payload exactly GT-D17, no contact. Venue
/// completeness is not checked here: the accept gate owns it. Nothing is
/// invented for an unknown or malformed row; the caller records the refusal.
pub fn ingest_standing_request(
    conn: &mut Connection,
    request_id: &str,
    token: &str,
    bags_per_cycle: i64,
    requested_at: &str,
) -> Result<IngestOutcome, String> {
    if request_id.trim().is_empty() {
        return Ok(IngestOutcome::Refused("invalid"));
    }
    let known: bool = conn
        .query_row(
            "SELECT 1 FROM mkt_standing_requests WHERE request_id = ?1",
            [request_id],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    if known {
        return Ok(IngestOutcome::Known);
    }
    let token_ok = token.len() == 32 && token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
    if !token_ok
        || bags_per_cycle < 1
        || chrono::DateTime::parse_from_rfc3339(requested_at).is_err()
    {
        return Ok(IngestOutcome::Refused("invalid"));
    }
    let sample: Option<(String, String)> = conn
        .query_row(
            "SELECT venue_id, varieties FROM mkt_samples WHERE token = ?1",
            [token],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((venue_id, varieties_json)) = sample else {
        return Ok(IngestOutcome::Refused("unknown_token"));
    };
    let varieties: Vec<String> = serde_json::from_str(&varieties_json).unwrap_or_default();
    if varieties.is_empty() {
        return Ok(IngestOutcome::Refused("invalid"));
    }
    let created_at = projection::handler_now();
    let event = EventRecord::originated(
        Kind::StandingRequested,
        "standing_request",
        request_id.to_string(),
        json!({
            "requestId": request_id,
            "token": token,
            "venueId": venue_id,
            "varieties": varieties,
            "bagsPerCycle": bags_per_cycle,
            "requestedAt": requested_at,
        }),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(IngestOutcome::Written)
}

/// R3 reader (signed items 2, 4, 5, 7). Read-only. Standing lines come from
/// mkt_stages (per venue target for this crop, or the unsplit hint); wholesale
/// lines from wholesale_orders + lines in state `ordered` with harvest_date
/// ≤ today + NEARBY_WHOLESALE_DAYS (overdue flagged). Marks are the newest
/// harvest_coverage row for (crop, today) and are shown only when a non-undone
/// harvest of the crop exists that day. Nothing here reads or writes capacity.
pub fn harvest_commitments(
    conn: &Connection,
    crop_id: &str,
) -> Result<HarvestCommitmentsView, String> {
    harvest_commitments_on(conn, crop_id, &db::local_date_today())
}
pub fn harvest_commitments_on(
    conn: &Connection,
    crop_id: &str,
    today: &str,
) -> Result<HarvestCommitmentsView, String> {
    let crop_name: String = conn
        .query_row("SELECT name FROM crops WHERE id = ?1", [crop_id], |r| {
            r.get(0)
        })
        .map_err(|_| format!("unknown crop: {crop_id}"))?;
    let by_key = crop_key_index(conn)?.by_key;
    let names_this_crop = |key: &str| by_key.get(key).map(|id| id == crop_id).unwrap_or(false);
    let mut lines: Vec<CommitmentLine> = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT s.venue_id, v.name, s.varieties, s.variety_targets
             FROM mkt_stages s
             JOIN mkt_venues v ON v.venue_id = s.venue_id
             WHERE s.stage = 'standing' AND v.archived_at IS NULL
             ORDER BY v.name, s.venue_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (venue_id, name, varieties_json, targets_json) = row.map_err(|e| e.to_string())?;
        let targets = targets_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<BTreeMap<String, i64>>(j).ok());
        if let Some(targets) = targets {
            if let Some((_, n)) = targets.iter().find(|(k, _)| names_this_crop(k)) {
                lines.push(CommitmentLine {
                    kind: "standing".into(),
                    id: venue_id,
                    text: format!("{name} · standing {n}/week"),
                    covered: false,
                });
            }
        } else if let Some(names) = varieties_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
        {
            if names.iter().any(|k| names_this_crop(k)) {
                lines.push(CommitmentLine {
                    kind: "standing".into(),
                    id: venue_id,
                    text: format!("{name} · standing, not split by variety"),
                    covered: false,
                });
            }
        }
    }
    drop(stmt);
    let horizon = add_days(today, NEARBY_WHOLESALE_DAYS)?;
    let mut stmt = conn
        .prepare(
            "SELECT o.id, v.name, o.harvest_date, l.trays
             FROM wholesale_orders o
             JOIN wholesale_order_lines l ON l.order_id = o.id
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE o.state = 'ordered' AND l.crop_id = ?1 AND o.harvest_date <= ?2
             ORDER BY o.harvest_date, o.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![crop_id, horizon], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (order_id, name, harvest_date, trays) = row.map_err(|e| e.to_string())?;
        let when = crate::reachability::format_mon_d_local(&harvest_date)?;
        let unit = if trays == 1 { "tray" } else { "trays" };
        let mut text = format!("{name} · order {when} · {trays} {unit}");
        if harvest_date.as_str() < today {
            text.push_str(" · overdue");
        }
        lines.push(CommitmentLine {
            kind: "wholesale".into(),
            id: order_id,
            text,
            covered: false,
        });
    }
    drop(stmt);
    let harvested_today: bool = conn
        .query_row(
            "SELECT 1 FROM trays WHERE crop_id = ?1 AND state = 'harvested' AND harvested_on = ?2 LIMIT 1",
            params![crop_id, today],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    let covers: Vec<CoverageRef> = if harvested_today {
        conn.query_row(
            "SELECT covers FROM harvest_coverage
             WHERE crop_id = ?1 AND harvested_on = ?2
             ORDER BY created_at DESC, coverage_id DESC LIMIT 1",
            params![crop_id, today],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    for line in &mut lines {
        line.covered = covers
            .iter()
            .any(|c| c.kind == line.kind && c.id == line.id);
    }
    Ok(HarvestCommitmentsView {
        crop_id: crop_id.to_string(),
        header: format!("Committed for {crop_name}:"),
        empty_line: format!("No open commitments for {crop_name}."),
        crop_name,
        harvested_on: today.to_string(),
        lines,
    })
}
/// R3 handler (signed items 3, 6). Writes harvest.covered for (crop, today)
/// with the full current list; "Undo mark" is the same call with the line
/// removed. Handler-only checks: crop, venue and order must exist. Nothing
/// here touches trays, capacity, standing or money.
pub fn record_harvest_coverage(
    conn: &mut Connection,
    crop_id: &str,
    covers: Vec<CoverageRef>,
) -> Result<HarvestCommitmentsView, String> {
    let _: String = conn
        .query_row("SELECT name FROM crops WHERE id = ?1", [crop_id], |r| {
            r.get(0)
        })
        .map_err(|_| format!("unknown crop: {crop_id}"))?;
    let mut clean: Vec<CoverageRef> = Vec::new();
    for c in covers {
        if !COVERAGE_KINDS.contains(&c.kind.as_str()) {
            return Err(format!(
                "coverage kind must be standing or wholesale, got {}",
                c.kind
            ));
        }
        if c.id.trim().is_empty() {
            return Err("coverage id must be non-empty".into());
        }
        let exists: bool = if c.kind == "standing" {
            conn.query_row(
                "SELECT 1 FROM mkt_venues WHERE venue_id = ?1",
                [&c.id],
                |_| Ok(true),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or(false)
        } else {
            conn.query_row(
                "SELECT 1 FROM wholesale_orders WHERE id = ?1",
                [&c.id],
                |_| Ok(true),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or(false)
        };
        if !exists {
            let what = if c.kind == "standing" {
                "venue"
            } else {
                "order"
            };
            return Err(format!("{what} not found: {}", c.id));
        }
        if !clean.iter().any(|x| x.kind == c.kind && x.id == c.id) {
            clean.push(c);
        }
    }
    let harvested_on = db::local_date_today();
    let coverage_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let event = EventRecord::originated(
        Kind::HarvestCovered,
        "harvest_coverage",
        coverage_id.clone(),
        json!({
            "coverageId": coverage_id,
            "cropId": crop_id,
            "harvestedOn": harvested_on,
            "covers": clean,
        }),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    harvest_commitments_on(conn, crop_id, &harvested_on)
}

fn get_standing_request(
    conn: &Connection,
    request_id: &str,
) -> Result<StandingRequestView, String> {
    conn.query_row(
        "SELECT r.request_id, r.token, r.venue_id, v.name, r.varieties, r.bags_per_cycle,
                r.requested_at, r.contact, r.created_at, r.decided_at, r.outcome
         FROM mkt_standing_requests r
         JOIN mkt_venues v ON v.venue_id = r.venue_id
         WHERE r.request_id = ?1",
        [request_id],
        |r| {
            let varieties_json: String = r.get(4)?;
            let varieties: Vec<String> = serde_json::from_str(&varieties_json).unwrap_or_default();
            Ok(StandingRequestView {
                request_id: r.get(0)?,
                token: r.get(1)?,
                venue_id: r.get(2)?,
                venue_name: r.get(3)?,
                varieties,
                bags_per_cycle: r.get(5)?,
                requested_at: r.get(6)?,
                contact: r.get(7)?,
                created_at: r.get(8)?,
                decided_at: r.get(9)?,
                outcome: r.get(10)?,
            })
        },
    )
    .map_err(|_| format!("standing request not found: {request_id}"))
}

/// GT-D17 / fence 3: the ONE decide door. Accept = standing.request_decided
/// (accepted) + stage.changed through the same builder change_stage uses;
/// dismiss = standing.request_decided (dismissed) only. The open attention row,
/// the decision and (on accept) the standing write land in one transaction.
/// Handler-only gates: already decided; accept on an incomplete venue or a
/// quantity below 1 (rulings 8/9). Signed item 6: one variety maps to a target,
/// several land as trays_week only — the existing amber "Re-state by variety"
/// path. Ruling 7: 1 bag = 1 tray. Ruling 8: an already-standing venue is
/// replaced by the UPSERT. Signed item 5: any stage may accept, passed included.
pub fn decide_standing_request(
    conn: &mut Connection,
    request_id: &str,
    outcome: &str,
) -> Result<StandingRequestDecision, String> {
    if outcome != "accepted" && outcome != "dismissed" {
        return Err(format!(
            "outcome must be accepted or dismissed, got {outcome}"
        ));
    }
    let req = get_standing_request(conn, request_id)?;
    if req.decided_at.is_some() {
        return Err(STANDING_REQUEST_ALREADY_DECIDED.to_string());
    }
    let created_at = projection::handler_now();
    let mut stage_event: Option<EventRecord> = None;
    if outcome == "accepted" {
        let venue = get_venue(conn, &req.venue_id)?;
        if !venue_ready_for_qr(&venue) || req.bags_per_cycle < 1 {
            return Err(STANDING_ACCEPT_REFUSAL.to_string());
        }
        let targets = if req.varieties.len() == 1 {
            let mut m = std::collections::BTreeMap::new();
            m.insert(req.varieties[0].clone(), req.bags_per_cycle);
            Some(m)
        } else {
            None
        };
        stage_event = Some(stage_changed_event(
            StagePayload {
                venue_id: req.venue_id.clone(),
                stage: "standing".to_string(),
                changed_on: db::local_date_today(),
                trays_week: Some(req.bags_per_cycle),
                varieties: Some(req.varieties.clone()),
                variety_targets: targets,
                note: None,
            },
            &created_at,
        )?);
    }
    let decided = EventRecord::originated(
        Kind::StandingRequestDecided,
        "standing_request",
        request_id.to_string(),
        json!({
            "requestId": request_id,
            "outcome": outcome,
            "decidedAt": created_at,
        }),
        json!({ "op": "none" }),
        created_at.clone(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let action = if outcome == "accepted" {
        "accept"
    } else {
        "dismiss"
    };
    let open_attention: Option<String> = conn
        .query_row(
            "SELECT id FROM attention
             WHERE kind = ?1 AND entity_id = ?2 AND resolved_at IS NULL LIMIT 1",
            params![STANDING_REQUEST_ATTENTION_KIND, request_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    if let Some(id) = open_attention {
        attention::resolve_open_in_tx(&tx, &id, action, &created_at)?;
    }
    write_pair(&tx, &decided)?;
    if let Some(ev) = &stage_event {
        write_pair(&tx, ev)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    let stage = if outcome == "accepted" {
        Some(get_stage(conn, &req.venue_id)?)
    } else {
        None
    };
    Ok(StandingRequestDecision {
        request_id: request_id.to_string(),
        outcome: outcome.to_string(),
        decided_at: created_at,
        stage,
    })
}

/// Signed 2026-08-17 (fence 3, item 3a). Debug builds only: writes a real
/// standing.requested through the normal choke point so the card can be
/// keyboard-proven before the Fence 5 pull exists. The ledger states its origin
/// (request_id "dev-…", contact "dev seed"). Payload frozen from the token's
/// sample — the exact shape the pull will use. dev.backdated is the precedent.
#[cfg(debug_assertions)]
pub fn dev_seed_standing_request(
    conn: &mut Connection,
    token: &str,
    bags_per_cycle: i64,
) -> Result<StandingRequestView, String> {
    let (venue_id, varieties_json): (String, String) = conn
        .query_row(
            "SELECT venue_id, varieties FROM mkt_samples WHERE token = ?1",
            [token],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "no sample carries that token".to_string())?;
    let varieties: Vec<String> = serde_json::from_str(&varieties_json).unwrap_or_default();
    let request_id = format!("dev-{}", projection::handler_new_id());
    let created_at = projection::handler_now();
    let event = EventRecord::originated(
        Kind::StandingRequested,
        "standing_request",
        request_id.clone(),
        json!({
            "requestId": request_id,
            "token": token,
            "venueId": venue_id,
            "varieties": varieties,
            "bagsPerCycle": bags_per_cycle,
            "requestedAt": created_at,
            "contact": "dev seed",
        }),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_standing_requests(conn)?;
    get_standing_request(conn, &request_id)
}

pub fn marketing_summary(conn: &Connection) -> Result<MarketingSummary, String> {
    let standing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(trays_week), 0)
             FROM mkt_stages WHERE stage = 'standing'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some((n, trays)) = standing {
        if n > 0 {
            return Ok(MarketingSummary {
                time_to_first_standing_order: format!(
                    "Standing order: {trays} trays/week across {n} venues."
                ),
            });
        }
    }
    let earliest: Option<String> = conn
        .query_row("SELECT MIN(dropped_on) FROM mkt_samples", [], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    let line = match earliest {
        None => "No sample dropped yet.".to_string(),
        Some(first) => {
            let today = db::local_date_today();
            let start = NaiveDate::parse_from_str(&first, "%Y-%m-%d").map_err(|e| e.to_string())?;
            let end = NaiveDate::parse_from_str(&today, "%Y-%m-%d").map_err(|e| e.to_string())?;
            let n = (end - start).num_days().max(0);
            format!("No standing order yet. {n} days since first sample drop.")
        }
    };
    Ok(MarketingSummary {
        time_to_first_standing_order: line,
    })
}

/// B6 — crop identity survives a rename. Standing-target keys are name
/// strings frozen inside event payloads; they are never rewritten. Resolve
/// each key to a crop id through the live crops table first, then through
/// crop_aliases, and report the crop's CURRENT name. A key that names no
/// crop stays exactly as the operator typed it and still resolves to zero
/// sown — an unmatched target is visible, not invented and not hidden.
/// H-7 move 3 - these two maps used to ride out as a bare tuple, with their
/// meanings recorded only in trailing comments on the locals. Naming them
/// retires the type_complexity allow and makes each call site say which map
/// it took.
struct CropKeyIndex {
    /// Crop key - a live crop name, or a former name from crop_aliases -> crop_id.
    by_key: BTreeMap<String, String>,
    /// crop_id -> the crop's CURRENT name. Live crops only.
    live: BTreeMap<String, String>,
}

fn crop_key_index(conn: &Connection) -> Result<CropKeyIndex, String> {
    let mut by_key: BTreeMap<String, String> = BTreeMap::new();
    let mut live: BTreeMap<String, String> = BTreeMap::new();
    let mut stmt = conn
        .prepare("SELECT id, name FROM crops")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for r in rows {
        let (id, name) = r.map_err(|e| e.to_string())?;
        live.insert(id.clone(), name.clone());
        by_key.insert(name, id);
    }
    let mut astmt = conn
        .prepare("SELECT former_name, crop_id FROM crop_aliases")
        .map_err(|e| e.to_string())?;
    let arows = astmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for r in arows {
        let (former, id) = r.map_err(|e| e.to_string())?;
        // or_insert: a live crop name is never shadowed by a former one.
        by_key.entry(former).or_insert(id);
    }
    Ok(CropKeyIndex { by_key, live })
}
fn canonical_variety(
    key: &str,
    by_key: &BTreeMap<String, String>,
    live: &BTreeMap<String, String>,
) -> String {
    by_key
        .get(key)
        .and_then(|id| live.get(id))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// GT-D18 (signed 2026-08-17, fence 4 item 4). Composes the link a QR encodes:
/// `{endpoint}/s/{token}?v={variety}&v=…&g={growth_days}&g=…`. Read-only.
/// The token is the only identifier that leaves the farm; the raw sample id
/// never does. `g` is crops.growth_days per resolvable variety (blackout is
/// inside it); the page renders the longest. Nothing is pushed anywhere.
pub fn qr_link_for_token(conn: &Connection, token: &str) -> Result<String, String> {
    let endpoint = crate::scans::config(conn)?
        .endpoint_url
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| QR_LINK_NEEDS_ENDPOINT.to_string())?;
    let varieties_json: String = conn
        .query_row(
            "SELECT varieties FROM mkt_samples WHERE token = ?1",
            [token],
            |r| r.get(0),
        )
        .map_err(|_| format!("no sample carries token {token}"))?;
    let varieties: Vec<String> = serde_json::from_str(&varieties_json).unwrap_or_default();
    if varieties.is_empty() {
        return Err("this sample lists no varieties".into());
    }
    let by_key = crop_key_index(conn)?.by_key;
    let mut growth: Vec<i64> = Vec::new();
    for name in &varieties {
        if let Some(id) = by_key.get(name) {
            let g: i64 = conn
                .query_row("SELECT growth_days FROM crops WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .map_err(|e| e.to_string())?;
            growth.push(g);
        }
    }
    if growth.is_empty() {
        return Err("no growth days are known for this sample's varieties".into());
    }
    let mut url = format!("{endpoint}/s/{token}");
    let mut sep = '?';
    for v in &varieties {
        url.push(sep);
        url.push_str("v=");
        url.push_str(&percent_encode(v));
        sep = '&';
    }
    for g in &growth {
        url.push_str(&format!("&g={g}"));
    }
    Ok(url)
}

/// Phase 3: read-only standing-demand view. Per-variety targets are primary;
/// legacy NULL-target rows contribute to display totals only, never shortfall.
pub fn standing_demand(conn: &Connection) -> Result<StandingDemandView, String> {
    let today = db::local_date_today();
    let window_start = add_days(&today, -6)?;
    let CropKeyIndex { by_key, live } = crop_key_index(conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT v.name, s.trays_week, s.variety_targets, s.varieties
             FROM mkt_stages s
             JOIN mkt_venues v ON v.venue_id = s.venue_id
             WHERE s.stage = 'standing' AND v.archived_at IS NULL
             ORDER BY v.name, s.venue_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut demand_by_variety: BTreeMap<String, i64> = BTreeMap::new();
    let mut unallocated_venues: Vec<String> = Vec::new();
    let mut unallocated_trays_week: i64 = 0;
    let mut variety_set = BTreeSet::new();
    let mut standing_venues: i64 = 0;
    let mut named_sum: i64 = 0;
    for row in rows {
        let (name, trays_week, targets_json, varieties_json) = row.map_err(|e| e.to_string())?;
        standing_venues += 1;
        let parsed_targets = targets_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<BTreeMap<String, i64>>(j).ok());
        if let Some(targets) = parsed_targets {
            for (k, n) in targets {
                let k = canonical_variety(&k, &by_key, &live);
                *demand_by_variety.entry(k.clone()).or_insert(0) += n;
                named_sum += n;
                variety_set.insert(k);
            }
        } else {
            unallocated_venues.push(name);
            unallocated_trays_week += trays_week.unwrap_or(0);
            if let Some(json) = varieties_json.as_deref() {
                if let Ok(names) = serde_json::from_str::<Vec<String>>(json) {
                    for n in names {
                        variety_set.insert(canonical_variety(&n, &by_key, &live));
                    }
                }
            }
        }
    }
    let sown_last_7_days: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(quantity), 0)
             FROM trays
             WHERE sown_on IS NOT NULL
               AND sown_on >= ?1 AND sown_on <= ?2
               AND state != 'discarded'",
            rusqlite::params![window_start, today],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut sown_stmt = conn
        .prepare(
            "SELECT c.name, COALESCE(SUM(t.quantity), 0)
             FROM trays t
             JOIN crops c ON c.id = t.crop_id
             WHERE t.sown_on IS NOT NULL
               AND t.sown_on >= ?1 AND t.sown_on <= ?2
               AND t.state != 'discarded'
             GROUP BY c.name",
        )
        .map_err(|e| e.to_string())?;
    let sown_rows = sown_stmt
        .query_map(rusqlite::params![window_start, today], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut sown_by_variety: BTreeMap<String, i64> = BTreeMap::new();
    for row in sown_rows {
        let (name, qty) = row.map_err(|e| e.to_string())?;
        sown_by_variety.insert(name, qty);
    }
    let varieties: Vec<VarietyDemand> = demand_by_variety
        .into_iter()
        .map(|(name, trays_week)| {
            let sown = sown_by_variety.get(&name).copied().unwrap_or(0);
            VarietyDemand {
                name,
                trays_week,
                sown_last_7_days: sown,
                shortfall: (trays_week - sown).max(0),
            }
        })
        .collect();
    let shortfall: i64 = varieties.iter().map(|v| v.shortfall).sum();
    Ok(StandingDemandView {
        standing_venues,
        trays_week: named_sum + unallocated_trays_week,
        sown_last_7_days,
        shortfall,
        standing_varieties: variety_set.into_iter().collect(),
        varieties,
        unallocated_venues,
        unallocated_trays_week,
    })
}

pub fn list_venues(conn: &Connection, include_archived: bool) -> Result<Vec<VenueView>, String> {
    let sql = if include_archived {
        "SELECT venue_id, name, venue_type, contact, phone, address, note,
                archived_at, created_at, updated_at
         FROM mkt_venues ORDER BY name, venue_id"
    } else {
        "SELECT venue_id, name, venue_type, contact, phone, address, note,
                archived_at, created_at, updated_at
         FROM mkt_venues WHERE archived_at IS NULL ORDER BY name, venue_id"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            let mut v = VenueView {
                venue_id: r.get(0)?,
                name: r.get(1)?,
                venue_type: r.get(2)?,
                contact: r.get(3)?,
                phone: r.get(4)?,
                address: r.get(5)?,
                note: r.get(6)?,
                archived_at: r.get(7)?,
                created_at: r.get(8)?,
                updated_at: r.get(9)?,
                qr_ready: false,
            };
            v.qr_ready = venue_ready_for_qr(&v);
            Ok(v)
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn list_open_followups(conn: &Connection) -> Result<Vec<FollowupView>, String> {
    let today = db::local_date_today();
    let mut stmt = conn
        .prepare(
            "SELECT f.followup_id, f.venue_id, v.name, f.due_on, f.what,
                    f.cleared_at, f.created_at, f.updated_at,
                    (SELECT a.id FROM attention a
                     WHERE a.kind = 'marketing.followup_due'
                       AND a.entity_id = f.followup_id
                       AND a.resolved_at IS NULL
                     LIMIT 1)
             FROM mkt_followups f
             JOIN mkt_venues v ON v.venue_id = f.venue_id
             WHERE f.cleared_at IS NULL
             ORDER BY
               CASE WHEN f.due_on <= ?1 THEN 0 ELSE 1 END,
               f.due_on,
               f.followup_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&today], |r| {
            Ok(FollowupView {
                followup_id: r.get(0)?,
                venue_id: r.get(1)?,
                venue_name: r.get(2)?,
                due_on: r.get(3)?,
                what: r.get(4)?,
                cleared_at: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
                attention_id: r.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn list_samples(conn: &Connection) -> Result<Vec<SampleView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.sample_id, s.venue_id, v.name, s.dropped_on, s.varieties,
                    s.pack_count, s.note, s.created_at, s.token
             FROM mkt_samples s
             JOIN mkt_venues v ON v.venue_id = s.venue_id
             ORDER BY s.dropped_on DESC, s.sample_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            let varieties_json: String = r.get(4)?;
            let varieties: Vec<String> = serde_json::from_str(&varieties_json).unwrap_or_default();
            Ok(SampleView {
                sample_id: r.get(0)?,
                venue_id: r.get(1)?,
                venue_name: r.get(2)?,
                dropped_on: r.get(3)?,
                varieties,
                pack_count: r.get(5)?,
                note: r.get(6)?,
                created_at: r.get(7)?,
                token: r.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn get_venue(conn: &Connection, venue_id: &str) -> Result<VenueView, String> {
    conn.query_row(
        "SELECT venue_id, name, venue_type, contact, phone, address, note,
                archived_at, created_at, updated_at
         FROM mkt_venues WHERE venue_id = ?1",
        [venue_id],
        |r| {
            let mut v = VenueView {
                venue_id: r.get(0)?,
                name: r.get(1)?,
                venue_type: r.get(2)?,
                contact: r.get(3)?,
                phone: r.get(4)?,
                address: r.get(5)?,
                note: r.get(6)?,
                archived_at: r.get(7)?,
                created_at: r.get(8)?,
                updated_at: r.get(9)?,
                qr_ready: false,
            };
            v.qr_ready = venue_ready_for_qr(&v);
            Ok(v)
        },
    )
    .map_err(|_| format!("venue not found: {venue_id}"))
}

fn get_sample(conn: &Connection, sample_id: &str) -> Result<SampleView, String> {
    conn.query_row(
        "SELECT s.sample_id, s.venue_id, v.name, s.dropped_on, s.varieties,
                s.pack_count, s.note, s.created_at, s.token
         FROM mkt_samples s
         JOIN mkt_venues v ON v.venue_id = s.venue_id
         WHERE s.sample_id = ?1",
        [sample_id],
        |r| {
            let varieties_json: String = r.get(4)?;
            let varieties: Vec<String> = serde_json::from_str(&varieties_json).unwrap_or_default();
            Ok(SampleView {
                sample_id: r.get(0)?,
                venue_id: r.get(1)?,
                venue_name: r.get(2)?,
                dropped_on: r.get(3)?,
                varieties,
                pack_count: r.get(5)?,
                note: r.get(6)?,
                created_at: r.get(7)?,
                token: r.get(8)?,
            })
        },
    )
    .map_err(|_| format!("sample not found: {sample_id}"))
}

fn get_touch(conn: &Connection, touch_id: &str) -> Result<TouchView, String> {
    conn.query_row(
        "SELECT t.touch_id, t.venue_id, v.name, t.touched_on, t.channel,
                t.outcome, t.note, t.created_at
         FROM mkt_touches t
         JOIN mkt_venues v ON v.venue_id = t.venue_id
         WHERE t.touch_id = ?1",
        [touch_id],
        |r| {
            Ok(TouchView {
                touch_id: r.get(0)?,
                venue_id: r.get(1)?,
                venue_name: r.get(2)?,
                touched_on: r.get(3)?,
                channel: r.get(4)?,
                outcome: r.get(5)?,
                note: r.get(6)?,
                created_at: r.get(7)?,
            })
        },
    )
    .map_err(|_| format!("touch not found: {touch_id}"))
}

fn get_followup(conn: &Connection, followup_id: &str) -> Result<FollowupView, String> {
    conn.query_row(
        "SELECT f.followup_id, f.venue_id, v.name, f.due_on, f.what,
                f.cleared_at, f.created_at, f.updated_at, NULL
         FROM mkt_followups f
         JOIN mkt_venues v ON v.venue_id = f.venue_id
         WHERE f.followup_id = ?1",
        [followup_id],
        |r| {
            Ok(FollowupView {
                followup_id: r.get(0)?,
                venue_id: r.get(1)?,
                venue_name: r.get(2)?,
                due_on: r.get(3)?,
                what: r.get(4)?,
                cleared_at: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
                attention_id: r.get(8)?,
            })
        },
    )
    .map_err(|_| format!("followup not found: {followup_id}"))
}

// --- Projection apply_* (deterministic; ids/timestamps from payload) ---

fn req_str(p: &Value, key: &str) -> Result<String, String> {
    p.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("payload missing {key}"))
}

fn opt_str(p: &Value, key: &str) -> Option<String> {
    p.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

pub fn apply_venue_recorded(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p = &event.payload;
    let venue_id = req_str(p, "venueId")?;
    let name = req_str(p, "name")?;
    let venue_type = req_str(p, "venueType")?;
    tx.execute(
        "INSERT INTO mkt_venues
         (venue_id, name, venue_type, contact, phone, address, note,
          archived_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?8)",
        params![
            venue_id,
            name,
            venue_type,
            opt_str(p, "contact"),
            opt_str(p, "phone"),
            opt_str(p, "address"),
            opt_str(p, "note"),
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn apply_venue_corrected(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p = &event.payload;
    let venue_id = req_str(p, "venueId")?;
    let n = tx
        .execute(
            "UPDATE mkt_venues SET name = ?1, venue_type = ?2, contact = ?3,
             phone = ?4, address = ?5, note = ?6, updated_at = ?7
             WHERE venue_id = ?8",
            params![
                req_str(p, "name")?,
                req_str(p, "venueType")?,
                opt_str(p, "contact"),
                opt_str(p, "phone"),
                opt_str(p, "address"),
                opt_str(p, "note"),
                event.created_at,
                venue_id,
            ],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!("venue.corrected: venue not found: {venue_id}"));
    }
    Ok(())
}

pub fn apply_venue_archived(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p = &event.payload;
    let venue_id = req_str(p, "venueId")?;
    let archived_at = req_str(p, "archivedAt")?;
    let n = tx
        .execute(
            "UPDATE mkt_venues SET archived_at = ?1, updated_at = ?2 WHERE venue_id = ?3",
            params![archived_at, event.created_at, venue_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!("venue.archived: venue not found: {venue_id}"));
    }
    Ok(())
}

pub fn apply_sample_dropped(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p = &event.payload;
    let varieties = p
        .get("varieties")
        .ok_or_else(|| "sample.dropped missing varieties".to_string())?;
    let varieties_json = serde_json::to_string(varieties).map_err(|e| e.to_string())?;
    let pack_count = p
        .get("packCount")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "sample.dropped packCount must be an integer".to_string())?;
    tx.execute(
        "INSERT INTO mkt_samples
         (sample_id, venue_id, dropped_on, varieties, pack_count, note, token, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            req_str(p, "sampleId")?,
            req_str(p, "venueId")?,
            req_str(p, "droppedOn")?,
            varieties_json,
            pack_count,
            opt_str(p, "note"),
            opt_str(p, "token"),
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn apply_touch_logged(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p = &event.payload;
    tx.execute(
        "INSERT INTO mkt_touches
         (touch_id, venue_id, touched_on, channel, outcome, note, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            req_str(p, "touchId")?,
            req_str(p, "venueId")?,
            req_str(p, "touchedOn")?,
            req_str(p, "channel")?,
            opt_str(p, "outcome"),
            opt_str(p, "note"),
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn apply_followup_set(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p = &event.payload;
    tx.execute(
        "INSERT INTO mkt_followups
         (followup_id, venue_id, due_on, what, cleared_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5)",
        params![
            req_str(p, "followupId")?,
            req_str(p, "venueId")?,
            req_str(p, "dueOn")?,
            req_str(p, "what")?,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn apply_followup_cleared(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p = &event.payload;
    let followup_id = req_str(p, "followupId")?;
    let cleared_at = req_str(p, "clearedAt")?;
    let n = tx
        .execute(
            "UPDATE mkt_followups SET cleared_at = ?1, updated_at = ?2
             WHERE followup_id = ?3",
            params![cleared_at, event.created_at, followup_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!(
            "followup.cleared: followup not found: {followup_id}"
        ));
    }
    Ok(())
}

/// The ONE builder for stage.changed. change_stage and decide_standing_request
/// both write the event this returns — there is no second standing writer.
/// Validation, body bytes and entity are exactly what change_stage wrote before.
///
/// H-7b Fence 3b - the payload is built by the two callers now and handed in.
/// StagePayload was always this function's first act, so taking it directly
/// adds no type and moves no bytes. Validation still happens here, which is
/// why neither caller can skip it.
fn stage_changed_event(payload: StagePayload, created_at: &str) -> Result<EventRecord, String> {
    validate_stage_fields(&payload)?;
    let mut body = json!({
        "venueId": payload.venue_id,
        "stage": payload.stage,
        "changedOn": payload.changed_on,
    });
    let obj = body.as_object_mut().unwrap();
    if let Some(t) = payload.trays_week {
        obj.insert("traysWeek".into(), json!(t));
    }
    if let Some(v) = &payload.varieties {
        obj.insert("varieties".into(), json!(v));
    }
    if let Some(vt) = &payload.variety_targets {
        obj.insert("varietyTargets".into(), json!(vt));
    }
    if let Some(n) = payload.note {
        obj.insert("note".into(), json!(n));
    }
    Ok(EventRecord::originated(
        Kind::StageChanged,
        "venue",
        payload.venue_id.clone(),
        body,
        json!({ "op": "none" }),
        created_at.to_string(),
        None,
        None,
        Some(projection::handler_new_id()),
    ))
}

#[allow(clippy::too_many_arguments)] // H-7 Class D: mirrors its Tauri command's arg list; both narrow together under H-7b.
pub fn change_stage(
    conn: &mut Connection,
    venue_id: &str,
    stage: &str,
    changed_on: &str,
    trays_week: Option<i64>,
    varieties: Option<Vec<String>>,
    variety_targets: Option<std::collections::BTreeMap<String, i64>>,
    note: Option<String>,
) -> Result<StageView, String> {
    let _ = get_venue(conn, venue_id)?;
    let created_at = projection::handler_now();
    let event = stage_changed_event(
        StagePayload {
            venue_id: venue_id.to_string(),
            stage: stage.to_string(),
            changed_on: changed_on.to_string(),
            trays_week,
            varieties,
            variety_targets,
            note,
        },
        &created_at,
    )?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_stage(conn, venue_id)
}

pub fn observe_reviews(
    conn: &mut Connection,
    observed_on: &str,
    count: i64,
    source: &str,
) -> Result<ReviewObservationView, String> {
    let observation_id = projection::handler_new_id();
    let created_at = projection::handler_now();
    let event = EventRecord::originated(
        Kind::ReviewsObserved,
        "reviews",
        observation_id.clone(),
        json!({
            "observationId": observation_id,
            "observedOn": observed_on,
            "count": count,
            "source": source.trim(),
        }),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    raise_overdue_followups(conn)?;
    get_review(conn, &observation_id)
}

pub fn record_review_request(
    conn: &mut Connection,
    venue_id: &str,
    outcome: &str,
) -> Result<ReviewRequestView, String> {
    get_venue(conn, venue_id)?;
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mkt_review_requests WHERE venue_id = ?1",
            [venue_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists > 0 {
        return Err(format!("review already decided for venue {venue_id}"));
    }
    let decided_on = db::local_date_today();
    let created_at = projection::handler_now();
    let event = EventRecord::originated(
        Kind::ReviewRequested,
        "venue",
        venue_id.to_string(),
        json!({
            "venueId": venue_id,
            "decidedOn": decided_on,
            "outcome": outcome.trim(),
        }),
        json!({ "op": "none" }),
        created_at,
        None,
        None,
        Some(projection::handler_new_id()),
    );
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    write_pair(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_review_request(conn, venue_id)
}

pub fn list_review_requests(conn: &Connection) -> Result<Vec<ReviewRequestView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT r.venue_id, v.name, r.decided_on, r.outcome, r.created_at
             FROM mkt_review_requests r
             JOIN mkt_venues v ON v.venue_id = r.venue_id
             ORDER BY r.decided_on DESC, v.name, r.venue_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ReviewRequestView {
                venue_id: r.get(0)?,
                venue_name: r.get(1)?,
                decided_on: r.get(2)?,
                outcome: r.get(3)?,
                created_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn get_review_request(conn: &Connection, venue_id: &str) -> Result<ReviewRequestView, String> {
    list_review_requests(conn)?
        .into_iter()
        .find(|r| r.venue_id == venue_id)
        .ok_or_else(|| format!("review request not found: {venue_id}"))
}

pub fn gbp_verified_on(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT gbp_verified_on FROM business_profile WHERE id = 'default'",
        [],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
    .map(|row| row.flatten())
}

pub fn set_gbp_verified(conn: &Connection, on: Option<&str>) -> Result<Option<String>, String> {
    let date = match on {
        Some(d) if !d.trim().is_empty() => {
            validate_calendar_date(d.trim(), "gbp_verified_on")?;
            Some(d.trim().to_string())
        }
        _ => None,
    };
    let now = db::utc_now_rfc3339();
    conn.execute(
        "INSERT INTO business_profile (id, gbp_verified_on, updated_at)
         VALUES ('default', ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET
           gbp_verified_on = excluded.gbp_verified_on,
           updated_at = excluded.updated_at",
        params![date, now],
    )
    .map_err(|e| e.to_string())?;
    gbp_verified_on(conn)
}

pub fn list_stages(conn: &Connection) -> Result<Vec<StageView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.venue_id, v.name, s.stage, s.trays_week, s.varieties,
                    s.variety_targets, s.changed_on, s.note, s.updated_at
             FROM mkt_stages s
             JOIN mkt_venues v ON v.venue_id = s.venue_id
             WHERE v.archived_at IS NULL
             ORDER BY s.stage, v.name, s.venue_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            let varieties_json: Option<String> = r.get(4)?;
            let varieties = varieties_json
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok());
            let targets_json: Option<String> = r.get(5)?;
            let variety_targets = targets_json
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok());
            Ok(StageView {
                venue_id: r.get(0)?,
                venue_name: r.get(1)?,
                stage: r.get(2)?,
                trays_week: r.get(3)?,
                varieties,
                variety_targets,
                changed_on: r.get(6)?,
                note: r.get(7)?,
                updated_at: r.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn latest_review(conn: &Connection) -> Result<Option<ReviewObservationView>, String> {
    conn.query_row(
        "SELECT observation_id, observed_on, count, source, created_at
         FROM mkt_reviews
         ORDER BY observed_on DESC, created_at DESC
         LIMIT 1",
        [],
        |r| {
            Ok(ReviewObservationView {
                observation_id: r.get(0)?,
                observed_on: r.get(1)?,
                count: r.get(2)?,
                source: r.get(3)?,
                created_at: r.get(4)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Generated weekly actions with live capacity sight (GT-D12-R) and explicit drop count.
pub fn weekly_actions(conn: &Connection) -> Result<WeeklyActionsView, String> {
    weekly_actions_with_capacity(conn, &capacity_gate::capacity_sight_live(conn))
}

/// Same as `weekly_actions` but with an injected capacity state (tests / inversion).
pub fn weekly_actions_with_capacity(
    conn: &Connection,
    capacity: &CapacityState,
) -> Result<WeeklyActionsView, String> {
    let today = db::local_date_today();
    let quiet_before = add_days(&today, -14)?;
    let mut all: Vec<WeeklyAction> = Vec::new();

    // Overdue follow-ups first.
    let mut stmt = conn
        .prepare(
            "SELECT f.followup_id, f.venue_id, v.name, f.due_on, f.what,
                    (SELECT a.id FROM attention a
                     WHERE a.kind = 'marketing.followup_due'
                       AND a.entity_id = f.followup_id
                       AND a.resolved_at IS NULL
                     LIMIT 1)
             FROM mkt_followups f
             JOIN mkt_venues v ON v.venue_id = f.venue_id
             WHERE f.cleared_at IS NULL AND f.due_on <= ?1
             ORDER BY f.due_on, f.followup_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&today], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (_fid, venue_id, name, due_on, what, attention_id) = row.map_err(|e| e.to_string())?;
        all.push(WeeklyAction {
            kind: "overdue_followup".into(),
            title: format!("Overdue follow-up at {name} ({due_on}): {what}"),
            venue_id: Some(venue_id),
            attention_id,
        });
    }

    // Venues gone quiet: talking or trial, no touch in 14 days.
    let mut stmt = conn
        .prepare(
            "SELECT s.venue_id, v.name, s.stage
             FROM mkt_stages s
             JOIN mkt_venues v ON v.venue_id = s.venue_id
             WHERE v.archived_at IS NULL
               AND s.stage IN ('talking', 'trial', 'standing')
               AND NOT EXISTS (
                 SELECT 1 FROM mkt_touches t
                 WHERE t.venue_id = s.venue_id AND t.touched_on > ?1
               )
             ORDER BY v.name, s.venue_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&quiet_before], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (venue_id, name, stage) = row.map_err(|e| e.to_string())?;
        all.push(WeeklyAction {
            kind: "gone_quiet".into(),
            title: format!("{name} ({stage}) — no touch in 14 days"),
            venue_id: Some(venue_id),
            attention_id: None,
        });
    }

    // This week's planned drops: open follow-ups due this week that aren't overdue.
    let week_end = add_days(&today, 7)?;
    let mut stmt = conn
        .prepare(
            "SELECT f.venue_id, v.name, f.due_on, f.what
             FROM mkt_followups f
             JOIN mkt_venues v ON v.venue_id = f.venue_id
             WHERE f.cleared_at IS NULL
               AND f.due_on > ?1 AND f.due_on <= ?2
               AND f.what LIKE '%sample%'
             ORDER BY f.due_on, f.followup_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![today, week_end], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (venue_id, name, due_on, what) = row.map_err(|e| e.to_string())?;
        all.push(WeeklyAction {
            kind: "planned_drop".into(),
            title: format!("Planned drop at {name} ({due_on}): {what}"),
            venue_id: Some(venue_id),
            attention_id: None,
        });
    }

    // GT-D15 + ruling C: the ask is fired by a delivery. A venue at
    // standing or trial that has taken a delivery and has no decision
    // recorded raises exactly one ask. Stage alone raises nothing --
    // the app asks at the moment the relationship earned it.
    //
    // Current stage, not stage-at-delivery: reading the historical stage
    // would need event archaeology GT-D15 did not buy.
    //
    // No GBP precondition. GT-D15 makes GBP verification reference data,
    // never a gate on this query.
    let mut stmt = conn
        .prepare(
            "SELECT s.venue_id, v.name
             FROM mkt_stages s
             JOIN mkt_venues v ON v.venue_id = s.venue_id
             WHERE v.archived_at IS NULL
               AND s.stage IN ('standing', 'trial')
               AND EXISTS (
                 SELECT 1 FROM wholesale_orders o
                 WHERE o.venue_id = s.venue_id
                   AND o.delivered_on IS NOT NULL
               )
               AND NOT EXISTS (
                 SELECT 1 FROM mkt_review_requests r
                 WHERE r.venue_id = s.venue_id
               )
             ORDER BY v.name, s.venue_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (venue_id, name) = row.map_err(|e| e.to_string())?;
        all.push(WeeklyAction {
            kind: "review_ask".into(),
            title: format!("Ask {name} for a review"),
            venue_id: Some(venue_id),
            attention_id: None,
        });
    }
    // P7-CLOSE - review_stale. A review count that was true a month ago is a
    // number the operator can no longer stand behind. The date already rides
    // beside the count on the Marketing face; this is the line that makes it
    // cost something. It retires by itself the moment a fresher
    // observe_reviews lands - nothing to dismiss, nothing written.
    //
    // Venue set is ruling B: the same non-archived standing-or-trial set the
    // ask above draws from, without that query's delivered and not-yet-asked
    // clauses, which are about the ask and not about whether the farm has
    // anyone to be reviewed by. The 21-day warm filter in reputation_counts is
    // deliberately not used. Lexicographic compare on YYYY-MM-DD is the date
    // compare this module already uses.
    let stale_before = add_days(&today, -30)?;
    if let Some(review) = latest_review(conn)? {
        if review.observed_on <= stale_before {
            let active: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM mkt_stages s
                     JOIN mkt_venues v ON v.venue_id = s.venue_id
                     WHERE v.archived_at IS NULL
                       AND s.stage IN ('standing', 'trial')",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if active > 0 {
                all.push(WeeklyAction {
                    kind: "review_stale".into(),
                    title: REVIEW_STALE_TITLE.to_string(),
                    venue_id: None,
                    attention_id: None,
                });
            }
        }
    }
    // P7-CLOSE - gbp_verify. The one-time item. business_profile already holds
    // the date and p7t9 already freezes it as a date rather than a flag; this
    // is the line that stops it sitting empty forever. It is not a gate:
    // GT-D15 makes GBP reference data and p7t10 proves the ask above does not
    // wait on it.
    if gbp_verified_on(conn)?.is_none() {
        all.push(WeeklyAction {
            kind: "gbp_verify".into(),
            title: GBP_VERIFY_TITLE.to_string(),
            venue_id: None,
            attention_id: None,
        });
    }
    let pressure = crate::reachability::shelf_pressure_on(conn, &today)?;
    let pitching = capacity_gate::pitching_advice(capacity, &pressure).map(|s| s.to_string());

    let dropped = all.len().saturating_sub(WEEKLY_ACTIONS_CAP);
    all.truncate(WEEKLY_ACTIONS_CAP);
    Ok(WeeklyActionsView {
        actions: all,
        dropped,
        pitching_advice: pitching,
    })
}

pub fn reputation_counts(conn: &Connection) -> Result<ReputationCounts, String> {
    let today = db::local_date_today();
    let warm_since = add_days(&today, -21)?;
    let active_warm: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mkt_stages s
             JOIN mkt_venues v ON v.venue_id = s.venue_id
             WHERE v.archived_at IS NULL
               AND s.stage IN ('sampled', 'talking', 'trial')
               AND EXISTS (
                 SELECT 1 FROM mkt_touches t
                 WHERE t.venue_id = s.venue_id AND t.touched_on >= ?1
               )",
            [&warm_since],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let samples_outstanding: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mkt_samples s
             WHERE NOT EXISTS (
               SELECT 1 FROM mkt_touches t
               WHERE t.venue_id = s.venue_id AND t.touched_on >= s.dropped_on
             )",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let (standing_orders, standing_trays_week): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(trays_week), 0)
             FROM mkt_stages WHERE stage = 'standing'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let overdue_followups: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mkt_followups
             WHERE cleared_at IS NULL AND due_on <= ?1",
            [&today],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let review = latest_review(conn)?;
    Ok(ReputationCounts {
        active_warm_venues: active_warm,
        samples_outstanding,
        standing_orders,
        standing_trays_week,
        overdue_followups,
        google_review_count: review.as_ref().map(|r| r.count),
        google_review_observed_on: review.map(|r| r.observed_on),
    })
}

fn get_stage(conn: &Connection, venue_id: &str) -> Result<StageView, String> {
    conn.query_row(
        "SELECT s.venue_id, v.name, s.stage, s.trays_week, s.varieties,
                s.variety_targets, s.changed_on, s.note, s.updated_at
         FROM mkt_stages s
         JOIN mkt_venues v ON v.venue_id = s.venue_id
         WHERE s.venue_id = ?1",
        [venue_id],
        |r| {
            let varieties_json: Option<String> = r.get(4)?;
            let varieties = varieties_json
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok());
            let targets_json: Option<String> = r.get(5)?;
            let variety_targets = targets_json
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok());
            Ok(StageView {
                venue_id: r.get(0)?,
                venue_name: r.get(1)?,
                stage: r.get(2)?,
                trays_week: r.get(3)?,
                varieties,
                variety_targets,
                changed_on: r.get(6)?,
                note: r.get(7)?,
                updated_at: r.get(8)?,
            })
        },
    )
    .map_err(|_| format!("stage not found for venue: {venue_id}"))
}

fn get_review(conn: &Connection, observation_id: &str) -> Result<ReviewObservationView, String> {
    conn.query_row(
        "SELECT observation_id, observed_on, count, source, created_at
         FROM mkt_reviews WHERE observation_id = ?1",
        [observation_id],
        |r| {
            Ok(ReviewObservationView {
                observation_id: r.get(0)?,
                observed_on: r.get(1)?,
                count: r.get(2)?,
                source: r.get(3)?,
                created_at: r.get(4)?,
            })
        },
    )
    .map_err(|_| format!("review observation not found: {observation_id}"))
}

pub fn apply_stage_changed(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p = &event.payload;
    let venue_id = req_str(p, "venueId")?;
    let stage = req_str(p, "stage")?;
    let changed_on = req_str(p, "changedOn")?;
    let trays_week = p.get("traysWeek").and_then(|v| v.as_i64());
    let varieties = p.get("varieties").cloned();
    let varieties_json = match varieties {
        Some(v) => Some(serde_json::to_string(&v).map_err(|e| e.to_string())?),
        None => None,
    };
    let variety_targets = p.get("varietyTargets").cloned();
    let variety_targets_json = match variety_targets {
        Some(v) => Some(serde_json::to_string(&v).map_err(|e| e.to_string())?),
        None => None,
    };
    let note = opt_str(p, "note");
    tx.execute(
        "INSERT INTO mkt_stages
         (venue_id, stage, trays_week, varieties, variety_targets, changed_on, note, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(venue_id) DO UPDATE SET
           stage = excluded.stage,
           trays_week = excluded.trays_week,
           varieties = excluded.varieties,
           variety_targets = excluded.variety_targets,
           changed_on = excluded.changed_on,
           note = excluded.note,
           updated_at = excluded.updated_at",
        params![
            venue_id,
            stage,
            trays_week,
            varieties_json,
            variety_targets_json,
            changed_on,
            note,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn apply_reviews_observed(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p = &event.payload;
    tx.execute(
        "INSERT INTO mkt_reviews
         (observation_id, observed_on, count, source, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            req_str(p, "observationId")?,
            req_str(p, "observedOn")?,
            p.get("count")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "payload missing count".to_string())?,
            req_str(p, "source")?,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn apply_review_requested(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p: ReviewRequestPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("review.requested payload refused: {e}"))?;
    tx.execute(
        "INSERT OR IGNORE INTO mkt_review_requests
         (venue_id, decided_on, outcome, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![p.venue_id, p.decided_on, p.outcome, event.created_at],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// GT-D17. Insert-only projection of a candidate. Plain INSERT on purpose: a
/// second event with the same request_id is an ingest defect and fails replay
/// loudly instead of being hidden (the sample.dropped precedent, not the
/// review-request one). decided_at / outcome stay NULL here — Fence 3's
/// standing.request_decided owns them. No lookups: only what the payload froze.
pub fn apply_standing_requested(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p: StandingRequestPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("standing.requested payload refused: {e}"))?;
    let varieties_json = serde_json::to_string(&p.varieties).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO mkt_standing_requests
         (request_id, token, venue_id, varieties, bags_per_cycle, requested_at, contact,
          created_at, decided_at, outcome)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL)",
        params![
            p.request_id,
            p.token,
            p.venue_id,
            varieties_json,
            p.bags_per_cycle,
            p.requested_at,
            p.contact,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// GT-D17, second kind. Writes the decision onto the open candidate row only.
/// 0 rows means the row is missing or already decided — a handler defect that
/// must fail replay loudly (the followup.cleared precedent), never be hidden.
pub fn apply_standing_request_decided(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p: StandingRequestDecidedPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("standing.request_decided payload refused: {e}"))?;
    let n = tx
        .execute(
            "UPDATE mkt_standing_requests
             SET decided_at = ?1, outcome = ?2
             WHERE request_id = ?3 AND decided_at IS NULL",
            params![p.decided_at, p.outcome, p.request_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!(
            "standing.request_decided: request not open: {}",
            p.request_id
        ));
    }
    Ok(())
}

/// GT-D19. Insert-only, lookup-free; PK conflict fails loudly.
pub fn apply_harvest_covered(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    validate_marketing_event(event)?;
    let p: HarvestCoveredPayload = serde_json::from_value(event.payload.clone())
        .map_err(|e| format!("harvest.covered payload refused: {e}"))?;
    let covers_json = serde_json::to_string(&p.covers).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO harvest_coverage (coverage_id, crop_id, harvested_on, covers, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            p.coverage_id,
            p.crop_id,
            p.harvested_on,
            covers_json,
            event.created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
