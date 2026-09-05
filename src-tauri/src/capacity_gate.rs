//! GT-D12 — capacity sight from the newest Farm OS snapshot.
//!
//! Groundtruth may look at a Farm OS snapshot; it may not leave a fingerprint
//! on one. This module contains no INSERT, UPDATE, DELETE, CREATE, PRAGMA-write
//! or filesystem write of any kind. Snapshots are opened with immutable=1 and
//! SQLITE_OPEN_READ_ONLY so SQLite creates neither -wal nor -shm.

use crate::models::CapacityRow;
use crate::observed::Observed;
use crate::trays;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Remaining trays at or below this total mark a sight near-committed.
/// C5 (INT-006): this no longer inverts pitching advice.
pub const COMMITTED_REMAINING_THRESHOLD: i64 = 4;

/// Origin stamp for live capacity sight (GT-D12-R).
pub const LIVE_ORIGIN: &str = "live farm database";

static DATA_DIR_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// RETIRED by GT-D12-R (Phase 2): the Farm OS snapshots froze at cutover,
/// so this read can only age. Nothing calls it. Stripped in P-2.
#[allow(dead_code)]
/// Test / fixture hook: when set, `latest_farmos_snapshot` derives the Farm OS
/// snapshots path from this directory instead of the process data dir.
pub fn set_data_dir_override(dir: Option<PathBuf>) {
    *DATA_DIR_OVERRIDE.lock().unwrap() = dir;
}

/// RETIRED by GT-D12-R (Phase 2): the Farm OS snapshots froze at cutover,
/// so this read can only age. Nothing calls it. Stripped in P-2.
#[allow(dead_code)]
fn groundtruth_data_dir() -> Option<PathBuf> {
    if let Ok(guard) = DATA_DIR_OVERRIDE.lock() {
        if let Some(dir) = guard.as_ref() {
            return Some(dir.clone());
        }
    }
    // Prefer the same APPDATA layout Tauri uses for this identifier.
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("com.prairieroots.groundtruth"))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                PathBuf::from(h)
                    .join(".local")
                    .join("share")
                    .join("com.prairieroots.groundtruth")
            })
        })
}

/// RETIRED by GT-D12-R (Phase 2): the Farm OS snapshots froze at cutover,
/// so this read can only age. Nothing calls it. Stripped in P-2.
#[allow(dead_code)]
/// Newest `.db` under the Farm OS snapshots directory, or None if absent.
pub fn latest_farmos_snapshot() -> Option<PathBuf> {
    let gt = groundtruth_data_dir()?;
    let parent = gt.parent()?;
    let name = gt.file_name()?.to_str()?;
    if name != "com.prairieroots.groundtruth" {
        // Override dirs used in tests still replace the final component when
        // it matches; otherwise treat the override itself as the GT data dir
        // sibling layout: <override>/../com.prairieroots.farmos/snapshots is
        // wrong — instead join sibling by replacing the leaf.
    }
    let farmos_snaps = parent.join("com.prairieroots.farmos").join("snapshots");
    if !farmos_snaps.is_dir() {
        return None;
    }
    let mut dbs: Vec<PathBuf> = std::fs::read_dir(&farmos_snaps)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("db"))
                .unwrap_or(false)
        })
        .collect();
    if dbs.is_empty() {
        return None;
    }
    dbs.sort_by(|a, b| {
        let an = a.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let bn = b.file_name().and_then(|s| s.to_str()).unwrap_or("");
        an.cmp(bn)
    });
    dbs.pop()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacitySight {
    pub rows: Vec<CapacityRow>,
    /// Snapshot age — RFC3339 when parseable from the filename, else mtime.
    pub taken_at: String,
    pub file_name: String,
}

impl CapacitySight {
    /// C5 (INT-006): no production caller. A date-wide, cross-crop total cannot
    /// speak for pitching — a crop's surplus netting out another crop's deficit
    /// is the double-sell D21a exists to prevent (`trays.rs:1993-1996`).
    /// Live in tests; dead only in the lib target.
    #[allow(dead_code)]
    pub fn remaining_trays_total(&self) -> i64 {
        self.rows.iter().map(|r| r.remaining_trays).sum()
    }
    /// C5 (INT-006): no production caller. Live in tests; dead only in the lib target.
    #[allow(dead_code)]
    pub fn near_committed(&self) -> bool {
        self.remaining_trays_total() <= COMMITTED_REMAINING_THRESHOLD
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "state", content = "value")]
pub enum CapacityState {
    Known(CapacitySight),
    Unknown(String),
}

