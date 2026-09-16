# GT-D SERIES — DECISIONS AND THE FAILURES THEY PREVENT

Base: `alexjvv52-ops/prairie-roots-farm-os` @ `40e1d34`.
Source of truth: the Claude Groundtruth full-depth research document,
2026-08-09.

This file records WHY. The code records what. When a guard here looks
arbitrary later and there is pressure to relax it, the failure it prevents is
written below — read that before touching it. Several of these exist because
the failure had already happened once.

It lives in `docs/` rather than `decisions/` for one reason: `decisions/` is
gitignored, so a file written there would exist on one laptop and nowhere
else — not in git, not in an export bundle, not in a snapshot. A record of
why the guards exist has to survive the dead-laptop drill, or it is not a
record.

---

## GT-D1 — Marketing is a third event domain

**Decision.** Admit `marketing` alongside `grow` and `register`: its own
closed kind set, `event_class` NULL as grow rows carry, payloads sealed at
the same write choke point that seals consumption, projections registered in
replay.

**Forbids.** Any money key on a marketing payload — `amount`, `price`,
`cents`, `value` and the rest of the forbidden list are structurally
unrepresentable, not merely rejected. Marketing writes nothing to trays,
capacity, stock or any register table.

**Prevents.** A marketing table with an amount column. That is the dual-books
attack on this system: pipeline revenue, estimated deal value and
receivables accumulating outside the register until two sets of numbers
exist. A standing order is trays per week; money exists only where it
arrives.

**Proposed** 2026-08-09. **Executed** 2026-08-11 (`wave2-warm-path`).

---

## GT-D2 — Import lineage is judged on farm truth only

**Decision.** Two farms are never combined, but lineage is judged by
`identity::is_farm_truth` — not by domain, and not by counting every row.

**Forbids.** Treating marketing history, `snapshot.taken` or
`attention.resolved` as evidence that a database is already somebody's farm.

**Prevents.** The cutover being refused by the operator's own habits. During
the parallel period Groundtruth accumulates marketing events, and resolving a
marketing follow-up writes `attention.resolved`, which is grow-domain. A
lineage rule based on domain would count those, decide this was "a different
farm's records", and block the final import — discovered at the worst
possible moment. A marketing-only database has no farm lineage yet; it is the
same operator's farm waiting to be born.

**Proposed** 2026-08-09. **Executed** 2026-08-12 (`wave4-proof`).

---

## GT-D3 — The three Wave 0 freeze guards

**Decision.** Groundtruth refuses to run inside the Farm OS data folder;
databases are stamped with a Groundtruth `application_id` and foreign ones
are refused; `CUTOVER_LIVE` is a compile-time constant set to false, checked
at the write choke point.

**Forbids.** Any originating farm write, and opening any database belonging
to another app, before a deliberate tagged cutover.

**Prevents.** One 6 a.m. sow in the wrong app. That single act creates the
dual-books event the whole project names as its absolute stop. The lock makes
it impossible rather than discouraged, and flipping the constant is a code
edit, a rebuild and a tag — not a setting a tired thumb can toggle.

**Signed** by running the Wave 0 block, 2026-08-10 (`wave0-forge`).

---

## GT-D4 — v1 Health "capture paths" is the manual path plus silence honesty

**Decision.** With no automated scan capture, H2 verifies the manual path —
the marketing partition of `events.jsonl` flushes clean and a marketing write
is recorded — and additionally degrades when active venues exist and the
marketing log has been silent for seven days.

**Forbids.** Reporting Healthy on a check whose real subject does not yet
exist, and hiding that the second brain is starving.

**Prevents.** Quiet abandonment reading as health. The most likely way this
project dies is the logging habit lapsing while the page still looks fine.
H2's silence rule turns that into a visible amber sentence naming both
numbers instead of a slow lie.

**Proposed** 2026-08-09. **Realized** 2026-08-12 (`wave3-cadence`),
corrected by G3-A.

---

## GT-D5 — Automated scan capture. DEFERRED. NOT SIGNED.

**Status.** Deferred past v1. No code exists and none may be written against
it without a fresh decision.

**What it would be.** A Cloudflare Worker redirect behind the QR on future
label print runs, pulled read-only by the desktop, with monotonic ids so a
missed pull is detectable as a gap rather than as silence.

**The blind spot it leaves, stated rather than papered over.** Sample packs
already printed carry a QR pointing straight at the storefront order page.
Scans of those packs are uncounted and will remain uncounted. The conversion
still happens; only the counter is missing. The system says so rather than
pretending.

---

