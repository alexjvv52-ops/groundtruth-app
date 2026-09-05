use crate::db::{self, SCHEMA_VERSION};
use crate::events;
use crate::events::{EventRecord, Kind};
use crate::models::SnapshotInfo;
use crate::projection;
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const NOT_A_FARM: &str = "That file isn't a Farm OS farm.";

/// Best-effort snapshot for launch/shutdown. Never takes the app down.
pub fn try_take_snapshot(conn: &mut Connection, snapshots_dir: &Path) {
    if let Err(e) = take_snapshot(conn, snapshots_dir) {
        eprintln!("snapshot failed: {e}");
        if let Err(ae) = crate::attention::raise_snapshot_failed(conn) {
            eprintln!("attention raise failed: {ae}");
        }
    }
}

pub fn take_snapshot(conn: &mut Connection, snapshots_dir: &Path) -> Result<SnapshotInfo, String> {
    fs::create_dir_all(snapshots_dir).map_err(|e| e.to_string())?;
    // One local clock read for filename + retention; one UTC read for the event.
    let local_now = Local::now();
    let dest = unique_snapshot_path(snapshots_dir, local_now)?;
    let dest_str = dest
        .to_str()
        .ok_or_else(|| "snapshot path is not valid UTF-8".to_string())?;
    // VACUUM INTO cannot run inside a transaction — file first, then register event.
    conn.execute("VACUUM INTO ?1", rusqlite::params![dest_str])
        .map_err(|e| e.to_string())?;
    let info = snapshot_info_for_path(&dest)?;
    apply_retention(snapshots_dir, local_now)?;

    let now = projection::handler_now();
    let payload = json!({
        "fileName": info.file_name,
        "path": info.path,
        "takenAt": info.taken_at,
        "sizeBytes": info.size_bytes,
    });
    let event = EventRecord::originated(
        Kind::SnapshotTaken,
        "snapshot",
        info.file_name.clone(),
        payload,
        json!({ "op": "none" }),
        now,
        None,
        None,
        Some(projection::handler_new_id()),
    );

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    // snapshot.taken is an explicit SQL no-op (Ruling 2 cat. 3); apply_event is
    // called anyway so every kind flows through the same live/replay path.
    projection::apply_event(&tx, &event)?;
    events::insert_event(&tx, &event)?;
    tx.commit().map_err(|e| e.to_string())?;

    Ok(info)
}

pub fn list_snapshots(snapshots_dir: &Path) -> Result<Vec<SnapshotInfo>, String> {
    if !snapshots_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(snapshots_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if parse_snapshot_name(name).is_none() {
            continue;
        }
        out.push(snapshot_info_for_path(&path)?);
    }
    out.sort_by(|a, b| b.taken_at.cmp(&a.taken_at));
    Ok(out)
}

pub fn last_snapshot_at(snapshots_dir: &Path) -> Result<Option<String>, String> {
    Ok(list_snapshots(snapshots_dir)?
        .into_iter()
        .next()
        .map(|s| s.taken_at))
}

