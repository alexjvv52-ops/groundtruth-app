//! Export bundle — one action, no network, no account (Track 6).
//!
//! Authority: ROADMAP Track 6 done-whens; BOOKS-BOUNDARY outranks.
//! The manifest is the contract. A file absent from manifest.json is not
//! exported; a file listed in it must exist and must match its checksum.
//! This module READS live farm data and WRITES only into the new bundle
//! directory. It must never call snapshots::take_snapshot (that appends a
//! snapshot.taken event) and never flushes events.jsonl.
//! Origin is absolute: only farm_os rows are the grower's own.

use crate::attention;
use crate::categories;
use crate::db;
use crate::event_file;
use crate::export_scrub;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    /// Relative to the bundle root, forward slashes on every platform.
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestCounts {
    pub event_log_rows: i64,
    /// C3 (INT-003): event_log rows with origin commercial_app — another
    /// system's expense records, marked and excluded from every total and
    /// CSV. Defaulted so bundles written before C3 still parse at the door.
    #[serde(default)]
    pub commercial_app_records: i64,
    pub cost_events: i64,
    pub consumption_events: i64,
    pub mileage_trips: i64,
    pub mileage_trips_excluded_voided: i64,
    pub assets: i64,
    pub assets_excluded_voided: i64,
    pub income_events: i64,
    pub income_events_excluded_voided: i64,
    pub stripe_orders_paid: i64,
    pub stripe_orders_excluded_not_paid: i64,
    pub receipts: i64,
    pub marketing_events: i64,
    pub money_corrections: i64,
    pub receivables_orders: i64,
    pub receivables_total_cents: i64,
    pub receivables_orders_partial: i64,
    pub receivables_as_of: String,
    pub income_corrections: i64,
    pub income_corrections_unknown_before: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub manifest_version: i64,
    pub app_schema_version: i32,
    pub exported_at: String,
    pub origin: String,
    pub event_log_watermark: i64,
    pub live_max_seq: i64,
    pub flush_lag: i64,
    pub files: Vec<ManifestFile>,
    pub counts: ManifestCounts,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub bundle_path: String,
    pub file_count: i64,
    pub total_bytes: u64,
    pub exported_at: String,
}

