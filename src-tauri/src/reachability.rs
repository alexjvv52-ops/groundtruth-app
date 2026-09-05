//! B3 — reachability math. One place where "can sowing still fix this date?"
//! is answered.
//!
//! sow_by = harvest_date - growth_days. A COVER card is REACHABLE when THIS
//! crop's growth_days fits inside (harvest_date - today): sow it today and it
//! is ready on the day. D3: another crop reaching the date is not this
//! promise's answer. Otherwise the card is UNREACHABLE and no tray of this
//! crop sown today can serve it - the operator is owed that sentence, not a
//! picker.
//!
//! Everything here is derived on demand from the `crops` table and the ONE
//! capacity formula (`trays::capacity_by_harvest_date`). No stored flag, no
//! cached "reachable", no second remaining calculation - B5 stays open and
//! separate.
//!
//! HEALTH-JAR: the plan also carries each date's jar (`seed::seed_on_hand`,
//! read once per plan) so the sowing sentence can say when the seed to act
//! on it is not there. The jar is attention inside the sentence, never a
//! gate: `sow_can_serve` does not read it, and an unknown jar (an unweighed
//! sow in the window) grows no tail.

use crate::db;
use crate::models::CapacityRow;
use crate::trays;
use chrono::{Duration, NaiveDate};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CropReach {
    pub crop_id: String,
    pub crop_name: String,
    pub growth_days: i64,
    /// harvest_date - growth_days. The last local day a sow of this crop reaches it.
    pub sow_by: String,
    /// Sown today, or this crop can no longer serve the date.
    pub must_sow_today: bool,
    /// Trays of THIS crop that could be sown today and still fit for the
    /// whole span it needs. None when the ceiling is unknown.
    pub slots_free: Option<i64>,
    /// This crop can be sown today AND its whole span fits on the shelf.
    /// The one door the sow grid reads. slots_free stays beside it as the
    /// number -- the boolean is derived from it here, once, so no surface
    /// has to combine two fields and get the combination wrong.
    /// True when the ceiling is unknown: an unset ceiling constrains nothing.
    pub can_sow_now: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DateReachability {
    pub harvest_date: String,
    pub crop_id: String,
    pub crop_name: String,
    pub days_until_harvest: i64,
    /// This crop sown today is ready on the date. D3: another crop
    /// reaching the date is not this promise's answer.
    pub reachable: bool,
    /// This crop's growth_days. Present even when unreachable - the
    /// operator is owed the number that made the date impossible.
    pub crop_growth_days: Option<i64>,
    /// harvest_date - crop_growth_days. In the past when unreachable; that is
    /// the honest answer to "when could I have acted?".
    pub last_sow_by: Option<String>,
    /// last_sow_by == today. Tomorrow this date is unreachable.
    pub must_sow_today: bool,
    /// Only the short crop, and only when it reaches (0 or 1). Keeps
    /// SowSheet's shape; do not change its type.
    pub crops: Vec<CropReach>,
    /// Straight from trays::capacity_by_harvest_date. Never recomputed here.
    pub trays_growing: i64,
    pub committed_trays: i64,
    /// COVER answer for THIS (date, crop), not the sale answer (D21a / D22a).
    pub remaining_trays: i64,
    /// The order-entry sentence, built here so the form and the card can never
    /// drift into two different truths.
    pub entry_line: String,
    /// slots_free for THIS crop's span. None when the ceiling is unknown --
    /// every shelf sentence is suppressed.
    pub shelf_slots_free: Option<i64>,
    /// A sow offered right now can actually serve this date: reachable in
    /// time AND at least one slot free for the span. This is the door that
    /// decides whether a sow is OFFERED. `reachable` keeps its exact
    /// present meaning and decides what the UNREACHABLE SENTENCE says.
    /// They are different questions and must not be merged.
    pub sow_can_serve: bool,
    /// Live trays of THIS crop past their harvest date, still on the shelf.
    pub overdue_on_shelf: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverOrder {
    pub order_id: String,
    pub venue_name: String,
    pub state: String,
    pub trays: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverDate {
    pub harvest_date: String,
    pub crop_id: String,
    pub crop_name: String,
    pub short_trays: i64,
    /// The one COVER sentence. attention.rs raises this string verbatim and the
    /// Today card renders the same bytes - the card and the plan cannot disagree.
    pub message: String,
    pub reachability: DateReachability,
    /// Non-voided wholesale orders standing on the date that contain THIS crop
    /// - what void acts on. A card must not offer to void an order that does
    ///   not contain its crop.
    pub orders: Vec<CoverOrder>,
    /// Trays grown for exactly this harvest date that have since been harvested
    /// -- off the shelf and into a bag. This is the evidence that a delivery is
    /// physically possible. Counted at plan time so the severity arms stay pure.
    pub harvested_trays: i64,
    /// HEALTH-JAR: THIS crop's jar at plan time - `seed::seed_on_hand`'s
    /// on_hand_oz. `Some` only when the crop has a receipt and the figure is
    /// known; an unweighed sow in the window leaves it `None` (BLANK A), and
    /// `None` never grows a tail. Negative is a short jar, never clamped.
    pub jar_on_hand_oz: Option<f64>,
}

pub fn parse_date(date: &str, label: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| format!("{label} must be YYYY-MM-DD: {date}"))
}

fn minus_days(d: NaiveDate, n: i64) -> Result<NaiveDate, String> {
    d.checked_sub_signed(Duration::days(n))
        .ok_or_else(|| format!("date out of range: {d} minus {n} days"))
}

/// harvest_date - growth_days. The whole of B3 in one line.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn sow_by(harvest_date: &str, growth_days: i64) -> Result<String, String> {
    let h = parse_date(harvest_date, "harvest_date")?;
    Ok(minus_days(h, growth_days)?.format("%Y-%m-%d").to_string())
}

/// "Fri Aug 21" - the shape every money card already uses.
pub(crate) fn format_mon_d_local(date: &str) -> Result<String, String> {
    let d = parse_date(date, "date")?;
    Ok(d.format("%a %b %e").to_string().replace("  ", " "))
}

/// A date computed from growth_days. Growth days are estimates
/// (db.rs TODO(stage-3:self-correcting-estimates)), so every date derived
/// from them says so, in one place, for every surface.
pub(crate) fn format_est_mon_d_local(date: &str) -> Result<String, String> {
    Ok(format!("{} (est.)", format_mon_d_local(date)?))
}

pub(crate) fn tray_word(n: i64) -> String {
    if n == 1 {
        "1 tray".to_string()
    } else {
        format!("{n} trays")
    }
}

fn crop_growth_days(conn: &Connection) -> Result<Vec<(String, String, i64, i64)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, growth_days, blackout_days FROM crops
             ORDER BY growth_days ASC, sort_order ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Public: crop_id required. Today from the clock.
pub fn for_date_for_crop(
    conn: &Connection,
    harvest_date: &str,
    crop_id: &str,
) -> Result<DateReachability, String> {
    let today = db::local_date_today();
    for_date_for_crop_on(conn, harvest_date, crop_id, &today)
}

/// Public wrapper: crop_id required. The COVER card's own math.
pub fn for_date_for_crop_on(
    conn: &Connection,
    harvest_date: &str,
    crop_id: &str,
    today: &str,
) -> Result<DateReachability, String> {
    let caps = trays::capacity_by_harvest_date(conn)?;
    for_date_for_crop_with_caps(conn, harvest_date, crop_id, today, &caps)
}

fn for_date_for_crop_with_caps(
    conn: &Connection,
    harvest_date: &str,
    crop_id: &str,
    today: &str,
    caps: &[CapacityRow],
) -> Result<DateReachability, String> {
    let h = parse_date(harvest_date, "harvest_date")?;
    let t = parse_date(today, "today")?;
    let days_until_harvest = h.signed_duration_since(t).num_days();

    let all = crop_growth_days(conn)?;
    let this = all.iter().find(|(id, _, _, _)| id == crop_id).cloned();
    let (crop_name, crop_growth_days_n, blackout) = match &this {
        Some((_, name, g, b)) => (name.clone(), Some(*g), *b),
        None => (String::new(), None, 0),
    };
    let last_sow_by = match crop_growth_days_n {
        Some(g) => Some(minus_days(h, g)?.format("%Y-%m-%d").to_string()),
        None => None,
    };
    // This crop sown today is ready on the date. D3: another crop
    // reaching the date is not this promise's answer.
    let reachable = crop_growth_days_n
        .map(|g| g <= days_until_harvest)
        .unwrap_or(false);
    let must_sow_today = last_sow_by.as_deref() == Some(today);

    let mut crops = Vec::new();
    if reachable {
        if let Some((id, name, g, _)) = &this {
            let by = minus_days(h, *g)?.format("%Y-%m-%d").to_string();
            let slots_free = trays::slots_free_for_span(conn, today, *g, blackout, today)?;
            crops.push(CropReach {
                crop_id: id.clone(),
                crop_name: name.clone(),
                growth_days: *g,
                must_sow_today: by == today,
                sow_by: by,
                slots_free,
                can_sow_now: slots_free.is_none_or(|k| k > 0),
            });
        }
    }

    let row = caps
        .iter()
        .find(|r| r.harvest_date == harvest_date && r.crop_id == crop_id);
    let trays_growing = row.map(|r| r.trays).unwrap_or(0);
    let committed_trays = row.map(|r| r.sold_trays).unwrap_or(0);
    let remaining_trays = row.map(|r| r.cover_remaining).unwrap_or(0);

    let shelf_slots_free = match crop_growth_days_n {
        Some(g) => trays::slots_free_for_span(conn, today, g, blackout, today)?,
        None => None,
    };
    let sow_can_serve = reachable && shelf_slots_free.is_none_or(|k| k > 0);
    let overdue_on_shelf = trays::overdue_trays_on_shelf_for_crop(conn, today, crop_id)?;

    let mut r = DateReachability {
        harvest_date: harvest_date.to_string(),
        crop_id: crop_id.to_string(),
        crop_name,
        days_until_harvest,
        reachable,
        crop_growth_days: crop_growth_days_n,
        last_sow_by,
        must_sow_today,
        crops,
        trays_growing,
        committed_trays,
        remaining_trays,
        entry_line: String::new(),
        shelf_slots_free,
        sow_can_serve,
        overdue_on_shelf,
    };
    r.entry_line = entry_line(&r)?;
    Ok(r)
}

/// The rule in primitives, so every surface that needs it reads the same one.
///
/// All three facts must hold. days_until_harvest == 0 says the date is today;
/// harvested_trays > 0 says trays for that date were actually grown and taken
/// off the shelf; has_open_order says a promise is still standing. Drop the
/// middle fact and an order taken this morning for trays that were never sown
/// reads exactly like a van waiting to be loaded.
pub fn settle_by_delivering_facts(
    days_until_harvest: i64,
    harvested_trays: i64,
    has_open_order: bool,
) -> bool {
    days_until_harvest == 0 && harvested_trays > 0 && has_open_order
}

/// True when the shortfall sits on today, trays for it were harvested, and an
/// unfulfilled order still stands. The honest action is then to deliver (or
/// void) that order rather than to phone the venue. One rule, used by the Today
/// card, by Health M2, and by the record-time signal, so the three can never
/// give different instructions for the same fact.
pub fn settle_by_delivering(c: &CoverDate) -> bool {
    settle_by_delivering_facts(
        c.reachability.days_until_harvest,
        c.harvested_trays,
        c.orders.iter().any(|o| o.state == "ordered"),
    )
}

/// Reachable in time, but the shelf cannot hold the trays the date needs.
/// ONE door. band() and Health M2 both read this; nothing re-derives the
/// comparison. Two places writing this rule is how M2 came to contradict
/// the Today card in the first place.
pub fn shelf_blocked(c: &CoverDate) -> bool {
    c.reachability.reachable
        && c.reachability
            .shelf_slots_free
            .is_some_and(|room| room < c.short_trays)
}

/// HEALTH-JAR (TRIGGER C). Reachable in time, room on the shelf, and the
/// crop's jar is known to hold nothing - or less than nothing. ONE door: the
/// sentence tail, the must-sow-today ordering and Health M2's third Unhealthy
/// conjunct all read this; nothing re-derives the comparison. Unknown is not
/// zero (BLANK A): a `None` jar is never empty. Arms A-D never see it: an
/// unreachable or shelf-blocked date is not fixed by seed.
pub fn jar_empty(c: &CoverDate) -> bool {
    c.reachability.reachable && !shelf_blocked(c) && c.jar_on_hand_oz.is_some_and(|oz| oz <= 0.0)
}

/// The locked HEALTH-JAR tail. `{x}` is the jar's own figure - already at 0.1
/// from `seed_on_hand` - printed with the one decimal the Farm jar line
/// prints. Empty unless the jar is.
fn jar_tail(c: &CoverDate) -> String {
    if !jar_empty(c) {
        return String::new();
    }
    match c.jar_on_hand_oz {
        Some(oz) if oz < 0.0 => {
            format!(
                " The jar is short {:.1} oz — record seed in on Farm, then sow.",
                oz.abs()
            )
        }
        _ => " The jar is empty — record seed in on Farm, then sow.".to_string(),
    }
}

/// The COVER sentence. B3's hard rule lives here: an unreachable date says
/// "cannot be fixed by sowing" and never invites a sow. No caller may soften it.
pub fn cover_message(c: &CoverDate) -> String {
    let d = format_mon_d_local(&c.harvest_date).unwrap_or_else(|_| c.harvest_date.clone());
    let n = tray_word(c.short_trays);
    let crop = &c.crop_name;
    let r = &c.reachability;

    if !r.reachable {
        // B3's hard rule is untouched: an unreachable date always says
        // "cannot be fixed by sowing". Only the action clause moves.
        if settle_by_delivering(c) {
            // A
            return format!(
                "{d} is short {n} of {crop} and cannot be fixed by sowing - \
                 deliver or void the open order."
            );
        }
        let tail = match r.last_sow_by.as_deref() {
            Some(s) => format!(
                " The last day a {crop} sow could reach it was {}.",
                format_est_mon_d_local(s).unwrap_or_else(|_| s.to_string())
            ),
            None => String::new(),
        };
        // B
        return format!(
            "{d} is short {n} of {crop} and cannot be fixed by sowing - call the venue.{tail}"
        );
    }

    if let Some(room) = r.shelf_slots_free {
        if room < c.short_trays {
            let tail = if r.overdue_on_shelf > 0 {
                format!(
                    " {} past their harvest date and still on the shelf.",
                    tray_word(r.overdue_on_shelf)
                )
            } else {
                String::new()
            };
            // Two real actions, and only these two: take something off the
            // shelf, or shrink the promise. Both verbs already exist in the
            // tree (harvest_trays, void). The shape mirrors the unreachable
            // arm's "deliver or void the open order."
            //
            // The arm is chosen by the same fact the tail reports. Trays
            // past their harvest date are LATE, so "harvest early" would
            // contradict the sentence it follows. This is why the clause is
            // conditional and not a single string.
            let action = if r.overdue_on_shelf > 0 {
                " Harvest them or void the order."
            } else {
                " Harvest early or void the order."
            };
            if room == 0 {
                // C
                return format!(
                    "{d} is short {n} of {crop} - sowing reaches it but there is no \
                     room.{tail}{action}"
                );
            }
            // D
            return format!(
                "{d} is short {n} of {crop} - sowing reaches it but there is only room \
                 for {room}.{tail}{action}"
            );
        }
    }

    // HEALTH-JAR: only the three sowing arms carry the jar. A-D returned above.
    let jar = jar_tail(c);
    if r.must_sow_today {
        // E
        return format!(
            "Sow {n} of {crop} today to cover {d} - today is the last day that reaches it.{jar}"
        );
    }

    match r
        .last_sow_by
        .as_deref()
        .and_then(|s| format_est_mon_d_local(s).ok())
    {
        // F
        Some(by) => format!("{d} is short {n} of {crop} - sow by {by} to cover it.{jar}"),
        // G
        None => format!("{d} is short {n} of {crop} - sowing still covers it.{jar}"),
    }
}

/// The order-entry sentence. Attention, not a gate (GT-D14).
fn entry_line(r: &DateReachability) -> Result<String, String> {
    let d = format_mon_d_local(&r.harvest_date)?;
    let crop = &r.crop_name;
    let when = match r.days_until_harvest {
        k if k < 0 => format!("{d} has already passed."),
        0 => format!("{d} is today."),
        1 => format!("{d} is 1 day out."),
        k => format!("{d} is {k} days out."),
    };
    let growing = match r.trays_growing {
        0 => format!("No {crop} is growing for that date."),
        1 => format!("1 tray of {crop} is already growing for that date."),
        n => format!("{n} trays of {crop} are already growing for that date."),
    };
    let promised = if r.committed_trays > 0 {
        format!(
            " {} of {crop} already promised.",
            tray_word(r.committed_trays)
        )
    } else {
        String::new()
    };
    if !r.reachable {
        let why = match r.crop_growth_days {
            Some(f) => format!(" Nothing sown today is ready by then - {crop} needs {f} days."),
            None => String::new(),
        };
        return Ok(format!("{when}{why} {growing}{promised}"));
    }
    let by = r
        .last_sow_by
        .as_deref()
        .and_then(|s| format_est_mon_d_local(s).ok())
        .map(|s| format!(" Sowing {crop} still reaches it - sow by {s} at the latest."))
        .unwrap_or_default();
    let room = match r.shelf_slots_free {
        None => String::new(),
        Some(0) => format!(" No room to sow more {crop} for that date."),
        Some(k) => format!(" Room to sow {k} more {crop} for that date."),
    };
    Ok(format!("{when}{by}{room} {growing}{promised}"))
}

/// Trays grown for this date that have been harvested -- off the shelf and into
/// a bag. A projection of THE formula, not a second query: the same number the
/// capacity math uses to retire a retail claim is the number that licenses the
/// word "deliver" on a card. Discarded trays are excluded there, so they are
/// excluded here.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn harvested_trays_for_date(conn: &Connection, harvest_date: &str) -> Result<i64, String> {
    Ok(crate::trays::capacity_by_harvest_date(conn)?
        .into_iter()
        .filter(|r| r.harvest_date == harvest_date)
        .map(|r| r.harvested_trays)
        .sum())
}

