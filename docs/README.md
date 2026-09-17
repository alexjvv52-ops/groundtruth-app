# Groundtruth documentation
Groundtruth is a free, local-first farm operating system for small growers.
It answers two questions and refuses to guess at either: **what should I do
right now, and are the numbers true?**
This folder is the public documentation pack. It explains the laws the
software obeys, how it was built without softening those laws, what was
built in what order, and how you leave with everything you own. The
application itself lives in the repository root (`src/`, `src-tauri/`); the
front door is the root `README.md`.
## Read in ten minutes
| If you are | Start with | Then |
|------------|------------|------|
| A grower who wants to run it | `00-start-here/` | `export-and-exit/` |
| A builder who wants to fork or send a change | `doctrine/` | `process/`, `architecture/` |
| A reader who wants to know how this was built | `journey/` | `process/` |
## Folders
- `00-start-here/` — your first morning with the app. Screens and clicks, no theory.
- `doctrine/` — the six hard laws and the list of things this project refuses to optimize for. These pages are the product; features are downstream.
- `process/` — how work is specified, implemented, and accepted so that speed cannot quietly break a law.
- `journey/` — what was built, in what order, why that order, and what is still open.
- `architecture/` — the shape of the system in one page: one writer, one event log, computed truth surfaces.
- `export-and-exit/` — the exit tax is zero. What export produces, what restore refuses, and why snapshots are not a backup.
## Working material
- `_raw/` — the unedited project record this pack was written from: signed decisions, audits, briefs, and working notes. It is kept as evidence, not as documentation. Nothing in it is required reading.
- `_SANITIZE-MANIFEST.md` — the classification of every file in `_raw/` (paths are relative to `_raw/`), with the tag each file received before this pack was written.
## Two rules for reading
1. If a page here disagrees with a signed decision in `_raw/`, the signed decision wins until it is re-signed. The pages interpret; they do not outrank.
2. If a page here disagrees with the running application, the application is the fact and the page is the defect. Report it.
License: Apache-2.0, as in the repository root.
