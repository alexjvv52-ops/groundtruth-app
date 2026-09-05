//! EXPORT B (C1 security sweep). The export bundle travels — a USB stick, a
//! cloud folder, a new machine — and the two connection secrets stay home.
//! `export::write_bundle` calls [`scrub_bundle_copy`] on the bundle's own
//! farm.db copy, after VACUUM INTO and before the manifest hashes the file.
//!
//! Scope, exactly: `stripe_config.restricted_key` and `scan_config.pull_token`
//! in the COPY. The live farm.db is never opened here — this module takes a
//! path, never the live connection. Snapshots (snapshots.rs) are untouched:
//! they never leave this disk and restore needs them whole. verify-replay
//! neither copies nor compares either table (projection::verify::EXCLUSION_LIST),
//! so a scrubbed bundle passes "Bring in a bundle" exactly as before; after a
//! restore the grower re-enters the key on Money and the pull token on
//! Settings, Connections.
//!
//! This lives beside export.rs, not inside it, because ex4 in export_tests
//! scans export.rs production source for connection vocabulary and must keep
//! doing so.
use rusqlite::Connection;
use std::path::Path;
/// One sentence for manifest.json `notes`. The bundle root gains no file.
pub(crate) const MANIFEST_NOTE: &str = "the Stripe restricted key and the scan-endpoint pull token are not in this bundle's farm.db; they stay on the machine that exported it and are re-entered on Money (Connect Stripe) and Settings, Connections after a restore";
/// NULL the two secrets in the bundle copy at `bundle_farm_db`.
/// VACUUM INTO writes a rollback-journal file, so this write leaves no -wal or
/// -shm beside the copy; `secure_delete` zeroes the bytes the old values
/// occupied, so they cannot be read back with a hex editor either.
pub(crate) fn scrub_bundle_copy(bundle_farm_db: &Path) -> Result<(), String> {
    let copy = Connection::open(bundle_farm_db).map_err(|e| e.to_string())?;
    copy.execute_batch(
        "PRAGMA secure_delete = ON;
         UPDATE stripe_config SET restricted_key = NULL WHERE id = 1;
         UPDATE scan_config SET pull_token = NULL WHERE id = 1;",
    )
    .map_err(|e| e.to_string())?;
    copy.close().map_err(|(_, e)| e.to_string())
}
