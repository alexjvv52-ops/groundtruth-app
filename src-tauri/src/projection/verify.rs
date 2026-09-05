use crate::db::{self, SCHEMA_VERSION};
use crate::divergence::{KnownDivergence, KNOWN_DIVERGENCES};
use crate::event_file;
use crate::events::EventRecord;
use crate::projection::apply_event;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Declared every run (Ruling 2). Do not grow without a ruling.
pub const EXCLUSION_LIST: &[&str] = &[
    "trays.growth_days_at_sow — crops-library reference join at write (Ruling 2 cat. 2; payload freeze deferred)",
    "trays.blackout_days_at_sow — crops-library reference join at write (Ruling 2 cat. 2; payload freeze deferred)",
    "snapshot files under snapshots/ — filesystem artifact; snapshot.taken is an explicit SQL no-op (Ruling 2 cat. 3)",
    "attention table — operator collateral; attention.resolved is an explicit SQL no-op on replay (decisions/RULING-attention-outside-replay-ledger.md)",
    "reference seed: the crops table is copied from the snapshot into the replay database; crop_aliases (B6) is reference data beside it but is neither copied nor compared — no apply_* writes it and nothing reads it during replay",
    "shelf_capacity (Phase 6): physical ceiling, reference data beside crops. No apply_* writes it and nothing reads it during replay, so it is neither copied nor compared",
    "business_profile (Phase 7): GBP verification date, reference data beside crops. No apply_* writes it and nothing reads it during replay, so it is neither copied nor compared",
    "storefront_observations: remote observation cache, not derived from events",
    "health_evidence: check evidence rows, not derived from events",
    "scan_config (Phase 8): scan endpoint URL and pull token, operator-set reference data. No apply_* writes it and nothing reads it during replay, so it is neither copied nor compared",
    "scan_observations: remote pull log, not derived from events",
    "standing_pull_observations (customer QR fence 5): remote pull log, not derived from events",
    "standing_pull_refusals (customer QR fence 5): durable refusal trace of pulled rows the desktop would not turn into candidates, not derived from events",
    "field_devices (rack-side fence 2): the paired field terminal — device id, role, token hash, paired / retired stamps — operator-set reference data. No apply_* writes it and nothing reads it during replay, so it is neither copied nor compared",
    "phone_pull_observations (rack-side fence 2): remote pull log, not derived from events",
    "phone_pull_refusals (rack-side fence 2): durable refusal trace of pulled rows the desktop would not turn into proposals, not derived from events",
    "offers: operator-set prices and Stripe ids for the retail door, written outside the event log; no apply_* writes it and nothing reads it during replay",
    "harvest_links: retired harvest Payment Link rows kept by migration; no apply_* writes it and nothing reads it during replay",
    "stripe_config / stripe_cursor: the restricted key, account identity and poll cursors — connection state, not farm truth; no apply_* writes them",
    "farm_config (INV-A): the farm's display name for the invoice header — operator-set reference data. No apply_* writes it and nothing reads it during replay, so it is neither copied nor compared",
    "cost_events / mileage_trips / assets / income_events row-level BEFORE INSERT triggers are dropped on the replay database only (they read date('now','localtime')); the live database keeps them, and the same rules are enforced in Rust by validate_cost_event / validate_mileage_event / validate_asset_event / validate_income_event inside every apply_*",
];

/// Same flags as `snapshots::validate_farm_file` — proven against this DB.
pub const VERIFY_SOURCE_OPEN_FLAGS: OpenFlags = OpenFlags::SQLITE_OPEN_READ_ONLY;

/// The four row-level guards that compare an operator date to
/// date('now','localtime'). They protect LIVE writes. On the scratch
/// replay database they would abort a legal historic event whenever the
/// replaying machine's local date is earlier than the event's operator
/// date (clock set back, a machine in a westward zone, a dead-laptop
/// drill) — a clock read inside the replay path. Dropped on the replay
/// database only; declared on EXCLUSION_LIST.
pub const REPLAY_DROPPED_TRIGGERS: &[&str] = &[
    "cost_events_before_insert",
    "mileage_trips_before_insert",
    "assets_before_insert",
    "income_events_before_insert",
];

#[derive(Debug, Clone)]
pub struct FieldDiff {
    pub table: String,
    pub key: String,
    pub field: String,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone)]
pub struct CompareReport {
    pub events_read: usize,
    pub events_replayed: usize,
    pub tables_compared: usize,
    pub rows_compared: usize,
    pub watermark: i64,
    pub live_max_seq: i64,
    pub flush_lag: i64,
    pub known_divergences: usize,
    pub matched_ledger: Vec<KnownDivergence>,
    pub unknown_diffs: Vec<FieldDiff>,
    pub exclusions: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum VerifyOutcome {
    Pass { report: CompareReport },
    PassWithKnown { report: CompareReport },
    Fail { report: CompareReport },
}

impl VerifyOutcome {
    pub fn exit_nonzero(&self) -> bool {
        matches!(self, VerifyOutcome::Fail { .. })
    }

