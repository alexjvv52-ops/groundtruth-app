use crate::event_partition::{EventClass, EventDomain};
use rusqlite::{params, Connection, Transaction};
use serde_json::{json, Value};
use uuid::Uuid;

pub use crate::event_partition::Kind;

/// Full event_log row as carried by events.jsonl / built by handlers.
#[derive(Debug, Clone)]
pub struct EventRecord {
    pub seq: Option<i64>,
    pub event_id: String,
    pub kind: Kind,
    pub entity_type: String,
    pub entity_id: String,
    pub payload: Value,
    pub inverse: Value,
    pub origin: String,
    pub event_domain: String,
    pub event_class: Option<String>,
    pub reverses_event_id: Option<String>,
    pub undoes_seq: Option<i64>,
    pub undone_at: Option<String>,
    pub created_at: String,
}

impl EventRecord {
    /// Build an originated Farm OS event. Tier comes from `Kind` — callers
    /// never choose `event_domain` / `event_class`. `created_at` is required;
    /// this constructor does not read the clock.
    #[allow(clippy::too_many_arguments)] // H-7 Class B: permanent. The GT-D9 write choke point - every writer in the tree passes through this signature.
    pub fn originated(
        kind: Kind,
        entity_type: impl Into<String>,
        entity_id: impl Into<String>,
        payload: Value,
        inverse: Value,
        created_at: impl Into<String>,
        undoes_seq: Option<i64>,
        reverses_event_id: Option<&str>,
        event_id: Option<String>,
    ) -> EventRecord {
        let (domain, class) = kind.tier();
        EventRecord {
            seq: None,
            event_id: event_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            kind,
            entity_type: entity_type.into(),
            entity_id: entity_id.into(),
            payload,
            inverse,
            origin: "farm_os".into(),
            event_domain: domain.as_str().to_string(),
            event_class: class.map(|c| c.as_str().to_string()),
            reverses_event_id: reverses_event_id.map(|s| s.to_string()),
            undoes_seq,
            undone_at: None,
            created_at: created_at.into(),
        }
    }

    pub fn from_jsonl_value(v: &Value) -> Result<EventRecord, String> {
        let kind_s = v
            .get("kind")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "event missing kind".to_string())?;
        let kind = Kind::parse(kind_s)?;
        let payload = v
            .get("payload")
            .cloned()
            .ok_or_else(|| "event missing payload".to_string())?;
        let inverse = v
            .get("inverse")
            .cloned()
            .ok_or_else(|| "event missing inverse".to_string())?;
        Ok(EventRecord {
            seq: v.get("seq").and_then(|x| x.as_i64()),
            event_id: req_str(v, "event_id")?,
            kind,
            entity_type: req_str(v, "entity_type")?,
            entity_id: req_str(v, "entity_id")?,
            payload,
            inverse,
            origin: req_str(v, "origin")?,
            event_domain: req_str(v, "event_domain")?,
            event_class: opt_str(v, "event_class"),
            reverses_event_id: opt_str(v, "reverses_event_id"),
            undoes_seq: v.get("undoes_seq").and_then(|x| x.as_i64()),
            undone_at: opt_str(v, "undone_at"),
            created_at: req_str(v, "created_at")?,
        })
    }
}

fn req_str(v: &Value, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("event missing {key}"))
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| if x.is_null() { None } else { x.as_str() })
        .map(|s| s.to_string())
}

/// Originating write entry point. Tier is taken from `event.kind` via the
/// total `Kind::tier` map — `event.event_domain` / `event.event_class` on the
/// struct are ignored for the INSERT so a caller cannot mislabel. Never reads
/// the clock; `created_at` must already be set.
pub fn write_event(tx: &Transaction<'_>, event: &EventRecord) -> Result<i64, String> {
    // Retired freeze gate (identity::CUTOVER_LIVE): lock_is_active is always
    // false, so enforce_cutover_lock is never reached. Kept for the cleanup.
    if crate::identity::lock_is_active() {
        crate::identity::enforce_cutover_lock(event.kind)?;
    }
    write_event_inner(tx, event)
}