## GT-D6 — Thresholds

**Decision.** Follow-up default +3 days. Quiet venue 14 days. Warm decay 21
days. Storefront Degraded past 7 days, Unhealthy at 3 consecutive failures.
Full verify Degraded past 30 days.

**Forbids.** Nothing structurally — these are tunable numbers, recorded so
that changing one is a decision rather than a drift.

**Prevents.** Silent recalibration. A threshold quietly loosened to stop an
alarm is how a health system stops meaning anything.

**Proposed** 2026-08-09. **First applied** 2026-08-11 (`wave1-true-sight`).

---

## GT-D7 — `snapshot.taken` and `attention.resolved` are housekeeping

**Decision.** Those two kinds describe the app to itself and create no farm
truth. They pass the freeze; every other grow or register kind is refused
until cutover.

**Forbids.** Treating "domain is grow or register" as a synonym for "farm
truth". Those domains also carry the app's own plumbing.

**Prevents.** A Groundtruth that takes zero backups of itself and cannot
clear a single attention item. The freeze as originally written refused
`snapshot.taken`, so every launch and shutdown failed to snapshot and raised
an undismissable "A backup could not be saved" card — undismissable because
dismissing it writes `attention.resolved`, which the same lock refused. The
whole test suite stayed green throughout, because the lock was compiled out
of tests. This is why the lock now has a wiring test.

**Signed** 2026-08-11 (`wave0-forge-a`, G0-A).

---

## GT-D8 — Originating writes and imported writes are separate paths

**Decision.** `write_event` is the originating path and carries the freeze.
`write_imported_event` replays an operator-accepted bundle and does not.
`import::apply_import` carries its own gate, refusing a farm bundle whole
rather than per record.

**Forbids.** Weakening the ordinary import gate to make cutover possible.

**Prevents.** A cutover that cannot happen. `apply_import` writes through the
same choke point as a live sow, so the documented order — import, verify,
then flip — was mechanically impossible until the two paths were separated.
Found by reading the code, not by remembering it.

**Signed** 2026-08-11 (`wave0-forge-a`, G0-A).

---

## GT-D9 — The freeze binds every door into the database

**Decision.** Databases are stamped at creation inside `open_and_migrate`,
before migrations run. Any restore source must carry that stamp. While the
freeze is on, a backup containing farm records is refused even if stamped.

**Forbids.** Restoring a Farm OS database into Groundtruth.

**Prevents.** Dual books four taps away. Restore is a file replacement, not
an event write, so the write-path lock never saw it — and the "Moved
computers?" button opens a file picker filtered to `*.db`. A Farm OS
`farm.db` has `application_id` 0 and was accepted. Pointing it at
`%APPDATA%\com.prairieroots.farmos\farm.db` would have loaded every farm
record into Groundtruth while Farm OS kept running. The button most likely to
be pressed is the one an operator reaches for when a laptop dies.

**Signed** 2026-08-11 (`wave0-forge-b`, G0-B).

---

## GT-D10 — `farm_dir_verify` refuses a directory that is not Groundtruth's

**Decision.** The verify tool checks the Groundtruth stamp before it reads or
writes anything, and it reads the stamp straight from the SQLite header
bytes — no database handle, so a refused directory gets no report file and no
WAL sidecars.

**Forbids.** Running verify-replay against the Farm OS data directory.

**Prevents.** Groundtruth writing into the folder the README promises is
never written. `farm_dir_verify` ends by writing `last-verify-replay.txt`
into whatever directory it is handed, and it was handed the Farm OS one.
Nothing was lost — a status file and a `-shm` sidecar — but a guarantee that
has already been crossed once without anyone noticing is not a guarantee.
This door writes, unlike the other three.

**Signed** 2026-08-11 (`wave1-true-sight-a`, G1-A).

---

## GT-D11 — The closed kind set is amended by numbered decision

**Decision.** `stage.changed` and `reviews.observed` join the marketing
domain for Wave 3. Both are `(Marketing, None)`. No `EventClass` is added.

**Forbids.** Adding a marketing kind without an amendment. Wave 2
deliberately admitted only the kinds Wave 2 needed.

**Prevents.** Scope drift toward CRM. A closed set that grows quietly becomes
an open set. Every new record type costs a numbered decision, which is the
friction that keeps this from becoming an ERP.

**Signed** 2026-08-12 (`wave3-cadence`).

---

## GT-D12 — The capacity gate reads a Farm OS snapshot, read-only

**RETIRED 2026-08-13 by GT-D12-R (below). The original rationale stands as history.**