    pub fn report(&self) -> &CompareReport {
        match self {
            VerifyOutcome::Pass { report }
            | VerifyOutcome::PassWithKnown { report }
            | VerifyOutcome::Fail { report } => report,
        }
    }

    /// Display string only. Verdict decision lives in `decide_outcome`.
    pub fn summary_line(&self) -> String {
        match self {
            VerifyOutcome::Pass { .. } => "VERIFY-REPLAY: PASS".to_string(),
            VerifyOutcome::PassWithKnown { report } => format!(
                "VERIFY-REPLAY: PASS WITH {} KNOWN DIVERGENCES",
                report.known_divergences
            ),
            VerifyOutcome::Fail { report } if report.flush_lag > 0 => format!(
                "VERIFY-REPLAY: FAIL — {} event(s) pending flush.",
                report.flush_lag
            ),
            VerifyOutcome::Fail { .. } => "VERIFY-REPLAY: FAIL".to_string(),
        }
    }
}

pub fn print_exclusions(out: &mut dyn Write) {
    let _ = writeln!(out, "verify-replay exclusions (declared every run):");
    for line in EXCLUSION_LIST {
        let _ = writeln!(out, "  - {line}");
    }
}

fn open_verify_source(path: &Path) -> Result<Connection, String> {
    // Mirror snapshots.rs validate_farm_file: READ_ONLY open, no db::configure.
    // journal_mode WAL is a write and must not run on this handle.
    Connection::open_with_flags(path, VERIFY_SOURCE_OPEN_FLAGS).map_err(|e| e.to_string())
}

fn print_operator_details(outcome: &VerifyOutcome) {
    let report = outcome.report();
    println!("FLUSH LAG: {} event(s) pending", report.flush_lag);
    println!("{}", outcome.summary_line());
    if report.flush_lag > 0 {
        println!("Open Farm OS to flush, close it normally, and re-run.");
    }
    println!("watermark={}", report.watermark);
    println!("live_max_seq={}", report.live_max_seq);
    println!("events_read={}", report.events_read);
    println!("events_replayed={}", report.events_replayed);
    println!("tables_compared={}", report.tables_compared);
    println!("rows_compared={}", report.rows_compared);
    println!("matched ledger entries ({}):", report.matched_ledger.len());
    if report.matched_ledger.is_empty() {
        println!("  (none)");
    } else {
        for d in &report.matched_ledger {
            println!(
                "  {} table={} key={} column={} seq_range={}",
                d.id, d.table, d.pk, d.column, d.seq_range
            );
        }
    }
    if let VerifyOutcome::Fail { report } = outcome {
        if report.flush_lag == 0 && !report.unknown_diffs.is_empty() {
            println!("unexplained divergences:");
            for d in &report.unknown_diffs {
                println!(
                    "  table={} key={} column={} expected={} actual={}",
                    d.table, d.key, d.field, d.expected, d.actual
                );
            }
        }
    }
}

/// Consistent read via VACUUM INTO from a read-only source, then replay.
///
/// `scratch_root` is the live farm folder. The scratch copy and the replay
/// database live at `<scratch_root>/snapshots/verify-<uuid>/`, created for
/// this run and removed on every path out of it. Never the OS temp
/// directory: on the installed app the standard token could not create
/// under `%TEMP%`, so every System Scan ended "Access is denied. (os error
/// 5)" before anything was compared, while the farm folder itself was
/// writable. `list_snapshots` / retention skip directories, so the scratch
/// is invisible to the backup list while it exists.
pub fn verify_replay_paths(
    farm_db: &Path,
    events_jsonl: &Path,
    scratch_root: &Path,
) -> Result<VerifyOutcome, String> {
    let snapshots_dir = scratch_root.join("snapshots");
    let snapshots_dir_existed = snapshots_dir.is_dir();
    let scratch = snapshots_dir.join(format!("verify-{}", uuid::Uuid::new_v4()));
    if let Err(e) = fs::create_dir_all(&scratch) {
        if !snapshots_dir_existed {
            let _ = fs::remove_dir(&snapshots_dir);
        }
        return Err(format!("could not create {}: {e}", scratch.display()));
    }
    let snap = scratch.join("snapshot.db");
    let replay = scratch.join("replay.db");

    let outcome = vacuum_then_replay(farm_db, events_jsonl, &snap, &replay);

    // Remove per run, whatever the outcome. A `snapshots/` this run created
    // (a bare farm dir) is given back when nothing else landed in it.
    let _ = fs::remove_dir_all(&scratch);
    if !snapshots_dir_existed {
        let _ = fs::remove_dir(&snapshots_dir);
    }
    outcome
}

/// 1. Consistent read — never a raw file copy under WAL. VACUUM INTO works
///    from SQLITE_OPEN_READ_ONLY (source unchanged; dest is a new file).
/// 2. Replay against that copy.
fn vacuum_then_replay(
    farm_db: &Path,
    events_jsonl: &Path,
    snap: &Path,
    replay: &Path,
) -> Result<VerifyOutcome, String> {
    {
        let live = open_verify_source(farm_db)?;
        let dest = snap
            .to_str()
            .ok_or_else(|| "snapshot path not UTF-8".to_string())?;
        live.execute("VACUUM INTO ?1", params![dest])
            .map_err(|e| e.to_string())?;
    }
    verify_replay(snap, events_jsonl, replay)
}

pub fn verify_replay(
    snapshot_db: &Path,
    events_jsonl: &Path,
    replay_db: &Path,
) -> Result<VerifyOutcome, String> {
    let mut stdout = std::io::stdout();
    print_exclusions(&mut stdout);

    let snap = Connection::open(snapshot_db).map_err(|e| e.to_string())?;
    db::configure(&snap)?;

    // 2. Fresh DB at current schema; seed reference tables from snapshot.
    if replay_db.exists() {
        fs::remove_file(replay_db)
            .map_err(|e| format!("could not remove {}: {e}", replay_db.display()))?;
    }
    let mut replay = Connection::open(replay_db).map_err(|e| e.to_string())?;
    db::configure(&replay)?;
    db::migrate(&replay)?;
    for t in REPLAY_DROPPED_TRIGGERS {
        replay
            .execute_batch(&format!("DROP TRIGGER IF EXISTS {t};"))
            .map_err(|e| e.to_string())?;
    }
    println!(
        "replay guards dropped: {}",
        REPLAY_DROPPED_TRIGGERS.join(", ")
    );
    copy_reference_tables(&snap, &replay)?;
    println!("reference seed: copied crops from snapshot into replay DB (schema {SCHEMA_VERSION})");

    // 3. Replay events.jsonl.
    let text = fs::read_to_string(events_jsonl)
        .map_err(|e| format!("could not read {}: {e}", events_jsonl.display()))?;
    let mut parsed: Vec<(i64, EventRecord)> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).map_err(|e| {
            format!("events.jsonl line {line_no}: expected a JSON event object ({e})")
        })?;
        let rec = EventRecord::from_jsonl_value(&v).map_err(|e| {
            format!("events.jsonl line {line_no}: expected a valid event record ({e})")
        })?;
        let seq = rec.seq.ok_or_else(|| {
            format!("events.jsonl line {line_no}: expected a seq field on the event record")
        })?;
        parsed.push((seq, rec));
    }
    parsed.sort_by_key(|(s, _)| *s);

    let events_read = parsed.len();
    for (_seq, event) in &parsed {
        let tx = replay.transaction().map_err(|e| e.to_string())?;
        insert_event_row(&tx, event)?;
        apply_event(&tx, event)?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    let events_replayed = events_read;

    // Comparison boundary is the flush watermark (Ruling 1).
    let watermark = event_file::read_watermark(events_jsonl)?;
    let live_max_seq: i64 = snap
        .query_row("SELECT IFNULL(MAX(seq), 0) FROM event_log", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let flush_lag = (live_max_seq - watermark).max(0);

    // 4. Compare in-scope set — live rows only up to the watermark.
    let mut report = CompareReport {
        events_read,
        events_replayed,
        tables_compared: 0,
        rows_compared: 0,
        watermark,
        live_max_seq,
        flush_lag,
        known_divergences: 0,
        matched_ledger: Vec::new(),
        unknown_diffs: Vec::new(),
        exclusions: EXCLUSION_LIST.iter().map(|s| (*s).to_string()).collect(),
    };
    compare_event_log(&snap, &replay, watermark, &mut report)?;
    compare_trays(&snap, &replay, &mut report)?;
    compare_orders(&snap, &replay, &mut report)?;
    compare_cost_events(&snap, &replay, &mut report)?;
    compare_money_corrections(&snap, &replay, &mut report)?;
    compare_consumption_events(&snap, &replay, &mut report)?;
    compare_mileage_trips(&snap, &replay, &mut report)?;
    compare_assets(&snap, &replay, &mut report)?;
    compare_income_events(&snap, &replay, &mut report)?;
    compare_mkt_venues(&snap, &replay, &mut report)?;
    compare_mkt_samples(&snap, &replay, &mut report)?;
    compare_mkt_touches(&snap, &replay, &mut report)?;
    compare_mkt_followups(&snap, &replay, &mut report)?;
    compare_mkt_stages(&snap, &replay, &mut report)?;
    compare_mkt_reviews(&snap, &replay, &mut report)?;
    compare_mkt_review_requests(&snap, &replay, &mut report)?;
    compare_mkt_standing_requests(&snap, &replay, &mut report)?;
    compare_harvest_coverage(&snap, &replay, &mut report)?;
    compare_phone_proposals(&snap, &replay, &mut report)?;
    compare_wholesale_orders(&snap, &replay, &mut report)?;
    compare_wholesale_order_lines(&snap, &replay, &mut report)?;
    compare_wholesale_write_offs(&snap, &replay, &mut report)?;
    compare_wholesale_bad_debts(&snap, &replay, &mut report)?;
    compare_stripe_unapplied_facts(&snap, &replay, &mut report)?;
    compare_leftover_listings(&snap, &replay, &mut report)?;
    compare_seed_receipts(&snap, &replay, &mut report)?;
    // attention is outside the replay ledger — not compared (see EXCLUSION_LIST).

    let outcome = decide_outcome(report);
    print_operator_details(&outcome);
    Ok(outcome)
}

/// Verdict selection after compare.
/// Non-zero flush lag is FAIL (Ruling 1). Zero work is FAIL (Ruling C).
#[allow(clippy::if_same_then_else)] // H-17 / H-9: three arms return one verdict
                                    // because each cites a different signed ruling. Collapsing erases that.
fn decide_outcome(report: CompareReport) -> VerifyOutcome {
    if report.flush_lag > 0 {
        VerifyOutcome::Fail { report }
    } else if report.events_replayed == 0 || report.rows_compared == 0 {
        VerifyOutcome::Fail { report }
    } else if !report.unknown_diffs.is_empty() {
        VerifyOutcome::Fail { report }
    } else if report.known_divergences > 0 {
        VerifyOutcome::PassWithKnown { report }
    } else {
        VerifyOutcome::Pass { report }
    }
}

fn copy_reference_tables(from: &Connection, to: &Connection) -> Result<(), String> {
    // Clear seeded crops and copy snapshot crops exactly (ids/names/days/rates).
    to.execute("DELETE FROM crops", [])
        .map_err(|e| e.to_string())?;
    let mut stmt = from
        .prepare(
            "SELECT id, name, growth_days, blackout_days, expected_yield_oz, sort_order,
                    seed_rate_oz_per_tray FROM crops",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<f64>>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (id, name, g, b, y, s, rate) = row.map_err(|e| e.to_string())?;
        to.execute(
            "INSERT INTO crops
             (id, name, growth_days, blackout_days, expected_yield_oz, sort_order,
              seed_rate_oz_per_tray)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, name, g, b, y, s, rate],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn insert_event_row(tx: &rusqlite::Transaction<'_>, event: &EventRecord) -> Result<(), String> {
    let payload_s = serde_json::to_string(&event.payload).map_err(|e| e.to_string())?;
    let inverse_s = serde_json::to_string(&event.inverse).map_err(|e| e.to_string())?;
    let seq = event
        .seq
        .ok_or_else(|| "replay insert requires seq".to_string())?;
    tx.execute(
        "INSERT INTO event_log
         (seq, id, kind, entity_type, entity_id, payload, inverse, undone_at, undoes_seq, created_at,
          origin, event_domain, event_class, reverses_event_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            seq,
            event.event_id,
            event.kind.as_str(),
            event.entity_type,
            event.entity_id,
            payload_s,
            inverse_s,
            event.undone_at,
            event.undoes_seq,
            event.created_at,
            event.origin,
            event.event_domain,
            event.event_class,
            event.reverses_event_id,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn push_diff(
    report: &mut CompareReport,
    table: &str,
    key: &str,
    field: &str,
    expected: impl ToString,
    actual: impl ToString,
) {
    let expected = expected.to_string();
    let actual = actual.to_string();
    if expected == actual {
        return;
    }
    if let Some(known) = KNOWN_DIVERGENCES
        .iter()
        .find(|d| d.table == table && d.pk == key && d.column == field)
    {
        report.known_divergences += 1;
        if !report.matched_ledger.iter().any(|m| m.id == known.id) {
            report.matched_ledger.push(*known);
        }
        return;
    }
    report.unknown_diffs.push(FieldDiff {
        table: table.into(),
        key: key.into(),
        field: field.into(),
        expected,
        actual,
    });
}

fn compare_event_log(
    snap: &Connection,
    replay: &Connection,
    watermark: i64,
    report: &mut CompareReport,
) -> Result<(), String> {
    report.tables_compared += 1;
    let cols = [
        "seq",
        "id",
        "kind",
        "entity_type",
        "entity_id",
        "payload",
        "inverse",
        "undone_at",
        "undoes_seq",
        "created_at",
        "origin",
        "event_domain",
        "event_class",
        "reverses_event_id",
    ];
    // Live rows with seq > watermark are pending flush — not compared (Ruling 1).
    let sql = format!(
        "SELECT {} FROM event_log WHERE seq <= ?1 ORDER BY seq",
        cols.join(", ")
    );
    let snap_rows = load_string_rows_params(snap, &sql, params![watermark])?;
    let replay_rows = load_string_rows_params(replay, &sql, params![watermark])?;
    report.rows_compared += snap_rows.len().max(replay_rows.len());
    if snap_rows.len() != replay_rows.len() {
        push_diff(
            report,
            "event_log",
            "*",
            "row_count",
            snap_rows.len(),
            replay_rows.len(),
        );
    }
    let n = snap_rows.len().min(replay_rows.len());
    for i in 0..n {
        let key = match snap_rows.get(i).and_then(|r| r.get(1)) {
            Some(k) => k.clone(),
            None => continue,
        };
        for (ci, col) in cols.iter().enumerate() {
            let exp = snap_rows
                .get(i)
                .and_then(|r| r.get(ci))
                .map(|s| s.as_str())
                .unwrap_or("");
            let act = replay_rows
                .get(i)
                .and_then(|r| r.get(ci))
                .map(|s| s.as_str())
                .unwrap_or("");
            push_diff(report, "event_log", &key, col, exp, act);
        }
    }
    Ok(())
}

fn compare_trays(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    // In-scope tray columns (excludes growth_days_at_sow / blackout_days_at_sow).
    let cols = [
        "id",
        "crop_id",
        "state",
        "quantity",
        "planned_on",
        "sown_on",
        "blackout_on",
        "light_on",
        "harvested_on",
        "discarded_on",
        "actual_yield_oz",
        "created_at",
        "updated_at",
    ];
    compare_keyed(snap, replay, "trays", "id", &cols, report)
}

fn compare_orders(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = [
        "id",
        "stripe_session_id",
        "stripe_payment_intent",
        "harvest_date",
        "crop_id",
        "quantity",
        "amount_cents",
        "currency",
        "customer_email",
        "state",
        "capacity_consumed",
        "paid_at",
        "created_at",
        "updated_at",
        "client_reference",
    ];
    compare_keyed(snap, replay, "orders", "id", &cols, report)
}

fn compare_cost_events(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::costs::COST_EVENTS_COLUMNS;
    compare_keyed(snap, replay, "cost_events", "event_id", cols, report)
}

fn compare_money_corrections(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::costs::MONEY_CORRECTIONS_COLUMNS;
    compare_keyed(
        snap,
        replay,
        "money_corrections",
        "correction_event_id",
        cols,
        report,
    )
}

fn compare_consumption_events(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::consumption::CONSUMPTION_EVENTS_COLUMNS;
    compare_keyed(snap, replay, "consumption_events", "event_id", cols, report)
}

fn compare_mkt_venues(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::marketing::MKT_VENUES_COLUMNS;
    compare_keyed(snap, replay, "mkt_venues", "venue_id", cols, report)
}

fn compare_mkt_samples(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::marketing::MKT_SAMPLES_COLUMNS;
    compare_keyed(snap, replay, "mkt_samples", "sample_id", cols, report)
}

fn compare_mkt_touches(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::marketing::MKT_TOUCHES_COLUMNS;
    compare_keyed(snap, replay, "mkt_touches", "touch_id", cols, report)
}

fn compare_mkt_followups(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::marketing::MKT_FOLLOWUPS_COLUMNS;
    compare_keyed(snap, replay, "mkt_followups", "followup_id", cols, report)
}

fn compare_mkt_stages(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::marketing::MKT_STAGES_COLUMNS;
    compare_keyed(snap, replay, "mkt_stages", "venue_id", cols, report)
}

fn compare_mkt_reviews(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::marketing::MKT_REVIEWS_COLUMNS;
    compare_keyed(snap, replay, "mkt_reviews", "observation_id", cols, report)
}

fn compare_mkt_review_requests(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::marketing::MKT_REVIEW_REQUESTS_COLUMNS;
    compare_keyed(
        snap,
        replay,
        "mkt_review_requests",
        "venue_id",
        cols,
        report,
    )
}

fn compare_mkt_standing_requests(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::marketing::MKT_STANDING_REQUESTS_COLUMNS;
    compare_keyed(
        snap,
        replay,
        "mkt_standing_requests",
        "request_id",
        cols,
        report,
    )
}

fn compare_harvest_coverage(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::marketing::HARVEST_COVERAGE_COLUMNS;
    compare_keyed(
        snap,
        replay,
        "harvest_coverage",
        "coverage_id",
        cols,
        report,
    )
}

fn compare_phone_proposals(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::phone::PHONE_PROPOSALS_COLUMNS;
    compare_keyed(snap, replay, "phone_proposals", "proposal_id", cols, report)
}

fn compare_wholesale_orders(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    compare_keyed(
        snap,
        replay,
        "wholesale_orders",
        "id",
        crate::wholesale::WHOLESALE_ORDERS_COLUMNS,
        report,
    )
}

fn compare_wholesale_order_lines(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    compare_keyed_sql(
        snap,
        replay,
        "wholesale_order_lines",
        "order_id || '|' || crop_id",
        "order_line",
        crate::wholesale::WHOLESALE_ORDER_LINES_COLUMNS,
        report,
    )
}

fn compare_wholesale_write_offs(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    compare_keyed(
        snap,
        replay,
        "wholesale_write_offs",
        "event_id",
        crate::wholesale::WHOLESALE_WRITE_OFFS_COLUMNS,
        report,
    )
}

fn compare_wholesale_bad_debts(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    compare_keyed(
        snap,
        replay,
        "wholesale_bad_debts",
        "event_id",
        crate::wholesale::WHOLESALE_BAD_DEBTS_COLUMNS,
        report,
    )
}

fn compare_stripe_unapplied_facts(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    compare_keyed(
        snap,
        replay,
        "stripe_unapplied_facts",
        "event_id",
        crate::money::STRIPE_UNAPPLIED_FACTS_COLUMNS,
        report,
    )
}

fn compare_leftover_listings(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    compare_keyed(
        snap,
        replay,
        "leftover_listings",
        "listing_id",
        crate::leftover::LEFTOVER_LISTINGS_COLUMNS,
        report,
    )
}

fn compare_seed_receipts(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    compare_keyed(
        snap,
        replay,
        "seed_receipts",
        "receipt_id",
        crate::seed::SEED_RECEIPTS_COLUMNS,
        report,
    )
}

fn compare_mileage_trips(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::mileage::MILEAGE_TRIPS_COLUMNS;
    compare_keyed(snap, replay, "mileage_trips", "trip_id", cols, report)
}

fn compare_assets(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::assets::ASSETS_COLUMNS;
    compare_keyed(snap, replay, "assets", "asset_id", cols, report)
}

fn compare_income_events(
    snap: &Connection,
    replay: &Connection,
    report: &mut CompareReport,
) -> Result<(), String> {
    let cols = crate::income::INCOME_EVENTS_COLUMNS;
    compare_keyed(snap, replay, "income_events", "income_id", cols, report)
}

fn compare_keyed(
    snap: &Connection,
    replay: &Connection,
    table: &str,
    pk: &str,
    cols: &[&str],
    report: &mut CompareReport,
) -> Result<(), String> {
    report.tables_compared += 1;
    let sql = format!("SELECT {} FROM {table} ORDER BY {pk}", cols.join(", "));
    let snap_rows = load_string_rows(snap, &sql)?;
    let replay_rows = load_string_rows(replay, &sql)?;
    report.rows_compared += snap_rows.len().max(replay_rows.len());

    use std::collections::BTreeMap;
    let mut snap_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut replay_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let pk_idx = cols
        .iter()
        .position(|c| *c == pk)
        .ok_or_else(|| format!("internal error: pk {pk} missing from column list for {table}"))?;
    for row in snap_rows {
        let key = match row.get(pk_idx) {
            Some(k) => k.clone(),
            None => continue,
        };
        snap_map.insert(key, row);
    }
    for row in replay_rows {
        let key = match row.get(pk_idx) {
            Some(k) => k.clone(),
            None => continue,
        };
        replay_map.insert(key, row);
    }
    for key in snap_map.keys() {
        if !replay_map.contains_key(key) {
            push_diff(report, table, key, pk, "present", "missing");
        }
    }
    for key in replay_map.keys() {
        if !snap_map.contains_key(key) {
            push_diff(report, table, key, pk, "missing", "present");
        }
    }
    for (key, srow) in &snap_map {
        if let Some(rrow) = replay_map.get(key) {
            for (ci, col) in cols.iter().enumerate() {
                let exp = srow.get(ci).map(|s| s.as_str()).unwrap_or("");
                let act = rrow.get(ci).map(|s| s.as_str()).unwrap_or("");
                push_diff(report, table, key, col, exp, act);
            }
        }
    }
    Ok(())
}

/// compare_keyed for a table whose identity is an expression (composite PK).
/// Column 0 of the SELECT is the key; columns 1..n are compared by name.
fn compare_keyed_sql(
    snap: &Connection,
    replay: &Connection,
    table: &str,
    key_expr: &str,
    key_label: &str,
    cols: &[&str],
    report: &mut CompareReport,
) -> Result<(), String> {
    report.tables_compared += 1;
    let sql = format!(
        "SELECT {key_expr}, {} FROM {table} ORDER BY 1",
        cols.join(", ")
    );
    let snap_rows = load_string_rows(snap, &sql)?;
    let replay_rows = load_string_rows(replay, &sql)?;
    report.rows_compared += snap_rows.len().max(replay_rows.len());

    use std::collections::BTreeMap;
    let mut snap_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut replay_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in snap_rows {
        let key = match row.first() {
            Some(k) => k.clone(),
            None => continue,
        };
        snap_map.insert(key, row);
    }
    for row in replay_rows {
        let key = match row.first() {
            Some(k) => k.clone(),
            None => continue,
        };
        replay_map.insert(key, row);
    }
    for key in snap_map.keys() {
        if !replay_map.contains_key(key) {
            push_diff(report, table, key, key_label, "present", "missing");
        }
    }
    for key in replay_map.keys() {
        if !snap_map.contains_key(key) {
            push_diff(report, table, key, key_label, "missing", "present");
        }
    }
    for (key, srow) in &snap_map {
        if let Some(rrow) = replay_map.get(key) {
            for (ci, col) in cols.iter().enumerate() {
                let exp = srow.get(ci + 1).map(|s| s.as_str()).unwrap_or("");
                let act = rrow.get(ci + 1).map(|s| s.as_str()).unwrap_or("");
                push_diff(report, table, key, col, exp, act);
            }
        }
    }
    Ok(())
}

fn load_string_rows(conn: &Connection, sql: &str) -> Result<Vec<Vec<String>>, String> {
    load_string_rows_params(conn, sql, [])
}

fn load_string_rows_params(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Vec<String>>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let col_count = stmt.column_count();
    let mut rows = Vec::new();
    let mut query = stmt.query(params).map_err(|e| e.to_string())?;
    while let Some(row) = query.next().map_err(|e| e.to_string())? {
        let mut vals = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let v: Option<String> = match row.get_ref(i).map_err(|e| e.to_string())? {
                rusqlite::types::ValueRef::Null => None,
                rusqlite::types::ValueRef::Integer(n) => Some(n.to_string()),
                rusqlite::types::ValueRef::Real(n) => Some(n.to_string()),
                rusqlite::types::ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
                rusqlite::types::ValueRef::Blob(_) => Some("<blob>".into()),
            };
            vals.push(v.unwrap_or_default());
        }
        rows.push(vals);
    }
    Ok(rows)
}

/// Persist last verify-replay outcome for spine-report inclusion.
pub fn write_verify_status(farm_dir: &Path, outcome: &VerifyOutcome) -> Result<(), String> {
    let path = farm_dir.join("last-verify-replay.txt");
    let report = outcome.report();
    let mut body = String::new();
    body.push_str(&format!("when={}\n", db::utc_now_rfc3339()));
    body.push_str(&format!("{}\n", outcome.summary_line()));
    body.push_str(&format!("outcome={}\n", outcome.summary_line()));
    body.push_str("exclusions:\n");
    for e in EXCLUSION_LIST {
        body.push_str(&format!("  - {e}\n"));
    }
    body.push_str(&format!("ledger_entries={}\n", KNOWN_DIVERGENCES.len()));
    body.push_str(&format!(
        "FLUSH LAG: {} event(s) pending\n",
        report.flush_lag
    ));
    body.push_str(&format!("watermark={}\n", report.watermark));
    body.push_str(&format!("live_max_seq={}\n", report.live_max_seq));
    body.push_str(&format!("events_read={}\n", report.events_read));
    body.push_str(&format!("events_replayed={}\n", report.events_replayed));
    body.push_str(&format!("tables_compared={}\n", report.tables_compared));
    body.push_str(&format!("rows_compared={}\n", report.rows_compared));
    body.push_str(&format!("known_divergences={}\n", report.known_divergences));
    body.push_str("matched_ledger:\n");
    if report.matched_ledger.is_empty() {
        body.push_str("  (none)\n");
    } else {
        for d in &report.matched_ledger {
            body.push_str(&format!(
                "  {} table={} key={} column={} seq_range={}\n",
                d.id, d.table, d.pk, d.column, d.seq_range
            ));
        }
    }
    if !report.unknown_diffs.is_empty() {
        body.push_str("unexplained_divergences:\n");
        for d in &report.unknown_diffs {
            body.push_str(&format!(
                "  table={} key={} column={} expected={} actual={}\n",
                d.table, d.key, d.field, d.expected, d.actual
            ));
        }
    }
    fs::write(path, body).map_err(|e| e.to_string())
}

/// A verify that could not run at all. Distinct from FAIL: nothing was
/// compared, so no divergence was found or ruled out.
pub const VERIFY_INCOMPLETE_PREFIX: &str = "VERIFY-REPLAY: COULD NOT COMPLETE";

/// Durable status for a run that errored before producing an outcome.
/// Replaces any older PASS file, so H4 cannot read an old verdict as
/// current.
pub fn write_verify_incomplete(farm_dir: &Path, reason: &str) -> Result<(), String> {
    let path = farm_dir.join("last-verify-replay.txt");
    let mut body = String::new();
    body.push_str(&format!("when={}\n", db::utc_now_rfc3339()));
    body.push_str(&format!("{VERIFY_INCOMPLETE_PREFIX} — {reason}\n"));
    body.push_str(&format!("outcome={VERIFY_INCOMPLETE_PREFIX} — {reason}\n"));
    body.push_str("exclusions:\n");
    for e in EXCLUSION_LIST {
        body.push_str(&format!("  - {e}\n"));
    }
    body.push_str("FLUSH LAG: unknown (nothing was compared)\n");
    fs::write(path, body).map_err(|e| e.to_string())
}

pub fn farm_dir_verify(farm_dir: &Path) -> Result<VerifyOutcome, String> {
    let farm_db = farm_dir.join("farm.db");
    let events = event_file::events_path(farm_dir);
    if !farm_db.is_file() {
        return Err(format!(
            "farm.db not found (looked in {})",
            farm_dir.display()
        ));
    }
    if !events.is_file() {
        return Err(format!(
            "events.jsonl not found (looked in {})",
            farm_dir.display()
        ));
    }
    crate::identity::assert_groundtruth_farm_dir(farm_dir)?;
    match verify_replay_paths(&farm_db, &events, farm_dir) {
        Ok(outcome) => {
            write_verify_status(farm_dir, &outcome)?;
            Ok(outcome)
        }
        Err(e) => {
            let _ = write_verify_incomplete(farm_dir, &e);
            Err(e)
        }
    }
}

#[cfg(test)]
mod open_flags_tests {
    use super::*;
    use crate::db;

    #[test]
    fn verify_source_open_flags_are_sqlite_open_read_only() {
        assert_eq!(VERIFY_SOURCE_OPEN_FLAGS, OpenFlags::SQLITE_OPEN_READ_ONLY);
    }

    #[test]
    fn verify_source_handle_rejects_writes() {
        let dir = std::env::temp_dir().join(format!("farm-os-ro-flags-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let farm = dir.join("farm.db");
        {
            let conn = db::open_and_migrate(&farm).unwrap();
            drop(conn);
        }
        let live = open_verify_source(&farm).unwrap();
        let write = live.execute("CREATE TABLE verify_ro_probe(x INTEGER)", []);
        assert!(write.is_err(), "read-only source handle must reject writes");
        let _ = fs::remove_dir_all(&dir);
    }

    fn empty_report() -> CompareReport {
        CompareReport {
            events_read: 0,
            events_replayed: 0,
            tables_compared: 0,
            rows_compared: 0,
            watermark: 0,
            live_max_seq: 0,
            flush_lag: 0,
            known_divergences: 0,
            matched_ledger: Vec::new(),
            unknown_diffs: Vec::new(),
            exclusions: Vec::new(),
        }
    }

    #[test]
    fn decide_outcome_zero_rows_compared_is_fail() {
        let mut report = empty_report();
        report.events_read = 1;
        report.events_replayed = 1;
        report.tables_compared = 4;
        report.rows_compared = 0;
        let outcome = decide_outcome(report);
        assert!(matches!(outcome, VerifyOutcome::Fail { .. }));
        assert_eq!(outcome.summary_line(), "VERIFY-REPLAY: FAIL");
    }

    #[test]
    fn decide_outcome_zero_events_replayed_is_fail() {
        let mut report = empty_report();
        report.tables_compared = 4;
        report.rows_compared = 10;
        let outcome = decide_outcome(report);
        assert!(matches!(outcome, VerifyOutcome::Fail { .. }));
    }

    #[test]
    fn decide_outcome_nonzero_flush_lag_is_fail() {
        let mut report = empty_report();
        report.events_read = 1;
        report.events_replayed = 1;
        report.tables_compared = 4;
        report.rows_compared = 10;
        report.watermark = 5;
        report.live_max_seq = 7;
        report.flush_lag = 2;
        let outcome = decide_outcome(report);
        assert!(outcome.exit_nonzero());
        assert_eq!(
            outcome.summary_line(),
            "VERIFY-REPLAY: FAIL — 2 event(s) pending flush."
        );
    }
}

#[cfg(test)]
mod scratch_tests {
    use super::*;
    use crate::db;
    use crate::event_file;

    fn bare_farm(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("farm-os-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let conn = db::open_and_migrate(&dir.join("farm.db")).unwrap();
        drop(conn);
        fs::write(event_file::events_path(&dir), "").unwrap();
        dir
    }

    fn verify_leftovers(farm_dir: &Path) -> Vec<String> {
        fs::read_dir(farm_dir.join("snapshots"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("verify-"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// PATH A: a scratch root that cannot hold `snapshots/verify-<uuid>/`
    /// fails before compare, and the error names that path. The bare
    /// "Access is denied. (os error 5)" named nothing and sent the operator
    /// to take ownership of a folder that was never the denied object.
    #[test]
    fn scratch_root_that_is_a_file_fails_naming_the_path() {
        let dir = bare_farm("scratch-file");
        let root = dir.join("not-a-folder");
        fs::write(&root, b"a file where the scratch root should be").unwrap();
        let err = verify_replay_paths(&dir.join("farm.db"), &event_file::events_path(&dir), &root)
            .unwrap_err();
        assert!(err.starts_with("could not create "), "{err}");
        assert!(err.contains(&root.display().to_string()), "{err}");
        assert!(
            !dir.join("snapshots").exists(),
            "nothing created beside a refused scratch"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// FIX A: scratch lives under the farm's `snapshots/` for the run only.
    /// A `snapshots/` that already held a backup keeps it and gains nothing;
    /// a farm dir that had no `snapshots/` ends without one.
    #[test]
    fn scratch_is_removed_per_run_and_snapshots_dir_is_left_as_found() {
        let with = bare_farm("scratch-with-snapshots");
        let snapshots = with.join("snapshots");
        fs::create_dir_all(&snapshots).unwrap();
        let keep = snapshots.join("farm-2026-01-01-000000.db");
        fs::write(&keep, b"kept").unwrap();
        let outcome = verify_replay_paths(
            &with.join("farm.db"),
            &event_file::events_path(&with),
            &with,
        );
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(
            keep.is_file(),
            "an existing backup must survive the scratch"
        );
        assert!(
            verify_leftovers(&with).is_empty(),
            "{:?}",
            verify_leftovers(&with)
        );

        let without = bare_farm("scratch-without-snapshots");
        let outcome = verify_replay_paths(
            &without.join("farm.db"),
            &event_file::events_path(&without),
            &without,
        );
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(
            !without.join("snapshots").exists(),
            "a snapshots/ created only for the scratch is given back"
        );

        let _ = fs::remove_dir_all(&with);
        let _ = fs::remove_dir_all(&without);
    }
}
