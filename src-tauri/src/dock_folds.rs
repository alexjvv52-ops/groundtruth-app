//! The three folds — grouping, worst-of, and surface+rank.
//!
//! S2a signed 2026-08-21; the desk and the later dock port both READ these;
//! neither recomputes them.

use crate::attention;
use crate::health::{CheckStatus, Severity, REPORTED_CHECKS};
use crate::marketing;
use crate::models::AttentionItem;
use crate::phone;
use crate::phone_pull;
use crate::reachability::{self, CoverDate};
use crate::scans;
use crate::trays;
use crate::wholesale::{self, WholesaleOrderView};
use chrono::NaiveDate;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Farm-scope checks. Unknown ids land in System — the healthScopes.ts:13 rule,
/// unchanged.
pub const FARM_CHECKS: &[&str] = &["M1", "M2", "M3", "F1", "F2"];

pub const UNCLASSIFIED_TODAY_RANK: i64 = 7;
pub const STANDING_SHORTFALL_RANK: i64 = 3;
pub const MOVE_ROW_RANK: i64 = 8;
pub const HARVEST_ROW_RANK: i64 = 9;

const TODAY_RANK_PAIRS: &[(&str, i64)] = &[
    ("money.delivered_unpaid", 0),
    ("money.delivery_due", 5),
    ("money.capacity_short", 2),
    ("order.unrecorded", 1),
    ("order.refunded", 1),
    ("order.disputed", 1),
    ("order.oversold", 1),
    ("stripe.account_mismatch", 1),
    ("stripe.unrecognised_session", 1),
    ("wholesale.overcommitted", 1),
    ("tray.overdue_harvest", 4),
    ("tray.overdue_light", 6),
];

const MARKETING_KINDS: &[&str] = &[
    "marketing.followup_due",
    "marketing.standing_quiet",
    "marketing.standing_request",
];
const REALITY_KINDS: &[&str] = &[
    "recount.surplus",
    "recount.shortfall",
    "farm.restored",
    "phone.proposal",
];
const UPCOMING_KINDS: &[&str] = &["snapshot.failed", "poll.failed"];

// FI-3 - six nodes. Order follows the locked brainstorm's own list:
// Money, Capacity/Cover, Promise, Rack/Physical, Phone queue, Health.
// The sixth key stays `system`; its display title is FI-4 work.
const CARD_ORDER: &[&str] = &["money", "cover", "promise", "rack", "phone_queue", "system"];

/// FI-1 — the port document's shape version. Bumped only by a signed fence.
pub const PORT_DOCUMENT_VERSION: u32 = 9;

/// FI-9 - carried by every packet, wherever it came from. Recorded in
/// AI-READ-ONRAMP-BRAINSTORM.md; not invented here. The desk carries the same
/// line from healthScopes.ts, and f9a proves the two are identical.
pub const AUTHORITY_LINE: &str = "AUTHORITY: PC sole writer. Snapshot only. Do not invent numbers.";

/// The desk's evidence header, byte for byte. Composing on the PC means the em
/// dash is the real character - the ASCII shell had to spell it \u2014.
pub const DIAGNOSIS_HEAD: &str = "Groundtruth — health evidence";

/// The human half, first, per brainstorm 7.1.
pub const SUMMARY_HEAD: &str = "Groundtruth — field summary";

/// Signed heading for the pull-health section. Printed verbatim by dock_shell.rs
/// and by diagnosis_text. If this changes, f11a goes red until the shell matches.
pub const PULL_HEAD: &str = "Captures reaching the PC";

/// FI-1 — the one operator-facing datetime pattern the dock uses:
/// "Fri Aug 21, 3:04 pm". Deliberately the desk's own vocabulary
/// (monthDayLabel + formatClock, src/farm/dates.ts) so the phone does not open
/// a fourth date format. ASCII, no year - the desk carries none either, and the
/// defect this closes is a day-old snapshot reading like a six-minute-old one.
///
/// Composed HERE, on the PC, in the PC's zone. The phone substitutes the
/// finished string and owns no clock in this path.
pub const WHEN_FORMAT: &str = "%a %b %-d, %-I:%M %P";

/// Render a PC-owned RFC3339 instant as the signed `{when}` string.
pub fn compose_when(rfc3339: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format(WHEN_FORMAT)
                .to_string()
        })
}