/// Keep every snapshot from the last 48 hours, plus the newest from each of
/// the last 30 calendar days. Delete the rest.
pub fn apply_retention(snapshots_dir: &Path, now: DateTime<Local>) -> Result<(), String> {
    if !snapshots_dir.exists() {
        return Ok(());
    }

    let mut entries: Vec<(PathBuf, DateTime<Local>)> = Vec::new();
    for entry in fs::read_dir(snapshots_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(taken) = parse_snapshot_name(name) else {
            continue;
        };
        entries.push((path, taken));
    }

    let keep = retention_keep_set(&entries, now);
    for (path, _) in &entries {
        if !keep.contains(path) {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

/// Pure retention decision for tests.
pub fn retention_keep_set(
    entries: &[(PathBuf, DateTime<Local>)],
    now: DateTime<Local>,
) -> HashSet<PathBuf> {
    let cutoff_48h = now - Duration::hours(48);
    let today = now.date_naive();
    let first_day = today - chrono::Days::new(29);

    let mut keep: HashSet<PathBuf> = HashSet::new();
    for (path, taken) in entries {
        if *taken >= cutoff_48h {
            keep.insert(path.clone());
        }
    }

    let mut newest_per_day: HashMap<NaiveDate, (PathBuf, DateTime<Local>)> = HashMap::new();
    for (path, taken) in entries {
        let day = taken.date_naive();
        if day < first_day || day > today {
            continue;
        }
        match newest_per_day.get(&day) {
            Some((_, prev)) if *prev >= *taken => {}
            _ => {
                newest_per_day.insert(day, (path.clone(), *taken));
            }
        }
    }
    for (_, (path, _)) in newest_per_day {
        keep.insert(path);
    }
    keep
}

pub fn validate_farm_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(NOT_A_FARM.to_string());
    }
    let meta = fs::metadata(path).map_err(|_| NOT_A_FARM.to_string())?;
    if meta.len() == 0 {
        return Err(NOT_A_FARM.to_string());
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| NOT_A_FARM.to_string())?;

    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| NOT_A_FARM.to_string())?;
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(NOT_A_FARM.to_string());
    }

    let app_id: i32 = conn
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(|_| NOT_A_FARM.to_string())?;
    if app_id != crate::identity::GROUNDTRUTH_APPLICATION_ID {
        return Err(
            "This database belongs to another app. Groundtruth will not open it.".to_string(),
        );
    }

    // Retired freeze gate (identity::CUTOVER_LIVE): lock_is_active is always
    // false, so a backup with farm records is accepted and this refusal is
    // never returned. Kept for the cleanup; the sentence is kept verbatim.
    if crate::identity::lock_is_active() && crate::identity::contains_farm_truth(&conn)? {
        return Err("That backup contains farm records. Farm truth lives in \
             Farm OS until cutover."
            .to_string());
    }

    for table in ["crops", "trays", "event_log"] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|_| NOT_A_FARM.to_string())?;
        if exists == 0 {
            return Err(NOT_A_FARM.to_string());
        }
    }
    Ok(())
}

/// Restore in the exact order required by Stage 3.
pub fn restore_snapshot(
    db: &Mutex<Connection>,
    farm_db_path: &Path,
    snapshots_dir: &Path,
    source_path: &Path,
) -> Result<(), String> {
    let mut guard = db.lock().map_err(|e| e.to_string())?;

    // 1. Snapshot the on-disk live farm first. Failure aborts — never overwrite an
    // unbacked farm. Open by path so this still works when the managed connection
    // was already released to free file locks (restore sidecars / WAL decoys).
    {
        let mut live = Connection::open(farm_db_path).map_err(|e| e.to_string())?;
        db::configure(&live)?;
        take_snapshot(&mut live, snapshots_dir)?;
    }

    // 2. Validate the source file.
    validate_farm_file(source_path)?;

    // 3. Release the live connection before touching files on disk.
    let old = std::mem::replace(
        &mut *guard,
        Connection::open_in_memory().map_err(|e| e.to_string())?,
    );
    drop(old);

    // 4. Delete farm.db, farm.db-wal, and farm.db-shm.
    remove_farm_files(farm_db_path)?;

    // 5. Copy the snapshot into place as farm.db.
    if let Some(parent) = farm_db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::copy(source_path, farm_db_path).map_err(|e| e.to_string())?;

    // 6. Reopen, re-apply pragmas, migrate, put back.
    let new_conn = db::open_and_migrate(farm_db_path)?;

    // 6.5 Locked restore policy: archive the old log, rebuild from the
    // restored event_log, so database and log match by construction.
    let farm_dir = farm_db_path
        .parent()
        .ok_or_else(|| "farm.db has no parent directory".to_string())?;
    let rebuild =
        crate::event_file::archive_and_rebuild(&new_conn, farm_dir, &crate::db::utc_now_rfc3339());
    if let Err(e) = rebuild {
        // The database restore itself succeeded; only the log rebuild failed
        // and was rolled back, so the fork stands. Put the REAL connection
        // back before returning — never leave the session on the in-memory
        // placeholder from step 3, where Health would compute against an
        // empty decoy and new writes would evaporate on exit.
        *guard = new_conn;
        return Err(format!(
            "The farm was restored, but the log could not be rebuilt: {e}. \
             The old log was put back; the log and database still disagree \
             and Health will say so."
        ));
    }

    // Raise farm.restored on the restored database (observation, not a ledger event).
    let label = snapshot_info_for_path(source_path)
        .ok()
        .map(|info| crate::attention::restore_label_from_taken_at(&info.taken_at))
        .unwrap_or_else(|| "a previous backup".to_string());
    if let Err(e) = crate::attention::raise_farm_restored(&new_conn, &label) {
        eprintln!("attention raise failed: {e}");
    }

    *guard = new_conn;
    Ok(())
}

