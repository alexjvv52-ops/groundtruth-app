# Suite split — fast lane vs slow lane (F-C class)

Written for III-c acceptance. Schema v34. No product change in this job.

Every exclusion is named with a reason. There is no anonymous skip list.
The unified suite returns when F-C lands timeouts.

---

## A1  Slow lane (minimum five)

Reason for each: **fs-bound / F-C class** (VACUUM / `sync_all` / cutover-import hashing). These do not gate a product land.

| Test | Reason |
|---|---|
| `shutdown_flush_io_failure_keeps_event_log_for_next_start` | fs-bound / F-C class |
| `shutdown_flush_clears_close_snapshot_lag` | fs-bound / F-C class |
| `w2_real_farm_truth_still_refuses_different_farm` | fs-bound / F-C class |
| `w4_cutover_import_refuses_each_gate_with_its_own_sentence` | fs-bound / F-C class |
| `w6_cutover_receipt_names_manifest_sha256` | fs-bound / F-C class |

---

## A2  Slow-test marker on disk — extend from evidence, do not pad

Command run:

```
Select-String -Path D:\groundtruth\fence3c-test.log,D:\groundtruth\fence3b-test.log,D:\groundtruth\fence3a2-test.log `
  -Pattern "has been running for over"
```

III-c's run left six tests unfinished and named only three (`w2`, `w4`, `w6`) in its kill footer. The two `shutdown_flush_*` members were already argv-skipped in that run.

### What the marker named

Rust's `has been running for over 60 seconds` line is not an F-C hang signal. It fires for any test still executing at the 60s tick under parallel load. The three logs name **123 distinct tests**. 120 of those later printed `... ok` in a finished suite (`fence3b-test.log`: 724 passed in 1555.15s; `fence3a2-test.log`: 717 passed in 1450.94s).

The two A1 `shutdown_flush_*` names never appear in this grep: they were filtered / argv-skipped, so the marker never had a chance to name them.

### Unfinished after the marker (no `... ok` / `FAILED` in that same log)

| Name | Log | Already in A1? |
|---|---|---|
| `wave4_tests::w2_real_farm_truth_still_refuses_different_farm` | fence3c-test.log | yes |
| `wave4_tests::w4_cutover_import_refuses_each_gate_with_its_own_sentence` | fence3c-test.log | yes |
| `wave4_tests::w6_cutover_receipt_names_manifest_sha256` | fence3c-test.log | yes |
| `import_tests::im10_preview_writes_nothing` | fence3c-test.log only | no |

`im10_preview_writes_nothing` is **not** added. It completed in both finished suites (`fence3b-test.log` and `fence3a2-test.log` both end with `test import_tests::im10_preview_writes_nothing ... ok`). In the killed III-c run it was one of the unnamed-at-footer in-flight leftovers, not a demonstrated F-C hang.

`correction_tests::c4_voided_expense_cannot_be_corrected_or_voided_again` printed the 60s line in `fence3b-test.log` and then `ok` on a following line (stdout interleaved with verify-replay text). Not unfinished.

### A2 decision

**Nothing beyond the five joins the slow lane.** The marker surfaced 118 names that are not in A1; they are not padded onto the skip list. Adding every 60s tick would turn a named F-C split into an anonymous skip of the verify / replay spine.

### Distinct names the marker printed (evidence, not the skip list)

Each line is `name | logs that named it`.

```
cadence_tests::g10_verify_and_export_import_stage_reviews | fence3a2-test.log, fence3b-test.log, fence3c-test.log
consumption_tests::h7_harvest_spine_round_trip_verify_replay | fence3a2-test.log, fence3b-test.log, fence3c-test.log
consumption_tests::t9_spine_round_trip_verify_replay | fence3a2-test.log, fence3b-test.log, fence3c-test.log
correction_tests::c11_track_check_rejects_income | fence3a2-test.log, fence3b-test.log, fence3c-test.log
correction_tests::c2_correct_leaves_original_event_and_updates_in_place | fence3a2-test.log, fence3b-test.log, fence3c-test.log
correction_tests::c3_void_sets_voided_at_and_leaves_figure_readable | fence3a2-test.log, fence3b-test.log, fence3c-test.log
correction_tests::c4_voided_expense_cannot_be_corrected_or_voided_again | fence3a2-test.log, fence3b-test.log, fence3c-test.log
correction_tests::c5_money_corrections_is_append_only | fence3a2-test.log, fence3b-test.log, fence3c-test.log
correction_tests::c6_trail_shows_both_sides_of_a_correction | fence3a2-test.log, fence3b-test.log, fence3c-test.log
correction_tests::c7_verify_replay_passes_with_corrections_compared | fence3a2-test.log, fence3b-test.log, fence3c-test.log
correction_tests::c8_export_import_round_trips_corrections | fence3a2-test.log, fence3b-test.log, fence3c-test.log
correction_tests::c9_non_goal_no_ledger_debit_credit_fields | fence3a2-test.log, fence3b-test.log, fence3c-test.log
cost_event_tests::cost_event_replays_byte_identically | fence3a2-test.log, fence3b-test.log, fence3c-test.log
cost_event_tests::phase2_failed_receipt_write_commits_nothing | fence3a2-test.log, fence3c-test.log
cost_event_tests::phase2_no_base64_in_persisted_event | fence3a2-test.log, fence3b-test.log, fence3c-test.log
cost_event_tests::phase2_receipt_file_ref_relative_with_sha256 | fence3a2-test.log, fence3b-test.log, fence3c-test.log
cost_event_tests::phase2_receipt_written_before_cost_events_commit | fence3a2-test.log, fence3b-test.log, fence3c-test.log
cost_event_tests::undo_last_never_selects_a_cost_and_reverses_the_real_last_action | fence3a2-test.log, fence3b-test.log, fence3c-test.log
cost_per_tray_tests::cp13_verify_replay_unaffected | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_receivables_tests::b8t1_delivered_unpaid_appears_with_amount_and_age | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_receivables_tests::b8t2_unpriced_lines_never_invent_a_total | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_receivables_tests::b8t3_income_void_is_a_reconstructable_row | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_receivables_tests::b8t4_income_correction_carries_before_and_after | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_receivables_tests::b8t5_cost_correction_export_is_untouched | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_receivables_tests::b8t6_manifest_names_both_new_files_and_its_own_counts | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex_manual_bundle_list_names_every_exported_file | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex1_one_action_produces_the_full_bundle | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex10_flush_lag_refuses_and_writes_nothing | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex11_manifest_notes_are_generated | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex2_manifest_is_the_contract_both_directions | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex3_receipts_are_bit_identical | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex4_export_completes_with_no_network_and_no_account | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex5_every_exported_cost_row_carries_both_tax_lines_and_a_descriptor | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex6_second_export_of_the_same_state_matches | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex7_export_mutates_nothing | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex8_csv_escaping_and_stability | fence3a2-test.log, fence3b-test.log, fence3c-test.log
export_tests::ex9_voided_rows_excluded_and_counted | fence3a2-test.log, fence3b-test.log, fence3c-test.log
fence1_tests::f1_01_a_tampered_order_row_fails_verify | fence3a2-test.log, fence3b-test.log, fence3c-test.log
fence1_tests::f1_01_wholesale_book_is_compared_not_excluded | fence3a2-test.log, fence3b-test.log, fence3c-test.log
fence1_tests::f1_07_replay_does_not_read_the_clock | fence3a2-test.log, fence3b-test.log, fence3c-test.log
fence2_tests::f2_10_an_imported_undo_reverses_the_event_it_named | fence3a2-test.log, fence3b-test.log, fence3c-test.log
fence2_tests::f2_10_an_undo_that_names_nothing_is_refused | fence3a2-test.log, fence3b-test.log, fence3c-test.log
health_tests::restore_rebuild_failure_keeps_session_on_restored_db | fence3a2-test.log, fence3b-test.log, fence3c-test.log
health_tests::restore_rebuild_matches_db | fence3a2-test.log, fence3b-test.log, fence3c-test.log
health_tests::restore_rebuild_no_prior_log | fence3a2-test.log, fence3b-test.log, fence3c-test.log
health_tests::restore_rebuild_rolls_back_on_guard_failure | fence3a2-test.log, fence3b-test.log, fence3c-test.log
identity_tests::t11_vacuum_into_preserves_the_stamp | fence3a2-test.log, fence3b-test.log, fence3c-test.log
identity_tests::t13_validate_farm_file_accepts_a_groundtruth_snapshot | fence3a2-test.log, fence3b-test.log, fence3c-test.log
identity_tests::t16_read_application_id_from_file | fence3a2-test.log, fence3b-test.log, fence3c-test.log
identity_tests::t17_farm_dir_verify_refuses_an_unstamped_farm_dir | fence3a2-test.log, fence3b-test.log, fence3c-test.log
identity_tests::t18_refusal_writes_nothing_into_the_directory | fence3a2-test.log, fence3b-test.log, fence3c-test.log
identity_tests::t19_farm_dir_verify_still_passes_on_a_groundtruth_dir | fence3a2-test.log, fence3b-test.log, fence3c-test.log
identity_tests::t9_open_and_migrate_stamps_a_fresh_database | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im1_second_import_performs_zero_writes | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im10_preview_writes_nothing | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im11_schema_version_mismatch_refuses_both_directions | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im12_refusal_different_farm_and_the_snapshot_only_exception | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im2_foreign_records_are_marked_and_excluded_from_totals | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im3_refusal_event_without_stable_id | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im4_refusal_farm_os_conflict_surfaces_and_never_overwrites | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im5_refusal_commercial_sale_or_payment_claiming_farm_os | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im6_log_versus_database_disagreement_halts | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im7_no_upsert_no_merge_anywhere | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im8_import_never_mutates_the_source_bundle | fence3a2-test.log, fence3b-test.log, fence3c-test.log
import_tests::im9_manifest_mismatch_refuses | fence3a2-test.log, fence3b-test.log, fence3c-test.log
income_tests::in9_income_csv_recorded_and_stripe_with_manifest_counts | fence3a2-test.log, fence3b-test.log, fence3c-test.log
marketing_tests::gt17b_candidate_projects_replays_and_is_delete_proof | fence3a2-test.log, fence3b-test.log, fence3c-test.log
marketing_tests::gt17l_decisions_replay_and_double_decision_fails_loudly | fence3a2-test.log, fence3b-test.log, fence3c-test.log
marketing_tests::m5_marketing_event_flushes_with_zero_lag | fence3a2-test.log, fence3b-test.log, fence3c-test.log
marketing_tests::m8_verify_replay_passes_with_marketing_tables | fence3a2-test.log, fence3b-test.log, fence3c-test.log
marketing_tests::m9_export_import_marketing_only_pre_cutover | fence3a2-test.log, fence3b-test.log, fence3c-test.log
mileage_asset_tests::s3_spine_round_trip_verify_replay_covers_new_tables | fence3a2-test.log, fence3b-test.log, fence3c-test.log
phone_pull_tests::f2g_pulled_capture_confirms_through_the_fence_1_gate_and_replay_passes | fence3a2-test.log, fence3b-test.log, fence3c-test.log
phone_tests::pf14_replay_rebuilds_phone_proposals_and_verify_passes | fence3a2-test.log, fence3b-test.log, fence3c-test.log
projection::verify::open_flags_tests::verify_source_handle_rejects_writes | fence3a2-test.log, fence3b-test.log, fence3c-test.log
r3_tests::r3c_mark_is_durable_hidden_without_harvest_undoable_and_replays | fence3a2-test.log, fence3b-test.log, fence3c-test.log
round_trip_tests::rt1_round_trip_event_fields_identical | fence3a2-test.log, fence3b-test.log, fence3c-test.log
round_trip_tests::rt2_round_trip_receipts_bit_identical | fence3a2-test.log, fence3b-test.log, fence3c-test.log
round_trip_tests::rt3_round_trip_derived_view_identical | fence3a2-test.log, fence3b-test.log, fence3c-test.log
round_trip_tests::rt4_round_trip_no_extra_events | fence3a2-test.log, fence3b-test.log, fence3c-test.log
round_trip_tests::rt5_round_trip_second_import_changes_nothing | fence3a2-test.log, fence3b-test.log, fence3c-test.log
standing_pull_tests::f5d8_pulled_candidates_replay_and_the_pull_log_is_excluded | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::advance_harvest_discard_date_stamps_round_trip | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::divergence_not_in_ledger_fails_verify_replay | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::farm_dir_verify_pass_with_known_divergences | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::farm_dir_verify_unledgered_divergence_fails | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::farm_dir_verify_writes_only_last_verify_replay_txt | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::handler_stamps_event_and_order_from_one_clock_read | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::healthy_fixture_work_counts_nonzero_and_pass | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::ledgered_divergence_reports_known_and_passes | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::malformed_events_jsonl_names_line_number | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::new_paid_session_client_reference_survives_replay | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::nonzero_flush_lag_fails_even_when_compared_rows_match | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::phase1_schema_triggers_user_version_evidence | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::restore_removes_stale_wal_from_previous_farm | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::restore_round_trip_removes_second_sow | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::restore_takes_pre_restore_snapshot_before_touching | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::restore_validation_rejects_bad_files_live_farm_untouched | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::rows_beyond_watermark_are_excluded_from_comparison | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::shutdown_flush_close_snapshot_reaches_events_jsonl | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::shutdown_flush_idempotent_no_duplicate_lines | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::shutdown_flush_two_sessions_no_accumulation | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::snapshot_writes_register_snapshot_event | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::spine_report_includes_verify_verdict_from_last_verify_file | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::tray_handler_event_created_at_equals_tray_updated_at | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::vacuum_into_includes_committed_wal_content | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::verify_replay_attention_resolved_is_noop | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::verify_replay_determinism_byte_identical_projections | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::verify_replay_fails_on_mutated_in_scope_field | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::verify_replay_fails_when_middle_jsonl_line_removed | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::verify_replay_grow_kinds_reproduce_in_scope | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::verify_replay_prints_exclusion_list | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::verify_replay_register_shapes_and_snapshot_noop | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::verify_replay_undo_reproduces_undone_at | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::zero_events_replayed_is_fail | fence3a2-test.log, fence3b-test.log, fence3c-test.log
tests::zero_flush_lag_with_matching_rows_is_pass | fence3a2-test.log, fence3b-test.log, fence3c-test.log
unapplied_fact_tests::ra5_verify_replay_applies_historic_and_enriched_refund_payloads | fence3a2-test.log, fence3b-test.log, fence3c-test.log
unapplied_fact_tests::sb5_verify_replay_rebuilds_the_row | fence3a2-test.log, fence3b-test.log, fence3c-test.log
wave4_tests::w1_marketing_only_plus_trap_imports_farm_without_different_farm | fence3a2-test.log, fence3b-test.log, fence3c-test.log
wave4_tests::w2_real_farm_truth_still_refuses_different_farm | fence3a2-test.log, fence3b-test.log, fence3c-test.log
wave4_tests::w4_cutover_import_refuses_each_gate_with_its_own_sentence | fence3a2-test.log, fence3b-test.log, fence3c-test.log
wave4_tests::w6_cutover_receipt_names_manifest_sha256 | fence3a2-test.log, fence3b-test.log, fence3c-test.log
wholesale_tests::phase5ws_export_carries_states | fence3a2-test.log, fence3b-test.log, fence3c-test.log
```

123 names. A2 adds none of them to the skip list beyond the five already in A1.

---

## A3  Commands (verbatim)

### Fast lane — acceptance suite for every product land

A2 added no name, so the skip list is the five from A1:

```
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-fail-fast -- \
  --skip shutdown_flush_io_failure_keeps_event_log_for_next_start \
  --skip shutdown_flush_clears_close_snapshot_lag \
  --skip w2_real_farm_truth_still_refuses_different_farm \
  --skip w4_cutover_import_refuses_each_gate_with_its_own_sentence \
  --skip w6_cutover_receipt_names_manifest_sha256