/// The local calendar date of a PC-owned RFC3339 instant, so one wall-clock
/// read serves both `servedAt` and the day boundary instead of two reads that
/// can straddle midnight.
pub fn local_date_of(rfc3339: &str) -> Option<NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Local).date_naive())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DockFoldsView {
    pub overall: Option<Severity>,
    pub by_scope: ByScope,
    pub cards: Vec<DockCard>,
    pub checks: Vec<DockCheck>,
    pub surfaces: SurfacesTable,
    pub ranks: RanksTable,
    pub today_attention_order: Vec<String>,
    pub worst_clash: Option<WorstClash>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ByScope {
    pub farm: Option<Severity>,
    pub system: Option<Severity>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DockCard {
    pub card: String,
    pub severity: Option<Severity>,
    pub check_ids: Vec<String>,
    /// FI-4b (P-1) — the face sentence: the body half of the WORST check on
    /// this card, ties broken by `ids_for_card` order. Projected from a row
    /// already on the wire. Nothing is composed here and health.rs is not
    /// touched — the words are the ones health.rs already wrote.
    ///
    /// `None` when the card has no checks (phone_queue, S5) or when the worst
    /// row does not carry "{id} {title} — {body}" (B-1(b)). A headless clause
    /// or a bare check id on the face is the defect FI-4 closed.
    pub sentence: Option<String>,
    /// FI-4b (A-1) — the oldest real PC instant among this card's checks, and
    /// only when it is strictly older than the serve.
    ///
    /// M1-M4, F1 and F2 stamp the read instant by signed design
    /// (health.rs `money_ran_at`: "its timestamp is the moment you looked"), so
    /// those cards ship `None` here rather than restating the top line six
    /// times as if it were six independent freshness facts.
    pub oldest_ran_at: Option<String>,
    pub oldest_ran_at_display: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DockCheck {
    pub check_id: String,
    pub title: String,
    pub scope: String,
    pub card: String,
    /// 3-A: projection only. These three are what `health::compute_status`
    /// already produced and `checks_for` used to throw away. Nothing is
    /// evaluated here. `severity` is Option only because `checks_for` is total
    /// over a possibly-partial row set - the real path fills all nine.
    pub severity: Option<Severity>,
    pub sentence: String,
    pub ran_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SurfacesTable {
    pub map: BTreeMap<String, String>,
    pub default: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RanksTable {
    pub map: BTreeMap<String, i64>,
    pub unclassified: i64,
    pub standing_shortfall: i64,
    #[serde(rename = "move")]
    pub move_due: i64,
    pub harvest: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorstClash {
    pub source: String,
    pub kind: String,
    pub entity_id: Option<String>,
    pub rank: i64,
    /// FI-5 - every clash carries a PC sentence. Before FI-5 three of the four
    /// sources shipped None and the phone printed a bare key.
    pub sentence: Option<String>,
    /// FI-5 - the ordered edge, owner first. `None` means this fact stays
    /// inside one loop and draws no line.
    pub cards: Option<Vec<String>>,
}

pub fn scope_for_check(id: &str) -> &'static str {
    if FARM_CHECKS.contains(&id) {
        "farm"
    } else {
        "system"
    }
}

/// One card per check. No check maps to `phone_queue`.
///
/// FI-3 - M2 and M3 are separate loops, and they always were: M2 is the cover
/// plan (reachable, shelf room, sow-by), M3 is what is promised and not yet
/// delivered. Splitting them needed no new check and no new sentence - only
/// this table telling the truth about which loop each one already reports on.
///
/// This table and `ids_for_card` are hand-mirrored. Test f3b proves they agree;
/// edit them together or the proof fails.
pub fn card_for_check(id: &str) -> &'static str {
    match id {
        "M1" => "money",
        "M2" => "cover",
        "M3" => "promise",
        "F1" | "F2" => "rack",
        "H2" | "H3" | "H4" | "M4" => "system",
        _ => "system",
    }
}

pub fn ids_for_card(card: &str) -> Vec<String> {
    match card {
        "money" => vec!["M1".into()],
        "cover" => vec!["M2".into()],
        "promise" => vec!["M3".into()],
        "rack" => vec!["F1".into(), "F2".into()],
        // Still sourceless. S5 stands: no check maps here, so `cards_for`
        // reports no severity rather than inventing one.
        "phone_queue" => vec![],
        "system" => vec!["H2".into(), "H3".into(), "H4".into(), "M4".into()],
        _ => vec![],
    }
}

/// The title half of a check's sentence: derived, never reworded.
///
/// This began as a port of a desk-side helper in healthScopes.ts. That helper is
/// gone - the desk reads titles off this fold now ("Grouping, titles and
/// worst-of live on the PC", healthScopes.ts header) - so the old citation named
/// an address that no longer holds the logic. This function is the only owner.
pub fn check_title(check_id: &str, sentence: &str) -> String {
    let prefix = format!("{check_id} ");
    if let Some(rest) = sentence.strip_prefix(&prefix) {
        if let Some(dash) = rest.find(" — ") {
            if dash > 0 {
                return rest[..dash].to_string();
            }
        }
    }
    check_id.to_string()
}

/// FI-4b — the complement of `check_title`, on the same sentence grammar.
/// `check_title` keeps the half before the dash (the card already shows a
/// display title for that half); this keeps the half after it, which is the
/// only part the operator has not already read on the face.
///
/// Total, and deliberately strict: a row that does not carry
/// "{id} {title} — {body}" yields `None` (B-1(b)). Three such arms live in the
/// tree today, all system-scope, and one of them would otherwise print a raw
/// check id at the operator. Their sentence shapes are health.rs work and are
/// recorded as the FI-4d residual, not repaired here.
pub fn check_body(check_id: &str, sentence: &str) -> Option<String> {
    let prefix = format!("{check_id} ");
    let rest = sentence.strip_prefix(&prefix)?;
    let dash = rest.find(" — ")?;
    if dash == 0 {
        return None;
    }
    let body = rest[dash + " — ".len()..].trim();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

pub fn worst_of(rows: &[CheckStatus]) -> Option<Severity> {
    if rows.is_empty() {
        return None;
    }
    if rows.iter().any(|s| s.severity == Severity::Unhealthy) {
        return Some(Severity::Unhealthy);
    }
    if rows.iter().any(|s| s.severity == Severity::Degraded) {
        return Some(Severity::Degraded);
    }
    Some(Severity::Healthy)
}

pub fn severity_for_ids(rows: &[CheckStatus], ids: &[&str]) -> Option<Severity> {
    let filtered: Vec<CheckStatus> = rows
        .iter()
        .filter(|s| ids.iter().any(|id| s.check_id == *id))
        .cloned()
        .collect();
    worst_of(&filtered)
}

pub fn severity_for_scope(rows: &[CheckStatus], scope: &str) -> Option<Severity> {
    let filtered: Vec<CheckStatus> = rows
        .iter()
        .filter(|s| scope_for_check(&s.check_id) == scope)
        .cloned()
        .collect();
    worst_of(&filtered)
}

pub fn overall_severity(rows: &[CheckStatus]) -> Option<Severity> {
    worst_of(rows)
}

pub fn surface_for_kind(kind: &str) -> &'static str {
    if MARKETING_KINDS.contains(&kind) {
        "marketing"
    } else if REALITY_KINDS.contains(&kind) {
        "reality"
    } else if UPCOMING_KINDS.contains(&kind) {
        "upcoming"
    } else {
        "today"
    }
}

pub fn today_rank(kind: &str) -> i64 {
    TODAY_RANK_PAIRS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, r)| *r)
        .unwrap_or(UNCLASSIFIED_TODAY_RANK)
}

fn surfaces_table() -> SurfacesTable {
    let mut map = BTreeMap::new();
    for kind in MARKETING_KINDS {
        map.insert((*kind).to_string(), "marketing".into());
    }
    for kind in REALITY_KINDS {
        map.insert((*kind).to_string(), "reality".into());
    }
    for kind in UPCOMING_KINDS {
        map.insert((*kind).to_string(), "upcoming".into());
    }
    SurfacesTable {
        map,
        default: "today".into(),
    }
}

fn ranks_table() -> RanksTable {
    let mut map = BTreeMap::new();
    for (kind, rank) in TODAY_RANK_PAIRS {
        map.insert((*kind).to_string(), *rank);
    }
    RanksTable {
        map,
        unclassified: UNCLASSIFIED_TODAY_RANK,
        standing_shortfall: STANDING_SHORTFALL_RANK,
        move_due: MOVE_ROW_RANK,
        harvest: HARVEST_ROW_RANK,
    }
}

fn debt_age_key(item: &AttentionItem, orders: &[WholesaleOrderView]) -> Option<String> {
    let entity_id = item.entity_id.as_ref()?;
    let order = orders.iter().find(|o| o.id == *entity_id)?;
    match item.kind.as_str() {
        "money.delivered_unpaid" => order.delivered_on.clone(),
        "money.delivery_due" => Some(order.harvest_date.clone()),
        _ => None,
    }
}

fn cover_rank(item: &AttentionItem, cover: &[CoverDate]) -> usize {
    let id = item.entity_id.as_deref().unwrap_or("");
    if let Some((date, crop)) = id.split_once('|') {
        cover
            .iter()
            .position(|c| c.harvest_date == date && c.crop_id == crop)
            .unwrap_or(cover.len())
    } else {
        cover
            .iter()
            .position(|c| c.harvest_date == id)
            .unwrap_or(cover.len())
    }
}

fn order_today_items(
    conn: &Connection,
    items: Vec<AttentionItem>,
) -> Result<Vec<AttentionItem>, String> {
    let orders = wholesale::list_orders(conn)?;
    let cover = reachability::cover_plan(conn)?;
    let mut today: Vec<AttentionItem> = items
        .into_iter()
        .filter(|a| surface_for_kind(&a.kind) == "today")
        .collect();
    today.sort_by(|a, b| {
        today_rank(&a.kind)
            .cmp(&today_rank(&b.kind))
            .then_with(
                || match (debt_age_key(a, &orders), debt_age_key(b, &orders)) {
                    (Some(ka), Some(kb)) => ka.cmp(&kb),
                    _ => std::cmp::Ordering::Equal,
                },
            )
            .then_with(|| cover_rank(a, &cover).cmp(&cover_rank(b, &cover)))
    });
    Ok(today)
}

fn today_attention_items(conn: &Connection) -> Result<Vec<AttentionItem>, String> {
    order_today_items(conn, attention::check_attention(conn)?)
}

fn today_attention_items_read_only(conn: &Connection) -> Result<Vec<AttentionItem>, String> {
    order_today_items(conn, attention::open_items(conn)?)
}

/// Today-surface attention ids, in the three-key order Today.tsx:694-700 used:
/// today_rank → debt_age_key → cover_rank. An unreadable age compares equal.
pub fn today_attention_order(conn: &Connection) -> Result<Vec<String>, String> {
    Ok(today_attention_items(conn)?
        .into_iter()
        .map(|a| a.id)
        .collect())
}

/// FI-5 - the signed edge table, and nothing else.
///
/// An edge exists when the fact is OWNED by one loop and its CONSEQUENCE lands
/// in another. Source is the loop whose check owns the fact; target is the loop
/// that bears it. A fact whose consequence stays inside its own loop draws no
/// line - it is work, not an interaction.
///
/// Closed list. An unlisted kind returns None and draws no line: a guessed edge
/// would be a claim about the farm that no check ever made.
pub fn clash_cards(source: &str, kind: &str) -> Option<(&'static str, &'static str)> {
    match source {
        "attention" => match kind {
            // Owned by money, and the promise is already discharged. Cash only.
            "money.delivered_unpaid" => None,
            "money.delivery_due" => Some(("promise", "money")),
            "money.capacity_short" => Some(("cover", "promise")),
            "order.unrecorded" | "order.refunded" | "order.disputed" => Some(("money", "system")),
            "order.oversold" => Some(("promise", "cover")),
            "stripe.account_mismatch" | "stripe.unrecognised_session" => Some(("money", "system")),
            "wholesale.overcommitted" => Some(("promise", "cover")),
            "tray.overdue_harvest" => Some(("rack", "promise")),
            "tray.overdue_light" => Some(("rack", "cover")),
            _ => None,
        },
        "standing_shortfall" => Some(("cover", "promise")),
        // Work due on the rack, not an interaction between loops.
        "move_due" | "harvest_due" => None,
        _ => None,
    }
}

fn cards_vec(pair: Option<(&'static str, &'static str)>) -> Option<Vec<String>> {
    pair.map(|(a, b)| vec![a.to_string(), b.to_string()])
}

/// FI-5 / A'-1 - one builder, two projections. `worstClash` is the head of this
/// list, so the pulse line and the clash set can never disagree.
///
/// Every candidate carries a PC sentence and its edge from the signed table.
/// Nothing here composes operator text on the phone's behalf beyond these four
/// signed forms.
fn clash_candidates(
    conn: &Connection,
    items: Vec<AttentionItem>,
) -> Result<Vec<WorstClash>, String> {
    let mut candidates: Vec<WorstClash> = Vec::new();
    for item in items {
        let rank = today_rank(&item.kind);
        let cards = cards_vec(clash_cards("attention", &item.kind));
        candidates.push(WorstClash {
            source: "attention".into(),
            kind: item.kind,
            entity_id: item.entity_id,
            rank,
            sentence: Some(item.message),
            cards,
        });
    }
    // C1 (INT-001, D2). A failed standing-demand read is not "no shortfall".
    // It propagates exactly like `today_view` below: the document fails, and
    // the phone shows its stale banner instead of "Today's queue is clear."
    let demand = marketing::standing_demand(conn)?;
    if demand.shortfall > 0 {
        candidates.push(WorstClash {
            source: "standing_shortfall".into(),
            kind: String::new(),
            entity_id: None,
            rank: STANDING_SHORTFALL_RANK,
            sentence: Some(format!(
                "Standing orders are short {} this week.",
                reachability::tray_word(demand.shortfall)
            )),
            cards: cards_vec(clash_cards("standing_shortfall", "")),
        });
    }
    let view = trays::today_view(conn)?;
    if let Some(mtl) = &view.move_to_light {
        candidates.push(WorstClash {
            source: "move_due".into(),
            kind: String::new(),
            entity_id: None,
            rank: MOVE_ROW_RANK,
            sentence: Some(format!(
                "{} are due to move to light.",
                reachability::tray_word(mtl.tray_count)
            )),
            cards: cards_vec(clash_cards("move_due", "")),
        });
    }
    if let Some(hs) = &view.harvest_summary {
        let sentence = match &hs.single_crop_name {
            Some(name) => format!(
                "{} of {name} are due to harvest.",
                reachability::tray_word(hs.tray_count)
            ),
            None => format!(
                "{} across {} varieties are due to harvest.",
                reachability::tray_word(hs.tray_count),
                hs.variety_count
            ),
        };
        candidates.push(WorstClash {
            source: "harvest_due".into(),
            kind: String::new(),
            entity_id: None,
            rank: HARVEST_ROW_RANK,
            sentence: Some(sentence),
            cards: cards_vec(clash_cards("harvest_due", "")),
        });
    }
    // Stable sort: equal ranks keep insertion order, so the head is exactly the
    // element the old min_by_key picked. f5d proves head == worstClash.
    candidates.sort_by_key(|c| c.rank);
    Ok(candidates)
}

/// The top live row of Today's queue. Receipts are out of this pick.
pub fn worst_clash(conn: &Connection) -> Result<Option<WorstClash>, String> {
    Ok(clash_candidates(conn, today_attention_items(conn)?)?
        .into_iter()
        .next())
}

/// Every open clash, ranked. Read-only: `open_items`, never `check_attention`.
pub fn clashes_read_only(conn: &Connection) -> Result<Vec<WorstClash>, String> {
    clash_candidates(conn, today_attention_items_read_only(conn)?)
}

#[allow(dead_code)] // H-2: the port's read-only path, named in
                    // CLAUDE-ANNEX-DOCK-TREE-FACTS; exercised by dock_folds_tests f1/f2.
pub fn worst_clash_read_only(conn: &Connection) -> Result<Option<WorstClash>, String> {
    Ok(clashes_read_only(conn)?.into_iter().next())
}

/// FI-4b (P-1) — the row that deserves the face. Same precedence as `worst_of`,
/// so a card's sentence can never come from a row less severe than the card's
/// own severity. Ties keep `ids_for_card` order, which is the order `mine` was
/// built in.
fn worst_row<'a>(mine: &[&'a CheckStatus]) -> Option<&'a CheckStatus> {
    mine.iter()
        .find(|r| r.severity == Severity::Unhealthy)
        .or_else(|| mine.iter().find(|r| r.severity == Severity::Degraded))
        .or_else(|| mine.iter().find(|r| r.severity == Severity::Healthy))
        .copied()
}