fn remove_farm_files(farm_db_path: &Path) -> Result<(), String> {
    let wal = farm_wal_path(farm_db_path);
    let shm = farm_shm_path(farm_db_path);
    for path in [farm_db_path, wal.as_path(), shm.as_path()] {
        if path.exists() {
            fs::remove_file(path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn farm_wal_path(farm_db_path: &Path) -> PathBuf {
    sidecar_path(farm_db_path, "-wal")
}

pub fn farm_shm_path(farm_db_path: &Path) -> PathBuf {
    sidecar_path(farm_db_path, "-shm")
}

fn sidecar_path(farm_db_path: &Path, suffix: &str) -> PathBuf {
    let mut s = farm_db_path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn unique_snapshot_path(snapshots_dir: &Path, now: DateTime<Local>) -> Result<PathBuf, String> {
    let stamp = now.format("%Y-%m-%d-%H%M%S");
    let mut path = snapshots_dir.join(format!("farm-{stamp}.db"));
    let mut n = 1u32;
    while path.exists() {
        path = snapshots_dir.join(format!("farm-{stamp}-{n}.db"));
        n += 1;
        if n > 10_000 {
            return Err("could not find a unique snapshot name".to_string());
        }
    }
    Ok(path)
}

fn snapshot_info_for_path(path: &Path) -> Result<SnapshotInfo, String> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "invalid snapshot filename".to_string())?
        .to_string();
    let taken = parse_snapshot_name(&file_name)
        .ok_or_else(|| format!("unrecognized snapshot name: {file_name}"))?;
    let size_bytes = fs::metadata(path).map_err(|e| e.to_string())?.len() as i64;
    let path_str = path
        .to_str()
        .ok_or_else(|| "snapshot path is not valid UTF-8".to_string())?
        .to_string();
    Ok(SnapshotInfo {
        file_name,
        path: path_str,
        taken_at: taken.to_rfc3339(),
        size_bytes,
    })
}

/// Parse `farm-YYYY-MM-DD-HHMMSS.db` or `farm-YYYY-MM-DD-HHMMSS-N.db`.
pub fn parse_snapshot_name(name: &str) -> Option<DateTime<Local>> {
    let stem = name.strip_suffix(".db")?;
    let rest = stem.strip_prefix("farm-")?;
    let parts: Vec<&str> = rest.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let y: i32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    let hms = parts[3];
    if hms.len() != 6 || !hms.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let hour: u32 = hms[0..2].parse().ok()?;
    let min: u32 = hms[2..4].parse().ok()?;
    let sec: u32 = hms[4..6].parse().ok()?;
    let naive = NaiveDateTime::new(
        NaiveDate::from_ymd_opt(y, m, d)?,
        chrono::NaiveTime::from_hms_opt(hour, min, sec)?,
    );
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
        .or_else(|| Local.from_local_datetime(&naive).latest())
}