/// Replay of an operator-accepted bundle. NOT an originating write: what
/// enters is decided by import::apply_import's plan, which refuses a bundle
/// whole rather than per-record. The freeze gate that once sat beside that
/// plan is retired (identity::CUTOVER_LIVE). GT-D8.
pub fn write_imported_event(tx: &Transaction<'_>, event: &EventRecord) -> Result<i64, String> {
    write_event_inner(tx, event)
}

fn write_event_inner(tx: &Transaction<'_>, event: &EventRecord) -> Result<i64, String> {
    if event.created_at.is_empty() {
        return Err("write_event requires created_at (no clock in the write path)".into());
    }
    // Seal the NEW consumption kind only — grow/cost payloads stay open.
    if event.kind == Kind::ConsumptionPhysical {
        crate::consumption::validate_consumption_event(event)?;
    }
    // Seal every marketing kind at the same choke point (GT-D1).
    if !crate::identity::HOUSEKEEPING_KINDS.contains(&event.kind)
        && matches!(
            event.kind.tier().0,
            crate::event_partition::EventDomain::Marketing
        )
    {
        crate::marketing::validate_marketing_event(event)?;
    }
    // Seal the Track 4 residual kinds and cost correction kinds.
    crate::mileage::validate_mileage_event(event)?;
    crate::assets::validate_asset_event(event)?;
    crate::income::validate_income_event(event)?;
    crate::costs::validate_cost_event(event)?;
    crate::wholesale::validate_wholesale_event(event)?;
    // Seal the GT-D24 leftover kind (register tier). Shape only: the harvested
    // cap is the write door's gate, never a read at this choke point.
    crate::leftover::validate_leftover_event(event)?;
    // Seal the GT-D25 seed kind (register tier). Shape only: the crop check is
    // the write door's gate, and apply_* never resolves a crop name.
    crate::seed::validate_seed_event(event)?;
    // Seal the GT-D20 proposal kinds (grow tier, candidate only).
    crate::phone::validate_phone_event(event)?;
    let (domain, class) = event.kind.tier();
    raw_insert_event_log(
        tx,
        &event.event_id,
        event.kind.as_str(),
        &event.entity_type,
        &event.entity_id,
        &event.payload,
        &event.inverse,
        event.undone_at.as_deref(),
        event.undoes_seq,
        &event.created_at,
        &event.origin,
        domain,
        class,
        event.reverses_event_id.as_deref(),
    )
}

/// Compatibility name used by existing tests and handlers. Same choke point.
pub fn insert_event(tx: &Transaction<'_>, event: &EventRecord) -> Result<i64, String> {
    write_event(tx, event)
}

/// String-typed entry for tests and the operator refusal demo only.
/// Returns Err for anything outside the Kind enum and inserts nothing.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
#[allow(clippy::too_many_arguments)] // H-7 Class B: permanent. The GT-D9 write choke point - every writer in the tree passes through this signature.
pub fn try_write_event_kind(
    tx: &Transaction<'_>,
    kind: &str,
    entity_type: &str,
    entity_id: &str,
    payload: &Value,
    inverse: &Value,
    created_at: &str,
    undoes_seq: Option<i64>,
    reverses_event_id: Option<&str>,
) -> Result<i64, String> {
    let kind = Kind::parse(kind)?;
    let event = EventRecord::originated(
        kind,
        entity_type,
        entity_id,
        payload.clone(),
        inverse.clone(),
        created_at,
        undoes_seq,
        reverses_event_id,
        None,
    );
    write_event(tx, &event)
}