/// FI-4b (A-1) — the oldest of this card's real PC instants, and only when it is
/// genuinely older than the serve.
///
/// Compared as instants, never as strings: two encodings of one moment must not
/// read as an age. An unparseable stamp is skipped rather than guessed.
fn oldest_ran_at_before_serve(mine: &[&CheckStatus], served_at: &str) -> Option<String> {
    let served = chrono::DateTime::parse_from_rfc3339(served_at).ok()?;
    let mut best: Option<(chrono::DateTime<chrono::FixedOffset>, String)> = None;
    for r in mine {
        let Some(at) = r.ran_at.as_deref() else {
            continue;
        };
        let Ok(t) = chrono::DateTime::parse_from_rfc3339(at) else {
            continue;
        };
        if t >= served {
            continue;
        }
        if best.as_ref().map(|(b, _)| t < *b).unwrap_or(true) {
            best = Some((t, at.to_string()));
        }
    }
    best.map(|(_, s)| s)
}

fn cards_for(rows: &[CheckStatus], served_at: &str) -> Vec<DockCard> {
    CARD_ORDER
        .iter()
        .map(|card| {
            let check_ids = ids_for_card(card);
            let severity = if *card == "phone_queue" {
                None
            } else {
                let id_refs: Vec<&str> = check_ids.iter().map(String::as_str).collect();
                severity_for_ids(rows, &id_refs)
            };
            // This card's own rows, in ids_for_card order. A card with no
            // checks (phone_queue) yields an empty set and therefore no
            // sentence and no age — Q-1b puts the queue's count line on that
            // face from `phoneQueue.sentence`, which is already on the wire.
            let mine: Vec<&CheckStatus> = check_ids
                .iter()
                .filter_map(|id| rows.iter().find(|s| s.check_id == *id))
                .collect();
            let sentence = worst_row(&mine).and_then(|r| check_body(&r.check_id, &r.sentence));
            let oldest_ran_at = oldest_ran_at_before_serve(&mine, served_at);
            let oldest_ran_at_display = oldest_ran_at.as_deref().and_then(compose_when);
            DockCard {
                card: (*card).to_string(),
                severity,
                check_ids,
                sentence,
                oldest_ran_at,
                oldest_ran_at_display,
            }
        })
        .collect()
}

