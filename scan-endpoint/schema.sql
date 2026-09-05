-- GT-D16: ids must be monotonic and never reused, or the gap signal lies.
-- AUTOINCREMENT is load-bearing. Plain INTEGER PRIMARY KEY reuses ids after
-- a delete, which would make a shrinking log look like a clean one.
CREATE TABLE IF NOT EXISTS scans (
  scan_id    INTEGER PRIMARY KEY AUTOINCREMENT,
  scanned_at TEXT NOT NULL
);

-- GT-D18: standing-request candidates from the customer page. seq is the
-- Fence 5 pull cursor: monotonic, never reused, for the same reason as scans.
-- request_id is the stable id GT-D17 keys the desktop candidate on. Nothing
-- about the token is stored beyond the token itself.
CREATE TABLE IF NOT EXISTS standing_requests (
  seq            INTEGER PRIMARY KEY AUTOINCREMENT,
  request_id     TEXT NOT NULL UNIQUE,
  token          TEXT NOT NULL,
  bags_per_cycle INTEGER NOT NULL CHECK (bags_per_cycle >= 1),
  requested_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_standing_requests_token
  ON standing_requests (token);

-- GT-D21: phone-proposal candidates from the field terminal. seq is the pull cursor
-- (monotonic, never reused, as scans / standing_requests). proposal_id is phone-minted
-- and UNIQUE so a re-push after a dropped response is idempotent. Nothing about
-- devices beyond the token on the row; nothing about crops beyond the id.
CREATE TABLE IF NOT EXISTS field_proposals (
  seq               INTEGER PRIMARY KEY AUTOINCREMENT,
  proposal_id       TEXT NOT NULL UNIQUE,
  device_token      TEXT NOT NULL,
  verb              TEXT NOT NULL CHECK (verb IN ('move_to_light', 'harvest')),
  crop_id           TEXT NOT NULL,
  quantity          INTEGER NOT NULL CHECK (quantity >= 1 AND quantity <= 999),
  actual_yield_oz   REAL NULL,
  phone_captured_at TEXT NOT NULL,
  note              TEXT NULL,
  received_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_field_proposals_token ON field_proposals (device_token);