/// Private. The only SQL INSERT into event_log for originated writes.
#[allow(clippy::too_many_arguments)] // H-7 Class B: permanent. The GT-D9 write choke point - every writer in the tree passes through this signature.
fn raw_insert_event_log(
    tx: &Transaction<'_>,
    event_id: &str,
    kind: &str,
    entity_type: &str,
    entity_id: &str,
    payload: &Value,
    inverse: &Value,
    undone_at: Option<&str>,
    undoes_seq: Option<i64>,
    created_at: &str,
    origin: &str,
    domain: EventDomain,
    class: Option<EventClass>,
    reverses_event_id: Option<&str>,
) -> Result<i64, String> {
    let payload_s = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    let inverse_s = serde_json::to_string(inverse).map_err(|e| e.to_string())?;
    let class_s = class.map(|c| c.as_str());

    tx.execute(
        "INSERT INTO event_log
         (id, kind, entity_type, entity_id, payload, inverse, undone_at, undoes_seq, created_at,
          origin, event_domain, event_class, reverses_event_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            event_id,
            kind,
            entity_type,
            entity_id,
            payload_s,
            inverse_s,
            undone_at,
            undoes_seq,
            created_at,
            origin,
            domain.as_str(),
            class_s,
            reverses_event_id,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(tx.last_insert_rowid())
}

pub struct UndoableEvent {
    pub seq: i64,
    pub id: String,
    pub kind: String,
    pub inverse: String,
}

/// Kinds whose reversal is NOT carried by `inverse` but performed explicitly
/// by the undo path. attention.resolved records {"op":"none"} yet genuinely
/// reopens the attention row (trays::undo_last). Anything added here MUST have
/// a real reversal there — this list is the narrow exception, not a way to
/// keep a kind looking undoable.
pub const REVERSED_BY_UNDO_PATH: &[&str] = &["attention.resolved"];

/// The one answer to "would undoing this actually reverse anything?".
///
/// L9: wholesale.* and every marketing kind ship inverse {"op":"none"}.
/// Undoing one stamped undone_at, appended an undo event, and changed nothing
/// — a recorded lie, and it ate the operator's real last action on the way
/// past. Derived from the row rather than from a kind list, so a new kind that
/// ships {"op":"none"} is refused the day it lands instead of the day someone
/// remembers to update a list. That drift is how this bug reached the money
/// engine after the same thing was already fixed for costs.
///
/// An inverse that will not parse cannot be shown to reverse anything, so it
/// does not: ambiguity refuses the undo rather than recording one.
pub fn reverses_something(kind: &str, inverse: &str) -> bool {
    if REVERSED_BY_UNDO_PATH.contains(&kind) {
        return true;
    }
    match serde_json::from_str::<Value>(inverse) {
        Ok(v) => v
            .get("op")
            .and_then(|o| o.as_str())
            .map(|op| op != "none")
            .unwrap_or(false),
        Err(_) => false,
    }
}

pub fn newest_undoable(conn: &Connection) -> Result<Option<UndoableEvent>, String> {
    // Skip undo markers, externally-originated stripe.* observations, and
    // register-tier physical consumption — undoing a payment locally would
    // desync Stripe; consumption records must not block grow undo after sow.
    // Mileage / asset / income corrections are made by appending a correction
    // event, not by undo — undoing one locally would leave the projection row
    // standing.
    // cost.money_out cannot be undone: its inverse is inert and no delete path
    // for cost_events exists — leaving it eligible meant Undo silently consumed
    // itself on a cost and left the operator's actual last action in place.
    // B7: the SQL list above is POLICY — kinds we decline to undo even when they
    // could be. Whether an event reverses anything at all is a property of the
    // row, answered by reverses_something, so the scan below skips inert events
    // without needing to name them. Both filters are load-bearing; neither
    // subsumes the other.
    let mut stmt = conn
        .prepare(
            "SELECT seq, id, kind, inverse FROM event_log
             WHERE undone_at IS NULL
               AND kind <> 'undo'
               AND kind NOT LIKE 'stripe.%'
               AND kind <> 'consumption.physical'
               AND kind NOT IN ('mileage.trip','mileage.trip_corrected',
                                'mileage.trip_voided','asset.recorded','asset.corrected',
                                'asset.voided','income.received','income.corrected',
                                'income.voided','cost.money_out','wholesale.payment_reversed',
                                'wholesale.bad_debt')
             ORDER BY seq DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(UndoableEvent {
                seq: row.get(0)?,
                id: row.get(1)?,
                kind: row.get(2)?,
                inverse: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    for r in rows {
        let e = r.map_err(|err| err.to_string())?;
        if reverses_something(&e.kind, &e.inverse) {
            return Ok(Some(e));
        }
    }
    Ok(None)
}

/// Apply an inverse JSON object to current-state tables.
/// `updated_at` must come from the applying event's created_at (no clock reads).
pub fn apply_inverse(
    tx: &Transaction<'_>,
    inverse_json: &str,
    updated_at: &str,
) -> Result<(), String> {
    let v: Value = serde_json::from_str(inverse_json).map_err(|e| e.to_string())?;
    let op = v
        .get("op")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "inverse missing op".to_string())?;

    match op {
        "none" => Ok(()),
        "delete_tray" => {
            let tray_id = v
                .get("trayId")
                .and_then(|x| x.as_str())
                .ok_or_else(|| "inverse delete_tray missing trayId".to_string())?;
            tx.execute("DELETE FROM trays WHERE id = ?1", [tray_id])
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        "set_tray_state" => apply_set_tray_state(tx, &v, updated_at),
        "set_trays_state" => {
            let trays = v
                .get("trays")
                .and_then(|x| x.as_array())
                .ok_or_else(|| "inverse set_trays_state missing trays".to_string())?;
            for entry in trays {
                apply_set_tray_state(tx, entry, updated_at)?;
            }
            Ok(())
        }
        "shift_tray_dates" => {
            let tray_id = v
                .get("trayId")
                .and_then(|x| x.as_str())
                .ok_or_else(|| "inverse shift_tray_dates missing trayId".to_string())?;
            let days = v
                .get("days")
                .and_then(|x| x.as_i64())
                .ok_or_else(|| "inverse shift_tray_dates missing days".to_string())?;
            shift_tray_dates(tx, tray_id, days, updated_at)
        }
        "restore_discard" => apply_restore_discard(tx, &v, updated_at),
        "restore_recount" => apply_restore_recount(tx, &v, updated_at),
        // Inert: attention mutations belong to the command handler layer
        // (decisions/RULING-attention-outside-replay-ledger.md). Legacy sealed
        // events may still carry this op; new events emit {"op":"none"}.
        "reopen_attention" => Ok(()),
        other => Err(format!("unknown inverse op: {other}")),
    }
}

fn apply_restore_recount(tx: &Transaction<'_>, v: &Value, updated_at: &str) -> Result<(), String> {
    let items = v
        .get("items")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "inverse restore_recount missing items".to_string())?;
    for item in items {
        let item_op = item
            .get("op")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "restore_recount item missing op".to_string())?;
        match item_op {
            "set_tray_state" => apply_set_tray_state(tx, item, updated_at)?,
            "unsplit" => {
                let source_id = item
                    .get("sourceTrayId")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| "unsplit missing sourceTrayId".to_string())?;
                let restore_qty = item
                    .get("restoreQuantity")
                    .and_then(|x| x.as_i64())
                    .ok_or_else(|| "unsplit missing restoreQuantity".to_string())?;
                let delete_id = item
                    .get("deleteTrayId")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| "unsplit missing deleteTrayId".to_string())?;
                tx.execute(
                    "UPDATE trays SET quantity = ?1, updated_at = ?2 WHERE id = ?3",
                    params![restore_qty, updated_at, source_id],
                )
                .map_err(|e| e.to_string())?;
                tx.execute("DELETE FROM trays WHERE id = ?1", [delete_id])
                    .map_err(|e| e.to_string())?;
            }
            "delete_tray" => {
                let tray_id = item
                    .get("trayId")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| "delete_tray missing trayId".to_string())?;
                tx.execute("DELETE FROM trays WHERE id = ?1", [tray_id])
                    .map_err(|e| e.to_string())?;
            }
            other => {
                return Err(format!("unknown restore_recount item op: {other}"));
            }
        }
    }
    Ok(())
}

