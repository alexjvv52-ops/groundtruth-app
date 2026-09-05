use crate::db;
use crate::events;
use crate::events::{EventRecord, Kind};
use crate::models::{AttentionItem, ResolveResult};
use crate::projection;
use chrono::{Datelike, Local, NaiveDate, Timelike};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;
use uuid::Uuid;

/// FI-1 (F-c) — process-lifetime evaluation stamp.
///
/// `check_attention` is the single funnel every desk path reaches
/// (commands.rs check_attention and dock_folds::today_attention_items in
/// production). Stamping here, and only here, means all twelve signed caller
/// paths record the same fact and none of them changes.
///
/// Process lifetime by ruling F-c: the stamp is lost on restart, and after a
/// restart the desk genuinely has not evaluated yet, so "not evaluated since
/// it started" is true rather than a gap.
///
/// It is a process global rather than a field on the managed `Db` because the
/// dock port thread - the only reader that needs it - has no Tauri state, and
/// threading state into `check_attention` would change every signed caller.
static ATTENTION_EVALUATED_AT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The last time `check_attention` completed an evaluation in this process.
/// `None` means the PC has not evaluated since it started.
pub fn attention_evaluated_at() -> Option<String> {
    ATTENTION_EVALUATED_AT
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
}

fn record_evaluation(at_utc: String) {
    if let Ok(mut guard) = ATTENTION_EVALUATED_AT.lock() {
        *guard = Some(at_utc);
    }
}

/// Insert an open attention item. Idempotent via partial unique index on
/// (kind, entity_id) WHERE resolved_at IS NULL.
pub fn raise(
    conn: &Connection,
    kind: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    message: &str,
    actions: &[&str],
) -> Result<(), String> {
    let created_at = db::utc_now_rfc3339();
    raise_at(
        conn,
        kind,
        entity_type,
        entity_id,
        message,
        actions,
        &created_at,
    )
}

