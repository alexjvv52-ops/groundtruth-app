mod assets;
mod attention;
mod capacity_gate;
mod categories;
mod commands;
mod consumption;
mod cost_per_tray;
mod costs;
mod db;
mod divergence;
mod dock_folds;
mod dock_port;
mod dock_shell;
mod event_file;
mod event_partition;
mod events;
mod export;
mod export_scrub;
mod field_devices;
mod health;
mod identity;
mod import;
mod income;
mod invoice;
mod key_at_rest;
mod leftover;
mod marketing;
mod mileage;
mod models;
mod money;
mod observed;
mod offers;
mod phone;
mod phone_pull;
mod poll;
mod projection;
mod reachability;
mod scans;
mod seed;
mod seed_prefill;
mod shop;
mod snapshots;
mod standing_pull;
mod storefront;
mod stripe_client;
mod trays;
mod wholesale;

#[cfg(test)]
mod cadence_tests;
#[cfg(test)]
mod choke_point_tests;
#[cfg(test)]
mod consumption_tests;
#[cfg(test)]
mod correction_tests;
#[cfg(test)]
mod cost_event_tests;
#[cfg(test)]
mod cost_per_tray_tests;
#[cfg(test)]
mod cover_jar_tests;
#[cfg(test)]
mod crop_library_tests;
#[cfg(test)]
mod dock_folds_tests;
#[cfg(test)]
mod dock_lab_tests;
#[cfg(test)]
mod dock_port_tests;
#[cfg(test)]
mod export_receivables_tests;
#[cfg(test)]
mod export_tests;
#[cfg(test)]
mod face_tests;
#[cfg(test)]
mod fence1_tests;
#[cfg(test)]
mod fence2_tests;
#[cfg(test)]
mod fence3a_tests;
#[cfg(test)]
mod fence3b_tests;
#[cfg(test)]
mod fence3c_tests;
#[cfg(test)]
mod fence4a_tests;
#[cfg(test)]
mod fence4b_tests;
#[cfg(test)]
mod fence_bad_debt_tests;
#[cfg(test)]
mod fence_bounce_tests;
#[cfg(test)]
mod fence_fa_tests;
#[cfg(test)]
mod health_tests;
#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod import_tests;
#[cfg(test)]
mod income_tests;
#[cfg(test)]
mod inv_a_tests;
#[cfg(test)]
mod j4_tests;
#[cfg(test)]
mod key_at_rest_tests;
#[cfg(test)]
mod key_door_tests;
#[cfg(test)]
mod link_retire_tests;
#[cfg(test)]
mod list_state_tests;
#[cfg(test)]
mod lo_a_tests;
#[cfg(test)]
mod lo_b_cash_tests;
#[cfg(test)]
mod lo_b_tests;
#[cfg(test)]
mod lo_c_tests;
#[cfg(test)]
mod lo_speak_tests;
#[cfg(test)]
mod marketing_tests;
#[cfg(test)]
mod mileage_asset_tests;
#[cfg(test)]
mod owed_lo_tests;
#[cfg(test)]
mod phone_pull_tests;
#[cfg(test)]
mod phone_tests;
#[cfg(test)]
mod price_gate_tests;
#[cfg(test)]
mod r3_tests;
#[cfg(test)]
mod reputation_tests;
#[cfg(test)]
mod rf_pi_tests;
#[cfg(test)]
mod round_trip_tests;
#[cfg(test)]
mod scan_tests;
#[cfg(test)]
mod seed_a_tests;
#[cfg(test)]
mod seed_b_tests;
#[cfg(test)]
mod shelf_ceiling_tests;
#[cfg(test)]
mod shop_door_tests;
#[cfg(test)]
mod standing_pull_tests;
#[cfg(test)]
mod storefront_tests;
#[cfg(test)]
mod strings_tests;
#[cfg(test)]
mod till_a_tests;
#[cfg(test)]
mod till_b_tests;
#[cfg(test)]
mod unapplied_fact_tests;
#[cfg(test)]
mod undo_refusal_tests;
#[cfg(test)]
mod usd_mint_tests;
#[cfg(test)]
mod wave4_tests;
#[cfg(test)]
mod wholesale_tests;

/// Verify-replay CLI entry (binary links this crate).
pub use projection::{farm_dir_verify, VerifyOutcome};

/// Named CLI argument errors for `verify_replay` (stale flags must name themselves).
pub fn verify_replay_cli_error(extra: Option<&str>) -> String {
    match extra {
        Some(arg) => format!("unexpected argument: {arg}\nUsage: verify_replay <farm-directory>"),
        None => "Usage: verify_replay <farm-directory>".to_string(),
    }
}

