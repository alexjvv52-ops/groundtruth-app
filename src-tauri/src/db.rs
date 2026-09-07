use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct Db(pub Arc<Mutex<Connection>>);

/// Paths for the live farm file and automatic snapshots.
pub struct FarmPaths {
    pub farm_db_path: PathBuf,
    pub folder_path: PathBuf,
    pub snapshots_dir: PathBuf,
}

pub const SCHEMA_VERSION: i32 = 44;

/// Frozen tray id seeded into `open_v1_in_memory` (Phase 1 Ruling 2).
#[cfg(test)]
pub const FIXTURE_V1_TRAY_ID: &str = "b370c73f-9627-4684-aea2-beb59e662fb9";
/// Frozen tray id seeded into `open_v2_in_memory` (Phase 1 Ruling 2).
#[cfg(test)]
pub const FIXTURE_V2_TRAY_ID: &str = "e57c0a5d-2930-468f-875f-0df5b7257afc";

const SCHEMA_V8_EVENT_LOG_SQL: &str = r#"
ALTER TABLE event_log ADD COLUMN origin TEXT;
ALTER TABLE event_log ADD COLUMN event_domain TEXT;
ALTER TABLE event_log ADD COLUMN event_class TEXT;
ALTER TABLE event_log ADD COLUMN reverses_event_id TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_event_log_id ON event_log(id);

CREATE TRIGGER IF NOT EXISTS event_log_before_insert
BEFORE INSERT ON event_log
BEGIN
  SELECT CASE
    WHEN NEW.id IS NULL OR NEW.id = ''
      THEN RAISE(ABORT, 'event_log.id required')
    WHEN NEW.origin IS NULL OR NEW.origin NOT IN ('farm_os', 'commercial_app')
      THEN RAISE(ABORT, 'event_log.origin invalid')
    WHEN NEW.event_domain IS NULL OR NEW.event_domain NOT IN ('grow', 'register')
      THEN RAISE(ABORT, 'event_log.event_domain invalid')
    WHEN NEW.event_domain = 'register' AND (
      NEW.event_class IS NULL OR NEW.event_class NOT IN (
        'money_out', 'physical_consumption', 'mileage', 'asset_register',
        'sale_farm_os_path', 'capacity_commitment', 'snapshot'
      )
    )
      THEN RAISE(ABORT, 'event_log.event_class invalid for register')
    WHEN NEW.event_domain = 'grow' AND NEW.event_class IS NOT NULL
      THEN RAISE(ABORT, 'event_log.event_class must be NULL for grow')
    WHEN NEW.event_domain = 'grow' AND (
      NEW.kind IS NULL OR NEW.kind NOT IN (
        'tray.sown', 'trays.advanced', 'trays.harvested', 'tray.discarded',
        'trays.discarded', 'recount.applied', 'undo', 'dev.backdated',
        'attention.resolved', 'stripe.session_paid', 'stripe.refunded',
        'stripe.disputed'
      )
    )
      THEN RAISE(ABORT, 'event_log.kind invalid for grow')
  END;
END;

CREATE TRIGGER IF NOT EXISTS event_log_before_update
BEFORE UPDATE ON event_log
BEGIN
  SELECT CASE
    WHEN OLD.id IS NOT NEW.id
      OR OLD.seq IS NOT NEW.seq
      OR OLD.origin IS NOT NEW.origin
      OR OLD.event_domain IS NOT NEW.event_domain
      OR OLD.event_class IS NOT NEW.event_class
      OR OLD.kind IS NOT NEW.kind
      THEN RAISE(ABORT, 'event_log immutable columns')
  END;
END;

CREATE TRIGGER IF NOT EXISTS event_log_before_delete
BEFORE DELETE ON event_log
BEGIN
  SELECT RAISE(ABORT, 'event_log is append-only');
END;
"#;

/// Event_log triggers — generated from `event_partition` so the flush guard
/// cannot drift from INSERT/UPDATE enforcement. Named schema_v9 historically;
/// v10+ reinstalls the same generator after Kind changes.
fn schema_v9_event_log_triggers_sql() -> String {
    crate::event_partition::schema_v9_event_log_triggers_sql()
}

const SCHEMA_V10_COST_EVENTS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS cost_events (
  event_id            TEXT PRIMARY KEY,
  origin              TEXT NOT NULL CHECK (origin IN ('farm_os', 'commercial_app')),
  date_paid           TEXT NOT NULL,
  amount_cents        INTEGER NOT NULL CHECK (amount_cents > 0),
  payee               TEXT NOT NULL CHECK (length(trim(payee)) > 0),
  canonical_category  TEXT NOT NULL,
  schedule_f_line     TEXT NOT NULL,
  schedule_c_line     TEXT NOT NULL,
  descriptor          TEXT NOT NULL DEFAULT '',
  quantity            REAL,
  unit_price_cents    INTEGER,
  delivery_date       TEXT,
  invoice_reference   TEXT,
  receipt_file_ref    TEXT,
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL
);

CREATE TRIGGER IF NOT EXISTS cost_events_before_insert
BEFORE INSERT ON cost_events
BEGIN
  SELECT CASE
    WHEN (instr(lower(NEW.schedule_f_line), 'other') > 0
          OR instr(lower(NEW.schedule_c_line), 'other') > 0)
         AND (NEW.descriptor IS NULL OR trim(NEW.descriptor) = '')
      THEN RAISE(ABORT, 'cost_events.descriptor required for other line')
    WHEN NEW.amount_cents IS NULL OR NEW.amount_cents <= 0
      THEN RAISE(ABORT, 'cost_events.amount_cents must be positive')
    WHEN NEW.payee IS NULL OR trim(NEW.payee) = ''
      THEN RAISE(ABORT, 'cost_events.payee required')
    WHEN NEW.date_paid > date('now', 'localtime')
      THEN RAISE(ABORT, 'cost_events.date_paid cannot be future')
  END;
END;
"#;

/// Flat append-only mirror of consumption.physical event fields (Track 4).
const SCHEMA_V11_CONSUMPTION_EVENTS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS consumption_events (
  event_id              TEXT PRIMARY KEY,
  origin                TEXT NOT NULL CHECK (origin = 'farm_os'),
  occurred_at           TEXT NOT NULL,
  variety_or_item       TEXT NOT NULL CHECK (length(trim(variety_or_item)) > 0),
  unit                  TEXT NOT NULL CHECK (length(trim(unit)) > 0),
  quantity              REAL NOT NULL CHECK (quantity > 0),
  linked_cost_event_id  TEXT,
  notes                 TEXT
);
"#;

