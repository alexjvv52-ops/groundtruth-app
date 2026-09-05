# BOOKS-BOUNDARY §6 done-when 2 — Dead laptop drill

This drill was permanently removed from scope by operator order on 2026-08-12. The procedure below is kept as written for whoever runs a restore by hand; it is not pending and is not scheduled.

## Before you start

These four things do not travel in a bundle and must be re-entered by hand:

- Your Stripe restricted key. Deliberate: a payment key does not auto-migrate between machines. Have it ready if you use payment links.
- Your farm name. Reference data on this machine (farm_config) — config, not an event, so it is not in the log. Still readable in the bundle's farm.db; re-enter it in Settings. It prints at the top of every invoice.
- Your seed rates. Rate edits are settings, not events, so they are not in the log. The old values are still readable in the bundle's farm.db if you need them.
- Anything in the attention list. It is operator collateral and is rebuilt from the state of the farm, not carried.

Marketing does travel: venues, samples, touches, follow-ups, stages and review observations are in the bundle and in the log.

## Procedure

1. On the working machine, open the backup sheet and Export everything.
2. Copy the whole export folder to a USB stick. Eject it.
3. Write down the start time to the minute. The clock starts now.
4. Simulate the dead laptop: close Groundtruth, then RENAME (do not delete) `%APPDATA%\com.prairieroots.groundtruth` to `...-drill-set-aside`.
5. Launch Groundtruth. It creates a fresh, empty farm.
6. Open the backup line, choose Bring in a bundle, pick `manifest.json` on the USB stick.
7. Read the preview. Confirm it reports the full event count, zero already present, and no refusals.
8. Bring it in.
9. Re-enter your Stripe key if you use payment links (**Connect Stripe** on Money).
10. Re-enter your farm name in Settings, then your seed rates.
11. Sow a tray.
12. Write down the stop time. The clock stops here.
13. Verification, all of these:
    - Today shows your trays as they were.
    - Open a cost with a receipt and confirm the receipt opens.
    - Open What a tray costs, work it out over the same window you used on the old machine, and confirm the figure and the method statement match.
    - Run: `cargo run --bin verify_replay -- "$env:APPDATA\com.prairieroots.groundtruth"` and confirm FLUSH LAG 0 and PASS or PASS WITH KNOWN.
    - The Marketing page opens and the TtFSO line reads what it read before.
    - Every venue, sample, touch, follow-up, stage and review observation is present with the same counts.
    - One follow-up can still be resolved in one tap.
    - Health: H1 through H4 all render with real timestamps after the restore.
14. Put the real farm back: close Groundtruth, delete the drill farm folder, rename `...-drill-set-aside` back to `com.prairieroots.groundtruth`.

## Results

This drill was permanently removed from scope by operator order on 2026-08-12.
The rows below record that decision; they are not run results.

| field | value |
|---|---|
| drill date | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| start time | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| stop time | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| elapsed minutes | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| receipts opened | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| cost per tray matched | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| verify_replay | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| marketing rows restored | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| TtFSO line matched | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| H1-H4 all reporting | REMOVED FROM SCOPE by operator order, 2026-08-12 |
| outcome | REMOVED FROM SCOPE by operator order, 2026-08-12 |
