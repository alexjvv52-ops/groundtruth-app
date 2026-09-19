# Integrity

This page is for offline diagnosis. Health names the failure. This
page says what a pass and a fail mean, and what you do next. The
running app is the fact if this page disagrees.

## VERIFY-REPLAY

**Run System Scan** on Health replays the event log against the
database.

A pass means the live database still matches its own history. The
check's sentence will say so. Keep working.

A fail means the log and the database disagree, or events are
pending flush, or a restore left them apart. The sentence on Health
is the fact.

What you do next: re-read Health and follow the check's own
recovery box. Never hand-edit the database.

If the box says events are pending flush, close the app normally,
reopen it, and run System Scan again. If the box names
last-verify-replay.txt, open the farm folder from Settings and read
that file. Do not record new events while the log and the database
disagree.

## The farm folder

Settings shows the path. Farm shows the same folder behind
**Farm saved automatically · last backup …**. Open it once and note
it.

Snapshots are NOT a backup — they live on the same disk they
protect. If that disk dies, they die with it. Copy the folder
yourself, on a schedule you decide. The app will not do this for
you.

## Export, restore, uninstall

**Export everything** (inside Backup & Export) writes a portable
bundle: the database, the event log, receipts, CSVs, and a checksum
manifest. Full detail lives in
[export-and-exit](../export-and-exit/README.md).

Restore refuses to merge two different farms. **Bring in a bundle**
previews first. A refusal means nothing was brought in. There is no
override.

Uninstall: leave the "delete application data" checkbox unchecked
if you might come back. That folder is the whole farm.

## Pins as of this tip

schema 46, Kind 54.

The running app is the fact if this page disagrees.
