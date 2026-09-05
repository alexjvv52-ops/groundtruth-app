//! Import — keyed on event_id, idempotent, refusing (Track 7).
//!
//! Authority: ROADMAP Track 7 done-whens; BOOKS-BOUNDARY outranks.
//! events.jsonl is authoritative. farm.db in the bundle is state and is
//! verified against the log; where they disagree the import halts and says so.
//! There is no UPSERT, no ON CONFLICT, no merge and no overwrite anywhere in
//! this module. A conflict stops the import and is reported to the operator.
//! Origin is absolute: farm_os records are the grower's own; commercial_app
//! records arrive marked, are never editable, and are excluded from totals —
//! which is why the door admits another system's `cost.money_out` and nothing
//! else from it (C3, INT-003): that is the one kind whose projection carries
//! the origin and whose totals filter on it. Any other kind from another
//! system would be counted as this farm's own, so it is refused whole.
//! Two farms are never combined.
//! The source bundle is read-only. Nothing is written into it, ever.

use crate::db;
use crate::event_partition::EventClass;
use crate::events::{self, EventRecord};
use crate::export::Manifest;
use crate::projection::{self, VerifyOutcome};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ImportRefusal {
    MissingEventId {
        line_no: usize,
    },
    FarmOsConflict {
        event_id: String,
        field: String,
        in_this_farm: String,
        in_the_bundle: String,
    },
    CommercialClaimingFarmOs {
        event_id: String,
        detail: String,
    },
    /// C3 (INT-003): a commercial_app record of a kind this app cannot keep
    /// marked and out of totals. Only `cost.money_out` is carried from
    /// another system.
    ForeignKindNotCarried {
        event_id: String,
        event_kind: String,
    },
    DifferentFarm {
        farm_records_here: i64,
        events_in_bundle: i64,
    },
    LogVersusDatabase {
        detail: String,
    },
    ManifestMismatch {
        path: String,
        detail: String,
    },
    SchemaVersion {
        bundle: i32,
        this_app: i32,
    },
    Malformed {
        line_no: usize,
        detail: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub bundle_path: String,
    pub bundle_exported_at: String,
    pub events_in_bundle: i64,
    pub shared_event_ids: i64,
    pub already_present_identical: i64,
    pub would_be_added: i64,
    pub foreign_records_in_bundle: i64,
    pub refusals: Vec<ImportRefusal>,
    /// True only when refusals is empty. The UI must not offer Apply otherwise.
    pub can_apply: bool,
    /// Generated sentences, one per refusal, in plain operator language.
    pub explanations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub events_added: i64,
    pub events_skipped_identical: i64,
    pub foreign_records_added: i64,
    pub receipts_copied: i64,
}

/// Preview an import. Takes `&Connection` — writes nothing durable anywhere
/// (the verify scratch under the live farm's `snapshots/` is removed per run).
pub fn preview_import(conn: &Connection, bundle_dir: &Path) -> Result<ImportPlan, String> {
    build_plan(conn, bundle_dir)
}

/// Apply an import. Re-runs the full preview first; refuses rather than partial.
/// Ordinary path — and, since the freeze retired (identity::CUTOVER_LIVE),
/// the one import door: farm truth is admitted whenever the plan is clean.
/// The lock_is_active clause in apply_import_inner is inert.
pub fn apply_import(conn: &mut Connection, bundle_dir: &Path) -> Result<ImportResult, String> {
    apply_import_inner(conn, bundle_dir, false)
}

/// The one-shot cutover import. Requires the operator to type CUTOVER. Still
/// registered (commands::apply_cutover_import) though the desk no longer
/// offers it (Settings fence 1, S5). Differs from apply_import only by the
/// typed word, the has-farm-truth check and the receipt; with the freeze
/// retired it admits nothing apply_import refuses.
pub fn apply_cutover_import(
    conn: &mut Connection,
    bundle_dir: &Path,
    confirmation: &str,
) -> Result<ImportResult, String> {
    if confirmation != "CUTOVER" {
        return Err("Type CUTOVER exactly to bring in the Farm OS cutover bundle.".to_string());
    }
    // C3 (INT-009): one-shot in fact, not only in name. The receipt is the
    // gate — a farm that already holds cutover-import.txt refuses a second
    // cutover, and the receipt is never overwritten. The farm directory is
    // resolved before anything is applied, so a farm with no on-disk
    // directory refuses here instead of after its events were committed.
    let farm_dir = target_farm_dir(conn).ok_or_else(|| {
        "Cutover import needs an on-disk farm directory for the receipt.".to_string()
    })?;
    let receipt_path = farm_dir.join("cutover-import.txt");
    if receipt_path.is_file() {
        return Err(
            "This farm already brought in its cutover bundle; the receipt is \
             cutover-import.txt. A cutover happens once — use Bring in a bundle \
             for anything later."
                .to_string(),
        );
    }
    let plan = preview_import(conn, bundle_dir)?;
    if !plan.can_apply || !plan.refusals.is_empty() {
        return Err("Cutover import refused: the bundle preview is not clean.".to_string());
    }
    let events_path = bundle_dir.join("events.jsonl");
    let records = parse_bundle_records(&events_path)?;
    if !records
        .iter()
        .any(|r| crate::identity::is_farm_truth(r.kind))
    {
        return Err("This bundle has no farm truth. Use Bring in a bundle instead.".to_string());
    }
    let result = apply_import_inner(conn, bundle_dir, true)?;
    write_cutover_receipt(&receipt_path, bundle_dir, &result)?;
    Ok(result)
}

fn apply_import_inner(
    conn: &mut Connection,
    bundle_dir: &Path,
    allow_farm_truth: bool,
) -> Result<ImportResult, String> {
    let plan = build_plan(conn, bundle_dir)?;
    if !plan.can_apply {
        let msg = plan
            .explanations
            .first()
            .cloned()
            .unwrap_or_else(|| "Import refused.".into());
        return Err(msg);
    }

    // Receipts land before the transaction: a receipt failure must leave the
    // ledger untouched. A later transaction failure leaves copied files that
    // are correct on the next attempt.
    let mut receipts_copied = 0i64;
    if let Some(farm_dir) = target_farm_dir(conn) {
        receipts_copied = copy_receipts_into_farm(bundle_dir, &farm_dir)?;
    }

    let events_path = bundle_dir.join("events.jsonl");
    let records = parse_bundle_records(&events_path)?;
    // Re-derive which ids are already present (identical) — only add the rest.
    let mut to_add: Vec<EventRecord> = Vec::new();
    let mut skipped = 0i64;
    let mut foreign_added = 0i64;
    for rec in records {
        match lookup_existing(conn, &rec.event_id)? {
            None => {
                if rec.origin == "commercial_app" {
                    foreign_added += 1;
                }
                to_add.push(rec);
            }
            Some(_) => {
                skipped += 1;
            }
        }
    }

    // Retired freeze gate: lock_is_active is always false
    // (identity::CUTOVER_LIVE), so this refusal is never returned. Kept for
    // the cleanup; the sentence is kept verbatim.
    if !allow_farm_truth
        && crate::identity::lock_is_active()
        && to_add
            .iter()
            .any(|r| crate::identity::is_farm_truth(r.kind))
    {
        return Err(
            "Farm truth lives in Farm OS until cutover. The farm bundle \
             is imported at cutover."
                .to_string(),
        );
    }

    let events_added = to_add.len() as i64;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for mut rec in to_add {
        projection::apply_event(&tx, &rec)?;
        // C3 (INT-010): `undoes_seq` is THIS farm's ordinal of the event the
        // undo names, never the source farm's. The name (`reverses_event_id`,
        // Ruling 4) is the link apply_undo just resolved; the column is
        // re-derived from it here so the FK points at the row the undo
        // actually reversed. The payload keeps the source's `undoesSeq`
        // verbatim — those are the record's own words. Rows imported before
        // C3 keep the source ordinal; nothing resolves by it.
        if let (Some(_), Some(target_id)) = (rec.undoes_seq, rec.reverses_event_id.clone()) {
            let local_seq: Option<i64> = tx
                .query_row(
                    "SELECT seq FROM event_log WHERE id = ?1",
                    params![target_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let local_seq = local_seq.ok_or_else(|| {
                format!("undo names an event this farm does not have: {target_id}")
            })?;
            rec.undoes_seq = Some(local_seq);
        }
        events::write_imported_event(&tx, &rec)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ImportResult {
        events_added,
        events_skipped_identical: skipped,
        foreign_records_added: foreign_added,
        receipts_copied,
    })
}

/// Written once per farm, at the path `apply_cutover_import` already checked
/// is absent (C3, INT-009). Never overwrites.
fn write_cutover_receipt(
    receipt_path: &Path,
    bundle_dir: &Path,
    result: &ImportResult,
) -> Result<(), String> {
    let manifest_bytes = fs::read(bundle_dir.join("manifest.json")).map_err(|e| e.to_string())?;
    let manifest_sha = sha256_hex(&manifest_bytes);
    let bundle_path = bundle_dir
        .to_str()
        .ok_or_else(|| "bundle path is not valid UTF-8".to_string())?;
    let body = format!(
        "utc={}\nbundle_path={}\nmanifest_sha256={}\nevents_added={}\nevents_skipped_identical={}\nforeign_records_added={}\n",
        db::utc_now_rfc3339(),
        bundle_path,
        manifest_sha,
        result.events_added,
        result.events_skipped_identical,
        result.foreign_records_added,
    );
    fs::write(receipt_path, body).map_err(|e| e.to_string())
}

fn target_farm_dir(conn: &Connection) -> Option<PathBuf> {
    let file: String = conn
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .ok()?;
    if file.is_empty() {
        return None; // in-memory database — nothing to copy into
    }
    Path::new(&file).parent().map(|p| p.to_path_buf())
}

/// Copy the bundle's receipts into the target farm folder. Additive and
/// idempotent: a receipt already present with identical bytes is skipped,
/// so a second import copies nothing. Reads from the bundle, writes only
/// into the farm folder — the bundle is never touched.
fn copy_receipts_into_farm(bundle_dir: &Path, farm_dir: &Path) -> Result<i64, String> {
    let source = bundle_dir.join("receipts");
    if !source.exists() {
        return Ok(0);
    }
    let dest_dir = farm_dir.join("receipts");
    fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;

    let mut copied = 0i64;
    let entries = fs::read_dir(&source).map_err(|e| e.to_string())?;
    for entry in entries {
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
        let dest = dest_dir.join(&name);
        let src_bytes = fs::read(&path).map_err(|e| e.to_string())?;

        if dest.exists() {
            let dest_bytes = fs::read(&dest).map_err(|e| e.to_string())?;
            if dest_bytes == src_bytes {
                continue;
            }
            return Err(format!(
                "A receipt on this machine does not match the one in the bundle: \
                 {name}. Nothing was brought in."
            ));
        }

        fs::write(&dest, &src_bytes).map_err(|e| e.to_string())?;
        let written = fs::read(&dest).map_err(|e| e.to_string())?;
        let src_digest = sha256_hex(&src_bytes);
        let dest_digest = sha256_hex(&written);
        if src_digest != dest_digest {
            return Err(format!("receipt {name} failed integrity check after copy"));
        }
        copied += 1;
    }
    Ok(copied)
}

fn build_plan(conn: &Connection, bundle_dir: &Path) -> Result<ImportPlan, String> {
    let mut refusals: Vec<ImportRefusal> = Vec::new();

    // Step 1 — manifest contract both directions.
    let manifest = read_manifest(bundle_dir)?;
    refusals.extend(check_manifest(bundle_dir, &manifest)?);

    // Step 2 — schema version.
    if manifest.app_schema_version != db::SCHEMA_VERSION {
        refusals.push(ImportRefusal::SchemaVersion {
            bundle: manifest.app_schema_version,
            this_app: db::SCHEMA_VERSION,
        });
    }

    // Step 3 — log versus database via the paths form (no status file written
    // into the bundle directory). The scratch root is the LIVE farm folder
    // (FIX A), resolved from the connection like the safety copies and the
    // cutover receipt — never the bundle, which may sit on media this app
    // must not write. An in-memory farm has no folder to hold the scratch.
    let farm_db = bundle_dir.join("farm.db");
    let events_jsonl = bundle_dir.join("events.jsonl");
    let mut verify_err: Option<String> = None;
    match target_farm_dir(conn) {
        None => {
            verify_err =
                Some("verify-replay needs an on-disk farm directory for its scratch".to_string());
        }
        Some(scratch_root) => {
            match projection::verify_replay_paths(&farm_db, &events_jsonl, &scratch_root) {
                Ok(VerifyOutcome::Pass { .. }) | Ok(VerifyOutcome::PassWithKnown { .. }) => {}
                Ok(VerifyOutcome::Fail { report }) => {
                    refusals.push(ImportRefusal::LogVersusDatabase {
                        detail: report_summary_detail(&report),
                    });
                }
                Err(e) => {
                    verify_err = Some(e);
                }
            }
        }
    }

    // Step 4 — parse every line; MissingEventId / Malformed.
    let text = if events_jsonl.is_file() {
        fs::read_to_string(&events_jsonl).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let mut records: Vec<EventRecord> = Vec::new();
    let mut parse_refusals = 0usize;
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                refusals.push(ImportRefusal::Malformed {
                    line_no,
                    detail: e.to_string(),
                });
                parse_refusals += 1;
                continue;
            }
        };

        // REFUSAL RULE 1 — check event_id explicitly before from_jsonl_value.
        match v.get("event_id").and_then(|x| x.as_str()) {
            None => {
                refusals.push(ImportRefusal::MissingEventId { line_no });
                parse_refusals += 1;
                continue;
            }
            Some(id) if id.trim().is_empty() => {
                refusals.push(ImportRefusal::MissingEventId { line_no });
                parse_refusals += 1;
                continue;
            }
            Some(_) => {}
        }

        let rec = match EventRecord::from_jsonl_value(&v) {
            Ok(r) => r,
            Err(e) => {
                refusals.push(ImportRefusal::Malformed { line_no, detail: e });
                parse_refusals += 1;
                continue;
            }
        };

        // Step 5 — REFUSAL RULE 3, origin laundering.
        if let Some(r) = check_commercial_claiming_farm_os(&rec) {
            refusals.push(r);
            parse_refusals += 1;
            continue;
        }
        // Step 5b — C3 (INT-003). The origin word is a closed set, and a
        // commercial_app record is carried only as cost.money_out: the one
        // kind whose projection keeps the mark and whose totals filter on
        // it. Anything else from another system would be counted as this
        // farm's own, so it is refused at the door, whole-bundle, exactly
        // like laundering. A third origin word never reaches the trigger.
        if rec.origin != "farm_os" && rec.origin != "commercial_app" {
            refusals.push(ImportRefusal::Malformed {
                line_no,
                detail: format!("origin {:?} is not farm_os or commercial_app", rec.origin),
            });
            parse_refusals += 1;
            continue;
        }
        if rec.origin == "commercial_app" && rec.kind != events::Kind::CostMoneyOut {
            refusals.push(ImportRefusal::ForeignKindNotCarried {
                event_id: rec.event_id.clone(),
                event_kind: rec.kind.as_str().to_string(),
            });
            parse_refusals += 1;
            continue;
        }
        records.push(rec);
    }

    if let Some(e) = verify_err {
        if parse_refusals == 0 {
            refusals.push(ImportRefusal::LogVersusDatabase { detail: e });
        }
    }

    // Step 6 — REFUSAL RULE 2 and idempotency.
    let mut already_present_identical = 0i64;
    let mut would_be_added = 0i64;
    let mut conflict_count = 0i64;
    for rec in &records {
        match lookup_existing(conn, &rec.event_id)? {
            None => would_be_added += 1,
            Some(existing) => match first_conflict_field(&existing, rec) {
                None => already_present_identical += 1,
                Some((field, here, bundle)) => {
                    conflict_count += 1;
                    refusals.push(ImportRefusal::FarmOsConflict {
                        event_id: rec.event_id.clone(),
                        field,
                        in_this_farm: here,
                        in_the_bundle: bundle,
                    });
                }
            },
        }
    }
    let shared_event_ids = already_present_identical + conflict_count;
    let events_in_bundle = count_event_lines(&text);

    // Step 6b — REFUSAL RULE 4, two farms are never combined.
    // Lineage is judged by identity::is_farm_truth (GT-D2): marketing kinds,
    // snapshot.taken, and attention.resolved are not farm lineage. A
    // marketing-only Groundtruth is the same operator's farm waiting to be born.
    let farm_truth_kinds: Vec<&str> = crate::events::Kind::ALL
        .iter()
        .filter(|k| crate::identity::is_farm_truth(**k))
        .map(|k| k.as_str())
        .collect();
    let farm_records_here: i64 = if farm_truth_kinds.is_empty() {
        0
    } else {
        let placeholders = farm_truth_kinds
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT COUNT(*) FROM event_log WHERE kind IN ({placeholders})");
        conn.query_row(&sql, params_from_iter(farm_truth_kinds.iter()), |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?
    };
    if farm_records_here > 0 && shared_event_ids == 0 {
        refusals.push(ImportRefusal::DifferentFarm {
            farm_records_here,
            events_in_bundle,
        });
    }

    // Step 7 — foreign records (legal; not a refusal). After Step 5b every
    // one of them is a cost.money_out, so the desk's "never counted in your
    // totals" is the truth for each.
    let foreign_records_in_bundle = records
        .iter()
        .filter(|r| r.origin == "commercial_app")
        .count() as i64;

    let explanations: Vec<String> = refusals.iter().map(explain_refusal).collect();
    let can_apply = refusals.is_empty();

    let bundle_path = bundle_dir
        .to_str()
        .ok_or_else(|| "bundle path is not valid UTF-8".to_string())?
        .to_string();

    Ok(ImportPlan {
        bundle_path,
        bundle_exported_at: manifest.exported_at,
        events_in_bundle,
        shared_event_ids,
        already_present_identical,
        would_be_added,
        foreign_records_in_bundle,
        refusals,
        can_apply,
        explanations,
    })
}

fn count_event_lines(text: &str) -> i64 {
    text.lines().filter(|l| !l.trim().is_empty()).count() as i64
}

fn read_manifest(bundle_dir: &Path) -> Result<Manifest, String> {
    let path = bundle_dir.join("manifest.json");
    let text =
        fs::read_to_string(&path).map_err(|e| format!("could not read manifest.json: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("manifest.json is not valid: {e}"))
}

fn check_manifest(bundle_dir: &Path, manifest: &Manifest) -> Result<Vec<ImportRefusal>, String> {
    let mut refusals = Vec::new();
    let mut listed: BTreeSet<String> = BTreeSet::new();

    for entry in &manifest.files {
        listed.insert(entry.path.clone());
        let abs = bundle_dir.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if !abs.is_file() {
            refusals.push(ImportRefusal::ManifestMismatch {
                path: entry.path.clone(),
                detail: "listed in the manifest but missing from the bundle".into(),
            });
            continue;
        }
        let bytes = fs::read(&abs).map_err(|e| e.to_string())?;
        if bytes.len() as u64 != entry.size_bytes {
            refusals.push(ImportRefusal::ManifestMismatch {
                path: entry.path.clone(),
                detail: format!(
                    "size is {} bytes; manifest says {}",
                    bytes.len(),
                    entry.size_bytes
                ),
            });
            continue;
        }
        let digest = sha256_hex(&bytes);
        if digest != entry.sha256 {
            refusals.push(ImportRefusal::ManifestMismatch {
                path: entry.path.clone(),
                detail: format!("checksum is {digest}; manifest says {}", entry.sha256),
            });
        }
    }

    // Bundle must not contain files the manifest does not list (manifest.json excepted).
    for path in walk_bundle_files(bundle_dir) {
        if path == "manifest.json" {
            continue;
        }
        if !listed.contains(&path) {
            refusals.push(ImportRefusal::ManifestMismatch {
                path: path.clone(),
                detail: "present in the bundle but not listed in the manifest".into(),
            });
        }
    }

    Ok(refusals)
}

fn walk_bundle_files(bundle_dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    fn walk(base: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(rel) = path.strip_prefix(base) {
                let s = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(s);
            }
        }
    }
    walk(bundle_dir, bundle_dir, &mut out);
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn report_summary_detail(report: &projection::CompareReport) -> String {
    let summary = if report.flush_lag > 0 {
        format!(
            "VERIFY-REPLAY: FAIL — {} event(s) pending flush.",
            report.flush_lag
        )
    } else {
        "VERIFY-REPLAY: FAIL".to_string()
    };
    let mut parts = vec![summary];
    for d in report.unknown_diffs.iter().take(5) {
        parts.push(format!(
            "table={} column={} key={}",
            d.table, d.field, d.key
        ));
    }
    parts.join("; ")
}

fn check_commercial_claiming_farm_os(rec: &EventRecord) -> Option<ImportRefusal> {
    // (a) event_class present but does not parse as EventClass.
    if let Some(class) = rec.event_class.as_deref() {
        if EventClass::parse(class).is_err() {
            return Some(ImportRefusal::CommercialClaimingFarmOs {
                event_id: rec.event_id.clone(),
                detail: format!("event_class {class:?} is not a Farm OS class"),
            });
        }
    }

    // (b) record origin farm_os but payload origin copy says commercial_app.
    if rec.origin == "farm_os" {
        if let Some(payload_origin) = rec.payload.get("origin").and_then(|x| x.as_str()) {
            if payload_origin == "commercial_app" {
                return Some(ImportRefusal::CommercialClaimingFarmOs {
                    event_id: rec.event_id.clone(),
                    detail:
                        "the payload origin copy says commercial_app while the record says farm_os"
                            .into(),
                });
            }
        }
    }

    // (c) farm_os sale/payment kind whose payload contains commercial_app as key or string value.
    if rec.origin == "farm_os" {
        let kind = rec.kind.as_str();
        if matches!(
            kind,
            "stripe.session_paid" | "stripe.refunded" | "stripe.disputed" | "cost.money_out"
        ) && payload_mentions_commercial_app(&rec.payload)
        {
            return Some(ImportRefusal::CommercialClaimingFarmOs {
                event_id: rec.event_id.clone(),
                detail: "a sale or payment payload carries commercial_app".into(),
            });
        }
    }

    None
}

fn payload_mentions_commercial_app(v: &Value) -> bool {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == "commercial_app" {
                    return true;
                }
                if payload_mentions_commercial_app(val) {
                    return true;
                }
            }
            false
        }
        Value::Array(items) => items.iter().any(payload_mentions_commercial_app),
        Value::String(s) => s == "commercial_app",
        _ => false,
    }
}

struct ExistingRow {
    kind: String,
    entity_type: String,
    entity_id: String,
    payload: Value,
    inverse: Value,
    origin: String,
    event_domain: String,
    event_class: Option<String>,
    reverses_event_id: Option<String>,
    undone_at: Option<String>,
    created_at: String,
}

fn lookup_existing(conn: &Connection, event_id: &str) -> Result<Option<ExistingRow>, String> {
    conn.query_row(
        "SELECT kind, entity_type, entity_id, payload, inverse, origin,
                event_domain, event_class, reverses_event_id,
                undone_at, created_at
         FROM event_log WHERE id = ?1",
        params![event_id],
        |r| {
            let payload_s: String = r.get(3)?;
            let inverse_s: String = r.get(4)?;
            Ok(ExistingRow {
                kind: r.get(0)?,
                entity_type: r.get(1)?,
                entity_id: r.get(2)?,
                payload: serde_json::from_str(&payload_s).unwrap_or(Value::String(payload_s)),
                inverse: serde_json::from_str(&inverse_s).unwrap_or(Value::String(inverse_s)),
                origin: r.get(5)?,
                event_domain: r.get(6)?,
                event_class: r.get(7)?,
                reverses_event_id: r.get(8)?,
                undone_at: r.get(9)?,
                created_at: r.get(10)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn first_conflict_field(
    existing: &ExistingRow,
    bundle: &EventRecord,
) -> Option<(String, String, String)> {
    // Compare these fields: kind, entity_type, entity_id, payload, inverse,
    // origin, event_domain, event_class, reverses_event_id, undone_at,
    // created_at. Do NOT compare seq — it is local ordering — nor undoes_seq,
    // which since C3 (INT-010) is this farm's ordinal of the event named by
    // reverses_event_id, and the name IS compared.
    // undone_at is compared deliberately. Importing a bundle, undoing locally,
    // then re-importing the same bundle is a genuine log-versus-farm
    // disagreement and must surface rather than be papered over.
    // Compare payload and inverse as parsed Value so key ordering cannot
    // manufacture a false conflict.
    if existing.kind != bundle.kind.as_str() {
        return Some((
            "kind".into(),
            existing.kind.clone(),
            bundle.kind.as_str().to_string(),
        ));
    }
    if existing.entity_type != bundle.entity_type {
        return Some((
            "entity_type".into(),
            existing.entity_type.clone(),
            bundle.entity_type.clone(),
        ));
    }
    if existing.entity_id != bundle.entity_id {
        return Some((
            "entity_id".into(),
            existing.entity_id.clone(),
            bundle.entity_id.clone(),
        ));
    }
    if existing.payload != bundle.payload {
        return Some((
            "payload".into(),
            existing.payload.to_string(),
            bundle.payload.to_string(),
        ));
    }
    if existing.inverse != bundle.inverse {
        return Some((
            "inverse".into(),
            existing.inverse.to_string(),
            bundle.inverse.to_string(),
        ));
    }
    if existing.origin != bundle.origin {
        return Some((
            "origin".into(),
            existing.origin.clone(),
            bundle.origin.clone(),
        ));
    }
    if existing.event_domain != bundle.event_domain {
        return Some((
            "event_domain".into(),
            existing.event_domain.clone(),
            bundle.event_domain.clone(),
        ));
    }
    if existing.event_class != bundle.event_class {
        return Some((
            "event_class".into(),
            opt_disp(&existing.event_class),
            opt_disp(&bundle.event_class),
        ));
    }
    if existing.reverses_event_id != bundle.reverses_event_id {
        return Some((
            "reverses_event_id".into(),
            opt_disp(&existing.reverses_event_id),
            opt_disp(&bundle.reverses_event_id),
        ));
    }
    if existing.undone_at != bundle.undone_at {
        return Some((
            "undone_at".into(),
            opt_disp(&existing.undone_at),
            opt_disp(&bundle.undone_at),
        ));
    }
    if existing.created_at != bundle.created_at {
        return Some((
            "created_at".into(),
            existing.created_at.clone(),
            bundle.created_at.clone(),
        ));
    }
    None
}

fn opt_disp(v: &Option<String>) -> String {
    v.clone().unwrap_or_default()
}

fn parse_bundle_records(events_jsonl: &Path) -> Result<Vec<EventRecord>, String> {
    let text = fs::read_to_string(events_jsonl).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("events.jsonl line {}: {e}", idx + 1))?;
        let id = v
            .get("event_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        if id.is_empty() {
            return Err(format!("events.jsonl line {}: missing event_id", idx + 1));
        }
        let rec = EventRecord::from_jsonl_value(&v)?;
        if check_commercial_claiming_farm_os(&rec).is_some() {
            return Err(format!(
                "events.jsonl line {}: commercial claiming farm_os",
                idx + 1
            ));
        }
        out.push(rec);
    }
    Ok(out)
}

fn explain_refusal(r: &ImportRefusal) -> String {
    match r {
        ImportRefusal::MissingEventId { line_no } => format!(
            "Line {line_no} of the log has no id. Every event needs a stable id \
             or it cannot be matched, so nothing was brought in."
        ),
        ImportRefusal::FarmOsConflict {
            event_id,
            field,
            in_this_farm,
            in_the_bundle,
        } => format!(
            "This farm already has event {event_id}, and the bundle disagrees about \
             {field}: this farm says {in_this_farm}, the bundle says {in_the_bundle}. \
             Nothing was changed. Two farms cannot be merged."
        ),
        ImportRefusal::CommercialClaimingFarmOs { event_id, detail } => format!(
            "Event {event_id} is a sale or payment from another system labelled as if \
             it were yours. {detail}. Nothing was brought in."
        ),
        ImportRefusal::ForeignKindNotCarried {
            event_id,
            event_kind,
        } => format!(
            "Event {event_id} is a {event_kind} record from another system. Groundtruth \
             carries another system's expense records only, marked and never counted; \
             a {event_kind} from elsewhere would be counted as this farm's own. \
             Nothing was brought in."
        ),
        ImportRefusal::DifferentFarm {
            farm_records_here,
            events_in_bundle,
        } => format!(
            "This looks like a different farm's records. Two farms cannot be merged. \
             This farm has {farm_records_here} records of its own and shares none of \
             the {events_in_bundle} events in this bundle. Nothing was brought in."
        ),
        ImportRefusal::LogVersusDatabase { detail } => format!(
            "The bundle's log and its database do not agree. {detail}. \
             Nothing was brought in."
        ),
        ImportRefusal::ManifestMismatch { path, detail } => format!(
            "The bundle's manifest does not match the file {path}: {detail}. \
             Nothing was brought in."
        ),
        ImportRefusal::SchemaVersion { bundle, this_app } => format!(
            "This bundle was made for schema version {bundle}, and this app is \
             schema version {this_app}. Cross-version import is not supported, so \
             nothing was brought in."
        ),
        ImportRefusal::Malformed { line_no, detail } => format!(
            "Line {line_no} of the log could not be read ({detail}). \
             Nothing was brought in."
        ),
    }
}
