use rusqlite::Connection;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::event_partition::EventDomain;

const NOT_A_FARM: &str = "That file isn't a Farm OS farm.";
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const FOREIGN_DB_REFUSAL: &str =
    "This database belongs to another app. Groundtruth will not open it.";

/// SQLite application_id stamp for Groundtruth ("GTRU").
pub const GROUNDTRUTH_APPLICATION_ID: i32 = 0x47545255; // "GTRU"

/// Freeze retired 2026-08-12: this app is the writer of farm truth. `true`
/// makes `lock_is_active` return false unconditionally, so every gate that
/// consults it — `events::write_event`, the ordinary path of
/// `import::apply_import`, `snapshots::validate_farm_file` — is inert and
/// "Bring in a bundle" is the one import door. The gates and the test opt-in
/// below are kept for the freeze-machinery cleanup; the desk's cutover door
/// is already gone (Settings fence 1, S5). C4 (INT-004) made these comments
/// say what the suite says.
pub const CUTOVER_LIVE: bool = true;

/// Kinds this app writes about ITSELF. Neither is farm truth
/// (`is_farm_truth`), and neither is sealed by the marketing validator
/// (events.rs). Named under the freeze (GT-D7), when they were the only kinds
/// the app could originate; the classification outlived the freeze.
pub const HOUSEKEEPING_KINDS: [crate::events::Kind; 2] = [
    crate::events::Kind::SnapshotTaken,
    crate::events::Kind::AttentionResolved,
];

/// GT-D20: phone proposals are candidates, not farm truth. A database holding
/// only proposals has no farm lineage (GT-D2); `is_farm_truth` never counted
/// them, under the freeze or since.
pub const PROPOSAL_KINDS: [crate::events::Kind; 2] = [
    crate::events::Kind::PhoneProposed,
    crate::events::Kind::PhoneProposalDecided,
];