fn checks_for(rows: &[CheckStatus]) -> Vec<DockCheck> {
    REPORTED_CHECKS
        .iter()
        .map(|id| {
            let row = rows.iter().find(|s| s.check_id == *id);
            let sentence = row.map(|s| s.sentence.as_str()).unwrap_or("");
            DockCheck {
                check_id: (*id).to_string(),
                title: check_title(id, sentence),
                scope: scope_for_check(id).to_string(),
                card: card_for_check(id).to_string(),
                severity: row.map(|s| s.severity),
                sentence: sentence.to_string(),
                ran_at: row.and_then(|s| s.ran_at.clone()),
            }
        })
        .collect()
}

/// FI-1 / A'-1 — the one pass. Both projections take their health rows, their
/// clock and their evaluation stamp from here, so the desk and the port can
/// never be built from two different instants.
///
/// Each projection keeps its OWN attention reader: the desk evaluates
/// (`worst_clash`), the port must not (`worst_clash_read_only`). That split is
/// signed and is not touched by FI-1.
pub struct InstrumentSnapshot {
    pub rows: Vec<CheckStatus>,
    pub served_at: String,
    pub served_at_display: String,
    pub attention_evaluated_at: Option<String>,
    pub attention_evaluated_at_display: Option<String>,
}

/// FI-7 - what the PC is holding from the phone, and nothing else.
///
/// `pending` is exactly `phone_proposals.decided_at IS NULL` - captures the PC
/// has PULLED and not yet decided. It is deliberately NOT "what the operator
/// sent": a capture can sit unsent on the phone, or sent to the relay and not
/// yet pulled, and neither is visible here. The sentence says "pulled" for that
/// reason - the honesty is carried in the words, not in a footnote.
///
/// No gate is run. A blocked row is never persisted as blocked (gate_reason is
/// written only by discard, which also sets decided_at), so a preview would
/// mean computing a decision on a read path. Q-1b signed that out.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PhoneQueueRow {
    pub proposal_id: String,
    pub captured_at: String,
    /// phone::capture_message, reused verbatim. The PC composes; the phone
    /// prints. FI-7 writes no new row sentence.
    pub sentence: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PhoneQueueView {
    pub pending_count: i64,
    /// The signed count line, composed here so the phone never pluralises.
    pub sentence: String,
    pub rows: Vec<PhoneQueueRow>,
}