use db::{Db, FarmPaths};
use std::sync::{Arc, Mutex};
use tauri::{Manager, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            identity::assert_data_dir_allowed(&data_dir)?;
            let farm_path = data_dir.join("farm.db");
            let snapshots_dir = data_dir.join("snapshots");
            std::fs::create_dir_all(&snapshots_dir).map_err(|e| e.to_string())?;

            let mut conn = db::open_and_migrate(&farm_path)?;
            identity::stamp_or_refuse(&conn)?;
            // On launch, after migration, before any command can run.
            snapshots::try_take_snapshot(&mut conn, &snapshots_dir);
            // Snapshot may have appended event_log; catch up the file + report.
            event_file::on_app_start(&conn, &data_dir);

            // H4 quick_check + H2/H3 evidence on open. Errors become evidence —
            // app start must not fail because a check could not run.
            //
            // The storefront GET that used to run here is gone with ruling 6.1:
            // the observer block was removed from the Farm surface, H1 is no
            // longer in health::REPORTED_CHECKS, and the fetch reached a URL
            // that is none of Groundtruth's business. storefront.rs and its
            // tests stay on disk, uncalled at start.
            let now = db::utc_now_rfc3339();
            let today = chrono::Local::now().date_naive();
            let _ = health::record_quick_check(&conn, &now);
            let _ = health::record_h2(&conn, &data_dir, &now, today);
            let _ = health::record_h3(&conn, &data_dir, &snapshots_dir, &now, today);
            let _ = marketing::raise_overdue_followups(&conn);

            app.manage(Db(Arc::new(Mutex::new(conn))));
            app.manage(FarmPaths {
                farm_db_path: farm_path,
                folder_path: data_dir,
                snapshots_dir,
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // CloseRequested covers clean shutdown; avoid Destroyed too or we snapshot twice.
            if let WindowEvent::CloseRequested { .. } = event {
                let app = window.app_handle();
                if let (Some(db), Some(paths)) =
                    (app.try_state::<Db>(), app.try_state::<FarmPaths>())
                {
                    if let Ok(mut conn) = db.0.lock() {
                        snapshots::try_take_snapshot(&mut conn, &paths.snapshots_dir);
                        // Mirror of setup(): the snapshot just appended event_log. Catch the file
                        // up before the process exits, or this session's last event is never logged.
                        event_file::on_app_shutdown(&conn, &paths.folder_path);
                    }
                }
            }
        });

    #[cfg(debug_assertions)]
    {
        builder
            .invoke_handler(tauri::generate_handler![
                commands::list_crops,
                commands::update_crop_seed_rate,
                commands::add_crop,
                commands::rename_crop,
                commands::update_crop_growth_days,
                commands::list_trays,
                commands::today_view,
                commands::sow_tray,
                commands::advance_tray,
                commands::advance_trays,
                commands::harvest_tray,
                commands::harvest_trays,
                commands::harvest_groups,
                commands::early_harvest_groups,
                commands::discard_tray,
                commands::discard_from_group,
                commands::undo_last,
                commands::capacity_by_harvest_date,
                commands::shelf_capacity,
                commands::set_shelf_capacity,
                commands::shelf_pressure,
                commands::overcommit_warning,
                commands::pay_amount_warning,
                commands::duplicate_income_warning,
                commands::money_status,
                commands::list_orders,
                commands::preview_stripe_key,
                commands::confirm_stripe_key,
                commands::set_checkout_endpoint_url,
                commands::list_offers,
                commands::set_offer,
                commands::remove_offer,
                commands::generate_shop_page,
                commands::open_shop_page_folder,
                commands::write_leftover_shop_page,
                commands::poll_stripe,
                commands::take_new_paid_orders,
                commands::reconciliation,
                commands::dev_backdate_tray,
                commands::dev_seed_standing_request,
                commands::dev_seed_phone_proposal,
                commands::list_snapshots,
                commands::take_snapshot,
                commands::restore_snapshot,
                commands::farm_location,
                commands::open_farm_folder,
                commands::export_bundle,
                commands::open_export_folder,
                commands::preview_import,
                commands::apply_import,
                commands::apply_cutover_import,
                commands::recount_state,
                commands::apply_recount,
                commands::check_attention,
                commands::resolve_attention,
                commands::dismiss_attention,
                commands::list_cost_categories,
                commands::receipt_source_info,
                commands::record_cost,
                commands::list_expenses,
                commands::list_money_corrections,
                commands::list_unapplied_facts,
                commands::correct_expense,
                commands::void_expense,
                commands::cost_per_tray,
                commands::list_mileage_trips,
                commands::record_mileage_trip,
                commands::correct_mileage_trip,
                commands::void_mileage_trip,
                commands::list_assets,
                commands::record_asset,
                commands::correct_asset,
                commands::void_asset,
                commands::list_income_categories,
                commands::list_income,
                commands::record_income,
                commands::correct_income,
                commands::void_income,
                commands::storefront_reading,
                commands::refresh_storefront,
                commands::health_status,
                commands::dock_folds,
                commands::dock_port_start,
                commands::dock_port_stop,
                commands::dock_port_status,
                commands::run_full_verify,
                commands::marketing_summary,
                commands::list_venues,
                commands::record_venue,
                commands::drop_sample,
                commands::correct_venue,
                commands::sample_gate_line,
                commands::decide_standing_request,
                commands::qr_link_for_token,
                commands::standing_pull_view,
                commands::pull_standing_requests,
                commands::harvest_commitments,
                commands::record_harvest_coverage,
                commands::log_touch,
                commands::list_open_followups,
                commands::list_samples,
                commands::resolve_followup,
                commands::change_stage,
                commands::observe_reviews,
                commands::list_stages,
                commands::weekly_actions,
                commands::reputation_counts,
                commands::capacity_sight,
                commands::latest_review,
                commands::record_review_request,
                commands::list_review_requests,
                commands::gbp_verified_on,
                commands::set_gbp_verified,
                commands::scan_view,
                commands::scan_config,
                commands::set_scan_config,
                commands::pull_scans,
                commands::standing_demand,
                commands::record_wholesale_order,
                commands::deliver_wholesale_order,
                commands::pay_wholesale_order,
                commands::settle_order_with_income,
                commands::write_off_summary,
                commands::cash_collected,
                commands::cash_rows,
                commands::write_off_rows,
                commands::unpriced_exposure,
                commands::unpriced_orders,
                commands::cash_out,
                commands::expense_rows,
                commands::net_cash,
                commands::bad_debt_summary,
                commands::bad_debt_rows,
                commands::cash_by_category,
                commands::income_corrections_count,
                commands::income_correction_rows,
                commands::void_wholesale_order,
                commands::reverse_wholesale_payment,
                commands::bad_debt_confirm_line,
                commands::bad_debt_trail_line,
                commands::write_off_bad_debt,
                commands::mint_wholesale_payment_link,
                commands::wholesale_payment_link_qr,
                commands::leftover_payment_link_qr,
                commands::record_leftover_listing,
                commands::list_leftover_listings,
                commands::mint_leftover_payment_link,
                commands::pay_leftover_listing_cash,
                commands::record_seed_received,
                commands::seed_on_hand,
                commands::farm_display_name,
                commands::set_farm_display_name,
                commands::wholesale_invoice_bill,
                commands::leftover_invoice_bill,
                commands::list_wholesale_orders,
                commands::pack_by_customer,
                commands::unpriced_settlement_line,
                commands::deliver_refusal_line,
                commands::owed_summary,
                commands::cover_plan,
                commands::reachability_for_date,
                commands::phone_captures,
                commands::confirm_phone_captures,
                commands::discard_phone_capture,
                commands::admin_phone_status,
                commands::pair_admin_phone,
                commands::retire_admin_phone,
                commands::phone_pull_view,
                commands::pull_phone_captures,
                commands::auto_pull_phone_captures,
            ])
            .run(tauri::generate_context!())
            .expect("error while running tauri application");
    }

    #[cfg(not(debug_assertions))]
    {
        builder
            .invoke_handler(tauri::generate_handler![
                commands::list_crops,
                commands::update_crop_seed_rate,
                commands::add_crop,
                commands::rename_crop,
                commands::update_crop_growth_days,
                commands::list_trays,
                commands::today_view,
                commands::sow_tray,
                commands::advance_tray,
                commands::advance_trays,
                commands::harvest_tray,
                commands::harvest_trays,
                commands::harvest_groups,
                commands::early_harvest_groups,
                commands::discard_tray,
                commands::discard_from_group,
                commands::undo_last,
                commands::capacity_by_harvest_date,
                commands::shelf_capacity,
                commands::set_shelf_capacity,
                commands::shelf_pressure,
                commands::overcommit_warning,
                commands::pay_amount_warning,
                commands::duplicate_income_warning,
                commands::money_status,
                commands::list_orders,
                commands::preview_stripe_key,
                commands::confirm_stripe_key,
                commands::set_checkout_endpoint_url,
                commands::list_offers,
                commands::set_offer,
                commands::remove_offer,
                commands::generate_shop_page,
                commands::open_shop_page_folder,
                commands::write_leftover_shop_page,
                commands::poll_stripe,
                commands::take_new_paid_orders,
                commands::reconciliation,
                commands::list_snapshots,
                commands::take_snapshot,
                commands::restore_snapshot,
                commands::farm_location,
                commands::open_farm_folder,
                commands::export_bundle,
                commands::open_export_folder,
                commands::preview_import,
                commands::apply_import,
                commands::apply_cutover_import,
                commands::recount_state,
                commands::apply_recount,
                commands::check_attention,
                commands::resolve_attention,
                commands::dismiss_attention,
                commands::list_cost_categories,
                commands::receipt_source_info,
                commands::record_cost,
                commands::list_expenses,
                commands::list_money_corrections,
                commands::list_unapplied_facts,
                commands::correct_expense,
                commands::void_expense,
                commands::cost_per_tray,
                commands::list_mileage_trips,
                commands::record_mileage_trip,
                commands::correct_mileage_trip,
                commands::void_mileage_trip,
                commands::list_assets,
                commands::record_asset,
                commands::correct_asset,
                commands::void_asset,
                commands::list_income_categories,
                commands::list_income,
                commands::record_income,
                commands::correct_income,
                commands::void_income,
                commands::storefront_reading,
                commands::refresh_storefront,
                commands::health_status,
                commands::dock_folds,
                commands::dock_port_start,
                commands::dock_port_stop,
                commands::dock_port_status,
                commands::run_full_verify,
                commands::marketing_summary,
                commands::list_venues,
                commands::record_venue,
                commands::drop_sample,
                commands::correct_venue,
                commands::sample_gate_line,
                commands::decide_standing_request,
                commands::qr_link_for_token,
                commands::standing_pull_view,
                commands::pull_standing_requests,
                commands::harvest_commitments,
                commands::record_harvest_coverage,
                commands::log_touch,
                commands::list_open_followups,
                commands::list_samples,
                commands::resolve_followup,
                commands::change_stage,
                commands::observe_reviews,
                commands::list_stages,
                commands::weekly_actions,
                commands::reputation_counts,
                commands::capacity_sight,
                commands::latest_review,
                commands::record_review_request,
                commands::list_review_requests,
                commands::gbp_verified_on,
                commands::set_gbp_verified,
                commands::scan_view,
                commands::scan_config,
                commands::set_scan_config,
                commands::pull_scans,
                commands::standing_demand,
                commands::record_wholesale_order,
                commands::deliver_wholesale_order,
                commands::pay_wholesale_order,
                commands::settle_order_with_income,
                commands::write_off_summary,
                commands::cash_collected,
                commands::cash_rows,
                commands::write_off_rows,
                commands::unpriced_exposure,
                commands::unpriced_orders,
                commands::cash_out,
                commands::expense_rows,
                commands::net_cash,
                commands::bad_debt_summary,
                commands::bad_debt_rows,
                commands::cash_by_category,
                commands::income_corrections_count,
                commands::income_correction_rows,
                commands::void_wholesale_order,
                commands::reverse_wholesale_payment,
                commands::bad_debt_confirm_line,
                commands::bad_debt_trail_line,
                commands::write_off_bad_debt,
                commands::mint_wholesale_payment_link,
                commands::wholesale_payment_link_qr,
                commands::leftover_payment_link_qr,
                commands::record_leftover_listing,
                commands::list_leftover_listings,
                commands::mint_leftover_payment_link,
                commands::pay_leftover_listing_cash,
                commands::record_seed_received,
                commands::seed_on_hand,
                commands::farm_display_name,
                commands::set_farm_display_name,
                commands::wholesale_invoice_bill,
                commands::leftover_invoice_bill,
                commands::list_wholesale_orders,
                commands::pack_by_customer,
                commands::unpriced_settlement_line,
                commands::deliver_refusal_line,
                commands::owed_summary,
                commands::cover_plan,
                commands::reachability_for_date,
                commands::phone_captures,
                commands::confirm_phone_captures,
                commands::discard_phone_capture,
                commands::admin_phone_status,
                commands::pair_admin_phone,
                commands::retire_admin_phone,
                commands::phone_pull_view,
                commands::pull_phone_captures,
                commands::auto_pull_phone_captures,
            ])
            .run(tauri::generate_context!())
            .expect("error while running tauri application");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Local, TimeZone};
    use rusqlite::{params, Connection};
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn mem() -> Connection {
        db::open_in_memory().expect("open in-memory db")
    }

    fn temp_farm_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("farm-os-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_bytes(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap_or_default()
    }

    #[test]
    fn migration_and_seed_eight_crops_idempotent() {
        let conn = mem();
        let crops = trays::list_crops(&conn).unwrap();
        assert_eq!(crops.len(), 8);
        assert_eq!(crops[0].id, "dun-peas");
        assert_eq!(crops[0].growth_days, 9);
        assert_eq!(crops[0].blackout_days, 3);
        assert_eq!(crops[1].id, "mellow-mix");
        assert_eq!(crops[2].id, "spicy-mix");
        assert_eq!(crops[3].id, "red-arrow-radish");
        assert_eq!(crops[4].id, "purple-kohlrabi");
        assert_eq!(crops[5].id, "sunflower");
        assert_eq!(crops[6].id, "broccoli");
        assert_eq!(crops[7].id, "kale");
        for (i, c) in crops.iter().enumerate() {
            assert_eq!(c.sort_order, (i as i64) + 1);
        }

        // Running migrate again must not duplicate.
        db::migrate(&conn).unwrap();
        let again = trays::list_crops(&conn).unwrap();
        assert_eq!(again.len(), 8);
    }

    #[test]
    fn sow_tray_writes_tray_and_event() {
        let mut conn = mem();
        let tray = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        assert_eq!(tray.state, "blackout");
        assert_eq!(tray.crop_id, "dun-peas");
        assert_eq!(tray.quantity, 1);
        assert_eq!(tray.growth_days_at_sow, Some(9));
        assert_eq!(tray.blackout_days_at_sow, Some(3));
        assert!(tray.sown_on.is_some());
        assert_eq!(tray.sown_on, tray.blackout_on);
        assert!(tray.cover_check_date.is_some());
        assert!(tray.expected_harvest_date.is_some());

        let listed = trays::list_trays(&conn).unwrap();
        assert_eq!(listed.len(), 1);

        // tray.sown + consumption.physical (trays)
        assert_eq!(trays::count_event_log(&conn).unwrap(), 2);
        assert!(trays::event_inverse_nonempty(&conn, 1).unwrap());
    }

    #[test]
    fn undo_last_after_sow_clears_tray_and_appends_undo() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        assert_eq!(trays::list_trays(&conn).unwrap().len(), 1);

        let result = trays::undo_last(&mut conn).unwrap();
        assert!(result.is_some());
        let u = result.unwrap();
        assert_eq!(u.undone_kind, "tray.sown");
        assert_eq!(u.undoes_seq, 1);

        assert_eq!(trays::list_trays(&conn).unwrap().len(), 0);
        assert!(trays::event_undone_at(&conn, 1).unwrap().is_some());
        // tray.sown + tray consumption + undo marker
        assert_eq!(trays::count_event_log(&conn).unwrap(), 3);
    }

    #[test]
    fn advance_then_two_undos_returns_to_sown_and_light_refuses_advance() {
        // Construct in sown explicitly — sow_tray now lands in blackout.
        let mut conn = mem();
        let tray = trays::insert_sown_tray(&mut conn, "dun-peas", 1).unwrap();
        let id = tray.id.clone();
        assert_eq!(tray.state, "sown");

        let t = trays::advance_tray(&mut conn, &id).unwrap();
        assert_eq!(t.state, "blackout");
        assert!(t.blackout_on.is_some());

        let t = trays::advance_tray(&mut conn, &id).unwrap();
        assert_eq!(t.state, "light");
        assert!(t.light_on.is_some());

        let err = trays::advance_tray(&mut conn, &id).unwrap_err();
        assert!(
            err.contains("already under light"),
            "third advance must refuse, got: {err}"
        );

        trays::undo_last(&mut conn).unwrap();
        trays::undo_last(&mut conn).unwrap();

        let t = trays::get_tray(&conn, &id).unwrap();
        assert_eq!(t.state, "sown");
        assert!(t.sown_on.is_some());
        assert!(t.blackout_on.is_none());
        assert!(t.light_on.is_none());
        assert!(t.harvested_on.is_none());
    }

    #[test]
    fn discard_from_blackout_then_undo_restores() {
        let mut conn = mem();
        let tray = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let id = tray.id.clone();
        assert_eq!(tray.state, "blackout");

        let t = trays::discard_tray(&mut conn, &id).unwrap();
        assert_eq!(t.state, "discarded");
        assert!(t.discarded_on.is_some());
        // list_trays excludes discarded
        assert_eq!(trays::list_trays(&conn).unwrap().len(), 0);

        trays::undo_last(&mut conn).unwrap();
        let t = trays::get_tray(&conn, &id).unwrap();
        assert_eq!(t.state, "blackout");
        assert!(t.discarded_on.is_none());
        assert!(t.blackout_on.is_some());
    }

    #[test]
    fn sow_tray_lands_in_blackout_with_matching_dates() {
        let mut conn = mem();
        let before = trays::count_event_log(&conn).unwrap();
        let tray = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        assert_eq!(tray.state, "blackout");
        assert_eq!(tray.sown_on, tray.blackout_on);
        assert!(tray.sown_on.is_some());
        assert_eq!(trays::count_event_log(&conn).unwrap(), before + 2);
        assert_eq!(trays::count_event_kind(&conn, "tray.sown").unwrap(), 1);

        trays::undo_last(&mut conn).unwrap();
        assert_eq!(trays::list_trays(&conn).unwrap().len(), 0);
    }

    #[test]
    fn advance_trays_writes_one_event_undo_restores_all() {
        let mut conn = mem();
        let mut ids = Vec::new();
        for _ in 0..3 {
            let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
            ids.push(t.id);
        }
        let before = trays::count_event_log(&conn).unwrap();
        trays::advance_trays(&mut conn, &ids).unwrap();
        assert_eq!(trays::count_event_log(&conn).unwrap(), before + 1);
        assert_eq!(trays::count_event_kind(&conn, "trays.advanced").unwrap(), 1);

        for id in &ids {
            assert_eq!(trays::get_tray(&conn, id).unwrap().state, "light");
        }

        trays::undo_last(&mut conn).unwrap();
        for id in &ids {
            let t = trays::get_tray(&conn, id).unwrap();
            assert_eq!(t.state, "blackout");
            assert!(t.light_on.is_none());
        }
    }

    /// Harvest writes one trays.harvested event (plus planting consumption); one undo restores all.
    #[test]
    fn harvest_trays_writes_one_event_undo_restores() {
        let mut conn = mem();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
            trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
            ids.push(t.id);
        }
        let _before = trays::count_event_log(&conn).unwrap();
        let harvested_before = trays::count_event_kind(&conn, "trays.harvested").unwrap();
        let consumption_before = trays::count_event_kind(&conn, "consumption.physical").unwrap();
        trays::harvest_trays(&mut conn, &ids, 20.0).unwrap();
        assert_eq!(
            trays::count_event_kind(&conn, "trays.harvested").unwrap(),
            harvested_before + 1,
            "a multi-tray harvest must remain ONE atomic grow event so one undo reverses it all"
        );
        assert_eq!(
            trays::count_event_kind(&conn, "consumption.physical").unwrap(),
            consumption_before + 2,
            "harvest emits one planting consumption record per harvested tray row"
        );
        assert_eq!(
            trays::count_event_kind(&conn, "trays.harvested").unwrap(),
            1
        );

        for id in &ids {
            let t = trays::get_tray(&conn, id).unwrap();
            assert_eq!(t.state, "harvested");
            assert!(t.actual_yield_oz.is_some());
        }

        trays::undo_last(&mut conn).unwrap();
        for id in &ids {
            let t = trays::get_tray(&conn, id).unwrap();
            assert_eq!(t.state, "light");
            assert!(t.harvested_on.is_none());
            assert!(t.actual_yield_oz.is_none());
        }
    }

    #[test]
    fn today_view_seeded_day_sums_quantity() {
        let mut conn = mem();
        // One row quantity=4 past cover check → move_to_light count 4
        let mtl = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        trays::test_shift_dates(&mut conn, &mtl.id, -4).unwrap();

        // One row quantity=6 past harvest → need light + past harvest date
        let harv = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&harv.id)).unwrap();
        // growth_days=9; shift back 10 so expected_harvest <= today
        trays::test_shift_dates(&mut conn, &harv.id, -10).unwrap();

        let view = trays::today_view(&conn).unwrap();
        let mtl = view.move_to_light.expect("move_to_light");
        assert_eq!(mtl.tray_count, 4);
        assert_eq!(mtl.tray_ids.len(), 1);

        assert_eq!(view.harvests.len(), 1);
        assert_eq!(view.harvests[0].crop_id, "dun-peas");
        assert_eq!(view.harvests[0].tray_count, 6);
        assert_eq!(view.harvests[0].estimated_yield_oz, 60.0);
        let hs = view.harvest_summary.expect("harvest_summary");
        assert_eq!(hs.tray_count, 6);
        assert_eq!(hs.variety_count, 1);
        assert_eq!(hs.estimated_yield_oz, 60.0);
        assert_eq!(hs.single_crop_name.as_deref(), Some("Dun peas"));
        assert_eq!(view.active_tray_count, 10);
    }

    #[test]
    fn today_view_sown_today_has_next_event_no_rows() {
        let mut conn = mem();
        let tray = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let view = trays::today_view(&conn).unwrap();
        assert!(view.move_to_light.is_none());
        assert!(view.harvests.is_empty());
        assert_eq!(view.active_tray_count, 1);
        assert!(view.sown_today);
        assert_eq!(view.next_events.len(), 1);
        let ne = &view.next_events[0];
        assert_eq!(ne.kind, "light");
        assert_eq!(ne.date, tray.cover_check_date.unwrap());
        assert_eq!(ne.tray_count, 1);
        assert_eq!(ne.crop_name, "Dun peas");
    }

    #[test]
    fn due_day_reachable_via_backdate() {
        let mut conn = mem();
        let tray = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let id = tray.id.clone();

        let v = trays::today_view(&conn).unwrap();
        assert!(v.move_to_light.is_none());
        assert!(v.harvests.is_empty());
        assert!(v.sown_today);
        assert_eq!(v.next_events[0].kind, "light");

        trays::dev_backdate_tray(&mut conn, &id, 4).unwrap();
        let v = trays::today_view(&conn).unwrap();
        let mtl = v.move_to_light.expect("move due after backdate");
        assert_eq!(mtl.tray_count, 1);
        assert_eq!(mtl.tray_ids, vec![id.clone()]);

        trays::advance_trays(&mut conn, &mtl.tray_ids).unwrap();
        assert_eq!(trays::count_event_kind(&conn, "trays.advanced").unwrap(), 1);

        trays::dev_backdate_tray(&mut conn, &id, 10).unwrap();
        let v = trays::today_view(&conn).unwrap();
        assert!(v.move_to_light.is_none());
        assert_eq!(v.harvests.len(), 1);
        assert_eq!(v.harvests[0].tray_count, 1);
        assert_eq!(v.harvests[0].estimated_yield_oz, 10.0);

        trays::harvest_trays(&mut conn, &v.harvests[0].tray_ids, 10.0).unwrap();
        assert_eq!(
            trays::count_event_kind(&conn, "trays.harvested").unwrap(),
            1
        );
        trays::undo_last(&mut conn).unwrap();
        assert_eq!(trays::get_tray(&conn, &id).unwrap().state, "light");
    }

    /// Harvest writes one trays.harvested event (plus planting consumption); one undo restores all.
    #[test]
    fn harvest_groups_three_crops_one_event_undo_restores() {
        let mut conn = mem();
        let crops = ["dun-peas", "mellow-mix", "spicy-mix"];
        let mut all_ids = Vec::new();
        let mut groups = Vec::new();
        for crop in crops {
            let t = trays::sow_tray(&mut conn, crop, 2).unwrap();
            trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
            all_ids.push(t.id.clone());
            groups.push(crate::models::HarvestInput {
                tray_ids: vec![t.id],
                actual_yield_oz: 14.0,
            });
        }
        let _before = trays::count_event_log(&conn).unwrap();
        let harvested_before = trays::count_event_kind(&conn, "trays.harvested").unwrap();
        let consumption_before = trays::count_event_kind(&conn, "consumption.physical").unwrap();
        trays::harvest_groups(&mut conn, &groups).unwrap();
        assert_eq!(
            trays::count_event_kind(&conn, "trays.harvested").unwrap(),
            harvested_before + 1,
            "a multi-tray harvest must remain ONE atomic grow event so one undo reverses it all"
        );
        assert_eq!(
            trays::count_event_kind(&conn, "consumption.physical").unwrap(),
            consumption_before + 3,
            "harvest emits one planting consumption record per harvested tray row"
        );
        assert_eq!(
            trays::count_event_kind(&conn, "trays.harvested").unwrap(),
            1
        );

        for id in &all_ids {
            let t = trays::get_tray(&conn, id).unwrap();
            assert_eq!(t.state, "harvested");
            assert!(t.actual_yield_oz.is_some());
        }

        trays::undo_last(&mut conn).unwrap();
        for id in &all_ids {
            let t = trays::get_tray(&conn, id).unwrap();
            assert_eq!(t.state, "light");
            assert!(t.harvested_on.is_none());
            assert!(t.actual_yield_oz.is_none());
        }
    }

    #[test]
    fn harvest_groups_illegal_leaves_db_unchanged() {
        let mut conn = mem();
        let a = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&a.id)).unwrap();
        let b = trays::sow_tray(&mut conn, "mellow-mix", 1).unwrap();
        // b stays in blackout — illegal to harvest

        let events_before = trays::count_event_log(&conn).unwrap();
        let state_a = trays::get_tray(&conn, &a.id).unwrap().state;
        let state_b = trays::get_tray(&conn, &b.id).unwrap().state;

        let err = trays::harvest_groups(
            &mut conn,
            &[
                crate::models::HarvestInput {
                    tray_ids: vec![a.id.clone()],
                    actual_yield_oz: 10.0,
                },
                crate::models::HarvestInput {
                    tray_ids: vec![b.id.clone()],
                    actual_yield_oz: 7.0,
                },
            ],
        )
        .unwrap_err();
        assert!(err.contains("cannot harvest") || err.contains("light"));

        assert_eq!(trays::count_event_log(&conn).unwrap(), events_before);
        assert_eq!(trays::get_tray(&conn, &a.id).unwrap().state, state_a);
        assert_eq!(trays::get_tray(&conn, &b.id).unwrap().state, state_b);
        assert!(trays::get_tray(&conn, &a.id)
            .unwrap()
            .actual_yield_oz
            .is_none());
    }

    #[test]
    fn today_view_harvest_summary_three_crops() {
        let mut conn = mem();
        for crop in ["dun-peas", "mellow-mix", "spicy-mix"] {
            let t = trays::sow_tray(&mut conn, crop, 2).unwrap();
            trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
            trays::test_shift_dates(&mut conn, &t.id, -10).unwrap();
        }
        let view = trays::today_view(&conn).unwrap();
        assert_eq!(view.harvests.len(), 3);
        let hs = view.harvest_summary.expect("summary");
        assert_eq!(hs.variety_count, 3);
        assert_eq!(hs.tray_count, 6); // 2+2+2 SUM(quantity)
                                      // 2*10 + 2*7 + 2*6.5 = 20+14+13 = 47
        assert_eq!(hs.estimated_yield_oz, 47.0);
        assert!(hs.single_crop_name.is_none());
    }

    #[test]
    fn today_view_harvest_summary_single_crop_name() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
        trays::test_shift_dates(&mut conn, &t.id, -10).unwrap();
        let view = trays::today_view(&conn).unwrap();
        let hs = view.harvest_summary.expect("summary");
        assert_eq!(hs.variety_count, 1);
        assert_eq!(hs.tray_count, 3);
        assert_eq!(hs.estimated_yield_oz, 30.0);
        assert_eq!(hs.single_crop_name.as_deref(), Some("Dun peas"));
    }

    #[test]
    fn advance_trays_illegal_leaves_db_unchanged() {
        let mut conn = mem();
        let a = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let b = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        // Advance b to light, then harvested so it's terminal.
        trays::advance_trays(&mut conn, std::slice::from_ref(&b.id)).unwrap();
        trays::harvest_trays(&mut conn, std::slice::from_ref(&b.id), 5.0).unwrap();

        let state_before: Vec<(String, String)> = {
            let mut out = Vec::new();
            for id in [&a.id, &b.id] {
                let t = trays::get_tray(&conn, id).unwrap();
                out.push((t.id.clone(), t.state.clone()));
            }
            out
        };
        let events_before = trays::count_event_log(&conn).unwrap();

        let err = trays::advance_trays(&mut conn, &[a.id.clone(), b.id.clone()]).unwrap_err();
        assert!(err.contains("cannot advance") || err.contains("harvested"));

        assert_eq!(trays::count_event_log(&conn).unwrap(), events_before);
        for (id, state) in state_before {
            assert_eq!(trays::get_tray(&conn, &id).unwrap().state, state);
        }
    }

    fn light_group(conn: &mut Connection, crop: &str, qty: i64) -> (String, Vec<String>) {
        let t = trays::sow_tray(conn, crop, qty).unwrap();
        trays::advance_trays(conn, std::slice::from_ref(&t.id)).unwrap();
        (t.id.clone(), vec![t.id])
    }

    #[test]
    fn discard_from_group_partial_one_event_returns_reduced_group() {
        let mut conn = mem();
        let (id, ids) = light_group(&mut conn, "dun-peas", 6);
        let before = trays::count_event_log(&conn).unwrap();
        let remaining = trays::discard_from_group(&mut conn, &ids, 2)
            .unwrap()
            .expect("group remains");
        assert_eq!(trays::count_event_log(&conn).unwrap(), before + 1);
        assert_eq!(
            trays::count_event_kind(&conn, "trays.discarded").unwrap(),
            1
        );
        assert_eq!(remaining.tray_count, 4);
        assert_eq!(remaining.estimated_yield_oz, 40.0);
        assert_eq!(remaining.tray_ids, vec![id.clone()]);
        assert_eq!(trays::get_tray(&conn, &id).unwrap().quantity, 4);
    }

    #[test]
    fn discard_from_group_whole_returns_none() {
        let mut conn = mem();
        let (id, ids) = light_group(&mut conn, "dun-peas", 6);
        let remaining = trays::discard_from_group(&mut conn, &ids, 6).unwrap();
        assert!(remaining.is_none());
        assert_eq!(trays::get_tray(&conn, &id).unwrap().state, "discarded");
        assert!(trays::list_trays(&conn).unwrap().is_empty());
        let view = trays::today_view(&conn).unwrap();
        assert!(view.harvests.is_empty());
    }

    #[test]
    fn discard_from_group_split_creates_discarded_row() {
        let mut conn = mem();
        let (id, ids) = light_group(&mut conn, "dun-peas", 6);
        trays::discard_from_group(&mut conn, &ids, 2).unwrap();
        assert_eq!(trays::get_tray(&conn, &id).unwrap().quantity, 4);
        assert_eq!(trays::get_tray(&conn, &id).unwrap().state, "light");

        let discarded_qty: i64 = conn
            .query_row(
                "SELECT quantity FROM trays WHERE state = 'discarded'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(discarded_qty, 2);
        let discarded_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM trays WHERE state = 'discarded'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(discarded_count, 1);
    }

    #[test]
    fn discard_from_group_undo_restores_partial_and_full() {
        let mut conn = mem();
        let (id, ids) = light_group(&mut conn, "dun-peas", 6);
        trays::test_shift_dates(&mut conn, &id, -10).unwrap();
        assert_eq!(trays::today_view(&conn).unwrap().harvests[0].tray_count, 6);

        trays::discard_from_group(&mut conn, &ids, 2).unwrap();
        assert_eq!(trays::today_view(&conn).unwrap().harvests[0].tray_count, 4);

        trays::undo_last(&mut conn).unwrap();
        assert_eq!(trays::get_tray(&conn, &id).unwrap().quantity, 6);
        assert_eq!(trays::get_tray(&conn, &id).unwrap().state, "light");
        let discarded_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM trays WHERE state = 'discarded'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(discarded_count, 0);
        assert_eq!(trays::today_view(&conn).unwrap().harvests[0].tray_count, 6);

        trays::discard_from_group(&mut conn, &ids, 6).unwrap();
        trays::undo_last(&mut conn).unwrap();
        let t = trays::get_tray(&conn, &id).unwrap();
        assert_eq!(t.state, "light");
        assert!(t.discarded_on.is_none());
        assert_eq!(t.quantity, 6);
        assert_eq!(trays::today_view(&conn).unwrap().active_tray_count, 6);
        assert_eq!(trays::today_view(&conn).unwrap().harvests[0].tray_count, 6);
    }

    #[test]
    fn discard_from_group_illegal_leaves_db_unchanged() {
        let mut conn = mem();
        let (id, ids) = light_group(&mut conn, "dun-peas", 4);
        let events_before = trays::count_event_log(&conn).unwrap();
        let qty_before = trays::get_tray(&conn, &id).unwrap().quantity;

        for bad in [0_i64, -1, 5] {
            let err = trays::discard_from_group(&mut conn, &ids, bad).unwrap_err();
            assert!(!err.is_empty());
            assert_eq!(trays::count_event_log(&conn).unwrap(), events_before);
            assert_eq!(trays::get_tray(&conn, &id).unwrap().quantity, qty_before);
            assert_eq!(trays::get_tray(&conn, &id).unwrap().state, "light");
        }
    }

    #[test]
    fn discard_from_group_then_harvest_returned_ids_ok_stale_fails() {
        let mut conn = mem();
        let a = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let b = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        trays::advance_trays(&mut conn, &[a.id.clone(), b.id.clone()]).unwrap();
        let stale = vec![a.id.clone(), b.id.clone()];

        let remaining = trays::discard_from_group(&mut conn, &stale, 3)
            .unwrap()
            .expect("three remain");
        assert_eq!(remaining.tray_count, 3);

        // Stale ids include a fully discarded row → reject, no change.
        let events_before = trays::count_event_log(&conn).unwrap();
        let err = trays::harvest_groups(
            &mut conn,
            &[crate::models::HarvestInput {
                tray_ids: stale,
                actual_yield_oz: 30.0,
            }],
        )
        .unwrap_err();
        assert!(err.contains("light") || err.contains("cannot harvest"));
        assert_eq!(trays::count_event_log(&conn).unwrap(), events_before);

        trays::harvest_groups(
            &mut conn,
            &[crate::models::HarvestInput {
                tray_ids: remaining.tray_ids,
                actual_yield_oz: 28.0,
            }],
        )
        .unwrap();
        assert_eq!(
            trays::count_event_kind(&conn, "trays.harvested").unwrap(),
            1
        );
    }

    #[test]
    fn today_view_excludes_discarded_from_all_aggregates() {
        let mut conn = mem();
        // Harvest-due crop, then discard it.
        let harv = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&harv.id)).unwrap();
        trays::test_shift_dates(&mut conn, &harv.id, -10).unwrap();

        // Move-to-light crop that we also discard via discard_tray (blackout).
        let mtl = trays::sow_tray(&mut conn, "mellow-mix", 2).unwrap();
        trays::test_shift_dates(&mut conn, &mtl.id, -4).unwrap();

        let view = trays::today_view(&conn).unwrap();
        assert_eq!(view.active_tray_count, 6);
        assert!(view.move_to_light.is_some());
        assert_eq!(view.harvests.len(), 1);

        trays::discard_from_group(&mut conn, std::slice::from_ref(&harv.id), 4).unwrap();
        trays::discard_tray(&mut conn, &mtl.id).unwrap();

        let view = trays::today_view(&conn).unwrap();
        assert!(view.move_to_light.is_none());
        assert!(view.harvests.is_empty());
        assert!(view.harvest_summary.is_none());
        assert!(view.next_events.is_empty());
        assert_eq!(view.active_tray_count, 0);
    }

    #[test]
    fn sowbug_selected_crop_is_written_and_shown() {
        let mut conn = mem();
        let crops = trays::list_crops(&conn).unwrap();
        let first = &crops[0];
        let picked = &crops[1];
        assert!(
            picked.sort_order > first.sort_order,
            "picked crop must not be first by sort_order"
        );
        let picked_id = picked.id.clone();
        let picked_name = picked.name.clone();
        let first_name = first.name.clone();

        let tray = trays::sow_tray(&mut conn, &picked_id, 1).unwrap();

        let payload: String = conn
            .query_row(
                "SELECT payload FROM event_log WHERE kind = 'tray.sown' ORDER BY seq ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            v.get("cropId").and_then(|x| x.as_str()),
            Some(picked_id.as_str())
        );

        let row_crop_id: String = conn
            .query_row("SELECT crop_id FROM trays WHERE id = ?1", [&tray.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(row_crop_id, picked_id);

        let view = trays::today_view(&conn).unwrap();
        assert_eq!(view.next_events.len(), 1);
        assert_eq!(view.next_events[0].crop_name, picked_name);
        assert_eq!(view.next_events[0].tray_count, 1);
        assert_ne!(
            view.next_events[0].crop_name, first_name,
            "must not stamp the first crop's name"
        );
    }

    #[test]
    fn sowbug_two_crops_same_cover_date_stay_distinct() {
        let mut conn = mem();
        let crops = trays::list_crops(&conn).unwrap();
        let a_id = crops[0].id.clone();
        let a_name = crops[0].name.clone();
        let a_blackout = crops[0].blackout_days;
        let b_id = crops[1].id.clone();
        let b_name = crops[1].name.clone();

        conn.execute(
            "UPDATE crops SET blackout_days = ?1 WHERE id = ?2",
            params![a_blackout, &b_id],
        )
        .unwrap();

        trays::sow_tray(&mut conn, &a_id, 1).unwrap();
        trays::sow_tray(&mut conn, &b_id, 1).unwrap();

        let view = trays::today_view(&conn).unwrap();
        assert_eq!(view.next_events.len(), 2);
        assert_eq!(view.next_events[0].date, view.next_events[1].date);
        let names: HashSet<&str> = view
            .next_events
            .iter()
            .map(|ne| ne.crop_name.as_str())
            .collect();
        assert!(names.contains(a_name.as_str()));
        assert!(names.contains(b_name.as_str()));
        assert_eq!(names.len(), 2);
        assert!(view.next_events.iter().all(|ne| ne.tray_count == 1));
        assert!(
            !view
                .next_events
                .iter()
                .any(|ne| ne.crop_name == a_name && ne.tray_count == 2),
            "must not sum both under the first crop's name"
        );
    }

    #[test]
    fn sowbug_different_dates_both_crops_visible() {
        let mut conn = mem();
        let crops = trays::list_crops(&conn).unwrap();
        let a_id = crops[0].id.clone();
        let a_name = crops[0].name.clone();
        let a_blackout = crops[0].blackout_days;
        let b = crops
            .iter()
            .find(|c| c.blackout_days != a_blackout)
            .expect("seed has crops with different blackout_days");
        let b_id = b.id.clone();
        let b_name = b.name.clone();

        trays::sow_tray(&mut conn, &a_id, 1).unwrap();
        trays::sow_tray(&mut conn, &b_id, 1).unwrap();

        let view = trays::today_view(&conn).unwrap();
        assert_eq!(view.next_events.len(), 2);
        assert!(
            view.next_events[0].date < view.next_events[1].date,
            "dates must be ascending, got {} then {}",
            view.next_events[0].date,
            view.next_events[1].date
        );
        let names: HashSet<&str> = view
            .next_events
            .iter()
            .map(|ne| ne.crop_name.as_str())
            .collect();
        assert!(names.contains(a_name.as_str()));
        assert!(names.contains(b_name.as_str()));
        assert!(view.next_events.iter().all(|ne| ne.tray_count == 1));
    }

    // --- Stage 3: snapshots and restore ---

    #[test]
    fn vacuum_into_includes_committed_wal_content() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snap_dir = dir.join("snapshots");
        fs::create_dir_all(&snap_dir).unwrap();

        let mut conn = db::open_and_migrate(&farm).unwrap();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        // Committed pages may still sit in the WAL; do not checkpoint.
        // A plain file copy of farm.db alone could miss them — VACUUM INTO must not.

        let info = snapshots::take_snapshot(&mut conn, &snap_dir).unwrap();
        // Snapshot is a single compact file — no -wal / -shm beside it.
        assert!(!Path::new(&(info.path.clone() + "-wal")).exists());
        assert!(!Path::new(&(info.path.clone() + "-shm")).exists());

        let snap = Connection::open(&info.path).unwrap();
        let count: i64 = snap
            .query_row(
                "SELECT COUNT(*) FROM trays WHERE state != 'discarded'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let crop: String = snap
            .query_row("SELECT crop_id FROM trays LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(crop, "dun-peas");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_keeps_48h_and_one_per_day_for_30_days() {
        let now = Local
            .with_ymd_and_hms(2026, 8, 5, 18, 0, 0)
            .single()
            .unwrap();

        let mut entries: Vec<(PathBuf, chrono::DateTime<Local>)> = Vec::new();
        // Two within 48h on the same day — both kept.
        let a = PathBuf::from("farm-2026-08-05-100000.db");
        let b = PathBuf::from("farm-2026-08-04-200000.db");
        entries.push((a.clone(), now - Duration::hours(8)));
        entries.push((b.clone(), now - Duration::hours(22)));
        // Older than 48h but within 30 days — keep newest per day only.
        let c_old = PathBuf::from("farm-2026-07-20-080000.db");
        let c_new = PathBuf::from("farm-2026-07-20-210000.db");
        let day20 = Local
            .with_ymd_and_hms(2026, 7, 20, 8, 0, 0)
            .single()
            .unwrap();
        let day20_late = Local
            .with_ymd_and_hms(2026, 7, 20, 21, 0, 0)
            .single()
            .unwrap();
        entries.push((c_old.clone(), day20));
        entries.push((c_new.clone(), day20_late));
        // Outside 30 calendar days — deleted.
        let ancient = PathBuf::from("farm-2026-06-01-120000.db");
        let ancient_t = Local
            .with_ymd_and_hms(2026, 6, 1, 12, 0, 0)
            .single()
            .unwrap();
        entries.push((ancient.clone(), ancient_t));
        // Another day within 30 days.
        let d = PathBuf::from("farm-2026-07-15-090000.db");
        let day15 = Local
            .with_ymd_and_hms(2026, 7, 15, 9, 0, 0)
            .single()
            .unwrap();
        entries.push((d.clone(), day15));

        let keep = snapshots::retention_keep_set(&entries, now);
        let expected: HashSet<PathBuf> = [a, b, c_new, d].into_iter().collect();
        assert_eq!(keep, expected);
        assert!(!keep.contains(&c_old));
        assert!(!keep.contains(&ancient));
    }

    #[test]
    fn restore_validation_rejects_bad_files_live_farm_untouched() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snap_dir = dir.join("snapshots");
        fs::create_dir_all(&snap_dir).unwrap();

        let mut conn = db::open_and_migrate(&farm).unwrap();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        drop(conn);
        // Reopen managed-style.
        let db = Mutex::new(db::open_and_migrate(&farm).unwrap());
        let before = file_bytes(&farm);

        let empty = dir.join("empty.db");
        fs::write(&empty, []).unwrap();
        let err = snapshots::restore_snapshot(&db, &farm, &snap_dir, &empty).unwrap_err();
        assert_eq!(err, "That file isn't a Farm OS farm.");
        assert_eq!(file_bytes(&farm), before);

        let nonsense = dir.join("nonsense.db");
        fs::write(&nonsense, b"not a sqlite database at all!!").unwrap();
        let err = snapshots::restore_snapshot(&db, &farm, &snap_dir, &nonsense).unwrap_err();
        assert_eq!(err, "That file isn't a Farm OS farm.");
        assert_eq!(file_bytes(&farm), before);

        // Valid SQLite, wrong schema (no farm tables / user_version 0).
        let wrong = dir.join("wrong.db");
        {
            let c = Connection::open(&wrong).unwrap();
            c.execute_batch("CREATE TABLE unrelated (id INTEGER);")
                .unwrap();
        }
        let err = snapshots::restore_snapshot(&db, &farm, &snap_dir, &wrong).unwrap_err();
        assert_eq!(err, "That file isn't a Farm OS farm.");
        assert_eq!(file_bytes(&farm), before);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_round_trip_removes_second_sow() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snap_dir = dir.join("snapshots");
        fs::create_dir_all(&snap_dir).unwrap();

        let db = Mutex::new(db::open_and_migrate(&farm).unwrap());
        {
            let mut conn = db.lock().unwrap();
            trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
            let snap = snapshots::take_snapshot(&mut conn, &snap_dir).unwrap();
            trays::sow_tray(&mut conn, "mellow-mix", 1).unwrap();
            assert_eq!(trays::list_trays(&conn).unwrap().len(), 2);
            drop(conn);
            snapshots::restore_snapshot(&db, &farm, &snap_dir, Path::new(&snap.path)).unwrap();
        }
        let conn = db.lock().unwrap();
        let trays = trays::list_trays(&conn).unwrap();
        assert_eq!(trays.len(), 1);
        assert_eq!(trays[0].crop_id, "dun-peas");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_removes_stale_wal_from_previous_farm() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snap_dir = dir.join("snapshots");
        fs::create_dir_all(&snap_dir).unwrap();

        let db = Mutex::new(db::open_and_migrate(&farm).unwrap());
        let snap_path = {
            let mut conn = db.lock().unwrap();
            trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
            let snap = snapshots::take_snapshot(&mut conn, &snap_dir).unwrap();
            // Pre-restore transaction that must not survive the restore.
            trays::sow_tray(&mut conn, "mellow-mix", 2).unwrap();
            assert_eq!(trays::list_trays(&conn).unwrap().len(), 2);
            snap.path
        };

        // Release file mappings, then plant decoy sidecars beside farm.db.
        // If restore left these in place when copying the snapshot, reopen
        // would fail or apply foreign WAL frames.
        {
            let mut guard = db.lock().unwrap();
            let old = std::mem::replace(&mut *guard, Connection::open_in_memory().unwrap());
            drop(old);
        }
        let wal = snapshots::farm_wal_path(&farm);
        let shm = snapshots::farm_shm_path(&farm);
        let _ = fs::remove_file(&wal);
        let _ = fs::remove_file(&shm);
        fs::write(&wal, b"stale-wal-bytes").unwrap();
        fs::write(&shm, b"stale-shm-bytes").unwrap();

        // Restore must succeed — proving decoys were deleted before the copy.
        // A fresh -wal after journal_mode=WAL reopen is normal; do not assert
        // on sidecar existence after restore returns.
        snapshots::restore_snapshot(&db, &farm, &snap_dir, Path::new(&snap_path)).unwrap();

        let conn = db.lock().unwrap();
        let live = trays::list_trays(&conn).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].crop_id, "dun-peas");
        assert!(!live.iter().any(|t| t.crop_id == "mellow-mix"));

        // Restored farm matches the snapshot exactly.
        let snap_conn = Connection::open(&snap_path).unwrap();
        let snap_trays = trays::list_trays(&snap_conn).unwrap();
        assert_eq!(live.len(), snap_trays.len());
        assert_eq!(live[0].id, snap_trays[0].id);
        assert_eq!(live[0].crop_id, snap_trays[0].crop_id);
        assert_eq!(live[0].quantity, snap_trays[0].quantity);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_takes_pre_restore_snapshot_before_touching() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snap_dir = dir.join("snapshots");
        fs::create_dir_all(&snap_dir).unwrap();

        let db = Mutex::new(db::open_and_migrate(&farm).unwrap());
        let source = {
            let mut conn = db.lock().unwrap();
            trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
            let early = snapshots::take_snapshot(&mut conn, &snap_dir).unwrap();
            trays::sow_tray(&mut conn, "spicy-mix", 3).unwrap();
            early.path
        };
        let before_count = snapshots::list_snapshots(&snap_dir).unwrap().len();

        snapshots::restore_snapshot(&db, &farm, &snap_dir, Path::new(&source)).unwrap();

        let after = snapshots::list_snapshots(&snap_dir).unwrap();
        assert!(after.len() > before_count);
        // Newest pre-restore snapshot should contain the spicy-mix tray.
        let newest = &after[0];
        let probe = Connection::open(&newest.path).unwrap();
        let spicy: i64 = probe
            .query_row(
                "SELECT COUNT(*) FROM trays WHERE crop_id = 'spicy-mix'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(spicy, 1);
        // Live farm is restored to pre-spicy state.
        let conn = db.lock().unwrap();
        assert_eq!(trays::list_trays(&conn).unwrap().len(), 1);
        assert_eq!(trays::list_trays(&conn).unwrap()[0].crop_id, "dun-peas");

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Stage 3: recount ---

    #[test]
    fn recount_state_one_row_per_active_crop_sorted() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "kale", 2).unwrap();
        trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        trays::sow_tray(&mut conn, "mellow-mix", 1).unwrap();
        // Harvested trays are inactive — must not appear.
        let done = trays::sow_tray(&mut conn, "broccoli", 1).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&done.id)).unwrap();
        trays::harvest_trays(&mut conn, &[done.id], 5.0).unwrap();

        let state = trays::recount_state(&conn).unwrap();
        assert_eq!(state.len(), 3);
        assert_eq!(state[0].crop_id, "dun-peas");
        assert_eq!(state[0].app_quantity, 3);
        assert_eq!(state[1].crop_id, "mellow-mix");
        assert_eq!(state[1].app_quantity, 1);
        assert_eq!(state[2].crop_id, "kale");
        assert_eq!(state[2].app_quantity, 2);
        assert!(!state[0].tray_ids.is_empty());
    }

    #[test]
    fn recount_all_match_writes_no_event() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        trays::sow_tray(&mut conn, "kale", 1).unwrap();
        let before = trays::count_event_log(&conn).unwrap();
        let result = trays::apply_recount(
            &mut conn,
            &[
                crate::models::RecountEntry {
                    crop_id: "dun-peas".into(),
                    counted_quantity: 2,
                },
                crate::models::RecountEntry {
                    crop_id: "kale".into(),
                    counted_quantity: 1,
                },
            ],
        )
        .unwrap();
        assert_eq!(result.unchanged, 2);
        assert!(result.adjusted_down.is_empty());
        assert!(result.adjusted_up.is_empty());
        assert_eq!(trays::count_event_log(&conn).unwrap(), before);
        assert_eq!(trays::today_view(&conn).unwrap().active_tray_count, 3);
    }

    #[test]
    fn recount_three_crops_one_event_undo_reverses_all() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        trays::sow_tray(&mut conn, "mellow-mix", 2).unwrap();
        trays::sow_tray(&mut conn, "kale", 1).unwrap();
        let before = trays::count_event_log(&conn).unwrap();

        let result = trays::apply_recount(
            &mut conn,
            &[
                crate::models::RecountEntry {
                    crop_id: "dun-peas".into(),
                    counted_quantity: 2, // -2
                },
                crate::models::RecountEntry {
                    crop_id: "mellow-mix".into(),
                    counted_quantity: 2, // match
                },
                crate::models::RecountEntry {
                    crop_id: "kale".into(),
                    counted_quantity: 3, // +2
                },
            ],
        )
        .unwrap();
        assert_eq!(result.adjusted_down.len(), 1);
        assert_eq!(result.adjusted_down[0].quantity, 2);
        assert_eq!(result.adjusted_up.len(), 1);
        assert_eq!(result.adjusted_up[0].quantity, 2);
        assert_eq!(result.unchanged, 1);
        assert_eq!(trays::count_event_log(&conn).unwrap(), before + 1);
        assert_eq!(
            trays::count_event_kind(&conn, "recount.applied").unwrap(),
            1
        );
        assert_eq!(trays::today_view(&conn).unwrap().active_tray_count, 7); // 2+2+3

        trays::undo_last(&mut conn).unwrap();
        // After undo: back to 4 + 2 + 1.
        let state = trays::recount_state(&conn).unwrap();
        let peas = state.iter().find(|c| c.crop_id == "dun-peas").unwrap();
        let mix = state.iter().find(|c| c.crop_id == "mellow-mix").unwrap();
        let kale = state.iter().find(|c| c.crop_id == "kale").unwrap();
        assert_eq!(peas.app_quantity, 4);
        assert_eq!(mix.app_quantity, 2);
        assert_eq!(kale.app_quantity, 1);
        assert_eq!(trays::today_view(&conn).unwrap().active_tray_count, 7);
    }

    #[test]
    fn recount_shortfall_splits_multi_quantity_row() {
        let mut conn = mem();
        let tray = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        trays::apply_recount(
            &mut conn,
            &[crate::models::RecountEntry {
                crop_id: "dun-peas".into(),
                counted_quantity: 4,
            }],
        )
        .unwrap();
        let source = trays::get_tray(&conn, &tray.id).unwrap();
        assert_eq!(source.quantity, 4);
        assert_eq!(source.state, "blackout");
        let discarded_qty: i64 = conn
            .query_row(
                "SELECT quantity FROM trays WHERE state = 'discarded'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(discarded_qty, 2);
        let discarded_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM trays WHERE state = 'discarded'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(discarded_count, 1);
    }

    #[test]
    fn recount_surplus_inherits_from_newest_active_row() {
        let mut conn = mem();
        let first = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        trays::dev_backdate_tray(&mut conn, &first.id, 3).unwrap();
        let second = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        // second is newer (today); surplus should inherit its sown_on / growth / blackout.
        trays::apply_recount(
            &mut conn,
            &[crate::models::RecountEntry {
                crop_id: "dun-peas".into(),
                counted_quantity: 5, // +2
            }],
        )
        .unwrap();
        let added_id: String = conn
            .query_row(
                "SELECT id FROM trays
                 WHERE crop_id = 'dun-peas' AND state <> 'discarded'
                   AND id NOT IN (?1, ?2)",
                params![&first.id, &second.id],
                |r| r.get(0),
            )
            .unwrap();
        let added = trays::get_tray(&conn, &added_id).unwrap();
        assert_eq!(added.quantity, 2);
        assert_eq!(added.state, second.state);
        assert_eq!(added.sown_on, second.sown_on);
        assert_eq!(added.growth_days_at_sow, second.growth_days_at_sow);
        assert_eq!(added.blackout_days_at_sow, second.blackout_days_at_sow);
    }

    #[test]
    fn recount_invalid_entry_leaves_db_unchanged() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let events_before = trays::count_event_log(&conn).unwrap();
        let active_before = trays::today_view(&conn).unwrap().active_tray_count;
        let qty_before: i64 = conn
            .query_row(
                "SELECT quantity FROM trays WHERE crop_id = 'dun-peas' AND state <> 'discarded'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let err = trays::apply_recount(
            &mut conn,
            &[crate::models::RecountEntry {
                crop_id: "dun-peas".into(),
                counted_quantity: -1,
            }],
        )
        .unwrap_err();
        assert!(!err.is_empty());

        let err2 = trays::apply_recount(
            &mut conn,
            &[crate::models::RecountEntry {
                crop_id: "not-a-crop".into(),
                counted_quantity: 1,
            }],
        )
        .unwrap_err();
        assert!(err2.contains("unknown crop"));

        assert_eq!(trays::count_event_log(&conn).unwrap(), events_before);
        assert_eq!(
            trays::today_view(&conn).unwrap().active_tray_count,
            active_before
        );
        let qty_after: i64 = conn
            .query_row(
                "SELECT quantity FROM trays WHERE crop_id = 'dun-peas' AND state <> 'discarded'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(qty_after, qty_before);
    }

    #[test]
    fn recount_updates_today_view_aggregates() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        trays::test_shift_dates(&mut conn, &t.id, -4).unwrap();
        assert_eq!(
            trays::today_view(&conn)
                .unwrap()
                .move_to_light
                .unwrap()
                .tray_count,
            4
        );

        trays::apply_recount(
            &mut conn,
            &[crate::models::RecountEntry {
                crop_id: "dun-peas".into(),
                counted_quantity: 1,
            }],
        )
        .unwrap();

        let view = trays::today_view(&conn).unwrap();
        assert_eq!(view.active_tray_count, 1);
        assert_eq!(view.move_to_light.unwrap().tray_count, 1);
    }

    // --- Stage 3: attention ---

    #[test]
    fn migration_upgrades_v1_farm_in_place_no_row_loss() {
        let conn = db::open_v1_in_memory().unwrap();
        let tray = trays::get_tray(&conn, db::FIXTURE_V1_TRAY_ID).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);

        db::migrate(&conn).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, db::SCHEMA_VERSION);

        let still = trays::get_tray(&conn, &tray.id).unwrap();
        assert_eq!(still.quantity, 3);
        assert_eq!(still.crop_id, "dun-peas");

        let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='attention'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table, 1);
    }

    #[test]
    fn attention_idempotent_until_resolved_then_can_recreate() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
        // growth_days=9; shift 13 → 4 days past harvest.
        trays::test_shift_dates(&mut conn, &t.id, -13).unwrap();

        let a = attention::check_attention(&conn).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, "tray.overdue_harvest");
        let id1 = a[0].id.clone();

        let b = attention::check_attention(&conn).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].id, id1);

        attention::dismiss_attention(&mut conn, &id1).unwrap();
        let resolved: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attention WHERE id = ?1 AND resolved_at IS NOT NULL",
                [&id1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved, 1);

        // Finding 23: a same-day dismissal holds. The kind is recurring — after
        // the fact moves (one more day) it raises again with a new id.
        let quiet = attention::check_attention(&conn).unwrap();
        assert!(
            quiet.iter().all(|i| i.kind != "tray.overdue_harvest"),
            "dismissed overdue harvest must stay quiet on the same day"
        );

        trays::test_shift_dates(&mut conn, &t.id, -1).unwrap();
        let c = attention::check_attention(&conn).unwrap();
        assert_eq!(c.len(), 1);
        assert_ne!(c[0].id, id1);
        assert_eq!(c[0].kind, "tray.overdue_harvest");
    }

    #[test]
    fn recount_and_attention_share_one_transaction() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_event BEFORE INSERT ON event_log
             BEGIN SELECT RAISE(ABORT, 'forced event failure'); END;",
        )
        .unwrap();

        let err = trays::apply_recount(
            &mut conn,
            &[crate::models::RecountEntry {
                crop_id: "dun-peas".into(),
                counted_quantity: 4, // surplus
            }],
        )
        .unwrap_err();
        assert!(err.contains("forced") || !err.is_empty());

        assert_eq!(trays::today_view(&conn).unwrap().active_tray_count, 2);
        let open: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attention WHERE resolved_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open, 0);
        assert_eq!(
            trays::count_event_kind(&conn, "recount.applied").unwrap(),
            0
        );
    }

    #[test]
    fn overdue_harvest_at_four_days_not_two_resolves_on_harvest() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();

        trays::test_shift_dates(&mut conn, &t.id, -11).unwrap(); // 2 days past
        assert!(attention::check_attention(&conn)
            .unwrap()
            .iter()
            .all(|i| i.kind != "tray.overdue_harvest"));

        trays::test_shift_dates(&mut conn, &t.id, -2).unwrap(); // now 4 days past
        let items = attention::check_attention(&conn).unwrap();
        let overdue: Vec<_> = items
            .iter()
            .filter(|i| i.kind == "tray.overdue_harvest")
            .collect();
        assert_eq!(overdue.len(), 1);
        assert!(overdue[0].message.contains("6 trays"));
        assert!(overdue[0].message.contains("4 days"));
        let id = overdue[0].id.clone();

        trays::harvest_trays(&mut conn, std::slice::from_ref(&t.id), 60.0).unwrap();
        assert_eq!(trays::get_tray(&conn, &t.id).unwrap().state, "harvested");

        // R1 — this card used to survive the harvest. A card whose condition is
        // gone cannot be satisfied, so it is now resolved on the same evaluation
        // pass that raises it. The row is closed, not deleted.
        let after = attention::check_attention(&conn).unwrap();
        assert!(after.iter().all(|i| i.kind != "tray.overdue_harvest"));
        let resolved: String = conn
            .query_row(
                "SELECT resolved_by FROM attention WHERE id = ?1 AND resolved_at IS NOT NULL",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved, "condition_cleared");
    }

    /// R1 — the light half. Move to light is the verb the card names; taking it
    /// must retire the card without a Dismiss.
    #[test]
    fn overdue_light_card_resolves_when_the_trays_move_to_light() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let today = db::local_date_today();
        trays::test_shift_dates(&mut conn, &t.id, -6).unwrap(); // 3 days past cover check

        let items = attention::check_attention(&conn).unwrap();
        let id = items
            .iter()
            .find(|i| i.kind == "tray.overdue_light")
            .expect("the card is raised")
            .id
            .clone();

        trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
        assert!(
            trays::overdue_light_groups(&conn, &today, trays::COVER_CARD_DAYS_PAST)
                .unwrap()
                .is_empty()
        );

        let after = attention::check_attention(&conn).unwrap();
        assert!(after.iter().all(|i| i.kind != "tray.overdue_light"));
        let resolved: String = conn
            .query_row(
                "SELECT resolved_by FROM attention WHERE id = ?1 AND resolved_at IS NOT NULL",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved, "condition_cleared");
    }

    /// R1 — per crop, not globally. One crop clearing must not silence another,
    /// and one crop still raising must not keep a cleared crop's card alive.
    #[test]
    fn tray_resolvers_are_per_crop() {
        let mut conn = mem();
        let peas = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let mix = trays::sow_tray(&mut conn, "spicy-mix", 4).unwrap();
        trays::test_shift_dates(&mut conn, &peas.id, -6).unwrap();
        trays::test_shift_dates(&mut conn, &mix.id, -6).unwrap();

        let open = attention::check_attention(&conn).unwrap();
        assert_eq!(
            open.iter()
                .filter(|i| i.kind == "tray.overdue_light")
                .count(),
            2,
            "one card per crop"
        );

        // Only the peas move.
        trays::advance_trays(&mut conn, std::slice::from_ref(&peas.id)).unwrap();
        let after = attention::check_attention(&conn).unwrap();
        let light: Vec<_> = after
            .iter()
            .filter(|i| i.kind == "tray.overdue_light")
            .collect();
        assert_eq!(
            light.len(),
            1,
            "the mix card stays: its trays are still under cover"
        );
        assert!(light[0].message.contains("spicy") || !light[0].message.contains("Dun peas"));
    }

    /// RB1 / F1-09 — the card waits two days; Health F2 raises on day one.
    /// Before this fence Today showed the card and Health's Farm scope read
    /// "None raising" until the third day. Health is now never quieter.
    #[test]
    fn overdue_light_card_waits_two_days_f2_raises_on_day_one() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let today = db::local_date_today();
        // dun-peas: blackout_days = 3, growth_days = 9. Two days past the cover
        // check is still quiet for the card — its threshold is "more than 2".
        trays::test_shift_dates(&mut conn, &t.id, -5).unwrap();
        assert!(
            trays::overdue_light_groups(&conn, &today, trays::COVER_CARD_DAYS_PAST)
                .unwrap()
                .is_empty()
        );
        assert!(attention::check_attention(&conn)
            .unwrap()
            .iter()
            .all(|i| i.kind != "tray.overdue_light"));
        let light_day_one = trays::overdue_light_trays(&conn, &today).unwrap();
        assert_eq!(light_day_one, 6);
        let now = db::utc_now_rfc3339();
        let f2_day_one = health::severity_for(
            "F2",
            None,
            &now,
            &health::CheckInputs {
                overdue_light_trays: light_day_one,
                ..health::CheckInputs::default()
            },
        );
        assert_eq!(f2_day_one.severity, health::Severity::Degraded);
        // Three days past.
        trays::test_shift_dates(&mut conn, &t.id, -1).unwrap();
        let groups =
            trays::overdue_light_groups(&conn, &today, trays::COVER_CARD_DAYS_PAST).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].trays, 6);
        assert_eq!(groups[0].days_past, 3);
        // The card is raised from those rows, byte-for-byte as before the fence.
        let items = attention::check_attention(&conn).unwrap();
        let card = items
            .iter()
            .find(|i| i.kind == "tray.overdue_light")
            .expect("Today raises the overdue-light card");
        assert_eq!(
            card.message,
            "6 trays of Dun peas have been under cover 3 days longer than expected."
        );
        // F1 is honestly clean here: growth_days = 9, nothing is past harvest.
        // That is why F1 could not close this hole.
        assert_eq!(trays::overdue_trays_on_shelf(&conn, &today).unwrap(), 0);
        let light = trays::overdue_light_trays(&conn, &today).unwrap();
        assert_eq!(light, 6);
        let statuses = vec![
            health::severity_for("F1", None, &now, &health::CheckInputs::default()),
            health::severity_for(
                "F2",
                None,
                &now,
                &health::CheckInputs {
                    overdue_light_trays: light,
                    ..health::CheckInputs::default()
                },
            ),
        ];
        assert!(health::cover_invariant_holds(light, &statuses));
        // Moving the trays to light clears the condition F2 reads.
        trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
        assert_eq!(trays::overdue_light_trays(&conn, &today).unwrap(), 0);
        let cleared = health::severity_for(
            "F2",
            None,
            &now,
            &health::CheckInputs {
                overdue_light_trays: trays::overdue_light_trays(&conn, &today).unwrap(),
                ..health::CheckInputs::default()
            },
        );
        assert_eq!(cleared.severity, health::Severity::Healthy);
    }

    #[test]
    fn dismiss_attention_one_event_undo_reopens() {
        let mut conn = mem();
        attention::raise(
            &conn,
            "farm.restored",
            Some("farm"),
            Some("test-restore"),
            "The farm was restored from a backup taken Monday at 6:30 pm.",
            &["dismiss"],
        )
        .unwrap();
        let items = attention::check_attention(&conn).unwrap();
        assert_eq!(items.len(), 1);
        let id = items[0].id.clone();
        let before = trays::count_event_log(&conn).unwrap();

        attention::dismiss_attention(&mut conn, &id).unwrap();
        assert_eq!(trays::count_event_log(&conn).unwrap(), before + 1);
        assert_eq!(
            trays::count_event_kind(&conn, "attention.resolved").unwrap(),
            1
        );
        assert!(attention::check_attention(&conn)
            .unwrap()
            .iter()
            .all(|i| i.id != id));

        trays::undo_last(&mut conn).unwrap();
        let reopened = attention::check_attention(&conn).unwrap();
        assert!(reopened.iter().any(|i| i.id == id));
    }

    /// Pass 14 — inverse layer must not mutate attention (legacy reopen_attention op).
    #[test]
    fn pass14_inverse_reopen_attention_is_inert_without_row() {
        let mut conn = mem();
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM attention", [], |r| r.get(0))
            .unwrap();
        let tx = conn.transaction().unwrap();
        events::apply_inverse(
            &tx,
            r#"{"op":"reopen_attention","attentionId":"missing-row-id"}"#,
            "2026-08-07T12:00:00.000Z",
        )
        .unwrap();
        tx.commit().unwrap();
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM attention", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            before, after,
            "inverse must not create or alter attention rows"
        );
        let open = attention::check_attention(&conn).unwrap();
        assert!(
            open.iter().all(|i| i.id != "missing-row-id"),
            "missing attention id must not appear as open"
        );
    }

    /// Pass 14 — newly emitted attention.resolved stores {"op":"none"}.
    #[test]
    fn pass14_new_attention_resolved_inverse_is_op_none() {
        let mut conn = mem();
        attention::raise(
            &conn,
            "farm.restored",
            Some("farm"),
            Some("test-restore"),
            "restored",
            &["dismiss"],
        )
        .unwrap();
        let id = attention::check_attention(&conn).unwrap()[0].id.clone();
        attention::dismiss_attention(&mut conn, &id).unwrap();
        let inverse: String = conn
            .query_row(
                "SELECT inverse FROM event_log
                 WHERE kind = 'attention.resolved' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&inverse).unwrap();
        assert_eq!(v, serde_json::json!({ "op": "none" }));
    }

    /// Pass 14 — handler-path reopen_attention Errs when the row is missing.
    #[test]
    fn pass14_reopen_attention_errs_on_missing_row() {
        let mut conn = mem();
        let tx = conn.transaction().unwrap();
        let err = attention::reopen_attention(&tx, "no-such-attention").unwrap_err();
        assert!(
            err.contains("failed to reopen attention"),
            "expected Err on missing row, got: {err}"
        );
    }

    #[test]
    fn resolve_harvest_now_returns_ids_without_harvesting() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
        trays::test_shift_dates(&mut conn, &t.id, -13).unwrap();
        let items = attention::check_attention(&conn).unwrap();
        let item = items
            .iter()
            .find(|i| i.kind == "tray.overdue_harvest")
            .unwrap();

        let result = attention::resolve_attention(&mut conn, &item.id, "harvest_now").unwrap();
        assert_eq!(result.tray_ids, vec![t.id.clone()]);
        assert_eq!(trays::get_tray(&conn, &t.id).unwrap().state, "light");
        assert!(trays::get_tray(&conn, &t.id)
            .unwrap()
            .harvested_on
            .is_none());
    }

    #[test]
    fn check_attention_open_only_oldest_first() {
        let mut conn = mem();
        attention::raise(
            &conn,
            "farm.restored",
            Some("farm"),
            Some("a"),
            "First item.",
            &["dismiss"],
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        attention::raise(
            &conn,
            "farm.restored",
            Some("farm"),
            Some("b"),
            "Second item.",
            &["dismiss"],
        )
        .unwrap();

        let open = attention::check_attention(&conn).unwrap();
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].message, "First item.");
        assert_eq!(open[1].message, "Second item.");

        attention::dismiss_attention(&mut conn, &open[0].id).unwrap();
        let after = attention::check_attention(&conn).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].message, "Second item.");
        assert!(!after.iter().any(|i| i.message == "First item."));
    }

    #[test]
    fn recount_raises_surplus_and_shortfall_attention() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        trays::sow_tray(&mut conn, "kale", 1).unwrap();

        trays::apply_recount(
            &mut conn,
            &[
                crate::models::RecountEntry {
                    crop_id: "dun-peas".into(),
                    counted_quantity: 2,
                },
                crate::models::RecountEntry {
                    crop_id: "kale".into(),
                    counted_quantity: 3,
                },
            ],
        )
        .unwrap();

        let items = attention::check_attention(&conn).unwrap();
        let kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        assert!(kinds.contains(&"recount.shortfall"));
        assert!(kinds.contains(&"recount.surplus"));
        let short = items
            .iter()
            .find(|i| i.kind == "recount.shortfall")
            .unwrap();
        let sur = items.iter().find(|i| i.kind == "recount.surplus").unwrap();
        assert!(short.message.contains("Dun peas"));
        assert!(!short.message.contains("estimated sow date"));
        assert!(sur.message.contains("Kale"));
        assert!(sur.message.contains("estimated sow date"));
    }

    // --- Stage 4 Prompt 1: money engine (no network) ---

    fn seed_offer_price(
        conn: &Connection,
        harvest_date: &str,
        crop_id: &str,
        price_id: &str,
        price_cents: i64,
    ) {
        let id = format!("offer_{price_id}");
        conn.execute(
            "INSERT INTO offers
             (id, harvest_date, crop_id, price_cents, stripe_price_id,
              stripe_link_id, stripe_link_url, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6)
             ON CONFLICT(harvest_date, crop_id) DO UPDATE SET
               stripe_price_id = excluded.stripe_price_id,
               price_cents = excluded.price_cents",
            params![
                id,
                harvest_date,
                crop_id,
                price_cents,
                price_id,
                "2026-08-05T12:00:00.000Z"
            ],
        )
        .unwrap();
    }

    fn paid_session(
        conn: &Connection,
        session_id: &str,
        harvest_date: &str,
        crop_id: &str,
        qty: i64,
    ) -> money::PaidSession {
        paid_session_at(conn, session_id, harvest_date, crop_id, qty, 1_700_000_000)
    }

    fn paid_session_at(
        conn: &Connection,
        session_id: &str,
        harvest_date: &str,
        crop_id: &str,
        qty: i64,
        created: i64,
    ) -> money::PaidSession {
        // Stable per crop/date so multiple sessions in one poll share one offer price.
        let price_id = format!("price_{harvest_date}_{crop_id}");
        seed_offer_price(conn, harvest_date, crop_id, &price_id, 1200);
        money::PaidSession {
            session_id: session_id.into(),
            payment_intent: Some(format!("pi_{session_id}")),
            lines: vec![money::PaidLine {
                price_id,
                quantity: qty,
                amount_cents: qty * 1200,
            }],
            currency: "cad".into(),
            customer_email: Some("buyer@example.com".into()),
            paid_at: "2026-08-05T12:00:00.000Z".into(),
            created,
            amount_cents: qty * 1200,
            client_reference: None,
            payment_link: None,
        }
    }

    fn paid_session_raw(
        session_id: &str,
        lines: Vec<money::PaidLine>,
        created: i64,
    ) -> money::PaidSession {
        let amount_cents = lines.iter().map(|l| l.amount_cents).sum();
        money::PaidSession {
            session_id: session_id.into(),
            payment_intent: Some(format!("pi_{session_id}")),
            lines,
            currency: "cad".into(),
            customer_email: Some("buyer@example.com".into()),
            paid_at: "2026-08-05T12:00:00.000Z".into(),
            created,
            amount_cents,
            client_reference: None,
            payment_link: None,
        }
    }

    fn harvest_date_for(conn: &Connection, tray_id: &str) -> String {
        trays::get_tray(conn, tray_id)
            .unwrap()
            .expected_harvest_date
            .expect("expected harvest date")
    }

    fn future_harvest_date() -> String {
        (Local::now().date_naive() + Duration::days(14))
            .format("%Y-%m-%d")
            .to_string()
    }

    fn past_harvest_date() -> String {
        (Local::now().date_naive() - Duration::days(2))
            .format("%Y-%m-%d")
            .to_string()
    }

    #[test]
    fn apply_paid_session_idempotent_same_session_id() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session(&conn, "cs_test_1", &hd, "dun-peas", 2);

        let first = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            first,
            crate::models::AppliedOutcome::Applied { .. }
        ));
        let events_after_first = trays::count_event_kind(&conn, "stripe.session_paid").unwrap();
        assert_eq!(events_after_first, 1);

        let snapshot: Vec<u8> = {
            // Capture order row + event count as the "otherwise unchanged" baseline.
            let orders = money::list_orders(&conn, None).unwrap();
            assert_eq!(orders.len(), 1);
            serde_json::to_vec(&orders).unwrap()
        };

        let second = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            second,
            crate::models::AppliedOutcome::AlreadyApplied
        ));
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            1
        );
        let after = serde_json::to_vec(&money::list_orders(&conn, None).unwrap()).unwrap();
        assert_eq!(snapshot, after);
    }

    #[test]
    fn apply_paid_session_unique_rejection_path_one_order() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session(&conn, "cs_race", &hd, "dun-peas", 1);

        // Drive both applications through the same connection; UNIQUE on
        // stripe_session_id is the only gate — no pre-check.
        let a = money::apply_paid_session(&mut conn, &session).unwrap();
        let b = money::apply_paid_session(&mut conn, &session).unwrap();
        let applied = matches!(a, crate::models::AppliedOutcome::Applied { .. }) as i32
            + matches!(b, crate::models::AppliedOutcome::Applied { .. }) as i32;
        let already = matches!(a, crate::models::AppliedOutcome::AlreadyApplied) as i32
            + matches!(b, crate::models::AppliedOutcome::AlreadyApplied) as i32;
        assert_eq!(applied, 1);
        assert_eq!(already, 1);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
    }

    #[test]
    fn oversell_commits_order_and_attention_one_transaction() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session(&conn, "cs_over", &hd, "dun-peas", 2);

        money::apply_paid_session(&mut conn, &session).unwrap();
        let orders = money::list_orders(&conn, Some(&hd)).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].state, "paid");
        assert_eq!(orders[0].capacity_consumed, 2);

        // The guarantee this test is named for: the oversold row committed in the same
        // transaction as the order. Read it straight from the table, deliberately WITHOUT
        // filtering resolved_at — B2 retires this row on the next evaluation and that
        // retirement is no part of the atomicity claim. Same idiom as
        // event_write_failure_rolls_back_order_and_attention below.
        let oversold_key = format!("{hd}|dun-peas");
        let (message, actions): (String, String) = conn
            .query_row(
                "SELECT message, actions FROM attention
                 WHERE kind = 'order.oversold' AND entity_id = ?1",
                [&oversold_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(message.contains("oversold by 1 tray"));
        assert!(actions.contains("dismiss"));

        // B2 handoff: the operator still sees the condition, now as the standing card.
        let items = attention::check_attention(&conn).unwrap();
        let short_key = format!("{hd}|dun-peas");
        let short = items
            .iter()
            .find(|i| {
                i.kind == "money.capacity_short"
                    && i.entity_id.as_deref() == Some(short_key.as_str())
            })
            .expect("standing money.capacity_short card for the oversold date");
        assert!(short
            .message
            .contains("Sow 1 tray of Dun peas today to cover"));
        assert!(short.actions.iter().any(|a| a == "dismiss"));
        assert!(
            !items.iter().any(|i| i.kind == "order.oversold"),
            "one condition, one card"
        );
        let retired: String = conn
            .query_row(
                "SELECT resolved_by FROM attention
                 WHERE kind = 'order.oversold' AND entity_id = ?1",
                [&oversold_key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(retired, "superseded_by_money_capacity_short");

        let cap = trays::capacity_by_harvest_date(&conn).unwrap();
        let row = cap
            .iter()
            .find(|r| r.harvest_date == hd && r.crop_id == "dun-peas")
            .unwrap();
        assert_eq!(row.trays, 1);
        assert_eq!(row.sold_trays, 2);
        assert_eq!(row.remaining_trays, -1);
    }

    #[test]
    fn event_write_failure_rolls_back_order_and_attention() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let hd = harvest_date_for(&conn, &t.id);

        conn.execute_batch(
            "CREATE TRIGGER fail_stripe_event BEFORE INSERT ON event_log
             WHEN NEW.kind = 'stripe.session_paid'
             BEGIN SELECT RAISE(ABORT, 'forced event failure'); END;",
        )
        .unwrap();

        let session = paid_session(&conn, "cs_fail", &hd, "dun-peas", 2);
        let err = money::apply_paid_session(&mut conn, &session).unwrap_err();
        assert!(err.contains("forced") || !err.is_empty());

        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 0);
        let open: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attention WHERE resolved_at IS NULL AND kind = 'order.oversold'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open, 0);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            0
        );
    }

    #[test]
    fn refund_before_and_after_harvest_capacity_effects() {
        let mut conn = mem();
        let future = future_harvest_date();
        let past = past_harvest_date();

        let future_session = paid_session(&conn, "cs_future", &future, "dun-peas", 2);
        let past_session = paid_session(&conn, "cs_past", &past, "dun-peas", 2);
        money::apply_paid_session(&mut conn, &future_session).unwrap();
        money::apply_paid_session(&mut conn, &past_session).unwrap();

        // Before harvest → capacity released.
        money::apply_refund(
            &mut conn,
            &money::RefundRecord {
                refund_id: "re_future".into(),
                payment_intent: Some("pi_cs_future".into()),
                session_id: Some("cs_future".into()),
                created: 1_700_000_100,
                amount_cents: Some(2400),
                currency: Some("cad".into()),
                status: Some("succeeded".into()),
            },
        )
        .unwrap();
        let future_order = money::list_orders(&conn, Some(&future)).unwrap();
        assert_eq!(future_order[0].state, "refunded");
        assert_eq!(future_order[0].capacity_consumed, 0);
        let items = attention::check_attention(&conn).unwrap();
        assert!(items
            .iter()
            .any(|i| i.kind == "order.refunded" && i.message.contains("available again")));

        // After / on harvest date → capacity retained.
        money::apply_refund(
            &mut conn,
            &money::RefundRecord {
                refund_id: "re_past".into(),
                payment_intent: Some("pi_cs_past".into()),
                session_id: Some("cs_past".into()),
                created: 1_700_000_101,
                amount_cents: Some(2400),
                currency: Some("cad".into()),
                status: Some("succeeded".into()),
            },
        )
        .unwrap();
        let past_order = money::list_orders(&conn, Some(&past)).unwrap();
        assert_eq!(past_order[0].state, "refunded");
        assert_eq!(past_order[0].capacity_consumed, 2);
        let items = attention::check_attention(&conn).unwrap();
        assert!(items
            .iter()
            .any(|i| i.kind == "order.refunded_after_harvest"));
    }

    #[test]
    fn dispute_never_releases_capacity() {
        let mut conn = mem();
        let hd = future_harvest_date();
        let session = paid_session(&conn, "cs_disp", &hd, "dun-peas", 2);
        money::apply_paid_session(&mut conn, &session).unwrap();

        money::apply_dispute(
            &mut conn,
            &money::DisputeRecord {
                dispute_id: "dp_1".into(),
                payment_intent: Some("pi_cs_disp".into()),
                session_id: Some("cs_disp".into()),
                created: 1_700_000_200,
                amount_cents: None,
                currency: None,
                status: None,
            },
        )
        .unwrap();

        let order = &money::list_orders(&conn, Some(&hd)).unwrap()[0];
        assert_eq!(order.state, "disputed");
        assert_eq!(order.capacity_consumed, 2);
        let items = attention::check_attention(&conn).unwrap();
        let d = items.iter().find(|i| i.kind == "order.disputed").unwrap();
        assert!(d.message.contains("disputed"));
        assert!(d.actions.iter().any(|a| a == "open_in_stripe"));
        // Capacity retained on dispute.
        assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), -2);
    }

    #[test]
    fn refund_and_dispute_idempotent() {
        let mut conn = mem();
        let hd = future_harvest_date();
        let session = paid_session(&conn, "cs_idemp", &hd, "mellow-mix", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();

        let refund = money::RefundRecord {
            refund_id: "re_1".into(),
            payment_intent: Some("pi_cs_idemp".into()),
            session_id: Some("cs_idemp".into()),
            created: 1_700_000_300,
            amount_cents: Some(1200),
            currency: Some("cad".into()),
            status: Some("succeeded".into()),
        };
        money::apply_refund(&mut conn, &refund).unwrap();
        let after_first = serde_json::to_vec(&money::list_orders(&conn, None).unwrap()).unwrap();
        let attn_first = attention::check_attention(&conn).unwrap().len();
        money::apply_refund(&mut conn, &refund).unwrap();
        assert_eq!(
            serde_json::to_vec(&money::list_orders(&conn, None).unwrap()).unwrap(),
            after_first
        );
        assert_eq!(attention::check_attention(&conn).unwrap().len(), attn_first);

        // Fresh paid order for dispute path.
        let session2 = paid_session(&conn, "cs_idemp2", &hd, "mellow-mix", 1);
        money::apply_paid_session(&mut conn, &session2).unwrap();
        let dispute = money::DisputeRecord {
            dispute_id: "dp_1".into(),
            payment_intent: Some("pi_cs_idemp2".into()),
            session_id: Some("cs_idemp2".into()),
            created: 1_700_000_301,
            amount_cents: None,
            currency: None,
            status: None,
        };
        money::apply_dispute(&mut conn, &dispute).unwrap();
        let after_d = serde_json::to_vec(&money::list_orders(&conn, None).unwrap()).unwrap();
        let attn_d = attention::check_attention(&conn).unwrap().len();
        money::apply_dispute(&mut conn, &dispute).unwrap();
        assert_eq!(
            serde_json::to_vec(&money::list_orders(&conn, None).unwrap()).unwrap(),
            after_d
        );
        assert_eq!(attention::check_attention(&conn).unwrap().len(), attn_d);
    }

    #[test]
    fn undo_last_skips_stripe_events_leaves_order() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);

        let session = paid_session(&conn, "cs_undo", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);

        let undone = trays::undo_last(&mut conn).unwrap().expect("undo sow");
        assert_eq!(undone.undone_kind, "tray.sown");
        assert!(trays::get_tray(&conn, &t.id).is_err());
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            1
        );
    }

    #[test]
    fn capacity_subtracts_sold_and_is_computed() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 5).unwrap();
        let hd = harvest_date_for(&conn, &t.id);

        let before = trays::capacity_by_harvest_date(&conn).unwrap();
        let row = before
            .iter()
            .find(|r| r.harvest_date == hd && r.crop_id == "dun-peas")
            .unwrap();
        assert_eq!(row.trays, 5);
        assert_eq!(row.sold_trays, 0);
        assert_eq!(row.remaining_trays, 5);

        let session = paid_session(&conn, "cs_cap", &hd, "dun-peas", 3);
        money::apply_paid_session(&mut conn, &session).unwrap();
        let after = trays::capacity_by_harvest_date(&conn).unwrap();
        let row = after
            .iter()
            .find(|r| r.harvest_date == hd && r.crop_id == "dun-peas")
            .unwrap();
        assert_eq!(row.trays, 5);
        assert_eq!(row.sold_trays, 3);
        assert_eq!(row.remaining_trays, 2);

        // No capacity column anywhere — values come from the join each call.
        let cols: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('trays') WHERE name LIKE '%capacity%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cols, 0);
    }

    /// Audit S8. Three sown for D, a venue claims all three, a stale shop page
    /// still lists them, a buyer pays. Before B5 the retail door subtracted
    /// retail only, saw 3 - 1 >= 0, and said nothing at all.
    #[test]
    fn b5_retail_oversell_sees_the_wholesale_claim() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let v =
            crate::marketing::record_venue(&mut conn, "Verdant", "cafe", None, None, None, None)
                .unwrap();
        crate::wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &hd,
            vec![crate::wholesale::OrderLine {
                crop_id: "dun-peas".into(),
                trays: 3,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            0,
            "the venue has claimed every tray, but none is oversold yet"
        );
        let session = paid_session(&conn, "cs_b5_s8", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            -1,
            "one formula: the shop sold a tray the venue already owns"
        );
        let oversold_key = format!("{hd}|dun-peas");
        let msg: String = conn
            .query_row(
                "SELECT message FROM attention
                 WHERE kind = 'order.oversold' AND entity_id = ?1 AND resolved_at IS NULL",
                [&oversold_key],
                |r| r.get(0),
            )
            .expect("order.oversold must raise when wholesale claimed the trays");
        assert!(msg.contains("oversold by 1 tray"), "{msg}");
    }

    /// Every door, one number. The point of B5 is not that the retail door got
    /// stricter; it is that no door can answer this question differently.
    #[test]
    fn b5_every_door_reports_the_same_remaining() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let v =
            crate::marketing::record_venue(&mut conn, "Verdant", "cafe", None, None, None, None)
                .unwrap();
        crate::wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &hd,
            vec![crate::wholesale::OrderLine {
                crop_id: "dun-peas".into(),
                trays: 3,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
        let session = paid_session(&conn, "cs_b5_agree", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();
        let one = trays::remaining_for_date(&conn, &hd).unwrap();
        assert_eq!(one, -1);
        let row = trays::capacity_by_harvest_date(&conn)
            .unwrap()
            .into_iter()
            .find(|r| r.harvest_date == hd && r.crop_id == "dun-peas")
            .unwrap();
        assert_eq!(row.remaining_trays, one, "capacity view");
        let plan = crate::reachability::cover_plan(&conn).unwrap();
        let c = plan
            .iter()
            .find(|c| c.harvest_date == hd)
            .expect("COVER must see the same shortfall");
        assert_eq!(c.short_trays, -one, "COVER card");
        assert_eq!(c.reachability.remaining_trays, one, "reachability view");
        // capacity_sight() takes no Connection (retired Farm OS snapshot
        // reader). capacity_gate already calls capacity_by_harvest_date
        // directly, so it is on the one formula by construction.
    }

    /// The netting compares a claim's paid_at against `updated_at` on a
    /// harvested tray. That only means "the harvest instant" because a harvested
    /// tray is terminal -- nothing writes to the row afterwards. Pin it, so a
    /// future write to a harvested row fails here instead of silently making
    /// the capacity formula lie.
    #[test]
    fn b5t0_updated_at_on_a_harvested_tray_is_the_harvest_instant() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let before = trays::get_tray(&conn, &t.id).unwrap().updated_at;
        trays::advance_tray(&mut conn, &t.id).unwrap();
        trays::harvest_tray(&mut conn, &t.id, 10.0).unwrap();
        let after = trays::get_tray(&conn, &t.id).unwrap();
        assert_eq!(after.state, "harvested");
        assert!(
            after.updated_at >= before,
            "harvest must advance updated_at: {before} -> {}",
            after.updated_at
        );
        assert!(
            after.harvested_on.is_some(),
            "a harvested row carries its harvest date"
        );
    }

    /// Helper: the harvest instant for a date, straight off the harvested rows.
    fn last_harvest_at(conn: &Connection, hd: &str) -> String {
        conn.query_row(
            "SELECT MAX(t.updated_at) FROM trays t
              WHERE t.state = 'harvested'
                AND date(t.sown_on, '+' || t.growth_days_at_sow || ' days') = ?1",
            [hd],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Helper: force a date's retail claims to a chosen paid_at, so a test can
    /// state the ordering it is testing instead of depending on clock skew.
    fn set_claim_time(conn: &Connection, hd: &str, paid_at: &str) {
        conn.execute(
            "UPDATE orders SET paid_at = ?1 WHERE harvest_date = ?2",
            rusqlite::params![paid_at, hd],
        )
        .unwrap();
    }

    /// The hole 21de3ad opened: harvest everything, sell the stock elsewhere,
    /// then a stale shop page takes one more. Those trays were already gone and
    /// cannot reach backwards to cover it.
    #[test]
    fn b5t1_a_claim_made_after_the_harvest_is_not_absorbed() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        trays::advance_tray(&mut conn, &t.id).unwrap();
        trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();
        let harvest_at = last_harvest_at(&conn, &hd);
        let session = paid_session(&conn, "cs_b5t_after", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();
        // The sale happens after the shelf was cleared.
        set_claim_time(&conn, &hd, "2099-01-01T00:00:00Z");
        let _ = harvest_at;
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            -1,
            "a buyer paid for a tray that does not exist -- this must not read 0"
        );
        let plan = crate::reachability::cover_plan(&conn).unwrap();
        assert!(
            plan.iter().any(|c| c.harvest_date == hd),
            "COVER must see it: {plan:?}"
        );
    }

    /// The designed path 21de3ad got right must survive: a claim that existed
    /// when the trays left is still discharged by them.
    #[test]
    fn b5t2_a_claim_made_before_the_harvest_is_still_discharged() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session(&conn, "cs_b5t_before", &hd, "dun-peas", 3);
        money::apply_paid_session(&mut conn, &session).unwrap();
        set_claim_time(&conn, &hd, "2000-01-01T00:00:00Z");
        assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), 0);
        trays::advance_tray(&mut conn, &t.id).unwrap();
        trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            0,
            "the trays left the shelf and the promise left with them"
        );
        assert!(crate::reachability::cover_plan(&conn).unwrap().is_empty());
    }

    /// Both kinds of claim on one date. The harvest discharges the one that
    /// predates it and leaves the later one standing.
    #[test]
    fn b5t3_the_harvest_discharges_only_what_predates_it() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let early = paid_session(&conn, "cs_b5t_early", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &early).unwrap();
        set_claim_time(&conn, &hd, "2000-01-01T00:00:00Z");
        trays::advance_tray(&mut conn, &t.id).unwrap();
        trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();
        let late = paid_session(&conn, "cs_b5t_late", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &late).unwrap();
        conn.execute(
            "UPDATE orders SET paid_at = ?1 WHERE stripe_session_id = ?2",
            rusqlite::params!["2099-01-01T00:00:00Z", "cs_b5t_late"],
        )
        .unwrap();
        let row = trays::capacity_by_harvest_date(&conn)
            .unwrap()
            .into_iter()
            .find(|r| r.harvest_date == hd && r.crop_id == "dun-peas")
            .unwrap();
        assert_eq!(row.harvested_trays, 3);
        assert_eq!(
            row.sold_trays, 1,
            "the early claim is discharged, the late one stands"
        );
        assert_eq!(row.remaining_trays, -1);
    }

    /// The inclusive boundary. A claim paid at the EXACT instant of the harvest
    /// counts as having existed when the trays left the shelf, so it discharges;
    /// a second later it does not, because by then those trays were gone.
    ///
    /// Until this test that rule lived only in a comment in the SQL. Every other
    /// test sits days clear of the line — b5c1/b5c2/b5c3 on the eligible side,
    /// b5t1/b5t3 at 2099 on the other — so all of them would pass unchanged if
    /// the comparison were "always discharge" or "never". Walking one claim
    /// across the instant is what distinguishes `<=` from either.
    #[test]
    fn b5t4_a_claim_paid_at_the_harvest_instant_still_discharges() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        trays::advance_tray(&mut conn, &t.id).unwrap();
        trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();

        // The instant the formula actually compares against.
        let harvest_at = last_harvest_at(&conn, &hd);

        let session = paid_session(&conn, "cs_b5t_boundary", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();

        // Exactly on the line: the promise existed as the trays came off.
        set_claim_time(&conn, &hd, &harvest_at);
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            0,
            "inclusive: a claim paid at the harvest instant is discharged by it"
        );

        // One second past the line: the same claim, trays already gone.
        let after = (chrono::DateTime::parse_from_rfc3339(&harvest_at)
            .expect("the harvest instant must be RFC3339")
            + chrono::Duration::seconds(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        set_claim_time(&conn, &hd, &after);
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            -1,
            "one second later, trays that already left cannot cover it"
        );
    }

    /// The lie this completion kills: sell the trays, hand them over, harvest,
    /// and the claim used to reserve trays that no longer existed -- forever,
    /// with no honest exit and no venue to call.
    #[test]
    fn b5c1_retail_claim_retires_on_harvest_evidence() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);

        let session = paid_session(&conn, "cs_b5c_retire", &hd, "dun-peas", 3);
        money::apply_paid_session(&mut conn, &session).unwrap();
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            0,
            "claim is live: three grown, three sold"
        );

        trays::advance_tray(&mut conn, &t.id).unwrap();
        trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();

        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            0,
            "the trays left the shelf and the claim left with them -- NOT -3"
        );
        let plan = crate::reachability::cover_plan(&conn).unwrap();
        assert!(
            !plan.iter().any(|c| c.harvest_date == hd),
            "no COVER card for a date that was honestly fulfilled: {plan:?}"
        );
    }

    /// The guard on the rule's shape. A boolean "evidence exists" would wipe all
    /// three claims on the strength of one harvested tray and report +2
    /// remaining while three buyers are still owed trays.
    #[test]
    fn b5c2_partial_harvest_retires_only_what_left_the_shelf() {
        let mut conn = mem();
        let one = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let two = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        let hd = harvest_date_for(&conn, &one.id);
        assert_eq!(harvest_date_for(&conn, &two.id), hd, "same date fixture");

        let session = paid_session(&conn, "cs_b5c_partial", &hd, "dun-peas", 3);
        money::apply_paid_session(&mut conn, &session).unwrap();
        assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), 0);

        // Harvest ONE of the three. Two buyers are still owed trays.
        trays::advance_tray(&mut conn, &one.id).unwrap();
        trays::harvest_tray(&mut conn, &one.id, 10.0).unwrap();

        let row = trays::capacity_by_harvest_date(&conn)
            .unwrap()
            .into_iter()
            .find(|r| r.harvest_date == hd && r.crop_id == "dun-peas")
            .unwrap();
        assert_eq!(row.trays, 2, "two still growing");
        assert_eq!(row.harvested_trays, 1);
        assert_eq!(row.sold_trays, 2, "one claim discharged, two still live");
        assert_eq!(
            row.remaining_trays, 0,
            "two owed against two growing -- never +2"
        );
    }

    /// The channels retire on different signals by design, and must not bleed.
    #[test]
    fn b5c3_harvest_retires_retail_but_not_the_open_wholesale_promise() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let v =
            crate::marketing::record_venue(&mut conn, "Verdant", "cafe", None, None, None, None)
                .unwrap();
        crate::wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &hd,
            vec![crate::wholesale::OrderLine {
                crop_id: "dun-peas".into(),
                trays: 2,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
        let session = paid_session(&conn, "cs_b5c_cross", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();
        assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), 0);

        trays::advance_tray(&mut conn, &t.id).unwrap();
        trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();

        // D2(b): harvest evidence now discharges wholesale as well as retail.
        // The old claim ("the venue's two are still owed until delivered")
        // was GT-D14's delivered-retires rule; delivery is not evidence.
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            0,
            "retail and wholesale both retire on harvest evidence — NOT -2"
        );
        let row = trays::capacity_by_harvest_date(&conn)
            .unwrap()
            .into_iter()
            .find(|r| r.harvest_date == hd && r.crop_id == "dun-peas")
            .unwrap();
        assert_eq!(row.sold_trays, 0);
        assert_eq!(row.cover_remaining, 0);
    }

    /// Coverage for the first branch that shipped untested in original B5:
    /// has_open_order_on returns false, so oversold falls back to the venue.
    #[test]
    fn b5c4_oversold_on_an_unreachable_date_says_call_the_venue() {
        let mut conn = mem();
        let hd = (chrono::Local::now().date_naive() + chrono::Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let session = paid_session(&conn, "cs_b5c_unreach", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();

        assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), -1);
        let oversold_key = format!("{hd}|dun-peas");
        let msg: String = conn
            .query_row(
                "SELECT message FROM attention
                 WHERE kind = 'order.oversold' AND entity_id = ?1 AND resolved_at IS NULL",
                [&oversold_key],
                |r| r.get(0),
            )
            .unwrap();
        assert!(msg.contains("oversold by 1 tray"), "{msg}");
        assert!(msg.contains("cannot be fixed by sowing"), "{msg}");
        assert!(msg.contains("call the venue"), "{msg}");
        assert!(!msg.contains("deliver or void"), "{msg}");
    }

    /// Coverage for the second untested branch: has_open_order_on returns true
    /// and harvest evidence exists, so oversold points at the delivery.
    #[test]
    fn b5c5_oversold_today_with_stock_in_hand_says_deliver() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let growth = t.growth_days_at_sow.expect("growth days");
        trays::dev_backdate_tray(&mut conn, &t.id, growth).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        assert_eq!(
            hd,
            crate::db::local_date_today(),
            "fixture must mature today (check backdate sign)"
        );
        trays::advance_tray(&mut conn, &t.id).unwrap();
        trays::harvest_tray(&mut conn, &t.id, 30.0).unwrap();

        let v =
            crate::marketing::record_venue(&mut conn, "Verdant", "cafe", None, None, None, None)
                .unwrap();
        crate::wholesale::record_order(
            &mut conn,
            &v.venue_id,
            &hd,
            vec![crate::wholesale::OrderLine {
                crop_id: "dun-peas".into(),
                trays: 3,
                price_cents_per_tray: Some(900),
            }],
            false,
        )
        .unwrap();
        // D2(b): harvest evidence discharges wholesale. Three harvested
        // trays cover the venue's three; the extra retail tray is the
        // oversold that still points at delivery (stock in hand, open order).
        let session = paid_session(&conn, "cs_b5c_deliver", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();

        let oversold_key = format!("{hd}|dun-peas");
        let msg: String = conn
            .query_row(
                "SELECT message FROM attention
                 WHERE kind = 'order.oversold' AND entity_id = ?1 AND resolved_at IS NULL",
                [&oversold_key],
                |r| r.get(0),
            )
            .unwrap();
        assert!(msg.contains("cannot be fixed by sowing"), "{msg}");
        assert!(msg.contains("deliver or void the open order"), "{msg}");
        assert!(!msg.contains("call the venue"), "{msg}");
    }

    #[test]
    fn fake_gateway_lists_programmable_sessions() {
        use money::fake::FakeGateway;
        use money::StripeGateway;

        let gw = FakeGateway::new().with_account(money::AccountInfo {
            account_id: "acct_test".into(),
            account_name: "Farm OS Test".into(),
            mode: "test".into(),
        });
        let session = paid_session_raw(
            "cs_gw",
            vec![money::PaidLine {
                price_id: "price_cs_gw".into(),
                quantity: 1,
                amount_cents: 1200,
            }],
            1_700_000_000,
        );
        gw.push_session(session.clone());
        gw.push_session(session); // duplicate id
        assert_eq!(gw.account().unwrap().mode, "test");
        let sessions = gw.list_paid_sessions(None).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "cs_gw");
    }

    #[test]
    fn migration_v2_to_v3_no_row_loss_no_orders() {
        let conn = db::open_v2_in_memory().unwrap();
        let tray = trays::get_tray(&conn, db::FIXTURE_V2_TRAY_ID).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);

        db::migrate(&conn).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, db::SCHEMA_VERSION);

        let still = trays::get_tray(&conn, &tray.id).unwrap();
        assert_eq!(still.quantity, 2);
        assert_eq!(still.crop_id, "kale");

        let orders: i64 = conn
            .query_row("SELECT COUNT(*) FROM orders", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orders, 0);

        for table in ["stripe_config", "offers", "orders", "stripe_cursor"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {table}");
        }

        let status = money::money_status(&conn).unwrap();
        assert!(!status.configured);
        assert_eq!(status.open_order_count, 0);
    }

    // --- Stage 4 Prompt 2: Stripe key + gateway ---

    /// KEY-AT-REST: the column holds a sealed, tagged value; these tests
    /// compare the opened plaintext, and `stored_key_raw` pins the tag.
    fn stored_key(conn: &Connection) -> Option<String> {
        key_at_rest::open(stored_key_raw(conn).as_deref()).value()
    }

    fn stored_key_raw(conn: &Connection) -> Option<String> {
        conn.query_row(
            "SELECT restricted_key FROM stripe_config WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .ok()
        .flatten()
    }

    #[test]
    #[allow(clippy::assertions_on_constants)] // H-17: tripwire, not a tautology.
                                              // Fails loudly if ALLOW_LIVE_KEYS is ever flipped back. Same class as c13's pin.
    fn rk_live_accepted_while_allow_live_keys_true() {
        assert!(money::ALLOW_LIVE_KEYS);
        let conn = mem();
        assert_eq!(
            money::validate_restricted_key("rk_live_abc123").unwrap(),
            "live"
        );
        money::store_stripe_key(
            &conn,
            "rk_live_abc123",
            &money::AccountInfo {
                account_id: "acct_x".into(),
                account_name: "Live Farm".into(),
                mode: "live".into(),
            },
        )
        .unwrap();
        assert_eq!(stored_key(&conn).as_deref(), Some("rk_live_abc123"));
        // The key's mode still has to match the account it came from.
        let conn2 = mem();
        let mismatch = money::store_stripe_key(
            &conn2,
            "rk_live_abc123",
            &money::AccountInfo {
                account_id: "acct_y".into(),
                account_name: "Wrong Mode".into(),
                mode: "test".into(),
            },
        )
        .unwrap_err();
        assert!(mismatch.contains("did not match"), "{mismatch}");
        assert!(stored_key(&conn2).is_none() || stored_key(&conn2).as_deref() == Some(""));
    }

    #[test]
    fn secret_keys_refused() {
        for key in ["sk_test_abc", "sk_live_abc"] {
            let err = money::validate_restricted_key(key).unwrap_err();
            assert!(err.contains("secret key"), "{key}: {err}");
            assert!(err.contains("restricted key"), "{key}: {err}");
        }
    }

    #[test]
    fn rk_test_accepted_and_stored() {
        let conn = mem();
        let key = "rk_test_farm_os_unit_test_key_001";
        assert_eq!(money::validate_restricted_key(key).unwrap(), "test");
        money::store_stripe_key(
            &conn,
            key,
            &money::AccountInfo {
                account_id: "acct_test_1".into(),
                account_name: "Farm OS Test".into(),
                mode: "test".into(),
            },
        )
        .unwrap();
        assert_eq!(stored_key(&conn).as_deref(), Some(key));
        let raw = stored_key_raw(&conn).expect("a stored value");
        assert!(raw.starts_with(key_at_rest::TAG), "{raw}");
        #[cfg(windows)]
        assert!(!raw.contains(key), "the column never holds the plaintext");
        let status = money::money_status(&conn).unwrap();
        assert!(status.configured);
        assert_eq!(status.mode.as_deref(), Some("test"));
        assert_eq!(status.account_name.as_deref(), Some("Farm OS Test"));
    }

    #[test]
    fn different_account_id_refused_raises_attention() {
        let conn = mem();
        money::store_stripe_key(
            &conn,
            "rk_test_first",
            &money::AccountInfo {
                account_id: "acct_original".into(),
                account_name: "Original".into(),
                mode: "test".into(),
            },
        )
        .unwrap();

        let err = money::store_stripe_key(
            &conn,
            "rk_test_second",
            &money::AccountInfo {
                account_id: "acct_other".into(),
                account_name: "Other".into(),
                mode: "test".into(),
            },
        )
        .unwrap_err();
        assert!(err.contains("different Stripe account"));
        assert_eq!(stored_key(&conn).as_deref(), Some("rk_test_first"));

        let items = attention::check_attention(&conn).unwrap();
        assert!(items.iter().any(|i| i.kind == "stripe.account_mismatch"));
    }

    #[test]
    fn gateway_errors_are_readable_err_not_panic() {
        use crate::stripe_client::fake_http::FakeHttp;
        use crate::stripe_client::StripeClient;
        use money::StripeGateway;

        let http = FakeHttp::new();
        http.push_get_err("/v1/checkout/sessions", "connection reset by peer");
        let client = StripeClient::new(http, "test");
        let err = client.list_paid_sessions(None).unwrap_err();
        assert!(!err.is_empty());
        assert!(err.contains("connection reset") || err.contains("peer"));
    }

    #[test]
    fn gateway_pagination_returns_every_record_from_both_pages() {
        use crate::stripe_client::fake_http::FakeHttp;
        use crate::stripe_client::StripeClient;
        use money::StripeGateway;
        use serde_json::json;

        let http = FakeHttp::new();
        http.push_get(
            "/v1/checkout/sessions",
            json!({
                "object": "list",
                "has_more": true,
                "data": [{
                    "id": "cs_page1",
                    "object": "checkout.session",
                    "payment_status": "paid",
                    "amount_total": 1200,
                    "currency": "cad",
                    "created": 1_700_000_000,
                    "payment_intent": "pi_1"
                }]
            }),
        );
        http.push_get(
            "/v1/checkout/sessions/cs_page1/line_items",
            json!({
                "object": "list",
                "has_more": false,
                "data": [{
                    "id": "li_1",
                    "quantity": 1,
                    "amount_total": 1200,
                    "price": { "id": "price_p1", "unit_amount": 1200 }
                }]
            }),
        );
        http.push_get(
            "/v1/checkout/sessions",
            json!({
                "object": "list",
                "has_more": false,
                "data": [
                    {
                        "id": "cs_page2a",
                        "object": "checkout.session",
                        "payment_status": "paid",
                        "amount_total": 2400,
                        "currency": "cad",
                        "created": 1_700_000_100,
                        "payment_intent": "pi_2"
                    },
                    {
                        "id": "cs_unpaid",
                        "object": "checkout.session",
                        "payment_status": "unpaid",
                        "amount_total": 1200,
                        "currency": "cad",
                        "created": 1_700_000_200
                    }
                ]
            }),
        );
        http.push_get(
            "/v1/checkout/sessions/cs_page2a/line_items",
            json!({
                "object": "list",
                "has_more": false,
                "data": [{
                    "id": "li_2",
                    "quantity": 2,
                    "amount_total": 2400,
                    "price": { "id": "price_p2", "unit_amount": 1200 }
                }]
            }),
        );

        let client = StripeClient::new(http, "test");
        let sessions = client.list_paid_sessions(None).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "cs_page1");
        assert_eq!(sessions[0].lines[0].quantity, 1);
        assert_eq!(sessions[1].session_id, "cs_page2a");
        assert_eq!(sessions[1].lines[0].quantity, 2);
    }

    #[test]
    fn stored_key_never_appears_in_errors_or_redaction() {
        let key = "rk_test_super_secret_should_not_leak";
        let leaked = format!("auth failed for {key} buyer@example.com");
        let cleaned = crate::stripe_client::redact_secrets(&leaked, key);
        assert!(!cleaned.contains(key));
        assert!(!cleaned.contains("buyer@example.com"));
        assert!(cleaned.contains("[redacted]"));

        let conn = mem();
        money::store_stripe_key(
            &conn,
            key,
            &money::AccountInfo {
                account_id: "acct_keep".into(),
                account_name: "Keep".into(),
                mode: "test".into(),
            },
        )
        .unwrap();
        let err = money::store_stripe_key(
            &conn,
            "rk_test_other_key_value",
            &money::AccountInfo {
                account_id: "acct_swap".into(),
                account_name: "Swap".into(),
                mode: "test".into(),
            },
        )
        .unwrap_err();
        assert!(!err.contains(key));
        assert!(!err.contains("rk_test_other_key_value"));
    }

    // --- Stage 4 Prompt 3: offers + shop page ---

    fn seed_stripe_config(conn: &Connection) {
        money::store_stripe_key(
            conn,
            "rk_test_shop_page_seed_key_do_not_leak",
            &money::AccountInfo {
                account_id: "acct_shop".into(),
                account_name: "Shop Test".into(),
                mode: "test".into(),
            },
        )
        .unwrap();
        money::set_checkout_endpoint_url(conn, "https://checkout.example.com/").unwrap();
    }

    #[test]
    fn set_offer_creates_one_price_and_link_updates_not_duplicates() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();

        let first = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();
        assert!(first.id.is_some());
        assert!(first.price_cents.is_some());
        {
            let st = gw.state.lock().unwrap();
            assert_eq!(st.prices_created.len(), 1);
            assert_eq!(st.prices_created[0].price_cents, 1200);
            assert_eq!(st.prices_created[0].crop_id, "dun-peas");
            assert_eq!(st.prices_created[0].harvest_date, hd);
        }

        let second = offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1500).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.price_cents, Some(1500));
        {
            let st = gw.state.lock().unwrap();
            assert_eq!(st.prices_created.len(), 2);
        }
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM offers WHERE harvest_date = ?1 AND crop_id = 'dun-peas'",
                [&hd],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn set_offer_link_carries_harvest_crop_and_offer_id_metadata() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "mellow-mix", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        let view = offers::set_offer_with(&mut conn, &gw, &hd, "mellow-mix", 900).unwrap();
        let st = gw.state.lock().unwrap();
        let offer = &st.prices_created[0];
        assert_eq!(offer.harvest_date, hd);
        assert_eq!(offer.crop_id, "mellow-mix");
        assert_eq!(Some(offer.id.clone()), view.id);
        assert!(!offer.id.is_empty());
    }

    #[test]
    fn generate_shop_page_under_100kb() {
        let mut conn = mem();
        seed_stripe_config(&conn);
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = money::fake::FakeGateway::new();
        offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1100).unwrap();

        let dir = temp_farm_dir();
        let page = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap();
        assert!(page.size_bytes < 100 * 1024, "size {}", page.size_bytes);
        assert!(std::path::Path::new(&page.file_path).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_html_has_no_key_or_customer_data() {
        let mut conn = mem();
        let secret = "rk_test_shop_page_seed_key_do_not_leak";
        seed_stripe_config(&conn);
        let t = trays::sow_tray(&mut conn, "kale", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = money::fake::FakeGateway::new();
        offers::set_offer_with(&mut conn, &gw, &hd, "kale", 800).unwrap();

        let dir = temp_farm_dir();
        let page = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap();
        let html = fs::read_to_string(&page.file_path).unwrap();
        assert!(!html.contains(secret));
        assert!(!html.to_ascii_lowercase().contains("rk_"));
        assert!(!html.to_ascii_lowercase().contains("sk_"));
        assert!(!html.contains("@"));
        assert!(!html.contains("customer"));
        assert!(!html.contains("quantity="));
        assert!(!html.contains("buy.stripe.com"));
        // Inline script only — no third-party script origins.
        assert!(!html.contains("http://"));
        assert!(
            !html.contains("https://cdn")
                && !html.contains("googleapis")
                && !html.contains("googletag")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shop_page_lists_only_remaining_capacity_gt_zero() {
        let mut conn = mem();
        seed_stripe_config(&conn);
        let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = money::fake::FakeGateway::new();
        offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1000).unwrap();

        // Sell out via a paid order consuming the only tray.
        let sold = paid_session(&conn, "cs_sold_out", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &sold).unwrap();

        let listings = offers::shop_listings(&conn).unwrap();
        assert!(listings.is_empty());

        let dir = temp_farm_dir();
        let page = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap();
        let html = fs::read_to_string(&page.file_path).unwrap();
        assert!(html.contains("Nothing available") || !html.contains("Dun peas"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shop_page_availability_matches_capacity_snapshot() {
        let mut conn = mem();
        seed_stripe_config(&conn);
        let t = trays::sow_tray(&mut conn, "dun-peas", 5).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = money::fake::FakeGateway::new();
        offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1000).unwrap();
        let sold = paid_session(&conn, "cs_cap_snap", &hd, "dun-peas", 2);
        money::apply_paid_session(&mut conn, &sold).unwrap();

        let cap = trays::capacity_by_harvest_date(&conn).unwrap();
        let row = cap
            .iter()
            .find(|r| r.harvest_date == hd && r.crop_id == "dun-peas")
            .unwrap();
        assert_eq!(row.trays, 5);
        assert_eq!(row.sold_trays, 2);
        assert_eq!(row.remaining_trays, 3);

        let listings = offers::shop_listings(&conn).unwrap();
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].available, 5);
        assert_eq!(listings[0].sold, 2);
        assert_eq!(listings[0].remaining, 3);

        let dir = temp_farm_dir();
        let page = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap();
        let html = fs::read_to_string(&page.file_path).unwrap();
        assert!(html.contains("data-available=\"5\""));
        assert!(html.contains("data-sold=\"2\""));
        assert!(html.contains("data-remaining=\"3\""));
        assert_eq!(listings[0].remaining, row.remaining_trays);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_offer_deactivates_link_and_drops_from_next_page() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "broccoli", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        let view = offers::set_offer_with(&mut conn, &gw, &hd, "broccoli", 700).unwrap();
        let id = view.id.unwrap();

        assert_eq!(offers::shop_listings(&conn).unwrap().len(), 1);
        offers::remove_offer_with(&mut conn, &gw, &id).unwrap();
        assert!(offers::shop_listings(&conn).unwrap().is_empty());

        let dir = temp_farm_dir();
        seed_stripe_config(&conn);
        let page = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap();
        let html = fs::read_to_string(&page.file_path).unwrap();
        assert!(!html.contains("Broccoli"));
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Stage 4 Prompt 4: poll loop ---

    fn cursor_sessions(conn: &Connection) -> Option<String> {
        poll::sessions_since(conn).unwrap()
    }

    fn last_poll_err(conn: &Connection) -> Option<String> {
        conn.query_row(
            "SELECT last_poll_err FROM stripe_cursor WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn poll_fail_count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COALESCE(poll_fail_count, 0) FROM stripe_cursor WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn poll_three_sessions_creates_orders_advances_cursor_once_after_commit() {
        use money::fake::FakeGateway;
        use money::StripeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        gw.push_session(paid_session_at(&conn, "cs_p1", &hd, "dun-peas", 1, 100));
        gw.push_session(paid_session_at(&conn, "cs_p2", &hd, "dun-peas", 1, 101));
        gw.push_session(paid_session_at(&conn, "cs_p3", &hd, "dun-peas", 1, 102));

        assert!(cursor_sessions(&conn).is_none());
        let result = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(result.ok);
        assert_eq!(result.sessions_applied, 3);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 3);
        assert_eq!(cursor_sessions(&conn).as_deref(), Some("102"));
        // Gateway listed once; cursor advanced after the page committed.
        let _ = gw.list_paid_sessions(None);
    }

    #[test]
    fn poll_fail_midway_leaves_cursor_rerun_no_duplicates() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        gw.push_session(paid_session_at(&conn, "cs_m1", &hd, "dun-peas", 1, 200));
        gw.push_session(paid_session_at(&conn, "cs_m2", &hd, "dun-peas", 1, 201));
        gw.push_session(paid_session_at(&conn, "cs_m3", &hd, "dun-peas", 1, 202));

        // Fail after the first session is handled — cursor stays at that record.
        conn.execute_batch(
            "CREATE TRIGGER fail_second_paid BEFORE INSERT ON event_log
             WHEN NEW.kind = 'stripe.session_paid'
               AND (SELECT COUNT(*) FROM event_log WHERE kind = 'stripe.session_paid') >= 1
             BEGIN SELECT RAISE(ABORT, 'midway network drop'); END;",
        )
        .unwrap();

        let result = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(!result.ok);
        assert_eq!(cursor_sessions(&conn).as_deref(), Some("200"));
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);

        conn.execute_batch("DROP TRIGGER fail_second_paid;")
            .unwrap();

        let result = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(result.ok);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 3);
        assert_eq!(cursor_sessions(&conn).as_deref(), Some("202"));
        // Re-poll creates nothing extra.
        let again = poll::run_poll(&mut conn, &gw).unwrap();
        assert_eq!(again.sessions_applied, 0);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 3);
    }

    #[test]
    fn poll_replay_already_applied_page_creates_nothing_no_attention() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        let page = vec![
            paid_session_at(&conn, "cs_r1", &hd, "dun-peas", 1, 300),
            paid_session_at(&conn, "cs_r2", &hd, "dun-peas", 1, 301),
        ];
        {
            let mut st = gw.state.lock().unwrap();
            st.session_pages = vec![money::SessionPage::from_parsed(page.clone())];
        }

        assert!(poll::run_poll(&mut conn, &gw).unwrap().ok);
        // Reset cursor as if the page were re-delivered without advancing.
        conn.execute(
            "UPDATE stripe_cursor SET sessions_since = NULL WHERE id = 1",
            [],
        )
        .unwrap();
        let attn_before = attention::check_attention(&conn).unwrap().len();
        let result = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(result.ok);
        assert_eq!(result.sessions_applied, 0);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 2);
        assert_eq!(
            attention::check_attention(&conn).unwrap().len(),
            attn_before
        );
    }

    #[test]
    fn poll_three_failures_one_item_fourth_no_second_success_clears_err() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let gw = FakeGateway::new();
        gw.fail_sessions("network down");

        for _ in 0..3 {
            let r = poll::run_poll(&mut conn, &gw).unwrap();
            assert!(!r.ok);
        }
        assert_eq!(poll_fail_count(&conn), 3);
        let items = attention::check_attention(&conn).unwrap();
        let failed: Vec<_> = items.iter().filter(|i| i.kind == "poll.failed").collect();
        assert_eq!(failed.len(), 1);
        assert!(failed[0]
            .message
            .contains("hasn't been able to reach Stripe"));
        assert!(failed[0].actions.iter().any(|a| a == "try_now"));

        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(!r.ok);
        let items = attention::check_attention(&conn).unwrap();
        assert_eq!(items.iter().filter(|i| i.kind == "poll.failed").count(), 1);

        gw.clear_session_fail();
        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        assert!(last_poll_err(&conn).is_none());
        assert_eq!(poll_fail_count(&conn), 0);
    }

    #[test]
    fn poll_oversell_order_and_attention_one_transaction() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        gw.push_session(paid_session_at(&conn, "cs_ov", &hd, "dun-peas", 2, 400));

        assert!(poll::run_poll(&mut conn, &gw).unwrap().ok);
        let orders = money::list_orders(&conn, Some(&hd)).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].capacity_consumed, 2);

        // Written in the poll's own transaction. Read directly, unfiltered by resolved_at
        // (see the note in oversell_commits_order_and_attention_one_transaction).
        let oversold_key = format!("{hd}|dun-peas");
        let raised: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attention
                 WHERE kind = 'order.oversold' AND entity_id = ?1",
                [&oversold_key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raised, 1);

        // B2 handoff: standing card carries it, the one-shot does not linger.
        let items = attention::check_attention(&conn).unwrap();
        let short_key = format!("{hd}|dun-peas");
        assert!(items.iter().any(|i| {
            i.kind == "money.capacity_short" && i.entity_id.as_deref() == Some(short_key.as_str())
        }));
        assert!(!items.iter().any(|i| i.kind == "order.oversold"));
    }

    #[test]
    fn poll_refund_before_session_awaiting_then_applies() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        gw.push_refund(money::RefundRecord {
            refund_id: "re_early".into(),
            payment_intent: Some("pi_cs_late".into()),
            session_id: Some("cs_late".into()),
            created: 500,
            amount_cents: Some(1200),
            currency: Some("cad".into()),
            status: Some("succeeded".into()),
        });

        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        assert_eq!(r.refunds_applied, 0);
        let refunds_since: Option<String> = conn
            .query_row(
                "SELECT refunds_since FROM stripe_cursor WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(refunds_since.is_none());

        gw.push_session(paid_session_at(&conn, "cs_late", &hd, "dun-peas", 1, 501));
        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        assert_eq!(r.sessions_applied, 1);
        assert_eq!(r.refunds_applied, 1);
        let order = &money::list_orders(&conn, None).unwrap()[0];
        assert_eq!(order.state, "refunded");
        assert_eq!(order.capacity_consumed, 0);
    }

    #[test]
    fn poll_pagination_two_pages_applies_all_before_cursor_moves_past() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 8).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        {
            let mut st = gw.state.lock().unwrap();
            st.session_pages = vec![
                money::SessionPage::from_parsed(vec![
                    paid_session_at(&conn, "cs_a1", &hd, "dun-peas", 1, 600),
                    paid_session_at(&conn, "cs_a2", &hd, "dun-peas", 1, 601),
                ]),
                money::SessionPage::from_parsed(vec![
                    paid_session_at(&conn, "cs_a3", &hd, "dun-peas", 1, 602),
                    paid_session_at(&conn, "cs_a4", &hd, "dun-peas", 1, 603),
                ]),
            ];
        }

        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        assert_eq!(r.sessions_applied, 4);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 4);
        assert_eq!(cursor_sessions(&conn).as_deref(), Some("603"));
    }

    #[test]
    fn poll_hard_kill_between_apply_and_cursor_safe_replay() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let page = vec![
            paid_session_at(&conn, "cs_hk1", &hd, "dun-peas", 1, 700),
            paid_session_at(&conn, "cs_hk2", &hd, "dun-peas", 1, 701),
        ];
        let gw = FakeGateway::new();
        {
            let mut st = gw.state.lock().unwrap();
            st.session_pages = vec![money::SessionPage::from_parsed(page.clone())];
        }

        // Simulate crash after applies, before cursor advance.
        poll::apply_sessions_page_no_cursor(&mut conn, &page).unwrap();
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 2);
        assert!(cursor_sessions(&conn).is_none());

        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        assert_eq!(r.sessions_applied, 0);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 2);
        assert_eq!(cursor_sessions(&conn).as_deref(), Some("701"));
    }

    // --- Stage 4 P0: unique narrowing, unparsed sessions, cursor walk ---

    fn open_attention_kind(conn: &Connection, kind: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM attention WHERE resolved_at IS NULL AND kind = ?1",
            [kind],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn unknown_price_id_rejected_not_already_applied() {
        let mut conn = mem();
        let session = paid_session_raw(
            "cs_bad_price",
            vec![money::PaidLine {
                price_id: "price_unknown".into(),
                quantity: 1,
                amount_cents: 1200,
            }],
            800,
        );

        let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            outcome,
            crate::models::AppliedOutcome::Rejected { .. }
        ));
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 0);
        assert_eq!(open_attention_kind(&conn, "stripe.unrecognised_session"), 1);
        let item = attention::check_attention(&conn)
            .unwrap()
            .into_iter()
            .find(|i| i.kind == "stripe.unrecognised_session")
            .unwrap();
        assert_eq!(item.entity_type.as_deref(), Some("stripe_session"));
        assert_eq!(item.entity_id.as_deref(), Some("cs_bad_price"));
        assert!(item.message.contains("couldn't match"));
        assert!(item.actions.iter().any(|a| a == "open_in_stripe"));
    }

    #[test]
    fn unknown_price_id_twice_one_attention_no_order() {
        let mut conn = mem();
        let session = paid_session_raw(
            "cs_bad_price2",
            vec![money::PaidLine {
                price_id: "price_unknown2".into(),
                quantity: 1,
                amount_cents: 1200,
            }],
            801,
        );

        let _ = money::apply_paid_session(&mut conn, &session).unwrap();
        let _ = money::apply_paid_session(&mut conn, &session).unwrap();
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 0);
        assert_eq!(open_attention_kind(&conn, "stripe.unrecognised_session"), 1);
    }

    #[test]
    fn duplicate_session_id_still_already_applied_no_attention() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session_at(&conn, "cs_dup_ok", &hd, "dun-peas", 1, 802);

        let first = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            first,
            crate::models::AppliedOutcome::Applied { .. }
        ));
        let attn_before = attention::check_attention(&conn).unwrap().len();
        let second = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            second,
            crate::models::AppliedOutcome::AlreadyApplied
        ));
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
        assert_eq!(
            attention::check_attention(&conn).unwrap().len(),
            attn_before
        );
        assert_eq!(open_attention_kind(&conn, "order.unrecorded"), 0);
    }

    #[test]
    fn is_already_applied_violation_narrow() {
        use rusqlite::ffi::{Error, SQLITE_CONSTRAINT_FOREIGNKEY, SQLITE_CONSTRAINT_UNIQUE};
        use rusqlite::ErrorCode;

        let session_unique = rusqlite::Error::SqliteFailure(
            Error {
                code: ErrorCode::ConstraintViolation,
                extended_code: SQLITE_CONSTRAINT_UNIQUE,
            },
            Some("UNIQUE constraint failed: orders.stripe_session_id, orders.crop_id".into()),
        );
        assert!(money::is_already_applied_violation(&session_unique));

        let ref_unique = rusqlite::Error::SqliteFailure(
            Error {
                code: ErrorCode::ConstraintViolation,
                extended_code: SQLITE_CONSTRAINT_UNIQUE,
            },
            Some("UNIQUE constraint failed: orders.client_reference, orders.crop_id".into()),
        );
        assert!(money::is_already_applied_violation(&ref_unique));

        let other_unique = rusqlite::Error::SqliteFailure(
            Error {
                code: ErrorCode::ConstraintViolation,
                extended_code: SQLITE_CONSTRAINT_UNIQUE,
            },
            Some("UNIQUE constraint failed: offers.harvest_date, offers.crop_id".into()),
        );
        assert!(!money::is_already_applied_violation(&other_unique));

        let fk = rusqlite::Error::SqliteFailure(
            Error {
                code: ErrorCode::ConstraintViolation,
                extended_code: SQLITE_CONSTRAINT_FOREIGNKEY,
            },
            Some("FOREIGN KEY constraint failed".into()),
        );
        assert!(!money::is_already_applied_violation(&fk));
    }

    #[test]
    fn unparsed_session_surfaces_attention_and_order_for_parsed() {
        use crate::stripe_client::fake_http::FakeHttp;
        use crate::stripe_client::StripeClient;
        use money::fake::FakeGateway;
        use money::StripeGateway;
        use serde_json::json;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);

        seed_offer_price(&conn, &hd, "dun-peas", "price_good", 1200);

        let http = FakeHttp::new();
        http.push_get(
            "/v1/checkout/sessions",
            json!({
                "object": "list",
                "has_more": false,
                "data": [
                    {
                        "id": "cs_good",
                        "object": "checkout.session",
                        "payment_status": "paid",
                        "amount_total": 1200,
                        "currency": "cad",
                        "created": 900,
                        "payment_intent": "pi_good"
                    },
                    {
                        "id": "cs_bad_meta",
                        "object": "checkout.session",
                        "payment_status": "paid",
                        "amount_total": 2400,
                        "currency": "cad",
                        "created": 901,
                        "payment_intent": "pi_bad"
                    }
                ]
            }),
        );
        http.push_get(
            "/v1/checkout/sessions/cs_good/line_items",
            json!({
                "object": "list",
                "has_more": false,
                "data": [{
                    "id": "li_good",
                    "quantity": 1,
                    "amount_total": 1200,
                    "price": { "id": "price_good", "unit_amount": 1200 }
                }]
            }),
        );
        // Missing price id → unparsed.
        http.push_get(
            "/v1/checkout/sessions/cs_bad_meta/line_items",
            json!({
                "object": "list",
                "has_more": false,
                "data": [{
                    "id": "li_bad",
                    "quantity": 1,
                    "amount_total": 2400
                }]
            }),
        );

        let client = StripeClient::new(http, "test");
        let pages = client.list_paid_session_pages(None).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].parsed.len(), 1);
        assert_eq!(pages[0].unparsed.len(), 1);
        assert_eq!(pages[0].parsed[0].session_id, "cs_good");
        assert_eq!(pages[0].unparsed[0].session_id, "cs_bad_meta");

        let gw = FakeGateway::new();
        {
            let mut st = gw.state.lock().unwrap();
            st.session_pages = pages;
        }
        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
        assert_eq!(open_attention_kind(&conn, "stripe.unrecognised_session"), 1);
        let item = attention::check_attention(&conn)
            .unwrap()
            .into_iter()
            .find(|i| i.kind == "stripe.unrecognised_session")
            .unwrap();
        assert!(item.message.contains("couldn't match"));
        assert_eq!(item.entity_id.as_deref(), Some("cs_bad_meta"));
    }

    #[test]
    fn unparsed_session_poll_twice_one_attention() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let gw = FakeGateway::new();
        {
            let mut st = gw.state.lock().unwrap();
            st.session_pages = vec![money::SessionPage {
                parsed: vec![],
                unparsed: vec![money::UnparsedSession {
                    session_id: "cs_once".into(),
                    created: 910,
                    amount_cents: 1200,
                    currency: "cad".into(),
                    reason: "session missing crop_id metadata".into(),
                }],
            }];
        }

        assert!(poll::run_poll(&mut conn, &gw).unwrap().ok);
        assert!(poll::run_poll(&mut conn, &gw).unwrap().ok);
        assert_eq!(open_attention_kind(&conn, "stripe.unrecognised_session"), 1);
    }

    #[test]
    fn poll_three_pages_newest_first_applies_all_cursor_at_max() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        let s_new = paid_session_at(&conn, "cs_new", &hd, "dun-peas", 1, 1030);
        let s_mid = paid_session_at(&conn, "cs_mid", &hd, "dun-peas", 1, 1020);
        let s_old = paid_session_at(&conn, "cs_old", &hd, "dun-peas", 1, 1010);
        {
            let mut st = gw.state.lock().unwrap();
            // Newest-first page order from Stripe.
            st.session_pages = vec![
                money::SessionPage::from_parsed(vec![s_new]),
                money::SessionPage::from_parsed(vec![s_mid]),
                money::SessionPage::from_parsed(vec![s_old]),
            ];
        }

        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        assert_eq!(r.sessions_applied, 3);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 3);
        assert_eq!(cursor_sessions(&conn).as_deref(), Some("1030"));
    }

    #[test]
    fn poll_stranding_regression_older_sessions_still_apply() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();

        // Apply only the oldest page, park cursor at its max — then a full poll
        // must still apply every remaining session with no duplicates.
        let s_old = paid_session_at(&conn, "cs_old_page", &hd, "dun-peas", 1, 1110);
        money::apply_paid_session(&mut conn, &s_old).unwrap();
        conn.execute(
            "UPDATE stripe_cursor SET sessions_since = ?1 WHERE id = 1",
            ["1110"],
        )
        .unwrap();

        let s_new = paid_session_at(&conn, "cs_new_page", &hd, "dun-peas", 1, 1130);
        let s_mid = paid_session_at(&conn, "cs_mid_page", &hd, "dun-peas", 1, 1120);
        {
            let mut st = gw.state.lock().unwrap();
            st.session_pages = vec![
                money::SessionPage::from_parsed(vec![s_new]),
                money::SessionPage::from_parsed(vec![s_mid]),
                money::SessionPage::from_parsed(vec![s_old]),
            ];
        }

        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        let orders = money::list_orders(&conn, None).unwrap();
        assert_eq!(orders.len(), 3);
        let ids: HashSet<_> = orders
            .iter()
            .map(|o| o.stripe_session_id.as_str())
            .collect();
        assert!(ids.contains("cs_old_page"));
        assert!(ids.contains("cs_mid_page"));
        assert!(ids.contains("cs_new_page"));
        assert_eq!(cursor_sessions(&conn).as_deref(), Some("1130"));
    }

    #[test]
    fn poll_same_created_across_two_polls_both_orders() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();

        // First poll delivers only one of two same-second sessions.
        gw.push_session(paid_session_at(
            &conn,
            "cs_same_a",
            &hd,
            "dun-peas",
            1,
            1200,
        ));
        assert!(poll::run_poll(&mut conn, &gw).unwrap().ok);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
        assert_eq!(cursor_sessions(&conn).as_deref(), Some("1200"));

        // Second poll: gte re-fetches the boundary; the sibling in the same second arrives.
        gw.push_session(paid_session_at(
            &conn,
            "cs_same_b",
            &hd,
            "dun-peas",
            1,
            1200,
        ));
        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 2);
        assert_eq!(cursor_sessions(&conn).as_deref(), Some("1200"));
    }

    // --- raise_once: dismissal sticks for immutable Stripe facts ---

    fn attention_rows_for(conn: &Connection, kind: &str, entity_id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM attention WHERE kind = ?1 AND entity_id = ?2",
            params![kind, entity_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn raise_once_order_unrecorded_dismissal_sticks() {
        let mut conn = mem();
        attention::raise_once(
            &conn,
            "order.unrecorded",
            Some("stripe_session"),
            Some("cs_x"),
            "A payment for $12.00 couldn't be recorded: Farm OS doesn't have a crop called \"dun-pea\".",
            &["open_in_stripe", "dismiss"],
        )
        .unwrap();
        let open = attention::check_attention(&conn).unwrap();
        let item = open
            .iter()
            .find(|i| i.kind == "order.unrecorded" && i.entity_id.as_deref() == Some("cs_x"))
            .unwrap();
        let id = item.id.clone();
        attention::dismiss_attention(&mut conn, &id).unwrap();

        attention::raise_once(
            &conn,
            "order.unrecorded",
            Some("stripe_session"),
            Some("cs_x"),
            "A payment for $12.00 couldn't be recorded: Farm OS doesn't have a crop called \"dun-pea\".",
            &["open_in_stripe", "dismiss"],
        )
        .unwrap();

        assert_eq!(attention_rows_for(&conn, "order.unrecorded", "cs_x"), 1);
        let resolved: Option<String> = conn
            .query_row(
                "SELECT resolved_at FROM attention WHERE id = ?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(resolved.is_some());
        assert!(!attention::check_attention(&conn)
            .unwrap()
            .iter()
            .any(|i| i.id == id));
    }

    #[test]
    fn raise_once_unrecognised_session_dismissal_sticks() {
        let mut conn = mem();
        attention::raise_once(
            &conn,
            "stripe.unrecognised_session",
            Some("stripe_session"),
            Some("cs_y"),
            "A payment of $12.00 arrived that Farm OS couldn't match to a crop and harvest date.",
            &["open_in_stripe", "dismiss"],
        )
        .unwrap();
        let id = attention::check_attention(&conn)
            .unwrap()
            .into_iter()
            .find(|i| i.kind == "stripe.unrecognised_session")
            .unwrap()
            .id;
        attention::dismiss_attention(&mut conn, &id).unwrap();

        attention::raise_once(
            &conn,
            "stripe.unrecognised_session",
            Some("stripe_session"),
            Some("cs_y"),
            "A payment of $12.00 arrived that Farm OS couldn't match to a crop and harvest date.",
            &["open_in_stripe", "dismiss"],
        )
        .unwrap();

        assert_eq!(
            attention_rows_for(&conn, "stripe.unrecognised_session", "cs_y"),
            1
        );
        let resolved: Option<String> = conn
            .query_row(
                "SELECT resolved_at FROM attention WHERE id = ?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(resolved.is_some());
        assert!(!attention::check_attention(&conn)
            .unwrap()
            .iter()
            .any(|i| i.kind == "stripe.unrecognised_session"));
    }

    #[test]
    fn poll_unparsed_dismiss_between_cycles_stays_resolved() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let gw = FakeGateway::new();
        {
            let mut st = gw.state.lock().unwrap();
            st.session_pages = vec![money::SessionPage {
                parsed: vec![],
                unparsed: vec![money::UnparsedSession {
                    session_id: "cs_sticky".into(),
                    created: 1300,
                    amount_cents: 1200,
                    currency: "cad".into(),
                    reason: "session missing crop_id metadata".into(),
                }],
            }];
        }

        assert!(poll::run_poll(&mut conn, &gw).unwrap().ok);
        let id = attention::check_attention(&conn)
            .unwrap()
            .into_iter()
            .find(|i| i.kind == "stripe.unrecognised_session")
            .unwrap()
            .id;
        attention::dismiss_attention(&mut conn, &id).unwrap();

        assert!(poll::run_poll(&mut conn, &gw).unwrap().ok);
        assert_eq!(
            attention_rows_for(&conn, "stripe.unrecognised_session", "cs_sticky"),
            1
        );
        let resolved: Option<String> = conn
            .query_row(
                "SELECT resolved_at FROM attention WHERE entity_id = 'cs_sticky'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(resolved.is_some());
        assert_eq!(open_attention_kind(&conn, "stripe.unrecognised_session"), 0);
    }

    #[test]
    fn raise_once_keys_on_kind_and_entity_id() {
        let conn = mem();
        attention::raise_once(
            &conn,
            "order.unrecorded",
            Some("stripe_session"),
            Some("cs_a"),
            "A payment for $12.00 couldn't be recorded: Farm OS doesn't have a crop called \"x\".",
            &["open_in_stripe", "dismiss"],
        )
        .unwrap();
        attention::raise_once(
            &conn,
            "order.unrecorded",
            Some("stripe_session"),
            Some("cs_b"),
            "A payment for $12.00 couldn't be recorded: Farm OS doesn't have a crop called \"y\".",
            &["open_in_stripe", "dismiss"],
        )
        .unwrap();
        assert_eq!(attention_rows_for(&conn, "order.unrecorded", "cs_a"), 1);
        assert_eq!(attention_rows_for(&conn, "order.unrecorded", "cs_b"), 1);
        assert_eq!(
            attention::check_attention(&conn)
                .unwrap()
                .iter()
                .filter(|i| i.kind == "order.unrecorded")
                .count(),
            2
        );
    }

    #[test]
    fn raise_once_does_not_apply_to_recurring_overdue_harvest() {
        // Recurring kind: a dismissal is never permanent silence. Finding 23
        // holds the episode for the day; the card returns when the fact moves.
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap();
        trays::test_shift_dates(&mut conn, &t.id, -13).unwrap();

        let a = attention::check_attention(&conn).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, "tray.overdue_harvest");
        let id1 = a[0].id.clone();
        attention::dismiss_attention(&mut conn, &id1).unwrap();

        let quiet = attention::check_attention(&conn).unwrap();
        assert!(
            quiet.iter().all(|i| i.kind != "tray.overdue_harvest"),
            "dismissed overdue harvest must stay quiet on the same day"
        );

        trays::test_shift_dates(&mut conn, &t.id, -1).unwrap();
        let c = attention::check_attention(&conn).unwrap();
        assert_eq!(c.len(), 1);
        assert_ne!(c[0].id, id1);
        assert_eq!(c[0].kind, "tray.overdue_harvest");
    }

    // --- Stage 4: multi-line carts ---

    #[test]
    fn two_line_session_two_orders_one_event_capacity_down_by_five() {
        let mut conn = mem();
        let peas = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &peas.id);
        let _sun = trays::sow_tray(&mut conn, "sunflower", 4).unwrap();
        seed_offer_price(&conn, &hd, "dun-peas", "price_peas", 1200);
        seed_offer_price(&conn, &hd, "sunflower", "price_sun", 1000);

        let before = trays::remaining_for_date(&conn, &hd).unwrap();
        let session = paid_session_raw(
            "cs_cart",
            vec![
                money::PaidLine {
                    price_id: "price_peas".into(),
                    quantity: 3,
                    amount_cents: 3600,
                },
                money::PaidLine {
                    price_id: "price_sun".into(),
                    quantity: 2,
                    amount_cents: 2000,
                },
            ],
            1_700_000_500,
        );

        let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            outcome,
            crate::models::AppliedOutcome::Applied { .. }
        ));

        let rows: Vec<(String, i64, i64)> = conn
            .prepare(
                "SELECT crop_id, quantity, capacity_consumed FROM orders
                 WHERE stripe_session_id = 'cs_cart' ORDER BY crop_id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![("dun-peas".into(), 3, 3), ("sunflower".into(), 2, 2),]
        );
        eprintln!(
            "SELECT crop_id, quantity, capacity_consumed FROM orders → {:?}",
            rows
        );
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            1
        );
        assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), before - 5);
    }

    #[test]
    fn two_line_session_twice_already_applied_no_second_event() {
        let mut conn = mem();
        let peas = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &peas.id);
        let _sun = trays::sow_tray(&mut conn, "sunflower", 4).unwrap();
        seed_offer_price(&conn, &hd, "dun-peas", "price_peas2", 1200);
        seed_offer_price(&conn, &hd, "sunflower", "price_sun2", 1000);

        let session = paid_session_raw(
            "cs_cart2",
            vec![
                money::PaidLine {
                    price_id: "price_peas2".into(),
                    quantity: 3,
                    amount_cents: 3600,
                },
                money::PaidLine {
                    price_id: "price_sun2".into(),
                    quantity: 2,
                    amount_cents: 2000,
                },
            ],
            1_700_000_501,
        );

        let first = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            first,
            crate::models::AppliedOutcome::Applied { .. }
        ));
        let second = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            second,
            crate::models::AppliedOutcome::AlreadyApplied
        ));
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 2);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            1
        );
    }

    #[test]
    fn session_with_unknown_price_writes_zero_rows() {
        let mut conn = mem();
        let peas = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &peas.id);
        seed_offer_price(&conn, &hd, "dun-peas", "price_known_only", 1200);

        let session = paid_session_raw(
            "cs_partial_bad",
            vec![
                money::PaidLine {
                    price_id: "price_known_only".into(),
                    quantity: 1,
                    amount_cents: 1200,
                },
                money::PaidLine {
                    price_id: "price_not_in_offers".into(),
                    quantity: 2,
                    amount_cents: 2000,
                },
            ],
            1_700_000_502,
        );

        let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            outcome,
            crate::models::AppliedOutcome::Rejected { .. }
        ));
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 0);
        assert_eq!(open_attention_kind(&conn, "stripe.unrecognised_session"), 1);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            0
        );
    }

    #[test]
    fn zero_quantity_line_omitted_other_lines_land() {
        let mut conn = mem();
        let peas = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &peas.id);
        let _sun = trays::sow_tray(&mut conn, "sunflower", 3).unwrap();
        seed_offer_price(&conn, &hd, "dun-peas", "price_z_peas", 1200);
        seed_offer_price(&conn, &hd, "sunflower", "price_z_sun", 1000);

        let session = paid_session_raw(
            "cs_zero_line",
            vec![
                money::PaidLine {
                    price_id: "price_z_peas".into(),
                    quantity: 0,
                    amount_cents: 0,
                },
                money::PaidLine {
                    price_id: "price_z_sun".into(),
                    quantity: 2,
                    amount_cents: 2000,
                },
            ],
            1_700_000_503,
        );

        let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            outcome,
            crate::models::AppliedOutcome::Applied { .. }
        ));
        let orders = money::list_orders(&conn, None).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].crop_id, "sunflower");
        assert_eq!(orders[0].quantity, 2);
    }

    #[test]
    fn refund_two_line_future_releases_full_capacity() {
        let mut conn = mem();
        let hd = future_harvest_date();
        // D3: the pool is per (date, crop). A sunflower sale never consumes
        // pea trays. Amends the date-wide remaining_for_date claim this test
        // used to pin.
        let t = trays::sow_tray(&mut conn, "dun-peas", 10).unwrap();
        // Align sown tray harvest to the future date used by offers/orders.
        conn.execute(
            "UPDATE trays SET sown_on = date(?1, '-' || growth_days_at_sow || ' days')
             WHERE id = ?2",
            params![&hd, &t.id],
        )
        .unwrap();
        seed_offer_price(&conn, &hd, "dun-peas", "price_rf_peas", 1200);
        seed_offer_price(&conn, &hd, "sunflower", "price_rf_sun", 1000);

        let peas_before = trays::remaining_exact_for(&conn, &hd, "dun-peas").unwrap();
        let sun_before = trays::remaining_exact_for(&conn, &hd, "sunflower").unwrap();
        let session = paid_session_raw(
            "cs_rf_cart",
            vec![
                money::PaidLine {
                    price_id: "price_rf_peas".into(),
                    quantity: 3,
                    amount_cents: 3600,
                },
                money::PaidLine {
                    price_id: "price_rf_sun".into(),
                    quantity: 2,
                    amount_cents: 2000,
                },
            ],
            1_700_000_504,
        );
        money::apply_paid_session(&mut conn, &session).unwrap();
        assert_eq!(
            trays::remaining_exact_for(&conn, &hd, "dun-peas").unwrap(),
            peas_before - 3
        );
        assert_eq!(
            trays::remaining_exact_for(&conn, &hd, "sunflower").unwrap(),
            sun_before - 2
        );

        money::apply_refund(
            &mut conn,
            &money::RefundRecord {
                refund_id: "re_cart".into(),
                payment_intent: Some("pi_cs_rf_cart".into()),
                session_id: Some("cs_rf_cart".into()),
                created: 1_700_000_600,
                amount_cents: Some(5600),
                currency: Some("cad".into()),
                status: Some("succeeded".into()),
            },
        )
        .unwrap();

        let orders = money::list_orders(&conn, Some(&hd)).unwrap();
        assert_eq!(orders.len(), 2);
        assert!(orders.iter().all(|o| o.state == "refunded"));
        assert!(orders.iter().all(|o| o.capacity_consumed == 0));
        assert_eq!(
            trays::remaining_exact_for(&conn, &hd, "dun-peas").unwrap(),
            peas_before
        );
        assert_eq!(
            trays::remaining_exact_for(&conn, &hd, "sunflower").unwrap(),
            sun_before
        );
    }

    #[test]
    fn migration_v4_to_v5_preserves_single_line_order() {
        let conn = db::open_v4_in_memory().unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 4);

        conn.execute(
            "INSERT INTO orders
             (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
              quantity, amount_cents, currency, customer_email, state,
              capacity_consumed, paid_at, created_at, updated_at)
             VALUES
             ('ord_dun_survive', 'cs_survive', 'pi_survive', '2026-08-14', 'dun-peas',
              1, 1200, 'cad', 'buyer@example.com', 'paid',
              1, '2026-08-05T12:00:00.000Z', '2026-08-05T12:00:00.000Z',
              '2026-08-05T12:00:00.000Z')",
            [],
        )
        .unwrap();

        db::migrate(&conn).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, db::SCHEMA_VERSION);

        let row: (String, i64, i64, Option<String>) = conn
            .query_row(
                "SELECT crop_id, quantity, capacity_consumed, client_reference FROM orders
                 WHERE stripe_session_id = 'cs_survive'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, "dun-peas");
        assert_eq!(row.1, 1);
        assert_eq!(row.2, 1);
        assert!(row.3.is_none());

        let hl: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='harvest_links'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hl, 1);

        // Composite unique: same session + different crop ok; same crop fails.
        conn.execute(
            "INSERT INTO orders
             (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
              quantity, amount_cents, currency, customer_email, state,
              capacity_consumed, paid_at, created_at, updated_at)
             VALUES
             ('ord_sun', 'cs_survive', 'pi_survive', '2026-08-14', 'sunflower',
              1, 1000, 'cad', NULL, 'paid',
              1, '2026-08-05T12:00:00.000Z', '2026-08-05T12:00:00.000Z',
              '2026-08-05T12:00:00.000Z')",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO orders
             (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
              quantity, amount_cents, currency, customer_email, state,
              capacity_consumed, paid_at, created_at, updated_at)
             VALUES
             ('ord_dup', 'cs_survive', 'pi_survive', '2026-08-14', 'dun-peas',
              1, 1200, 'cad', NULL, 'paid',
              1, '2026-08-05T12:00:00.000Z', '2026-08-05T12:00:00.000Z',
              '2026-08-05T12:00:00.000Z')",
            [],
        );
        assert!(dup.is_err());
        assert!(money::is_already_applied_violation(&dup.unwrap_err()));
    }

    // --- Stage 4 Prompt 1/3: client_reference idempotency ---

    #[test]
    fn same_client_reference_different_sessions_already_applied_once() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let before = trays::remaining_for_date(&conn, &hd).unwrap();

        let mut first = paid_session(&conn, "cs_ref_a", &hd, "dun-peas", 2);
        first.client_reference = Some("cart_abc".into());
        let mut second = paid_session(&conn, "cs_ref_b", &hd, "dun-peas", 2);
        second.client_reference = Some("cart_abc".into());

        let a = money::apply_paid_session(&mut conn, &first).unwrap();
        assert!(matches!(a, crate::models::AppliedOutcome::Applied { .. }));
        let b = money::apply_paid_session(&mut conn, &second).unwrap();
        // J4 REF-DUP-FACT: the second session under a booked reference is
        // named, never booked — Rejected, one duplicate_reference fact keyed
        // by the new session id, and still one order.
        assert!(matches!(b, crate::models::AppliedOutcome::Rejected { .. }));
        let dup_facts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM stripe_unapplied_facts
                 WHERE stripe_object = 'checkout_session'
                   AND stripe_id = 'cs_ref_b'
                   AND status = 'duplicate_reference'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dup_facts, 1);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
        assert_eq!(
            money::list_orders(&conn, None).unwrap()[0]
                .client_reference
                .as_deref(),
            Some("cart_abc")
        );
        assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), before - 2);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            1
        );
    }

    #[test]
    fn null_client_reference_sessions_do_not_collide() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &t.id);

        let a = paid_session(&conn, "cs_null_a", &hd, "dun-peas", 1);
        let b = paid_session(&conn, "cs_null_b", &hd, "dun-peas", 1);
        assert!(a.client_reference.is_none());
        assert!(b.client_reference.is_none());

        assert!(matches!(
            money::apply_paid_session(&mut conn, &a).unwrap(),
            crate::models::AppliedOutcome::Applied { .. }
        ));
        assert!(matches!(
            money::apply_paid_session(&mut conn, &b).unwrap(),
            crate::models::AppliedOutcome::Applied { .. }
        ));
        let orders = money::list_orders(&conn, None).unwrap();
        assert_eq!(orders.len(), 2);
        assert!(orders.iter().all(|o| o.client_reference.is_none()));
    }

    #[test]
    fn foreign_key_failure_still_rejected_order_unrecorded() {
        let mut conn = mem();
        let hd = future_harvest_date();
        seed_offer_price(&conn, &hd, "dun-peas", "price_fk_gone", 1200);

        // Leave the offer pointing at a crop that no longer exists.
        conn.pragma_update(None, "foreign_keys", false).unwrap();
        conn.execute("DELETE FROM crops WHERE id = 'dun-peas'", [])
            .unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        let session = paid_session_raw(
            "cs_fk_fail",
            vec![money::PaidLine {
                price_id: "price_fk_gone".into(),
                quantity: 1,
                amount_cents: 1200,
            }],
            1_700_000_700,
        );
        let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            outcome,
            crate::models::AppliedOutcome::Rejected { .. }
        ));
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 0);
        assert_eq!(open_attention_kind(&conn, "order.unrecorded"), 1);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            0
        );
    }

    #[test]
    fn duplicate_stripe_session_id_still_already_applied() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session(&conn, "cs_dup_sess", &hd, "dun-peas", 1);

        let first = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            first,
            crate::models::AppliedOutcome::Applied { .. }
        ));
        let second = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            second,
            crate::models::AppliedOutcome::AlreadyApplied
        ));
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
    }

    #[test]
    fn migration_v5_to_v6_preserves_orders_client_reference_null() {
        let conn = db::open_v5_in_memory().unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 5);

        conn.execute(
            "INSERT INTO orders
             (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
              quantity, amount_cents, currency, customer_email, state,
              capacity_consumed, paid_at, created_at, updated_at)
             VALUES
             ('ord_v5', 'cs_v5', 'pi_v5', '2026-08-14', 'dun-peas',
              2, 2400, 'cad', 'buyer@example.com', 'paid',
              2, '2026-08-05T12:00:00.000Z', '2026-08-05T12:00:00.000Z',
              '2026-08-05T12:00:00.000Z')",
            [],
        )
        .unwrap();

        db::migrate(&conn).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, db::SCHEMA_VERSION);

        let row: (String, i64, Option<String>) = conn
            .query_row(
                "SELECT crop_id, quantity, client_reference FROM orders WHERE id = 'ord_v5'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0, "dun-peas");
        assert_eq!(row.1, 2);
        assert!(row.2.is_none());

        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_orders_reference'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1);
    }

    // --- Stage 4 Prompt 3/3: farm-page cart steppers ---

    #[test]
    fn shop_page_has_steppers_total_pay_no_payment_links() {
        let mut conn = mem();
        seed_stripe_config(&conn);
        let peas = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &peas.id);
        let _sun = trays::sow_tray(&mut conn, "sunflower", 3).unwrap();
        let gw = money::fake::FakeGateway::new();
        offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();
        offers::set_offer_with(&mut conn, &gw, &hd, "sunflower", 1000).unwrap();

        let dir = temp_farm_dir();
        let page = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap();
        assert!(page.size_bytes < 100 * 1024, "size {}", page.size_bytes);
        let html = fs::read_to_string(&page.file_path).unwrap();
        assert_eq!(html.matches("data-stepper=").count(), 2);
        assert!(html.contains("id=\"total\""));
        assert_eq!(html.matches("id=\"pay\"").count(), 1);
        assert!(html.contains(">Pay</button>"));
        assert!(!html.contains("buy.stripe.com"));
        assert!(html.contains("data-price-id="));
        assert!(html.contains("data-price-cents="));
        assert!(html.contains("https://checkout.example.com/"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shop_page_mints_reference_inside_pay_handler_only() {
        let mut conn = mem();
        seed_stripe_config(&conn);
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = money::fake::FakeGateway::new();
        offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();

        let dir = temp_farm_dir();
        let page = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap();
        assert!(page.size_bytes < 100 * 1024, "size {}", page.size_bytes);
        let html = fs::read_to_string(&page.file_path).unwrap();

        // No page-scoped reference — minted only inside the Pay click handler.
        let pay_handler = html
            .split("pay.addEventListener('click'")
            .nth(1)
            .expect("Pay click handler");
        let before_pay = html.split("pay.addEventListener('click'").next().unwrap();
        assert!(
            !before_pay.contains("var reference="),
            "must not mint reference before Pay"
        );
        assert!(
            !before_pay.contains("crypto.randomUUID"),
            "randomUUID must not appear before the Pay handler"
        );
        assert!(
            pay_handler.contains("var reference="),
            "reference must be minted inside Pay"
        );
        assert!(
            pay_handler.contains("crypto.randomUUID"),
            "randomUUID must appear inside the Pay handler"
        );

        assert_eq!(html.matches("data-stepper=").count(), 1);
        assert!(html.contains("id=\"total\""));
        assert_eq!(html.matches("id=\"pay\"").count(), 1);
        assert!(!html.contains("buy.stripe.com"));
        assert!(!html.to_ascii_lowercase().contains("rk_"));
        assert!(!html.to_ascii_lowercase().contains("sk_"));
        assert!(!html.contains("https://cdn"));
        assert!(!html.contains("googleapis"));
        assert!(!html.contains("googletag"));
        assert!(!html.contains("http://"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shop_page_no_stripe_key_no_third_party_script() {
        let mut conn = mem();
        seed_stripe_config(&conn);
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = money::fake::FakeGateway::new();
        offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1100).unwrap();
        let dir = temp_farm_dir();
        let page = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap();
        assert!(page.size_bytes < 100 * 1024, "size {}", page.size_bytes);
        let html = fs::read_to_string(&page.file_path).unwrap();
        assert!(!html.to_ascii_lowercase().contains("rk_"));
        assert!(!html.to_ascii_lowercase().contains("sk_"));
        assert!(!html.contains("https://cdn"));
        assert!(!html.contains("googleapis"));
        assert!(!html.contains("googletag"));
        assert!(!html.contains("http://"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_shop_page_requires_checkout_endpoint_url() {
        let mut conn = mem();
        money::store_stripe_key(
            &conn,
            "rk_test_shop_page_seed_key_do_not_leak",
            &money::AccountInfo {
                account_id: "acct_shop".into(),
                account_name: "Shop Test".into(),
                mode: "test".into(),
            },
        )
        .unwrap();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = money::fake::FakeGateway::new();
        offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1100).unwrap();
        let dir = temp_farm_dir();
        let err = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap_err();
        assert!(err.contains("Add your checkout address in Sell online"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_shop_page_retires_harvest_links() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        seed_stripe_config(&conn);
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let gw = FakeGateway::new();
        offers::set_offer_with(&mut conn, &gw, &hd, "dun-peas", 1200).unwrap();

        conn.execute(
            "INSERT INTO harvest_links
             (harvest_date, stripe_link_id, stripe_link_url, line_signature, created_at)
             VALUES (?1, 'plink_old_1', 'https://buy.stripe.com/test/old', 'sig', ?2)",
            params![&hd, "2026-08-05T12:00:00.000Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO harvest_links
             (harvest_date, stripe_link_id, stripe_link_url, line_signature, created_at)
             VALUES ('2099-01-01', 'plink_old_2', 'https://buy.stripe.com/test/old2', 'sig2', ?1)",
            ["2026-08-05T12:00:00.000Z"],
        )
        .unwrap();

        let dir = temp_farm_dir();
        let _ = shop::generate_shop_page_with(&mut conn, &gw, &dir).unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM harvest_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
        {
            let st = gw.state.lock().unwrap();
            assert!(st.deactivated_links.contains(&"plink_old_1".to_string()));
            assert!(st.deactivated_links.contains(&"plink_old_2".to_string()));
            assert_eq!(st.harvest_links_created.len(), 0);
        }
        let html = fs::read_to_string(shop::shop_page_path(&dir)).unwrap();
        assert!(!html.contains("buy.stripe.com"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_session_records_client_reference_on_every_line() {
        let mut conn = mem();
        let peas = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &peas.id);
        let _sun = trays::sow_tray(&mut conn, "sunflower", 4).unwrap();
        seed_offer_price(&conn, &hd, "dun-peas", "price_ref_peas", 1200);
        seed_offer_price(&conn, &hd, "sunflower", "price_ref_sun", 1000);

        let mut session = paid_session_raw(
            "cs_ref_lines",
            vec![
                money::PaidLine {
                    price_id: "price_ref_peas".into(),
                    quantity: 3,
                    amount_cents: 3600,
                },
                money::PaidLine {
                    price_id: "price_ref_sun".into(),
                    quantity: 2,
                    amount_cents: 2000,
                },
            ],
            1_700_000_800,
        );
        session.client_reference = Some("cart_shared_ref".into());
        money::apply_paid_session(&mut conn, &session).unwrap();

        let refs: Vec<Option<String>> = conn
            .prepare("SELECT client_reference FROM orders WHERE stripe_session_id = 'cs_ref_lines'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().all(|r| r.as_deref() == Some("cart_shared_ref")));
    }

    #[test]
    fn poll_same_client_reference_consumes_capacity_once() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 6).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let before = trays::remaining_for_date(&conn, &hd).unwrap();

        let mut a = paid_session_at(&conn, "cs_poll_ref_a", &hd, "dun-peas", 2, 1200);
        a.client_reference = Some("poll_cart_one".into());
        let mut b = paid_session_at(&conn, "cs_poll_ref_b", &hd, "dun-peas", 2, 1201);
        b.client_reference = Some("poll_cart_one".into());

        let gw = FakeGateway::new();
        {
            let mut st = gw.state.lock().unwrap();
            st.session_pages = vec![money::SessionPage::from_parsed(vec![a, b])];
        }
        let r = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(r.ok);
        // J4 REF-DUP-FACT: the poll names the second session once — one
        // duplicate_reference fact keyed by cs_poll_ref_b.
        let dup_facts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM stripe_unapplied_facts
                 WHERE stripe_object = 'checkout_session'
                   AND stripe_id = 'cs_poll_ref_b'
                   AND status = 'duplicate_reference'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dup_facts, 1);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
        assert_eq!(trays::remaining_for_date(&conn, &hd).unwrap(), before - 2);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            1
        );
    }

    #[test]
    fn migration_v6_to_v7_preserves_orders_and_offers() {
        let conn = db::open_v6_in_memory().unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 6);

        conn.execute(
            "INSERT INTO offers
             (id, harvest_date, crop_id, price_cents, stripe_price_id,
              stripe_link_id, stripe_link_url, created_at)
             VALUES
             ('off_v6', '2026-08-14', 'dun-peas', 1200, 'price_v6',
              NULL, NULL, '2026-08-05T12:00:00.000Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO orders
             (id, stripe_session_id, stripe_payment_intent, harvest_date, crop_id,
              quantity, amount_cents, currency, customer_email, state,
              capacity_consumed, paid_at, created_at, updated_at, client_reference)
             VALUES
             ('ord_v6', 'cs_v6', 'pi_v6', '2026-08-14', 'dun-peas',
              3, 3600, 'cad', NULL, 'paid',
              3, '2026-08-05T12:00:00.000Z', '2026-08-05T12:00:00.000Z',
              '2026-08-05T12:00:00.000Z', 'cart_v6')",
            [],
        )
        .unwrap();

        db::migrate(&conn).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, db::SCHEMA_VERSION);

        let offer_price: String = conn
            .query_row(
                "SELECT stripe_price_id FROM offers WHERE id = 'off_v6'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(offer_price, "price_v6");

        let order: (i64, Option<String>) = conn
            .query_row(
                "SELECT quantity, client_reference FROM orders WHERE id = 'ord_v6'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(order.0, 3);
        assert_eq!(order.1.as_deref(), Some("cart_v6"));

        let url: Option<String> = conn
            .query_row(
                "SELECT checkout_endpoint_url FROM stripe_config WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(url.is_none() || url.as_deref() == Some(""));
    }

    // --- Track 1 Phase 1: origin spine, triggers, atomicity ---

    fn event_log_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
            .unwrap()
    }

    fn tray_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM trays", [], |r| r.get(0))
            .unwrap()
    }

    fn insert_event_raw(
        conn: &Connection,
        id: Option<&str>,
        kind: &str,
        origin: Option<&str>,
        event_domain: Option<&str>,
        event_class: Option<&str>,
    ) -> Result<usize, rusqlite::Error> {
        conn.execute(
            "INSERT INTO event_log
             (id, kind, entity_type, entity_id, payload, inverse, created_at,
              origin, event_domain, event_class, reverses_event_id)
             VALUES (?1, ?2, 'test', 't1', '{}', '{}', '2026-08-06T00:00:00.000Z',
                     ?3, ?4, ?5, NULL)",
            params![id, kind, origin, event_domain, event_class],
        )
    }

    #[test]
    fn fixture_v1_seed_matches_frozen_sow_dump() {
        let conn = db::open_v1_in_memory().unwrap();
        #[allow(clippy::type_complexity)]
        // H-7 Class E: a query_row tuple, not a signature. Nothing here to narrow.
        let tray: (
            String,
            String,
            String,
            i64,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<f64>,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT id, crop_id, state, quantity, growth_days_at_sow, blackout_days_at_sow,
                        planned_on, sown_on, blackout_on, light_on, harvested_on, discarded_on,
                        actual_yield_oz, created_at, updated_at
                 FROM trays ORDER BY rowid",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                        r.get(13)?,
                        r.get(14)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(tray.0, db::FIXTURE_V1_TRAY_ID);
        assert_eq!(tray.1, "dun-peas");
        assert_eq!(tray.2, "blackout");
        assert_eq!(tray.3, 3);
        assert_eq!(tray.4, Some(9));
        assert_eq!(tray.5, Some(3));
        assert!(tray.6.is_none());
        assert_eq!(tray.7.as_deref(), Some("2026-08-06"));
        assert_eq!(tray.8.as_deref(), Some("2026-08-06"));
        assert!(tray.9.is_none());
        assert!(tray.10.is_none());
        assert!(tray.11.is_none());
        assert!(tray.12.is_none());
        assert_eq!(tray.13, "2026-08-06T18:30:24.535Z");
        assert_eq!(tray.14, "2026-08-06T18:30:24.535Z");

        #[allow(clippy::type_complexity)]
        // H-7 Class E: a query_row tuple, not a signature. Nothing here to narrow.
        let event: (
            i64,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            String,
        ) = conn
            .query_row(
                "SELECT seq, id, kind, entity_type, entity_id, payload, inverse,
                        undone_at, undoes_seq, created_at
                 FROM event_log ORDER BY seq",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(event.0, 1);
        assert_eq!(event.1, "949c3e7f-09c3-46bb-aae0-56c07f440000");
        assert_eq!(event.2, "tray.sown");
        assert_eq!(event.3, "tray");
        assert_eq!(event.4, db::FIXTURE_V1_TRAY_ID);
        assert_eq!(
            event.5,
            r#"{"blackoutOn":"2026-08-06","cropId":"dun-peas","quantity":3,"sownOn":"2026-08-06"}"#
        );
        assert_eq!(
            event.6,
            r#"{"op":"delete_tray","trayId":"b370c73f-9627-4684-aea2-beb59e662fb9"}"#
        );
        assert!(event.7.is_none());
        assert!(event.8.is_none());
        assert_eq!(event.9, "2026-08-06T18:30:24.535Z");
        assert_eq!(tray_count(&conn), 1);
        assert_eq!(event_log_count(&conn), 1);
    }

    #[test]
    fn fixture_v2_seed_matches_frozen_sow_dump() {
        let conn = db::open_v2_in_memory().unwrap();
        #[allow(clippy::type_complexity)]
        // H-7 Class E: a query_row tuple, not a signature. Nothing here to narrow.
        let tray: (
            String,
            String,
            String,
            i64,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<f64>,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT id, crop_id, state, quantity, growth_days_at_sow, blackout_days_at_sow,
                        planned_on, sown_on, blackout_on, light_on, harvested_on, discarded_on,
                        actual_yield_oz, created_at, updated_at
                 FROM trays ORDER BY rowid",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                        r.get(13)?,
                        r.get(14)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(tray.0, db::FIXTURE_V2_TRAY_ID);
        assert_eq!(tray.1, "kale");
        assert_eq!(tray.2, "blackout");
        assert_eq!(tray.3, 2);
        assert_eq!(tray.4, Some(9));
        assert_eq!(tray.5, Some(4));
        assert!(tray.6.is_none());
        assert_eq!(tray.7.as_deref(), Some("2026-08-06"));
        assert_eq!(tray.8.as_deref(), Some("2026-08-06"));
        assert!(tray.9.is_none());
        assert!(tray.10.is_none());
        assert!(tray.11.is_none());
        assert!(tray.12.is_none());
        assert_eq!(tray.13, "2026-08-06T18:30:24.536Z");
        assert_eq!(tray.14, "2026-08-06T18:30:24.536Z");

        #[allow(clippy::type_complexity)]
        // H-7 Class E: a query_row tuple, not a signature. Nothing here to narrow.
        let event: (
            i64,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            String,
        ) = conn
            .query_row(
                "SELECT seq, id, kind, entity_type, entity_id, payload, inverse,
                        undone_at, undoes_seq, created_at
                 FROM event_log ORDER BY seq",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(event.0, 1);
        assert_eq!(event.1, "0d4ad63a-70b3-473a-a493-c203edb181c5");
        assert_eq!(event.2, "tray.sown");
        assert_eq!(event.3, "tray");
        assert_eq!(event.4, db::FIXTURE_V2_TRAY_ID);
        assert_eq!(
            event.5,
            r#"{"blackoutOn":"2026-08-06","cropId":"kale","quantity":2,"sownOn":"2026-08-06"}"#
        );
        assert_eq!(
            event.6,
            r#"{"op":"delete_tray","trayId":"e57c0a5d-2930-468f-875f-0df5b7257afc"}"#
        );
        assert!(event.7.is_none());
        assert!(event.8.is_none());
        assert_eq!(event.9, "2026-08-06T18:30:24.536Z");
        assert_eq!(tray_count(&conn), 1);
        assert_eq!(event_log_count(&conn), 1);
    }

    #[test]
    fn event_log_rejects_null_id() {
        let conn = mem();
        let before = event_log_count(&conn);
        let err = insert_event_raw(
            &conn,
            None,
            "tray.sown",
            Some("farm_os"),
            Some("grow"),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("event_log.id required") || err.to_string().contains("ABORT")
        );
        assert_eq!(event_log_count(&conn), before);
    }

    #[test]
    fn event_log_rejects_grow_with_event_class() {
        let conn = mem();
        let before = event_log_count(&conn);
        let err = insert_event_raw(
            &conn,
            Some("e-grow-class"),
            "tray.sown",
            Some("farm_os"),
            Some("grow"),
            Some("snapshot"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("event_class"));
        assert_eq!(event_log_count(&conn), before);
    }

    #[test]
    fn event_log_rejects_register_with_null_event_class() {
        let conn = mem();
        let before = event_log_count(&conn);
        let err = insert_event_raw(
            &conn,
            Some("e-reg-null"),
            "snapshot.taken",
            Some("farm_os"),
            Some("register"),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("event_class"));
        assert_eq!(event_log_count(&conn), before);
    }

    #[test]
    fn event_log_rejects_commercial_event_classes_count_unchanged() {
        let conn = mem();
        let before = event_log_count(&conn);
        for class in [
            "commercial_order",
            "commercial_payment",
            "commercial_stock_movement",
            "commercial_expense",
        ] {
            let err = insert_event_raw(
                &conn,
                Some(&format!("e-{class}")),
                "foreign",
                Some("farm_os"),
                Some("register"),
                Some(class),
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("event_class"),
                "expected class abort for {class}, got {err}"
            );
            assert_eq!(
                event_log_count(&conn) - before,
                0,
                "count delta must be zero after rejecting {class}"
            );
        }
        assert_eq!(event_log_count(&conn), before);
    }

    #[test]
    fn event_log_rejects_nonsense_event_domain() {
        let conn = mem();
        let before = event_log_count(&conn);
        let err = insert_event_raw(
            &conn,
            Some("e-nonsense"),
            "tray.sown",
            Some("farm_os"),
            Some("nonsense"),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("event_domain"));
        assert_eq!(event_log_count(&conn), before);
    }

    #[test]
    fn event_log_update_immutable_aborts_undone_at_succeeds() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let seq: i64 = conn
            .query_row(
                "SELECT seq FROM event_log ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let err = conn
            .execute(
                "UPDATE event_log SET event_class = 'snapshot' WHERE seq = ?1",
                [seq],
            )
            .unwrap_err();
        assert!(err.to_string().contains("immutable"));

        conn.execute(
            "UPDATE event_log SET undone_at = '2026-08-06T12:00:00.000Z' WHERE seq = ?1",
            [seq],
        )
        .unwrap();
        let undone: Option<String> = conn
            .query_row(
                "SELECT undone_at FROM event_log WHERE seq = ?1",
                [seq],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(undone.as_deref(), Some("2026-08-06T12:00:00.000Z"));
    }

    #[test]
    fn event_log_delete_aborted() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let before = event_log_count(&conn);
        let err = conn.execute("DELETE FROM event_log", []).unwrap_err();
        assert!(err.to_string().contains("append-only"));
        assert_eq!(event_log_count(&conn), before);
    }

    #[test]
    fn sow_writes_grow_origin_spine_fields() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let row: (String, String, Option<String>, String) = conn
            .query_row(
                "SELECT id, origin, event_class, event_domain FROM event_log
                 WHERE kind = 'tray.sown' ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert!(!row.0.is_empty());
        assert_eq!(row.1, "farm_os");
        assert!(row.2.is_none());
        assert_eq!(row.3, "grow");
    }

    #[test]
    fn snapshot_writes_register_snapshot_event() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snap_dir = dir.join("snapshots");
        fs::create_dir_all(&snap_dir).unwrap();
        let mut conn = db::open_and_migrate(&farm).unwrap();
        let before = event_log_count(&conn);
        snapshots::take_snapshot(&mut conn, &snap_dir).unwrap();
        assert_eq!(event_log_count(&conn), before + 1);
        let row: (String, Option<String>) = conn
            .query_row(
                "SELECT event_domain, event_class FROM event_log
                 ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "register");
        assert_eq!(row.1.as_deref(), Some("snapshot"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sow_tray_event_failure_rolls_back_tray_and_log() {
        let mut conn = mem();
        conn.execute_batch(
            "CREATE TRIGGER fail_event_after_state BEFORE INSERT ON event_log
             BEGIN SELECT RAISE(ABORT, 'forced event failure'); END;",
        )
        .unwrap();
        let trays_before = tray_count(&conn);
        let events_before = event_log_count(&conn);
        let err = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap_err();
        assert!(err.contains("forced event failure") || err.contains("ABORT"));
        assert_eq!(tray_count(&conn), trays_before);
        assert_eq!(event_log_count(&conn), events_before);
    }

    #[test]
    fn pragma_wal_and_foreign_keys_after_configure() {
        let dir = temp_farm_dir();
        let farm = dir.join("pragma-farm.db");
        let conn = Connection::open(&farm).unwrap();
        db::configure(&conn).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase1_schema_triggers_user_version_evidence() {
        let mut conn = mem();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        println!("PRAGMA user_version = {version}");
        assert_eq!(version, db::SCHEMA_VERSION);

        // Seed one grow + one register row so the domain/class table is non-empty evidence.
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let dir = temp_farm_dir();
        let snap_dir = dir.join("snapshots");
        fs::create_dir_all(&snap_dir).unwrap();
        // File-backed snap needs a file DB for VACUUM; use in-memory path via take on mem is OK.
        // Prefer a file farm for the register evidence row:
        let farm = dir.join("farm.db");
        let mut file_conn = db::open_and_migrate(&farm).unwrap();
        trays::sow_tray(&mut file_conn, "kale", 1).unwrap();
        snapshots::take_snapshot(&mut file_conn, &snap_dir).unwrap();

        println!("event_log columns:");
        let mut stmt = conn.prepare("PRAGMA table_info(event_log)").unwrap();
        let cols = stmt
            .query_map([], |r| {
                Ok(format!(
                    "  cid={} name={} type={} notnull={} dflt_value={:?} pk={}",
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .unwrap()
            .map(|x| x.unwrap())
            .collect::<Vec<_>>();
        for line in &cols {
            println!("{line}");
        }
        assert!(cols.iter().any(|c| c.contains("name=origin")));
        assert!(cols.iter().any(|c| c.contains("name=event_domain")));
        assert!(cols.iter().any(|c| c.contains("name=event_class")));
        assert!(cols.iter().any(|c| c.contains("name=reverses_event_id")));

        println!("event_log triggers:");
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type='trigger' AND tbl_name='event_log'
                 ORDER BY name",
            )
            .unwrap();
        let triggers = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|x| x.unwrap())
            .collect::<Vec<_>>();
        for name in &triggers {
            println!("  {name}");
        }
        assert!(triggers.iter().any(|n| n == "event_log_before_insert"));
        assert!(triggers.iter().any(|n| n == "event_log_before_update"));
        assert!(triggers.iter().any(|n| n == "event_log_before_delete"));

        println!("event_log row counts by event_domain / event_class:");
        let mut stmt = file_conn
            .prepare(
                "SELECT IFNULL(event_domain, 'NULL'), IFNULL(event_class, 'NULL'), COUNT(*)
                 FROM event_log
                 GROUP BY event_domain, event_class
                 ORDER BY 1, 2",
            )
            .unwrap();
        let groups = stmt
            .query_map([], |r| {
                Ok(format!(
                    "  domain={} class={} count={}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .map(|x| x.unwrap())
            .collect::<Vec<_>>();
        for line in &groups {
            println!("{line}");
        }
        assert!(groups.iter().any(|g| g.contains("domain=grow")));
        assert!(groups
            .iter()
            .any(|g| g.contains("domain=register") && g.contains("class=snapshot")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_sets_reverses_event_id() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let sown_id: String = conn
            .query_row(
                "SELECT id FROM event_log WHERE kind = 'tray.sown' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        trays::undo_last(&mut conn).unwrap();
        let reverses: Option<String> = conn
            .query_row(
                "SELECT reverses_event_id FROM event_log
                 WHERE kind = 'undo' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reverses.as_deref(), Some(sown_id.as_str()));
    }

    // --- Track 1 Phase 2: classification correction ---

    #[test]
    fn paid_session_writes_register_sale_farm_os_path() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session(&conn, "cs_reg_paid", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();
        let row: (String, Option<String>) = conn
            .query_row(
                "SELECT event_domain, event_class FROM event_log
                 WHERE kind = 'stripe.session_paid' ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "register");
        assert_eq!(row.1.as_deref(), Some("sale_farm_os_path"));
    }

    #[test]
    fn refund_writes_sale_class_and_reverses_paid_session() {
        let mut conn = mem();
        let hd = future_harvest_date();
        let session = paid_session(&conn, "cs_reg_ref", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();
        let paid_id: String = conn
            .query_row(
                "SELECT id FROM event_log
                 WHERE kind = 'stripe.session_paid' AND entity_id = 'cs_reg_ref'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        money::apply_refund(
            &mut conn,
            &money::RefundRecord {
                refund_id: "re_reg".into(),
                payment_intent: Some("pi_cs_reg_ref".into()),
                session_id: Some("cs_reg_ref".into()),
                created: 1_700_000_400,
                amount_cents: Some(1200),
                currency: Some("cad".into()),
                status: Some("succeeded".into()),
            },
        )
        .unwrap();
        let row: (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT event_domain, event_class, reverses_event_id FROM event_log
                 WHERE kind = 'stripe.refunded' ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0, "register");
        assert_eq!(row.1.as_deref(), Some("sale_farm_os_path"));
        assert_eq!(row.2.as_deref(), Some(paid_id.as_str()));
    }

    #[test]
    fn dispute_writes_sale_class_reverses_null_status_only() {
        // Case: status change only — no funds movement, capacity retained.
        let mut conn = mem();
        let hd = future_harvest_date();
        let session = paid_session(&conn, "cs_reg_disp", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();
        money::apply_dispute(
            &mut conn,
            &money::DisputeRecord {
                dispute_id: "dp_reg".into(),
                payment_intent: Some("pi_cs_reg_disp".into()),
                session_id: Some("cs_reg_disp".into()),
                created: 1_700_000_401,
                amount_cents: None,
                currency: None,
                status: None,
            },
        )
        .unwrap();
        let row: (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT event_domain, event_class, reverses_event_id FROM event_log
                 WHERE kind = 'stripe.disputed' ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0, "register");
        assert_eq!(row.1.as_deref(), Some("sale_farm_os_path"));
        assert!(row.2.is_none());
    }

    #[test]
    fn money_kind_with_grow_domain_aborted_count_unchanged() {
        let conn = mem();
        let before = event_log_count(&conn);
        for kind in ["stripe.session_paid", "stripe.refunded", "stripe.disputed"] {
            let err = insert_event_raw(
                &conn,
                Some(&format!("bad-{kind}")),
                kind,
                Some("farm_os"),
                Some("grow"),
                None,
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("kind invalid for grow")
                    || err.to_string().contains("ABORT"),
                "expected grow-kind abort for {kind}, got {err}"
            );
            assert_eq!(event_log_count(&conn) - before, 0);
        }
        assert_eq!(event_log_count(&conn), before);
    }

    #[test]
    fn grow_kind_with_register_domain_aborted() {
        let conn = mem();
        let before = event_log_count(&conn);
        let err = insert_event_raw(
            &conn,
            Some("bad-grow-as-reg"),
            "tray.sown",
            Some("farm_os"),
            Some("register"),
            Some("sale_farm_os_path"),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("kind invalid for register")
                || err.to_string().contains("ABORT")
        );
        assert_eq!(event_log_count(&conn), before);
    }

    #[test]
    fn register_null_event_class_aborted() {
        let conn = mem();
        let before = event_log_count(&conn);
        let err = insert_event_raw(
            &conn,
            Some("bad-reg-null-class"),
            "stripe.session_paid",
            Some("farm_os"),
            Some("register"),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("event_class"));
        assert_eq!(event_log_count(&conn), before);
    }

    #[test]
    fn migration_9_corrects_null_and_mislabelled_rows_no_loss() {
        let conn = db::open_v8_in_memory().unwrap();
        db::drop_event_log_triggers(&conn).unwrap();
        // Pre-Phase-1 NULL spine on a grow kind.
        conn.execute(
            "INSERT INTO event_log
             (id, kind, entity_type, entity_id, payload, inverse, created_at,
              origin, event_domain, event_class, reverses_event_id)
             VALUES
             ('legacy-grow', 'tray.sown', 'tray', 't-legacy', '{}', '{}',
              '2026-08-01T00:00:00.000Z', NULL, NULL, NULL, NULL)",
            [],
        )
        .unwrap();
        // Post-Phase-1 mislabelled money row (grow tier).
        conn.execute(
            "INSERT INTO event_log
             (id, kind, entity_type, entity_id, payload, inverse, created_at,
              origin, event_domain, event_class, reverses_event_id)
             VALUES
             ('mis-paid', 'stripe.session_paid', 'stripe_session', 'cs_mis', '{}', '{}',
              '2026-08-06T00:00:00.000Z', 'farm_os', 'grow', NULL, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO event_log
             (id, kind, entity_type, entity_id, payload, inverse, created_at,
              origin, event_domain, event_class, reverses_event_id)
             VALUES
             ('mis-snap', 'snapshot.taken', 'snapshot', 'farm-x.db', '{}', '{}',
              '2026-08-06T00:00:00.000Z', 'farm_os', 'grow', NULL, NULL)",
            [],
        )
        .unwrap();
        // Restore Phase 1 triggers so the freeze point matches real v8 farms.
        conn.execute_batch(
            r#"
            CREATE TRIGGER IF NOT EXISTS event_log_before_insert
            BEFORE INSERT ON event_log
            BEGIN
              SELECT CASE
                WHEN NEW.id IS NULL OR NEW.id = ''
                  THEN RAISE(ABORT, 'event_log.id required')
                WHEN NEW.origin IS NULL OR NEW.origin NOT IN ('farm_os', 'commercial_app')
                  THEN RAISE(ABORT, 'event_log.origin invalid')
                WHEN NEW.event_domain IS NULL OR NEW.event_domain NOT IN ('grow', 'register')
                  THEN RAISE(ABORT, 'event_log.event_domain invalid')
                WHEN NEW.event_domain = 'register' AND (
                  NEW.event_class IS NULL OR NEW.event_class NOT IN (
                    'money_out', 'physical_consumption', 'mileage', 'asset_register',
                    'sale_farm_os_path', 'capacity_commitment', 'snapshot'
                  )
                )
                  THEN RAISE(ABORT, 'event_log.event_class invalid for register')
                WHEN NEW.event_domain = 'grow' AND NEW.event_class IS NOT NULL
                  THEN RAISE(ABORT, 'event_log.event_class must be NULL for grow')
                WHEN NEW.event_domain = 'grow' AND (
                  NEW.kind IS NULL OR NEW.kind NOT IN (
                    'tray.sown', 'trays.advanced', 'trays.harvested', 'tray.discarded',
                    'trays.discarded', 'recount.applied', 'undo', 'dev.backdated',
                    'attention.resolved', 'stripe.session_paid', 'stripe.refunded',
                    'stripe.disputed'
                  )
                )
                  THEN RAISE(ABORT, 'event_log.kind invalid for grow')
              END;
            END;
            CREATE TRIGGER IF NOT EXISTS event_log_before_update
            BEFORE UPDATE ON event_log
            BEGIN
              SELECT CASE
                WHEN OLD.id IS NOT NEW.id
                  OR OLD.seq IS NOT NEW.seq
                  OR OLD.origin IS NOT NEW.origin
                  OR OLD.event_domain IS NOT NEW.event_domain
                  OR OLD.event_class IS NOT NEW.event_class
                  OR OLD.kind IS NOT NEW.kind
                  THEN RAISE(ABORT, 'event_log immutable columns')
              END;
            END;
            CREATE TRIGGER IF NOT EXISTS event_log_before_delete
            BEFORE DELETE ON event_log
            BEGIN
              SELECT RAISE(ABORT, 'event_log is append-only');
            END;
            "#,
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 8).unwrap();

        let before = event_log_count(&conn);
        assert!(before >= 3);
        db::migrate(&conn).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, db::SCHEMA_VERSION);
        assert_eq!(event_log_count(&conn), before);

        let legacy: (String, Option<String>, String) = conn
            .query_row(
                "SELECT origin, event_class, event_domain FROM event_log WHERE id = 'legacy-grow'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(legacy.0, "farm_os");
        assert!(legacy.1.is_none());
        assert_eq!(legacy.2, "grow");

        let paid: (String, Option<String>) = conn
            .query_row(
                "SELECT event_domain, event_class FROM event_log WHERE id = 'mis-paid'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(paid.0, "register");
        assert_eq!(paid.1.as_deref(), Some("sale_farm_os_path"));

        let snap: (String, Option<String>) = conn
            .query_row(
                "SELECT event_domain, event_class FROM event_log WHERE id = 'mis-snap'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(snap.0, "register");
        assert_eq!(snap.1.as_deref(), Some("snapshot"));
    }

    #[test]
    fn migration_9_rerun_zero_rows_identical_digest() {
        let conn = db::open_v8_in_memory().unwrap();
        db::drop_event_log_triggers(&conn).unwrap();
        conn.execute(
            "INSERT INTO event_log
             (id, kind, entity_type, entity_id, payload, inverse, created_at,
              origin, event_domain, event_class, reverses_event_id)
             VALUES
             ('idem-grow', 'tray.sown', 'tray', 't1', '{}', '{}',
              '2026-08-01T00:00:00.000Z', NULL, NULL, NULL, NULL),
             ('idem-paid', 'stripe.session_paid', 'stripe_session', 'cs_i', '{}', '{}',
              '2026-08-06T00:00:00.000Z', 'farm_os', 'grow', NULL, NULL)",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 8).unwrap();
        db::migrate(&conn).unwrap();

        let digest1 = db::spine_tuple_digest(&conn).unwrap();
        let preview = db::preview_spine_backfill(&conn).unwrap();
        assert_eq!(preview.total(), 0);
        // Second application of corrective UPDATEs (triggers already v9; fills are no-ops).
        db::drop_event_log_triggers(&conn).unwrap();
        let touched = db::apply_spine_backfill(&conn).unwrap();
        assert_eq!(touched, 0);
        db::install_v9_event_log_triggers(&conn).unwrap();
        let digest2 = db::spine_tuple_digest(&conn).unwrap();
        assert_eq!(digest1, digest2);
    }

    #[test]
    fn spine_backfill_dry_run_report() {
        let conn = db::open_v8_in_memory().unwrap();
        db::drop_event_log_triggers(&conn).unwrap();
        conn.execute(
            "INSERT INTO event_log
             (id, kind, entity_type, entity_id, payload, inverse, created_at,
              origin, event_domain, event_class, reverses_event_id)
             VALUES
             ('dry-null', 'tray.sown', 'tray', 't', '{}', '{}',
              '2026-08-01T00:00:00.000Z', NULL, NULL, NULL, NULL)",
            [],
        )
        .unwrap();
        let preview = db::spine_backfill_dry_run(&conn).unwrap();
        assert!(preview.null_origin >= 1);
        assert!(preview.grow_rows_needing_domain >= 1);
        // Dry-run must not write.
        let origin: Option<String> = conn
            .query_row(
                "SELECT origin FROM event_log WHERE id = 'dry-null'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(origin.is_none());
    }

    #[test]
    fn event_domain_change_aborted_null_fill_succeeds() {
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let seq: i64 = conn
            .query_row(
                "SELECT seq FROM event_log WHERE kind = 'tray.sown' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let err = conn
            .execute(
                "UPDATE event_log SET event_domain = 'register' WHERE seq = ?1",
                [seq],
            )
            .unwrap_err();
        assert!(err.to_string().contains("immutable"));

        db::drop_event_log_triggers(&conn).unwrap();
        conn.execute(
            "UPDATE event_log SET event_domain = NULL WHERE seq = ?1",
            [seq],
        )
        .unwrap();
        db::install_v9_event_log_triggers(&conn).unwrap();

        conn.execute(
            "UPDATE event_log SET event_domain = 'grow' WHERE seq = ?1",
            [seq],
        )
        .unwrap();
        let domain: String = conn
            .query_row(
                "SELECT event_domain FROM event_log WHERE seq = ?1",
                [seq],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(domain, "grow");
    }

    // --- Track 1 Phase 3: refusal guard, events.jsonl, spine report ---

    fn insert_unguarded_event(
        conn: &Connection,
        id: &str,
        kind: &str,
        origin: Option<&str>,
        event_domain: Option<&str>,
        event_class: Option<&str>,
    ) {
        db::drop_event_log_triggers(conn).unwrap();
        conn.execute(
            "INSERT INTO event_log
             (id, kind, entity_type, entity_id, payload, inverse, created_at,
              origin, event_domain, event_class, reverses_event_id)
             VALUES (?1, ?2, 'test', 't1', '{}', '{}', '2026-08-06T00:00:00.000Z',
                     ?3, ?4, ?5, NULL)",
            params![id, kind, origin, event_domain, event_class],
        )
        .unwrap();
        db::install_v9_event_log_triggers(conn).unwrap();
    }

    #[test]
    fn flush_aborts_null_origin_file_untouched() {
        let dir = temp_farm_dir();
        let conn = mem();
        insert_unguarded_event(
            &conn,
            "bad-null-origin",
            "tray.sown",
            None,
            Some("grow"),
            None,
        );
        let before = file_bytes(&event_file::events_path(&dir));
        let err = event_file::flush_events(&conn, &dir).unwrap_err();
        assert!(err.contains("offending seq"));
        assert!(err.to_lowercase().contains("origin"));
        let after = file_bytes(&event_file::events_path(&dir));
        assert_eq!(before, after);
        assert!(!event_file::events_path(&dir).exists() || after == before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_aborts_register_null_event_class() {
        let dir = temp_farm_dir();
        let conn = mem();
        insert_unguarded_event(
            &conn,
            "bad-reg-null-class",
            "stripe.session_paid",
            Some("farm_os"),
            Some("register"),
            None,
        );
        let before = file_bytes(&event_file::events_path(&dir));
        let err = event_file::flush_events(&conn, &dir).unwrap_err();
        assert!(err.contains("offending seq"));
        assert!(err.contains("event_class"));
        assert_eq!(before, file_bytes(&event_file::events_path(&dir)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_aborts_grow_with_event_class() {
        let dir = temp_farm_dir();
        let conn = mem();
        insert_unguarded_event(
            &conn,
            "bad-grow-class",
            "tray.sown",
            Some("farm_os"),
            Some("grow"),
            Some("money_out"),
        );
        let before = file_bytes(&event_file::events_path(&dir));
        let err = event_file::flush_events(&conn, &dir).unwrap_err();
        assert!(err.contains("offending seq"));
        assert!(err.contains("grow") && err.contains("event_class"));
        assert_eq!(before, file_bytes(&event_file::events_path(&dir)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_aborts_kind_outside_partition() {
        let dir = temp_farm_dir();
        let conn = mem();
        insert_unguarded_event(
            &conn,
            "bad-kind",
            "not.a.real.kind",
            Some("farm_os"),
            Some("grow"),
            None,
        );
        let before = file_bytes(&event_file::events_path(&dir));
        let err = event_file::flush_events(&conn, &dir).unwrap_err();
        assert!(err.contains("offending seq"));
        assert!(err.contains("outside the partition") || err.contains("not in grow"));
        assert_eq!(before, file_bytes(&event_file::events_path(&dir)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_abort_names_offending_seqs() {
        let dir = temp_farm_dir();
        let conn = mem();
        insert_unguarded_event(&conn, "bad-a", "tray.sown", None, Some("grow"), None);
        insert_unguarded_event(&conn, "bad-b", "tray.sown", None, Some("grow"), None);
        let seqs: Vec<i64> = conn
            .prepare("SELECT seq FROM event_log WHERE id IN ('bad-a','bad-b') ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let err = event_file::flush_events(&conn, &dir).unwrap_err();
        for seq in seqs {
            assert!(
                err.contains(&seq.to_string()),
                "error should name seq {seq}: {err}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn initial_flush_writes_all_rows_seq_order_non_null_spine() {
        let dir = temp_farm_dir();
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        trays::sow_tray(&mut conn, "kale", 1).unwrap();
        let ok = event_file::flush_events(&conn, &dir).unwrap();
        // Each sow: tray.sown + consumption.physical (trays).
        assert_eq!(ok.lines_written, 4);
        let text = fs::read_to_string(event_file::events_path(&dir)).unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 4);
        let mut prev = 0i64;
        for line in lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let seq = v["seq"].as_i64().unwrap();
            assert!(seq > prev);
            prev = seq;
            assert!(!v["origin"].as_str().unwrap().is_empty());
            assert!(!v["event_domain"].as_str().unwrap().is_empty());
            assert!(v.get("event_id").and_then(|x| x.as_str()).is_some());
            assert!(v.get("created_at").is_some());
            assert!(v.get("payload").is_some());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_flush_byte_identical() {
        let dir = temp_farm_dir();
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        event_file::flush_events(&conn, &dir).unwrap();
        let before = file_bytes(&event_file::events_path(&dir));
        let ok = event_file::flush_events(&conn, &dir).unwrap();
        assert_eq!(ok.lines_written, 0);
        let after = file_bytes(&event_file::events_path(&dir));
        assert_eq!(before, after);
        assert_eq!(before.len(), after.len());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_one_event_preserves_prefix_bytes() {
        let dir = temp_farm_dir();
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        event_file::flush_events(&conn, &dir).unwrap();
        let prefix = file_bytes(&event_file::events_path(&dir));
        trays::sow_tray(&mut conn, "kale", 1).unwrap();
        let ok = event_file::flush_events(&conn, &dir).unwrap();
        // Second sow appends tray.sown + consumption.physical.
        assert_eq!(ok.lines_written, 2);
        let after = file_bytes(&event_file::events_path(&dir));
        assert!(after.len() > prefix.len());
        assert_eq!(&after[..prefix.len()], prefix.as_slice());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn integrity_ok_then_fails_when_middle_line_removed() {
        let dir = temp_farm_dir();
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        trays::sow_tray(&mut conn, "kale", 1).unwrap();
        trays::sow_tray(&mut conn, "broccoli", 1).unwrap();
        event_file::flush_events(&conn, &dir).unwrap();
        event_file::verify_integrity(&conn, &dir).unwrap();

        let path = event_file::events_path(&dir);
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        // 3 sows × (tray.sown + consumption) = 6 lines.
        assert_eq!(lines.len(), 6);
        let mut broken = String::new();
        broken.push_str(lines[0]);
        broken.push('\n');
        broken.push_str(lines[2]);
        broken.push('\n');
        fs::write(&path, broken).unwrap();

        let err = event_file::verify_integrity(&conn, &dir).unwrap_err();
        assert!(
            err.contains("integrity failed"),
            "expected loud integrity failure, got: {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_failure_does_not_fail_action_next_flush_catches_up() {
        let dir = temp_farm_dir();
        let mut conn = mem();
        // First successful flush establishes the file.
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        event_file::flush_events(&conn, &dir).unwrap();
        let before_len = file_bytes(&event_file::events_path(&dir)).len();

        event_file::force_next_flush_io_failure();
        let tray = trays::sow_tray(&mut conn, "kale", 1).unwrap();
        // Simulate the command-layer contract: action committed, then best-effort flush.
        event_file::try_flush_after_commit(&conn, &dir);
        assert_eq!(
            file_bytes(&event_file::events_path(&dir)).len(),
            before_len,
            "failed flush must not append"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0))
            .unwrap();
        assert!(count >= 2);
        assert!(!tray.id.is_empty());

        let status = event_file::read_last_flush_status(&dir);
        assert!(status.contains("simulated") || status.contains("aborted"));

        let ok = event_file::flush_events(&conn, &dir).unwrap();
        // Catch-up writes the missed kale sow pair (tray.sown + consumption).
        assert_eq!(ok.lines_written, 2);
        let after = file_bytes(&event_file::events_path(&dir));
        assert!(after.len() > before_len);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spine_report_on_start_domain_counts_and_aborted_flush() {
        let dir = temp_farm_dir();
        let mut conn = mem();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        insert_unguarded_event(&conn, "report-bad", "tray.sown", None, Some("grow"), None);
        let err = event_file::flush_events(&conn, &dir).unwrap_err();
        assert!(err.contains("offending seq"));
        event_file::write_spine_report(&conn, &dir).unwrap();
        let report = fs::read_to_string(event_file::spine_report_path(&dir)).unwrap();
        assert!(report.contains("PRAGMA user_version"));
        assert!(report.contains("event_domain="));
        assert!(report.contains("aborted") || report.contains("flush aborted"));
        assert!(report.contains("offending seq") || report.contains("origin"));
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Track 1 Phase 4: projection extraction + verify-replay ---

    fn file_farm() -> (PathBuf, Connection) {
        let dir = temp_farm_dir();
        let path = dir.join("farm.db");
        let conn = db::open_and_migrate(&path).unwrap();
        (dir, conn)
    }

    fn flush_dir(conn: &Connection, dir: &Path) {
        event_file::flush_events(conn, dir).unwrap();
    }

    fn verify_dir(dir: &Path) -> projection::VerifyOutcome {
        projection::verify_replay_paths(&dir.join("farm.db"), &event_file::events_path(dir), dir)
            .unwrap()
    }

    #[test]
    fn verify_replay_grow_kinds_reproduce_in_scope() {
        let (dir, mut conn) = file_farm();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap(); // blackout -> light
        trays::harvest_tray(&mut conn, &t.id, 4.0).unwrap();
        let t2 = trays::sow_tray(&mut conn, "kale", 2).unwrap();
        trays::discard_tray(&mut conn, &t2.id).unwrap(); // discard from blackout
        let t3 = trays::sow_tray(&mut conn, "broccoli", 3).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&t3.id)).unwrap(); // -> light
        trays::discard_from_group(&mut conn, std::slice::from_ref(&t3.id), 1).unwrap();
        trays::apply_recount(
            &mut conn,
            &[crate::models::RecountEntry {
                crop_id: "broccoli".into(),
                counted_quantity: 1,
            }],
        )
        .unwrap();
        flush_dir(&conn, &dir);
        drop(conn); // release WAL locks before VACUUM INTO
        let outcome = verify_dir(&dir);
        assert!(
            !outcome.exit_nonzero(),
            "verify-replay failed: {}",
            outcome.summary_line()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_replay_register_shapes_and_snapshot_noop() {
        let (dir, mut conn) = file_farm();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let mut session = paid_session(&conn, "cs_p4_nested", &hd, "dun-peas", 1);
        session.client_reference = Some("cart_p4_ref".into());
        money::apply_paid_session(&mut conn, &session).unwrap();
        // Flat historical shape — insert via apply directly then flush.
        let flat = events::EventRecord {
            seq: None,
            event_id: projection::handler_new_id(),
            kind: events::Kind::StripeSessionPaid,
            entity_type: "stripe_session".into(),
            entity_id: "cs_p4_flat".into(),
            payload: serde_json::json!({
                "orderId": "ord_flat_p4",
                "cropId": "dun-peas",
                "quantity": 1,
                "amountCents": 200,
                "sessionId": "cs_p4_flat",
                "paymentIntent": "pi_flat",
                "harvestDate": hd,
                "amountCentsSession": 200,
                "currency": "cad",
                "customerEmail": "a@b.c",
                "paidAt": "2026-08-06T12:00:00.000Z"
            }),
            inverse: serde_json::json!({ "op": "none" }),
            origin: "farm_os".into(),
            event_domain: "register".into(),
            event_class: Some("sale_farm_os_path".into()),
            reverses_event_id: None,
            undoes_seq: None,
            undone_at: None,
            created_at: "2026-08-06T12:00:00.000Z".into(),
        };
        // Fix flat payload to match apply_stripe_session_paid expected keys.
        let flat = {
            let mut e = flat;
            e.payload = serde_json::json!({
                "orderId": "ord_flat_p4",
                "cropId": "dun-peas",
                "quantity": 1,
                "amountCents": 200,
                "sessionId": "cs_p4_flat",
                "paymentIntent": "pi_flat",
                "harvestDate": hd,
                "currency": "cad",
                "customerEmail": "a@b.c",
                "paidAt": "2026-08-06T12:00:00.000Z"
            });
            e
        };
        {
            let tx = conn.transaction().unwrap();
            projection::apply_event(&tx, &flat).unwrap();
            events::insert_event(&tx, &flat).unwrap();
            tx.commit().unwrap();
        }
        let snaps = dir.join("snapshots");
        fs::create_dir_all(&snaps).unwrap();
        snapshots::take_snapshot(&mut conn, &snaps).unwrap();
        flush_dir(&conn, &dir);
        let cr: Option<String> = conn
            .query_row(
                "SELECT client_reference FROM orders WHERE stripe_session_id = 'cs_p4_nested'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cr.as_deref(), Some("cart_p4_ref"));
        drop(conn);
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_replay_undo_reproduces_undone_at() {
        let (dir, mut conn) = file_farm();
        let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        trays::undo_last(&mut conn).unwrap();
        let tray_gone: i64 = conn
            .query_row("SELECT COUNT(*) FROM trays WHERE id = ?1", [&t.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tray_gone, 0);
        let undone: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE kind = 'tray.sown' AND undone_at IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(undone, 1);
        flush_dir(&conn, &dir);
        drop(conn);
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_replay_fails_on_mutated_in_scope_field() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        conn.execute("UPDATE trays SET quantity = 99", []).unwrap();
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(outcome.exit_nonzero());
        match outcome {
            projection::VerifyOutcome::Fail { report } => {
                assert!(report
                    .unknown_diffs
                    .iter()
                    .any(|d| { d.table == "trays" && d.field == "quantity" }));
            }
            other => panic!("expected Fail, got {}", other.summary_line()),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_replay_fails_when_middle_jsonl_line_removed() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        trays::sow_tray(&mut conn, "kale", 1).unwrap();
        trays::sow_tray(&mut conn, "broccoli", 1).unwrap();
        flush_dir(&conn, &dir);
        let path = event_file::events_path(&dir);
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        let broken = format!("{}\n{}\n", lines[0], lines[2]);
        fs::write(&path, broken).unwrap();
        let outcome = projection::verify_replay_paths(&dir.join("farm.db"), &path, &dir).unwrap();
        assert!(outcome.exit_nonzero(), "{}", outcome.summary_line());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_replay_determinism_byte_identical_projections() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        flush_dir(&conn, &dir);
        let events = event_file::events_path(&dir);
        let farm = dir.join("farm.db");
        let r1 = dir.join("replay1.db");
        let r2 = dir.join("replay2.db");
        // Use VACUUM snapshot once.
        let snap = dir.join("snap.db");
        {
            let live = Connection::open(&farm).unwrap();
            live.execute("VACUUM INTO ?1", [&snap.to_str().unwrap()])
                .unwrap();
        }
        projection::verify_replay(&snap, &events, &r1).unwrap();
        projection::verify_replay(&snap, &events, &r2).unwrap();
        // Compare trays+orders+event_log dumps.
        let dump = |p: &Path| {
            let c = Connection::open(p).unwrap();
            let mut out = String::new();
            for table in ["event_log", "trays", "orders"] {
                let mut stmt = c
                    .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                    .unwrap();
                let cols = stmt.column_count();
                let mut q = stmt.query([]).unwrap();
                while let Some(row) = q.next().unwrap() {
                    for i in 0..cols {
                        let v: String = match row.get_ref(i).unwrap() {
                            rusqlite::types::ValueRef::Null => "NULL".into(),
                            rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                            rusqlite::types::ValueRef::Real(n) => n.to_string(),
                            rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into(),
                            rusqlite::types::ValueRef::Blob(_) => "<blob>".into(),
                        };
                        out.push_str(&v);
                        out.push('|');
                    }
                    out.push('\n');
                }
            }
            out
        };
        assert_eq!(dump(&r1), dump(&r2));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_panics_if_clock_read_reintroduced() {
        let conn = mem();
        let event = events::EventRecord {
            seq: None,
            event_id: "e1".into(),
            kind: events::Kind::TraySown,
            entity_type: "tray".into(),
            entity_id: "tray-clock-test".into(),
            payload: serde_json::json!({
                "cropId": "dun-peas",
                "quantity": 1,
                "sownOn": "2026-08-06",
                "blackoutOn": "2026-08-06"
            }),
            inverse: serde_json::json!({"op":"delete_tray","trayId":"tray-clock-test"}),
            origin: "farm_os".into(),
            event_domain: "grow".into(),
            event_class: None,
            reverses_event_id: None,
            undoes_seq: None,
            undone_at: None,
            created_at: "2026-08-06T00:00:00.000Z".into(),
        };
        // Healthy apply with clock forbidden must succeed (no clock in apply).
        let mut conn = conn;
        let tx = conn.transaction().unwrap();
        db::with_clock_forbidden(|| {
            projection::apply_event(&tx, &event).unwrap();
        });
        tx.commit().unwrap();
    }

    #[test]
    fn kind_dispatch_covers_every_variant() {
        // Compile-time exhaustiveness lives in projection::apply_event's match.
        // This runtime check ensures the Kind parser and as_str round-trip for all.
        use events::Kind::*;
        let all = [
            TraySown,
            TraysAdvanced,
            TraysHarvested,
            TrayDiscarded,
            TraysDiscarded,
            RecountApplied,
            Undo,
            DevBackdated,
            AttentionResolved,
            StripeSessionPaid,
            StripeRefunded,
            StripeDisputed,
            SnapshotTaken,
        ];
        for k in all {
            assert_eq!(events::Kind::parse(k.as_str()).unwrap(), k);
        }
    }

    #[test]
    fn flush_guard_rejects_undoes_without_reverses_accepts_reverses_alone() {
        let dir = temp_farm_dir();
        let conn = mem();
        insert_unguarded_event(
            &conn,
            "bad-undoes-only",
            "undo",
            Some("farm_os"),
            Some("grow"),
            None,
        );
        // Force undoes_seq without reverses_event_id.
        db::drop_event_log_triggers(&conn).unwrap();
        conn.execute(
            "UPDATE event_log SET undoes_seq = 1, reverses_event_id = NULL
             WHERE id = 'bad-undoes-only'",
            [],
        )
        .unwrap();
        db::install_v9_event_log_triggers(&conn).unwrap();
        let err = event_file::flush_events(&conn, &dir).unwrap_err();
        assert!(err.contains("undoes_seq") || err.contains("Ruling 4"));

        let dir2 = temp_farm_dir();
        let conn2 = mem();
        // Seed a paid session event id for reverses target shape isn't required by guard.
        db::drop_event_log_triggers(&conn2).unwrap();
        conn2
            .execute(
                "INSERT INTO event_log
                 (id, kind, entity_type, entity_id, payload, inverse, created_at,
                  origin, event_domain, event_class, reverses_event_id, undoes_seq)
                 VALUES ('refund-only', 'stripe.refunded', 'order', 'o1', '{}', '{\"op\":\"none\"}',
                         '2026-08-06T00:00:00.000Z', 'farm_os', 'register', 'sale_farm_os_path',
                         'some-paid-id', NULL)",
                [],
            )
            .unwrap();
        db::install_v9_event_log_triggers(&conn2).unwrap();
        // May abort on other guard issues if empty origin on other rows — only this row exists.
        let result = event_file::flush_events(&conn2, &dir2);
        assert!(
            result.is_ok(),
            "reverses_event_id alone must be accepted: {result:?}"
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn verify_replay_prints_exclusion_list() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(!outcome.exit_nonzero());
        match &outcome {
            projection::VerifyOutcome::Pass { report }
            | projection::VerifyOutcome::PassWithKnown { report } => {
                assert!(!report.exclusions.is_empty());
                assert!(report.exclusions.iter().any(|e| e.contains("growth_days")));
                assert!(report.exclusions.iter().any(|e| e.contains("snapshot")));
                assert!(report.exclusions.iter().any(|e| e.contains("attention")));
            }
            projection::VerifyOutcome::Fail { .. } => panic!("unexpected fail"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_replay_attention_resolved_is_noop() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        attention::raise(
            &conn,
            "test.prompt",
            Some("crop"),
            Some("dun-peas"),
            "test attention",
            &["dismiss"],
        )
        .unwrap();
        let items = attention::check_attention(&conn).unwrap();
        assert_eq!(items.len(), 1);
        attention::dismiss_attention(&mut conn, &items[0].id).unwrap();
        flush_dir(&conn, &dir);
        drop(conn);
        let outcome = verify_dir(&dir);
        assert!(
            !outcome.exit_nonzero(),
            "verify-replay failed: {}",
            outcome.summary_line()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_paid_session_client_reference_survives_replay() {
        let (dir, mut conn) = file_farm();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let mut session = paid_session(&conn, "cs_cr_fix", &hd, "dun-peas", 1);
        session.client_reference = Some("cart_after_fix".into());
        money::apply_paid_session(&mut conn, &session).unwrap();
        flush_dir(&conn, &dir);
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
        let payload: String = conn
            .query_row(
                "SELECT payload FROM event_log WHERE kind = 'stripe.session_paid'
                 ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            payload.contains("clientReference") && payload.contains("cart_after_fix"),
            "canonical writer must emit clientReference: {payload}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn divergence_not_in_ledger_fails_verify_replay() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        // Mutate an in-scope field on a row that is NOT in the ledger.
        conn.execute("UPDATE trays SET quantity = 42", []).unwrap();
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(outcome.exit_nonzero());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ledgered_divergence_reports_known_and_passes() {
        let (dir, mut conn) = file_farm();
        // Build a paid session whose order id is the ledgered PK, without
        // clientReference in the event (historical shape), while farm.db holds the ref.
        let pk = divergence::KNOWN_DIVERGENCES[0].pk;
        let event = events::EventRecord {
            seq: None,
            event_id: "ev-ledger-1".into(),
            kind: events::Kind::StripeSessionPaid,
            entity_type: "stripe_session".into(),
            entity_id: "cs_ledger".into(),
            payload: serde_json::json!({
                "orderIds": [pk],
                "sessionId": "cs_ledger",
                "paymentIntent": "pi_ledger",
                "harvestDate": "2026-08-20",
                "lines": [{
                    "orderId": pk,
                    "cropId": "dun-peas",
                    "quantity": 1,
                    "amountCents": 200
                }],
                "amountCents": 200,
                "currency": "cad",
                "customerEmail": null,
                "paidAt": "2026-08-06T00:00:00.000Z"
            }),
            inverse: serde_json::json!({ "op": "none" }),
            origin: "farm_os".into(),
            event_domain: "register".into(),
            event_class: Some("sale_farm_os_path".into()),
            reverses_event_id: None,
            undoes_seq: None,
            undone_at: None,
            created_at: "2026-08-06T00:00:00.000Z".into(),
        };
        {
            let tx = conn.transaction().unwrap();
            projection::apply_event(&tx, &event).unwrap();
            events::insert_event(&tx, &event).unwrap();
            // Historical damage: farm holds client_reference the event omitted.
            tx.execute(
                "UPDATE orders SET client_reference = 'historical-ref' WHERE id = ?1",
                [pk],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        flush_dir(&conn, &dir);
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        match outcome {
            projection::VerifyOutcome::PassWithKnown { report } => {
                assert!(report.known_divergences >= 1);
                assert!(report.unknown_diffs.is_empty());
            }
            other => panic!("expected PassWithKnown, got {}", other.summary_line()),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn advance_harvest_discard_date_stamps_round_trip() {
        let (dir, mut conn) = file_farm();
        let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        trays::advance_trays(&mut conn, std::slice::from_ref(&t.id)).unwrap(); // -> light
        let light_on: String = conn
            .query_row("SELECT light_on FROM trays WHERE id = ?1", [&t.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!light_on.is_empty());
        trays::harvest_tray(&mut conn, &t.id, 3.5).unwrap();
        let harvested_on: String = conn
            .query_row(
                "SELECT harvested_on FROM trays WHERE id = ?1",
                [&t.id],
                |r| r.get(0),
            )
            .unwrap();
        let t2 = trays::sow_tray(&mut conn, "kale", 1).unwrap();
        trays::discard_tray(&mut conn, &t2.id).unwrap();
        let discarded_on: String = conn
            .query_row(
                "SELECT discarded_on FROM trays WHERE id = ?1",
                [&t2.id],
                |r| r.get(0),
            )
            .unwrap();
        // Payloads must carry the stamps (Ruling 7).
        let adv_payload: String = conn
            .query_row(
                "SELECT payload FROM event_log WHERE kind = 'trays.advanced' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(adv_payload.contains("\"on\""), "{adv_payload}");
        let har_payload: String = conn
            .query_row(
                "SELECT payload FROM event_log WHERE kind = 'trays.harvested' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(har_payload.contains("harvestedOn"), "{har_payload}");
        let disc_payload: String = conn
            .query_row(
                "SELECT payload FROM event_log WHERE kind = 'tray.discarded' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(disc_payload.contains("discardedOn"), "{disc_payload}");
        flush_dir(&conn, &dir);
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
        // Stamps still present on live (and matched by replay compare).
        assert_eq!(
            harvested_on,
            conn.query_row(
                "SELECT harvested_on FROM trays WHERE id = ?1",
                [&t.id],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
        );
        assert!(!discarded_on.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn divergence_ledger_file_lists_known_pks() {
        let text = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/divergence-ledger.md"),
        )
        .unwrap();
        for d in divergence::KNOWN_DIVERGENCES {
            assert!(text.contains(d.pk), "ledger file must mention {}", d.pk);
        }
    }

    // --- Track 1 verify-replay binary follow-on (Rulings A–F) ---

    #[derive(Debug, PartialEq, Eq)]
    struct LedgerDocEntry {
        id: String,
        table: String,
        pk: String,
        column: String,
        seq_range: String,
    }

    fn parse_divergence_ledger_doc(text: &str) -> Vec<LedgerDocEntry> {
        let mut out = Vec::new();
        let mut cur_id: Option<String> = None;
        let mut table = String::new();
        let mut pk = String::new();
        let mut column = String::new();
        let mut seq_range = String::new();
        let flush = |out: &mut Vec<LedgerDocEntry>,
                     cur_id: &mut Option<String>,
                     table: &mut String,
                     pk: &mut String,
                     column: &mut String,
                     seq_range: &mut String| {
            if let Some(id) = cur_id.take() {
                out.push(LedgerDocEntry {
                    id,
                    table: std::mem::take(table),
                    pk: std::mem::take(pk),
                    column: std::mem::take(column),
                    seq_range: std::mem::take(seq_range),
                });
            }
        };
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("### ") {
                flush(
                    &mut out,
                    &mut cur_id,
                    &mut table,
                    &mut pk,
                    &mut column,
                    &mut seq_range,
                );
                cur_id = Some(rest.trim().to_string());
                continue;
            }
            if cur_id.is_none() {
                continue;
            }
            let cells: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
            if cells.len() < 3 {
                continue;
            }
            let field = cells[1];
            let value = cells[2].trim_matches('`').to_string();
            match field {
                "table" => table = value,
                "primary key" => pk = value,
                "column" => column = value,
                "seq range" => {
                    // Doc form: `12` (`stripe.session_paid`, ...)
                    let first = value.split_whitespace().next().unwrap_or("");
                    seq_range = first.trim_matches('`').to_string();
                }
                _ => {}
            }
        }
        flush(
            &mut out,
            &mut cur_id,
            &mut table,
            &mut pk,
            &mut column,
            &mut seq_range,
        );
        out
    }

    #[test]
    fn known_divergences_match_ledger_doc_entry_for_entry() {
        let text = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/divergence-ledger.md"),
        )
        .unwrap();
        let docs = parse_divergence_ledger_doc(&text);
        assert_eq!(
            docs.len(),
            divergence::KNOWN_DIVERGENCES.len(),
            "ledger doc entry count must match KNOWN_DIVERGENCES"
        );
        for (doc, code) in docs.iter().zip(divergence::KNOWN_DIVERGENCES.iter()) {
            assert_eq!(doc.id, code.id);
            assert_eq!(doc.table, code.table);
            assert_eq!(doc.pk, code.pk);
            assert_eq!(doc.column, code.column);
            assert_eq!(doc.seq_range, code.seq_range);
        }
    }

    #[test]
    fn zero_events_replayed_is_fail() {
        let (dir, conn) = file_farm();
        drop(conn);
        fs::write(event_file::events_path(&dir), "").unwrap();
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(outcome.exit_nonzero());
        assert_eq!(outcome.summary_line(), "VERIFY-REPLAY: FAIL");
        assert_eq!(outcome.report().events_replayed, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn healthy_fixture_work_counts_nonzero_and_pass() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        drop(conn);
        let outcome = projection::farm_dir_verify(&dir).unwrap();
        assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
        assert_eq!(outcome.summary_line(), "VERIFY-REPLAY: PASS");
        let r = outcome.report();
        assert!(r.events_read > 0);
        assert!(r.events_replayed > 0);
        assert!(r.tables_compared > 0);
        assert!(r.rows_compared > 0);
        let status = fs::read_to_string(dir.join("last-verify-replay.txt")).unwrap();
        assert!(status.contains("events_read="));
        assert!(status.contains("events_replayed="));
        assert!(status.contains("tables_compared="));
        assert!(status.contains("rows_compared="));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_events_jsonl_names_line_number() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        drop(conn);
        let path = event_file::events_path(&dir);
        let good = fs::read_to_string(&path).unwrap();
        let broken = format!("{good}\n{{not-json\n");
        fs::write(&path, broken).unwrap();
        let err = projection::verify_replay_paths(&dir.join("farm.db"), &path, &dir).unwrap_err();
        assert!(
            err.contains("line 2") || err.contains("line "),
            "expected line number in error, got: {err}"
        );
        assert!(err.contains("expected"), "expected operator wording: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_farm_db_clear_message_creates_nothing() {
        let dir = temp_farm_dir();
        fs::write(event_file::events_path(&dir), "").unwrap();
        let before: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        let err = projection::farm_dir_verify(&dir).unwrap_err();
        assert!(err.contains("farm.db"), "{err}");
        assert!(err.contains(dir.to_str().unwrap_or("")), "{err}");
        let after: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            before, after,
            "must not create files when farm.db is missing"
        );
        assert!(!dir.join("last-verify-replay.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn farm_dir_verify_pass_with_known_divergences() {
        let (dir, mut conn) = file_farm();
        let pk = divergence::KNOWN_DIVERGENCES[0].pk;
        let event = events::EventRecord {
            seq: None,
            event_id: "ev-ledger-bin-1".into(),
            kind: events::Kind::StripeSessionPaid,
            entity_type: "stripe_session".into(),
            entity_id: "cs_ledger_bin".into(),
            payload: serde_json::json!({
                "orderIds": [pk],
                "sessionId": "cs_ledger_bin",
                "paymentIntent": "pi_ledger_bin",
                "harvestDate": "2026-08-20",
                "lines": [{
                    "orderId": pk,
                    "cropId": "dun-peas",
                    "quantity": 1,
                    "amountCents": 200
                }],
                "amountCents": 200,
                "currency": "cad",
                "customerEmail": null,
                "paidAt": "2026-08-06T00:00:00.000Z"
            }),
            inverse: serde_json::json!({ "op": "none" }),
            origin: "farm_os".into(),
            event_domain: "register".into(),
            event_class: Some("sale_farm_os_path".into()),
            reverses_event_id: None,
            undoes_seq: None,
            undone_at: None,
            created_at: "2026-08-06T00:00:00.000Z".into(),
        };
        {
            let tx = conn.transaction().unwrap();
            projection::apply_event(&tx, &event).unwrap();
            events::insert_event(&tx, &event).unwrap();
            tx.execute(
                "UPDATE orders SET client_reference = 'historical-ref' WHERE id = ?1",
                [pk],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        flush_dir(&conn, &dir);
        drop(conn);
        let outcome = projection::farm_dir_verify(&dir).unwrap();
        assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
        assert!(
            outcome
                .summary_line()
                .starts_with("VERIFY-REPLAY: PASS WITH "),
            "{}",
            outcome.summary_line()
        );
        assert!(!outcome.report().matched_ledger.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn farm_dir_verify_unledgered_divergence_fails() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        conn.execute("UPDATE trays SET quantity = 42", []).unwrap();
        drop(conn);
        let outcome = projection::farm_dir_verify(&dir).unwrap();
        assert!(outcome.exit_nonzero());
        assert_eq!(outcome.summary_line(), "VERIFY-REPLAY: FAIL");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn farm_dir_verify_writes_only_last_verify_replay_txt() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        drop(conn);
        let before: std::collections::BTreeSet<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let _ = projection::farm_dir_verify(&dir).unwrap();
        // FIX A: scratch lives at snapshots/verify-<uuid>/ for the run only.
        let leftovers: Vec<String> = fs::read_dir(dir.join("snapshots"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("verify-"))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            leftovers.is_empty(),
            "verify scratch left behind: {leftovers:?}"
        );
        let after: std::collections::BTreeSet<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let added: Vec<_> = after.difference(&before).cloned().collect();
        // SQLite may materialize -wal/-shm when a READ_ONLY handle opens a WAL DB;
        // the only file we intentionally write is last-verify-replay.txt.
        assert!(
            added.iter().any(|n| n == "last-verify-replay.txt"),
            "expected last-verify-replay.txt; added={added:?}"
        );
        let unexpected: Vec<_> = added
            .iter()
            .filter(|n| {
                *n != "last-verify-replay.txt" && *n != "farm.db-wal" && *n != "farm.db-shm"
            })
            .cloned()
            .collect();
        assert!(
            unexpected.is_empty(),
            "unexpected new farm-dir entries: {unexpected:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spine_report_includes_verify_verdict_from_last_verify_file() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        let outcome = projection::farm_dir_verify(&dir).unwrap();
        let verdict = outcome.summary_line();
        event_file::write_spine_report(&conn, &dir).unwrap();
        let spine = fs::read_to_string(dir.join(event_file::SPINE_REPORT)).unwrap();
        assert!(
            spine.contains(&verdict),
            "spine-report must include verdict from last-verify-replay.txt; spine:\n{spine}"
        );
        assert!(
            spine.contains("when="),
            "spine-report must include verify timestamp"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_source_open_flags_constant_is_read_only() {
        assert_eq!(
            projection::VERIFY_SOURCE_OPEN_FLAGS,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        );
    }

    // --- Real-farm FAIL follow-on: flush lag + single clock (Rulings 1–2) ---

    #[test]
    fn nonzero_flush_lag_fails_even_when_compared_rows_match() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        // snapshot.taken is a SQL no-op on projected tables — lag without row diffs.
        let snaps = dir.join("snapshots");
        fs::create_dir_all(&snaps).unwrap();
        snapshots::take_snapshot(&mut conn, &snaps).unwrap();
        drop(conn);
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(outcome.exit_nonzero());
        assert_eq!(outcome.report().flush_lag, 1);
        assert_eq!(
            outcome.summary_line(),
            "VERIFY-REPLAY: FAIL — 1 event(s) pending flush."
        );
        assert!(
            outcome.report().unknown_diffs.is_empty(),
            "compared rows must match; lag alone fails: {:?}",
            outcome.report().unknown_diffs
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_flush_lag_with_matching_rows_is_pass() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        drop(conn);
        let outcome = projection::farm_dir_verify(&dir).unwrap();
        assert_eq!(outcome.report().flush_lag, 0);
        assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
        assert_eq!(outcome.summary_line(), "VERIFY-REPLAY: PASS");
        let status = fs::read_to_string(dir.join("last-verify-replay.txt")).unwrap();
        assert!(
            status.contains("FLUSH LAG: 0 event(s) pending"),
            "FLUSH LAG must print on PASS: {status}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rows_beyond_watermark_are_excluded_from_comparison() {
        let (dir, mut conn) = file_farm();
        trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        flush_dir(&conn, &dir);
        let snaps = dir.join("snapshots");
        fs::create_dir_all(&snaps).unwrap();
        snapshots::take_snapshot(&mut conn, &snaps).unwrap();
        let pending_id: String = conn
            .query_row(
                "SELECT id FROM event_log ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let live_max: i64 = conn
            .query_row("SELECT MAX(seq) FROM event_log", [], |r| r.get(0))
            .unwrap();
        let watermark = event_file::read_watermark(&event_file::events_path(&dir)).unwrap();
        assert!(live_max > watermark);
        drop(conn);
        let outcome = projection::verify_replay_paths(
            &dir.join("farm.db"),
            &event_file::events_path(&dir),
            &dir,
        )
        .unwrap();
        assert!(outcome.report().flush_lag > 0);
        assert!(
            !outcome
                .report()
                .unknown_diffs
                .iter()
                .any(|d| d.key == pending_id || (d.table == "event_log" && d.field == "row_count")),
            "pending event_log row must not appear as a divergence: {:?}",
            outcome.report().unknown_diffs
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Shutdown flush mirror (close-time snapshot must reach events.jsonl) ---

    #[test]
    fn shutdown_flush_clears_close_snapshot_lag() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snaps = dir.join("snapshots");
        fs::create_dir_all(&snaps).unwrap();

        // Session open (setup order after open_and_migrate).
        let mut conn = db::open_and_migrate(&farm).unwrap();
        snapshots::try_take_snapshot(&mut conn, &snaps);
        event_file::on_app_start(&conn, &dir);
        let after_open =
            projection::verify_replay_paths(&farm, &event_file::events_path(&dir), &dir).unwrap();
        assert_eq!(after_open.report().flush_lag, 0);

        // Close write WITHOUT the new flush — permanent lag of 1 (pre-fix defect).
        snapshots::try_take_snapshot(&mut conn, &snaps);
        let lagged =
            projection::verify_replay_paths(&farm, &event_file::events_path(&dir), &dir).unwrap();
        assert_eq!(lagged.report().flush_lag, 1);
        assert!(lagged.exit_nonzero());
        assert!(
            lagged.summary_line().contains("1 event(s) pending flush"),
            "{}",
            lagged.summary_line()
        );

        // Shutdown mirror clears the lag.
        event_file::on_app_shutdown(&conn, &dir);
        let after_close =
            projection::verify_replay_paths(&farm, &event_file::events_path(&dir), &dir).unwrap();
        assert_eq!(after_close.report().flush_lag, 0);
        assert!(
            !after_close.exit_nonzero(),
            "{}",
            after_close.summary_line()
        );
        assert!(after_close.report().unknown_diffs.is_empty());
        let watermark = event_file::read_watermark(&event_file::events_path(&dir)).unwrap();
        assert_eq!(watermark, after_close.report().live_max_seq);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shutdown_flush_two_sessions_no_accumulation() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snaps = dir.join("snapshots");
        fs::create_dir_all(&snaps).unwrap();

        for _ in 0..2 {
            let mut conn = db::open_and_migrate(&farm).unwrap();
            snapshots::try_take_snapshot(&mut conn, &snaps);
            event_file::on_app_start(&conn, &dir);
            snapshots::try_take_snapshot(&mut conn, &snaps);
            event_file::on_app_shutdown(&conn, &dir);
            drop(conn);
        }

        let conn = Connection::open(&farm).unwrap();
        db::configure(&conn).unwrap();
        let outcome =
            projection::verify_replay_paths(&farm, &event_file::events_path(&dir), &dir).unwrap();
        assert_eq!(outcome.report().flush_lag, 0);
        let watermark = event_file::read_watermark(&event_file::events_path(&dir)).unwrap();
        assert_eq!(watermark, outcome.report().live_max_seq);
        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shutdown_flush_close_snapshot_reaches_events_jsonl() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snaps = dir.join("snapshots");
        fs::create_dir_all(&snaps).unwrap();

        let mut conn = db::open_and_migrate(&farm).unwrap();
        snapshots::try_take_snapshot(&mut conn, &snaps);
        event_file::on_app_start(&conn, &dir);
        snapshots::try_take_snapshot(&mut conn, &snaps);
        event_file::on_app_shutdown(&conn, &dir);

        let (id, kind): (String, String) = conn
            .query_row(
                "SELECT id, kind FROM event_log WHERE seq = (SELECT MAX(seq) FROM event_log)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, "snapshot.taken");

        let jsonl = fs::read_to_string(event_file::events_path(&dir)).unwrap();
        assert!(
            jsonl.contains(&id),
            "newest event_log id must appear in events.jsonl"
        );

        let db_ids: HashSet<String> = conn
            .prepare("SELECT id FROM event_log")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let file_ids: HashSet<String> = jsonl
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["event_id"].as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(
            db_ids, file_ids,
            "every originated event_log id must be present in events.jsonl"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shutdown_flush_idempotent_no_duplicate_lines() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snaps = dir.join("snapshots");
        fs::create_dir_all(&snaps).unwrap();

        let mut conn = db::open_and_migrate(&farm).unwrap();
        snapshots::try_take_snapshot(&mut conn, &snaps);
        event_file::on_app_start(&conn, &dir);
        snapshots::try_take_snapshot(&mut conn, &snaps);
        event_file::on_app_shutdown(&conn, &dir);

        let path = event_file::events_path(&dir);
        let before = file_bytes(&path);
        let before_lines = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .count();

        event_file::on_app_shutdown(&conn, &dir);

        let after = file_bytes(&path);
        let after_lines = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .count();
        assert_eq!(before.len(), after.len());
        assert_eq!(before_lines, after_lines);
        event_file::verify_integrity(&conn, &dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shutdown_flush_io_failure_keeps_event_log_for_next_start() {
        let dir = temp_farm_dir();
        let farm = dir.join("farm.db");
        let snaps = dir.join("snapshots");
        fs::create_dir_all(&snaps).unwrap();

        let mut conn = db::open_and_migrate(&farm).unwrap();
        snapshots::try_take_snapshot(&mut conn, &snaps);
        event_file::on_app_start(&conn, &dir);
        snapshots::try_take_snapshot(&mut conn, &snaps);

        let snap_id: String = conn
            .query_row(
                "SELECT id FROM event_log WHERE seq = (SELECT MAX(seq) FROM event_log)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let kind: String = conn
            .query_row(
                "SELECT kind FROM event_log WHERE id = ?1",
                [&snap_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "snapshot.taken");

        event_file::force_next_flush_io_failure();
        event_file::on_app_shutdown(&conn, &dir);

        let status = event_file::read_last_flush_status(&dir);
        assert!(
            status.starts_with("aborted"),
            "expected aborted flush status, got: {status}"
        );
        let still: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE id = ?1 AND kind = 'snapshot.taken'",
                [&snap_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            still, 1,
            "failed shutdown flush must not remove event_log row"
        );

        // Next start catches up (same contract as command-layer flush failure).
        event_file::on_app_start(&conn, &dir);
        let jsonl = fs::read_to_string(event_file::events_path(&dir)).unwrap();
        assert!(jsonl.contains(&snap_id));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn handler_stamps_event_and_order_from_one_clock_read() {
        let (dir, mut conn) = file_farm();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let mut session = paid_session(&conn, "cs_one_clock", &hd, "dun-peas", 1);
        session.client_reference = Some("cart_one_clock".into());
        money::apply_paid_session(&mut conn, &session).unwrap();
        let (order_created, order_updated, event_created): (String, String, String) = conn
            .query_row(
                "SELECT o.created_at, o.updated_at, e.created_at
                 FROM orders o
                 JOIN event_log e ON e.kind = 'stripe.session_paid'
                 WHERE o.stripe_session_id = 'cs_one_clock'
                 ORDER BY e.seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            order_created, event_created,
            "order.created_at must equal event.created_at exactly"
        );
        assert_eq!(
            order_updated, event_created,
            "order.updated_at must equal event.created_at exactly"
        );
        flush_dir(&conn, &dir);
        drop(conn);
        let outcome = projection::farm_dir_verify(&dir).unwrap();
        assert!(!outcome.exit_nonzero(), "{}", outcome.summary_line());
        assert!(
            !outcome.report().unknown_diffs.iter().any(|d| {
                d.table == "orders" && (d.field == "created_at" || d.field == "updated_at")
            }),
            "post-fix order timestamps must replay byte-identical: {:?}",
            outcome.report().unknown_diffs
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tray_handler_event_created_at_equals_tray_updated_at() {
        let (_dir, mut conn) = file_farm();
        let t = trays::sow_tray(&mut conn, "dun-peas", 1).unwrap();
        let (tray_created, tray_updated, event_created): (String, String, String) = conn
            .query_row(
                "SELECT t.created_at, t.updated_at, e.created_at
                 FROM trays t
                 JOIN event_log e ON e.entity_id = t.id AND e.kind = 'tray.sown'
                 WHERE t.id = ?1",
                [&t.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(tray_created, event_created);
        assert_eq!(tray_updated, event_created);
    }

    // --- Track 2: acceptance-purchase proofs (money path) ---

    #[test]
    fn track2_same_session_twice_one_event_one_capacity_decrement() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 5).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session(&conn, "cs_track2_idem", &hd, "dun-peas", 2);

        let remaining_before = trays::remaining_for_date(&conn, &hd).unwrap();
        let first = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            first,
            crate::models::AppliedOutcome::Applied { .. }
        ));
        let remaining_after_first = trays::remaining_for_date(&conn, &hd).unwrap();
        assert_eq!(remaining_after_first, remaining_before - 2);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            1
        );

        let second = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            second,
            crate::models::AppliedOutcome::AlreadyApplied
        ));
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            1,
            "second apply must not append another event_log row"
        );
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            remaining_after_first,
            "second apply must not consume capacity again"
        );
        let sold: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(capacity_consumed), 0) FROM orders
                 WHERE stripe_session_id = 'cs_track2_idem'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sold, 2);
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 1);
    }

    #[test]
    fn track2_abandoned_session_leaves_capacity_byte_identical() {
        use money::fake::FakeGateway;

        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 4).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        seed_offer_price(&conn, &hd, "dun-peas", "price_track2_abandon", 1200);

        let before = serde_json::to_vec(&trays::capacity_by_harvest_date(&conn).unwrap()).unwrap();
        let remaining_before = trays::remaining_for_date(&conn, &hd).unwrap();

        // Checkout Session creation is the Worker's job and never touches Farm OS.
        // An unpaid / abandoned session never appears in the paid poll list, so
        // apply_paid_session is never called — capacity must stay byte-identical.
        let gw = FakeGateway::new();
        let result = poll::run_poll(&mut conn, &gw).unwrap();
        assert!(result.ok);
        assert_eq!(result.sessions_applied, 0);

        let after = serde_json::to_vec(&trays::capacity_by_harvest_date(&conn).unwrap()).unwrap();
        assert_eq!(after, before);
        assert_eq!(
            trays::remaining_for_date(&conn, &hd).unwrap(),
            remaining_before
        );
        assert_eq!(money::list_orders(&conn, None).unwrap().len(), 0);
        assert_eq!(
            trays::count_event_kind(&conn, "stripe.session_paid").unwrap(),
            0
        );
    }

    #[test]
    fn track2_confirmed_payment_one_register_sale_via_kind() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 3).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session(&conn, "cs_track2_sale", &hd, "dun-peas", 1);

        let outcome = money::apply_paid_session(&mut conn, &session).unwrap();
        assert!(matches!(
            outcome,
            crate::models::AppliedOutcome::Applied { .. }
        ));

        let rows: Vec<(String, String, Option<String>, String)> = conn
            .prepare(
                "SELECT kind, origin, event_class, event_domain FROM event_log
                 WHERE kind = 'stripe.session_paid'",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "stripe.session_paid");
        assert_eq!(rows[0].1, "farm_os");
        assert_eq!(rows[0].2.as_deref(), Some("sale_farm_os_path"));
        assert_eq!(rows[0].3, "register");

        // Kind entry point stamps the tier — same mapping the live writer uses.
        let (domain, class) = events::Kind::StripeSessionPaid.tier();
        assert_eq!(domain.as_str(), "register");
        assert_eq!(class.map(|c| c.as_str()), Some("sale_farm_os_path"));
    }

    #[test]
    fn track2_event_created_at_matches_order_timestamps() {
        let mut conn = mem();
        let t = trays::sow_tray(&mut conn, "dun-peas", 2).unwrap();
        let hd = harvest_date_for(&conn, &t.id);
        let session = paid_session(&conn, "cs_track2_clock", &hd, "dun-peas", 1);
        money::apply_paid_session(&mut conn, &session).unwrap();

        let (order_created, order_updated, event_created): (String, String, String) = conn
            .query_row(
                "SELECT o.created_at, o.updated_at, e.created_at
                 FROM orders o
                 JOIN event_log e ON e.kind = 'stripe.session_paid'
                                  AND e.entity_id = o.stripe_session_id
                 WHERE o.stripe_session_id = 'cs_track2_clock'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            order_created, event_created,
            "Ruling 2: order.created_at must equal event.created_at on the money path"
        );
        assert_eq!(
            order_updated, event_created,
            "Ruling 2: order.updated_at must equal event.created_at on the money path"
        );
    }

    /// Manual operator aid: same flush as app start. Not part of the suite.
    ///
    /// Refuses to run unless `PRAIRIE_ROOTS_OPERATOR_FLUSH_FARM` is set to an
    /// explicit farm directory. `cargo test -- --ignored` cannot reach
    /// the farm data directory (or any path) by accident.
    #[test]
    #[ignore = "writes to a real farm outbox; requires PRAIRIE_ROOTS_OPERATOR_FLUSH_FARM"]
    fn operator_flush_real_farm_outbox() {
        let farm = match std::env::var("PRAIRIE_ROOTS_OPERATOR_FLUSH_FARM") {
            Ok(p) if !p.trim().is_empty() => PathBuf::from(p.trim()),
            _ => panic!(
                "refusing operator flush: set PRAIRIE_ROOTS_OPERATOR_FLUSH_FARM to an explicit farm directory"
            ),
        };
        let db_path = farm.join("farm.db");
        assert!(
            db_path.is_file(),
            "PRAIRIE_ROOTS_OPERATOR_FLUSH_FARM must point at a farm directory containing farm.db: {}",
            farm.display()
        );
        let conn = Connection::open(&db_path).unwrap();
        db::configure(&conn).unwrap();
        let ok = event_file::flush_events(&conn, &farm).unwrap();
        eprintln!(
            "flushed lines_written={} watermark={}",
            ok.lines_written, ok.watermark
        );
        event_file::write_spine_report(&conn, &farm).unwrap();
    }
}