```

### Slow lane — own cadence, does not gate a product land, not run in this job

```
cargo test --manifest-path src-tauri/Cargo.toml --lib shutdown_flush
cargo test --manifest-path src-tauri/Cargo.toml --lib wave4_tests
```

The slow lane runs **broader than the five named** — it sweeps the whole fs-bound neighbourhood (`shutdown_flush*` and the whole `wave4_tests` module). That is acceptable for a lane that gates nothing.

---

## A4

- Every exclusion is named with a reason (`fs-bound / F-C class`).
- There is no anonymous skip list.
- The unified suite returns when F-C lands timeouts.

---

## A5  Margin measured (amended 2026-08-21, board signature)

The split was justified first as a hang fix (F-C disproved that), then as
wall-clock margin. Measured against every run on the desk, **the margin is not
visible in the data**. The board accepted the current margin as-is rather than
retiring the split or raising the box.

### What the five actually cost

Measured alone in `fc-test.log`:

| Test | alone |
|---|---|
| `w4_cutover_import_refuses_each_gate_with_its_own_sentence` | 47.27s |
| `shutdown_flush_clears_close_snapshot_lag` | 33.26s |
| `w6_cutover_receipt_names_manifest_sha256` | 32.05s |
| `w2_real_farm_truth_still_refuses_different_farm` | 31.42s |
| `shutdown_flush_io_failure_keeps_event_log_for_next_start` | 8.24s |
| **sum** | **152.24s** |

That 152s is **harness** time, not wall time. The 60-second marker names
≥120 tests each running ≥60s, so harness time is ≥7200s against ~1500s wall:
parallelism **P ≥ 4.8**. Removing 152s of harness at that parallelism should
return roughly **20–30s of wall clock**.

### Wall clock on the desk

| Run | lane | running | wall (s) | s/test |
|---|---|---|---|---|
| `fence3a2-test` | full (2 filtered) | 729 | 1450.94 | 1.990 |
| `fence3b-test` | full (2 filtered) | 736 | 1555.15 | 2.113 |
| `fence3c2-fastlane` | fast | 740 | 1482.67 | 2.004 |
| `fa-test` | fast | 745 | 1464.22 | 1.965 |
| `bounce-test` | fast | 753 | 1442.41 | 1.916 |
| `cleanup-fastlane` | fast | 749 | 1511.89 | 2.019 |
| `notes-fastlane` | fast | 749 | 1588.69 | 2.121 |
| `bad-debt-fastlane` | fast | 758 | 1551.71 | 2.047 |
| `bad-debt2-fastlane` | fast | 758 | 1409.37 | 1.859 |
| `slice2-fastlane` | fast | 766 | 1567.78 | 2.047 |

Fast lane, 8 runs: min 1409.37 · max 1588.69 · **spread 179.32** · mean
1502.34 · sd 59.52. The expected 20–30s saving is about one sixth of the noise
it would have to beat to be visible, and **both pre-split full-suite runs fall
inside the fast lane's own range**.

Seconds-per-test sits in **1.859–2.121 across every run, split or not**, with no
separation between lanes. Wall clock rose over the residual arc because the
suite grew 729 → 766 tests (+5.1%), not because the suite decayed.

### Counterweight, recorded rather than hidden

`w2`, `w4` and `w6` are named by the 60s marker in **both** pre-split runs
though they take 31–47s alone. Under parallel load fs contention makes them
cost more in-suite than alone, so the 20–30s figure is a lower-bound
derivation, not a measurement. Settling it would take a paired unified/fast
run on a quiet box; the board did not authorise one.

### Where the cost actually lives

`slice2-fastlane.log` names 121 tests over 60s across 25 modules — `tests`
(lib.rs inline) 34, `import_tests` 12, `export_tests` 12, `correction_tests` 9,
`identity_tests` 7, `export_receivables_tests` 6, `cost_event_tests` 6,
`round_trip_tests` 5, `marketing_tests` 5, then a long tail. The five in the
skip list are roughly **2%** of that slow mass. The verify/replay spine is the
suite. Real headroom, if it is ever wanted, is there — not in the skip list.

### Band provenance corrected

The full-suite band is **~1451–1555s** (`fence3a2-test.log` 1450.94s;
`fence3b-test.log` 1555.15s — both already quoted in A2 above). A lower bound of
1482 was `fence3c2-fastlane.log` at 1482.67s, which reports 5 filtered out: a
fast-lane run, not a full suite.

### A5 decision

1. The five in A1 stay exactly as they are. The skip list does not grow.
2. `shutdown_flush_close_snapshot_reaches_events_jsonl`,
   `shutdown_flush_idempotent_no_duplicate_lines` and
   `shutdown_flush_two_sessions_no_accumulation` each exceed 60s in the current
   fast lane and **deliberately stay in it**. The family is not the unit of
   exclusion; only the two the F-C job measured are named. Recorded here so the
   asymmetry is a ruling, not a silence.
3. The split is kept because the five are **fs-bound and named**, not because it
   demonstrably buys margin. A2's refusal to pad the list stands and is
   reinforced.
4. Rule 4 is unchanged: the unified suite returns when the board raises the box
   or lands harness work.