/// Mileage trips — per-trip, dated, MILES. No dollar column exists here and
/// none may be added (Track 4 residual).
const SCHEMA_V13_MILEAGE_TRIPS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS mileage_trips (
  trip_id        TEXT PRIMARY KEY,
  origin         TEXT NOT NULL CHECK (origin = 'farm_os'),
  trip_date      TEXT NOT NULL,
  miles          REAL NOT NULL CHECK (miles > 0),
  purpose        TEXT,
  voided_at      TEXT,
  last_event_id  TEXT NOT NULL,
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mileage_trips_date ON mileage_trips(trip_date);

CREATE TRIGGER IF NOT EXISTS mileage_trips_before_insert
BEFORE INSERT ON mileage_trips
BEGIN
  SELECT CASE
    WHEN NEW.miles IS NULL OR NEW.miles <= 0
      THEN RAISE(ABORT, 'mileage_trips.miles must be positive')
    WHEN NEW.trip_date IS NULL OR NEW.trip_date = ''
      THEN RAISE(ABORT, 'mileage_trips.trip_date required')
    WHEN NEW.trip_date > date('now', 'localtime')
      THEN RAISE(ABORT, 'mileage_trips.trip_date cannot be future')
  END;
END;

CREATE TRIGGER IF NOT EXISTS mileage_trips_before_update
BEFORE UPDATE ON mileage_trips
BEGIN
  SELECT CASE
    WHEN NEW.trip_id IS NOT OLD.trip_id
      THEN RAISE(ABORT, 'mileage_trips.trip_id immutable')
    WHEN NEW.origin IS NOT OLD.origin
      THEN RAISE(ABORT, 'mileage_trips.origin immutable')
    WHEN NEW.created_at IS NOT OLD.created_at
      THEN RAISE(ABORT, 'mileage_trips.created_at immutable')
    WHEN NEW.miles IS NULL OR NEW.miles <= 0
      THEN RAISE(ABORT, 'mileage_trips.miles must be positive')
  END;
END;
"#;

/// Asset register — the four operator fields and nothing derived. Adding a
/// computed column here is a BOOKS-BOUNDARY violation (Track 4 residual).
const SCHEMA_V13_ASSETS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS assets (
  asset_id              TEXT PRIMARY KEY,
  origin                TEXT NOT NULL CHECK (origin = 'farm_os'),
  description           TEXT NOT NULL CHECK (length(trim(description)) > 0),
  placed_in_service_on  TEXT NOT NULL,
  cost_cents            INTEGER NOT NULL CHECK (cost_cents > 0),
  disposal_date         TEXT,
  last_event_id         TEXT NOT NULL,
  created_at            TEXT NOT NULL,
  updated_at            TEXT NOT NULL,
  -- voided_at is last so the 2.2 shape-convergence ALTER produces DDL
  -- identical to a fresh CREATE. Do not reorder.
  voided_at             TEXT
);

CREATE TRIGGER IF NOT EXISTS assets_before_insert
BEFORE INSERT ON assets
BEGIN
  SELECT CASE
    WHEN NEW.cost_cents IS NULL OR NEW.cost_cents <= 0
      THEN RAISE(ABORT, 'assets.cost_cents must be positive')
    WHEN NEW.description IS NULL OR trim(NEW.description) = ''
      THEN RAISE(ABORT, 'assets.description required')
    WHEN NEW.placed_in_service_on > date('now', 'localtime')
      THEN RAISE(ABORT, 'assets.placed_in_service_on cannot be future')
    WHEN NEW.disposal_date IS NOT NULL
         AND NEW.disposal_date < NEW.placed_in_service_on
      THEN RAISE(ABORT, 'assets.disposal_date before placed_in_service_on')
  END;
END;

CREATE TRIGGER IF NOT EXISTS assets_before_update
BEFORE UPDATE ON assets
BEGIN
  SELECT CASE
    WHEN NEW.asset_id IS NOT OLD.asset_id
      THEN RAISE(ABORT, 'assets.asset_id immutable')
    WHEN NEW.origin IS NOT OLD.origin
      THEN RAISE(ABORT, 'assets.origin immutable')
    WHEN NEW.created_at IS NOT OLD.created_at
      THEN RAISE(ABORT, 'assets.created_at immutable')
    WHEN NEW.cost_cents IS NULL OR NEW.cost_cents <= 0
      THEN RAISE(ABORT, 'assets.cost_cents must be positive')
    WHEN NEW.disposal_date IS NOT NULL
         AND NEW.disposal_date < NEW.placed_in_service_on
      THEN RAISE(ABORT, 'assets.disposal_date before placed_in_service_on')
  END;
END;
"#;

/// Money arriving. Mirrors cost_events, plus the correction columns the
/// mileage and asset registers use. Computes nothing.
const SCHEMA_V14_INCOME_EVENTS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS income_events (
  income_id           TEXT PRIMARY KEY,
  origin              TEXT NOT NULL CHECK (origin IN ('farm_os','commercial_app')),
  date_received       TEXT NOT NULL,
  amount_cents        INTEGER NOT NULL CHECK (amount_cents > 0),
  source              TEXT NOT NULL CHECK (length(trim(source)) > 0),
  canonical_category  TEXT NOT NULL,
  schedule_f_line     TEXT NOT NULL,
  schedule_c_line     TEXT NOT NULL,
  descriptor          TEXT NOT NULL DEFAULT '',
  receipt_file_ref    TEXT,
  last_event_id       TEXT NOT NULL,
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL,
  voided_at           TEXT
);
CREATE INDEX IF NOT EXISTS idx_income_events_date ON income_events(date_received);
CREATE TRIGGER IF NOT EXISTS income_events_before_insert
BEFORE INSERT ON income_events
BEGIN
  SELECT CASE
    WHEN NEW.amount_cents IS NULL OR NEW.amount_cents <= 0
      THEN RAISE(ABORT, 'income_events.amount_cents must be positive')
    WHEN NEW.source IS NULL OR trim(NEW.source) = ''
      THEN RAISE(ABORT, 'income_events.source required')
    WHEN (instr(lower(NEW.schedule_f_line), 'other') > 0
          OR instr(lower(NEW.schedule_c_line), 'other') > 0)
         AND (NEW.descriptor IS NULL OR trim(NEW.descriptor) = '')
      THEN RAISE(ABORT, 'income_events.descriptor required for other line')
    WHEN NEW.date_received > date('now', 'localtime')
      THEN RAISE(ABORT, 'income_events.date_received cannot be future')
  END;
END;
CREATE TRIGGER IF NOT EXISTS income_events_before_update
BEFORE UPDATE ON income_events
BEGIN
  SELECT CASE
    WHEN NEW.income_id IS NOT OLD.income_id
      THEN RAISE(ABORT, 'income_events.income_id immutable')
    WHEN NEW.origin IS NOT OLD.origin
      THEN RAISE(ABORT, 'income_events.origin immutable')
    WHEN NEW.created_at IS NOT OLD.created_at
      THEN RAISE(ABORT, 'income_events.created_at immutable')
    WHEN NEW.amount_cents IS NULL OR NEW.amount_cents <= 0
      THEN RAISE(ABORT, 'income_events.amount_cents must be positive')
  END;
END;
"#;

/// Permanent money_corrections trail + immutable cost_events update gate.
/// Spine columns are added separately and idempotently (fixtures rewind
/// user_version after a current-schema open).
const SCHEMA_V18_MONEY_CORRECTIONS_SQL: &str = r#"
CREATE TRIGGER IF NOT EXISTS cost_events_before_update
BEFORE UPDATE ON cost_events
BEGIN
  SELECT CASE
    WHEN NEW.event_id IS NOT OLD.event_id
      THEN RAISE(ABORT, 'cost_events.event_id immutable')
    WHEN NEW.origin IS NOT OLD.origin
      THEN RAISE(ABORT, 'cost_events.origin immutable')
  END;
END;

CREATE TABLE IF NOT EXISTS money_corrections (
  correction_event_id TEXT PRIMARY KEY,
  target_event_id     TEXT NOT NULL,
  track               TEXT NOT NULL CHECK (track IN ('cost','income')),
  action              TEXT NOT NULL CHECK (action IN ('corrected','voided')),
  before_json         TEXT NOT NULL,
  after_json          TEXT,
  before_amount_cents INTEGER NOT NULL,
  after_amount_cents  INTEGER,
  before_date         TEXT NOT NULL,
  after_date          TEXT,
  before_payee        TEXT NOT NULL,
  after_payee         TEXT,
  reason              TEXT,
  corrected_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_money_corrections_target
  ON money_corrections (target_event_id);
CREATE TRIGGER IF NOT EXISTS money_corrections_before_update
BEFORE UPDATE ON money_corrections
BEGIN SELECT RAISE(ABORT, 'money_corrections is append-only'); END;
CREATE TRIGGER IF NOT EXISTS money_corrections_before_delete
BEFORE DELETE ON money_corrections
BEGIN SELECT RAISE(ABORT, 'money_corrections is append-only'); END;
"#;

/// Narrow money_corrections.track CHECK to expense-only. SQLite cannot ALTER
/// a CHECK in place, so the table is rebuilt. v18 CREATE is left frozen.
const SCHEMA_V19_MONEY_CORRECTIONS_NARROW_SQL: &str = r#"
DROP TRIGGER IF EXISTS money_corrections_before_update;
DROP TRIGGER IF EXISTS money_corrections_before_delete;

CREATE TABLE money_corrections_new (
  correction_event_id TEXT PRIMARY KEY,
  target_event_id     TEXT NOT NULL,
  track               TEXT NOT NULL CHECK (track IN ('cost')),
  action              TEXT NOT NULL CHECK (action IN ('corrected','voided')),
  before_json         TEXT NOT NULL,
  after_json          TEXT,
  before_amount_cents INTEGER NOT NULL,
  after_amount_cents  INTEGER,
  before_date         TEXT NOT NULL,
  after_date          TEXT,
  before_payee        TEXT NOT NULL,
  after_payee         TEXT,
  reason              TEXT,
  corrected_at        TEXT NOT NULL
);
INSERT INTO money_corrections_new
  (correction_event_id, target_event_id, track, action,
   before_json, after_json, before_amount_cents, after_amount_cents,
   before_date, after_date, before_payee, after_payee, reason, corrected_at)
SELECT correction_event_id, target_event_id, track, action,
       before_json, after_json, before_amount_cents, after_amount_cents,
       before_date, after_date, before_payee, after_payee, reason, corrected_at
FROM money_corrections;
DROP TABLE money_corrections;
ALTER TABLE money_corrections_new RENAME TO money_corrections;

CREATE INDEX IF NOT EXISTS idx_money_corrections_target
  ON money_corrections (target_event_id);
CREATE TRIGGER money_corrections_before_update
BEFORE UPDATE ON money_corrections
BEGIN SELECT RAISE(ABORT, 'money_corrections is append-only'); END;
CREATE TRIGGER money_corrections_before_delete
BEFORE DELETE ON money_corrections
BEGIN SELECT RAISE(ABORT, 'money_corrections is append-only'); END;
"#;

/// Phase 5 wholesale order book (GT-D14). Prices live on order lines, never
/// on marketing venue rows.
const SCHEMA_V21_WHOLESALE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS wholesale_orders (
  id TEXT PRIMARY KEY,
  venue_id TEXT NOT NULL,
  harvest_date TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('ordered','delivered','paid','voided','written_off')),
  ordered_on TEXT NOT NULL,
  delivered_on TEXT NULL,
  paid_on TEXT NULL,
  income_event_id TEXT NULL,
  voided_at TEXT NULL,
  void_reason TEXT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS wholesale_order_lines (
  order_id TEXT NOT NULL,
  crop_id TEXT NOT NULL,
  trays INTEGER NOT NULL CHECK (trays >= 1),
  price_cents_per_tray INTEGER NULL CHECK (price_cents_per_tray IS NULL OR price_cents_per_tray >= 1),
  PRIMARY KEY (order_id, crop_id)
);
"#;

const SCHEMA_V22_CROP_ALIASES_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS crop_aliases (
  former_name TEXT PRIMARY KEY,
  crop_id     TEXT NOT NULL REFERENCES crops(id),
  created_at  TEXT NOT NULL
);
"#;

const SCHEMA_V23_SHELF_CAPACITY_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS shelf_capacity (
  id             TEXT PRIMARY KEY CHECK (id = 'default'),
  light_slots    INTEGER,
  blackout_slots INTEGER,
  updated_at     TEXT NOT NULL
);
"#;

const SCHEMA_V24_REPUTATION_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS mkt_review_requests (
  venue_id    TEXT PRIMARY KEY REFERENCES mkt_venues(venue_id),
  decided_on  TEXT NOT NULL,
  outcome     TEXT NOT NULL CHECK (outcome IN ('asked', 'skipped')),
  created_at  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS business_profile (
  id              TEXT PRIMARY KEY CHECK (id = 'default'),
  gbp_verified_on TEXT,
  updated_at      TEXT NOT NULL
);
"#;

pub const SCHEMA_V25_SCANS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS scan_config (
  id            INTEGER PRIMARY KEY CHECK (id = 1),
  endpoint_url  TEXT,
  pull_token    TEXT,
  configured_at TEXT
);
INSERT OR IGNORE INTO scan_config (id) VALUES (1);

CREATE TABLE IF NOT EXISTS scan_observations (
  id                 TEXT PRIMARY KEY,
  fetched_at         TEXT NOT NULL,
  url                TEXT NOT NULL,
  http_status        INTEGER NULL,
  ok                 INTEGER NOT NULL CHECK (ok IN (0, 1)),
  served_at          TEXT NULL,
  new_count          INTEGER NULL,
  first_available_id INTEGER NULL,
  max_scan_id        INTEGER NULL,
  error              TEXT NULL
);
CREATE INDEX IF NOT EXISTS idx_scan_observations_fetched_at
    ON scan_observations (fetched_at);
"#;

const SCHEMA_V26_WRITE_OFFS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS wholesale_write_offs (
  event_id        TEXT PRIMARY KEY,
  order_id        TEXT NOT NULL,
  income_event_id TEXT NOT NULL,
  shortfall_cents INTEGER NOT NULL CHECK (shortfall_cents >= 1),
  category        TEXT NOT NULL CHECK (category IN (
                    'sales_discount','quality_spoilage',
                    'pricing_or_billing_error','customer_goodwill','other')),
  reason          TEXT NULL,
  written_off_on  TEXT NOT NULL,
  created_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wholesale_write_offs_order
  ON wholesale_write_offs (order_id);
CREATE TRIGGER IF NOT EXISTS wholesale_write_offs_before_update
BEFORE UPDATE ON wholesale_write_offs
BEGIN SELECT RAISE(ABORT, 'wholesale_write_offs is append-only'); END;
CREATE TRIGGER IF NOT EXISTS wholesale_write_offs_before_delete
BEFORE DELETE ON wholesale_write_offs
BEGIN SELECT RAISE(ABORT, 'wholesale_write_offs is append-only'); END;
"#;

pub(crate) const SCHEMA_V27_UNAPPLIED_FACTS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS stripe_unapplied_facts (
  event_id       TEXT PRIMARY KEY,
  stripe_object  TEXT NOT NULL CHECK (stripe_object IN
                   ('checkout_session','refund','dispute')),
  stripe_id      TEXT NOT NULL,
  status         TEXT NOT NULL CHECK (status IN
                   ('unmatched','unrecorded','no_paid_order')),
  amount_cents   INTEGER NULL,
  currency       TEXT NULL,
  stripe_created INTEGER NOT NULL,
  observed_at    TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_identity
  ON stripe_unapplied_facts (stripe_object, stripe_id, status);
CREATE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_observed
  ON stripe_unapplied_facts (observed_at);
CREATE TRIGGER IF NOT EXISTS stripe_unapplied_facts_before_update
BEFORE UPDATE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
CREATE TRIGGER IF NOT EXISTS stripe_unapplied_facts_before_delete
BEFORE DELETE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
"#;

/// C2 (INT-002). The refund gate needs four refusal reasons the v27 CHECK
/// does not admit. SQLite cannot ALTER a CHECK, so the table is rebuilt on
/// the v19 / v36 precedent: triggers off, new table, copy, drop, rename,
/// indexes and append-only triggers back. v27 CREATE is left frozen. No
/// Kind changes, so the event_log triggers are not reinstalled.
pub(crate) const SCHEMA_V37_UNAPPLIED_FACTS_WIDEN_SQL: &str = r#"
DROP TRIGGER IF EXISTS stripe_unapplied_facts_before_update;
DROP TRIGGER IF EXISTS stripe_unapplied_facts_before_delete;

CREATE TABLE stripe_unapplied_facts_new (
  event_id       TEXT PRIMARY KEY,
  stripe_object  TEXT NOT NULL CHECK (stripe_object IN
                   ('checkout_session','refund','dispute')),
  stripe_id      TEXT NOT NULL,
  status         TEXT NOT NULL CHECK (status IN
                   ('unmatched','unrecorded','no_paid_order',
                    'amount_partial','not_terminal','terminal_failed','not_comparable')),
  amount_cents   INTEGER NULL,
  currency       TEXT NULL,
  stripe_created INTEGER NOT NULL,
  observed_at    TEXT NOT NULL
);
INSERT INTO stripe_unapplied_facts_new
  (event_id, stripe_object, stripe_id, status, amount_cents, currency,
   stripe_created, observed_at)
SELECT event_id, stripe_object, stripe_id, status, amount_cents, currency,
       stripe_created, observed_at
FROM stripe_unapplied_facts;
DROP TABLE stripe_unapplied_facts;
ALTER TABLE stripe_unapplied_facts_new RENAME TO stripe_unapplied_facts;

CREATE UNIQUE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_identity
  ON stripe_unapplied_facts (stripe_object, stripe_id, status);
CREATE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_observed
  ON stripe_unapplied_facts (observed_at);
CREATE TRIGGER stripe_unapplied_facts_before_update
BEFORE UPDATE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
CREATE TRIGGER stripe_unapplied_facts_before_delete
BEFORE DELETE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
"#;

/// TILL-A (GT-D22). The minted Payment Link lives on the order it bills: three
/// nullable columns written only by apply_wholesale_link_minted from the
/// wholesale.link_minted payload + event.created_at, and compared by
/// verify-replay through WHOLESALE_ORDERS_COLUMNS. ADD COLUMN only — the v36
/// CHECK and every existing row are untouched.
const SCHEMA_V38_PAYMENT_LINK_SQL: &str = r#"
ALTER TABLE wholesale_orders ADD COLUMN payment_link_id TEXT NULL;
ALTER TABLE wholesale_orders ADD COLUMN payment_link_url TEXT NULL;
ALTER TABLE wholesale_orders ADD COLUMN payment_link_minted_at TEXT NULL;
"#;
/// TILL-A (GT-D22). Four named facts for money the poll saw on this farm's own
/// Payment Link and would not book. Same rebuild as v37 (triggers off, new
/// table, copy, drop, rename, indexes and append-only triggers back); rows kept.
pub(crate) const SCHEMA_V38_UNAPPLIED_FACTS_WIDEN_SQL: &str = r#"
DROP TRIGGER IF EXISTS stripe_unapplied_facts_before_update;
DROP TRIGGER IF EXISTS stripe_unapplied_facts_before_delete;

CREATE TABLE stripe_unapplied_facts_new (
  event_id       TEXT PRIMARY KEY,
  stripe_object  TEXT NOT NULL CHECK (stripe_object IN
                   ('checkout_session','refund','dispute')),
  stripe_id      TEXT NOT NULL,
  status         TEXT NOT NULL CHECK (status IN
                   ('unmatched','unrecorded','no_paid_order',
                    'amount_partial','not_terminal','terminal_failed','not_comparable',
                    'wholesale_not_delivered','wholesale_already_settled',
                    'wholesale_amount_mismatch','wholesale_payment',
                    'leftover_already_paid','leftover_amount_mismatch')),
  amount_cents   INTEGER NULL,
  currency       TEXT NULL,
  stripe_created INTEGER NOT NULL,
  observed_at    TEXT NOT NULL
);
INSERT INTO stripe_unapplied_facts_new
  (event_id, stripe_object, stripe_id, status, amount_cents, currency,
   stripe_created, observed_at)
SELECT event_id, stripe_object, stripe_id, status, amount_cents, currency,
       stripe_created, observed_at
FROM stripe_unapplied_facts;
DROP TABLE stripe_unapplied_facts;
ALTER TABLE stripe_unapplied_facts_new RENAME TO stripe_unapplied_facts;

CREATE UNIQUE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_identity
  ON stripe_unapplied_facts (stripe_object, stripe_id, status);
CREATE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_observed
  ON stripe_unapplied_facts (observed_at);
CREATE TRIGGER stripe_unapplied_facts_before_update
BEFORE UPDATE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
CREATE TRIGGER stripe_unapplied_facts_before_delete
BEFORE DELETE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
"#;

/// LO-A (GT-D24). The leftover listing's durable home: one row per
/// (crop_id, harvested_on), written only by apply_leftover_listed from the
/// leftover.listed payload + event.created_at and compared by verify-replay
/// through LEFTOVER_LISTINGS_COLUMNS. Delete-proof. Capacity-free by decision:
/// nothing here touches trays, orders, or offers.
const SCHEMA_V39_LEFTOVER_LISTINGS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS leftover_listings (
  listing_id   TEXT PRIMARY KEY,
  crop_id      TEXT NOT NULL REFERENCES crops(id),
  harvested_on TEXT NOT NULL,
  listed_oz    REAL NOT NULL CHECK (listed_oz > 0),
  created_at   TEXT NOT NULL,
  UNIQUE (crop_id, harvested_on)
);
CREATE INDEX IF NOT EXISTS idx_leftover_listings_crop_day
  ON leftover_listings (crop_id, harvested_on);
CREATE TRIGGER IF NOT EXISTS leftover_listings_before_delete
BEFORE DELETE ON leftover_listings
BEGIN SELECT RAISE(ABORT, 'leftover_listings is append-only'); END;
"#;

/// LO-B (GT-D24-B). The listing's money face: six nullable columns written
/// only by apply_leftover_link_minted / apply_leftover_paid from their
/// payloads + event.created_at. The before_update guard freezes the five LO-A
/// columns; the delete guard stays. Nothing here touches trays, orders, or
/// offers.
const SCHEMA_V40_LEFTOVER_LINK_SQL: &str = r#"
CREATE TRIGGER IF NOT EXISTS leftover_listings_before_update
BEFORE UPDATE OF listing_id, crop_id, harvested_on, listed_oz, created_at ON leftover_listings
BEGIN SELECT RAISE(ABORT, 'leftover_listings core columns are frozen'); END;
"#;

/// INV-A. farm_config — the farm's display name for the invoice header.
/// stripe_config-shaped: one row, id = 1, written by Settings on the PC,
/// never by an apply_*; declared on projection::verify::EXCLUSION_LIST.
const SCHEMA_V41_FARM_CONFIG_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS farm_config (
  id           INTEGER PRIMARY KEY CHECK (id = 1),
  display_name TEXT
);
INSERT OR IGNORE INTO farm_config (id) VALUES (1);
"#;

/// R-10 (PACK-RF-PI). A refund Stripe listed with neither a payment intent nor
/// a session id has no key any later poll can match, so it needs a reason of
/// its own — otherwise the walk passes it in silence and the desk never speaks.
/// Same rebuild as v37 / v38 (triggers off, new table, copy, drop, rename,
/// indexes and append-only triggers back); rows kept. No Kind changes, so the
/// event_log triggers are not reinstalled.
pub(crate) const SCHEMA_V42_UNAPPLIED_FACTS_WIDEN_SQL: &str = r#"
DROP TRIGGER IF EXISTS stripe_unapplied_facts_before_update;
DROP TRIGGER IF EXISTS stripe_unapplied_facts_before_delete;
CREATE TABLE stripe_unapplied_facts_new (
  event_id       TEXT PRIMARY KEY,
  stripe_object  TEXT NOT NULL CHECK (stripe_object IN
                   ('checkout_session','refund','dispute')),
  stripe_id      TEXT NOT NULL,
  status         TEXT NOT NULL CHECK (status IN
                   ('unmatched','unrecorded','no_paid_order',
                    'amount_partial','not_terminal','terminal_failed','not_comparable',
                    'wholesale_not_delivered','wholesale_already_settled',
                    'wholesale_amount_mismatch','wholesale_payment',
                    'leftover_already_paid','leftover_amount_mismatch',
                    'no_payment_intent')),
  amount_cents   INTEGER NULL,
  currency       TEXT NULL,
  stripe_created INTEGER NOT NULL,
  observed_at    TEXT NOT NULL
);
INSERT INTO stripe_unapplied_facts_new
  (event_id, stripe_object, stripe_id, status, amount_cents, currency,
   stripe_created, observed_at)
SELECT event_id, stripe_object, stripe_id, status, amount_cents, currency,
       stripe_created, observed_at
FROM stripe_unapplied_facts;
DROP TABLE stripe_unapplied_facts;
ALTER TABLE stripe_unapplied_facts_new RENAME TO stripe_unapplied_facts;
CREATE UNIQUE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_identity
  ON stripe_unapplied_facts (stripe_object, stripe_id, status);
CREATE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_observed
  ON stripe_unapplied_facts (observed_at);
CREATE TRIGGER stripe_unapplied_facts_before_update
BEFORE UPDATE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
CREATE TRIGGER stripe_unapplied_facts_before_delete
BEFORE DELETE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
"#;

/// J4 REF-DUP-FACT. A second Stripe session under a cart reference an order
/// already carries was answered as AlreadyApplied and left no trace — paid
/// money with no row behind it. It now joins the status CHECK as
/// `duplicate_reference`, keyed by the new session id. Same rebuild as
/// v37 / v38 / v42 (triggers off, new table, copy, drop, rename, indexes and
/// append-only triggers back); rows kept. No Kind changes, so the event_log
/// triggers are not reinstalled.
pub(crate) const SCHEMA_V44_UNAPPLIED_FACTS_WIDEN_SQL: &str = r#"
DROP TRIGGER IF EXISTS stripe_unapplied_facts_before_update;
DROP TRIGGER IF EXISTS stripe_unapplied_facts_before_delete;
CREATE TABLE stripe_unapplied_facts_new (
  event_id       TEXT PRIMARY KEY,
  stripe_object  TEXT NOT NULL CHECK (stripe_object IN
                   ('checkout_session','refund','dispute')),
  stripe_id      TEXT NOT NULL,
  status         TEXT NOT NULL CHECK (status IN
                   ('unmatched','unrecorded','no_paid_order',
                    'amount_partial','not_terminal','terminal_failed','not_comparable',
                    'wholesale_not_delivered','wholesale_already_settled',
                    'wholesale_amount_mismatch','wholesale_payment',
                    'leftover_already_paid','leftover_amount_mismatch',
                    'no_payment_intent','duplicate_reference')),
  amount_cents   INTEGER NULL,
  currency       TEXT NULL,
  stripe_created INTEGER NOT NULL,
  observed_at    TEXT NOT NULL
);
INSERT INTO stripe_unapplied_facts_new
  (event_id, stripe_object, stripe_id, status, amount_cents, currency,
   stripe_created, observed_at)
SELECT event_id, stripe_object, stripe_id, status, amount_cents, currency,
       stripe_created, observed_at
FROM stripe_unapplied_facts;
DROP TABLE stripe_unapplied_facts;
ALTER TABLE stripe_unapplied_facts_new RENAME TO stripe_unapplied_facts;
CREATE UNIQUE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_identity
  ON stripe_unapplied_facts (stripe_object, stripe_id, status);
CREATE INDEX IF NOT EXISTS idx_stripe_unapplied_facts_observed
  ON stripe_unapplied_facts (observed_at);
CREATE TRIGGER stripe_unapplied_facts_before_update
BEFORE UPDATE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
CREATE TRIGGER stripe_unapplied_facts_before_delete
BEFORE DELETE ON stripe_unapplied_facts
BEGIN SELECT RAISE(ABORT, 'stripe_unapplied_facts is append-only'); END;
"#;

/// SEED-A (GT-D25). The seed receipt's durable home: one row per
/// seed.received event, written only by apply_seed_received from the payload
/// + event.created_at and compared by verify-replay through
/// SEED_RECEIPTS_COLUMNS. Add-only by trigger (no update, no delete). The
/// crop is a foreign key by id — never a name. Capacity-free by decision:
/// nothing here touches trays, consumption_events, orders, or offers.
const SCHEMA_V43_SEED_RECEIPTS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS seed_receipts (
  receipt_id   TEXT PRIMARY KEY,
  crop_id      TEXT NOT NULL REFERENCES crops(id),
  received_oz  REAL NOT NULL CHECK (received_oz > 0),
  created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_seed_receipts_crop
  ON seed_receipts (crop_id, created_at);
CREATE TRIGGER IF NOT EXISTS seed_receipts_before_update
BEFORE UPDATE ON seed_receipts
BEGIN SELECT RAISE(ABORT, 'seed_receipts is append-only'); END;
CREATE TRIGGER IF NOT EXISTS seed_receipts_before_delete
BEFORE DELETE ON seed_receipts
BEGIN SELECT RAISE(ABORT, 'seed_receipts is append-only'); END;
"#;

/// Customer QR fence 1 (brief 2026-08-17, ruling 2): the opaque per-drop token
/// lives on the sample row. Schema-only — no GT-D number, no event kind, so the
/// event_log triggers are untouched. Partial unique: one token = one drop;
/// historic drops carry NULL and are not indexed.
const SCHEMA_V28_SAMPLE_TOKEN_SQL: &str = r#"
CREATE UNIQUE INDEX IF NOT EXISTS idx_mkt_samples_token
  ON mkt_samples(token) WHERE token IS NOT NULL;
"#;

/// GT-D17 — the standing-request candidate's durable home. Rows are written only
/// by apply_standing_requested (standing.requested); decided_at / outcome are
/// written only by standing.request_decided (Fence 3, same GT-D). Delete-proof:
/// a candidate is a commercial-demand fact and leaves a reconstructable trail.
/// PK only — several candidates may carry one token; each chef tap is its own
/// durable signal.
const SCHEMA_V29_STANDING_REQUESTS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS mkt_standing_requests (
  request_id      TEXT PRIMARY KEY,
  token           TEXT NOT NULL,
  venue_id        TEXT NOT NULL,
  varieties       TEXT NOT NULL,
  bags_per_cycle  INTEGER NOT NULL CHECK (bags_per_cycle >= 1),
  requested_at    TEXT NOT NULL,
  contact         TEXT NULL,
  created_at      TEXT NOT NULL,
  decided_at      TEXT NULL,
  outcome         TEXT NULL CHECK (outcome IS NULL OR outcome IN ('accepted', 'dismissed'))
);
CREATE INDEX IF NOT EXISTS idx_mkt_standing_requests_token
  ON mkt_standing_requests (token);
CREATE TRIGGER IF NOT EXISTS mkt_standing_requests_before_delete
BEFORE DELETE ON mkt_standing_requests
BEGIN SELECT RAISE(ABORT, 'mkt_standing_requests is append-only'); END;
"#;

/// Customer QR fence 5. The scans precedent, twice: one observation row per
/// pull (success or failure; cursor = MAX(max_seq) over ok pulls) and one
/// delete-proof refusal row per pulled candidate the desktop would not turn
/// into a candidate. Neither is derived from events (EXCLUSION_LIST). No
/// event kind, so the event_log triggers are untouched.
pub const SCHEMA_V31_STANDING_PULL_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS standing_pull_observations (
  id                  TEXT PRIMARY KEY,
  fetched_at          TEXT NOT NULL,
  url                 TEXT NOT NULL,
  http_status         INTEGER NULL,
  ok                  INTEGER NOT NULL CHECK (ok IN (0, 1)),
  served_at           TEXT NULL,
  rows_received       INTEGER NULL,
  new_count           INTEGER NULL,
  known_count         INTEGER NULL,
  refused_count       INTEGER NULL,
  first_available_seq INTEGER NULL,
  max_seq             INTEGER NULL,
  error               TEXT NULL
);
CREATE INDEX IF NOT EXISTS idx_standing_pull_observations_fetched_at
    ON standing_pull_observations (fetched_at);
CREATE TABLE IF NOT EXISTS standing_pull_refusals (
  id             TEXT PRIMARY KEY,
  seq            INTEGER NOT NULL,
  request_id     TEXT NOT NULL,
  token          TEXT NOT NULL,
  bags_per_cycle INTEGER NOT NULL,
  requested_at   TEXT NOT NULL,
  reason         TEXT NOT NULL CHECK (reason IN ('unknown_token', 'invalid')),
  observed_at    TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS standing_pull_refusals_before_delete
BEFORE DELETE ON standing_pull_refusals
BEGIN SELECT RAISE(ABORT, 'standing_pull_refusals is append-only'); END;
"#;

/// GT-D19 (R3). One row per harvest.covered event; append-only; the newest row
/// per (crop_id, harvested_on) — by created_at then coverage_id, both frozen in
/// the payload — is the current mark. Compared by verify-replay.
const SCHEMA_V32_HARVEST_COVERAGE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS harvest_coverage (
  coverage_id  TEXT PRIMARY KEY,
  crop_id      TEXT NOT NULL,
  harvested_on TEXT NOT NULL,
  covers       TEXT NOT NULL,
  created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_harvest_coverage_crop_day
  ON harvest_coverage (crop_id, harvested_on, created_at);
CREATE TRIGGER IF NOT EXISTS harvest_coverage_before_delete
BEFORE DELETE ON harvest_coverage
BEGIN SELECT RAISE(ABORT, 'harvest_coverage is append-only'); END;
"#;

/// GT-D20 (rack-side fence 1). The phone proposal's durable home. Rows are
/// written only by phone::apply_phone_proposed; the decided columns only by
/// phone::apply_phone_proposal_decided (the mkt_standing_requests shape).
/// Delete-proof; frozen fields immutable; a decision is final. gate_reason
/// mirrors phone::GATE_REASONS. Compared by verify-replay.
const SCHEMA_V33_PHONE_PROPOSALS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS phone_proposals (
  proposal_id       TEXT PRIMARY KEY,
  device_id         TEXT NOT NULL,
  verb              TEXT NOT NULL CHECK (verb IN ('move_to_light', 'harvest')),
  crop_id           TEXT NOT NULL,
  quantity          INTEGER NOT NULL CHECK (quantity >= 1),
  actual_yield_oz   REAL NULL,
  phone_captured_at TEXT NOT NULL,
  note              TEXT NULL,
  created_at        TEXT NOT NULL,
  decided_at        TEXT NULL,
  outcome           TEXT NULL CHECK (outcome IS NULL OR outcome IN ('accepted', 'discarded')),
  accepted_quantity INTEGER NULL,
  accepted_yield_oz REAL NULL,
  applied_event_ids TEXT NULL,
  gate_reason       TEXT NULL CHECK (gate_reason IS NULL OR gate_reason IN (
    'not_enough_blackout', 'not_enough_light', 'weight_not_positive',
    'same_day_harvest_exists', 'capture_in_future', 'capture_before_sow',
    'capture_before_light', 'batch_mismatch'))
);
CREATE INDEX IF NOT EXISTS idx_phone_proposals_open
  ON phone_proposals (decided_at, created_at);
CREATE TRIGGER IF NOT EXISTS phone_proposals_before_update
BEFORE UPDATE ON phone_proposals
BEGIN
  SELECT CASE
    WHEN OLD.proposal_id IS NOT NEW.proposal_id
      OR OLD.device_id IS NOT NEW.device_id
      OR OLD.verb IS NOT NEW.verb
      OR OLD.crop_id IS NOT NEW.crop_id
      OR OLD.quantity IS NOT NEW.quantity
      OR OLD.actual_yield_oz IS NOT NEW.actual_yield_oz
      OR OLD.phone_captured_at IS NOT NEW.phone_captured_at
      OR OLD.note IS NOT NEW.note
      OR OLD.created_at IS NOT NEW.created_at
      THEN RAISE(ABORT, 'phone_proposals frozen fields are immutable')
    WHEN OLD.decided_at IS NOT NULL
      THEN RAISE(ABORT, 'phone_proposals decision is final')
  END;
END;
CREATE TRIGGER IF NOT EXISTS phone_proposals_before_delete
BEFORE DELETE ON phone_proposals
BEGIN SELECT RAISE(ABORT, 'phone_proposals is append-only'); END;
"#;

/// Rack-side fence 2 (GT-D21). field_devices is reference data (the scan_config
/// precedent): the token is shown once inside the pairing link and only its
/// SHA-256 hash is stored; a secret never enters the ledger. At most one live
/// Admin by partial unique index; identity immutable; retirement final; rows
/// never deleted so a retired token still resolves to `retired_device`. The pull
/// log and refusal trace follow standing_pull (fence 5). None derived from
/// events (EXCLUSION_LIST).
pub const SCHEMA_V34_FIELD_DEVICES_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS field_devices (
  device_id  TEXT PRIMARY KEY,
  label      TEXT NULL,
  role       TEXT NOT NULL CHECK (role IN ('admin')),
  token_hash TEXT NOT NULL UNIQUE,
  paired_at  TEXT NOT NULL,
  retired_at TEXT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_field_devices_one_live_admin
  ON field_devices (role) WHERE role = 'admin' AND retired_at IS NULL;
CREATE TRIGGER IF NOT EXISTS field_devices_before_update
BEFORE UPDATE ON field_devices
BEGIN
  SELECT CASE
    WHEN OLD.device_id IS NOT NEW.device_id OR OLD.role IS NOT NEW.role
      OR OLD.token_hash IS NOT NEW.token_hash OR OLD.paired_at IS NOT NEW.paired_at
      THEN RAISE(ABORT, 'field_devices identity is immutable')
    WHEN OLD.retired_at IS NOT NULL AND NEW.retired_at IS NOT OLD.retired_at
      THEN RAISE(ABORT, 'field_devices retirement is final')
  END;
END;
CREATE TRIGGER IF NOT EXISTS field_devices_before_delete
BEFORE DELETE ON field_devices
BEGIN SELECT RAISE(ABORT, 'field_devices is append-only'); END;
CREATE TABLE IF NOT EXISTS phone_pull_observations (
  id                  TEXT PRIMARY KEY,
  fetched_at          TEXT NOT NULL,
  url                 TEXT NOT NULL,
  http_status         INTEGER NULL,
  ok                  INTEGER NOT NULL CHECK (ok IN (0, 1)),
  served_at           TEXT NULL,
  rows_received       INTEGER NULL,
  new_count           INTEGER NULL,
  known_count         INTEGER NULL,
  refused_count       INTEGER NULL,
  first_available_seq INTEGER NULL,
  max_seq             INTEGER NULL,
  error               TEXT NULL
);
CREATE INDEX IF NOT EXISTS idx_phone_pull_observations_fetched_at
    ON phone_pull_observations (fetched_at);
CREATE TABLE IF NOT EXISTS phone_pull_refusals (
  id                TEXT PRIMARY KEY,
  seq               INTEGER NOT NULL,
  proposal_id       TEXT NOT NULL,
  device_token_hash TEXT NOT NULL,
  verb              TEXT NOT NULL,
  crop_id           TEXT NOT NULL,
  quantity          INTEGER NOT NULL,
  actual_yield_oz   REAL NULL,
  phone_captured_at TEXT NOT NULL,
  reason            TEXT NOT NULL CHECK (reason IN ('unknown_device', 'retired_device', 'invalid', 'unknown_crop')),
  observed_at       TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS phone_pull_refusals_before_delete
BEFORE DELETE ON phone_pull_refusals
BEGIN SELECT RAISE(ABORT, 'phone_pull_refusals is append-only'); END;
"#;

const SCHEMA_V36_BAD_DEBT_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS wholesale_bad_debts (
  event_id       TEXT PRIMARY KEY,
  order_id       TEXT NOT NULL,
  amount_cents   INTEGER NOT NULL CHECK (amount_cents >= 1),
  written_off_on TEXT NOT NULL,
  created_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wholesale_bad_debts_order
  ON wholesale_bad_debts (order_id);
CREATE TRIGGER IF NOT EXISTS wholesale_bad_debts_before_update
BEFORE UPDATE ON wholesale_bad_debts
BEGIN SELECT RAISE(ABORT, 'wholesale_bad_debts is append-only'); END;
CREATE TRIGGER IF NOT EXISTS wholesale_bad_debts_before_delete
BEFORE DELETE ON wholesale_bad_debts
BEGIN SELECT RAISE(ABORT, 'wholesale_bad_debts is append-only'); END;
"#;

/// Rebuild wholesale_orders so the CHECK can include 'written_off'.
/// SQLite cannot ALTER a CHECK. Columns match WHOLESALE_ORDERS_COLUMNS.
/// Base SCHEMA_V21 carried no indexes or triggers on wholesale_orders;
/// no foreign keys reference wholesale_orders(id).
const SCHEMA_V36_WHOLESALE_ORDERS_REBUILD_SQL: &str = r#"
CREATE TABLE wholesale_orders_new (
  id TEXT PRIMARY KEY,
  venue_id TEXT NOT NULL,
  harvest_date TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('ordered','delivered','paid','voided','written_off')),
  ordered_on TEXT NOT NULL,
  delivered_on TEXT NULL,
  paid_on TEXT NULL,
  income_event_id TEXT NULL,
  voided_at TEXT NULL,
  void_reason TEXT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
INSERT INTO wholesale_orders_new
  (id, venue_id, harvest_date, state, ordered_on, delivered_on,
   paid_on, income_event_id, voided_at, void_reason, created_at, updated_at)
SELECT id, venue_id, harvest_date, state, ordered_on, delivered_on,
       paid_on, income_event_id, voided_at, void_reason, created_at, updated_at
FROM wholesale_orders;
DROP TABLE wholesale_orders;
ALTER TABLE wholesale_orders_new RENAME TO wholesale_orders;
"#;

fn cost_events_has_column(conn: &Connection, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("SELECT 1 FROM pragma_table_info('cost_events') WHERE name = ?1")
        .map_err(|e| e.to_string())?;
    let found = stmt.exists(params![column]).map_err(|e| e.to_string())?;
    Ok(found)
}

fn mkt_stages_has_column(conn: &Connection, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("SELECT 1 FROM pragma_table_info('mkt_stages') WHERE name = ?1")
        .map_err(|e| e.to_string())?;
    let found = stmt.exists(params![column]).map_err(|e| e.to_string())?;
    Ok(found)
}

fn mkt_samples_has_column(conn: &Connection, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("SELECT 1 FROM pragma_table_info('mkt_samples') WHERE name = ?1")
        .map_err(|e| e.to_string())?;
    let found = stmt.exists(params![column]).map_err(|e| e.to_string())?;
    Ok(found)
}

fn leftover_listings_has_column(conn: &Connection, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("SELECT 1 FROM pragma_table_info('leftover_listings') WHERE name = ?1")
        .map_err(|e| e.to_string())?;
    let found = stmt.exists(params![column]).map_err(|e| e.to_string())?;
    Ok(found)
}

const SCHEMA_V15_STOREFRONT_AND_HEALTH_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS storefront_observations (
    id              TEXT PRIMARY KEY,
    fetched_at      TEXT NOT NULL,
    url             TEXT NOT NULL,
    http_status     INTEGER,
    content_sha256  TEXT,
    ok              INTEGER NOT NULL,
    payload         TEXT,
    error           TEXT
);
CREATE INDEX IF NOT EXISTS idx_storefront_observations_fetched_at
    ON storefront_observations (fetched_at);

CREATE TABLE IF NOT EXISTS health_evidence (
    id         TEXT PRIMARY KEY,
    check_id   TEXT NOT NULL,
    ran_at     TEXT NOT NULL,
    ok         INTEGER NOT NULL,
    detail     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_health_evidence_check ON health_evidence (check_id, ran_at);
"#;

const SCHEMA_V16_MARKETING_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS mkt_venues (
    venue_id     TEXT PRIMARY KEY,
    name         TEXT NOT NULL CHECK (length(trim(name)) > 0),
    venue_type   TEXT NOT NULL,
    contact      TEXT,
    phone        TEXT,
    address      TEXT,
    note         TEXT,
    archived_at  TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS mkt_samples (
    sample_id   TEXT PRIMARY KEY,
    venue_id    TEXT NOT NULL,
    dropped_on  TEXT NOT NULL,
    varieties   TEXT NOT NULL,
    pack_count  INTEGER NOT NULL CHECK (pack_count > 0),
    note        TEXT,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mkt_samples_venue ON mkt_samples (venue_id, dropped_on);
CREATE TABLE IF NOT EXISTS mkt_touches (
    touch_id    TEXT PRIMARY KEY,
    venue_id    TEXT NOT NULL,
    touched_on  TEXT NOT NULL,
    channel     TEXT NOT NULL,
    outcome     TEXT,
    note        TEXT,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mkt_touches_venue ON mkt_touches (venue_id, touched_on);
CREATE TABLE IF NOT EXISTS mkt_followups (
    followup_id TEXT PRIMARY KEY,
    venue_id    TEXT NOT NULL,
    due_on      TEXT NOT NULL,
    what        TEXT NOT NULL,
    cleared_at  TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mkt_followups_due ON mkt_followups (due_on, cleared_at);
"#;

const SCHEMA_V17_CADENCE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS mkt_stages (
    venue_id     TEXT PRIMARY KEY,
    stage        TEXT NOT NULL CHECK (stage IN
                   ('scouted','sampled','talking','trial','standing',
                    'dormant','passed')),
    trays_week   INTEGER,
    varieties    TEXT,
    changed_on   TEXT NOT NULL,
    note         TEXT,
    updated_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS mkt_reviews (
    observation_id TEXT PRIMARY KEY,
    observed_on    TEXT NOT NULL,
    count          INTEGER NOT NULL CHECK (count >= 0),
    source         TEXT NOT NULL,
    created_at     TEXT NOT NULL
);
"#;

/// Operator-supplied seed rates (oz per 10x20 tray). NULL = no proposal.
/// Matched by existing crop id. Do not invent values for NULL lines.
const OPERATOR_SEED_RATES: &[(&str, Option<f64>)] = &[
    ("dun-peas", Some(8.0)),
    ("mellow-mix", Some(0.6)),
    ("spicy-mix", Some(0.6)),
    ("red-arrow-radish", Some(1.0)),
    ("purple-kohlrabi", Some(0.6)),
    ("sunflower", None),
    ("broccoli", None),
    ("kale", None),
];

const DROP_EVENT_LOG_TRIGGERS_SQL: &str = r#"
DROP TRIGGER IF EXISTS event_log_before_insert;
DROP TRIGGER IF EXISTS event_log_before_update;
DROP TRIGGER IF EXISTS event_log_before_delete;
"#;

const SCHEMA_V1_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS crops (
  id                TEXT PRIMARY KEY,
  name              TEXT NOT NULL UNIQUE,
  growth_days       INTEGER NOT NULL,
  blackout_days     INTEGER NOT NULL,
  expected_yield_oz REAL    NOT NULL,
  sort_order        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS trays (
  id                    TEXT PRIMARY KEY,
  crop_id               TEXT NOT NULL REFERENCES crops(id),
  state                 TEXT NOT NULL CHECK (state IN
                          ('planned','sown','blackout','light','harvested','discarded')),
  quantity              INTEGER NOT NULL CHECK (quantity >= 1),
  growth_days_at_sow    INTEGER,
  blackout_days_at_sow  INTEGER,
  planned_on            TEXT,
  sown_on               TEXT,
  blackout_on           TEXT,
  light_on              TEXT,
  harvested_on          TEXT,
  discarded_on          TEXT,
  actual_yield_oz       REAL,
  created_at            TEXT NOT NULL,
  updated_at            TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_trays_state ON trays(state);
CREATE INDEX IF NOT EXISTS idx_trays_sown_on ON trays(sown_on);

CREATE TABLE IF NOT EXISTS event_log (
  seq         INTEGER PRIMARY KEY AUTOINCREMENT,
  id          TEXT    NOT NULL UNIQUE,
  kind        TEXT    NOT NULL,
  entity_type TEXT    NOT NULL,
  entity_id   TEXT    NOT NULL,
  payload     TEXT    NOT NULL,
  inverse     TEXT    NOT NULL,
  undone_at   TEXT,
  undoes_seq  INTEGER REFERENCES event_log(seq),
  created_at  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_event_log_undone ON event_log(undone_at, seq);
"#;

const SCHEMA_V2_ATTENTION_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS attention (
  id           TEXT PRIMARY KEY,
  kind         TEXT NOT NULL,
  entity_type  TEXT,
  entity_id    TEXT,
  message      TEXT NOT NULL,
  actions      TEXT NOT NULL,
  created_at   TEXT NOT NULL,
  resolved_at  TEXT,
  resolved_by  TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_attention_open
  ON attention(kind, entity_id) WHERE resolved_at IS NULL;
"#;

const SCHEMA_V3_MONEY_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS stripe_config (
  id             INTEGER PRIMARY KEY CHECK (id = 1),
  restricted_key TEXT,
  account_id     TEXT,
  account_name   TEXT,
  mode           TEXT CHECK (mode IN ('test','live')),
  configured_at  TEXT
);

CREATE TABLE IF NOT EXISTS offers (
  id              TEXT PRIMARY KEY,
  harvest_date    TEXT NOT NULL,
  crop_id         TEXT NOT NULL REFERENCES crops(id),
  price_cents     INTEGER NOT NULL CHECK (price_cents > 0),
  stripe_price_id TEXT,
  stripe_link_id  TEXT,
  stripe_link_url TEXT,
  created_at      TEXT NOT NULL,
  UNIQUE (harvest_date, crop_id)
);

CREATE TABLE IF NOT EXISTS orders (
  id                    TEXT PRIMARY KEY,
  stripe_session_id     TEXT NOT NULL UNIQUE,
  stripe_payment_intent TEXT,
  harvest_date          TEXT NOT NULL,
  crop_id               TEXT NOT NULL REFERENCES crops(id),
  quantity              INTEGER NOT NULL CHECK (quantity >= 1),
  amount_cents          INTEGER NOT NULL,
  currency              TEXT NOT NULL,
  customer_email        TEXT,
  state                 TEXT NOT NULL CHECK (state IN ('paid','refunded','disputed')),
  capacity_consumed     INTEGER NOT NULL,
  paid_at               TEXT NOT NULL,
  created_at            TEXT NOT NULL,
  updated_at            TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS stripe_cursor (
  id             INTEGER PRIMARY KEY CHECK (id = 1),
  sessions_since TEXT,
  refunds_since  TEXT,
  disputes_since TEXT,
  last_poll_ok   TEXT,
  last_poll_err  TEXT
);

INSERT OR IGNORE INTO stripe_config (id) VALUES (1);
INSERT OR IGNORE INTO stripe_cursor (id) VALUES (1);
"#;

// TODO(stage-3:self-correcting-estimates): growth_days and blackout_days are seeded
// estimates. Replace with values derived from the grower's own logged harvests,
// and label them "estimate" in the UI until his history takes over.
// seed_rate_oz_per_tray: operator-supplied (Track 4); NULL = blank pre-fill.
#[allow(clippy::type_complexity)] // H-7 Class E: a seed-data table, not a signature. Nothing here to narrow.
const SEED_CROPS: &[(&str, &str, i64, i64, f64, i64, Option<f64>)] = &[
    ("dun-peas", "Dun peas", 9, 3, 10.0, 1, Some(8.0)),
    ("mellow-mix", "Mellow mix", 8, 3, 7.0, 2, Some(0.6)),
    ("spicy-mix", "Spicy mix", 8, 3, 6.5, 3, Some(0.6)),
    (
        "red-arrow-radish",
        "Red arrow radish",
        7,
        3,
        8.0,
        4,
        Some(1.0),
    ),
    (
        "purple-kohlrabi",
        "Purple kohlrabi",
        9,
        4,
        5.5,
        5,
        Some(0.6),
    ),
    ("sunflower", "Sunflower", 9, 3, 11.0, 6, None),
    ("broccoli", "Broccoli", 8, 4, 5.0, 7, None),
    ("kale", "Kale", 9, 4, 5.0, 8, None),
];

pub fn open_and_migrate(path: &std::path::Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    configure(&conn)?;
    crate::identity::stamp_or_refuse(&conn)?;
    migrate(&conn)?;
    // Spine report after every migration (and every open that runs migrate).
    if let Some(parent) = path.parent() {
        crate::event_file::on_app_start(&conn, parent);
    }
    Ok(conn)
}

#[cfg(test)]
pub fn open_in_memory() -> Result<Connection, String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

pub(crate) fn configure(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// On-disk safety copy before a schema migration. Uses VACUUM INTO (WAL-safe).
/// Skipped for :memory: databases. Failure refuses the migration.
fn safety_snapshot_before_migration(conn: &Connection, label: &str) -> Result<(), String> {
    let file: String = conn
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if file.is_empty() {
        return Ok(());
    }
    let path = PathBuf::from(&file);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("farm");
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut dest = parent.join(format!("{stem}-pre-{label}-{stamp}.db"));
    let mut n = 1u32;
    while dest.exists() {
        dest = parent.join(format!("{stem}-pre-{label}-{stamp}-{n}.db"));
        n += 1;
        if n > 10_000 {
            return Err(format!(
                "pre-migration snapshot failed; refusing migration {label}: could not find unique path"
            ));
        }
    }
    let dest_str = dest.to_str().ok_or_else(|| {
        format!(
            "pre-migration snapshot failed; refusing migration {label}: path is not valid UTF-8"
        )
    })?;
    conn.execute("VACUUM INTO ?1", rusqlite::params![dest_str])
        .map_err(|e| format!("pre-migration snapshot failed; refusing migration {label}: {e}"))?;
    Ok(())
}

/// Rows the Phase 2 spine corrective UPDATEs would touch (dry-run; no writes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpineBackfillPreview {
    pub null_origin: i64,
    pub sale_rows_needing_register: i64,
    pub snapshot_rows_needing_register: i64,
    pub grow_rows_needing_domain: i64,
}

impl SpineBackfillPreview {
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub fn total(&self) -> i64 {
        self.null_origin
            + self.sale_rows_needing_register
            + self.snapshot_rows_needing_register
            + self.grow_rows_needing_domain
    }
}

/// Standalone dry-run for migration 9 corrective UPDATEs. Prints a report;
/// writes nothing. Invoke via `cargo test --lib spine_backfill_dry_run -- --nocapture`.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn spine_backfill_dry_run(conn: &Connection) -> Result<SpineBackfillPreview, String> {
    let preview = preview_spine_backfill(conn)?;
    println!("spine backfill dry-run (writes nothing):");
    println!(
        "  null_origin                         = {}",
        preview.null_origin
    );
    println!(
        "  sale_rows_needing_register           = {}",
        preview.sale_rows_needing_register
    );
    println!(
        "  snapshot_rows_needing_register       = {}",
        preview.snapshot_rows_needing_register
    );
    println!(
        "  grow_rows_needing_domain             = {}",
        preview.grow_rows_needing_domain
    );
    println!(
        "  total_predicate_matches             = {}",
        preview.total()
    );
    Ok(preview)
}

pub fn preview_spine_backfill(conn: &Connection) -> Result<SpineBackfillPreview, String> {
    let null_origin: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE origin IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let sale_rows_needing_register: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log
             WHERE kind IN ('stripe.session_paid', 'stripe.refunded', 'stripe.disputed')
               AND (
                 event_domain IS NULL
                 OR event_domain != 'register'
                 OR event_class IS NULL
                 OR event_class != 'sale_farm_os_path'
               )",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let snapshot_rows_needing_register: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log
             WHERE kind = 'snapshot.taken'
               AND (
                 event_domain IS NULL
                 OR event_domain != 'register'
                 OR event_class IS NULL
                 OR event_class != 'snapshot'
               )",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let grow_rows_needing_domain: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log
             WHERE kind IN (
               'tray.sown', 'trays.advanced', 'trays.harvested', 'tray.discarded',
               'trays.discarded', 'recount.applied', 'undo', 'dev.backdated',
               'attention.resolved'
             )
             AND (
               event_domain IS NULL
               OR event_domain != 'grow'
               OR event_class IS NOT NULL
             )",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(SpineBackfillPreview {
        null_origin,
        sale_rows_needing_register,
        snapshot_rows_needing_register,
        grow_rows_needing_domain,
    })
}

/// Canonical serialization of sorted (id, origin, event_domain, event_class).
/// Equal digests ⇔ byte-identical tuples.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn spine_tuple_digest(conn: &Connection) -> Result<String, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id,
                    IFNULL(origin, ''),
                    IFNULL(event_domain, ''),
                    IFNULL(event_class, '')
             FROM event_log
             ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(format!(
                "{}|{}|{}|{}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    for row in rows {
        lines.push(row.map_err(|e| e.to_string())?);
    }
    Ok(lines.join("\n"))
}

/// Corrective UPDATEs for the Phase 2 spine partition. Caller must ensure
/// BEFORE UPDATE triggers are dropped (migration 9) or that fills are permitted.
pub fn apply_spine_backfill(conn: &Connection) -> Result<usize, String> {
    let mut touched = 0usize;
    touched += conn
        .execute(
            "UPDATE event_log SET origin = 'farm_os' WHERE origin IS NULL",
            [],
        )
        .map_err(|e| e.to_string())?;
    touched += conn
        .execute(
            "UPDATE event_log
             SET event_domain = 'register', event_class = 'sale_farm_os_path'
             WHERE kind IN ('stripe.session_paid', 'stripe.refunded', 'stripe.disputed')
               AND (
                 event_domain IS NULL
                 OR event_domain != 'register'
                 OR event_class IS NULL
                 OR event_class != 'sale_farm_os_path'
               )",
            [],
        )
        .map_err(|e| e.to_string())?;
    touched += conn
        .execute(
            "UPDATE event_log
             SET event_domain = 'register', event_class = 'snapshot'
             WHERE kind = 'snapshot.taken'
               AND (
                 event_domain IS NULL
                 OR event_domain != 'register'
                 OR event_class IS NULL
                 OR event_class != 'snapshot'
               )",
            [],
        )
        .map_err(|e| e.to_string())?;
    touched += conn
        .execute(
            "UPDATE event_log
             SET event_domain = 'grow', event_class = NULL
             WHERE kind IN (
               'tray.sown', 'trays.advanced', 'trays.harvested', 'tray.discarded',
               'trays.discarded', 'recount.applied', 'undo', 'dev.backdated',
               'attention.resolved'
             )
             AND (
               event_domain IS NULL
               OR event_domain != 'grow'
               OR event_class IS NOT NULL
             )",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(touched)
}

pub fn migrate(conn: &Connection) -> Result<(), String> {
    let mut version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;

    if version < 1 {
        conn.execute_batch(SCHEMA_V1_SQL)
            .map_err(|e| e.to_string())?;
        seed_crops(conn)?;
        conn.pragma_update(None, "user_version", 1)
            .map_err(|e| e.to_string())?;
        version = 1;
    }

    if version < 2 {
        conn.execute_batch(SCHEMA_V2_ATTENTION_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 2)
            .map_err(|e| e.to_string())?;
        version = 2;
    }

    if version < 3 {
        conn.execute_batch(SCHEMA_V3_MONEY_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 3)
            .map_err(|e| e.to_string())?;
        version = 3;
    }

    if version < 4 {
        // Poll failure streak + watermark for "new paid orders" on Today.
        conn.execute_batch(
            "ALTER TABLE stripe_cursor ADD COLUMN poll_fail_count INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE stripe_cursor ADD COLUMN last_app_open TEXT;",
        )
        .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 4)
            .map_err(|e| e.to_string())?;
        version = 4;
    }

    if version < 5 {
        // One orders row per line; uniqueness is (session, crop). Harvest-date Payment Links.
        conn.execute_batch(
            r#"
            CREATE TABLE orders_new (
              id                    TEXT PRIMARY KEY,
              stripe_session_id     TEXT NOT NULL,
              stripe_payment_intent TEXT,
              harvest_date          TEXT NOT NULL,
              crop_id               TEXT NOT NULL REFERENCES crops(id),
              quantity              INTEGER NOT NULL CHECK (quantity >= 1),
              amount_cents          INTEGER NOT NULL,
              currency              TEXT NOT NULL,
              customer_email        TEXT,
              state                 TEXT NOT NULL CHECK (state IN ('paid','refunded','disputed')),
              capacity_consumed     INTEGER NOT NULL,
              paid_at               TEXT NOT NULL,
              created_at            TEXT NOT NULL,
              updated_at            TEXT NOT NULL,
              UNIQUE (stripe_session_id, crop_id)
            );
            INSERT INTO orders_new
              (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
               quantity, amount_cents, currency, customer_email, state,
               capacity_consumed, paid_at, created_at, updated_at)
            SELECT id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
                   quantity, amount_cents, currency, customer_email, state,
                   capacity_consumed, paid_at, created_at, updated_at
            FROM orders;
            DROP TABLE orders;
            ALTER TABLE orders_new RENAME TO orders;

            CREATE TABLE IF NOT EXISTS harvest_links (
              harvest_date    TEXT PRIMARY KEY,
              stripe_link_id  TEXT NOT NULL,
              stripe_link_url TEXT NOT NULL,
              line_signature  TEXT NOT NULL,
              created_at      TEXT NOT NULL
            );
            "#,
        )
        .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 5)
            .map_err(|e| e.to_string())?;
        version = 5;
    }

    if version < 6 {
        // Second idempotency key: browser-minted cart reference (nullable; partial unique).
        conn.execute_batch(
            "ALTER TABLE orders ADD COLUMN client_reference TEXT;
             CREATE UNIQUE INDEX IF NOT EXISTS idx_orders_reference
               ON orders(client_reference, crop_id)
               WHERE client_reference IS NOT NULL;",
        )
        .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 6)
            .map_err(|e| e.to_string())?;
        version = 6;
    }

    if version < 7 {
        // Public checkout Worker URL (not a secret). Cart posts here from the shop page.
        conn.execute_batch("ALTER TABLE stripe_config ADD COLUMN checkout_endpoint_url TEXT;")
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 7)
            .map_err(|e| e.to_string())?;
        version = 7;
    }

    if version < 8 {
        // Origin spine on event_log. Additive only — existing rows stay NULL until back-fill.
        safety_snapshot_before_migration(conn, "v8")?;
        conn.execute_batch(SCHEMA_V8_EVENT_LOG_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 8)
            .map_err(|e| e.to_string())?;
        version = 8;
    }

    if version < 9 {
        // Phase 2: classify money-path kinds as register, back-fill NULL spine
        // fields, shrink grow whitelist. Legitimate only while nothing has been
        // flushed to events.jsonl — after the first flush this in-place rewrite
        // of already-emitted values is impossible (events.jsonl is Phase 3).
        safety_snapshot_before_migration(conn, "v9")?;
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        let preview = preview_spine_backfill(conn)?;
        let touched = apply_spine_backfill(conn)?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 9)
            .map_err(|e| e.to_string())?;
        version = 9;
        // Persist migration-9 outcome for the operator spine report. Best-effort;
        // migration itself already succeeded.
        if let Err(e) = write_migration_9_outcome(conn, &preview, touched) {
            eprintln!("migration-9 outcome file not written: {e}");
        }
    }

    if version < 10 {
        // Track 3: cost_events state table + regenerate event_log triggers so the
        // new cost.money_out kind is whitelisted. Existing rows untouched.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V10_COST_EVENTS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 10)
            .map_err(|e| e.to_string())?;
        version = 10;
    }

    if version < 11 {
        // Track 4: seed_rate_oz_per_tray on crops + consumption_events mirror +
        // regenerate event_log triggers so consumption.physical is whitelisted.
        // One bump covers both (Track 3 / v10 shape). Existing rows untouched
        // except the new nullable rate column population.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch("ALTER TABLE crops ADD COLUMN seed_rate_oz_per_tray REAL NULL;")
            .map_err(|e| e.to_string())?;
        apply_operator_seed_rates(conn)?;
        conn.execute_batch(SCHEMA_V11_CONSUMPTION_EVENTS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 11)
            .map_err(|e| e.to_string())?;
        version = 11;
    }

    if version < 12 {
        // Track 4: sow_event_id on consumption_events (payload sowEventId mirror).
        // Existing rows stay NULL — no backfill.
        conn.execute_batch("ALTER TABLE consumption_events ADD COLUMN sow_event_id TEXT;")
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 12)
            .map_err(|e| e.to_string())?;
        version = 12;
    }

    if version < 13 {
        // Track 4 residual: mileage_trips + assets projection tables, and
        // regenerate event_log triggers so the five new register kinds are
        // whitelisted. Without the trigger reinstall every mileage/asset write
        // aborts at the database. Existing rows and columns untouched.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V13_MILEAGE_TRIPS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V13_ASSETS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 13)
            .map_err(|e| e.to_string())?;
        version = 13;
    }

    // v13 shape convergence — assets.voided_at was added to the v13 shape
    // while v13 was still unreleased. A development database that migrated to
    // v13 before that edit has the nine-column table and would never receive
    // the column, because the v13 block above is already satisfied. Additive,
    // idempotent, and safe to delete once no such database exists.
    if version == 13 && !assets_has_voided_at_column(conn)? {
        conn.execute_batch("ALTER TABLE assets ADD COLUMN voided_at TEXT;")
            .map_err(|e| e.to_string())?;
    }

    if version < 14 {
        // Money-in track: income_events + regenerate event_log triggers so the
        // three income kinds AND the new money_in event class are whitelisted.
        // Without the reinstall every income write aborts at the database.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V14_INCOME_EVENTS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 14)
            .map_err(|e| e.to_string())?;
        version = 14;
    }

    if version < 15 {
        // Wave 1: remote storefront observation cache + health evidence rows.
        // Neither table is derived from the event log.
        conn.execute_batch(SCHEMA_V15_STOREFRONT_AND_HEALTH_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 15)
            .map_err(|e| e.to_string())?;
        version = 15;
    }

    if version < 16 {
        // Wave 2: marketing domain. Reinstall event_log triggers so the seven
        // marketing kinds are whitelisted, then create the four mkt_ tables.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V16_MARKETING_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 16)
            .map_err(|e| e.to_string())?;
        version = 16;
    }

    if version < 17 {
        // Wave 3: GT-D11 stage + reviews kinds. Reinstall triggers, then create
        // mkt_stages and mkt_reviews. No money columns.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V17_CADENCE_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 17)
            .map_err(|e| e.to_string())?;
        version = 17;
    }

    if version < 18 {
        // Expense correct/void + money_corrections trail. Reinstall triggers so
        // the two new MoneyOut kinds are whitelisted, then add spine columns.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        // Idempotent ALTERs: open_in_memory fixtures rewind user_version after
        // a current-schema open, so these columns may already exist.
        if !cost_events_has_column(conn, "last_event_id")? {
            conn.execute_batch("ALTER TABLE cost_events ADD COLUMN last_event_id TEXT;")
                .map_err(|e| e.to_string())?;
        }
        if !cost_events_has_column(conn, "voided_at")? {
            conn.execute_batch("ALTER TABLE cost_events ADD COLUMN voided_at TEXT;")
                .map_err(|e| e.to_string())?;
        }
        conn.execute_batch(
            "UPDATE cost_events SET last_event_id = event_id WHERE last_event_id IS NULL;",
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V18_MONEY_CORRECTIONS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 18)
            .map_err(|e| e.to_string())?;
        version = 18;
    }

    if version < 19 {
        // Expense-only track CHECK. Rebuild money_corrections; copy every row.
        conn.execute_batch(SCHEMA_V19_MONEY_CORRECTIONS_NARROW_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 19)
            .map_err(|e| e.to_string())?;
        version = 19;
    }

    if version < 20 {
        // Per-variety standing targets. Idempotent ALTER: fixtures may rewind
        // user_version after a current-schema open.
        if !mkt_stages_has_column(conn, "variety_targets")? {
            conn.execute_batch("ALTER TABLE mkt_stages ADD COLUMN variety_targets TEXT NULL;")
                .map_err(|e| e.to_string())?;
        }
        conn.pragma_update(None, "user_version", 20)
            .map_err(|e| e.to_string())?;
        version = 20;
    }

    if version < 21 {
        // Phase 5: wholesale order book. Reinstall event_log triggers so the
        // four new register kinds are whitelisted, then create the tables.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V21_WHOLESALE_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 21)
            .map_err(|e| e.to_string())?;
        version = 21;
    }

    if version < 22 {
        // B6: a crop's former names. Reference data beside crops itself, and
        // covered by the same replay exclusion ("crops (and equivalents)").
        // Standing-target keys are recorded name strings inside frozen event
        // payloads; this is how a key still finds its crop after a rename.
        conn.execute_batch(SCHEMA_V22_CROP_ALIASES_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 22)
            .map_err(|e| e.to_string())?;
        version = 22;
    }

    if version < 23 {
        // Phase 6: the physical tray ceiling. Reference data beside crops --
        // a fact about the building, not a transaction. No event, no origin
        // guard, no flush; the update_crop_seed_rate / B6-crops precedent.
        // NULL slots mean unknown: the ceiling constrains nothing until the
        // operator supplies real numbers.
        conn.execute_batch(SCHEMA_V23_SHELF_CAPACITY_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 23)
            .map_err(|e| e.to_string())?;
        version = 23;
    }

    if version < 24 {
        // GT-D15. review.requested joins the marketing kind set, so the
        // event_log triggers must be reinstalled to whitelist it -- the
        // same reinstall v21 did for the wholesale kinds.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V24_REPUTATION_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 24)
            .map_err(|e| e.to_string())?;
        version = 24;
    }

    if version < 25 {
        // GT-D16. Scan config and the pull observation log. No event kind,
        // so the event_log triggers are untouched.
        conn.execute_batch(SCHEMA_V25_SCANS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 25)
            .map_err(|e| e.to_string())?;
        version = 25;
    }

    if version < 26 {
        // The silent shortfall. wholesale.write_off joins the register kind
        // set, so the event_log triggers must be reinstalled to whitelist it --
        // the same reinstall v21 and v24 did for their kinds.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V26_WRITE_OFFS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 26)
            .map_err(|e| e.to_string())?;
        version = 26;
    }

    if version < 27 {
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V27_UNAPPLIED_FACTS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 27)
            .map_err(|e| e.to_string())?;
        version = 27;
    }

    if version < 28 {
        // Customer QR fence 1 (brief 2026-08-17, ruling 2). Idempotent ALTER: fixtures
        // may rewind user_version after a current-schema open (the v20 pattern).
        if !mkt_samples_has_column(conn, "token")? {
            conn.execute_batch("ALTER TABLE mkt_samples ADD COLUMN token TEXT NULL;")
                .map_err(|e| e.to_string())?;
        }
        conn.execute_batch(SCHEMA_V28_SAMPLE_TOKEN_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 28)
            .map_err(|e| e.to_string())?;
        version = 28;
    }

    if version < 29 {
        // GT-D17. standing.requested joins the marketing kind set, so the
        // event_log triggers must be reinstalled to whitelist it -- the same
        // reinstall v21, v24, v26 and v27 did for their kinds.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V29_STANDING_REQUESTS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 29)
            .map_err(|e| e.to_string())?;
        version = 29;
    }

    if version < 30 {
        // GT-D17, second kind. standing.request_decided joins the marketing
        // kind set, so the event_log triggers must be reinstalled to whitelist
        // it -- the same reinstall v29 did for standing.requested. No table
        // change: decided_at / outcome already exist on mkt_standing_requests.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 30)
            .map_err(|e| e.to_string())?;
        version = 30;
    }

    if version < 31 {
        // Customer QR fence 5: pull log + refusal trace. No event kind, so
        // the event_log triggers are untouched.
        conn.execute_batch(SCHEMA_V31_STANDING_PULL_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 31)
            .map_err(|e| e.to_string())?;
        version = 31;
    }

    if version < 32 {
        // GT-D19. harvest.covered joins the marketing kind set, so the
        // event_log triggers must be reinstalled to whitelist it -- the same
        // reinstall v29 and v30 did for their kinds.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V32_HARVEST_COVERAGE_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 32)
            .map_err(|e| e.to_string())?;
        version = 32;
    }

    if version < 33 {
        // GT-D20 (rack-side fence 1). phone.proposed and phone.proposal_decided
        // join the grow kind set, so the event_log triggers must be reinstalled
        // to whitelist them -- the same reinstall v29, v30 and v32 did.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V33_PHONE_PROPOSALS_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 33)
            .map_err(|e| e.to_string())?;
        version = 33;
    }

    if version < 34 {
        // Rack-side fence 2 (GT-D21): device registry + pull log + refusal trace.
        // No event kind, so the event_log triggers are untouched.
        conn.execute_batch(SCHEMA_V34_FIELD_DEVICES_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 34)
            .map_err(|e| e.to_string())?;
        version = 34;
    }

    if version < 35 {
        // Bounce: wholesale.payment_reversed joins the register kind set, so
        // the event_log triggers must be reinstalled to whitelist it — the
        // same reinstall v29 / v30 / v32 / v33 did for their kinds. No table
        // change.
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 35)
            .map_err(|e| e.to_string())?;
        version = 35;
    }

    if version < 36 {
        // Bad debt (B): wholesale_orders gains the 'written_off' state.
        // SQLite cannot ALTER a CHECK, so the table is rebuilt — the same
        // rebuild the orders table took at v5/v13. wholesale.bad_debt also
        // joins the register kind set, so the event_log triggers are
        // reinstalled exactly as v35 did for wholesale.payment_reversed.
        conn.execute_batch(SCHEMA_V36_WHOLESALE_ORDERS_REBUILD_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V36_BAD_DEBT_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 36)
            .map_err(|e| e.to_string())?;
        version = 36;
    }

    if version < 37 {
        // C2 (INT-002): the refund gate's four refusal reasons join the
        // stripe_unapplied_facts.status CHECK. Table rebuild, rows kept.
        conn.execute_batch(SCHEMA_V37_UNAPPLIED_FACTS_WIDEN_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 37)
            .map_err(|e| e.to_string())?;
        version = 37;
    }

    if version < 38 {
        // TILL-A (GT-D22): payment-link columns on wholesale_orders; four
        // wholesale statuses join the stripe_unapplied_facts CHECK (rebuild,
        // rows kept); wholesale.link_minted joins the register kind set, so
        // the event_log triggers are reinstalled exactly as v35 / v36 did.
        conn.execute_batch(SCHEMA_V38_PAYMENT_LINK_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA_V38_UNAPPLIED_FACTS_WIDEN_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 38)
            .map_err(|e| e.to_string())?;
        version = 38;
    }

    if version < 39 {
        // LO-A (GT-D24): the leftover_listings table; leftover.listed joins the
        // register kind set, so the event_log triggers are reinstalled exactly
        // as v38 did.
        conn.execute_batch(SCHEMA_V39_LEFTOVER_LISTINGS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 39)
            .map_err(|e| e.to_string())?;
        version = 39;
    }

    if version < 40 {
        // LO-B (GT-D24-B): six nullable money columns on leftover_listings and
        // the core-column freeze; leftover.link_minted and leftover.paid join
        // the register kind set, so the event_log triggers are reinstalled
        // exactly as v39 did.
        // Idempotent ALTERs: open_in_memory fixtures rewind user_version after
        // a current-schema open, so these columns may already exist.
        if !leftover_listings_has_column(conn, "payment_link_id")? {
            conn.execute_batch(
                "ALTER TABLE leftover_listings ADD COLUMN payment_link_id TEXT NULL;",
            )
            .map_err(|e| e.to_string())?;
        }
        if !leftover_listings_has_column(conn, "payment_link_url")? {
            conn.execute_batch(
                "ALTER TABLE leftover_listings ADD COLUMN payment_link_url TEXT NULL;",
            )
            .map_err(|e| e.to_string())?;
        }
        if !leftover_listings_has_column(conn, "payment_link_minted_at")? {
            conn.execute_batch(
                "ALTER TABLE leftover_listings ADD COLUMN payment_link_minted_at TEXT NULL;",
            )
            .map_err(|e| e.to_string())?;
        }
        if !leftover_listings_has_column(conn, "priced_total_cents")? {
            conn.execute_batch(
                "ALTER TABLE leftover_listings ADD COLUMN priced_total_cents INTEGER NULL;",
            )
            .map_err(|e| e.to_string())?;
        }
        if !leftover_listings_has_column(conn, "paid_session_id")? {
            conn.execute_batch(
                "ALTER TABLE leftover_listings ADD COLUMN paid_session_id TEXT NULL;",
            )
            .map_err(|e| e.to_string())?;
        }
        if !leftover_listings_has_column(conn, "paid_at")? {
            conn.execute_batch("ALTER TABLE leftover_listings ADD COLUMN paid_at TEXT NULL;")
                .map_err(|e| e.to_string())?;
        }
        conn.execute_batch(SCHEMA_V40_LEFTOVER_LINK_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 40)
            .map_err(|e| e.to_string())?;
        version = 40;
    }
    if version < 41 {
        conn.execute_batch(SCHEMA_V41_FARM_CONFIG_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 41)
            .map_err(|e| e.to_string())?;
        version = 41;
    }
    if version < 42 {
        // R-10 (PACK-RF-PI): the refusal reason for a refund Stripe listed
        // with neither a payment intent nor a session id joins the
        // stripe_unapplied_facts.status CHECK. Table rebuild, rows kept. No
        // Kind changes, so the event_log triggers are not reinstalled (v37).
        conn.execute_batch(SCHEMA_V42_UNAPPLIED_FACTS_WIDEN_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 42)
            .map_err(|e| e.to_string())?;
        version = 42;
    }
    if version < 43 {
        // SEED-A (GT-D25): the seed_receipts table; seed.received joins the
        // register kind set, so the event_log triggers are reinstalled exactly
        // as v39 did — without the reinstall every seed.received write aborts
        // at the database (v13).
        conn.execute_batch(SCHEMA_V43_SEED_RECEIPTS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&schema_v9_event_log_triggers_sql())
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 43)
            .map_err(|e| e.to_string())?;
        version = 43;
    }
    if version < 44 {
        // J4 REF-DUP-FACT: a second Stripe session under a cart reference an
        // order already carries joins the stripe_unapplied_facts.status CHECK
        // as `duplicate_reference`. Table rebuild, rows kept. No Kind changes,
        // so the event_log triggers are not reinstalled (v42).
        conn.execute_batch(SCHEMA_V44_UNAPPLIED_FACTS_WIDEN_SQL)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "user_version", 44)
            .map_err(|e| e.to_string())?;
        version = 44;
    }
    if version > SCHEMA_VERSION {
        return Err(format!(
            "farm database version {version} is newer than this app ({SCHEMA_VERSION})"
        ));
    }

    // Idempotent: ensure seed rows exist even on relaunch after a partial seed.
    seed_crops(conn)?;
    Ok(())
}

/// Populate operator-supplied seed rates. Rejects zero/negative. Unmatched
/// slugs are left unset (no crop row created).
fn apply_operator_seed_rates(conn: &Connection) -> Result<(), String> {
    for (id, rate) in OPERATOR_SEED_RATES {
        match rate {
            Some(r) => {
                if !r.is_finite() || *r <= 0.0 {
                    return Err(format!(
                        "seed_rate_oz_per_tray for {id} must be > 0, got {r}"
                    ));
                }
                let n = conn
                    .execute(
                        "UPDATE crops SET seed_rate_oz_per_tray = ?1 WHERE id = ?2",
                        rusqlite::params![r, id],
                    )
                    .map_err(|e| e.to_string())?;
                if n == 0 {
                    eprintln!(
                        "seed rate: crop id '{id}' not found — rate left unset (no row created)"
                    );
                }
            }
            None => {
                // Explicit NULL — leave unset. Ensure row stays NULL if present.
                let _ = conn.execute(
                    "UPDATE crops SET seed_rate_oz_per_tray = NULL WHERE id = ?1",
                    rusqlite::params![id],
                );
            }
        }
    }
    Ok(())
}

pub fn seed_crops(conn: &Connection) -> Result<(), String> {
    let has_rate = crops_has_seed_rate_column(conn)?;
    if has_rate {
        let mut stmt = conn
            .prepare(
                "INSERT OR IGNORE INTO crops
                 (id, name, growth_days, blackout_days, expected_yield_oz, sort_order,
                  seed_rate_oz_per_tray)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(|e| e.to_string())?;

        for (id, name, growth, blackout, yield_oz, sort, rate) in SEED_CROPS {
            if let Some(r) = rate {
                if !r.is_finite() || *r <= 0.0 {
                    return Err(format!(
                        "seed_rate_oz_per_tray for {id} must be > 0, got {r}"
                    ));
                }
            }
            stmt.execute(rusqlite::params![
                id, name, growth, blackout, yield_oz, sort, rate
            ])
            .map_err(|e| e.to_string())?;
        }
    } else {
        let mut stmt = conn
            .prepare(
                "INSERT OR IGNORE INTO crops
                 (id, name, growth_days, blackout_days, expected_yield_oz, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| e.to_string())?;

        for (id, name, growth, blackout, yield_oz, sort, _rate) in SEED_CROPS {
            stmt.execute(rusqlite::params![
                id, name, growth, blackout, yield_oz, sort
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn crops_has_seed_rate_column(conn: &Connection) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(crops)")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?;
    for r in rows {
        if r.map_err(|e| e.to_string())? == "seed_rate_oz_per_tray" {
            return Ok(true);
        }
    }
    Ok(false)
}

fn assets_has_voided_at_column(conn: &Connection) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(assets)")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?;
    for r in rows {
        if r.map_err(|e| e.to_string())? == "voided_at" {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn local_date_today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Local calendar date from an RFC3339 UTC stamp — no second clock read.
pub fn local_date_from_utc_rfc3339(utc_rfc3339: &str) -> Result<String, String> {
    let dt = chrono::DateTime::parse_from_rfc3339(utc_rfc3339)
        .map_err(|e| format!("invalid utc timestamp: {e}"))?;
    Ok(dt
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string())
}

thread_local! {
    static FORBID_CLOCK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test helper: while `f` runs, `utc_now_rfc3339` panics (proves apply_* is clock-free).
#[cfg(test)]
pub fn with_clock_forbidden<T>(f: impl FnOnce() -> T) -> T {
    FORBID_CLOCK.with(|c| c.set(true));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    FORBID_CLOCK.with(|c| c.set(false));
    match result {
        Ok(v) => v,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

pub fn utc_now_rfc3339() -> String {
    FORBID_CLOCK.with(|c| {
        if c.get() {
            panic!("clock read forbidden (apply_* must be deterministic)");
        }
    });
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn get_crop_growth_blackout(conn: &Connection, crop_id: &str) -> Result<(i64, i64), String> {
    conn.query_row(
        "SELECT growth_days, blackout_days FROM crops WHERE id = ?1",
        [crop_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("unknown crop: {crop_id}"))
}

/// Historical v1 farm with the exact tray + event_log rows formerly produced by
/// `sow_tray(..., "dun-peas", 3)` against this freeze point (Phase 1 Ruling 2).
#[cfg(test)]
pub fn open_v1_in_memory() -> Result<Connection, String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    configure(&conn)?;
    conn.execute_batch(SCHEMA_V1_SQL)
        .map_err(|e| e.to_string())?;
    seed_crops(&conn)?;
    // Frozen dump from probe of sow_tray(dun-peas, 3) on v1 — byte-identical fields.
    conn.execute(
        "INSERT INTO trays (
            id, crop_id, state, quantity,
            growth_days_at_sow, blackout_days_at_sow,
            planned_on, sown_on, blackout_on, light_on, harvested_on, discarded_on,
            actual_yield_oz, created_at, updated_at
         ) VALUES (
            ?1, 'dun-peas', 'blackout', 3,
            9, 3,
            NULL, '2026-08-06', '2026-08-06', NULL, NULL, NULL,
            NULL, '2026-08-06T18:30:24.535Z', '2026-08-06T18:30:24.535Z'
         )",
        [FIXTURE_V1_TRAY_ID],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO event_log
         (seq, id, kind, entity_type, entity_id, payload, inverse, undone_at, undoes_seq, created_at)
         VALUES (
            1,
            '949c3e7f-09c3-46bb-aae0-56c07f440000',
            'tray.sown',
            'tray',
            ?1,
            '{\"blackoutOn\":\"2026-08-06\",\"cropId\":\"dun-peas\",\"quantity\":3,\"sownOn\":\"2026-08-06\"}',
            '{\"op\":\"delete_tray\",\"trayId\":\"b370c73f-9627-4684-aea2-beb59e662fb9\"}',
            NULL,
            NULL,
            '2026-08-06T18:30:24.535Z'
         )",
        [FIXTURE_V1_TRAY_ID],
    )
    .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 1)
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Historical v2 farm with the exact tray + event_log rows formerly produced by
/// `sow_tray(..., "kale", 2)` against this freeze point (Phase 1 Ruling 2).
#[cfg(test)]
pub fn open_v2_in_memory() -> Result<Connection, String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    configure(&conn)?;
    conn.execute_batch(SCHEMA_V1_SQL)
        .map_err(|e| e.to_string())?;
    seed_crops(&conn)?;
    conn.execute_batch(SCHEMA_V2_ATTENTION_SQL)
        .map_err(|e| e.to_string())?;
    // Frozen dump from probe of sow_tray(kale, 2) on v2 — byte-identical fields.
    conn.execute(
        "INSERT INTO trays (
            id, crop_id, state, quantity,
            growth_days_at_sow, blackout_days_at_sow,
            planned_on, sown_on, blackout_on, light_on, harvested_on, discarded_on,
            actual_yield_oz, created_at, updated_at
         ) VALUES (
            ?1, 'kale', 'blackout', 2,
            9, 4,
            NULL, '2026-08-06', '2026-08-06', NULL, NULL, NULL,
            NULL, '2026-08-06T18:30:24.536Z', '2026-08-06T18:30:24.536Z'
         )",
        [FIXTURE_V2_TRAY_ID],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO event_log
         (seq, id, kind, entity_type, entity_id, payload, inverse, undone_at, undoes_seq, created_at)
         VALUES (
            1,
            '0d4ad63a-70b3-473a-a493-c203edb181c5',
            'tray.sown',
            'tray',
            ?1,
            '{\"blackoutOn\":\"2026-08-06\",\"cropId\":\"kale\",\"quantity\":2,\"sownOn\":\"2026-08-06\"}',
            '{\"op\":\"delete_tray\",\"trayId\":\"e57c0a5d-2930-468f-875f-0df5b7257afc\"}',
            NULL,
            NULL,
            '2026-08-06T18:30:24.536Z'
         )",
        [FIXTURE_V2_TRAY_ID],
    )
    .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 2)
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Farm stopped at schema v4 (single-line orders, no harvest_links).
#[cfg(test)]
pub fn open_v4_in_memory() -> Result<Connection, String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    configure(&conn)?;
    conn.execute_batch(SCHEMA_V1_SQL)
        .map_err(|e| e.to_string())?;
    seed_crops(&conn)?;
    conn.execute_batch(SCHEMA_V2_ATTENTION_SQL)
        .map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA_V3_MONEY_SQL)
        .map_err(|e| e.to_string())?;
    conn.execute_batch(
        "ALTER TABLE stripe_cursor ADD COLUMN poll_fail_count INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE stripe_cursor ADD COLUMN last_app_open TEXT;",
    )
    .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 4)
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Farm stopped at schema v5 (composite session unique, harvest_links, no client_reference).
#[cfg(test)]
pub fn open_v5_in_memory() -> Result<Connection, String> {
    let conn = open_v4_in_memory()?;
    conn.execute_batch(
        r#"
        CREATE TABLE orders_new (
          id                    TEXT PRIMARY KEY,
          stripe_session_id     TEXT NOT NULL,
          stripe_payment_intent TEXT,
          harvest_date          TEXT NOT NULL,
          crop_id               TEXT NOT NULL REFERENCES crops(id),
          quantity              INTEGER NOT NULL CHECK (quantity >= 1),
          amount_cents          INTEGER NOT NULL,
          currency              TEXT NOT NULL,
          customer_email        TEXT,
          state                 TEXT NOT NULL CHECK (state IN ('paid','refunded','disputed')),
          capacity_consumed     INTEGER NOT NULL,
          paid_at               TEXT NOT NULL,
          created_at            TEXT NOT NULL,
          updated_at            TEXT NOT NULL,
          UNIQUE (stripe_session_id, crop_id)
        );
        INSERT INTO orders_new
          (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
           quantity, amount_cents, currency, customer_email, state,
           capacity_consumed, paid_at, created_at, updated_at)
        SELECT id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
               quantity, amount_cents, currency, customer_email, state,
               capacity_consumed, paid_at, created_at, updated_at
        FROM orders;
        DROP TABLE orders;
        ALTER TABLE orders_new RENAME TO orders;

        CREATE TABLE IF NOT EXISTS harvest_links (
          harvest_date    TEXT PRIMARY KEY,
          stripe_link_id  TEXT NOT NULL,
          stripe_link_url TEXT NOT NULL,
          line_signature  TEXT NOT NULL,
          created_at      TEXT NOT NULL
        );
        "#,
    )
    .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 5)
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Farm stopped at schema v6 (client_reference, no checkout_endpoint_url).
#[cfg(test)]
pub fn open_v6_in_memory() -> Result<Connection, String> {
    let conn = open_v5_in_memory()?;
    conn.execute_batch(
        "ALTER TABLE orders ADD COLUMN client_reference TEXT;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_orders_reference
           ON orders(client_reference, crop_id)
           WHERE client_reference IS NOT NULL;",
    )
    .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 6)
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Farm stopped at schema v8 (Phase 1 spine + Phase 1 triggers; no Phase 2 back-fill).
#[cfg(test)]
pub fn open_v8_in_memory() -> Result<Connection, String> {
    let conn = open_v6_in_memory()?;
    conn.execute_batch("ALTER TABLE stripe_config ADD COLUMN checkout_endpoint_url TEXT;")
        .map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA_V8_EVENT_LOG_SQL)
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 8)
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Frozen at schema v10: cost_events present, no seed_rate column, triggers
/// whitelist cost.money_out but not consumption.physical.
#[cfg(test)]
pub fn open_v10_in_memory() -> Result<Connection, String> {
    let conn = open_v8_in_memory()?;
    // v9: corrected triggers + spine backfill (empty log → no-op touches).
    conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
        .map_err(|e| e.to_string())?;
    let _ = preview_spine_backfill(&conn)?;
    let _ = apply_spine_backfill(&conn)?;
    conn.execute_batch(&crate::event_partition::schema_v10_event_log_triggers_sql())
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 9)
        .map_err(|e| e.to_string())?;
    // v10: cost_events + same v10-era trigger whitelist (no consumption.physical).
    conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
        .map_err(|e| e.to_string())?;
    conn.execute_batch(&crate::event_partition::schema_v10_event_log_triggers_sql())
        .map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA_V10_COST_EVENTS_SQL)
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 10)
        .map_err(|e| e.to_string())?;
    // Crops without seed_rate column (column added only at v11).
    seed_crops(&conn)?;
    Ok(conn)
}

/// Frozen at schema v11: consumption_events present without sow_event_id.
#[cfg(test)]
pub fn open_v11_in_memory() -> Result<Connection, String> {
    let conn = open_v10_in_memory()?;
    conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
        .map_err(|e| e.to_string())?;
    conn.execute_batch(&schema_v9_event_log_triggers_sql())
        .map_err(|e| e.to_string())?;
    conn.execute_batch("ALTER TABLE crops ADD COLUMN seed_rate_oz_per_tray REAL NULL;")
        .map_err(|e| e.to_string())?;
    apply_operator_seed_rates(&conn)?;
    conn.execute_batch(SCHEMA_V11_CONSUMPTION_EVENTS_SQL)
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 11)
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

#[cfg(test)]
pub fn drop_event_log_triggers(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(DROP_EVENT_LOG_TRIGGERS_SQL)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
pub fn install_v9_event_log_triggers(conn: &Connection) -> Result<(), String> {
    drop_event_log_triggers(conn)?;
    conn.execute_batch(&schema_v9_event_log_triggers_sql())
        .map_err(|e| e.to_string())
}

/// Written once when migration 9 runs; read by the spine report.
pub const MIGRATION_9_OUTCOME_FILE: &str = "migration-9-outcome.txt";

fn write_migration_9_outcome(
    conn: &Connection,
    preview: &SpineBackfillPreview,
    touched: usize,
) -> Result<(), String> {
    let file: String = conn
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if file.is_empty() {
        return Ok(());
    }
    let path = PathBuf::from(&file);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let out = parent.join(MIGRATION_9_OUTCOME_FILE);
    // rows corrected = mislabelled domain/class rows; rows back-filled = NULL origin fills.
    let corrected = preview.sale_rows_needing_register
        + preview.snapshot_rows_needing_register
        + preview.grow_rows_needing_domain;
    let body = format!(
        "migration 9 outcome\n\
         rows_corrected={corrected}\n\
         rows_back_filled_origin={}\n\
         total_update_touches={touched}\n",
        preview.null_origin
    );
    std::fs::write(&out, body).map_err(|e| e.to_string())
}