/// The signed count sentences (Q-3).
fn phone_queue_sentence(n: i64) -> String {
    match n {
        0 => "No captures are waiting on the PC.".to_string(),
        1 => "1 capture pulled and waiting.".to_string(),
        _ => format!("{n} captures pulled and waiting."),
    }
}

/// Read-only projection of the pending captures. Calls only `phone_captures`,
/// which reads `open_proposals` and composes with `capture_message`.
pub fn phone_queue_facts(conn: &Connection) -> Result<PhoneQueueView, String> {
    let rows: Vec<PhoneQueueRow> = phone::phone_captures(conn)?
        .into_iter()
        .map(|c| PhoneQueueRow {
            proposal_id: c.proposal_id,
            captured_at: c.phone_captured_at,
            sentence: c.message,
        })
        .collect();
    let pending_count = rows.len() as i64;
    Ok(PhoneQueueView {
        pending_count,
        sentence: phone_queue_sentence(pending_count),
        rows,
    })
}

pub fn instrument_snapshot(
    conn: &Connection,
    farm_dir: &Path,
    snapshots_dir: &Path,
    now_utc: &str,
    today_local: NaiveDate,
) -> Result<InstrumentSnapshot, String> {
    let rows = crate::health::compute_status(conn, farm_dir, snapshots_dir, now_utc, today_local)?;
    let served_at_display =
        compose_when(now_utc).ok_or_else(|| "served instant is not readable".to_string())?;
    let attention_evaluated_at = attention::attention_evaluated_at();
    let attention_evaluated_at_display = match attention_evaluated_at.as_deref() {
        Some(at) => {
            Some(compose_when(at).ok_or_else(|| "evaluation instant is not readable".to_string())?)
        }
        None => None,
    };
    Ok(InstrumentSnapshot {
        rows,
        served_at: now_utc.to_string(),
        served_at_display,
        attention_evaluated_at,
        attention_evaluated_at_display,
    })
}

