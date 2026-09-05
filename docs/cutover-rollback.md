# Cutover rollback

Written before the flip. Fill the archive lines when the final Farm OS bundle is exported.

## Archive (fill at cutover)

1. Final Farm OS bundle path:Phase 0 closed as-is by operator order 2026-08-12 / 2026-08-13.
  Old Farm OS data deliberately abandoned as test data.
  Full start-clean performed. Live farm is empty and clean.
  Archive lines left blank intentionally. No rollback to the abandoned test data is intended.
2. Manifest SHA-256: Phase 0 closed as-is by operator order 2026-08-12 / 2026-08-13.
  Old Farm OS data deliberately abandoned as test data.
  Full start-clean performed. Live farm is empty and clean.
  Archive lines left blank intentionally. No rollback to the abandoned test data is intended.

## Machine state at each point

1. Before cutover import: Farm OS holds every farm record and is still the writer. Groundtruth holds marketing history (and housekeeping). `CUTOVER_LIVE` is false. Groundtruth cannot write farm truth.
2. After cutover import, before verify_replay: Groundtruth holds a copy of farm truth plus its marketing history. Farm OS is untouched. `CUTOVER_LIVE` is still false. Groundtruth still cannot write farm truth.
3. After verify_replay PASS, before the flip: same as (2). Safe abandon point.
4. After the flip (`CUTOVER_LIVE` true, rebuild, tag `cutover`): Groundtruth is the only writer of farm truth. Farm OS must not be used to write farm events.



## Abandon before the flip

1. Do nothing.
2. Farm OS still holds every farm record.
3. Groundtruth simply has a copy and cannot write to it.
4. This is the safe state, and it is why the flip comes last.



## Recover after the flip

1. Restore Farm OS from its own final bundle (archive lines above).
2. Set `CUTOVER_LIVE` back to `false` in `src-tauri/src/identity.rs`.
3. Rebuild.
4. Tag the revert.
5. Any farm event recorded in Groundtruth after the flip must be re-entered by hand in Farm OS. That is the cost of a late rollback.

