# Contributing

Non-negotiables and project purpose live in [README.md](README.md). Read those first.

## Read this first

1. `src/screens/Today.tsx` — the primary UI surface and the actions operators take
2. `src/farm/` — business logic the UI calls
3. `src-tauri/src/` — Rust backend, SQLite, event log
4. `decisions/README.md` — money-path boundary and operator sign-off policy

## Money-path boundary

PRs that touch the sales flow, consumption logic, or the database schema need operator sign-off before code is written. Details: [decisions/README.md](decisions/README.md).

## How the codebase is put together

A React frontend in a Tauri shell, a Rust backend over one SQLite file. Every state change is written through a single typed choke point into an append-only event log. The database is rebuildable from that log at any time.

## Tests

```bash
cd src-tauri
cargo test
```

The suite is expected to be fully green. Any failure is a real defect.

Frontend build:

```bash
npm run build
```

Checkout worker tests (no secrets needed):

```bash
cd checkout-endpoint
node --test
```

## Pull requests

Say what changed, how it was tested, and include a screenshot for UI changes.