**Decision.** The gate opens the most recent Farm OS snapshot with
`immutable=1` and READ_ONLY, never calls `db::configure`, and calls the same
`trays::capacity_by_harvest_date` the farm itself uses. A missing, unreadable
or unqueryable snapshot returns Unknown with a stated reason and the weekly
list offers no pitching advice at all.

**Forbids.** Guessing capacity, and leaving any trace in the Farm OS folder.

**Prevents.** Two failures. First, a reputation shredder: generating demand
against capacity that is already committed, so the advice must be able to
invert to "Hold pitching". Second, a fingerprint — `immutable=1` suppresses
`-shm` and `-wal` creation, and a test asserts the directory's file names and
mtimes are identical before and after. This is the one place Groundtruth
looks at a Farm OS file. It is sight, not touch.

**Signed** 2026-08-12 (`wave3-cadence`).

---

## GT-D12-R — Capacity sight moves to the live database; the gate is retired

**Decision.** `capacity_sight` computes from this app's own live database via
the same `trays::capacity_by_harvest_date` query Sell online trusts, stamped
"live farm database, computed now" with the computation timestamp. The Farm
OS snapshot read is retired: nothing calls it, and the code is stripped in
P-2. Pitch/hold advice is returned only as its own field, never as a row in
the capped weekly list, so it cannot be truncated.

**Forbids.** Presenting any Farm OS snapshot as capacity after cutover, and
any capacity figure without origin and timestamp.

**Prevents.** Soft numbers by construction. GT-D12 was written for the
parallel period: Farm OS held farm truth, so honest sight meant reading its
newest snapshot without leaving a fingerprint. At cutover (`CUTOVER_LIVE =
true`, Phase 0 closed at 8734ee3) Farm OS stopped being written and its
snapshots froze; from that moment the gate's number could only age, and an
honest-looking age label was doing the work the architecture should. The
failure GT-D12 prevented — generating demand against capacity that is
already committed — had inverted into the failure it caused. Sight-not-touch
dies with the system it watched. The no-fingerprint guarantee survives
trivially: the retired path is never invoked.

**Signed** 2026-08-13 (`phase2-capacity-truth`, Phase 2 of the Money Engine
Roadmap).

---

## GT-D13 — Phase 4 Path B: the shop is a rehearsal until the wholesale door earns otherwise

**Decision.** Chosen 2026-08-13 by operator order. Live Stripe keys stay off:
the checkout Worker keeps `ALLOW_LIVE_KEYS = false` and no live restricted key
is created. The shop, offers, poll, and capacity-on-payment loop continue to
run in Stripe test mode for development and acceptance. Real revenue travels
through the wholesale door (Money Engine Roadmap, Phase 5).

**Forbids.** Flipping `ALLOW_LIVE_KEYS`, onboarding a live key, or rewiring
currency for live sale — except by a future signed decision that names and
supersedes this one.

**Prevents.** An unmade decision compounding as silence. The Maximum Power
Audit named the pre-decision state — a fully hardened channel that cannot
legally accept a customer — as an unmade decision, not a channel. This entry
makes the rehearsal deliberate: the strongest loop in the codebase remains a
test harness on purpose, and the revenue path is wholesale by choice, not by
accident.

**Signed** 2026-08-13 (`phase4-rehearsal`, Phase 4 of the Money Engine
Roadmap, Path B).

---

## GT-D13-R — The live key door opens: live restricted keys are accepted, test keys still work

**Decision.** Chosen 2026-09-05 by operator order. `ALLOW_LIVE_KEYS = true` at
both sites — `src-tauri/src/money.rs` in the app and
`checkout-endpoint/src/handler.js` in the checkout Worker. A live restricted
key (`rk_live_…`) is accepted by the same door that has always accepted a test
key. Test keys still work and are still the right key for a rehearsal farm.
Nothing else about the money path moves: the key is config on this machine and
never an event, it is still scrubbed out of every export bundle, secret keys
are still refused, and the mode a farm is connected in is still printed on
Money beside the account name. This supersedes GT-D13, which held the flip shut
and named its own release condition — a future signed decision that names it.

**Forbids.** A second live door. The flip is these two consts and nothing else:
no setting, no checkbox, no environment variable, no request parameter, no flag
file. Live payment on for one install and off for another is a code change,
reviewed and rebuilt, the same door for every install.

**Prevents.** A door that opens by configuration. GT-D13's real guarantee was
never "test mode forever" — it was that the mode a farm runs in cannot be
changed by anyone who can edit a settings file, a deploy variable, or a
request body. That guarantee is untouched here, and it is the reason this flip
is written into the ledger at all: the decision moved, the mechanism did not.