/// RETIRED by GT-D12-R (Phase 2): the Farm OS snapshots froze at cutover,
/// so this read can only age. Nothing calls it. Stripped in P-2.
#[allow(dead_code)]
/// Open a Farm OS snapshot read-only + immutable and run the shared capacity query.
pub fn read_capacity(snapshot: &Path) -> Result<CapacitySight, String> {
    let file_name = snapshot
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("snapshot.db")
        .to_string();
    let taken_at = crate::snapshots::parse_snapshot_name(&file_name)
        .map(|dt| dt.with_timezone(&chrono::Utc).to_rfc3339())
        .or_else(|| {
            std::fs::metadata(snapshot)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.to_rfc3339()
                })
        })
        .unwrap_or_else(|| "unknown".into());

    // URI form so immutable=1 suppresses -shm/-wal. Do NOT call db::configure.
    let path_str = snapshot.to_string_lossy().replace('\\', "/");
    let uri = format!("file:{path_str}?immutable=1");
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(&uri, flags).map_err(|e| e.to_string())?;
    let rows = trays::capacity_by_harvest_date(&conn)?;
    Ok(CapacitySight {
        rows,
        taken_at,
        file_name,
    })
}

/// RETIRED by GT-D12-R (Phase 2): the Farm OS snapshots froze at cutover,
/// so this read can only age. Nothing calls it. Stripped in P-2.
#[allow(dead_code)]
/// Sight only — never guesses a number.
pub fn capacity_sight() -> CapacityState {
    let Some(path) = latest_farmos_snapshot() else {
        return CapacityState::Unknown(
            "Capacity unknown — last Farm OS snapshot not readable".into(),
        );
    };
    match read_capacity(&path) {
        Ok(sight) => CapacityState::Known(sight),
        Err(e) => CapacityState::Unknown(format!(
            "Capacity unknown — last Farm OS snapshot not readable ({e})"
        )),
    }
}

/// GT-D12-R (Phase 2): capacity sight computes from THIS app's live
/// database — the same query Sell online trusts. Never guesses: a query
/// failure returns Unknown with the reason, and no advice is offered.
pub fn capacity_sight_live(conn: &Connection) -> CapacityState {
    match trays::capacity_by_harvest_date(conn) {
        Ok(rows) => CapacityState::Known(CapacitySight {
            rows,
            taken_at: crate::db::utc_now_rfc3339(),
            file_name: LIVE_ORIGIN.to_string(),
        }),
        Err(e) => CapacityState::Unknown(format!(
            "Capacity unknown — live farm database query failed ({e})"
        )),
    }
}

/// UI wrapper: Observed only when Known. Unknown is returned as Err reason
/// so the page can render the silence line without inventing a value.
pub fn capacity_observed(conn: &Connection) -> Result<Observed<CapacitySight>, String> {
    match capacity_sight_live(conn) {
        CapacityState::Known(sight) => {
            let fetched_at = sight.taken_at.clone();
            let origin = sight.file_name.clone();
            Ok(Observed::new(sight, fetched_at, origin))
        }
        CapacityState::Unknown(reason) => Err(reason),
    }
}

/// Pitching advice line for weekly actions. None means offer no pitching advice.
///
/// C5 (INT-006): shelf pressure is the only signal that speaks here. The sight
/// is a date-wide, cross-crop rollup, and one crop's surplus netting out
/// another crop's deficit is the double-sell D21a exists to prevent
/// (`trays.rs:1993-1996`). `state` is kept in the signature so the caller at
/// `marketing.rs:3011` is unchanged; nothing reads it.
pub fn pitching_advice(
    _state: &CapacityState,
    pressure: &crate::reachability::ShelfPressure,
) -> Option<&'static str> {
    if pressure.firing {
        return Some("Hold pitching - you are out of space, not out of calendar.");
    }
    None
}
