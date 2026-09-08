//! KEY-AT-REST (signed 2026-09-07). The two connection secrets —
//! `stripe_config.restricted_key` and `scan_config.pull_token` — are sealed
//! to this Windows account before they touch the farm file and opened only
//! on the way to the wire. A raw copy of the folder read on another machine,
//! or as another account, holds two blobs that will not open.
//!
//! Format: the same TEXT columns, tagged. `dpapi1:` + base64 of a DPAPI
//! user-scope blob (`CRYPTPROTECT_UI_FORBIDDEN`, optional entropy = the
//! little-endian bytes of `identity::GROUNDTRUTH_APPLICATION_ID`). A build
//! for another OS writes `plain:` + the value, honestly labelled, and cannot
//! open a `dpapi1:` blob. An untagged non-empty value is the pre-wrap
//! plaintext: `seal_at_open` seals it on the next open, with `secure_delete`
//! on for the rewrite and a WAL checkpoint after, and never takes a
//! pre-migration snapshot. A tagged value this build or account cannot open
//! stays as it is: the readers report not connected / no token, and one
//! attention card says so. Export still NULLs both columns in the bundle
//! copy; snapshots copy the sealed bytes.

use crate::attention;
use rusqlite::{Connection, OptionalExtension};

const DPAPI_TAG: &str = "dpapi1:";
const PLAIN_TAG: &str = "plain:";

/// The tag this build writes in front of a sealed value.
#[cfg(windows)]
pub const TAG: &str = DPAPI_TAG;
/// The tag this build writes in front of a sealed value.
#[cfg(not(windows))]
pub const TAG: &str = PLAIN_TAG;

/// The one attention kind and its sentence (UNWRAP-FAIL A).
pub const UNREADABLE_KIND: &str = "secret.unreadable";
pub const UNREADABLE_LINE: &str =
    "This machine cannot read a stored secret. Connect Stripe or save the pull token again.";

/// What a stored column holds once opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// NULL or empty.
    Empty,
    /// Untagged and non-empty: plaintext from before the wrap.
    Plain(String),
    /// Tagged, and this build and account opened it.
    Secret(String),
    /// Tagged, and this build or this account cannot open it.
    Unreadable,
}

impl Opened {
    /// The usable value, if there is one.
    pub fn value(self) -> Option<String> {
        match self {
            Opened::Plain(v) | Opened::Secret(v) => Some(v),
            Opened::Empty | Opened::Unreadable => None,
        }
    }
}

/// Seal a plaintext value for the farm file.
pub fn seal(plain: &str) -> Result<String, String> {
    let body = platform::seal(plain.as_bytes())?;
    Ok(format!("{TAG}{body}"))
}

/// Open a stored column value. Never errors: a value that will not open is
/// `Unreadable`, and the callers treat it as absent.
pub fn open(stored: Option<&str>) -> Opened {
    let Some(stored) = stored.filter(|s| !s.is_empty()) else {
        return Opened::Empty;
    };
    if let Some(body) = stored.strip_prefix(DPAPI_TAG) {
        return platform::open_dpapi(body);
    }
    if let Some(body) = stored.strip_prefix(PLAIN_TAG) {
        return platform::open_plain(body);
    }
    Opened::Plain(stored.to_string())
}

/// MIGRATE A, RESIDUE B, UNWRAP-FAIL A — after `migrate`, on every open.
/// Seals any untagged non-empty value in the two columns (idempotent: a
/// tagged value is left alone), with `secure_delete` on for the rewrite and
/// a WAL checkpoint after so the old bytes leave the live file. Raises the
/// one attention card when a tagged value cannot be opened here. Never
/// takes a pre-migration snapshot.
pub fn seal_at_open(conn: &Connection) -> Result<(), String> {
    let mut rewrote = false;
    let mut unreadable = false;
    for (table, column) in [
        ("stripe_config", "restricted_key"),
        ("scan_config", "pull_token"),
    ] {
        let stored: Option<String> = conn
            .query_row(
                &format!("SELECT {column} FROM {table} WHERE id = 1"),
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        match open(stored.as_deref()) {
            Opened::Plain(plain) => {
                let sealed = seal(&plain)?;
                if !rewrote {
                    conn.execute_batch("PRAGMA secure_delete = ON;")
                        .map_err(|e| e.to_string())?;
                    rewrote = true;
                }
                conn.execute(
                    &format!("UPDATE {table} SET {column} = ?1 WHERE id = 1"),
                    [&sealed],
                )
                .map_err(|e| e.to_string())?;
            }
            Opened::Unreadable => unreadable = true,
            Opened::Empty | Opened::Secret(_) => {}
        }
    }
    if rewrote {
        conn.execute_batch("PRAGMA secure_delete = OFF; PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
    }
    if unreadable {
        attention::raise(
            conn,
            UNREADABLE_KIND,
            Some("secret"),
            Some("farm"),
            UNREADABLE_LINE,
            &["dismiss"],
        )?;
    }
    Ok(())
}

#[cfg(windows)]
mod platform {
    use super::Opened;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    /// ENTROPY B: the farm file's own stamp, little-endian.
    fn entropy() -> [u8; 4] {
        crate::identity::GROUNDTRUTH_APPLICATION_ID.to_le_bytes()
    }

    fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        }
    }

    /// Copy DPAPI's output out and free it.
    ///
    /// # Safety
    /// `out` must be a blob DPAPI filled: `pbData` points at `cbData` bytes
    /// allocated with LocalAlloc, and it is freed exactly once, here.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData.cast());
        bytes
    }

    pub fn seal(plain: &[u8]) -> Result<String, String> {
        let entropy = entropy();
        let input = blob(plain);
        let extra = blob(&entropy);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        // SAFETY: `input` and `extra` point at slices that outlive the call;
        // `out` is a live local DPAPI fills; the flags forbid any UI.
        let ok = unsafe {
            CryptProtectData(
                &input,
                ptr::null(),
                &extra,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err("This Windows account could not seal the secret.".to_string());
        }
        // SAFETY: DPAPI returned success, so `out` is its allocation.
        let bytes = unsafe { take(out) };
        Ok(B64.encode(bytes))
    }

    pub fn open_dpapi(body: &str) -> Opened {
        let Ok(sealed) = B64.decode(body) else {
            return Opened::Unreadable;
        };
        let entropy = entropy();
        let input = blob(&sealed);
        let extra = blob(&entropy);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        // SAFETY: as in `seal`; a description pointer is not requested.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                &extra,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Opened::Unreadable;
        }
        // SAFETY: DPAPI returned success, so `out` is its allocation.
        let bytes = unsafe { take(out) };
        match String::from_utf8(bytes) {
            Ok(plain) => Opened::Secret(plain),
            Err(_) => Opened::Unreadable,
        }
    }

    /// A `plain:` value came from a build for another OS; this one does not
    /// pass it through.
    pub fn open_plain(_body: &str) -> Opened {
        Opened::Unreadable
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Opened;

    /// PORTABILITY A: no DPAPI here — the value is stored as itself under
    /// the `plain:` tag, honestly labelled.
    pub fn seal(plain: &[u8]) -> Result<String, String> {
        String::from_utf8(plain.to_vec()).map_err(|e| e.to_string())
    }

    pub fn open_dpapi(_body: &str) -> Opened {
        Opened::Unreadable
    }

    pub fn open_plain(body: &str) -> Opened {
        Opened::Secret(body.to_string())
    }
}