**Signed** 2026-09-05 (`gt-d13-live`, GT-D13 LIVE).

---

## GT-D13-USD — The mint bills US dollars: every Payment Link and retail Price is minted `usd`; `cad` is history

**Decision.** Chosen 2026-09-08 by operator order (USD-OR-CAD: CURRENCY A,
SCOPE A, COPY A rewritten, CAP C, SEAL CAD-OR-USD), signed on the E lab tree;
it reaches D only when E lands. The Stripe currency string at every mint site
is `usd`: the Price under a wholesale (`wo-`) or leftover (`lo-`) Payment Link
and the retail offer Price in `src-tauri/src/stripe_client.rs`, the
`wholesale.link_minted` and `leftover.link_minted` payloads, and the offers
cart page's checkout post in `src-tauri/src/shop.rs`. The seal on both
link_minted kinds accepts `usd` and `cad` — `cad` is history: every link minted
before this decision replays and imports unchanged, and the poll's link gates
take a paid session in either currency to the same cents check. The `$`
printers do not move; they already print `usd` as `$`. There is no farm
currency column and no currency picker: the app mints one currency, and the
Connect Stripe sheet on Money says which. This names and supersedes GT-D13's
currency clause — "rewiring currency for live sale — except by a future signed
decision that names and supersedes this one" — which GT-D13-R left standing.

**Forbids.** A per-farm currency, an exchange rate, a second mint currency, or
a currency that arrives by setting, checkbox, environment variable, request
parameter or flag file. `cad` is accepted at the seal and the gate for history
only — nothing mints it.

**Prevents.** A register whose tax lines are Schedule F and C carrying links
that bill Canadian dollars while the desk prints a plain `$` — two currencies
under one symbol. GT-D13's real guarantee holds: the currency a farm mints in
cannot be changed by anyone who can edit a settings file, a deploy variable,
or a request body.

**Signed** 2026-09-08 (`usd-or-cad`, GT-D13 USD).

---

## GT-D14 — The wholesale door: four order-book kinds, one money register, one capacity pool

**Decision.** Four register-domain kinds are admitted for the wholesale order
book: `wholesale.ordered`, `wholesale.delivered`, `wholesale.paid`,
`wholesale.voided`. They project into `wholesale_orders` +
`wholesale_order_lines` (one line per variety, unit = whole tray, optional
`price_cents_per_tray`). Channel lives on the order; marketing venue rows
stay money-free (GT-D1 untouched — the order references a venue, the venue
never carries a price).

States: ordered → delivered (unpaid) → paid, with voided reachable from
ordered or delivered only — never from paid. Post-paid mistakes are fixed
through the income correction/void doctrine. Delivered-but-unpaid is
first-class: `delivered_on` is stored and its age is computed at render,
never stored.

Payment is not a new money fact: `wholesale.paid` carries `income_event_id`
referencing an `income.received` record written in the same transaction —
cash in has one register, and the order book points at it.

Capacity: one pool. Non-voided wholesale trays subtract alongside the
retail path in `capacity_by_harvest_date` and the shop's per-crop sold
counts, so neither door can sell what the other has committed. Capacity
moves only through these kinds and the existing retail path. An order may
exceed what is currently sown (chefs order before sowing); that is never
refused and never silent — it raises attention naming the date and the
overcommitted tray count.

`record_order` may return Err for an unacknowledged call, never for the
order. Any caller proceeds by passing `overcommit_ack`; the acknowledgement
rides in the `wholesale.ordered` payload. That is the "never silent" half
of this ruling, moved from the UI into the write path.

Layers, never summed: standing demand (trays/week, per variety) is the
SOW-PLANNING layer; wholesale orders are the CAPACITY-ALLOCATION layer. The
shortfall engine reads standing targets only. Nothing double-counts.

Mixes: a standing or ordered mix is expressed as its whole-tray
per-variety recipe at entry — the per-variety targets ARE the
decomposition, and a mix order decomposes into per-variety lines. One
shortfall engine; no fractional trays as capacity facts (blending below
tray granularity is packing, not capacity).

**Forbids.** Money keys on marketing rows; a second cash-in register;
stored ages; general ledger, AR aging, dunning, statements; fractional
trays; any capacity write outside these kinds and the existing retail path.

**Prevents.** The audit's gap #7/#12 pair: a wholesale business run from
memory beside an app that only counted the toy channel, and a Money page
that understated the day. Most wholesale money sits delivered-but-unpaid;
this decision makes that state visible, aged, and exportable instead of a
sticky note.