// Freeze-era test opt-in. Nothing reads it while `CUTOVER_LIVE` holds:
// `lock_is_active` returns before it is consulted, so the `set(true)` calls
// still in the suite are inert (int004_ pins this). Kept with the machinery
// it belongs to, for the same cleanup.
#[cfg(test)]
thread_local! {
    pub static LOCK_ACTIVE_IN_TEST: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Always false since the freeze retired (`CUTOVER_LIVE`). The branches
/// below are the retired freeze — live in real builds, per-thread opt-in in
/// tests — and are unreachable; kept only for the cleanup.
pub fn lock_is_active() -> bool {
    if CUTOVER_LIVE {
        return false;
    }
    #[cfg(test)]
    {
        LOCK_ACTIVE_IN_TEST.with(|c| c.get())
    }
    #[cfg(not(test))]
    {
        true
    }
}

fn read_exact(file: &mut File, buf: &mut [u8]) -> Result<(), String> {
    let mut off = 0;
    while off < buf.len() {
        match file.read(&mut buf[off..]) {
            Ok(0) => return Err(NOT_A_FARM.to_string()),
            Ok(n) => off += n,
            Err(_) => return Err(NOT_A_FARM.to_string()),
        }
    }
    Ok(())
}

fn u32_native(bytes: &[u8], big_endian: bool) -> u32 {
    if big_endian {
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    } else {
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}

/// If the stamp was written under WAL and never checkpointed, page 1 (and
/// thus application_id) lives only in the -wal file. Read it there — still
/// no database handle, so no -shm is created.
fn application_id_from_wal(wal_path: &Path) -> Option<i32> {
    let mut file = File::open(wal_path).ok()?;
    let mut hdr = [0u8; 32];
    read_exact(&mut file, &mut hdr).ok()?;
    // Per SQLite WAL format: 0x377f0682 → big-endian header/frame ints;
    // 0x377f0683 → little-endian.
    let magic = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let big_endian = match magic {
        0x377f0682 => true,
        0x377f0683 => false,
        _ => return None,
    };
    let mut page_size = u32_native(&hdr[8..12], big_endian) as usize;
    if page_size == 1 {
        page_size = 65536;
    }
    if !(512..=65536).contains(&page_size) {
        return None;
    }
    let frame_size = 24 + page_size;
    let mut latest: Option<i32> = None;
    let mut frame = vec![0u8; frame_size];
    loop {
        let mut off = 0;
        while off < frame_size {
            match file.read(&mut frame[off..]) {
                Ok(0) if off == 0 => return latest,
                Ok(0) => return latest, // truncated frame: ignore
                Ok(n) => off += n,
                Err(_) => return latest,
            }
        }
        let page_number = u32_native(&frame[0..4], big_endian);
        if page_number == 1 {
            let page = &frame[24..];
            if page.len() >= 72 && &page[0..16] == SQLITE_MAGIC.as_slice() {
                latest = Some(i32::from_be_bytes([page[68], page[69], page[70], page[71]]));
            }
        }
    }
}

/// Read application_id straight from the SQLite header. Opens no database
/// handle, so it cannot create -wal/-shm sidecars in the target directory.
/// When the main-file header still shows 0, also peeks at page 1 in a
/// sibling -wal file (read-only) so a stamp that has not been checkpointed
/// is still visible.
pub fn read_application_id_from_file(path: &Path) -> Result<i32, String> {
    let mut file = File::open(path).map_err(|_| NOT_A_FARM.to_string())?;
    let mut header = [0u8; 72];
    read_exact(&mut file, &mut header)?;
    if &header[0..16] != SQLITE_MAGIC.as_slice() {
        return Err(NOT_A_FARM.to_string());
    }
    let main_id = i32::from_be_bytes([header[68], header[69], header[70], header[71]]);
    if main_id != 0 {
        return Ok(main_id);
    }
    // "farm.db" → "farm.db-wal"
    let mut wal_os = path.as_os_str().to_os_string();
    wal_os.push("-wal");
    let wal_path = Path::new(&wal_os);
    if let Some(wal_id) = application_id_from_wal(wal_path) {
        return Ok(wal_id);
    }
    Ok(0)
}

/// Refuse a farm directory that is not Groundtruth's own.
pub fn assert_groundtruth_farm_dir(farm_dir: &Path) -> Result<(), String> {
    let farm_db = farm_dir.join("farm.db");
    let app_id = read_application_id_from_file(&farm_db)?;
    if app_id == GROUNDTRUTH_APPLICATION_ID {
        return Ok(());
    }
    Err(FOREIGN_DB_REFUSAL.to_string())
}

/// Refuse to run when the resolved data directory is the Farm OS folder.
pub fn assert_data_dir_allowed(dir: &Path) -> Result<(), String> {
    let path = match dir.canonicalize() {
        Ok(p) => p,
        Err(_) => dir.to_path_buf(),
    };
    if path.to_string_lossy().contains("com.prairieroots.farmos") {
        return Err("Groundtruth refuses to run inside the Farm OS data folder.".to_string());
    }
    Ok(())
}

/// Stamp a fresh DB with Groundtruth's application_id, or refuse a foreign one.
pub fn stamp_or_refuse(conn: &Connection) -> Result<(), String> {
    let app_id: i32 = conn
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;

    if app_id == GROUNDTRUTH_APPLICATION_ID {
        return Ok(());
    }

    if app_id == 0 {
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'event_log'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let empty_or_absent = if table_exists == 0 {
            true
        } else {
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM event_log", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            rows == 0
        };
        if empty_or_absent {
            conn.pragma_update(None, "application_id", GROUNDTRUTH_APPLICATION_ID)
                .map_err(|e| e.to_string())?;
            // Land the stamp in the main-file header (not only the WAL) so
            // read_application_id_from_file sees it while this connection is open.
            conn.execute_batch("PRAGMA wal_checkpoint(FULL);")
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
    }

    Err(FOREIGN_DB_REFUSAL.to_string())
}

/// True when this kind is farm truth: grow or register tier, minus the
/// housekeeping and proposal kinds. Read by lineage (GT-D2,
/// `import::build_plan`) and by the cutover door's has-farm-truth check;
/// the retired freeze gate (`enforce_cutover_lock`) read it too.
pub fn is_farm_truth(kind: crate::events::Kind) -> bool {
    if HOUSEKEEPING_KINDS.contains(&kind) || PROPOSAL_KINDS.contains(&kind) {
        return false;
    }
    let (domain, _) = kind.tier();
    matches!(domain, EventDomain::Grow | EventDomain::Register)
}

/// True when this database's event_log holds any farm-truth event.
/// Reads only. Unknown kind strings count as farm truth — fail closed.
/// Its only caller is the retired freeze gate in
/// `snapshots::validate_farm_file`, never reached while `CUTOVER_LIVE`.
pub fn contains_farm_truth(conn: &Connection) -> Result<bool, String> {
    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'event_log'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if table_exists == 0 {
        return Ok(false);
    }
    let mut stmt = conn
        .prepare("SELECT DISTINCT kind FROM event_log")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let raw = row.map_err(|e| e.to_string())?;
        match crate::events::Kind::parse(&raw) {
            Ok(kind) => {
                if is_farm_truth(kind) {
                    return Ok(true);
                }
            }
            Err(_) => return Ok(true), // unknown kind: fail closed
        }
    }
    Ok(false)
}

/// The retired freeze's refusal. Pure: refuses any farm-truth kind with the
/// freeze sentence whether or not a freeze is on. Real builds never reach it —
/// `events::write_event` consults `lock_is_active` first, which is always
/// false (`CUTOVER_LIVE`). Tests call it directly as a classifier
/// (identity t1, marketing m9 / m10). The sentence is kept verbatim.
pub fn enforce_cutover_lock(kind: crate::events::Kind) -> Result<(), String> {
    if is_farm_truth(kind) {
        return Err("Farm truth lives in Farm OS until cutover.".to_string());
    }
    Ok(())
}