/// One clock read at the top. Writes only into a new bundle directory.
pub fn export_bundle(conn: &Connection, farm_dir: &Path) -> Result<ExportResult, String> {
    let exported_at = db::utc_now_rfc3339();
    let stamp = stamp_from_rfc3339(&exported_at)?;

    let events_path = event_file::events_path(farm_dir);
    let watermark = event_file::read_watermark(&events_path)?;
    let live_max_seq: i64 = conn
        .query_row("SELECT IFNULL(MAX(seq), 0) FROM event_log", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let flush_lag = live_max_seq - watermark;
    if flush_lag != 0 {
        return Err(
            "Some events have not reached the log file yet. Close Farm OS \
             normally and open it again, then export."
                .into(),
        );
    }

    let exports_root = farm_dir.join("exports");
    fs::create_dir_all(&exports_root).map_err(|e| e.to_string())?;
    let bundle_dir = unique_bundle_dir(&exports_root, &stamp)?;
    fs::create_dir_all(&bundle_dir).map_err(|e| e.to_string())?;

    match write_bundle(
        conn,
        farm_dir,
        &bundle_dir,
        &exported_at,
        watermark,
        live_max_seq,
        flush_lag,
    ) {
        Ok(result) => Ok(result),
        Err(e) => {
            let _ = fs::remove_dir_all(&bundle_dir);
            Err(e)
        }
    }
}

fn write_bundle(
    conn: &Connection,
    farm_dir: &Path,
    bundle_dir: &Path,
    exported_at: &str,
    watermark: i64,
    live_max_seq: i64,
    flush_lag: i64,
) -> Result<ExportResult, String> {
    let mut files: Vec<ManifestFile> = Vec::new();

    // farm.db
    let farm_db_dest = bundle_dir.join("farm.db");
    let farm_db_dest_str = farm_db_dest
        .to_str()
        .ok_or_else(|| "bundle path is not valid UTF-8".to_string())?;
    // VACUUM INTO is WAL-safe and writes a clean single file. snapshots.rs
    // uses the same call but then appends a snapshot.taken event; export must
    // not, because export does not mutate the ledger.
    conn.execute("VACUUM INTO ?1", params![farm_db_dest_str])
        .map_err(|e| e.to_string())?;
    // EXPORT B (C1): the bundle travels; the two connection secrets stay home.
    // NULLed in this copy only, before the manifest hashes it. The live farm.db
    // is not opened for write here. See export_scrub.rs.
    export_scrub::scrub_bundle_copy(&farm_db_dest)?;
    files.push(manifest_file_for_path(bundle_dir, Path::new("farm.db"))?);

    // events.jsonl — plain byte copy; empty file if source is absent.
    let src_events = event_file::events_path(farm_dir);
    let dest_events = bundle_dir.join("events.jsonl");
    if src_events.exists() {
        let bytes = fs::read(&src_events).map_err(|e| e.to_string())?;
        fs::write(&dest_events, &bytes).map_err(|e| e.to_string())?;
    } else {
        fs::write(&dest_events, b"").map_err(|e| e.to_string())?;
    }
    files.push(manifest_file_for_path(
        bundle_dir,
        Path::new("events.jsonl"),
    )?);

    // receipts/
    let receipts_count = copy_receipts(farm_dir, bundle_dir, &mut files)?;

    // costs.csv
    // amount_cents stays integer cents, as every other artifact in this repo
    // does. A second dollars column could disagree with this one, and two
    // money fields that can disagree is the thing BOOKS-BOUNDARY refuses.
    // origin is omitted because this file is filtered to farm_os and the
    // manifest states it; a constant column is noise.
    // quantity, unit_price_cents, delivery_date and invoice_reference are
    // omitted because costs.rs writes NULL for all four on every row it has
    // ever created. They remain in farm.db and events.jsonl if ever populated.
    let cost_rows = write_costs_csv(conn, bundle_dir)?;
    files.push(manifest_file_for_path(bundle_dir, Path::new("costs.csv"))?);

    // income.csv — recorded farm_os income plus paid Stripe orders
    let income_export = write_income_csv(conn, bundle_dir)?;
    files.push(manifest_file_for_path(bundle_dir, Path::new("income.csv"))?);

    // assets.csv — live farm_os only
    let (asset_rows, assets_excluded_voided) = write_assets_csv(conn, bundle_dir)?;
    files.push(manifest_file_for_path(bundle_dir, Path::new("assets.csv"))?);

    // mileage.csv — live farm_os only; miles only
    let (mileage_rows, mileage_excluded_voided) = write_mileage_csv(conn, bundle_dir)?;
    files.push(manifest_file_for_path(
        bundle_dir,
        Path::new("mileage.csv"),
    )?);

    // marketing.csv — one flat row per marketing event; no money column
    let marketing_rows = write_marketing_csv(conn, bundle_dir)?;
    files.push(manifest_file_for_path(
        bundle_dir,
        Path::new("marketing.csv"),
    )?);

    // wholesale.csv — one row per order LINE, including delivered-but-unpaid
    let _wholesale_rows = write_wholesale_csv(conn, bundle_dir)?;
    files.push(manifest_file_for_path(
        bundle_dir,
        Path::new("wholesale.csv"),
    )?);

    // receivables.csv — same COLLECT answer as the Today card
    let receivables_as_of = db::local_date_today();
    let receivables = write_receivables_csv(conn, bundle_dir, &receivables_as_of)?;
    files.push(manifest_file_for_path(
        bundle_dir,
        Path::new("receivables.csv"),
    )?);

    // money-corrections.csv — permanent trail for the tax-prep export
    let money_correction_rows = write_money_corrections_csv(conn, bundle_dir)?;
    files.push(manifest_file_for_path(
        bundle_dir,
        Path::new("money-corrections.csv"),
    )?);

    // income-corrections.csv — void/correction trail derived from event_log
    let income_corr = write_income_corrections_csv(conn, bundle_dir)?;
    files.push(manifest_file_for_path(
        bundle_dir,
        Path::new("income-corrections.csv"),
    )?);

    // write-offs.csv — allowance trail derived from wholesale_write_offs
    let _write_off_rows = write_write_offs_csv(conn, bundle_dir)?;
    files.push(manifest_file_for_path(
        bundle_dir,
        Path::new("write-offs.csv"),
    )?);

    // categories.json
    let categories_json = serde_json::to_string_pretty(&categories::export_categories())
        .map_err(|e| e.to_string())?;
    let categories_bytes = format!("{categories_json}\n");
    fs::write(
        bundle_dir.join("categories.json"),
        categories_bytes.as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    files.push(manifest_file_for_path(
        bundle_dir,
        Path::new("categories.json"),
    )?);

    let event_log_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let commercial_app_records: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE origin = 'commercial_app'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let consumption_events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM consumption_events WHERE origin = 'farm_os'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    if cost_rows != count_farm_os_costs(conn)? {
        return Err("costs.csv row count disagrees with cost_events".into());
    }

    let counts = ManifestCounts {
        event_log_rows,
        commercial_app_records,
        cost_events: cost_rows,
        consumption_events,
        mileage_trips: mileage_rows,
        mileage_trips_excluded_voided: mileage_excluded_voided,
        assets: asset_rows,
        assets_excluded_voided,
        income_events: income_export.recorded_rows,
        income_events_excluded_voided: income_export.excluded_voided,
        stripe_orders_paid: income_export.stripe_paid_rows,
        stripe_orders_excluded_not_paid: income_export.excluded_not_paid,
        receipts: receipts_count,
        marketing_events: marketing_rows,
        money_corrections: money_correction_rows,
        receivables_orders: receivables.orders,
        receivables_total_cents: receivables.total_cents,
        receivables_orders_partial: receivables.partial_orders,
        receivables_as_of,
        income_corrections: income_corr.rows,
        income_corrections_unknown_before: income_corr.unknown_before,
    };

    let notes = build_notes(&counts);

    files.sort_by(|a, b| a.path.cmp(&b.path));

    let manifest = Manifest {
        manifest_version: 1,
        app_schema_version: db::SCHEMA_VERSION,
        exported_at: exported_at.to_string(),
        origin: "farm_os".into(),
        event_log_watermark: watermark,
        live_max_seq,
        flush_lag,
        files: files.clone(),
        counts,
        notes,
    };

    // manifest.json — written LAST, after every other file exists.
    let manifest_json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    fs::write(
        bundle_dir.join("manifest.json"),
        format!("{manifest_json}\n").as_bytes(),
    )
    .map_err(|e| e.to_string())?;

    let total_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
    let bundle_path = bundle_dir
        .to_str()
        .ok_or_else(|| "bundle path is not valid UTF-8".to_string())?
        .to_string();

    Ok(ExportResult {
        bundle_path,
        file_count: files.len() as i64,
        total_bytes,
        exported_at: exported_at.to_string(),
    })
}

fn build_notes(counts: &ManifestCounts) -> Vec<String> {
    vec![
        format!(
            "costs.csv carries, for every payment, a Schedule F line, a Schedule C line, \
             a descriptor and the receipt file reference, so no field needs re-typing at \
             handoff; descriptor is mandatory where either line is an \"other\" line"
        ),
        format!("amounts in costs.csv are integer cents"),
        format!(
            "derived cost per tray is deliberately absent because it is a pure function \
             of these records plus a query-time window, and is never stored as a fact"
        ),
        format!("mileage is recorded in miles and carries no dollar value"),
        format!(
            "{} voided mileage trips and {} voided assets were withdrawn by the operator \
             and are excluded from the CSVs; the full history remains in events.jsonl",
            counts.mileage_trips_excluded_voided, counts.assets_excluded_voided
        ),
        // C3 (INT-003): the note says what the log holds. Imported
        // commercial_app expense records are re-exported in events.jsonl
        // (marked, out of every total and CSV), so a farm that holds any is
        // not farm_os-only and does not claim to be.
        if counts.commercial_app_records == 0 {
            "this bundle contains only farm_os originated records".to_string()
        } else {
            format!(
                "this bundle contains {} commercial_app originated record(s) beside the \
                 farm_os records: another system's expense records, marked, never \
                 editable, and excluded from every total and CSV",
                counts.commercial_app_records
            )
        },
        format!(
            "income.csv carries both manually recorded income and paid Stripe orders, \
             distinguished by record_type, so no dollar is counted twice"
        ),
        format!(
            "{} voided income records were excluded from income.csv; \
             income-corrections.csv is the CSV trail for those voids",
            counts.income_events_excluded_voided
        ),
        format!(
            "{} refunded or disputed Stripe orders were excluded from income.csv; \
             there is no CSV trail for these — their stripe.refunded and \
             stripe.disputed events in events.jsonl carry the refund or dispute id, \
             the order ids and the amount in cents",
            counts.stripe_orders_excluded_not_paid
        ),
        format!(
            "marketing.csv carries {} marketing events (venues, samples, touches, \
             follow-ups) with no money column",
            counts.marketing_events
        ),
        format!(
            "wholesale.csv carries every order line including delivered-but-unpaid; \
             amounts are integer cents; payment amounts live in income.csv via income_event_id"
        ),
        format!(
            "receivables.csv carries every delivered-but-unpaid order with its age in \
             days as of {}; amounts are integer cents and count priced lines only",
            counts.receivables_as_of
        ),
        format!(
            "{} of those orders have at least one unpriced line, so their amount is \
             partial and marked amount_is_partial=true; the {} total counts only \
             fully-priced orders",
            counts.receivables_orders_partial, counts.receivables_total_cents
        ),
        format!(
            "income-corrections.csv is the income void and correction trail, symmetric \
             with money-corrections.csv; income corrections carry no reason field, so \
             that column is always empty. {} rows could not resolve a before-image and \
             leave those fields empty rather than guessing",
            counts.income_corrections_unknown_before
        ),
        format!(
            "write-offs.csv is the allowance trail: an order settled for less than its \
             priced total records the difference here with its category and, when the \
             category is Other, the operator's reason. It is not income and not an \
             expense — it is the money that was owed and forgiven, kept so the gap \
             between an order's price and its payment can be explained."
        ),
        // EXPORT B (C1): says what this bundle's farm.db does not carry.
        export_scrub::MANIFEST_NOTE.to_string(),
    ]
}

fn unique_bundle_dir(exports_root: &Path, stamp: &str) -> Result<PathBuf, String> {
    let mut path = exports_root.join(format!("export-{stamp}"));
    let mut n = 1u32;
    while path.exists() {
        path = exports_root.join(format!("export-{stamp}-{n}"));
        n += 1;
        if n > 10_000 {
            return Err("could not find a unique export folder name".into());
        }
    }
    Ok(path)
}

/// Format `YYYY-MM-DD-HHMMSS` from an RFC3339 UTC timestamp (the single clock read).
fn stamp_from_rfc3339(exported_at: &str) -> Result<String, String> {
    // Expected shape: 2026-08-09T08:48:00.123Z (millis, Zulu).
    if exported_at.len() < 19 || !exported_at.as_bytes().get(10).copied().eq(&Some(b'T')) {
        return Err(format!("exported_at is not RFC3339: {exported_at}"));
    }
    let date = &exported_at[0..10];
    let hour = &exported_at[11..13];
    let min = &exported_at[14..16];
    let sec = &exported_at[17..19];
    for part in [date, hour, min, sec] {
        if !part.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
            return Err(format!("exported_at is not RFC3339: {exported_at}"));
        }
    }
    Ok(format!("{date}-{hour}{min}{sec}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn manifest_file_for_path(bundle_dir: &Path, rel: &Path) -> Result<ManifestFile, String> {
    let abs = bundle_dir.join(rel);
    let bytes = fs::read(&abs).map_err(|e| e.to_string())?;
    let path = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    Ok(ManifestFile {
        path,
        size_bytes: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    })
}

fn copy_receipts(
    farm_dir: &Path,
    bundle_dir: &Path,
    files: &mut Vec<ManifestFile>,
) -> Result<i64, String> {
    let dest_dir = bundle_dir.join("receipts");
    fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let src_dir = farm_dir.join("receipts");
    if !src_dir.exists() {
        return Ok(0);
    }

    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(&src_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("receipt name is not valid UTF-8: {}", path.display()))?
            .to_string();
        names.push(name);
    }
    names.sort();

    let mut count = 0i64;
    for name in names {
        let src = src_dir.join(&name);
        let dest = dest_dir.join(&name);
        let bytes = fs::read(&src).map_err(|e| format!("could not read receipt {name}: {e}"))?;
        let src_digest = sha256_hex(&bytes);

        // Content-addressed stem check when the stem is 64 lowercase hex chars.
        if let Some(stem) = Path::new(&name).file_stem().and_then(|s| s.to_str()) {
            if stem.len() == 64
                && stem.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                && stem != src_digest
            {
                return Err(format!(
                    "receipt {name} is corrupted: filename stem does not match content digest"
                ));
            }
        }

        {
            let mut f =
                File::create(&dest).map_err(|e| format!("could not write receipt {name}: {e}"))?;
            f.write_all(&bytes)
                .map_err(|e| format!("could not write receipt {name}: {e}"))?;
        }
        let dest_bytes =
            fs::read(&dest).map_err(|e| format!("could not re-read receipt {name}: {e}"))?;
        let dest_digest = sha256_hex(&dest_bytes);
        if dest_digest != src_digest {
            return Err(format!("receipt {name} failed integrity check after copy"));
        }

        let rel = format!("receipts/{name}");
        files.push(ManifestFile {
            path: rel,
            size_bytes: dest_bytes.len() as u64,
            sha256: dest_digest,
        });
        count += 1;
    }
    Ok(count)
}

struct IncomeCsvCounts {
    recorded_rows: i64,
    excluded_voided: i64,
    stripe_paid_rows: i64,
    excluded_not_paid: i64,
}

fn write_income_csv(conn: &Connection, bundle_dir: &Path) -> Result<IncomeCsvCounts, String> {
    let excluded_voided: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM income_events
             WHERE origin = 'farm_os' AND voided_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    let excluded_not_paid: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM orders WHERE state <> 'paid'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    let rows = crate::income::cash_rows_between(conn, None, None)?;
    let recorded_rows = rows.iter().filter(|r| r.record_type == "recorded").count() as i64;
    let stripe_paid_rows = rows.iter().filter(|r| r.record_type == "stripe").count() as i64;

    let mut out = String::new();
    out.push_str(
        "record_type,income_id,date_received,amount_cents,source,canonical_category,schedule_f_line,schedule_c_line,descriptor,receipt_file_ref\n",
    );
    for row in &rows {
        out.push_str(&csv_line(&[
            CsvField::Text(&row.record_type),
            CsvField::Text(&row.income_id),
            CsvField::Text(&row.date_received),
            CsvField::Int(row.amount_cents),
            CsvField::Text(&row.source),
            CsvField::Text(&row.canonical_category),
            CsvField::Text(&row.schedule_f_line),
            CsvField::Text(&row.schedule_c_line),
            CsvField::Text(&row.descriptor),
            CsvField::Text(&row.receipt_file_ref),
        ]));
    }
    fs::write(bundle_dir.join("income.csv"), out.as_bytes()).map_err(|e| e.to_string())?;

    Ok(IncomeCsvCounts {
        recorded_rows,
        excluded_voided,
        stripe_paid_rows,
        excluded_not_paid,
    })
}

fn write_costs_csv(conn: &Connection, bundle_dir: &Path) -> Result<i64, String> {
    let mut rows = crate::costs::expense_rows_between(conn, None, None)?;
    rows.sort_by(|a, b| (&a.date_paid, &a.event_id).cmp(&(&b.date_paid, &b.event_id)));

    let mut out = String::new();
    out.push_str(
        "event_id,date_paid,amount_cents,payee,canonical_category,schedule_f_line,schedule_c_line,descriptor,receipt_file_ref\n",
    );
    let mut count = 0i64;
    // E4-a signed 2026-08-23 — these two columns are the values recorded on the
    // event, never today's COST_CATEGORIES mapping. A line number that changes
    // in source must not rewrite what a past year already reported.
    for row in rows {
        let receipt = row.receipt_file_ref.clone().unwrap_or_default();
        out.push_str(&csv_line(&[
            CsvField::Text(&row.event_id),
            CsvField::Text(&row.date_paid),
            CsvField::Int(row.amount_cents),
            CsvField::Text(&row.payee),
            CsvField::Text(&row.canonical_category),
            CsvField::Text(&row.schedule_f_line),
            CsvField::Text(&row.schedule_c_line),
            CsvField::Text(&row.descriptor),
            CsvField::Text(&receipt),
        ]));
        count += 1;
    }
    fs::write(bundle_dir.join("costs.csv"), out.as_bytes()).map_err(|e| e.to_string())?;
    Ok(count)
}

fn write_assets_csv(conn: &Connection, bundle_dir: &Path) -> Result<(i64, i64), String> {
    let excluded: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM assets
             WHERE origin = 'farm_os' AND voided_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT asset_id, description, placed_in_service_on, cost_cents, disposal_date
             FROM assets
             WHERE origin = 'farm_os' AND voided_at IS NULL
             ORDER BY placed_in_service_on, asset_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str("asset_id,description,placed_in_service_on,cost_cents,disposal_date\n");
    let mut count = 0i64;
    for row in rows {
        let (asset_id, description, placed, cost_cents, disposal) =
            row.map_err(|e| e.to_string())?;
        out.push_str(&csv_line(&[
            CsvField::Text(&asset_id),
            CsvField::Text(&description),
            CsvField::Text(&placed),
            CsvField::Int(cost_cents),
            CsvField::OptText(disposal.as_deref()),
        ]));
        count += 1;
    }
    fs::write(bundle_dir.join("assets.csv"), out.as_bytes()).map_err(|e| e.to_string())?;
    Ok((count, excluded))
}

fn write_mileage_csv(conn: &Connection, bundle_dir: &Path) -> Result<(i64, i64), String> {
    let excluded: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mileage_trips
             WHERE origin = 'farm_os' AND voided_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT trip_id, trip_date, miles, purpose
             FROM mileage_trips
             WHERE origin = 'farm_os' AND voided_at IS NULL
             ORDER BY trip_date, trip_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str("trip_id,trip_date,miles,purpose\n");
    let mut count = 0i64;
    for row in rows {
        let (trip_id, trip_date, miles, purpose) = row.map_err(|e| e.to_string())?;
        let miles_s = miles.to_string();
        out.push_str(&csv_line(&[
            CsvField::Text(&trip_id),
            CsvField::Text(&trip_date),
            CsvField::Text(&miles_s),
            CsvField::OptText(purpose.as_deref()),
        ]));
        count += 1;
    }
    fs::write(bundle_dir.join("mileage.csv"), out.as_bytes()).map_err(|e| e.to_string())?;
    Ok((count, excluded))
}

/// One flat row per marketing event_log row. No money column.
fn write_marketing_csv(conn: &Connection, bundle_dir: &Path) -> Result<i64, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, entity_type, entity_id, payload, created_at
             FROM event_log
             WHERE event_domain = 'marketing'
             ORDER BY seq",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str("event_id,kind,entity_type,entity_id,payload,created_at\n");
    let mut count = 0i64;
    for row in rows {
        let (event_id, kind, entity_type, entity_id, payload, created_at) =
            row.map_err(|e| e.to_string())?;
        out.push_str(&csv_line(&[
            CsvField::Text(&event_id),
            CsvField::Text(&kind),
            CsvField::Text(&entity_type),
            CsvField::Text(&entity_id),
            CsvField::Text(&payload),
            CsvField::Text(&created_at),
        ]));
        count += 1;
    }
    fs::write(bundle_dir.join("marketing.csv"), out.as_bytes()).map_err(|e| e.to_string())?;
    Ok(count)
}

/// One row per wholesale order line. Payment amounts live in income.csv.
fn write_wholesale_csv(conn: &Connection, bundle_dir: &Path) -> Result<i64, String> {
    let mut stmt = conn
        .prepare(
            "SELECT o.id, v.name, o.harvest_date, o.state, o.ordered_on,
                    o.delivered_on, o.paid_on, o.income_event_id, o.voided_at,
                    o.void_reason, c.name, l.trays, l.price_cents_per_tray
             FROM wholesale_order_lines l
             JOIN wholesale_orders o ON o.id = l.order_id
             JOIN mkt_venues v ON v.venue_id = o.venue_id
             JOIN crops c ON c.id = l.crop_id
             ORDER BY o.ordered_on, o.id, c.sort_order, l.crop_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, i64>(11)?,
                r.get::<_, Option<i64>>(12)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str(
        "order_id,venue_name,harvest_date,state,ordered_on,delivered_on,paid_on,income_event_id,voided_at,void_reason,crop_name,trays,price_cents_per_tray\n",
    );
    let mut count = 0i64;
    for row in rows {
        let (
            order_id,
            venue_name,
            harvest_date,
            state,
            ordered_on,
            delivered_on,
            paid_on,
            income_event_id,
            voided_at,
            void_reason,
            crop_name,
            trays,
            price_cents_per_tray,
        ) = row.map_err(|e| e.to_string())?;
        let price_s = price_cents_per_tray.map(|n| n.to_string());
        out.push_str(&csv_line(&[
            CsvField::Text(&order_id),
            CsvField::Text(&venue_name),
            CsvField::Text(&harvest_date),
            CsvField::Text(&state),
            CsvField::Text(&ordered_on),
            CsvField::OptText(delivered_on.as_deref()),
            CsvField::OptText(paid_on.as_deref()),
            CsvField::OptText(income_event_id.as_deref()),
            CsvField::OptText(voided_at.as_deref()),
            CsvField::OptText(void_reason.as_deref()),
            CsvField::Text(&crop_name),
            CsvField::Int(trays),
            CsvField::OptText(price_s.as_deref()),
        ]));
        count += 1;
    }
    fs::write(bundle_dir.join("wholesale.csv"), out.as_bytes()).map_err(|e| e.to_string())?;
    Ok(count)
}

struct ReceivablesCounts {
    orders: i64,
    total_cents: i64,
    partial_orders: i64,
}

/// B8 — what is owed, straight from THE evaluator. attention::money_debts_on
/// already answers "delivered and unpaid, how much, how old" for the Today
/// COLLECT card; this file is that same answer written down. A second SUM here
/// could disagree with the card, so there is not one.
///
/// Unpriced lines never invent a number. `cents` sums priced lines only, so an
/// order with any unpriced line is a PARTIAL total and says so; an order whose
/// priced lines total nothing leaves amount_cents EMPTY rather than writing 0,
/// because 0 in a receivables column reads as "nothing owed".
fn write_receivables_csv(
    conn: &Connection,
    bundle_dir: &Path,
    today: &str,
) -> Result<ReceivablesCounts, String> {
    let debts = attention::money_debts_on(conn, today)?;

    let mut out = String::new();
    out.push_str(
        "order_id,venue_name,delivered_on,days_outstanding,amount_cents,amount_is_partial\n",
    );
    let mut orders = 0i64;
    let mut total_cents = 0i64;
    let mut partial_orders = 0i64;
    for debt in &debts.collect {
        let amount_s = if debt.any_unpriced && debt.cents == 0 {
            String::new()
        } else {
            debt.cents.to_string()
        };
        // Same rule as amount_s above: leave the column EMPTY rather than write a
        // number that is not true. 0 in a days column reads as "delivered today".
        let days_s = if debt.age_countable {
            debt.days.to_string()
        } else {
            String::new()
        };
        let partial = if debt.any_unpriced { "true" } else { "false" };
        out.push_str(&csv_line(&[
            CsvField::Text(&debt.order_id),
            CsvField::Text(&debt.venue_name),
            CsvField::Text(&debt.delivered_on),
            CsvField::Text(&days_s),
            CsvField::Text(&amount_s),
            CsvField::Text(partial),
        ]));
        orders += 1;
        if debt.any_unpriced {
            partial_orders += 1;
        } else {
            total_cents += debt.cents;
        }
    }
    fs::write(bundle_dir.join("receivables.csv"), out.as_bytes()).map_err(|e| e.to_string())?;
    Ok(ReceivablesCounts {
        orders,
        total_cents,
        partial_orders,
    })
}

fn count_farm_os_costs(conn: &Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM cost_events WHERE origin = 'farm_os' AND voided_at IS NULL",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn write_money_corrections_csv(conn: &Connection, bundle_dir: &Path) -> Result<i64, String> {
    let mut stmt = conn
        .prepare(
            "SELECT correction_event_id, target_event_id, track, action,
                    before_amount_cents, after_amount_cents, before_date, after_date,
                    before_payee, after_payee, reason, corrected_at
             FROM money_corrections
             ORDER BY corrected_at, correction_event_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, String>(11)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str(
        "correction_event_id,target_event_id,track,action,before_amount_cents,after_amount_cents,before_date,after_date,before_payee,after_payee,reason,corrected_at\n",
    );
    let mut count = 0i64;
    for row in rows {
        let (
            correction_event_id,
            target_event_id,
            track,
            action,
            before_amount_cents,
            after_amount_cents,
            before_date,
            after_date,
            before_payee,
            after_payee,
            reason,
            corrected_at,
        ) = row.map_err(|e| e.to_string())?;
        let after_amt = after_amount_cents
            .map(|n| n.to_string())
            .unwrap_or_default();
        let after_d = after_date.unwrap_or_default();
        let after_p = after_payee.unwrap_or_default();
        let why = reason.unwrap_or_default();
        out.push_str(&csv_line(&[
            CsvField::Text(&correction_event_id),
            CsvField::Text(&target_event_id),
            CsvField::Text(&track),
            CsvField::Text(&action),
            CsvField::Int(before_amount_cents),
            CsvField::Text(&after_amt),
            CsvField::Text(&before_date),
            CsvField::Text(&after_d),
            CsvField::Text(&before_payee),
            CsvField::Text(&after_p),
            CsvField::Text(&why),
            CsvField::Text(&corrected_at),
        ]));
        count += 1;
    }
    fs::write(bundle_dir.join("money-corrections.csv"), out.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(count)
}

struct IncomeCorrectionCounts {
    rows: i64,
    unknown_before: i64,
}

/// B8 — the income void/correction trail as CSV, symmetric with
/// money-corrections.csv so the two files can be read the same way.
///
/// No new table and no payload change: correct_income and void_income already
/// set reverses_event_id to the event they supersede, and every income payload
/// carries the full state (build_income_payload). The before-image is therefore
/// already in the log — §8's complaint was that it was only in events.jsonl,
/// not that it was missing. This reads it out.
fn write_income_corrections_csv(
    conn: &Connection,
    bundle_dir: &Path,
) -> Result<IncomeCorrectionCounts, String> {
    let mut stmt = conn
        .prepare(
            "SELECT e.id, e.kind, e.entity_id, e.created_at, e.payload,
                    p.payload AS before_payload
               FROM event_log e
               LEFT JOIN event_log p ON p.id = e.reverses_event_id
              WHERE e.kind IN ('income.corrected','income.voided')
              ORDER BY e.seq",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str(
        "correction_event_id,target_income_id,track,action,before_amount_cents,after_amount_cents,before_date,after_date,before_source,after_source,reason,corrected_at\n",
    );
    let mut count = 0i64;
    let mut unknown_before = 0i64;
    for row in rows {
        let (event_id, kind, entity_id, created_at, payload_text, before_text) =
            row.map_err(|e| e.to_string())?;
        let action = match kind.as_str() {
            "income.corrected" => "corrected",
            "income.voided" => "voided",
            other => {
                return Err(format!("unexpected income correction kind: {other}"));
            }
        };

        let (before_amount, before_date, before_source, before_unknown) =
            income_before_fields(before_text.as_deref());
        if before_unknown {
            unknown_before += 1;
        }

        let (after_amount, after_date, after_source) = if action == "voided" {
            (None, None, None)
        } else {
            let after: serde_json::Value = serde_json::from_str(&payload_text)
                .map_err(|e| format!("income correction payload is not JSON: {e}"))?;
            (
                json_opt_i64(&after, "amountCents"),
                json_opt_str(&after, "dateReceived"),
                json_opt_str(&after, "source"),
            )
        };

        let before_amt_s = before_amount.map(|n| n.to_string()).unwrap_or_default();
        let after_amt_s = after_amount.map(|n| n.to_string()).unwrap_or_default();
        let before_d = before_date.unwrap_or_default();
        let after_d = after_date.unwrap_or_default();
        let before_s = before_source.unwrap_or_default();
        let after_s = after_source.unwrap_or_default();
        out.push_str(&csv_line(&[
            CsvField::Text(&event_id),
            CsvField::Text(&entity_id),
            CsvField::Text("income"),
            CsvField::Text(action),
            CsvField::Text(&before_amt_s),
            CsvField::Text(&after_amt_s),
            CsvField::Text(&before_d),
            CsvField::Text(&after_d),
            CsvField::Text(&before_s),
            CsvField::Text(&after_s),
            CsvField::Text(""),
            CsvField::Text(&created_at),
        ]));
        count += 1;
    }
    fs::write(bundle_dir.join("income-corrections.csv"), out.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(IncomeCorrectionCounts {
        rows: count,
        unknown_before,
    })
}

/// Allowance trail derived from wholesale_write_offs. Restore replays
/// wholesale.write_off events; this file is not read back by import.
fn write_write_offs_csv(conn: &Connection, bundle_dir: &Path) -> Result<i64, String> {
    let sql = format!(
        "SELECT w.written_off_on, w.order_id, v.name, w.shortfall_cents,
                w.category, w.reason, w.income_event_id
         FROM wholesale_write_offs w
         JOIN wholesale_orders o ON o.id = w.order_id
         JOIN mkt_venues v ON v.venue_id = o.venue_id
         WHERE {}
         ORDER BY w.written_off_on, w.order_id",
        crate::wholesale::WRITE_OFF_ORDER_STATE_SQL
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str("written_off_on,order_id,venue,shortfall_cents,category,reason,income_event_id\n");
    let mut count = 0i64;
    for row in rows {
        let (written_off_on, order_id, venue, shortfall_cents, category, reason, income_event_id) =
            row.map_err(|e| e.to_string())?;
        out.push_str(&csv_line(&[
            CsvField::Text(&written_off_on),
            CsvField::Text(&order_id),
            CsvField::Text(&venue),
            CsvField::Int(shortfall_cents),
            CsvField::Text(&category),
            CsvField::OptText(reason.as_deref()),
            CsvField::Text(&income_event_id),
        ]));
        count += 1;
    }
    fs::write(bundle_dir.join("write-offs.csv"), out.as_bytes()).map_err(|e| e.to_string())?;
    Ok(count)
}

fn income_before_fields(
    before_text: Option<&str>,
) -> (Option<i64>, Option<String>, Option<String>, bool) {
    let Some(text) = before_text else {
        return (None, None, None, true);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return (None, None, None, true);
    };
    let amount = json_opt_i64(&v, "amountCents");
    let date = json_opt_str(&v, "dateReceived");
    let source = json_opt_str(&v, "source");
    let unknown = amount.is_none() || date.is_none() || source.is_none();
    (amount, date, source, unknown)
}

fn json_opt_i64(v: &serde_json::Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| x.as_i64())
}

fn json_opt_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

enum CsvField<'a> {
    Text(&'a str),
    OptText(Option<&'a str>),
    Int(i64),
}

fn csv_line(fields: &[CsvField<'_>]) -> String {
    let mut parts = Vec::with_capacity(fields.len());
    for f in fields {
        let s = match f {
            CsvField::Text(t) => csv_escape(t),
            CsvField::OptText(None) => String::new(),
            CsvField::OptText(Some(t)) => csv_escape(t),
            CsvField::Int(n) => n.to_string(),
        };
        parts.push(s);
    }
    let mut line = parts.join(",");
    line.push('\n');
    line
}

/// RFC 4180 escaper shared by all three CSV files. Line terminator is `\n`.
fn csv_escape(field: &str) -> String {
    let needs_quotes = field
        .bytes()
        .any(|b| b == b',' || b == b'"' || b == b'\n' || b == b'\r');
    if !needs_quotes {
        return field.to_string();
    }
    let mut out = String::with_capacity(field.len() + 2);
    out.push('"');
    for ch in field.chars() {
        if ch == '"' {
            out.push('"');
            out.push('"');
        } else {
            out.push(ch);
        }
    }
    out.push('"');
    out
}