/// Is an unfulfilled wholesale promise still standing on this date? Read, not
/// assumed — record_order knows the answer by construction, no other door does.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn has_open_order_on(conn: &Connection, harvest_date: &str) -> Result<bool, String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wholesale_orders
              WHERE harvest_date = ?1 AND state = 'ordered'",
            [harvest_date],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

pub(crate) fn orders_on_date_for_crop(
    conn: &Connection,
    harvest_date: &str,
    crop_id: &str,
) -> Result<Vec<CoverOrder>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT o.id, v.name, o.state,
                    (SELECT COALESCE(SUM(l.trays), 0) FROM wholesale_order_lines l
                      WHERE l.order_id = o.id AND l.crop_id = ?2) AS trays
             FROM wholesale_orders o
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE o.harvest_date = ?1 AND o.state != 'voided'
               AND EXISTS (SELECT 1 FROM wholesale_order_lines l2
                            WHERE l2.order_id = o.id AND l2.crop_id = ?2)
             ORDER BY o.ordered_on ASC, o.id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([harvest_date, crop_id], |r| {
            Ok(CoverOrder {
                order_id: r.get(0)?,
                venue_name: r.get(1)?,
                state: r.get(2)?,
                trays: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Rank inside COVER. Unreachable first: it dies today and only a phone call
/// moves it. Then shelf-blocked: reachable in time but no room. Then
/// must-sow-today: today is the last day trays can reach it. Then reachable
/// with slack. Date ascending inside each band, after `jar_rank`.
pub(crate) fn band(c: &CoverDate) -> i64 {
    if !c.reachability.reachable {
        0
    } else if shelf_blocked(c) {
        1
    } else if c.reachability.must_sow_today {
        2
    } else {
        3
    }
}

/// HEALTH-JAR ordering inside the must-sow-today band only: a date whose jar
/// is empty sorts ahead of its band-mates, so it is the head Health M2
/// renders. No new band - slack stays 3 and band 1 is still exactly
/// shelf_blocked (p6t17). Every other row reads 1 here.
fn jar_rank(c: &CoverDate) -> i64 {
    if band(c) == 2 && jar_empty(c) {
        0
    } else {
        1
    }
}

pub fn cover_plan(conn: &Connection) -> Result<Vec<CoverDate>, String> {
    let today = db::local_date_today();
    cover_plan_on(conn, &today)
}

pub fn cover_plan_on(conn: &Connection, today: &str) -> Result<Vec<CoverDate>, String> {
    // Live dates only - the filter B1 landed. Once a date's trays are harvested
    // out of the capacity join the row reads negative forever; that is a
    // completed day, not a debt.
    let caps = trays::capacity_by_harvest_date(conn)?;
    // HEALTH-JAR: the jar, read once for the whole plan. A failed read fails
    // the plan, and the money slot renders "could not read" for M1/M2/M3
    // (INT-001) - never a calm sentence over a jar this render could not see.
    let jar = crate::seed::seed_on_hand(conn)?;
    let mut out = Vec::new();
    for row in caps.iter() {
        if row.cover_remaining >= 0 || row.harvest_date.as_str() < today {
            continue;
        }
        let short_trays = (-row.cover_remaining).max(0);
        let reachability =
            for_date_for_crop_with_caps(conn, &row.harvest_date, &row.crop_id, today, &caps)?;
        let orders = orders_on_date_for_crop(conn, &row.harvest_date, &row.crop_id)?;
        // SCOPE C / BLANK A: this crop's jar row when it has a receipt, its
        // figure when every sow in the window was weighed. Otherwise None.
        let jar_on_hand_oz = jar
            .iter()
            .find(|j| j.crop_id == row.crop_id)
            .and_then(|j| j.on_hand_oz);
        let mut c = CoverDate {
            harvest_date: row.harvest_date.clone(),
            crop_id: row.crop_id.clone(),
            crop_name: row.crop_name.clone(),
            short_trays,
            message: String::new(),
            reachability,
            orders,
            harvested_trays: row.harvested_trays,
            jar_on_hand_oz,
        };
        c.message = cover_message(&c);
        out.push(c);
    }
    out.sort_by(|a, b| {
        band(a)
            .cmp(&band(b))
            .then(jar_rank(a).cmp(&jar_rank(b)))
            .then(a.harvest_date.cmp(&b.harvest_date))
            .then(a.crop_name.cmp(&b.crop_name))
    });
    Ok(out)
}

/// The entry-time over-commit line. GT-D14 forbids REFUSING an order that
/// runs ahead of what is sown -- chefs order before sowing. This makes the
/// promise deliberate instead of silent, and it names the date and the
/// over-committed tray count, which is exactly what GT-D14 requires
/// attention to name.
///
/// None means there is nothing to confirm.
pub fn overcommit_line_on(
    conn: &Connection,
    harvest_date: &str,
    trays: i64,
    today: &str,
    crop_id: &str,
) -> Result<Option<String>, String> {
    let r = for_date_for_crop_on(conn, harvest_date, crop_id, today)?;
    // Trays already growing and unclaimed cover part or all of this order.
    // remaining_trays is THE formula's number -- never recomputed here.
    // Only the part that must still be SOWN can meet the shelf at all.
    let shortfall = trays - r.remaining_trays;
    if shortfall <= 0 {
        return Ok(None);
    }
    if !r.reachable {
        return Ok(None);
    }
    // D3: sowing is not the answer on a date this crop cannot reach, so the
    // shelf-room sentence stays silent and COVER plus the entry line own the
    // fact. Stated here rather than inferred from a None -- for_date_for_crop
    // now returns Some(_) for an unreachable crop, and relying on the old
    // None is what let this sentence gate an order it never used to gate.
    // Unknown ceiling still constrains nothing.
    let Some(room) = r.shelf_slots_free else {
        return Ok(None);
    };
    if shortfall <= room {
        return Ok(None);
    }
    let over = shortfall - room;
    let d = format_mon_d_local(harvest_date)?;
    let crop = &r.crop_name;
    Ok(Some(if room == 0 {
        format!(
            "{d} has no room for more trays. This order needs {} of {crop} sown, and \
             the shelf cannot hold any of them.",
            tray_word(shortfall)
        )
    } else {
        format!(
            "{d} has room for {}. This order needs {} of {crop} sown - {} more than \
             the shelf can hold.",
            tray_word(room),
            tray_word(shortfall),
            tray_word(over)
        )
    }))
}

pub const SHELF_PRESSURE_WINDOW_DAYS: i64 = 30;
pub const SHELF_PRESSURE_MIN_DATES: i64 = 3; // operator-set at Gate 0

pub struct ShelfPressure {
    pub dates: i64,
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub trays: i64,
    pub firing: bool,
}

pub fn shelf_pressure_on(conn: &Connection, today: &str) -> Result<ShelfPressure, String> {
    let start = parse_date(today, "today")?;
    let end = start
        .checked_add_signed(Duration::days(SHELF_PRESSURE_WINDOW_DAYS))
        .ok_or_else(|| {
            format!("date out of range: {today} plus {SHELF_PRESSURE_WINDOW_DAYS} days")
        })?;
    let end_s = end.format("%Y-%m-%d").to_string();
    let plan = cover_plan_on(conn, today)?;
    let mut dates = BTreeSet::new();
    let mut trays = 0i64;
    for c in plan {
        if c.harvest_date.as_str() < today || c.harvest_date.as_str() > end_s.as_str() {
            continue;
        }
        if c.reachability.reachable {
            if let Some(k) = c.reachability.shelf_slots_free {
                if k < c.short_trays {
                    dates.insert(c.harvest_date.clone());
                    trays += c.short_trays - k;
                }
            }
        }
    }
    let dates = dates.len() as i64;
    Ok(ShelfPressure {
        dates,
        trays,
        firing: dates >= SHELF_PRESSURE_MIN_DATES,
    })
}

pub fn shelf_pressure_line(p: &ShelfPressure) -> Option<String> {
    if !p.firing {
        return None;
    }
    Some(format!(
        "In the next 30 days, {} harvest dates are reachable in time but do not have enough shelf room. Your shelf, not your calendar, is the limit.",
        p.dates
    ))
}
