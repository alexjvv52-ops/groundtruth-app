# Groundtruth — a farm record that cannot lie to you

One app that answers two questions: what should I do right now, and are my
numbers true.

Groundtruth runs the business, not only the beds — the farm file is the books.
the free offline farm ledger that opens each morning on the promised-vs-sown gap and can prove its history.

It runs on your Windows computer. There is no account, no subscription, and
no cloud. Nothing leaves your machine unless you export it and hand it over
yourself.

## Who this is for — and who it is not for

For: small microgreens growers who sow trays, harvest by weight, sell to
real customers, and want one honest record of the work and the money.

Not for: teams that need multiple users, phone-first workflows, cloud sync,
a general farm ERP, a CRM, or an accounting package. If you want dashboards
that flatter you, this is the wrong tool — this one refuses to guess.

Built for real production pressure — including places where short-cycle crops and local markets matter more than supermarket convenience.
Local-first and free by design: no mandatory cloud, no hostage data.

## What it will never do

- The PC is the sole writer of farm truth. The phone proposes; it does not invent the farm.
- Money cannot lie. Unpaid is unpaid. Unpriced stays unpriced. No dual books.
- Health can be ugly. If something is short, late, or broken, the system says so.
- The morning loop ranks work by path to cash — not by what looks busy.
- AI does not write farm truth. Eyes only.
- You can leave. Export everything. No hostage data.

## Why this exists

A notebook forgets. A spreadsheet lets any cell be overwritten with no
trace, and its totals never show their work. Paid farm software puts your
records on someone else's server and charges rent on your own numbers.

Groundtruth keeps the record on your machine, append-only. Every number
that comes from somewhere else carries its age and origin on screen. Every
computed figure shows the method and the exact rows it used — or refuses,
in plain words, when the data cannot support an answer.

## What you actually use, day to day

- **Today** — one screen that says what is due: move trays to light,
  harvest (by real weight), sow, log money out, log miles. One-step Undo.
- **Count the shelf** — when the app and the shelf disagree, the shelf
  wins, on the record.
- **Money** — money in, money out with receipt attachments and tax-line
  categories; corrections and voids leave a permanent, non-editable trail.
  Miles stay miles; equipment is four facts; your tax preparer decides the
  rest.
- **What a tray costs** — computed only when you ask, never stored, with
  the full method statement underneath. No trays or no payments in the
  window: it refuses rather than guesses.
- **Selling** — on Money: wholesale orders and leftover listings, each with
  a **Payment link** (Stripe restricted key, test mode) or cash **Paid…**.
  Online capacity is reserved only when payment actually confirms — never
  by a conversation or a cart.
- **Marketing** — venues, sample drops, follow-ups, and a stage ladder from
  scouted to standing order. Follow-ups sort overdue-first. No pipeline
  dollar values, ever — money exists only where it arrives.
- **Health** — computed when you look, never stored. Healthy is a quiet
  green dot. Problems are one sentence naming the exact failure and the
  next action, with raw timestamps you can check by eye.

## Install on Windows

Before you build, you need:

- **Git** — to clone this repo.
- **Windows** — there is no Mac or Linux build.
- **Node.js** — the LTS installer.
- **Rust** — installed with rustup.
- **Visual Studio Build Tools** — with the workload "Desktop development with C++".
- **WebView2** — Windows 11 usually already has it.

Then open a new Command Prompt or PowerShell so Node and Rust are on PATH.

Right now Groundtruth is built from source (Node + Rust: `npm install`,
then `npm run tauri build`). The bundle lands under Cargo's target dir —
`src-tauri/target/release/bundle/` by default, with `nsis/` and `msi/`
inside it; a CARGO_TARGET_DIR on the build machine moves it.
After the build, run the generated installer in `nsis/` or `msi/`.
A downloadable public release is the next milestone — until it exists, there is deliberately no download link here.

Windows will warn you about the installer. That is expected: the installer
is not code-signed, because signing costs money every year and this
software is free. Click "More info", then "Run anyway".

## Your first 15 minutes

1. The app opens on Today with "Start here" over five cards:
    - Add your first venue
    - Record a standing order or drop a sample
    - Sow trays to cover that demand
    - Record your first wholesale order
    - Configure scan endpoint (optional)

    Beneath them, one line: No tray or venue records on this machine. · Restore a backup · Start fresh on this machine.
2. Sowing is "Sow trays to cover that demand" on Today, or "Sow your first tray" on Farm.
   Eight microgreens crops ship ready to use, and you can add your own or
   change their growing times.
3. Money left for seed or supplies? "Money just left" is offered inside the
   sow sheet and the harvest weight pad — record it the day it happens.
4. The farm folder is behind "Farm saved automatically · last backup …" at
   the bottom of Farm, and under Settings › Backup & Export. Open it once
   and note the folder path. That folder is your entire farm.
5. To send a **Payment link** instead of taking cash, connect Stripe: on
   **Money**, under Wholesale, press **Connect Stripe**. The sheet names
   the exact restricted-key permissions to turn on, so there is nothing to
   look up here. Test mode only — a live key is refused
   (`docs/manual/refused.md`). The key is config on this machine, not an
   event: it stays in that same farm folder and is scrubbed out of any
   bundle you export, so you re-enter it after a restore.

## Your data

Everything lives in one folder on your machine:
`%APPDATA%\com.prairieroots.groundtruth` — the database, the append-only
log of everything that ever happened, your receipts, and automatic
snapshots taken every launch and every close.

Snapshots are not a backup. They live on the same disk they protect. Copy
the folder to a USB stick or your own cloud folder on a schedule you
decide. "Export everything" produces a portable bundle — database, full
event log, receipts, costs/mileage/assets CSVs with tax lines filled in,
and a checksum manifest — readable by any SQLite tool or spreadsheet.

If your laptop dies: install on a new machine, choose "Restore a backup",
and bring in your bundle. The app refuses files that are not its own
(another app's database is rejected by a byte-level stamp check), refuses
to merge two different farms, and after any restore it rebuilds its log to
match the database exactly, archiving the old log read-only — nothing is
ever deleted.

## How mature is this?

Early and honest about it. Version 0.1.x. Windows only. One machine, one
user. The installer is unsigned. The integrity core — append-only log,
computed health, stamp-checked restore, full export — is built and tested.
The full dead-laptop recovery drill is written but was never run end to end
and was removed from scope; so keep your old machine or a USB copy after
any move.

## Deeper docs

- `docs/OPERATOR-MANUAL.md` — the full manual, in plain language.
- `docs/GT-D-SERIES.md` — every structural decision and the failure it
  prevents. This project writes down its "why".

License: Apache-2.0.
