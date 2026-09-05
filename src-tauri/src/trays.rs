use crate::attention;
use crate::db;
use crate::events::{self, EventRecord, Kind};
use crate::models::{
    CapacityRow, Crop, HarvestGroup, HarvestInput, HarvestSummary, MoveToLight, NextEvent,
    RecountCrop, RecountCropChange, RecountEntry, RecountResult, ShelfCapacity, TodayView,
    TrayView, UndoResult,
};
use crate::projection;
use chrono::{Duration, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
#[cfg(test)]
use uuid::Uuid;

const TRAY_VIEW_SELECT: &str = r#"
SELECT
  t.id,
  t.crop_id,
  c.name AS crop_name,
  t.state,
  t.quantity,
  t.growth_days_at_sow,
  t.blackout_days_at_sow,
  t.planned_on,
  t.sown_on,
  t.blackout_on,
  t.light_on,
  t.harvested_on,
  t.discarded_on,
  t.actual_yield_oz,
  CASE WHEN t.sown_on IS NOT NULL AND t.growth_days_at_sow IS NOT NULL
    THEN date(t.sown_on, '+' || t.growth_days_at_sow || ' days')
    ELSE NULL END AS expected_harvest_date,
  CASE WHEN t.sown_on IS NOT NULL AND t.blackout_days_at_sow IS NOT NULL
    THEN date(t.sown_on, '+' || t.blackout_days_at_sow || ' days')
    ELSE NULL END AS cover_check_date,
  t.created_at,
  t.updated_at
FROM trays t
JOIN crops c ON c.id = t.crop_id
"#;

fn map_tray_view(row: &Row<'_>) -> Result<TrayView, rusqlite::Error> {
    Ok(TrayView {
        id: row.get(0)?,
        crop_id: row.get(1)?,
        crop_name: row.get(2)?,
        state: row.get(3)?,
        quantity: row.get(4)?,
        growth_days_at_sow: row.get(5)?,
        blackout_days_at_sow: row.get(6)?,
        planned_on: row.get(7)?,
        sown_on: row.get(8)?,
        blackout_on: row.get(9)?,
        light_on: row.get(10)?,
        harvested_on: row.get(11)?,
        discarded_on: row.get(12)?,
        actual_yield_oz: row.get(13)?,
        expected_harvest_date: row.get(14)?,
        cover_check_date: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

pub fn list_crops(conn: &Connection) -> Result<Vec<Crop>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, growth_days, blackout_days, expected_yield_oz, sort_order,
                    seed_rate_oz_per_tray
             FROM crops ORDER BY sort_order ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok(Crop {
                id: row.get(0)?,
                name: row.get(1)?,
                growth_days: row.get(2)?,
                blackout_days: row.get(3)?,
                expected_yield_oz: row.get(4)?,
                sort_order: row.get(5)?,
                seed_rate_oz_per_tray: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Operator sets or clears `crops.seed_rate_oz_per_tray`. Reference table only —
/// no event, no origin guard, no flush.
pub fn update_crop_seed_rate(
    conn: &Connection,
    crop_id: &str,
    seed_rate_oz_per_tray: Option<f64>,
) -> Result<Crop, String> {
    if let Some(v) = seed_rate_oz_per_tray {
        if !v.is_finite() {
            return Err("seed_rate_oz_per_tray must be a finite number".into());
        }
        if v <= 0.0 {
            return Err("seed_rate_oz_per_tray must be > 0".into());
        }
    }

    let n = conn
        .execute(
            "UPDATE crops SET seed_rate_oz_per_tray = ?1 WHERE id = ?2",
            rusqlite::params![seed_rate_oz_per_tray, crop_id],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("unknown crop_id: {crop_id}"));
    }

    list_crops(conn)?
        .into_iter()
        .find(|c| c.id == crop_id)
        .ok_or_else(|| format!("unknown crop_id: {crop_id}"))
}

fn crop_id_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn validate_crop_days(growth_days: i64, blackout_days: i64) -> Result<(), String> {
    if growth_days < 1 {
        return Err("growth_days must be >= 1".into());
    }
    if blackout_days < 0 {
        return Err("blackout_days must be >= 0".into());
    }
    if blackout_days > growth_days {
        return Err("blackout days cannot outlast growth days".into());
    }
    Ok(())
}

fn next_crop_id(conn: &Connection, name: &str) -> Result<String, String> {
    let base = crop_id_slug(name);
    if base.is_empty() {
        return Err("crop name must contain a letter or number".into());
    }
    let mut id = base.clone();
    let mut n = 2i64;
    loop {
        let taken: i64 = conn
            .query_row("SELECT COUNT(*) FROM crops WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        if taken == 0 {
            return Ok(id);
        }
        id = format!("{base}-{n}");
        n += 1;
    }
}

/// Operator adds a crop to the library. Reference table only — no event,
/// no origin guard, no flush.
pub fn add_crop(
    conn: &Connection,
    name: &str,
    growth_days: i64,
    blackout_days: i64,
    expected_yield_oz: f64,
) -> Result<Crop, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("crop name must not be blank".into());
    }
    validate_crop_days(growth_days, blackout_days)?;
    if !expected_yield_oz.is_finite() {
        return Err("expected_yield_oz must be a finite number".into());
    }
    if expected_yield_oz <= 0.0 {
        return Err("expected_yield_oz must be > 0".into());
    }
    let taken: i64 = conn
        .query_row("SELECT COUNT(*) FROM crops WHERE name = ?1", [name], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if taken > 0 {
        return Err(format!("a crop is already named \"{name}\""));
    }
    // A live name always outranks a former one. If this name was some crop's
    // old name, that alias dies here rather than quietly out-resolving the
    // crop now carrying it.
    conn.execute("DELETE FROM crop_aliases WHERE former_name = ?1", [name])
        .map_err(|e| e.to_string())?;
    let id = next_crop_id(conn, name)?;
    let sort_order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM crops",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO crops
         (id, name, growth_days, blackout_days, expected_yield_oz, sort_order,
          seed_rate_oz_per_tray)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        params![
            id,
            name,
            growth_days,
            blackout_days,
            expected_yield_oz,
            sort_order
        ],
    )
    .map_err(|e| e.to_string())?;
    list_crops(conn)?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| format!("unknown crop_id: {id}"))
}

/// Existing trays are untouched by construction: they carry
/// growth_days_at_sow / blackout_days_at_sow and the capacity formula
/// reads those. This edit moves planning only — reachability, sow-by, and
/// what a tray sown from now on will do.
pub fn update_crop_growth_days(
    conn: &Connection,
    crop_id: &str,
    growth_days: i64,
    blackout_days: i64,
) -> Result<Crop, String> {
    validate_crop_days(growth_days, blackout_days)?;
    let n = conn
        .execute(
            "UPDATE crops SET growth_days = ?1, blackout_days = ?2 WHERE id = ?3",
            params![growth_days, blackout_days, crop_id],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("unknown crop_id: {crop_id}"));
    }
    list_crops(conn)?
        .into_iter()
        .find(|c| c.id == crop_id)
        .ok_or_else(|| format!("unknown crop_id: {crop_id}"))
}

/// Operator renames a crop. Reference table only — no event, no origin
/// guard, no flush. The alias and the name move in one transaction.
pub fn rename_crop(conn: &mut Connection, crop_id: &str, new_name: &str) -> Result<Crop, String> {
    let name = new_name.trim();
    if name.is_empty() {
        return Err("crop name must not be blank".into());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let old: String = tx
        .query_row("SELECT name FROM crops WHERE id = ?1", [crop_id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("unknown crop_id: {crop_id}"))?;
    if old == name {
        drop(tx);
        return list_crops(conn)?
            .into_iter()
            .find(|c| c.id == crop_id)
            .ok_or_else(|| format!("unknown crop_id: {crop_id}"));
    }
    let taken: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM crops WHERE name = ?1 AND id <> ?2",
            params![name, crop_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if taken > 0 {
        return Err(format!("another crop is already named \"{name}\""));
    }
    // A live name always outranks a former one. If this name was some crop's
    // old name, that alias dies here rather than quietly out-resolving the
    // crop now carrying it.
    tx.execute("DELETE FROM crop_aliases WHERE former_name = ?1", [name])
        .map_err(|e| e.to_string())?;
    // The crop keeps every standing target recorded under its old name.
    tx.execute(
        "INSERT OR REPLACE INTO crop_aliases (former_name, crop_id, created_at)
         VALUES (?1, ?2, ?3)",
        params![old, crop_id, db::utc_now_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE crops SET name = ?1 WHERE id = ?2",
        params![name, crop_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    list_crops(conn)?
        .into_iter()
        .find(|c| c.id == crop_id)
        .ok_or_else(|| format!("unknown crop_id: {crop_id}"))
}

pub fn list_trays(conn: &Connection) -> Result<Vec<TrayView>, String> {
    let sql = format!(
        "{TRAY_VIEW_SELECT}
         WHERE t.state <> 'discarded'
         ORDER BY t.created_at ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], map_tray_view)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn get_tray(conn: &Connection, tray_id: &str) -> Result<TrayView, String> {
    let sql = format!("{TRAY_VIEW_SELECT} WHERE t.id = ?1");
    conn.query_row(&sql, [tray_id], map_tray_view)
        .map_err(|e| e.to_string())
}

fn next_state(from: &str) -> Result<&'static str, String> {
    match from {
        "planned" => Ok("sown"),
        "sown" => Ok("blackout"),
        "blackout" => Ok("light"),
        // A harvest is a weight and a consumption record, written by
        // harvest_groups_in_tx. Advancing out of light produced a harvested row
        // with NULL yield and no consumption — reachable from Today by a single
        // double-tap on "Move to light". The door is the harvest door.
        "light" => Err(
            "Those trays are already under light. A harvest is recorded with its \
             weight on Today, never by moving."
                .to_string(),
        ),
        "harvested" | "discarded" => Err(format!("cannot advance from terminal state '{from}'")),
        other => Err(format!("unknown state '{other}'")),
    }
}

/// Build an originated event. Tier comes from `Kind` — this helper never
/// chooses domain/class and never reads the clock.
#[allow(clippy::too_many_arguments)] // H-7 Class C: the arg list mirrors the table's column list. Narrowing is H-7b (StorefrontReading precedent).
fn build_grow_event(
    kind: Kind,
    entity_type: &str,
    entity_id: &str,
    payload: Value,
    inverse: Value,
    undoes_seq: Option<i64>,
    reverses_event_id: Option<&str>,
    created_at: String,
) -> EventRecord {
    EventRecord::originated(
        kind,
        entity_type,
        entity_id,
        payload,
        inverse,
        created_at,
        undoes_seq,
        reverses_event_id,
        Some(projection::handler_new_id()),
    )
}

// --- tray.sown --------------------------------------------------------------

/// Sow without a seed-weight consumption record (blank seed field).
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn sow_tray(conn: &mut Connection, crop_id: &str, quantity: i64) -> Result<TrayView, String> {
    sow_tray_with_seed(conn, crop_id, quantity, None)
}

/// Sow and, in the same transaction, emit physical-consumption records:
/// always trays; seed oz only when `seed_oz` is Some (operator-entered > 0).
pub fn sow_tray_with_seed(
    conn: &mut Connection,
    crop_id: &str,
    quantity: i64,
    seed_oz: Option<f64>,
) -> Result<TrayView, String> {
    if quantity < 1 {
        return Err("quantity must be >= 1".to_string());
    }
    if let Some(oz) = seed_oz {
        if !oz.is_finite() || oz <= 0.0 {
            return Err("Seed weight must be greater than zero.".into());
        }
    }
    // Early existence check for a clean error before opening the transaction.
    // apply_tray_sown re-derives growth/blackout days from the same crops join.
    let _ = db::get_crop_growth_blackout(conn, crop_id)?;
    let crop_name: String = conn
        .query_row("SELECT name FROM crops WHERE id = ?1", [crop_id], |r| {
            r.get(0)
        })
        .map_err(|_| format!("unknown crop: {crop_id}"))?;
    // TODO(stage-3:self-correcting-estimates): snapshots freeze the crop defaults at sow time
    // so later corrections to crop day counts cannot rewrite trays already on the shelf.

    let tray_id = projection::handler_new_id();
    let now = projection::handler_now();
    let today = db::local_date_from_utc_rfc3339(&now)?;

    // Sowing and covering are one motion on a real bench — land in blackout.
    let payload = json!({
        "cropId": crop_id,
        "quantity": quantity,
        "sownOn": today,
        "blackoutOn": today,
    });
    let inverse = json!({
        "op": "delete_tray",
        "trayId": tray_id,
    });
    let event = build_grow_event(
        Kind::TraySown,
        "tray",
        &tray_id,
        payload,
        inverse,
        None,
        None,
        now.clone(),
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;

    // Physical consumption — same transaction (Track 3 shape). Tray always;
    // seed only when the operator entered a positive weight.
    crate::consumption::insert_consumption_in_tx(
        &tx,
        crate::consumption::RecordConsumptionInput {
            variety_or_item: crate::consumption::TRAY_VARIETY_OR_ITEM.to_string(),
            unit: crate::consumption::UNIT_TRAY.to_string(),
            quantity: quantity as f64,
            occurred_at: now.clone(),
            sow_event_id: Some(event.event_id.clone()),
            linked_cost_event_id: None,
            notes: None,
        },
    )?;
    if let Some(oz) = seed_oz {
        // variety_or_item: crop name (identifier is crop_id on tray.sown).
        crate::consumption::insert_consumption_in_tx(
            &tx,
            crate::consumption::RecordConsumptionInput {
                variety_or_item: crop_name,
                unit: crate::consumption::UNIT_OZ.to_string(),
                quantity: oz,
                occurred_at: now,
                sow_event_id: Some(event.event_id.clone()),
                linked_cost_event_id: None,
                notes: None,
            },
        )?;
    }

    tx.commit().map_err(|e| e.to_string())?;

    get_tray(conn, &tray_id)
}

/// Projection: INSERT the tray from `entity_id` + payload. Growth/blackout days
/// come from a live crops join (Ruling 2 category b — declared exclusion).
pub(crate) fn apply_tray_sown(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    let tray_id = &event.entity_id;
    let crop_id = event
        .payload
        .get("cropId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "tray.sown payload missing cropId".to_string())?;
    let quantity = event
        .payload
        .get("quantity")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "tray.sown payload missing quantity".to_string())?;
    let sown_on = event
        .payload
        .get("sownOn")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "tray.sown payload missing sownOn".to_string())?;
    let blackout_on = event
        .payload
        .get("blackoutOn")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "tray.sown payload missing blackoutOn".to_string())?;

    let (growth_days, blackout_days): (i64, i64) = tx
        .query_row(
            "SELECT growth_days, blackout_days FROM crops WHERE id = ?1",
            [crop_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;

    tx.execute(
        "INSERT INTO trays (
            id, crop_id, state, quantity,
            growth_days_at_sow, blackout_days_at_sow,
            planned_on, sown_on, blackout_on, light_on, harvested_on, discarded_on,
            actual_yield_oz, created_at, updated_at
         ) VALUES (
            ?1, ?2, 'blackout', ?3,
            ?4, ?5,
            NULL, ?6, ?7, NULL, NULL, NULL,
            NULL, ?8, ?8
         )",
        params![
            tray_id,
            crop_id,
            quantity,
            growth_days,
            blackout_days,
            sown_on,
            blackout_on,
            event.created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// --- trays.advanced ----------------------------------------------------------

struct AdvancePlan {
    tray_id: String,
    from: String,
    to: String,
}

fn plan_advance(conn: &Connection, tray_id: &str) -> Result<AdvancePlan, String> {
    let current = get_tray(conn, tray_id)?;
    let to = next_state(&current.state)?;
    // Validate a date column exists for the target state before committing to the plan.
    events::date_col_for_state(to).ok_or_else(|| format!("no date column for state '{to}'"))?;
    Ok(AdvancePlan {
        tray_id: tray_id.to_string(),
        from: current.state,
        to: to.to_string(),
    })
}

pub fn advance_trays(conn: &mut Connection, tray_ids: &[String]) -> Result<(), String> {
    if tray_ids.is_empty() {
        return Ok(());
    }

    let now = projection::handler_now();
    let today = db::local_date_from_utc_rfc3339(&now)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    advance_trays_in_tx(&tx, tray_ids, &today, &now)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// The one advance mutator. `on` is the physical date stamped on every tray
/// (Ruling 7: `on` is the stamped date): the public path passes today, the
/// phone Confirm gate passes the capture day (rack-side fence 1, ruling 3).
/// Validation is unchanged: every transition is planned before anything is
/// written. Returns the trays.advanced event id.
pub(crate) fn advance_trays_in_tx(
    tx: &Transaction<'_>,
    tray_ids: &[String],
    on: &str,
    created_at: &str,
) -> Result<String, String> {
    if tray_ids.is_empty() {
        return Err("advance_trays requires at least one tray".into());
    }

    // Validate every transition before mutating anything.
    let mut plans = Vec::with_capacity(tray_ids.len());
    for id in tray_ids {
        plans.push(plan_advance(tx, id)?);
    }

    if let Some(bad) = plans.iter().find(|p| p.to == "harvested") {
        return Err(format!(
            "advance refuses to harvest tray {} — the harvest door writes the weight",
            bad.tray_id
        ));
    }

    let mut payload_trays = Vec::new();
    let mut inverse_trays = Vec::new();
    for plan in &plans {
        let date_col = events::date_col_for_state(&plan.to)
            .ok_or_else(|| format!("no date column for state '{}'", plan.to))?;
        payload_trays.push(json!({
            "trayId": plan.tray_id,
            "from": plan.from,
            "to": plan.to,
            "on": on,
        }));
        inverse_trays.push(json!({
            "trayId": plan.tray_id,
            "state": plan.from,
            "clear": [date_col],
        }));
    }

    let entity_id = plans[0].tray_id.clone();
    let payload = json!({ "trays": payload_trays });
    let inverse = json!({
        "op": "set_trays_state",
        "trays": inverse_trays,
    });
    let event = build_grow_event(
        Kind::TraysAdvanced,
        "tray",
        &entity_id,
        payload,
        inverse,
        None,
        None,
        created_at.to_string(),
    );

    projection::apply_event(tx, &event)?;
    events::insert_event(tx, &event)?;
    Ok(event.event_id.clone())
}

pub fn advance_tray(conn: &mut Connection, tray_id: &str) -> Result<TrayView, String> {
    advance_trays(conn, &[tray_id.to_string()])?;
    get_tray(conn, tray_id)
}

/// Projection: `payload.trays[].{trayId,to,on}` — `on` is the stamped date (Ruling 7).
pub(crate) fn apply_trays_advanced(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    let trays = event
        .payload
        .get("trays")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "trays.advanced payload missing trays".to_string())?;
    for t in trays {
        let tray_id = t
            .get("trayId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "trays.advanced item missing trayId".to_string())?;
        let to = t
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "trays.advanced item missing to".to_string())?;
        let on = t
            .get("on")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "trays.advanced item missing on".to_string())?;
        let date_col = events::date_col_for_state(to)
            .ok_or_else(|| format!("no date column for state '{to}'"))?;
        let sql =
            format!("UPDATE trays SET state = ?1, {date_col} = ?2, updated_at = ?3 WHERE id = ?4");
        let n = tx
            .execute(&sql, params![to, on, event.created_at, tray_id])
            .map_err(|e| e.to_string())?;
        if n != 1 {
            return Err(format!("tray not found: {tray_id}"));
        }
    }
    Ok(())
}

// --- trays.harvested ---------------------------------------------------------

pub fn harvest_groups(conn: &mut Connection, groups: &[HarvestInput]) -> Result<(), String> {
    let now = projection::handler_now();
    let today = db::local_date_from_utc_rfc3339(&now)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    harvest_groups_in_tx(&tx, groups, &today, &now, &now)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// The one harvest mutator. `harvested_on` is the physical date stamped on
/// every tray and `occurred_at` the physical moment on the consumption rows:
/// the public path passes today / now, the phone Confirm gate passes the
/// capture day / phone_captured_at (rack-side fence 1, ruling 3). Validation
/// is unchanged: every tray in every group is checked before anything is
/// written. Returns the trays.harvested event id.
pub(crate) fn harvest_groups_in_tx(
    tx: &Transaction<'_>,
    groups: &[HarvestInput],
    harvested_on: &str,
    occurred_at: &str,
    created_at: &str,
) -> Result<String, String> {
    if groups.is_empty() {
        return Err("harvest_groups requires at least one group".to_string());
    }

    // Validate every tray in every group before mutating anything.
    struct PlannedTray {
        id: String,
        share: f64,
        crop_name: String,
        quantity: i64,
        sow_event_id: Option<String>,
    }
    struct PlannedGroup {
        tray_ids: Vec<String>,
        actual_yield_oz: f64,
        trays: Vec<PlannedTray>,
    }

    let mut planned: Vec<PlannedGroup> = Vec::with_capacity(groups.len());
    for g in groups {
        if g.tray_ids.is_empty() {
            return Err("harvest group requires at least one tray".to_string());
        }
        if g.actual_yield_oz <= 0.0 {
            return Err("actual_yield_oz must be > 0".to_string());
        }

        let mut trays = Vec::with_capacity(g.tray_ids.len());
        let mut total_qty: i64 = 0;
        for id in &g.tray_ids {
            let t = get_tray(tx, id)?;
            if t.state != "light" {
                return Err(format!(
                    "cannot harvest from state '{}'; expected 'light'",
                    t.state
                ));
            }
            let sow_event_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM event_log
                     WHERE kind = 'tray.sown' AND entity_id = ?1
                     ORDER BY seq ASC LIMIT 1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            total_qty += t.quantity;
            trays.push((t.id, t.quantity, t.crop_name, sow_event_id));
        }
        if total_qty < 1 {
            return Err("total quantity must be >= 1".to_string());
        }

        let mut assigned = 0.0;
        let mut planned_trays = Vec::with_capacity(trays.len());
        for (i, (id, qty, crop_name, sow_event_id)) in trays.iter().enumerate() {
            let share = if i + 1 == trays.len() {
                (g.actual_yield_oz - assigned).max(0.0)
            } else {
                let s =
                    ((g.actual_yield_oz * *qty as f64) / total_qty as f64 * 10.0).round() / 10.0;
                assigned += s;
                s
            };
            planned_trays.push(PlannedTray {
                id: id.clone(),
                share,
                crop_name: crop_name.clone(),
                quantity: *qty,
                sow_event_id: sow_event_id.clone(),
            });
        }
        planned.push(PlannedGroup {
            tray_ids: g.tray_ids.clone(),
            actual_yield_oz: g.actual_yield_oz,
            trays: planned_trays,
        });
    }

    let mut payload_groups = Vec::new();
    let mut inverse_trays = Vec::new();
    let entity_id = planned[0].trays[0].id.clone();

    for g in &planned {
        let mut payload_trays = Vec::new();
        for t in &g.trays {
            payload_trays.push(json!({
                "trayId": t.id,
                "from": "light",
                "to": "harvested",
                "actualYieldOz": t.share,
                "harvestedOn": harvested_on,
            }));
            inverse_trays.push(json!({
                "trayId": t.id,
                "state": "light",
                "clear": ["harvested_on", "actual_yield_oz"],
            }));
        }
        payload_groups.push(json!({
            "trayIds": g.tray_ids,
            "from": "light",
            "actualYieldOz": g.actual_yield_oz,
            "trays": payload_trays,
        }));
    }

    let payload = json!({ "groups": payload_groups });
    let inverse = json!({
        "op": "set_trays_state",
        "trays": inverse_trays,
    });
    let event = build_grow_event(
        Kind::TraysHarvested,
        "tray",
        &entity_id,
        payload,
        inverse,
        None,
        None,
        created_at.to_string(),
    );

    projection::apply_event(tx, &event)?;
    events::insert_event(tx, &event)?;

    for g in &planned {
        for t in &g.trays {
            crate::consumption::insert_consumption_in_tx(
                tx,
                crate::consumption::RecordConsumptionInput {
                    variety_or_item: t.crop_name.clone(),
                    unit: crate::consumption::UNIT_PLANTING.to_string(),
                    quantity: t.quantity as f64,
                    occurred_at: occurred_at.to_string(),
                    sow_event_id: t.sow_event_id.clone(),
                    linked_cost_event_id: None,
                    notes: None,
                },
            )?;
        }
    }

    Ok(event.event_id.clone())
}

pub fn harvest_trays(
    conn: &mut Connection,
    tray_ids: &[String],
    actual_yield_oz: f64,
) -> Result<(), String> {
    harvest_groups(
        conn,
        &[HarvestInput {
            tray_ids: tray_ids.to_vec(),
            actual_yield_oz,
        }],
    )
}

pub fn harvest_tray(
    conn: &mut Connection,
    tray_id: &str,
    actual_yield_oz: f64,
) -> Result<TrayView, String> {
    harvest_trays(conn, &[tray_id.to_string()], actual_yield_oz)?;
    get_tray(conn, tray_id)
}

/// Projection: `payload.groups[].trays[].{trayId,actualYieldOz,harvestedOn}`.
pub(crate) fn apply_trays_harvested(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    let groups = event
        .payload
        .get("groups")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "trays.harvested payload missing groups".to_string())?;
    for g in groups {
        let trays = g
            .get("trays")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "trays.harvested group missing trays".to_string())?;
        for t in trays {
            let tray_id = t
                .get("trayId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "trays.harvested item missing trayId".to_string())?;
            let actual_yield_oz = t
                .get("actualYieldOz")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "trays.harvested item missing actualYieldOz".to_string())?;
            let harvested_on = t
                .get("harvestedOn")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "trays.harvested item missing harvestedOn".to_string())?;
            let n = tx
                .execute(
                    "UPDATE trays SET state = 'harvested', harvested_on = ?1,
                     actual_yield_oz = ?2, updated_at = ?3 WHERE id = ?4",
                    params![harvested_on, actual_yield_oz, event.created_at, tray_id],
                )
                .map_err(|e| e.to_string())?;
            if n != 1 {
                return Err(format!("tray not found: {tray_id}"));
            }
        }
    }
    Ok(())
}

// --- tray.discarded / trays.discarded ----------------------------------------

pub fn discard_tray(conn: &mut Connection, tray_id: &str) -> Result<TrayView, String> {
    let current = get_tray(conn, tray_id)?;
    if current.state == "discarded" {
        return Err("tray is already discarded".to_string());
    }
    let from = current.state.clone();
    let now = projection::handler_now();
    let today = db::local_date_from_utc_rfc3339(&now)?;

    let payload = json!({ "from": from, "discardedOn": today });
    let inverse = events::inverse_set_state(tray_id, &from, &["discarded_on"]);
    let event = build_grow_event(
        Kind::TrayDiscarded,
        "tray",
        tray_id,
        payload,
        inverse,
        None,
        None,
        now,
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;

    get_tray(conn, tray_id)
}

/// Projection: `entity_id` is the tray; `payload.{from,discardedOn}`.
pub(crate) fn apply_tray_discarded(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    let tray_id = &event.entity_id;
    let discarded_on = event
        .payload
        .get("discardedOn")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "tray.discarded payload missing discardedOn".to_string())?;
    let n = tx
        .execute(
            "UPDATE trays SET state = 'discarded', discarded_on = ?1, updated_at = ?2 WHERE id = ?3",
            params![discarded_on, event.created_at, tray_id],
        )
        .map_err(|e| e.to_string())?;
    if n != 1 {
        return Err(format!("tray not found: {tray_id}"));
    }
    Ok(())
}

/// Greedy consume-`quantity`-physical-trays plan (oldest `sown_on`, then id).
/// Pure planning: does not touch the DB. New split-tray ids come from
/// `projection::handler_new_id` (handler-side; never called from apply_*).
/// Items carry `discardedOn` (Ruling 7); inverse items restore each row to its
/// prior state (full) or unsplit (partial).
fn plan_consume(
    rows: &[TrayView],
    quantity: i64,
    today: &str,
) -> Result<(Vec<Value>, Vec<Value>), String> {
    let mut remaining = quantity;
    let mut payload_items = Vec::new();
    let mut inverse_items = Vec::new();

    for row in rows {
        if remaining == 0 {
            break;
        }
        if row.quantity <= remaining {
            remaining -= row.quantity;
            payload_items.push(json!({
                "trayId": row.id,
                "consumed": row.quantity,
                "mode": "full",
                "from": row.state,
                "discardedOn": today,
            }));
            inverse_items.push(json!({
                "op": "set_tray_state",
                "trayId": row.id,
                "state": row.state,
                "clear": ["discarded_on"],
            }));
        } else {
            let consumed = remaining;
            let new_id = projection::handler_new_id();
            remaining = 0;
            payload_items.push(json!({
                "trayId": row.id,
                "consumed": consumed,
                "mode": "split",
                "newTrayId": new_id,
                "from": row.state,
                "discardedOn": today,
            }));
            inverse_items.push(json!({
                "op": "unsplit",
                "sourceTrayId": row.id,
                "restoreQuantity": row.quantity,
                "deleteTrayId": new_id,
            }));
        }
    }

    if remaining != 0 {
        return Err(format!(
            "could not consume full quantity; {remaining} left unmatched"
        ));
    }
    Ok((payload_items, inverse_items))
}

/// Apply one planned discard item (`mode` = `"full"` or `"split"`) purely from
/// its JSON fields plus a read of the source tray's own current columns for
/// the split copy — no clock, no random ids. Shared by `apply_trays_discarded`
/// and the shortfall half of `apply_recount_applied`.
fn apply_discard_item(tx: &Transaction<'_>, item: &Value, updated_at: &str) -> Result<(), String> {
    let tray_id = item
        .get("trayId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "discard item missing trayId".to_string())?;
    let mode = item
        .get("mode")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "discard item missing mode".to_string())?;
    let discarded_on = item
        .get("discardedOn")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "discard item missing discardedOn".to_string())?;

    match mode {
        "full" => {
            let n = tx
                .execute(
                    "UPDATE trays SET state = 'discarded', discarded_on = ?1, updated_at = ?2 WHERE id = ?3",
                    params![discarded_on, updated_at, tray_id],
                )
                .map_err(|e| e.to_string())?;
            if n != 1 {
                return Err(format!("tray not found: {tray_id}"));
            }
            Ok(())
        }
        "split" => {
            let consumed = item
                .get("consumed")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "split item missing consumed".to_string())?;
            let new_id = item
                .get("newTrayId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "split item missing newTrayId".to_string())?;

            #[allow(clippy::type_complexity)]
            // H-7 Class E: a query_row tuple, not a signature. Nothing here to narrow.
            let (
                crop_id,
                src_qty,
                growth_days,
                blackout_days,
                planned_on,
                sown_on,
                blackout_on,
                light_on,
                harvested_on,
            ): (
                String,
                i64,
                Option<i64>,
                Option<i64>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            ) = tx
                .query_row(
                    "SELECT crop_id, quantity, growth_days_at_sow, blackout_days_at_sow,
                            planned_on, sown_on, blackout_on, light_on, harvested_on
                     FROM trays WHERE id = ?1",
                    [tray_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                        ))
                    },
                )
                .map_err(|e| e.to_string())?;

            let new_qty = src_qty - consumed;
            let n = tx
                .execute(
                    "UPDATE trays SET quantity = ?1, updated_at = ?2 WHERE id = ?3",
                    params![new_qty, updated_at, tray_id],
                )
                .map_err(|e| e.to_string())?;
            if n != 1 {
                return Err(format!("tray not found: {tray_id}"));
            }

            tx.execute(
                "INSERT INTO trays (
                    id, crop_id, state, quantity,
                    growth_days_at_sow, blackout_days_at_sow,
                    planned_on, sown_on, blackout_on, light_on, harvested_on, discarded_on,
                    actual_yield_oz, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, 'discarded', ?3,
                    ?4, ?5,
                    ?6, ?7, ?8, ?9, ?10, ?11,
                    NULL, ?12, ?12
                 )",
                params![
                    new_id,
                    crop_id,
                    consumed,
                    growth_days,
                    blackout_days,
                    planned_on,
                    sown_on,
                    blackout_on,
                    light_on,
                    harvested_on,
                    discarded_on,
                    updated_at,
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        }
        other => Err(format!("unknown discard item mode: {other}")),
    }
}

/// Discard `quantity` physical trays from the named rows (greedy: oldest sown_on, then id).
/// One transaction, one `trays.discarded` event, one inverse. Returns the recomputed
/// harvest group for remaining light trays, or `None` when the group is empty.
pub fn discard_from_group(
    conn: &mut Connection,
    tray_ids: &[String],
    quantity: i64,
) -> Result<Option<HarvestGroup>, String> {
    if tray_ids.is_empty() {
        return Err("discard_from_group requires at least one tray".to_string());
    }
    if quantity < 1 {
        return Err("quantity must be >= 1".to_string());
    }

    let mut rows: Vec<TrayView> = Vec::with_capacity(tray_ids.len());
    for id in tray_ids {
        let t = get_tray(conn, id)?;
        if t.state == "discarded" {
            return Err(format!("tray already discarded: {id}"));
        }
        if t.state != "light" {
            return Err(format!(
                "cannot discard_from_group from state '{}'; expected 'light'",
                t.state
            ));
        }
        rows.push(t);
    }

    let crop_id = rows[0].crop_id.clone();
    let crop_name = rows[0].crop_name.clone();
    for t in &rows {
        if t.crop_id != crop_id {
            return Err("discard_from_group trayIds must share one crop".to_string());
        }
    }

    let total: i64 = rows.iter().map(|t| t.quantity).sum();
    if quantity > total {
        return Err(format!("quantity {quantity} exceeds group total {total}"));
    }

    rows.sort_by(|a, b| a.sown_on.cmp(&b.sown_on).then_with(|| a.id.cmp(&b.id)));

    let now = projection::handler_now();
    let today = db::local_date_from_utc_rfc3339(&now)?;
    let (payload_items, inverse_items) = plan_consume(&rows, quantity, &today)?;

    let entity_id = tray_ids[0].clone();
    let payload = json!({
        "trayIds": tray_ids,
        "quantity": quantity,
        "items": payload_items,
    });
    let inverse = json!({
        "op": "restore_discard",
        "items": inverse_items,
    });
    let event = build_grow_event(
        Kind::TraysDiscarded,
        "tray",
        &entity_id,
        payload,
        inverse,
        None,
        None,
        now,
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;

    // Recompute harvest group from named rows still in light.
    let mut remaining_rows: Vec<TrayView> = Vec::new();
    for id in tray_ids {
        let t = get_tray(conn, id)?;
        if t.state == "light" && t.quantity >= 1 {
            remaining_rows.push(t);
        }
    }
    if remaining_rows.is_empty() {
        return Ok(None);
    }
    remaining_rows.sort_by(|a, b| a.sown_on.cmp(&b.sown_on).then_with(|| a.id.cmp(&b.id)));
    let tray_count: i64 = remaining_rows.iter().map(|t| t.quantity).sum();
    let crops = list_crops(conn)?;
    let ey = crops
        .iter()
        .find(|c| c.id == crop_id)
        .map(|c| c.expected_yield_oz)
        .unwrap_or(0.0);
    Ok(Some(HarvestGroup {
        crop_id,
        crop_name,
        tray_ids: remaining_rows.iter().map(|t| t.id.clone()).collect(),
        tray_count,
        estimated_yield_oz: round1(tray_count as f64 * ey),
    }))
}

/// Projection: `payload.items[]` — each applied via `apply_discard_item`.
pub(crate) fn apply_trays_discarded(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    let items = event
        .payload
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "trays.discarded payload missing items".to_string())?;
    for item in items {
        apply_discard_item(tx, item, &event.created_at)?;
    }
    Ok(())
}

const ACTIVE_STATES: &str = "('planned','sown','blackout','light')";

fn active_rows_for_crop(conn: &Connection, crop_id: &str) -> Result<Vec<TrayView>, String> {
    let sql = format!(
        "{TRAY_VIEW_SELECT}
         WHERE t.crop_id = ?1 AND t.state IN {ACTIVE_STATES}
         ORDER BY t.sown_on ASC, t.id ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([crop_id], map_tray_view)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// One row per crop with active trays, sort_order ascending.
pub fn recount_state(conn: &Connection) -> Result<Vec<RecountCrop>, String> {
    let sql = format!(
        "SELECT c.id, c.name,
                COALESCE(SUM(t.quantity), 0) AS app_quantity
         FROM crops c
         JOIN trays t ON t.crop_id = c.id AND t.state IN {ACTIVE_STATES}
         GROUP BY c.id, c.name, c.sort_order
         HAVING COALESCE(SUM(t.quantity), 0) > 0
         ORDER BY c.sort_order ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mapped = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in mapped {
        let (crop_id, crop_name, app_quantity) = r.map_err(|e| e.to_string())?;
        let rows = active_rows_for_crop(conn, &crop_id)?;
        out.push(RecountCrop {
            crop_id,
            crop_name,
            app_quantity,
            tray_ids: rows.into_iter().map(|t| t.id).collect(),
        });
    }
    Ok(out)
}

// --- recount.applied ----------------------------------------------------------

/// Apply a shelf recount in one transaction with at most one `recount.applied` event.
///
/// Planning (this function) computes shortfall/surplus deltas and builds the
/// payload; `apply_recount_applied` performs the tray SQL purely from that
/// payload. Attention items are collateral, not part of the projected state,
/// so they are raised here in the same transaction — after `apply_event` —
/// reading the already-planned shortfall/surplus quantities, never inside
/// `apply_recount_applied` itself.
pub fn apply_recount(
    conn: &mut Connection,
    entries: &[RecountEntry],
) -> Result<RecountResult, String> {
    // Validate every entry before mutating.
    let crops = list_crops(conn)?;
    let crop_by_id: std::collections::HashMap<&str, &Crop> =
        crops.iter().map(|c| (c.id.as_str(), c)).collect();

    for entry in entries {
        if entry.counted_quantity < 0 {
            return Err("counted quantity cannot be negative".to_string());
        }
        if !crop_by_id.contains_key(entry.crop_id.as_str()) {
            return Err(format!("unknown crop: {}", entry.crop_id));
        }
    }

    let now = projection::handler_now();
    let today = db::local_date_from_utc_rfc3339(&now)?;

    struct ShortfallPlan {
        crop_id: String,
        crop_name: String,
        quantity: i64,
    }
    struct SurplusPlan {
        crop_id: String,
        crop_name: String,
        quantity: i64,
    }

    let mut adjusted_down = Vec::new();
    let mut adjusted_up = Vec::new();
    let mut unchanged: i64 = 0;
    let mut payload_crops = Vec::new();
    let mut inverse_items = Vec::new();
    let mut any_change = false;
    let mut shortfalls: Vec<ShortfallPlan> = Vec::new();
    let mut surpluses: Vec<SurplusPlan> = Vec::new();

    for entry in entries {
        let crop = crop_by_id
            .get(entry.crop_id.as_str())
            .ok_or_else(|| format!("unknown crop: {}", entry.crop_id))?;
        let mut rows = active_rows_for_crop(conn, &entry.crop_id)?;

        let app_quantity: i64 = rows.iter().map(|t| t.quantity).sum();
        let counted = entry.counted_quantity;

        if counted == app_quantity {
            unchanged += 1;
            payload_crops.push(json!({
                "cropId": entry.crop_id,
                "counted": counted,
                "appQuantity": app_quantity,
                "delta": 0,
            }));
            continue;
        }

        any_change = true;

        if counted < app_quantity {
            let shortfall = app_quantity - counted;
            rows.sort_by(|a, b| a.sown_on.cmp(&b.sown_on).then_with(|| a.id.cmp(&b.id)));
            let (payload_items, inv) = plan_consume(&rows, shortfall, &today)?;
            inverse_items.extend(inv);
            shortfalls.push(ShortfallPlan {
                crop_id: crop.id.clone(),
                crop_name: crop.name.clone(),
                quantity: shortfall,
            });
            adjusted_down.push(RecountCropChange {
                crop_id: crop.id.clone(),
                crop_name: crop.name.clone(),
                quantity: shortfall,
            });
            payload_crops.push(json!({
                "cropId": entry.crop_id,
                "counted": counted,
                "appQuantity": app_quantity,
                "delta": -shortfall,
                "items": payload_items,
            }));
        } else {
            let surplus = counted - app_quantity;
            if rows.is_empty() {
                return Err(format!(
                    "cannot add trays for {}: no active row to inherit from",
                    crop.name
                ));
            }
            // Newest active row: sown_on descending, then id descending.
            let template = rows
                .iter()
                .max_by(|a, b| a.sown_on.cmp(&b.sown_on).then_with(|| a.id.cmp(&b.id)))
                .unwrap();

            let new_id = projection::handler_new_id();

            surpluses.push(SurplusPlan {
                crop_id: crop.id.clone(),
                crop_name: crop.name.clone(),
                quantity: surplus,
            });
            adjusted_up.push(RecountCropChange {
                crop_id: crop.id.clone(),
                crop_name: crop.name.clone(),
                quantity: surplus,
            });
            inverse_items.push(json!({
                "op": "delete_tray",
                "trayId": new_id,
            }));
            payload_crops.push(json!({
                "cropId": entry.crop_id,
                "counted": counted,
                "appQuantity": app_quantity,
                "delta": surplus,
                "newTrayId": new_id,
                "inheritedSownOn": template.sown_on,
                "template": {
                    "state": template.state,
                    "sownOn": template.sown_on,
                    "blackoutOn": template.blackout_on,
                    "lightOn": template.light_on,
                },
            }));
        }
    }

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    if any_change {
        let payload = json!({ "crops": payload_crops });
        let inverse = json!({
            "op": "restore_recount",
            "items": inverse_items,
        });
        let event = build_grow_event(
            Kind::RecountApplied,
            "farm",
            "recount",
            payload,
            inverse,
            None,
            None,
            now.clone(),
        );

        projection::apply_event(&tx, &event)?;

        // Attention is collateral, not projected state — raised here, after
        // apply, from the already-planned shortfall/surplus quantities.
        for s in &shortfalls {
            crate::attention::raise_recount_shortfall_in_tx(
                &tx,
                &s.crop_id,
                &s.crop_name,
                s.quantity,
                &now,
            )?;
        }
        for s in &surpluses {
            crate::attention::raise_recount_surplus_in_tx(
                &tx,
                &s.crop_id,
                &s.crop_name,
                s.quantity,
                &now,
            )?;
        }

        events::insert_event(&tx, &event)?;
    }

    tx.commit().map_err(|e| e.to_string())?;

    Ok(RecountResult {
        adjusted_down,
        adjusted_up,
        unchanged,
    })
}

/// Projection: `payload.crops[]` — shortfall via `items[]` (shared with
/// `apply_trays_discarded`), surplus via `newTrayId` + `template` (Ruling 8:
/// growth/blackout days are a live crops join, category b exclusion).
pub(crate) fn apply_recount_applied(
    tx: &Transaction<'_>,
    event: &EventRecord,
) -> Result<(), String> {
    let crops = event
        .payload
        .get("crops")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "recount.applied payload missing crops".to_string())?;

    for c in crops {
        if let Some(items) = c.get("items").and_then(|v| v.as_array()) {
            for item in items {
                apply_discard_item(tx, item, &event.created_at)?;
            }
        }

        if let (Some(new_id), Some(template)) = (
            c.get("newTrayId").and_then(|v| v.as_str()),
            c.get("template"),
        ) {
            let crop_id = c
                .get("cropId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "recount surplus item missing cropId".to_string())?;
            let delta = c
                .get("delta")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "recount surplus item missing delta".to_string())?;
            let state = template
                .get("state")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "recount template missing state".to_string())?;
            let sown_on = template.get("sownOn").and_then(|v| v.as_str());
            let blackout_on = template.get("blackoutOn").and_then(|v| v.as_str());
            let light_on = template.get("lightOn").and_then(|v| v.as_str());

            let (growth_days, blackout_days): (i64, i64) = tx
                .query_row(
                    "SELECT growth_days, blackout_days FROM crops WHERE id = ?1",
                    [crop_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| e.to_string())?;

            tx.execute(
                "INSERT INTO trays (
                    id, crop_id, state, quantity,
                    growth_days_at_sow, blackout_days_at_sow,
                    planned_on, sown_on, blackout_on, light_on, harvested_on, discarded_on,
                    actual_yield_oz, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4,
                    ?5, ?6,
                    NULL, ?7, ?8, ?9, NULL, NULL,
                    NULL, ?10, ?10
                 )",
                params![
                    new_id,
                    crop_id,
                    state,
                    delta,
                    growth_days,
                    blackout_days,
                    sown_on,
                    blackout_on,
                    light_on,
                    event.created_at,
                ],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// --- undo ---------------------------------------------------------------------

/// Undo the newest undoable event.
///
/// `reverses_event_id` is the load-bearing link and `undoes_seq` is kept for
/// the flush guard and the FK — so no future edit drops it thinking it is
/// unused. The import conflict check compares the name, not the ordinal
/// (C3, INT-010).
pub fn undo_last(conn: &mut Connection) -> Result<Option<UndoResult>, String> {
    let target = match events::newest_undoable(conn)? {
        Some(e) => e,
        None => return Ok(None),
    };

    // B7 invariant as code. undo_last is the only writer of undone_at, and it
    // refuses anything that would reverse nothing. newest_undoable already
    // filters these out; this is the second lock, so no future caller can record
    // an undo that changed nothing without deleting this line on purpose.
    if !events::reverses_something(&target.kind, &target.inverse) {
        return Err(format!(
            "{} cannot be undone — nothing would be reversed",
            target.kind
        ));
    }

    let payload = json!({
        "undoesSeq": target.seq,
        "undoneKind": target.kind,
    });
    let inverse = json!({ "op": "none" });
    let now = projection::handler_now();
    let event = build_grow_event(
        Kind::Undo,
        "event",
        &target.seq.to_string(),
        payload,
        inverse,
        Some(target.seq),
        Some(target.id.as_str()),
        now,
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    // Live reopen — id from the resolved event payload (not the inverse).
    // Works for legacy reopen_attention inverses and new {"op":"none"} events.
    if target.kind == "attention.resolved" {
        let payload_s: String = tx
            .query_row(
                "SELECT payload FROM event_log WHERE seq = ?1",
                [target.seq],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let resolved_payload: Value =
            serde_json::from_str(&payload_s).map_err(|e| e.to_string())?;
        let attention_id = resolved_payload
            .get("attentionId")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "attention.resolved payload missing attentionId".to_string())?;
        attention::reopen_attention(&tx, attention_id)?;
    }
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;

    Ok(Some(UndoResult {
        undoes_seq: target.seq,
        undone_kind: target.kind,
    }))
}

/// Ruling 4 (event_file::guard_row) already forces every undo row to name
/// the event it reverses, so the NAME is available everywhere the row is.
/// Resolving by `undoes_seq` once deleted a batch the undo never named:
/// before C3 (INT-010) an imported row kept the source farm's ordinal, and
/// rows imported before C3 still do. The import door now re-derives the
/// column from the name; this handler resolves by the name regardless, so
/// replay reproduces the undo the row actually recorded.
pub(crate) fn apply_undo(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    let target_id = event
        .reverses_event_id
        .as_deref()
        .ok_or_else(|| "undo event does not name the event it reverses (Ruling 4)".to_string())?;
    let (target_seq, inverse_json): (i64, String) = tx
        .query_row(
            "SELECT seq, inverse FROM event_log WHERE id = ?1",
            [target_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("undo names an event this farm does not have: {target_id}"))?;
    events::apply_inverse(tx, &inverse_json, &event.created_at)?;
    // Idempotent, and stamped on the row the NAME resolved to — never on a
    // foreign ordinal.
    tx.execute(
        "UPDATE event_log SET undone_at = ?1 WHERE seq = ?2",
        params![event.created_at, target_seq],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// TODO(stage-3:self-correcting-estimates): replace seeded growth_days with the grower's own harvest history.
/// Computed capacity: one row per (harvest_date, crop) with an exact-date
/// sale figure and a ready-on-or-before cover figure. Never stored.
///
/// D21a — ready-on-or-before is COVER math only. What the retail door
/// publishes keeps exact-date credit, so one tray can never be offered
/// on two dates.
/// D22a — `remaining_trays` is exact-date / sale side; `cover_remaining`
/// is ready-on-or-before / cover side.
///
/// Two reads plus a documented cumulative pass in Rust — the ladder is
/// done in Rust, not SQL, because it must be readable and unit-testable
/// line by line.
pub fn capacity_by_harvest_date(conn: &Connection) -> Result<Vec<CapacityRow>, String> {
    struct BaseRow {
        harvest_date: String,
        crop_id: String,
        crop_name: String,
        trays: i64,
        expected_yield_oz: f64,
        harvested_trays: i64,
        retail_consumed: i64,
        retail_out: i64,
        w_gross: i64,
    }

    let mut stmt = conn
        .prepare(
            r#"
            WITH tray_cap AS (
              SELECT t.crop_id AS crop_id,
                     date(t.sown_on, '+' || t.growth_days_at_sow || ' days') AS harvest_date,
                     SUM(t.quantity) AS trays,
                     SUM(t.quantity * c.expected_yield_oz) AS expected_yield_oz
                FROM trays t JOIN crops c ON c.id = t.crop_id
               WHERE t.state NOT IN ('discarded','harvested')
                 AND t.sown_on IS NOT NULL AND t.growth_days_at_sow IS NOT NULL
               GROUP BY t.crop_id, harvest_date
            ),
            -- Trays grown for this date that have been harvested: proof that
            -- stock left the shelf. Discarded trays are excluded on purpose --
            -- a discarded tray is gone and was never handed to anybody, so it
            -- discharges no promise.
            --
            -- last_harvest_at is `updated_at` on a harvested row. A harvested
            -- tray is terminal, so the last write to that row IS the harvest,
            -- and its event created_at is the instant the trays left. That
            -- invariant is pinned by a test rather than assumed.
            harvested AS (
              SELECT t.crop_id AS crop_id,
                     date(t.sown_on, '+' || t.growth_days_at_sow || ' days') AS harvest_date,
                     SUM(t.quantity) AS trays,
                     MAX(t.updated_at) AS last_harvest_at
                FROM trays t
               WHERE t.state = 'harvested'
                 AND t.sown_on IS NOT NULL AND t.growth_days_at_sow IS NOT NULL
               GROUP BY t.crop_id, harvest_date
            ),
            -- A harvested tray discharges a promise that EXISTED when it left
            -- the shelf. It cannot reach backwards and satisfy a claim taken
            -- afterwards -- those trays were already gone. Both timestamps are
            -- UTC ISO-8601, compared with julianday(), so there is no local-time
            -- conversion here. The comparison is inclusive: a claim paid at the
            -- same instant as the harvest counts as having existed.
            --
            -- If either timestamp will not parse, julianday() is NULL, this CASE
            -- falls to 0, the claim keeps consuming and the oversold signal
            -- stays alive. Ambiguity leaves the alarm on.
            sold AS (
              SELECT o.crop_id AS crop_id, o.harvest_date AS harvest_date,
                     SUM(o.capacity_consumed) AS sold_trays,
                     SUM(CASE WHEN h.last_harvest_at IS NOT NULL
                               AND julianday(o.paid_at) <= julianday(h.last_harvest_at)
                              THEN o.capacity_consumed ELSE 0 END) AS dischargeable
                FROM orders o
                LEFT JOIN harvested h
                       ON h.harvest_date = o.harvest_date AND h.crop_id = o.crop_id
               GROUP BY o.crop_id, o.harvest_date
            ),
            -- D2(b): the reservation retires on harvest evidence, NEVER on delivery.
            -- 'voided' is the only state that ends a promise. This replaces
            -- `state = 'ordered'` and amends GT-D14's delivered-retires rule.
            wsold AS (
              SELECT l.crop_id AS crop_id, o.harvest_date AS harvest_date,
                     SUM(l.trays) AS w_trays
                FROM wholesale_orders o JOIN wholesale_order_lines l ON l.order_id = o.id
               WHERE o.state <> 'voided'
               GROUP BY l.crop_id, o.harvest_date
            ),
            keys AS (
              SELECT crop_id, harvest_date FROM tray_cap
              UNION SELECT crop_id, harvest_date FROM harvested
              UNION SELECT crop_id, harvest_date FROM sold
              UNION SELECT crop_id, harvest_date FROM wsold
            )
            SELECT k.harvest_date, k.crop_id, c.name,
                   COALESCE(t.trays,0), COALESCE(t.expected_yield_oz,0),
                   COALESCE(h.trays,0) AS harvested_trays,
                   -- Retail has no fulfilment state to read (audit L1: states are
                   -- paid/refunded/disputed, zero hits for fulfil). Harvested trays
                   -- are the only physical signal, so the claim retires by the COUNT
                   -- of trays that left, never by their mere existence. A boolean
                   -- here would wipe every claim on the strength of one harvested
                   -- tray and report capacity the farm does not have.
                   --
                   -- Eligibility is per claim and time-ordered (see the sold CTE);
                   -- the total is then capped by the trays actually harvested, so
                   -- neither more claims nor more trays than exist can be discharged.
                   --
                   -- retail consumed on its own date, by the unchanged rule
                   MIN(COALESCE(s.dischargeable,0), COALESCE(h.trays,0)) AS retail_consumed,
                   MAX(0, COALESCE(s.sold_trays,0)
                          - MIN(COALESCE(s.dischargeable,0), COALESCE(h.trays,0))) AS retail_out,
                   COALESCE(w.w_trays,0) AS w_gross
              FROM keys k
              JOIN crops c ON c.id = k.crop_id
              LEFT JOIN tray_cap  t ON t.crop_id=k.crop_id AND t.harvest_date=k.harvest_date
              LEFT JOIN harvested h ON h.crop_id=k.crop_id AND h.harvest_date=k.harvest_date
              LEFT JOIN sold      s ON s.crop_id=k.crop_id AND s.harvest_date=k.harvest_date
              LEFT JOIN wsold     w ON w.crop_id=k.crop_id AND w.harvest_date=k.harvest_date
             ORDER BY k.harvest_date ASC, k.crop_id ASC
            "#,
        )
        .map_err(|e| e.to_string())?;

    let base_rows = stmt
        .query_map([], |row| {
            Ok(BaseRow {
                harvest_date: row.get(0)?,
                crop_id: row.get(1)?,
                crop_name: row.get(2)?,
                trays: row.get(3)?,
                expected_yield_oz: row.get(4)?,
                harvested_trays: row.get(5)?,
                retail_consumed: row.get(6)?,
                retail_out: row.get(7)?,
                w_gross: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut base = Vec::new();
    for r in base_rows {
        base.push(r.map_err(|e| e.to_string())?);
    }

    // D5(1): harvest evidence by the day the trays LEFT the shelf.
    let mut ev_stmt = conn
        .prepare(
            r#"
            SELECT crop_id, harvested_on, SUM(quantity)
              FROM trays
             WHERE state = 'harvested' AND harvested_on IS NOT NULL
             GROUP BY crop_id, harvested_on
             ORDER BY crop_id, harvested_on
            "#,
        )
        .map_err(|e| e.to_string())?;
    let ev_rows = ev_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut evidence: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    for r in ev_rows {
        let (crop_id, harvested_on, trays) = r.map_err(|e| e.to_string())?;
        evidence
            .entry(crop_id)
            .or_default()
            .push((harvested_on, trays));
    }

    // Group base rows by crop so the cover ladder walks each crop in date order.
    // Rows stay the (date, crop) keys the base read produced — a harvest day
    // never invents a row.
    let mut by_crop: BTreeMap<String, Vec<BaseRow>> = BTreeMap::new();
    for row in base {
        by_crop.entry(row.crop_id.clone()).or_default().push(row);
    }

    let mut out = Vec::new();
    for (crop_id, mut rows) in by_crop {
        rows.sort_by(|a, b| a.harvest_date.cmp(&b.harvest_date));
        let harv_days = evidence.get(&crop_id).map(Vec::as_slice).unwrap_or(&[]);
        let mut harv_idx = 0usize;
        let mut running_supply = 0i64;
        let mut running_retail_out = 0i64;
        let mut running_w_gross = 0i64;
        let mut running_retail_consumed = 0i64;
        let mut running_harvested = 0i64;

        for row in rows {
            // Exact-date figures, sale side (D21a). Wholesale takes only the
            // evidence retail did not.
            let evidence_left = (row.harvested_trays - row.retail_consumed).max(0);
            let w_out = (row.w_gross - evidence_left).max(0);
            let sold_trays = row.retail_out + w_out;
            let remaining_trays = row.trays - sold_trays;

            // The cover ladder — one cumulative pass per crop, over that crop's
            // rows in date order.
            //
            // Why each line: supply is cumulative because a tray ready earlier
            // can still serve a later promise (D4(b)); evidence is cumulative
            // and keyed on the harvest day so a batch cut early discharges the
            // promise it was cut for (D5(1)); retail's consumption is subtracted
            // from evidence so no harvested tray discharges twice; a promise
            // stays promised until evidence covers it, delivered or not (D2(b)).
            running_supply += row.trays; // D4(b): ready on or before
            running_retail_out += row.retail_out; // retail keeps its own rule
            running_w_gross += row.w_gross;
            running_retail_consumed += row.retail_consumed;
            // fold in every harvest day for this crop with harvested_on <= this row's
            // date, once, in date order:
            while harv_idx < harv_days.len()
                && harv_days[harv_idx].0.as_str() <= row.harvest_date.as_str()
            {
                running_harvested += harv_days[harv_idx].1;
                harv_idx += 1;
            }

            let cover_evidence = (running_harvested - running_retail_consumed).max(0);
            let cover_wholesale = (running_w_gross - cover_evidence).max(0);
            let cover_promised = cover_wholesale + running_retail_out;
            let cover_remaining = running_supply - cover_promised;

            out.push(CapacityRow {
                harvest_date: row.harvest_date,
                crop_id: row.crop_id,
                crop_name: row.crop_name,
                trays: row.trays,
                expected_yield_oz: row.expected_yield_oz,
                sold_trays,
                remaining_trays,
                harvested_trays: row.harvested_trays,
                cover_supply: running_supply,
                cover_promised,
                cover_remaining,
            });
        }
    }

    out.sort_by(|a, b| {
        a.harvest_date
            .cmp(&b.harvest_date)
            .then(a.crop_id.cmp(&b.crop_id))
    });
    Ok(out)
}

/// SALE-SIDE date rollup: the sum of `remaining_trays` over that date's crop
/// rows. Exact-date credit only (D21a). A cover question must use
/// `cover_shortfall_on`, never this.
///
/// This is a projection of `capacity_by_harvest_date`, not a second query.
/// A date with no row (nothing growing, nothing open against it) is zero, not a
/// special case: after the today-boundary work a settled date legitimately drops
/// out of the result set.
///
/// D21a: the sale side is exact-date PER CROP. This date-wide rollup has no
/// production caller and must not gain one — a crop's surplus netting out
/// another crop's deficit is the double-sell D21a exists to prevent. Kept for
/// the ~25 existing test claims that read it; #[cfg(test)] is the fence.
#[cfg(test)]
pub fn remaining_for_date(conn: &Connection, harvest_date: &str) -> Result<i64, String> {
    Ok(capacity_by_harvest_date(conn)?
        .into_iter()
        .filter(|r| r.harvest_date == harvest_date)
        .map(|r| r.remaining_trays)
        .sum())
}

/// That crop's `remaining_trays` on that exact date; 0 when there is no row.
/// Sale side (D21a). A cover question must use `cover_remaining_for`.
pub fn remaining_exact_for(
    conn: &Connection,
    harvest_date: &str,
    crop_id: &str,
) -> Result<i64, String> {
    Ok(capacity_by_harvest_date(conn)?
        .into_iter()
        .find(|r| r.harvest_date == harvest_date && r.crop_id == crop_id)
        .map(|r| r.remaining_trays)
        .unwrap_or(0))
}

/// Σ over crops of MAX(0, -cover_remaining). THE cover question for a date:
/// a surplus in one crop can never mask a shortfall in another, which is the
/// whole of D3.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn cover_shortfall_on(conn: &Connection, harvest_date: &str) -> Result<i64, String> {
    Ok(capacity_by_harvest_date(conn)?
        .into_iter()
        .filter(|r| r.harvest_date == harvest_date)
        .map(|r| (-r.cover_remaining).max(0))
        .sum())
}

/// That crop's `cover_remaining` on that date; 0 when there is no row.
pub fn cover_remaining_for(
    conn: &Connection,
    harvest_date: &str,
    crop_id: &str,
) -> Result<i64, String> {
    Ok(capacity_by_harvest_date(conn)?
        .into_iter()
        .find(|r| r.harvest_date == harvest_date && r.crop_id == crop_id)
        .map(|r| r.cover_remaining)
        .unwrap_or(0))
}

pub fn shelf_capacity(conn: &Connection) -> Result<ShelfCapacity, String> {
    let row = conn
        .query_row(
            "SELECT light_slots, blackout_slots FROM shelf_capacity WHERE id = 'default'",
            [],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(match row {
        Some((light_slots, blackout_slots)) => ShelfCapacity {
            light_slots,
            blackout_slots,
        },
        None => ShelfCapacity {
            light_slots: None,
            blackout_slots: None,
        },
    })
}

pub fn set_shelf_capacity(
    conn: &Connection,
    light: Option<i64>,
    blackout: Option<i64>,
) -> Result<ShelfCapacity, String> {
    if let Some(n) = light {
        if n < 1 {
            return Err("slots must be at least 1".into());
        }
    }
    if let Some(n) = blackout {
        if n < 1 {
            return Err("slots must be at least 1".into());
        }
    }
    let now = db::utc_now_rfc3339();
    conn.execute(
        "INSERT INTO shelf_capacity (id, light_slots, blackout_slots, updated_at)
         VALUES ('default', ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET
           light_slots = excluded.light_slots,
           blackout_slots = excluded.blackout_slots,
           updated_at = excluded.updated_at",
        rusqlite::params![light, blackout, now],
    )
    .map_err(|e| e.to_string())?;
    shelf_capacity(conn)
}

// Phase 6. This is NOT a second answer to "how many trays are left for
// date D?" -- capacity_by_harvest_date owns that question and is not
// touched. This answers a different one that nothing in the tree asked
// before: "can another tray be put on the shelf for the span it needs?"
//
// Span is fully derived. No column was added for it:
//   blackout: [sown_on, sown_on + blackout_days_at_sow)
//   light:    [sown_on + blackout_days_at_sow, departure]
//   departure = harvested_on | discarded_on | sown_on + growth_days_at_sow
//
// Existing trays use *_at_sow (what they were sown with). A candidate
// sow uses live crops.growth_days / crops.blackout_days (what is true
// now). Same split the rest of the model already keeps.
//
// An overdue tray -- live, past its computed harvest date, not harvested
// -- still occupies its slot from today forward. It is physically on the
// shelf. Assuming it leaves on its estimated date would invent a harvest
// that has not happened. Ambiguity leaves the alarm on, as in the
// formula's own sold-CTE comment.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotUse {
    pub light: i64,
    pub blackout: i64,
}

struct OccupyingTray {
    quantity: i64,
    sown_on: NaiveDate,
    cover_end: NaiveDate,
    departure: NaiveDate,
}

fn parse_day(date: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| format!("invalid date: {date}"))
}

fn add_days(d: NaiveDate, n: i64) -> Result<NaiveDate, String> {
    d.checked_add_signed(Duration::days(n))
        .ok_or_else(|| format!("date out of range: {d} plus {n} days"))
}

fn occupying_trays(conn: &Connection, today: &str) -> Result<Vec<OccupyingTray>, String> {
    let today_d = parse_day(today)?;
    let mut stmt = conn
        .prepare(
            "SELECT t.quantity, t.sown_on, t.blackout_days_at_sow, t.growth_days_at_sow,
                    t.harvested_on, t.state
             FROM trays t
             WHERE t.sown_on IS NOT NULL
               AND t.growth_days_at_sow IS NOT NULL
               AND t.blackout_days_at_sow IS NOT NULL
               AND t.state NOT IN ('discarded')",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let (quantity, sown_on, blackout_days, growth_days, harvested_on, state) =
            row.map_err(|e| e.to_string())?;
        let sown = parse_day(&sown_on)?;
        let cover_end = add_days(sown, blackout_days)?;
        let nominal = add_days(sown, growth_days)?;
        // Live overdue trays have not left. MAX(nominal, today) pins them
        // through today; from today forward they still occupy, so departure
        // is unbounded until a harvest is recorded.
        let departure = if state == "harvested" {
            parse_day(
                harvested_on
                    .as_deref()
                    .ok_or("harvested tray missing harvested_on")?,
            )?
        } else if nominal < today_d {
            NaiveDate::from_ymd_opt(9999, 12, 31).expect("valid date")
        } else {
            nominal
        };
        out.push(OccupyingTray {
            quantity,
            sown_on: sown,
            cover_end,
            departure,
        });
    }
    Ok(out)
}

fn slot_use_on(trays: &[OccupyingTray], day: NaiveDate) -> SlotUse {
    let mut light = 0i64;
    let mut blackout = 0i64;
    for t in trays {
        let in_blackout = t.sown_on <= day && day < t.cover_end && day < t.departure;
        let in_light = t.cover_end <= day && day <= t.departure;
        if in_blackout {
            blackout += t.quantity;
        } else if in_light {
            light += t.quantity;
        }
    }
    SlotUse { light, blackout }
}

#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn occupancy_on(conn: &Connection, day: &str, today: &str) -> Result<SlotUse, String> {
    let d = parse_day(day)?;
    let trays = occupying_trays(conn, today)?;
    Ok(slot_use_on(&trays, d))
}

pub fn slots_free_for_span(
    conn: &Connection,
    sow_on: &str,
    growth_days: i64,
    blackout_days: i64,
    today: &str,
) -> Result<Option<i64>, String> {
    let cap = shelf_capacity(conn)?;
    // Unknown ceiling constrains nothing. None propagates to every caller
    // and every new sentence is suppressed. This is what makes Phase 6
    // dark-safe for a farm that has not entered its numbers.
    let (Some(l), Some(b)) = (cap.light_slots, cap.blackout_slots) else {
        return Ok(None);
    };

    let start = parse_day(sow_on)?;
    let end = add_days(start, growth_days)?;
    let cover_end = add_days(start, blackout_days)?;
    let trays = occupying_trays(conn, today)?;
    let mut uses: HashMap<NaiveDate, SlotUse> = HashMap::new();
    let mut d = start;
    while d <= end {
        uses.insert(d, slot_use_on(&trays, d));
        d = d
            .succ_opt()
            .ok_or_else(|| format!("date out of range: {d}"))?;
    }

    let mut min_head = i64::MAX;
    let mut d = start;
    while d <= end {
        let u = uses.get(&d).expect("span day was indexed");
        let headroom_d = if d < cover_end {
            b - u.blackout
        } else {
            l - u.light
        };
        min_head = min_head.min(headroom_d);
        d = d
            .succ_opt()
            .ok_or_else(|| format!("date out of range: {d}"))?;
    }
    if min_head == i64::MAX {
        min_head = 0;
    }
    Ok(Some(min_head.max(0)))
}

pub fn overdue_trays_on_shelf(conn: &Connection, today: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(SUM(quantity), 0) FROM trays
         WHERE state NOT IN ('harvested', 'discarded')
           AND sown_on IS NOT NULL
           AND growth_days_at_sow IS NOT NULL
           AND date(sown_on, '+' || growth_days_at_sow || ' days') < ?1",
        [today],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// D3: the same question scoped to one crop. A per-(date, crop) card may
/// only ever report overdue trays of ITS crop.
pub fn overdue_trays_on_shelf_for_crop(
    conn: &Connection,
    today: &str,
    crop_id: &str,
) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(SUM(quantity), 0) FROM trays
         WHERE state NOT IN ('harvested', 'discarded')
           AND sown_on IS NOT NULL
           AND growth_days_at_sow IS NOT NULL
           AND date(sown_on, '+' || growth_days_at_sow || ' days') < ?1
           AND crop_id = ?2",
        [today, crop_id],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// R1 — trays under light more than 3 days past their harvest date, grouped by
/// crop. The Today card's own predicate, moved here verbatim from
/// `attention::evaluate_overdue`.
///
/// ONE formula, two consumers: the raise builds `tray.overdue_harvest` from these
/// rows and the resolver closes any open row whose crop no longer appears in
/// them. A resolver that asked a different question could close a card the
/// operator still has work for.
///
/// Deliberately NOT `overdue_trays_on_shelf` below: that reader is F1's and is
/// broader ON PURPOSE — every live tray past its date, any state, from day one.
/// Health may be louder than Today. A resolver may not be: resolving against the
/// broader reader would close cards the card's own predicate still raises, and
/// resolving against a narrower one would strand them.
#[derive(Debug, Clone)]
pub struct OverdueHarvestGroup {
    pub crop_id: String,
    pub crop_name: String,
    pub trays: i64,
    pub days_past: i64,
}

pub fn overdue_harvest_groups(
    conn: &Connection,
    today: &str,
) -> Result<Vec<OverdueHarvestGroup>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name,
                    SUM(t.quantity) AS qty,
                    MAX(CAST(julianday(?1) - julianday(date(t.sown_on, '+' || t.growth_days_at_sow || ' days')) AS INTEGER)) AS days_past
             FROM trays t
             JOIN crops c ON c.id = t.crop_id
             WHERE t.state = 'light'
               AND t.sown_on IS NOT NULL
               AND t.growth_days_at_sow IS NOT NULL
               AND julianday(?1) - julianday(date(t.sown_on, '+' || t.growth_days_at_sow || ' days')) > 3
             GROUP BY c.id, c.name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([today], |row| {
            Ok(OverdueHarvestGroup {
                crop_id: row.get(0)?,
                crop_name: row.get(1)?,
                trays: row.get(2)?,
                days_past: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct OverdueLightGroup {
    pub crop_id: String,
    pub crop_name: String,
    pub trays: i64,
    pub days_past: i64,
}

/// Today's card waits for the third day (unchanged since RB1).
pub const COVER_CARD_DAYS_PAST: i64 = 2;
/// Health F2 counts a tray from the first day PAST its cover-check date.
/// Louder than the card, never quieter — the rule F1 already follows
/// with `< today`. A tray whose cover check is today is due, not past due.
pub const COVER_HEALTH_DAYS_PAST: i64 = 0;

/// RB1 — trays still under cover past their cover-check date, grouped by crop.
///
/// ONE query, two thresholds. `attention::evaluate_overdue` raises
/// `tray.overdue_light` from these rows at `COVER_CARD_DAYS_PAST` and
/// `health::severity_f2` reads their sum at `COVER_HEALTH_DAYS_PAST`, so Health
/// raises two days earlier than the card and can never be quieter than Today
/// about a tray past its cover check.
///
/// Deliberately NOT merged with `overdue_trays_on_shelf`: that reader answers a
/// different question (past the HARVEST date) and drives F1. A blackout tray can
/// be past its cover check and nowhere near its harvest date — that gap is
/// exactly the hole RB1 closes.
pub fn overdue_light_groups(
    conn: &Connection,
    today: &str,
    min_days_past: i64,
) -> Result<Vec<OverdueLightGroup>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name,
                    SUM(t.quantity) AS qty,
                    MAX(CAST(julianday(?1) - julianday(date(t.sown_on, '+' || t.blackout_days_at_sow || ' days')) AS INTEGER)) AS days_past
             FROM trays t
             JOIN crops c ON c.id = t.crop_id
             WHERE t.state = 'blackout'
               AND t.sown_on IS NOT NULL
               AND t.blackout_days_at_sow IS NOT NULL
               AND julianday(?1) - julianday(date(t.sown_on, '+' || t.blackout_days_at_sow || ' days')) > ?2
             GROUP BY c.id, c.name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![today, min_days_past], |row| {
            Ok(OverdueLightGroup {
                crop_id: row.get(0)?,
                crop_name: row.get(1)?,
                trays: row.get(2)?,
                days_past: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// The single number F2 reads. Sum of `overdue_light_groups` at
/// `COVER_HEALTH_DAYS_PAST`.
pub fn overdue_light_trays(conn: &Connection, today: &str) -> Result<i64, String> {
    Ok(overdue_light_groups(conn, today, COVER_HEALTH_DAYS_PAST)?
        .iter()
        .map(|g| g.trays)
        .sum())
}

/// Blackout trays of one crop whose cover check has arrived. The same
/// predicate `today_view` uses for its Move-to-light row
/// (`cover_check_date <= today`), so the overdue card's button and the
/// due row can never disagree about which trays are ready.
pub fn due_for_light_ids_for_crop(
    conn: &Connection,
    crop_id: &str,
    today: &str,
) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id FROM trays
             WHERE crop_id = ?1 AND state = 'blackout'
               AND sown_on IS NOT NULL AND blackout_days_at_sow IS NOT NULL
               AND date(sown_on, '+' || blackout_days_at_sow || ' days') <= ?2
             ORDER BY sown_on ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![crop_id, today], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// D24a: groups of THIS crop that could discharge THIS date's promise if
/// cut early - nominal date on or before the card's date (D4(b)) and not
/// yet due today. A reader. harvest_groups_in_tx stays the only writer.
pub fn early_harvest_groups_for(
    conn: &Connection,
    harvest_date: &str,
    crop_id: &str,
    today: &str,
) -> Result<Vec<HarvestGroup>, String> {
    let trays = list_trays(conn)?;
    let crops = list_crops(conn)?;
    let (crop_name, ey) = crops
        .iter()
        .find(|c| c.id == crop_id)
        .map(|c| (c.name.clone(), c.expected_yield_oz))
        .unwrap_or_else(|| (crop_id.to_string(), 0.0));

    let mut tray_ids = Vec::new();
    let mut tray_count: i64 = 0;
    for t in &trays {
        if t.state != "light" {
            continue;
        }
        if t.crop_id != crop_id {
            continue;
        }
        let Some(ref ehd) = t.expected_harvest_date else {
            continue;
        };
        if ehd.as_str() <= today || ehd.as_str() > harvest_date {
            continue;
        }
        tray_ids.push(t.id.clone());
        tray_count += t.quantity;
    }
    if tray_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![HarvestGroup {
        crop_id: crop_id.to_string(),
        crop_name,
        tray_ids,
        tray_count,
        estimated_yield_oz: round1(tray_count as f64 * ey),
    }])
}

pub fn today_view(conn: &Connection) -> Result<TodayView, String> {
    let today = db::local_date_today();
    let sql = format!(
        "{TRAY_VIEW_SELECT}
         WHERE t.state IN ('planned', 'sown', 'blackout', 'light')
         ORDER BY c.sort_order ASC, t.created_at ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], map_tray_view)
        .map_err(|e| e.to_string())?;

    let mut trays = Vec::new();
    for r in rows {
        trays.push(r.map_err(|e| e.to_string())?);
    }

    let active_tray_count: i64 = trays.iter().map(|t| t.quantity).sum();
    let sown_today = trays
        .iter()
        .any(|t| t.sown_on.as_deref() == Some(today.as_str()));

    // move_to_light — blackout trays whose cover_check_date <= today
    let mut mtl_ids = Vec::new();
    let mut mtl_count: i64 = 0;
    for t in &trays {
        if t.state == "blackout" {
            if let Some(ref ccd) = t.cover_check_date {
                if ccd.as_str() <= today.as_str() {
                    mtl_ids.push(t.id.clone());
                    mtl_count += t.quantity;
                }
            }
        }
    }
    let move_to_light = if mtl_ids.is_empty() {
        None
    } else {
        Some(MoveToLight {
            tray_ids: mtl_ids,
            tray_count: mtl_count,
        })
    };

    // harvests — light trays with expected_harvest_date <= today, grouped by crop
    let crops = list_crops(conn)?;
    let crop_yield: std::collections::HashMap<String, f64> = crops
        .iter()
        .map(|c| (c.id.clone(), c.expected_yield_oz))
        .collect();
    let crop_meta: std::collections::HashMap<String, (String, i64)> = crops
        .iter()
        .map(|c| (c.id.clone(), (c.name.clone(), c.sort_order)))
        .collect();

    #[allow(clippy::type_complexity)]
    // H-7 Class E: a local accumulator's type, not a signature. Nothing here to narrow.
    let mut harvest_map: std::collections::BTreeMap<
        (i64, String),
        (String, Vec<String>, i64, f64),
    > = std::collections::BTreeMap::new();

    for t in &trays {
        if t.state != "light" {
            continue;
        }
        let Some(ref ehd) = t.expected_harvest_date else {
            continue;
        };
        if ehd.as_str() > today.as_str() {
            continue;
        }
        let (name, sort) = crop_meta
            .get(&t.crop_id)
            .cloned()
            .unwrap_or_else(|| (t.crop_name.clone(), 999));
        let ey = crop_yield.get(&t.crop_id).copied().unwrap_or(0.0);
        let entry = harvest_map
            .entry((sort, t.crop_id.clone()))
            .or_insert_with(|| (name, Vec::new(), 0, 0.0));
        entry.1.push(t.id.clone());
        entry.2 += t.quantity;
        entry.3 += t.quantity as f64 * ey;
    }

    let mut harvest_est_total = 0.0;
    let harvests: Vec<HarvestGroup> = harvest_map
        .into_iter()
        .map(|((_, crop_id), (crop_name, tray_ids, tray_count, est))| {
            harvest_est_total += est;
            HarvestGroup {
                crop_id,
                crop_name,
                tray_ids,
                tray_count,
                estimated_yield_oz: round1(est),
            }
        })
        .collect();

    let harvest_summary = if harvests.is_empty() {
        None
    } else {
        let tray_count: i64 = harvests.iter().map(|h| h.tray_count).sum();
        let variety_count = harvests.len() as i64;
        let single_crop_name = if variety_count == 1 {
            Some(harvests[0].crop_name.clone())
        } else {
            None
        };
        Some(HarvestSummary {
            tray_count,
            variety_count,
            estimated_yield_oz: round1(harvest_est_total),
            single_crop_name,
        })
    };

    // next_events — future cover-checks and harvests, one entry per
    // (date, kind, crop). The retired single-slot version summed quantities
    // across crops under the first crop's name (sort order — Dun peas) and
    // hid later-dated crops entirely. Per-crop truth; ordering: date, then
    // light before harvest, then crop sort order.
    let mut next_map: std::collections::BTreeMap<(String, u8, i64, String), (String, i64)> =
        std::collections::BTreeMap::new();
    for t in &trays {
        let cand: Option<(&str, &str)> = match t.state.as_str() {
            "blackout" | "sown" => t
                .cover_check_date
                .as_deref()
                .filter(|d| *d > today.as_str())
                .map(|d| ("light", d)),
            "light" => t
                .expected_harvest_date
                .as_deref()
                .filter(|d| *d > today.as_str())
                .map(|d| ("harvest", d)),
            _ => None,
        };
        if let Some((kind, date)) = cand {
            let sort = crop_meta.get(&t.crop_id).map(|(_, s)| *s).unwrap_or(999);
            let kind_rank = if kind == "light" { 0u8 } else { 1u8 };
            let entry = next_map
                .entry((date.to_string(), kind_rank, sort, t.crop_id.clone()))
                .or_insert_with(|| (t.crop_name.clone(), 0));
            entry.1 += t.quantity;
        }
    }
    let next_events: Vec<NextEvent> = next_map
        .into_iter()
        .map(
            |((date, kind_rank, _, _), (crop_name, tray_count))| NextEvent {
                kind: if kind_rank == 0 {
                    "light".to_string()
                } else {
                    "harvest".to_string()
                },
                date,
                tray_count,
                crop_name,
            },
        )
        .collect();

    Ok(TodayView {
        move_to_light,
        harvests,
        harvest_summary,
        next_events,
        active_tray_count,
        sown_today,
    })
}

// --- dev.backdated (debug-only writer; projection replays in all builds) ------

/// Debug-only: shift every non-null date column on a tray back by `days`.
/// Compiles only under `debug_assertions` — absent from release builds.
#[cfg(debug_assertions)]
pub fn dev_backdate_tray(conn: &mut Connection, tray_id: &str, days: i64) -> Result<(), String> {
    if days < 1 {
        return Err("days must be >= 1".to_string());
    }
    // Ensure tray exists.
    let _ = get_tray(conn, tray_id)?;

    let payload = json!({ "trayId": tray_id, "days": days });
    // Inverse shifts forward by the same amount.
    let inverse = json!({
        "op": "shift_tray_dates",
        "trayId": tray_id,
        "days": days,
    });
    let now = projection::handler_now();
    let event = build_grow_event(
        Kind::DevBackdated,
        "tray",
        tray_id,
        payload,
        inverse,
        None,
        None,
        now,
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Projection: shift `payload.trayId` back by `payload.days`, stamped with
/// `event.created_at`. Not `cfg(debug_assertions)`-gated — verify-replay must
/// be able to replay historical `dev.backdated` rows in release builds too.
pub(crate) fn apply_dev_backdated(tx: &Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    let tray_id = event
        .payload
        .get("trayId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "dev.backdated payload missing trayId".to_string())?;
    let days = event
        .payload
        .get("days")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "dev.backdated payload missing days".to_string())?;
    events::shift_tray_dates(tx, tray_id, -days, &event.created_at)
}

#[cfg(test)]
pub fn count_event_log(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM event_log", [], |row| row.get(0))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
pub fn count_event_kind(conn: &Connection, kind: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM event_log WHERE kind = ?1",
        [kind],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
pub fn event_undone_at(conn: &Connection, seq: i64) -> Result<Option<String>, String> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT undone_at FROM event_log WHERE seq = ?1",
        [seq],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("event {seq} not found"))
}

#[cfg(test)]
pub fn event_inverse_nonempty(conn: &Connection, seq: i64) -> Result<bool, String> {
    let inv: String = conn
        .query_row(
            "SELECT inverse FROM event_log WHERE seq = ?1",
            [seq],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(!inv.is_empty() && inv != "null")
}

/// Test helper: insert a tray already in `sown` (bypasses sow_tray's blackout landing).
#[cfg(test)]
pub fn insert_sown_tray(
    conn: &mut Connection,
    crop_id: &str,
    quantity: i64,
) -> Result<TrayView, String> {
    let (growth_days, blackout_days) = db::get_crop_growth_blackout(conn, crop_id)?;
    let tray_id = Uuid::new_v4().to_string();
    let today = db::local_date_today();
    let now = db::utc_now_rfc3339();
    conn.execute(
        "INSERT INTO trays (
            id, crop_id, state, quantity,
            growth_days_at_sow, blackout_days_at_sow,
            planned_on, sown_on, blackout_on, light_on, harvested_on, discarded_on,
            actual_yield_oz, created_at, updated_at
         ) VALUES (
            ?1, ?2, 'sown', ?3,
            ?4, ?5,
            NULL, ?6, NULL, NULL, NULL, NULL,
            NULL, ?7, ?7
         )",
        params![
            tray_id,
            crop_id,
            quantity,
            growth_days,
            blackout_days,
            today,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    get_tray(conn, &tray_id)
}

/// Test helper: shift tray dates without going through the event log.
#[cfg(test)]
pub fn test_shift_dates(conn: &mut Connection, tray_id: &str, days: i64) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    events::shift_tray_dates(&tx, tray_id, days, &db::utc_now_rfc3339())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}
