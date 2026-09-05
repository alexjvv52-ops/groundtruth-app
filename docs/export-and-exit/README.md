# Export and exit
The exit tax is zero. This page says exactly what that means on disk.
## Where your farm lives
Everything is in one folder on your machine. The app shows you the path at
the bottom of Farm ("Farm saved automatically · last backup …") and under
Settings › Backup & Export. Open it once and note it. Inside:
- the database — one SQLite file;
- the append-only log of everything that ever happened;
- your receipts;
- automatic snapshots taken on every launch and every close.
## Snapshots are not a backup
They live on the same disk they protect. If that disk dies, they die with
it. Copy the folder to a USB stick or to your own cloud folder on a
schedule you decide. The app will not do this for you, and it will not
quietly sync your farm to anyone's server.
## Export everything
The app's "Export everything" action produces a portable bundle: the
database, the full event log, your receipts, CSVs for costs,
mileage, and assets with tax lines filled in, and a checksum manifest.
Every part of it is readable by any SQLite tool or spreadsheet. Nothing is
held back and nothing is in a format only this program can open.
Receivables and the income trail are in the CSVs. Write-offs and bad debts
are exported as themselves; they are never combined into a softer total.
## Restore
Install on a new machine, choose "Restore a backup", and bring in your
bundle. The app:
- refuses files that are not its own — every farm file carries a
  byte-level stamp written at creation, and a stranger's database is
  rejected by that check;
- refuses to merge two different farms;
- after any restore, rebuilds its log to match the database exactly and
  archives the old log read-only. Nothing is deleted.
## Prove it
A verify-replay pass checks that the database matches its own history. It
runs from Health, uses a scratch area under the farm's own snapshots, and
reports in plain words. The same footer appears on a printed invoice, so a
customer's bill carries the proof that the record it came from replays.
## Honest limits
- The full dead-laptop drill — export, wipe, install cold, restore, verify
  — is written but has not been run end to end, and was removed from
  scope for now. Keep the old machine or a USB copy after any move until
  that drill has a recorded result.
- Windows only, one machine, one user, in v0.1.
## Leaving for good
Export everything, copy the folder, uninstall. When uninstalling, leave
"delete application data" unchecked if you might come back; the farm
folder is the whole farm. Whatever you use next can read a SQLite file and
a CSV. That is the point.