pub fn dock_folds(
    conn: &Connection,
    farm_dir: &Path,
    snapshots_dir: &Path,
    now_utc: &str,
    today_local: NaiveDate,
) -> Result<DockFoldsView, String> {
    let snap = instrument_snapshot(conn, farm_dir, snapshots_dir, now_utc, today_local)?;
    Ok(DockFoldsView {
        overall: overall_severity(&snap.rows),
        by_scope: ByScope {
            farm: severity_for_scope(&snap.rows, "farm"),
            system: severity_for_scope(&snap.rows, "system"),
        },
        cards: cards_from_rows(&snap.rows, &snap.served_at),
        checks: checks_from_rows(&snap.rows),
        surfaces: surfaces_table(),
        ranks: ranks_table(),
        today_attention_order: today_attention_order(conn)?,
        worst_clash: worst_clash(conn)?,
    })
}

/// Build the health-derived cards from already-computed rows (tests).
///
/// FI-4b — `served_at` is the same instant the caller is about to publish, so
/// the age rule (A-1) is decided on the PC, in one place, against the very read
/// the operator is looking at.
pub fn cards_from_rows(rows: &[CheckStatus], served_at: &str) -> Vec<DockCard> {
    cards_for(rows, served_at)
}

/// Build the health-derived checks from already-computed rows (tests).
pub fn checks_from_rows(rows: &[CheckStatus]) -> Vec<DockCheck> {
    checks_for(rows)
}