/// Same as `raise`, but stamps `created_at` from the caller's single clock read.
pub fn raise_at(
    conn: &Connection,
    kind: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    message: &str,
    actions: &[&str],
    created_at: &str,
) -> Result<(), String> {
    let id = Uuid::new_v4().to_string();
    let actions_json = serde_json::to_string(actions).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO attention
         (id, kind, entity_type, entity_id, message, actions, created_at, resolved_at, resolved_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
        params![
            id,
            kind,
            entity_type,
            entity_id,
            message,
            actions_json,
            created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Raise an item only if this (kind, entity_id) has NEVER been raised —
/// open or already resolved. For facts that are true once and stay true,
/// so a dismissal sticks.
pub fn raise_once(
    conn: &Connection,
    kind: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    message: &str,
    actions: &[&str],
) -> Result<(), String> {
    let created_at = db::utc_now_rfc3339();
    raise_once_at(
        conn,
        kind,
        entity_type,
        entity_id,
        message,
        actions,
        &created_at,
    )
}

pub fn raise_once_at(
    conn: &Connection,
    kind: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    message: &str,
    actions: &[&str],
    created_at: &str,
) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM attention WHERE kind = ?1 AND entity_id = ?2 LIMIT 1",
            params![kind, entity_id],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    if exists {
        return Ok(());
    }
    raise_at(
        conn,
        kind,
        entity_type,
        entity_id,
        message,
        actions,
        created_at,
    )
}

/// Money-debt kinds. Standing conditions, re-derived on every check — never one-shot.
/// They carry `dismiss`, but a dismissal is an EPISODE, not a mute: it holds only for
/// today and only for the exact fact dismissed. A changed fact or a new day raises again.
/// Permanent silence while money is owed is structurally impossible.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub const MONEY_DEBT_KINDS: [&str; 3] = [
    "money.delivered_unpaid",
    "money.delivery_due",
    "money.capacity_short",
];

/// Raise if absent, refresh the message of an open row so ages never freeze, and honour
/// an episode dismissal.
///
/// The episode key is (kind, entity_id, message) on the operator's local day. Message is
/// part of the key on purpose: "Fri Aug 22 is short 2 trays" is not the same fact as
/// "…short 3 trays", and the collect message carries the age, so an episode can never
/// outlive the day even if a clock drifts. Dismissing acknowledges one fact for one day.
fn raise_or_refresh(
    conn: &Connection,
    kind: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    message: &str,
    actions: &[&str],
    today: &str,
) -> Result<(), String> {
    if dismissed_today(conn, kind, entity_id, message, today)? {
        return Ok(());
    }
    raise(conn, kind, entity_type, entity_id, message, actions)?;
    conn.execute(
        "UPDATE attention SET message = ?1
         WHERE kind = ?2 AND entity_id = ?3 AND resolved_at IS NULL AND message <> ?1",
        params![message, kind, entity_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// True when this exact fact was dismissed by the operator earlier on the same local day.
/// `resolved_at` is UTC; the comparison is made in local days with the shared helper, so
/// the episode boundary is the operator's midnight, not Greenwich's.
fn dismissed_today(
    conn: &Connection,
    kind: &str,
    entity_id: Option<&str>,
    message: &str,
    today: &str,
) -> Result<bool, String> {
    let last: Option<String> = conn
        .query_row(
            "SELECT resolved_at FROM attention
             WHERE kind = ?1 AND entity_id = ?2 AND message = ?3
               AND resolved_by = 'dismissed' AND resolved_at IS NOT NULL
             ORDER BY resolved_at DESC LIMIT 1",
            params![kind, entity_id, message],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(resolved_at) = last else {
        return Ok(false);
    };
    Ok(db::local_date_from_utc_rfc3339(&resolved_at)? == today)
}

pub fn raise_in_tx_at(
    tx: &Transaction<'_>,
    kind: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    message: &str,
    actions: &[&str],
    created_at: &str,
) -> Result<(), String> {
    let id = Uuid::new_v4().to_string();
    let actions_json = serde_json::to_string(actions).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR IGNORE INTO attention
         (id, kind, entity_type, entity_id, message, actions, created_at, resolved_at, resolved_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
        params![
            id,
            kind,
            entity_type,
            entity_id,
            message,
            actions_json,
            created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn raise_snapshot_failed(conn: &Connection) -> Result<(), String> {
    let now = db::utc_now_rfc3339();
    let today = db::local_date_from_utc_rfc3339(&now)?;
    let day = NaiveDate::parse_from_str(&today, "%Y-%m-%d").map_err(|e| e.to_string())?;
    let message = format!("A backup could not be saved on {}.", format_day_month(day));
    raise_at(
        conn,
        "snapshot.failed",
        Some("farm"),
        Some(&today),
        &message,
        &["try_now", "dismiss"],
        &now,
    )
}

pub fn raise_farm_restored(conn: &Connection, label: &str) -> Result<(), String> {
    let message = format!("The farm was restored from a backup taken {label}.");
    // entity_id unique per restore moment so successive restores each raise an item.
    let now = db::utc_now_rfc3339();
    raise_at(
        conn,
        "farm.restored",
        Some("farm"),
        Some(&now),
        &message,
        &["dismiss"],
        &now,
    )
}

pub fn raise_recount_surplus_in_tx(
    tx: &Transaction<'_>,
    crop_id: &str,
    crop_name: &str,
    quantity: i64,
    created_at: &str,
) -> Result<(), String> {
    let trays = tray_word(quantity);
    let message =
        format!("{trays} of {crop_name} were added by recount, with an estimated sow date.");
    raise_in_tx_at(
        tx,
        "recount.surplus",
        Some("crop"),
        Some(crop_id),
        &message,
        &["dismiss"],
        created_at,
    )
}

pub fn raise_recount_shortfall_in_tx(
    tx: &Transaction<'_>,
    crop_id: &str,
    crop_name: &str,
    quantity: i64,
    created_at: &str,
) -> Result<(), String> {
    let message =
        format!("The shelf had {quantity} fewer trays of {crop_name} than the app expected.");
    raise_in_tx_at(
        tx,
        "recount.shortfall",
        Some("crop"),
        Some(crop_id),
        &message,
        &["dismiss"],
        created_at,
    )
}

/// Evaluate derived overdue conditions into persistent rows, then return open items.
pub fn check_attention(conn: &Connection) -> Result<Vec<AttentionItem>, String> {
    evaluate_overdue(conn)?;
    // Stamp only after the evaluation actually completed. A failed evaluation
    // must never claim the farm was looked at.
    record_evaluation(db::utc_now_rfc3339());
    list_open(conn)
}

fn evaluate_overdue(conn: &Connection) -> Result<(), String> {
    let today = db::local_date_today();

    // Overdue harvest: light trays more than 3 days past expected harvest (≥ 4 days).
    // The card is now an episode: dismissed for the operator's local day, back
    // the moment the day turns or the fact changes — never permanently
    // silenced, and never silencing Health.
    let harvest_groups = crate::trays::overdue_harvest_groups(conn, &today)?;
    for g in &harvest_groups {
        let trays = tray_word(g.trays);
        let crop_name = &g.crop_name;
        let days_past = g.days_past;
        let day_word = if days_past == 1 { "day" } else { "days" };
        let message =
            format!("{trays} of {crop_name} were ready to harvest {days_past} {day_word} ago.");
        raise_or_refresh(
            conn,
            "tray.overdue_harvest",
            Some("crop"),
            Some(&g.crop_id),
            &message,
            &["harvest_now", "dismiss"],
            &today,
        )?;
    }

    // Overdue light: blackout more than 2 days past cover check (≥ 3 days).
    // The card is now an episode: dismissed for the operator's local day, back
    // the moment the day turns or the fact changes — never permanently
    // silenced, and never silencing Health.
    let light_groups =
        crate::trays::overdue_light_groups(conn, &today, crate::trays::COVER_CARD_DAYS_PAST)?;
    for g in &light_groups {
        let trays = tray_word(g.trays);
        let crop_name = &g.crop_name;
        let days_past = g.days_past;
        let day_word = if days_past == 1 { "day" } else { "days" };
        let message = format!(
            "{trays} of {crop_name} have been under cover {days_past} {day_word} longer than expected."
        );
        raise_or_refresh(
            conn,
            "tray.overdue_light",
            Some("crop"),
            Some(&g.crop_id),
            &message,
            &["move_now", "dismiss"],
            &today,
        )?;
    }

    // R1 — close the rows the world closed, against the rows just raised.
    resolve_cleared_tray_attention(conn, &harvest_groups, &light_groups)?;

    crate::marketing::raise_overdue_followups(conn)?;
    crate::marketing::raise_standing_quiet(conn)?;
    crate::marketing::raise_standing_requests(conn)?;
    crate::phone::raise_phone_proposals(conn)?;
    evaluate_money_debts(conn)?;
    Ok(())
}

/// B1's three money debts, derived once from live tables.
///
/// B4 reads this same struct for Health M1-M3. Two surfaces, one evaluator: the
/// "Health can never read Healthy while Today would show a money card"
/// invariant is a consequence of this function existing, not of two
/// implementations agreeing.
///
/// Note what this does NOT read: the attention table. A B2 dismissal quiets a
/// card on Today for one local day; it never makes the debt stop existing, so it
/// must never quiet Health. Health is where a debt cannot hide.
#[derive(Debug, Clone, Default)]
pub struct MoneyDebts {
    pub collect: Vec<CollectDebt>,
    pub deliver: Vec<DeliverDebt>,
    pub cover: Vec<crate::reachability::CoverDate>,
}

impl MoneyDebts {
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub fn has_any(&self) -> bool {
        !self.collect.is_empty() || !self.deliver.is_empty() || !self.cover.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct CollectDebt {
    pub order_id: String,
    pub venue_name: String,
    pub delivered_on: String,
    /// Days since delivered_on, local calendar.
    pub days: i64,
    /// False when the recorded delivery breaks the D2-extra invariant
    /// (wholesale::delivered_age_countable). `days` is left as recorded;
    /// only the sentence drops the age phrase.
    pub age_countable: bool,
    pub any_unpriced: bool,
    pub cents: i64,
    /// The Today card sentence, byte-for-byte.
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct DeliverDebt {
    pub order_id: String,
    pub venue_name: String,
    pub harvest_date: String,
    /// 0 = due today; > 0 = that many days past the harvest date.
    pub days_late: i64,
    pub trays: i64,
    pub message: String,
}

pub fn money_debts(conn: &Connection) -> Result<MoneyDebts, String> {
    let today = db::local_date_today();
    money_debts_on(conn, &today)
}

pub fn money_debts_on(conn: &Connection, today: &str) -> Result<MoneyDebts, String> {
    // ---- COLLECT — delivered, unpaid. Oldest first so order matches debt age.
    let mut stmt = conn
        .prepare(
            "SELECT o.id, v.name, o.delivered_on,
                    CAST(julianday(?1) - julianday(o.delivered_on) AS INTEGER) AS days,
                    (SELECT COUNT(*) FROM wholesale_order_lines l
                      WHERE l.order_id = o.id AND l.price_cents_per_tray IS NULL) AS unpriced,
                    (SELECT COALESCE(SUM(l.trays * l.price_cents_per_tray), 0)
                       FROM wholesale_order_lines l WHERE l.order_id = o.id) AS cents,
                    o.harvest_date
             FROM wholesale_orders o
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE o.state = 'delivered' AND o.delivered_on IS NOT NULL
             ORDER BY o.delivered_on ASC, o.id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([today], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut collect = Vec::new();
    for row in rows {
        let (order_id, venue_name, delivered_on, days, unpriced, cents, harvest_date) =
            row.map_err(|e| e.to_string())?;
        let age_countable =
            crate::wholesale::delivered_age_countable(&harvest_date, &delivered_on, today);
        let any_unpriced = unpriced > 0;
        // T0 signed 2026-08-23: no new bytes. When the age is not countable the
        // "delivered {when}" clause is dropped; nothing replaces it.
        let message = match (any_unpriced, age_countable) {
            (true, true) => format!(
                "Collect from {venue_name} — delivered {}, value partly unpriced.",
                day_phrase(days)
            ),
            (true, false) => format!("Collect from {venue_name} — value partly unpriced."),
            (false, true) => format!(
                "Collect {} — {venue_name}, delivered {}.",
                dollars(cents),
                day_phrase(days)
            ),
            (false, false) => format!("Collect {} — {venue_name}.", dollars(cents)),
        };
        collect.push(CollectDebt {
            order_id,
            venue_name,
            delivered_on,
            days,
            age_countable,
            any_unpriced,
            cents,
            message,
        });
    }

    // ---- DELIVER — still ordered, harvest date reached or passed.
    let mut stmt = conn
        .prepare(
            "SELECT o.id, v.name, o.harvest_date,
                    CAST(julianday(?1) - julianday(o.harvest_date) AS INTEGER) AS days,
                    (SELECT COALESCE(SUM(l.trays), 0) FROM wholesale_order_lines l
                      WHERE l.order_id = o.id) AS trays
             FROM wholesale_orders o
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             WHERE o.state = 'ordered' AND o.harvest_date <= ?1
             ORDER BY o.harvest_date ASC, o.id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([today], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut deliver = Vec::new();
    for row in rows {
        let (order_id, venue_name, harvest_date, days, trays) = row.map_err(|e| e.to_string())?;
        let days_late = days.max(0);
        let due = if days <= 0 {
            "due today".to_string()
        } else {
            format!("due {}", day_phrase(days))
        };
        let message = format!("Deliver {} to {venue_name} — {due}.", tray_word(trays));
        deliver.push(DeliverDebt {
            order_id,
            venue_name,
            harvest_date,
            days_late,
            trays,
            message,
        });
    }

    // ---- COVER — live dates only, split by reachability (B3). The plan owns
    // the sentence; nothing here re-words it.
    let cover = crate::reachability::cover_plan_on(conn, today)?;

    Ok(MoneyDebts {
        collect,
        deliver,
        cover,
    })
}

/// B1 — the three money debts, derived fresh on every check.
/// COLLECT: delivered and unpaid. DELIVER: ordered with harvest_date <= today.
/// COVER: a still-live harvest date (>= today) committed past what is sown.
fn evaluate_money_debts(conn: &Connection) -> Result<(), String> {
    let today = db::local_date_today();
    let debts = money_debts_on(conn, &today)?;

    for d in &debts.collect {
        raise_or_refresh(
            conn,
            "money.delivered_unpaid",
            Some("wholesale_order"),
            Some(&d.order_id),
            &d.message,
            &["dismiss"],
            &today,
        )?;
    }
    for d in &debts.deliver {
        raise_or_refresh(
            conn,
            "money.delivery_due",
            Some("wholesale_order"),
            Some(&d.order_id),
            &d.message,
            &["dismiss"],
            &today,
        )?;
    }
    retire_presplit_capacity_short(conn)?;

    for c in &debts.cover {
        let entity_id = format!("{}|{}", c.harvest_date, c.crop_id);
        raise_or_refresh(
            conn,
            "money.capacity_short",
            Some("harvest_date"),
            Some(&entity_id),
            &c.message,
            &["dismiss"],
            &today,
        )?;
    }

    let short: Vec<(String, i64)> = debts
        .cover
        .iter()
        .map(|c| (format!("{}|{}", c.harvest_date, c.crop_id), c.short_trays))
        .collect();
    sweep_legacy_money_attention(conn, &today, &short)?;
    resolve_cleared_money_debts(conn, &today, &short)?;
    Ok(())
}

/// One-time retirement: any open money.capacity_short row whose entity_id
/// contains no '|' is a pre-split date-keyed row. It re-raises under the
/// new key on the same scan if still short. Do NOT use "condition_cleared"
/// — it would be false for a date that is still short.
fn retire_presplit_capacity_short(conn: &Connection) -> Result<(), String> {
    let now = db::utc_now_rfc3339();
    conn.execute(
        "UPDATE attention SET resolved_at = ?1, resolved_by = 'superseded_by_per_crop_card'
         WHERE kind = 'money.capacity_short' AND resolved_at IS NULL
           AND instr(entity_id, '|') = 0",
        params![now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// B2 — the record-time one-shots never linger.
///
/// `wholesale.overcommitted` (wholesale.rs:394-401) and `order.oversold`
/// (money.rs:470-478) are both raised once, at the moment of recording, keyed
/// entity_id = harvest_date, and neither is ever re-evaluated. Each open row is now
/// either superseded by the standing COVER card for the same date, or resolved because
/// the condition it named no longer exists. Both raises stay where they are: they are the
/// instant signal between a write and the next check.
///
/// A date past today is never "still short" here. Post-harvest the tray side drops out of
/// the capacity join while non-voided order lines keep consuming, so every completed date
/// reads negative forever (the defect B1 named). Those rows are closed as `date_passed`.
/// Retail oversell on a date already gone has no in-app exit — the refund lives in Stripe —
/// and that residual is recorded in the commit rather than papered over with a card that
/// cannot be acted on.
fn sweep_legacy_money_attention(
    conn: &Connection,
    today: &str,
    short_dates: &[(String, i64)],
) -> Result<(), String> {
    let now = db::utc_now_rfc3339();
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, entity_id FROM attention
             WHERE kind IN ('wholesale.overcommitted', 'order.oversold')
               AND resolved_at IS NULL",
        )
        .map_err(|e| e.to_string())?;
    let open: Vec<(String, String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    for (id, kind, entity_id) in open {
        let raw = entity_id.unwrap_or_default();
        // order.oversold is now date|crop (same composite as money.capacity_short).
        // Compare the DATE PART so a still-short date still matches. A string
        // with no '|' is the whole id — wholesale.overcommitted stays date-only.
        let date = raw.split_once('|').map(|(d, _)| d).unwrap_or(raw.as_str());
        // A past date is simply gone: nothing re-raises for it, so 'date_passed'
        // is the true reason and it outranks the pre-split migration below.
        // Ordering matters — 'superseded_by_per_crop_card' asserts a live card
        // took over, which is false once the date is behind today.
        if date < today {
            conn.execute(
                "UPDATE attention SET resolved_at = ?1, resolved_by = ?2
                 WHERE id = ?3 AND resolved_at IS NULL",
                params![now, "date_passed", id],
            )
            .map_err(|e| e.to_string())?;
            continue;
        }
        // Legacy pre-split order.oversold on a LIVE date: re-raises under the
        // composite key on this same pass.
        if kind == "order.oversold" && !raw.contains('|') {
            conn.execute(
                "UPDATE attention SET resolved_at = ?1, resolved_by = 'superseded_by_per_crop_card'
                 WHERE id = ?2 AND resolved_at IS NULL",
                params![now, id],
            )
            .map_err(|e| e.to_string())?;
            continue;
        }
        // short_dates carries composite date|crop keys. A date-keyed
        // wholesale.overcommitted / order.oversold row is still short when
        // ANY crop on that date is still short.
        let still_short = short_dates
            .iter()
            .any(|(sd, _)| sd.split_once('|').map(|(d, _)| d).unwrap_or(sd.as_str()) == date);
        let reason = if still_short {
            // The live COVER card for this date now carries the condition. One condition,
            // one card. If the operator dismissed that card today, this one goes quiet with
            // it — that is the same acknowledgement, not a second silence.
            "superseded_by_money_capacity_short"
        } else {
            "condition_cleared"
        };
        conn.execute(
            "UPDATE attention SET resolved_at = ?1, resolved_by = ?2
             WHERE id = ?3 AND resolved_at IS NULL",
            params![now, reason, id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A money debt card disappears the moment the debt does. Nothing here touches
/// operator dismissal — that is B2.
fn resolve_cleared_money_debts(
    conn: &Connection,
    today: &str,
    short_dates: &[(String, i64)],
) -> Result<(), String> {
    let now = db::utc_now_rfc3339();

    conn.execute(
        "UPDATE attention SET resolved_at = ?1, resolved_by = 'condition_cleared'
         WHERE kind = 'money.delivered_unpaid' AND resolved_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM wholesale_orders o
                            WHERE o.id = attention.entity_id AND o.state = 'delivered')",
        params![now],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE attention SET resolved_at = ?1, resolved_by = 'condition_cleared'
         WHERE kind = 'money.delivery_due' AND resolved_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM wholesale_orders o
                            WHERE o.id = attention.entity_id
                              AND o.state = 'ordered' AND o.harvest_date <= ?2)",
        params![now, today],
    )
    .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT id, entity_id FROM attention
             WHERE kind = 'money.capacity_short' AND resolved_at IS NULL",
        )
        .map_err(|e| e.to_string())?;
    let open: Vec<(String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    for (id, entity_id) in open {
        let still = entity_id
            .as_deref()
            .map(|d| short_dates.iter().any(|(sd, _)| sd == d))
            .unwrap_or(false);
        // still compares the FULL composite — a cleared radish card must
        // resolve while the sunflower card on the same date stands.
        if !still {
            conn.execute(
                "UPDATE attention SET resolved_at = ?1, resolved_by = 'condition_cleared'
                 WHERE id = ?2 AND resolved_at IS NULL",
                params![now, id],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// R1 — a tray card disappears the moment its condition does.
///
/// The mirror of the hole RB1 closed on the Health side. `tray.overdue_light` and
/// `tray.overdue_harvest` had no resolver, so a card outlived the shelf: the
/// operator moved the trays to light, F2 went clean, and Today still asked for
/// the work. A card nobody can satisfy teaches an operator to ignore cards, and
/// that is a truth surface failing quietly.
///
/// Per CROP, not globally. `entity_id` IS the crop id, so the condition for a row
/// is "this crop still appears in the groups". Resolving only when every crop is
/// clear would leave a ghost card on whichever crop cleared first.
///
/// It is handed the raiser's OWN rows rather than re-deriving them, so there is
/// no second "is overdue" predicate that could drift: a row is closed exactly
/// when the loop above did not raise it.
///
/// It clears the attention row and nothing else — no event, no tray write, no log
/// entry. Same shape as `resolve_cleared_money_debts` above, for the same reason:
/// an attention row is derived state, not farm truth.
///
/// Operator dismissal is untouched. This closes rows the WORLD closed; Dismiss
/// stays for the rows the operator chooses to close.
fn resolve_cleared_tray_attention(
    conn: &Connection,
    harvest_groups: &[crate::trays::OverdueHarvestGroup],
    light_groups: &[crate::trays::OverdueLightGroup],
) -> Result<(), String> {
    let now = db::utc_now_rfc3339();
    let harvest: Vec<&str> = harvest_groups.iter().map(|g| g.crop_id.as_str()).collect();
    let light: Vec<&str> = light_groups.iter().map(|g| g.crop_id.as_str()).collect();
    resolve_tray_kind(conn, "tray.overdue_harvest", &harvest, &now)?;
    resolve_tray_kind(conn, "tray.overdue_light", &light, &now)?;
    Ok(())
}

/// Close every open row of `kind` whose entity_id is not among the crops still
/// raising it.
fn resolve_tray_kind(
    conn: &Connection,
    kind: &str,
    still_raising: &[&str],
    now: &str,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, entity_id FROM attention
             WHERE kind = ?1 AND resolved_at IS NULL",
        )
        .map_err(|e| e.to_string())?;
    let open: Vec<(String, Option<String>)> = stmt
        .query_map([kind], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    for (id, entity_id) in open {
        let crop = entity_id.unwrap_or_default();
        if still_raising.contains(&crop.as_str()) {
            continue;
        }
        conn.execute(
            "UPDATE attention SET resolved_at = ?1, resolved_by = 'condition_cleared'
             WHERE id = ?2 AND resolved_at IS NULL",
            params![now, id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn day_phrase(days: i64) -> String {
    match days {
        d if d <= 0 => "today".to_string(),
        1 => "1 day ago".to_string(),
        d => format!("{d} days ago"),
    }
}

pub(crate) fn dollars(cents: i64) -> String {
    format!("${}.{:02}", cents / 100, (cents % 100).abs())
}

/// "Fri Aug 21" — same shape the wholesale overcommit message already uses.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
fn format_mon_d_local(date: &str) -> Result<String, String> {
    crate::reachability::format_mon_d_local(date)
}

fn list_open(conn: &Connection) -> Result<Vec<AttentionItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, entity_type, entity_id, message, actions, created_at
             FROM attention
             WHERE resolved_at IS NULL
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        let (id, kind, entity_type, entity_id, message, actions_json, created_at) =
            r.map_err(|e| e.to_string())?;
        let actions: Vec<String> =
            serde_json::from_str(&actions_json).map_err(|e| e.to_string())?;
        out.push(AttentionItem {
            id,
            kind,
            entity_type,
            entity_id,
            message,
            actions,
            created_at,
        });
    }
    Ok(out)
}

/// The read half of `check_attention`: open rows as they stand, evaluating
/// nothing. The dock port reads through this so a GET can never raise a row
/// (S2c-2, signed 2026-08-21).
pub fn open_items(conn: &Connection) -> Result<Vec<AttentionItem>, String> {
    list_open(conn)
}

/// GT-D17 / fence 3: a standing-request card is closed only by
/// marketing::decide_standing_request, which writes the durable outcome in the
/// same transaction. The generic doors refuse it so nothing can quiet a
/// candidate without a decision.
/// GT-D20 / rack-side fence 1: a phone capture is closed only by
/// confirm_phone_captures / discard_phone_capture, which write the durable
/// outcome in the same transaction.
fn refuse_generic_close_of_standing_request(conn: &Connection, id: &str) -> Result<(), String> {
    if let Some(item) = get_open(conn, id)? {
        if item.kind == crate::marketing::STANDING_REQUEST_ATTENTION_KIND {
            return Err(crate::marketing::STANDING_REQUEST_DECIDE_ONLY.to_string());
        }
        if item.kind == crate::phone::PHONE_PROPOSAL_ATTENTION_KIND {
            return Err(crate::phone::PHONE_CAPTURE_DECIDE_ONLY.to_string());
        }
    }
    Ok(())
}

pub fn dismiss_attention(conn: &mut Connection, id: &str) -> Result<(), String> {
    refuse_generic_close_of_standing_request(conn, id)?;
    resolve_with_action(conn, id, "dismissed", false).map(|_| ())
}

/// DESK-LABEL-1 (2026-08-25) - the closed set of operator actions this build
/// knows how to perform. Every raise site in this file and in poll.rs draws
/// from it, and Today.tsx carries a label for each one; dl1 in
/// dock_port_tests.rs proves the three agree.
///
/// "dismissed" is deliberately absent. It is the internal close written by
/// dismiss_attention, never an operator action and never on a raised row.
pub const KNOWN_ACTIONS: &[&str] = &[
    "try_now",
    "harvest_now",
    "move_now",
    "open_in_stripe",
    "dismiss",
];
pub fn resolve_attention(
    conn: &mut Connection,
    id: &str,
    action: &str,
) -> Result<ResolveResult, String> {
    if action == "dismiss" || action == "dismissed" {
        return Err("use dismiss_attention to dismiss".to_string());
    }
    refuse_generic_close_of_standing_request(conn, id)?;
    resolve_with_action(conn, id, action, true)
}

fn resolve_with_action(
    conn: &mut Connection,
    id: &str,
    action: &str,
    require_listed_action: bool,
) -> Result<ResolveResult, String> {
    let item = get_open(conn, id)?.ok_or_else(|| format!("attention item not open: {id}"))?;
    // DESK-LABEL-1 - refused on every path, not only the listed one. Before
    // this guard an unrecognised key that happened to be listed on a row
    // passed the check below, matched no arm in the match that follows, did no
    // work, and still wrote AttentionResolved. The ask vanished and the farm
    // did not move. That silent close is the defect; the raw label was only
    // how it announced itself.
    if action != "dismissed" && !KNOWN_ACTIONS.contains(&action) {
        return Err(format!("action '{action}' is not available on this item"));
    }
    if require_listed_action && action != "dismissed" && !item.actions.iter().any(|a| a == action) {
        return Err(format!("action '{action}' is not available on this item"));
    }

    let tray_ids = match action {
        "harvest_now" => tray_ids_for_crop(conn, item.entity_id.as_deref(), "light")?,
        "move_now" => match item.entity_id.as_deref() {
            Some(crop_id) => {
                crate::trays::due_for_light_ids_for_crop(conn, crop_id, &db::local_date_today())?
            }
            None => Vec::new(),
        },
        _ => Vec::new(),
    };

    let open_url = if action == "open_in_stripe" {
        match (item.entity_type.as_deref(), item.entity_id.as_deref()) {
            (Some("stripe_session"), Some(session_id)) => Some(
                crate::money::stripe_session_dashboard_url(conn, session_id)?,
            ),
            (_, Some(order_id)) => crate::money::stripe_dashboard_url(conn, order_id)?,
            _ => None,
        }
    } else {
        None
    };

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    let payload = json!({
        "attentionId": id,
        "action": action,
        "kind": item.kind,
    });
    let inverse = json!({ "op": "none" });
    let event = EventRecord::originated(
        Kind::AttentionResolved,
        "attention",
        id,
        payload,
        inverse,
        projection::handler_now(),
        None,
        None,
        Some(projection::handler_new_id()),
    );

    // Live SQL update — apply_event is an explicit no-op for this kind
    // (attention outside the replay ledger; same pattern as snapshot.taken).
    apply_attention_resolved(&tx, &event)?;
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;

    tx.commit().map_err(|e| e.to_string())?;
    Ok(ResolveResult { tray_ids, open_url })
}

/// Read an open attention item by id (shared with marketing::resolve_followup).
pub(crate) fn get_open_item(conn: &Connection, id: &str) -> Result<Option<AttentionItem>, String> {
    get_open(conn, id)
}

/// Resolve an open attention row inside an open transaction (no commit).
/// Used by marketing::resolve_followup so touch.logged lands in the same tx.
pub(crate) fn resolve_open_in_tx(
    tx: &Transaction<'_>,
    id: &str,
    action: &str,
    created_at: &str,
) -> Result<(), String> {
    let item = get_open(tx, id)?.ok_or_else(|| format!("attention item not open: {id}"))?;
    if action != "dismissed" && !item.actions.iter().any(|a| a == action) {
        return Err(format!("action '{action}' is not available on this item"));
    }
    let payload = json!({
        "attentionId": id,
        "action": action,
        "kind": item.kind,
    });
    let event = EventRecord::originated(
        Kind::AttentionResolved,
        "attention",
        id,
        payload,
        json!({ "op": "none" }),
        created_at.to_string(),
        None,
        None,
        Some(projection::handler_new_id()),
    );
    apply_attention_resolved(tx, &event)?;
    projection::apply_event(tx, &event)?;
    events::insert_event(tx, &event)?;
    Ok(())
}

fn get_open(conn: &Connection, id: &str) -> Result<Option<AttentionItem>, String> {
    conn.query_row(
        "SELECT id, kind, entity_type, entity_id, message, actions, created_at
         FROM attention WHERE id = ?1 AND resolved_at IS NULL",
        [id],
        |row| {
            let actions_json: String = row.get(5)?;
            let actions: Vec<String> = serde_json::from_str(&actions_json).unwrap_or_default();
            Ok(AttentionItem {
                id: row.get(0)?,
                kind: row.get(1)?,
                entity_type: row.get(2)?,
                entity_id: row.get(3)?,
                message: row.get(4)?,
                actions,
                created_at: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn tray_ids_for_crop(
    conn: &Connection,
    crop_id: Option<&str>,
    state: &str,
) -> Result<Vec<String>, String> {
    let Some(crop_id) = crop_id else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare(
            "SELECT id FROM trays WHERE crop_id = ?1 AND state = ?2 ORDER BY sown_on ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![crop_id, state], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Live resolve of an open attention row from `payload.{attentionId,action}`,
/// stamped with `event.created_at` (no clock reads). Called only from the live
/// handler — `apply_event` treats `attention.resolved` as a no-op during replay
/// (attention outside the replay ledger).
pub(crate) fn apply_attention_resolved(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    let attention_id = event
        .payload
        .get("attentionId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "attention.resolved payload missing attentionId".to_string())?;
    let action = event
        .payload
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "attention.resolved payload missing action".to_string())?;
    let n = tx
        .execute(
            "UPDATE attention SET resolved_at = ?1, resolved_by = ?2
             WHERE id = ?3 AND resolved_at IS NULL",
            params![event.created_at, action, attention_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!("attention item not open: {attention_id}"));
    }
    Ok(())
}

pub fn reopen_attention(tx: &Transaction<'_>, attention_id: &str) -> Result<(), String> {
    let (kind, entity_id): (String, Option<String>) = tx
        .query_row(
            "SELECT kind, entity_id FROM attention WHERE id = ?1",
            [attention_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("failed to reopen attention {attention_id}"))?;

    let already_open: bool = tx
        .query_row(
            "SELECT 1 FROM attention
             WHERE kind = ?1 AND entity_id IS ?2 AND resolved_at IS NULL AND id <> ?3",
            params![kind, entity_id, attention_id],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    if already_open {
        return Ok(());
    }

    let n = tx
        .execute(
            "UPDATE attention SET resolved_at = NULL, resolved_by = NULL WHERE id = ?1",
            [attention_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!("failed to reopen attention {attention_id}"));
    }
    Ok(())
}

fn tray_word(n: i64) -> String {
    crate::reachability::tray_word(n)
}

fn format_day_month(d: NaiveDate) -> String {
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let month = months[(d.month0()) as usize];
    format!("{} {month}", d.day())
}

/// Plain-English snapshot time for farm.restored messages.
pub fn restore_label_from_taken_at(taken_at: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(taken_at) {
        let local = dt.with_timezone(&Local);
        return format_restore_label(local);
    }
    // Fallback: try parsing filename-style via snapshots helper path.
    taken_at.to_string()
}

pub fn format_restore_label(local: chrono::DateTime<Local>) -> String {
    let today = Local::now().date_naive();
    let day = local.date_naive();
    let time = format_clock(local);
    let diff = (today - day).num_days();
    if diff == 0 {
        format!("Today at {time}")
    } else if diff == 1 {
        format!("Yesterday at {time}")
    } else {
        let weekday = local.format("%A");
        format!("{weekday} at {time}")
    }
}

pub(crate) fn format_clock(local: chrono::DateTime<Local>) -> String {
    let hour24 = local.hour();
    let minute = local.minute();
    let (hour12, ampm) = match hour24 {
        0 => (12, "am"),
        1..=11 => (hour24, "am"),
        12 => (12, "pm"),
        _ => (hour24 - 12, "pm"),
    };
    format!("{hour12}:{minute:02} {ampm}")
}