**Residual, recorded.** Rack-side / phone capture is a public-v1
prerequisite (competitive assessment 2026-08-13, operator-locked). It is
NOT built in Phase 5; desktop-only entry is accepted for the private tree
and named here so the map shows the gap.

**Signed** 2026-08-13 (`phase5-gtd`, Phase 5 of the Money Engine Roadmap).

---

## GT-D15 — Reputation generates work: one ask per venue, permanently

**Decision.** `review.requested` joins the marketing domain for Phase 7.
It is `(Marketing, None)`. No `EventClass` is added. The payload is
`venue_id`, `decided_on`, `outcome` (`asked` | `skipped`). One row per
venue: `mkt_review_requests.venue_id` is the primary key, so a second
ask is an error, not an overwrite.

`business_profile.gbp_verified_on` is a date, not a boolean. NULL means
not verified. A date records when. The profile is reference data: no
event, no projection write, excluded from verify-replay the same way
`shelf_capacity` is. Review-ask work is offered only after that date
exists, and only for standing venues that have no request row.

**Forbids.** Adding a marketing kind without an amendment (GT-D11). A
second request for the same venue. Treating GBP verification as a
boolean. A capacity check or money key on this payload.

**Prevents.** Asking the same chef twice, and generating review work
against a profile that is not yet a public fact.

**Signed** 2026-08-14 (`phase7-reputation`, operator).

---

## GT-D16 — The scan endpoint counts hits and knows nothing else

**Decision.** (Recorded 2026-08-17 from the signed code text; the decision
shipped with Phase 8.) A Cloudflare Worker records one scan on `GET /s` and
redirects the person to `REDIRECT_URL`; `GET /scans` serves the count since a
cursor, read-only, pull-token gated. Ids are monotonic and never reused, so a
missed pull is a visible gap, never silence. The desktop pulls it (`scans.rs`)
into `scan_config` / `scan_observations` — no event kind, outside the replay
ledger.

**Forbids.** Any farm data at the endpoint. It knows nothing about venues,
orders or crops: a scan is an id and a timestamp. Scan-to-order attribution is
out of scope, and nothing is stored that would let it be added later without a
new decision. The count is a floor, not a total; packs printed before counting
began are dark forever.

**Prevents.** A second store of farm facts, and a "total" that quietly
undercounts.

**Signed** Phase 8 (operator); superseded narrowly by GT-D18.

---

## GT-D17 — The standing-request candidate: a chef's demand signal has a durable home

**Decision.** Two marketing-domain kinds are admitted for the customer QR →
standing conversion surface: `standing.requested` (this fence) and
`standing.request_decided` (Fence 3, under this same number). Both are
`(Marketing, None)`; no `EventClass` is added. They project into
`mkt_standing_requests`, keyed by `request_id` — the scan endpoint's stable
submission id, so a re-pull cannot mint a second candidate.