/// Port projection of `InstrumentSnapshot`. Fifteen fields at documentVersion 8.
/// Frozen by e1.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockPortDocument {
    pub document_version: u32,
    pub overall: Option<Severity>,
    pub by_scope: ByScope,
    pub cards: Vec<DockCard>,
    pub checks: Vec<DockCheck>,
    pub worst_clash: Option<WorstClash>,
    /// FI-5 - every open clash, ranked. `worstClash` is this list's head, taken
    /// from the same read, so the two can never disagree.
    pub clashes: Vec<WorstClash>,
    /// FI-7 - captures the PC has pulled and not yet decided. Read-only:
    /// no severity (S5 stands), no gate verdict, no control.
    pub phone_queue: PhoneQueueView,
    /// FI-9 - the export block, composed on the PC. Summary, authority line,
    /// then the packet. The phone prints it and composes nothing.
    pub diagnosis: String,
    /// FI-10 - the capture page's origin. The phone appends `/a/{token}` using
    /// the token it already holds; the secret never travels on this wire and
    /// never enters `diagnosis`.
    pub capture_endpoint: Option<String>,
    pub pull_health: phone_pull::PhonePullView,
    /// FI-10b - when the PC last checked the endpoint, PC-composed. It sits on
    /// the document and not inside `pull_health` because it is a different
    /// fact: pull_health says what the last check found, this says when it
    /// happened. `None` = no check has ever run, and the phone then prints no
    /// line rather than guessing one. The desk reads DockFoldsView and never
    /// this document, so the key is phone-only and types.ts stays untouched.
    pub last_pull_at_display: Option<String>,
    /// When this JSON was written.
    pub served_at: String,
    pub served_at_display: String,
    /// When the PC last evaluated the farm. `None` = not since it started.
    /// This is the age the operator actually needs: the port reads open rows
    /// and never evaluates, so a fresh `servedAt` over an old evaluation was
    /// the honest gap FI-1 closes.
    pub attention_evaluated_at: Option<String>,
    pub attention_evaluated_at_display: Option<String>,
}

/// The severity word exactly as the wire carries it. Written out rather than
/// derived from Debug or serde so the block and the JSON cannot drift; f9c
/// asserts the two agree.
fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Healthy => "Healthy",
        Severity::Degraded => "Degraded",
        Severity::Unhealthy => "Unhealthy",
    }
}

