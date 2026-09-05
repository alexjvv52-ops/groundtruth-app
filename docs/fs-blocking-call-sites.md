# Filesystem-blocking call sites

**Authority:** Residual "unbounded sync_all / VACUUM robustness", board option 2
signed 2026-08-21. Baseline tip `717c2ac`, schema v36.

**Status:** BACKLOG with a trip-condition. Not a Tier-1 residual, not closed as
no-risk. No production code was changed to produce this record.

**Placement:** this file lives in git on purpose, as a deliberate exception to
the internal-working-docs ignore policy at `.gitignore:61-69`. Ledgers,
inventories and decisions live outside this repo; this record does not, because
it names `file:line` in production code and has to diff against that code. If an
anchor below no longer matches the tree, that is the record going stale and it
must be re-checked, not trusted. Signed 2026-08-21 with board option 2.

This file names every production path that can block on the volume with no upper
bound. It is an inventory, not a defect list: F-C measured the five fs-bound
tests and found ordinary slow tests, not hangs, and no hang has been observed on
this farm.

## Trip-condition (signed 2026-08-21)

Revisit this item if the operator ever observes the app freezing during a write,
a launch, or a close, **or** if `farm.db` moves to a network, removable, or
encrypted volume. Those are the conditions under which the mechanism below stops
being theoretical on a local SSD.

## The seven sites

| # | Site | Op | Runs when | Automatic |
|---|---|---|---|---|
| 1 | `src-tauri/src/event_file.rs:401` (`flush_events`) | `sync_all` | after every committed write via `flush_ok` (`commands.rs:39-43`); app start (`lib.rs:152`); app shutdown (`lib.rs:188`) | yes — every write |
| 2 | `src-tauri/src/snapshots.rs:35` (`take_snapshot`) | `VACUUM INTO` | app start (`lib.rs:150`); app shutdown (`lib.rs:185`) | yes — every launch and close |
| 3 | `src-tauri/src/db.rs:1095` | `VACUUM INTO` | pre-migration safety copy inside `open_and_migrate` | yes, at launch after a schema bump |
| 4 | `src-tauri/src/costs.rs:296` | `sync_all` | receipt file write when recording a cost | operator-initiated |
| 5 | `src-tauri/src/event_file.rs:676` (`archive_and_rebuild`) | `sync_all` | restore, from `snapshots.rs:262` | operator-initiated |
| 6 | `src-tauri/src/export.rs:137` (`export_bundle`) | `VACUUM INTO` | export | operator-initiated |
| 7 | `src-tauri/src/projection/verify.rs:184` | `VACUUM INTO` | verify-replay / CLI | operator-initiated |

Every one is unbounded. `db::configure` (`db.rs:1050-1057`) sets only
`journal_mode = WAL` and `foreign_keys = ON`. A tree-wide search for
`busy_timeout`, `progress_handler` and `interrupt()` returns zero hits: no time
knob is set anywhere in this application today. That is a deliberate state, not
an oversight — see "What a timeout would and would not buy".

## The amplifier on site 1

`db.rs:5` — `pub struct Db(pub Mutex<Connection>);`. Every Tauri command opens
with `state.0.lock()`, and `flush_ok` (`commands.rs:39-43`) runs the flush while
that guard is still alive.

So a stall at site 1 does not hang one command: it holds the process's single
connection mutex, and every other command queues behind it. Site 1 is also the
only site that runs after every write. If one site is ever weighted above the
others, it is this one.

## Detection already exists, and it is immediate

`health.rs:393-400` — H3 reports **Unhealthy on FLUSH LAG > 0, immediately, with
no grace period**: "H3 Critical jobs — FLUSH LAG: {n} event(s) pending".
`health.rs:384` covers the forked case, where `events.jsonl` is ahead of the
database.

A flush that fails, or never completes, is therefore already loud on the Health
surface. The gap this item names is not "the software cannot tell"; it is "the
app blocks while it happens".

## What F-C proved

The five fs-bound tests each finish alone (`fc-test.log`): 8.24s, 31.42s,
32.05s, 33.26s, 47.27s. Ordinary slow tests, not hangs. Zero timeout code
landed, and that was the correct outcome.

III-0 Q11's mechanism stands: `sync_all` and `VACUUM INTO` block on a real OS
handle with no timeout, so a stuck volume, a locked file, or a hung fsync can
stall indefinitely.

Q11 also notes that the `"will retry"` text in `try_flush_after_commit` is a log
string and that the function does not retry. That is accurate about the call and
is **not** a defect: `event_file.rs:430` is an `eprintln!` to stderr, not an
operator surface — the operator surface is `last-flush-status.txt`, which
records `aborted: {e}` truthfully — and because a failed flush leaves the
watermark unchanged, the next `try_flush_after_commit` (next write, next launch)
re-attempts everything after it. The retry is real at the system level.

## What a timeout would and would not buy

- **`sync_all` cannot be bounded in-process.** Rust's `std::fs::File::sync_all`
  exposes no deadline, and the Windows call underneath (`FlushFileBuffers`) takes
  no timeout. There is no knob to set. Only structural change — keeping it off
  the critical path — would help, and that means touching the write choke point.
- **`VACUUM INTO` can be bounded against *slow*, not *stuck*.** rusqlite offers
  `progress_handler` and `interrupt()`, which abort a long-running statement. The
  handler fires on VM steps; while a write syscall is blocked the VM is not
  stepping, so it never runs.

Anyone proposing a timeout here should say which of the two they are buying.

## Growth risk, folded in rather than ranked separately

Site 2 runs a full `VACUUM INTO` of `farm.db` at every launch and every close.
That cost scales with the database. It is unrelated to stuck volumes and is fine
today, but it is the only one of the seven that gets worse on its own, purely
from the farm accumulating records. Named here so that a slow launch years from
now is a known item rather than a discovery. Board decision 2026-08-21: folded
into this note, not opened as a separate residual.