`standing.requested` carries `request_id`, `token` (the opaque per-drop token
of the 2026-08-17 fence-1 rulings), `venue_id` and `varieties` frozen at
ingest from the token's sample, `bags_per_cycle` (1 bag = 1 tray, ruling 7),
`requested_at` (the endpoint's clock; the event's own `created_at` is the
desktop's) and an optional `contact` — the single missing piece the customer
page may ask for. `standing.request_decided` carries the operator's `outcome`
(`accepted` | `dismissed`) and writes `decided_at` / `outcome` onto the same
row — the `followup.set` / `followup.cleared` pattern.

A candidate is a candidate. It never changes standing: acceptance writes
`stage.changed` through the existing `change_stage` path, and the candidate
row records only that the decision was made. The table is delete-proof.
Several candidates may carry one token; each chef tap is its own durable
signal.

**Forbids.** A money key on either payload (marketing tier, GT-D1). Adding a
kind for this surface without amending this decision (GT-D11). Holding a
candidate only as an attention row. Deleting a candidate. Any apply-time
lookup: both projections write only what the payload froze.

**Prevents.** Commercial demand that touched the system evaporating on a dead
laptop — attention rows do not travel in a bundle and are outside the replay
ledger. And a re-pull, a restore or a replay silently doubling or losing a
chef's request.

**Signed** 2026-08-17 (operator, customer QR fence 2). Fence 3 adds the
second kind under this number.

---

## GT-D18 — The scan endpoint hosts the customer page and holds standing candidates. Nothing else.

**Decision.** Supersedes GT-D16 narrowly. The same Worker serves the customer
page for a per-drop opaque token at `GET /s/<token>` (counting the hit like
any scan) and takes one write at `POST /s/<token>`: a standing-request
candidate stored as (endpoint-minted `request_id`, `token`, `bags_per_cycle`,
`requested_at`) in `standing_requests`, with a monotonic `seq` for the desktop
pull. `GET /s` keeps counting and redirecting for anything printed before
tokens. `FARM_NAME` (a Worker var) is the one line of identity on the page;
unset, the page refuses to serve.

The endpoint stores nothing about tokens. What the page shows — the sampled
varieties and the crop's cycle (`growth_days`; blackout is inside it) — rides
in the link the desktop composes (`?v=…&g=…`) and is never persisted. The
token is the only identifier that leaves the farm; the raw sample id never
does. The page collects a quantity and one tap: no account, no catalog, no
delivery day, no price, no invoice, no promise of availability. Tokens do not
expire.

**Forbids.** Any farm data at the endpoint beyond the candidate row. Any write
from the page into farm truth: a candidate never changes standing; the desktop
resolves each pulled candidate against its own samples and remains the final
authority (a well-formed forged token is refused there, with a trace). Any
external commercial URL or branding on the page.

**Prevents.** The conversion moment landing on someone else's storefront, and
a second store of farm facts growing beside the farm's own.

**Signed** 2026-08-17 (operator, customer QR fence 4).

---

## GT-D19 — A harvest may name what it covers. It never allocates.

**Decision.** One marketing-domain kind, `harvest.covered` `(Marketing, None)`,
records the operator's note that a day's harvest of a crop covers named
commitments — a standing venue or a wholesale order, per commitment line.
Payload: `coverage_id`, `crop_id`, `harvested_on`, `covers[] {kind, id}`.
Keyed by (crop, day), not by harvest event; append-only, projected into
`harvest_coverage`; the newest note per key is the truth, and "Undo mark" is
a new note with the line removed. At harvest the app shows, read-only, the
open commitments for that crop: standing venues with their own target for the
crop (or the unsplit hint) and wholesale orders still `ordered` with a harvest
date up to three days ahead (overdue flagged). Marks are shown only while a
non-undone harvest of the crop exists that day.

**Forbids.** Any capacity, standing-shortfall or money effect from a mark.
Reservation, subtraction, or automatic application of harvest to orders. A
money key on the payload (marketing tier, GT-D1). Adding a kind for this
surface without amending this decision (GT-D11). Any change to WeightPad
entry or confirm.

**Prevents.** The operator carrying the whole order book in memory at the
scale — and, equally, an "allocation" that would be a second, silent capacity
ledger.

**Signed** 2026-08-17 (operator, R3).

## GT-D20 — The phone proposal: a field capture has a durable home and one door into farm truth

**Decision.** Two grow-domain kinds are admitted for rack-side / phone capture:
`phone.proposed` and `phone.proposal_decided`, both `(Grow, None)`. Neither is
farm truth: `identity::is_farm_truth` excludes them (`PROPOSAL_KINDS`), so a
database holding only proposals has no farm lineage (GT-D2). They project into
`phone_proposals`, keyed by the phone-minted `proposal_id`, so a re-delivery can
never mint a second proposal; the table is delete-proof, its frozen fields are
immutable, and a decision is final.

`phone.proposed` freezes `proposal_id`, `device_id`, `verb` (`move_to_light` |
`harvest`), `crop_id`, `quantity`, `actual_yield_oz` (harvest only),
`phone_captured_at` (the phone's clock; the event's own `created_at` is the
desktop's) and an optional `note`. A proposal names a crop and a tray count,
never tray ids. `phone.proposal_decided` carries `outcome` (`accepted` |
`discarded`), `decided_at`, the values actually applied (`accepted_quantity`,
`accepted_yield_oz` — the operator may edit count and weight before Confirm),
`applied_event_ids` and, for a discarded row, `gate_reason`; it writes those onto
the same row. Provenance is the join: the proposal row and the decided event
carry the applied event ids; the written grow events are unchanged in shape and
origin.

A candidate is a candidate. It never changes trays. Confirm is the only door:
each accepted proposal is cross-referenced against live PC facts and, when
clean, applied through the existing write paths (`advance_trays_in_tx`,
`harvest_groups_in_tx`) on the phone capture day, in one transaction with its
decision; a blocked proposal stays pending and is shown its reason. The gate,
in order: a capture dated after today; harvest weight not > 0; fewer trays under
cover / in light than named; a harvest of that crop already recorded on the
capture day; whole batches oldest-first that cannot total the named count
exactly (`batch_mismatch`); a capture dated before the chosen trays were sown /
went into light. Reason codes are the closed set `phone::GATE_REASONS`,
mirrored by the `gate_reason` CHECK. Undecided proposals raise `phone.proposal`
attention on Reality, re-raised from the table on every check; the generic
dismiss / resolve doors refuse it.

**Forbids.** Any phone write into farm truth; a proposal that changes trays,
capacity, orders, money or standing; tray ids from the phone; splitting a batch;
a decision without a durable outcome; a proposal held only as an attention row;
a new `event_log.origin` value or a payload amendment of an existing kind for
provenance; adding a kind for this surface without amending this decision
(GT-D11).

**Prevents.** A second writer of farm truth arriving with the phone, and a phone
fact that either lands silently or evaporates: the capture is durable, the door
is one, the cross-check is against the PC, and the ledger says where each
written fact came from.

**Signed** 2026-08-17 (operator, rack-side / phone capture fence 1).

---

## GT-D21 — The scan endpoint hosts the field terminal and holds phone-proposal candidates. Nothing else.
**Decision.** Supersedes GT-D18 narrowly. The same Worker serves the field-terminal
page at `GET /a/<device_token>` and takes one write at `POST /a/<device_token>`: a
phone-proposal candidate stored as (phone-minted `proposal_id`, UNIQUE; the opaque
`device_token` from the path; `verb`; `crop_id`; `quantity`; `actual_yield_oz`;
`phone_captured_at`; `note`; `received_at`) in `field_proposals`, with a monotonic
`seq` for the desktop pull at `GET /field-proposals?after=<seq>` — SELECT-only under
the existing `PULL_TOKEN`. `GET /a/sw.js` serves the page's service worker so the
terminal opens at the rack with no signal. The page and both doors refuse until
`FARM_NAME` is set; the page records no scan.
The endpoint stores nothing about devices beyond the token on each row and nothing
about crops beyond the id: crop names ride in the pairing link the desktop composes
(`?c=<crop_id>:<name>&c=…`) and are never persisted. It cannot tell the live Admin's
token from a retired or forged one; the desktop is the authority. The desktop pairs
exactly one Admin phone at a time (`field_devices`: PC-minted 32-hex token shown once
inside the pairing link, SHA-256 hash stored, at most one live admin by partial unique
index, pairing a new phone retires the old in the same transaction, explicit Retire),
and at pull it resolves each row's token: a live Admin's rows enter through the ONE
ingest of GT-D20 (`phone::ingest_phone_proposal`); everything else leaves a
delete-proof refusal row (`unknown_device`, `retired_device`, `invalid`,
`unknown_crop`) and is never invented. The pull runs automatically while a live Admin
exists (app start, window focus, every 60 s) and on the operator's tap; a farm with no
paired phone makes no request. This fence is one-way: nothing farm-derived is
published to the endpoint; the phone sees only Queued / Sent.
**Forbids.** Any farm data at the endpoint beyond the candidate row; any write from
the endpoint into farm truth; device authentication at the relay; the token or its
hash in the event log; a second live Admin; a phone-side view of Health, capacity,
decisions or its own retirement (Fence 3, own decision); any external URL or branding
on the page.
**Prevents.** A relay quietly becoming a second writer or a second store of farm
facts, and a lost or replaced phone continuing to feed the farm.
**Signed** 2026-08-17 (operator, rack-side / phone capture fence 2).

---

GT-D22 and GT-D23 live in their signed briefs, not in this file.

## GT-D24 — A leftover listing is an operator-entered ounce quantity on Money, keyed by (crop_id, harvested_on), capped by harvested ounces for that crop-day, and capacity-free.
**Decision.** A leftover listing is an operator-entered ounce quantity on Money, keyed by (crop_id, harvested_on), capped by harvested ounces for that crop-day, and capacity-free.
**Forbids.** Selling leftover as a tray. Remounting SellOnlineSheet. Treating expected_yield_oz as x. Amending the WeightPad. Consuming or restoring tray capacity. A second listing for the same key. Stripe mint, poll, QR, or income.received in LO-A.
**Prevents.** Soft tray numbers after harvest. A shop-shaped leftover door. Silent surplus math from an estimate.
**Signed** 2026-08-27.

---

## GT-D25 — Seed in is an operator-entered ounce quantity for one crop, add-only, the IN side of the jar whose OUT side the sow path already writes.
**Decision.** `seed.received` joins the register tier (class `physical_consumption`, beside `consumption.physical`): payload `receipt_id`, `crop_id`, `received_oz` — the crop by id, never by name; ounces at 0.1; inverse none; one row per event in `seed_receipts`, add-only by trigger, compared by verify-replay. One door: Farm › Records › Record seed in — crop, ounces, Record. Schema 42 → 43 reinstalls the event_log triggers.
**Forbids.** A crop name in the payload or the row. A rate, a tray count, a dollar, or a jar total on this kind. Netting received against sown here. Changing the sow path’s `consumption.physical` oz-out row, coercing a blank sow weight to rate × trays, or undoing a sow through it — a blank sow stays unknown and an undone sow still leaves its oz-out standing. A ninth event class. Undo of a receipt — a wrong receipt is answered by a later record. A Health check or a Today sentence in this decision (later, through the cover plan).
**Prevents.** The jar living only in the operator’s head. A stock table that mutates. A seed figure guessed from trays. A second writer for the OUT side.
**Signed** 2026-09-01 (operator, SEED-A: IN A · IDENTITY A · UNIT A · BLANK A · UNDO A · HEALTH B · DOOR A · FIRST-FENCE A · SOAK A).

---

# AMENDMENTS

| ID | What it did | Tag | Date |
|---|---|---|---|
| G0-A | GT-D7 housekeeping allowlist; GT-D8 import path split | `wave0-forge-a` | 2026-08-11 |
| G0-B | GT-D9 stamp at creation; restore requires the stamp | `wave0-forge-b` | 2026-08-11 |
| G1-A | GT-D10 verify-replay refuses foreign farm directories | `wave1-true-sight-a` | 2026-08-11 |
| G3-A | H2 folded onto the single shared severity engine; Health page rendered from returned checks | `wave3-cadence-a` | 2026-08-12 |
| GT-D12-R | Capacity sight reads the live database; Farm OS gate retired; advice uncappable | `phase2-capacity-truth` | 2026-08-13 |

**G3-A, why it matters.** H2 had shipped on a parallel `severity_h2` that
took no clock. It therefore had no clock-skew detection, and its staleness
was computed by its caller instead of at render — so the check whose whole
purpose is detecting silence could itself have gone quiet about its own age.
Separately, the Health page was still printing a hardcoded "not yet
reporting" stub while the shell header, driven by the same computed statuses,
would have shown H2's real amber sentence. The page contradicted itself
inside one viewport, and the stub was the reassuring half. The page now
renders from the returned array, so a check cannot be added to the backend
and silently never reach the screen.

---

# POST-CUTOVER ITEMS (not GT-D numbered)

**Expense correct / void.** A wrong expense is fixed by a new append-only
event — never an edit or a delete. The original stays in the log forever; the
active list filters on `voided_at IS NULL`. The correction trail is a
projection with BEFORE UPDATE and BEFORE DELETE triggers that always abort,
so it is non-editable by construction. `money-truth`, 2026-08-12.

**Expense-only trail.** `money_corrections.track` is constrained to `'cost'`.
Income corrections are not written to the trail and the UI block is labelled
"Expense corrections", so the label claims exactly what it contains. This was
first decided as a label-only fix, then re-decided once the real cost —
SQLite cannot alter a CHECK, so the table must be rebuilt — was known. The
reversal is recorded deliberately: a decision whose price changes after
signing should be re-decided, not executed on momentum.
`money-truth-narrow`, 2026-08-12.

---

# OPERATING NOTES

These are not decisions. They are rules learned by getting them wrong.

**A version pin is not always a mirror.** Most hardcoded schema numbers in
tests mirror `SCHEMA_VERSION` and should reference the constant. At least one
is a tripwire whose purpose is to FAIL when the version moves, forcing a
human to re-check something — `phase2_schema_version_10_payload_keys_unchanged`
is one, and it was deleted once because a lint could not tell the difference.
Bump a tripwire; never delete one. `correction_tests.rs` c13 carries an allow
list for exactly this.

**State the operator-visible outcome first.** Two defects in this project had
the same shape: a backend capability landed and its visible half was
specified in a different task, or not at all. If a task cannot say what the
operator will see, it is not finished being written.

**Verification that cannot fail proves nothing.** A guard compiled out of the
test suite, a fixture that no longer reproduces the trap it was built for, a
drill doc satisfied by deleting three words — each of these was caught here,
and each looked green.

**`decisions/` is gitignored.** Everything in it, including
`RULING-attention-outside-replay-ledger.md` which `EXCLUSION_LIST` cites by
name, exists on one machine only. That is a separate exposure and a separate
decision; it is recorded here so it is not forgotten.