/// FI-9 - the whole diagnosis block, composed here so the phone prints and
/// composes nothing. Human summary first, then the authority line, then the
/// packet - brainstorm 7.
///
/// Every fact is one already in hand from this single read: no second query, no
/// recompute, and health.rs is untouched. The verify line comes from H4's row
/// in `snap.rows`; the desk's `system scan at ...` lines describe a scan the
/// operator just ran in that desk session, which is a different fact and is not
/// available here - E-2 signed that difference rather than hiding it.
pub fn diagnosis_text(
    snap: &InstrumentSnapshot,
    clashes: &[WorstClash],
    phone_queue: &PhoneQueueView,
    pull: &phone_pull::PhonePullView,
) -> Result<String, String> {
    let mut lines: Vec<String> = Vec::new();

    lines.push(SUMMARY_HEAD.to_string());
    lines.push(match overall_severity(&snap.rows) {
        Some(s) => format!("Overall: {}", severity_word(s)),
        None => "Overall: not reported.".to_string(),
    });
    lines.push(match clashes.first().and_then(|c| c.sentence.as_deref()) {
        Some(s) => format!("Worst now: {s}"),
        None => "Worst now: nothing open.".to_string(),
    });
    lines.push(if clashes.is_empty() {
        // FI-8's signed sentence, reused rather than reworded.
        "Today's queue is clear.".to_string()
    } else {
        format!("Today has {} open.", clashes.len())
    });
    // FI-7's signed sentence, reused.
    lines.push(phone_queue.sentence.clone());
    // FI-1's signed wording on both halves.
    lines.push(match snap.attention_evaluated_at_display.as_deref() {
        Some(w) => format!(
            "Read from the PC at {}. Farm last evaluated {w}.",
            snap.served_at_display
        ),
        None => format!(
            "Read from the PC at {}. Farm not evaluated on the PC since it started.",
            snap.served_at_display
        ),
    });

    lines.push(String::new());
    lines.push(DIAGNOSIS_HEAD.to_string());
    lines.push(AUTHORITY_LINE.to_string());

    let h4 = snap.rows.iter().find(|r| r.check_id == "H4");
    lines.push(match h4.and_then(|r| r.ran_at.as_deref()) {
        Some(at) => {
            let when =
                compose_when(at).ok_or_else(|| "verify instant is not readable".to_string())?;
            format!(
                "Last verify on record: {when} — {}",
                h4.map(|r| r.sentence.as_str()).unwrap_or("")
            )
        }
        None => "No verify on record.".to_string(),
    });

    // The desk's check line, byte for byte (healthScopes.ts evidenceText).
    for c in checks_from_rows(&snap.rows) {
        let sev = c.severity.map(severity_word).unwrap_or("");
        let at = c.ran_at.unwrap_or_else(|| "never reported".to_string());
        lines.push(format!(
            "{} [{}] {} (ran_at {})",
            c.check_id, sev, c.sentence, at
        ));
    }

    lines.push(String::new());
    lines.push(PULL_HEAD.to_string());
    lines.push(pull.message.clone());
    for extra in [
        pull.last_ok_message.as_ref(),
        pull.refusal_message.as_ref(),
        pull.gap_message.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        lines.push(extra.clone());
    }

    Ok(lines.join("\n"))
}

/// FI-10 - the capture page's ORIGIN, and never its URL.
///
/// `field_devices::pairing_link` builds `{endpoint}/a/{token}?c=...`. That token
/// is the phone's key: shown once, stored only as a hash, and the same secret
/// the dock port authenticates on. Putting the assembled URL on the wire would
/// write it into the port document - one key away from the export block FI-9
/// added, which the operator is expected to paste into an outside assistant.
///
/// So the wire carries the origin and the phone joins it with the token it
/// already holds. `ScanConfigView` exposes `token_set: bool` and not the token,
/// so this function cannot reach a secret even by mistake. f10b and f10c hold
/// the line from the other side.
pub fn capture_endpoint(conn: &Connection) -> Result<Option<String>, String> {
    Ok(scans::config(conn)?
        .endpoint_url
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty()))
}

pub fn port_document(
    conn: &Connection,
    farm_dir: &Path,
    snapshots_dir: &Path,
    now_utc: &str,
    today_local: NaiveDate,
) -> Result<DockPortDocument, String> {
    let snap = instrument_snapshot(conn, farm_dir, snapshots_dir, now_utc, today_local)?;
    let clashes = clashes_read_only(conn)?;
    let phone_queue = phone_queue_facts(conn)?;
    let capture = capture_endpoint(conn)?;
    let pull_health = phone_pull::latest_view(conn)?;
    let diagnosis = diagnosis_text(&snap, &clashes, &phone_queue, &pull_health)?;
    // Composed after the packet and deliberately not passed to it: F10b-1(b-i)
    // is shell-only, so `diagnosis_text` is byte-identical to the tip.
    let last_pull_at_display = phone_pull::last_pull_at(conn)?
        .as_deref()
        .and_then(compose_when);
    Ok(DockPortDocument {
        document_version: PORT_DOCUMENT_VERSION,
        overall: overall_severity(&snap.rows),
        by_scope: ByScope {
            farm: severity_for_scope(&snap.rows, "farm"),
            system: severity_for_scope(&snap.rows, "system"),
        },
        cards: cards_from_rows(&snap.rows, &snap.served_at),
        checks: checks_from_rows(&snap.rows),
        // One read, two views. Calling both readers would be two reads and two
        // chances to disagree.
        worst_clash: clashes.first().cloned(),
        clashes,
        phone_queue,
        diagnosis,
        capture_endpoint: capture,
        pull_health,
        last_pull_at_display,
        served_at: snap.served_at,
        served_at_display: snap.served_at_display,
        attention_evaluated_at: snap.attention_evaluated_at,
        attention_evaluated_at_display: snap.attention_evaluated_at_display,
    })
}