fn apply_restore_discard(tx: &Transaction<'_>, v: &Value, updated_at: &str) -> Result<(), String> {
    let items = v
        .get("items")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "inverse restore_discard missing items".to_string())?;
    for item in items {
        let item_op = item
            .get("op")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "restore_discard item missing op".to_string())?;
        match item_op {
            "set_tray_state" => apply_set_tray_state(tx, item, updated_at)?,
            "unsplit" => {
                let source_id = item
                    .get("sourceTrayId")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| "unsplit missing sourceTrayId".to_string())?;
                let restore_qty = item
                    .get("restoreQuantity")
                    .and_then(|x| x.as_i64())
                    .ok_or_else(|| "unsplit missing restoreQuantity".to_string())?;
                let delete_id = item
                    .get("deleteTrayId")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| "unsplit missing deleteTrayId".to_string())?;
                tx.execute(
                    "UPDATE trays SET quantity = ?1, updated_at = ?2 WHERE id = ?3",
                    params![restore_qty, updated_at, source_id],
                )
                .map_err(|e| e.to_string())?;
                tx.execute("DELETE FROM trays WHERE id = ?1", [delete_id])
                    .map_err(|e| e.to_string())?;
            }
            other => {
                return Err(format!("unknown restore_discard item op: {other}"));
            }
        }
    }
    Ok(())
}

