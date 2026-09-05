# 6. You can leave
**Law.** You can leave. Export everything. No hostage data.
**Why.** Records that only one program can read are a lease, not a
possession. A farm is a decade of trays, customers, and money; the
software that holds that record must be replaceable without losing it.
Groundtruth is free and local-first so that the only reason to stay is
that it is good.
**What it forbids.**
- Proprietary-only formats. The farm file is a single SQLite database plus
  an append-only event log; both are readable by ordinary tools.
- A mandatory cloud, account, or server between you and your own numbers.
- Lock-in by opacity — totals with no method, histories with no rows.
- An export that leaves something out. Receipts, costs, mileage, assets,
  and the full event history all leave with you.
**What it still allows.**
- **Export everything**: a portable bundle with the database, the complete
  event log, your receipts, CSVs for costs, mileage, and assets with tax
  lines filled in, and a checksum manifest — readable by any SQLite tool
  or spreadsheet.
- **Restore** onto a new machine from that bundle, with the app refusing
  anything that is not its own file and refusing to merge two farms.
- Snapshots on every launch and every close, and a farm folder you can
  copy to a USB stick or your own cloud folder on any schedule you choose.
- Forks that translate the interface, change the currency, or add regional
  crop defaults — under the same laws.
**How you can tell.** Open the farm folder the app shows you, or run
"Export everything". What comes out can be opened without this program.
Details in `../export-and-exit/`.
