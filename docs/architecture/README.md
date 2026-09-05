# Architecture
One page. Only what exists in the repository at tip `455b2e2`.
## Shape
A React frontend inside a Tauri shell, a Rust backend, one SQLite file.
The frontend lives in `src/`, the backend in `src-tauri/`. Two small
stateless workers live beside them: `scan-endpoint/` (customer QR scans;
carries no brand and holds no farm data) and `checkout-endpoint/`
(creates checkout sessions and answers the desk's payment-status poll).
Neither worker writes to the farm: the desk pulls from them, behind a
token, and a person accepts what arrives.
## One writer, one door, one log
Every state change goes through a single typed choke point into an
append-only event log. The database is a projection of that log and can
be rebuilt from it at any time. Corrections, voids, refunds, reversals,
and write-offs are new events; nothing is edited in place and nothing is
deleted. Row writers take typed input structs, so a caller cannot smuggle
an unsigned field into the record.
## Computed, not stored
Today, Health, cost per tray, Books, and the phone's snapshot are computed
from the record when they are drawn. Health is never cached as a verdict;
cost per tray is never stored; Books cannot enter money. Every value that
came from somewhere else — a payment provider, a scan, a phone pull — is
held as an observation with its age and origin, and the screen shows both.
## One capacity formula
Remaining capacity is one formula, keyed per date and crop. A harvest
retires a claim; a delivery date does not. Cover math counts trays ready on
or before the date; retail and online offers count exact-date only, so the
same tray cannot be sold twice. Every sentence that reads capacity was
inventoried and signed before it changed.
## Money
One register. Wholesale orders move through ordered, delivered, paid, and
voided; leftover listings are priced and closed by link or by cash; an
invoice is rendered from the row, never stored as a second truth, and
carries a verify-replay footer. Payment links are minted from an existing
row with a restricted key, test or live; the poll attaches a payment to that
row and nothing else; online capacity is consumed only after payment
confirms. A blank price cannot enter the record path, and neither
settlement door settles an unpriced line.
## Health
One severity engine, three words. Checks cover the shelf, the money, the
farm file, and the dock, and Health is rendered from the checks that
actually ran so that none can be stubbed. A failed read is Unhealthy.
## The phone
The desk serves a port document on the local network behind an admin
token. The document is a projection of the same snapshot pass that feeds
Today and Health, stamped with two PC ages. Captures from the phone land in
a proposal ledger and become events only when confirmed on the desk.
## The farm file
One folder: the database, the append-only log, receipts, and automatic
snapshots taken on every launch and every close. Every farm file carries a
byte-level stamp written at creation; restore refuses files without it,
refuses to merge two farms, and after any restore rebuilds the log to
match the database exactly, archiving the old log read-only. A
verify-replay pass can prove the database matches its history and uses a
scratch area under the farm's own snapshots, never a system temp folder.
Export is described in `../export-and-exit/`.
## Tests
The Rust suite runs with `cargo test` from `src-tauri/` and is expected to
be fully green; a small named set of slow, filesystem-bound tests runs on
its own cadence. The frontend builds with `npm run build`; the checkout
worker has its own tests with no secrets needed. A filter that matches
zero tests is treated as a failure.