fn apply_set_tray_state(tx: &Transaction<'_>, v: &Value, updated_at: &str) -> Result<(), String> {
    let tray_id = v
        .get("trayId")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "inverse set_tray_state missing trayId".to_string())?;
    let state = v
        .get("state")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "inverse set_tray_state missing state".to_string())?;
    let clear = v
        .get("clear")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();

    tx.execute(
        "UPDATE trays SET state = ?1, updated_at = ?2 WHERE id = ?3",
        params![state, updated_at, tray_id],
    )
    .map_err(|e| e.to_string())?;

    for col in clear {
        let col_name = col
            .as_str()
            .ok_or_else(|| "inverse clear entry not a string".to_string())?;
        let sql = match col_name {
            "sown_on" => "UPDATE trays SET sown_on = NULL WHERE id = ?1",
            "blackout_on" => "UPDATE trays SET blackout_on = NULL WHERE id = ?1",
            "light_on" => "UPDATE trays SET light_on = NULL WHERE id = ?1",
            "harvested_on" => "UPDATE trays SET harvested_on = NULL WHERE id = ?1",
            "discarded_on" => "UPDATE trays SET discarded_on = NULL WHERE id = ?1",
            "actual_yield_oz" => "UPDATE trays SET actual_yield_oz = NULL WHERE id = ?1",
            "planned_on" => "UPDATE trays SET planned_on = NULL WHERE id = ?1",
            other => return Err(format!("refusing to clear unknown column: {other}")),
        };
        tx.execute(sql, [tray_id]).map_err(|e| e.to_string())?;
    }
    tx.execute(
        "UPDATE trays SET updated_at = ?1 WHERE id = ?2",
        params![updated_at, tray_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Shift every non-null `*_on` date column on a tray by `days` (negative = back).
pub fn shift_tray_dates(
    tx: &Transaction<'_>,
    tray_id: &str,
    days: i64,
    updated_at: &str,
) -> Result<(), String> {
    let cols = [
        "planned_on",
        "sown_on",
        "blackout_on",
        "light_on",
        "harvested_on",
        "discarded_on",
    ];
    let modifier = if days >= 0 {
        format!("+{days} days")
    } else {
        format!("{days} days")
    };
    for col in cols {
        let sql = format!(
            "UPDATE trays SET {col} = date({col}, ?1), updated_at = ?2
             WHERE id = ?3 AND {col} IS NOT NULL"
        );
        tx.execute(&sql, params![modifier, updated_at, tray_id])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn date_col_for_state(state: &str) -> Option<&'static str> {
    match state {
        "planned" => Some("planned_on"),
        "sown" => Some("sown_on"),
        "blackout" => Some("blackout_on"),
        "light" => Some("light_on"),
        "harvested" => Some("harvested_on"),
        "discarded" => Some("discarded_on"),
        _ => None,
    }
}

pub fn inverse_set_state(tray_id: &str, from: &str, clear_cols: &[&str]) -> Value {
    json!({
        "op": "set_tray_state",
        "trayId": tray_id,
        "state": from,
        "clear": clear_cols,
    })
}
