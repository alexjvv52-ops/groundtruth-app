use crate::assets::{self, AssetView, CorrectAssetInput, RecordAssetInput};
use crate::attention;
use crate::categories::{self, CostCategoryView, IncomeCategoryView};
use crate::cost_per_tray::{CostPerTrayOutcome, CostPerTrayRequest};
use crate::costs::{
    self, CorrectExpenseInput, CostEventView, MoneyCorrectionView, ReceiptSourceInfo,
    RecordCostInput,
};
use crate::db::{self, Db, FarmPaths};
use crate::event_file;
use crate::export::{self, ExportResult};
use crate::health::{self, CheckStatus};
use crate::import::{self, ImportPlan, ImportResult};
use crate::income::{self, CorrectIncomeInput, IncomeView, RecordIncomeInput};
use crate::mileage::{self, CorrectMileageTripInput, MileageTripView, RecordMileageTripInput};
use crate::models::{
    AttentionItem, CapacityRow, Crop, FarmLocation, HarvestGroup, HarvestInput, MoneyStatus,
    OfferView, OrderView, ReconciliationDate, RecountCrop, RecountEntry, RecountResult,
    ResolveResult, ShelfCapacity, ShopPage, SnapshotInfo, StripeAccountPreview, TodayView,
    TrayView, UndoResult,
};
use crate::money;
use crate::observed::Observed;
use crate::offers;
use crate::poll::{self, NewPaidOrders, PollResult};
use crate::shop;
use crate::snapshots;
use crate::storefront::{self, Variety};
use crate::trays;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

/// After a committed write: best-effort events.jsonl flush. Flush failure never
/// fails the farm-day action.
fn flush_ok<T>(
    conn: &Connection,
    paths: &FarmPaths,
    result: Result<T, String>,
) -> Result<T, String> {
    if result.is_ok() {
        event_file::try_flush_after_commit(conn, &paths.folder_path);
    }
    result
}

#[tauri::command]
pub fn list_crops(state: State<'_, Db>) -> Result<Vec<Crop>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::list_crops(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_crop_seed_rate(
    state: State<'_, Db>,
    crop_id: String,
    seed_rate_oz_per_tray: Option<f64>,
) -> Result<Crop, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::update_crop_seed_rate(&conn, &crop_id, seed_rate_oz_per_tray)
}

#[tauri::command(rename_all = "camelCase")]
pub fn add_crop(
    state: State<'_, Db>,
    name: String,
    growth_days: i64,
    blackout_days: i64,
    expected_yield_oz: f64,
) -> Result<Crop, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::add_crop(&conn, &name, growth_days, blackout_days, expected_yield_oz)
}

#[tauri::command(rename_all = "camelCase")]
pub fn rename_crop(
    state: State<'_, Db>,
    crop_id: String,
    new_name: String,
) -> Result<Crop, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::rename_crop(&mut conn, &crop_id, &new_name)
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_crop_growth_days(
    state: State<'_, Db>,
    crop_id: String,
    growth_days: i64,
    blackout_days: i64,
) -> Result<Crop, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::update_crop_growth_days(&conn, &crop_id, growth_days, blackout_days)
}

#[tauri::command]
pub fn shelf_capacity(state: State<'_, Db>) -> Result<ShelfCapacity, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::shelf_capacity(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_shelf_capacity(
    state: State<'_, Db>,
    light_slots: Option<i64>,
    blackout_slots: Option<i64>,
) -> Result<ShelfCapacity, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::set_shelf_capacity(&conn, light_slots, blackout_slots)
}

#[tauri::command]
pub fn shelf_pressure(state: State<'_, Db>) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let today = db::local_date_today();
    let p = crate::reachability::shelf_pressure_on(&conn, &today)?;
    Ok(crate::reachability::shelf_pressure_line(&p))
}

#[tauri::command(rename_all = "camelCase")]
pub fn overcommit_warning(
    state: State<'_, Db>,
    harvest_date: String,
    crop_id: String,
    trays: i64,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::reachability::overcommit_line_on(
        &conn,
        &harvest_date,
        trays,
        &db::local_date_today(),
        &crop_id,
    )
}

#[tauri::command(rename_all = "camelCase")]
pub fn pay_amount_warning(
    state: State<'_, Db>,
    order_id: String,
    amount_cents: i64,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::pay_amount_line_on(&conn, &order_id, amount_cents)
}

#[tauri::command(rename_all = "camelCase")]
pub fn duplicate_income_warning(
    state: State<'_, Db>,
    source: String,
    amount_cents: i64,
    date_received: String,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::income::duplicate_income_warning(&conn, &source, amount_cents, &date_received)
}

#[tauri::command]
pub fn list_trays(state: State<'_, Db>) -> Result<Vec<TrayView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::list_trays(&conn)
}

#[tauri::command]
pub fn today_view(state: State<'_, Db>) -> Result<TodayView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::today_view(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn sow_tray(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    crop_id: String,
    quantity: i64,
    seed_oz: Option<f64>,
) -> Result<TrayView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::sow_tray_with_seed(&mut conn, &crop_id, quantity, seed_oz);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn advance_tray(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    tray_id: String,
) -> Result<TrayView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::advance_tray(&mut conn, &tray_id);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn advance_trays(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    tray_ids: Vec<String>,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::advance_trays(&mut conn, &tray_ids);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn harvest_tray(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    tray_id: String,
    actual_yield_oz: f64,
) -> Result<TrayView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::harvest_tray(&mut conn, &tray_id, actual_yield_oz);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn harvest_trays(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    tray_ids: Vec<String>,
    actual_yield_oz: f64,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::harvest_trays(&mut conn, &tray_ids, actual_yield_oz);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn harvest_groups(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    groups: Vec<HarvestInput>,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::harvest_groups(&mut conn, &groups);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn early_harvest_groups(
    state: State<'_, Db>,
    harvest_date: String,
    crop_id: String,
) -> Result<Vec<HarvestGroup>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let today = db::local_date_today();
    trays::early_harvest_groups_for(&conn, &harvest_date, &crop_id, &today)
}

#[tauri::command(rename_all = "camelCase")]
pub fn discard_tray(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    tray_id: String,
) -> Result<TrayView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::discard_tray(&mut conn, &tray_id);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn discard_from_group(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    tray_ids: Vec<String>,
    quantity: i64,
) -> Result<Option<HarvestGroup>, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::discard_from_group(&mut conn, &tray_ids, quantity);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn undo_last(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<Option<UndoResult>, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::undo_last(&mut conn);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn capacity_by_harvest_date(state: State<'_, Db>) -> Result<Vec<CapacityRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::capacity_by_harvest_date(&conn)
}

#[tauri::command]
pub fn money_status(state: State<'_, Db>) -> Result<MoneyStatus, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    money::money_status(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_orders(
    state: State<'_, Db>,
    harvest_date: Option<String>,
) -> Result<Vec<OrderView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    money::list_orders(&conn, harvest_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn preview_stripe_key(
    state: State<'_, Db>,
    key: String,
) -> Result<StripeAccountPreview, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    money::preview_stripe_key(&conn, &key)
}

#[tauri::command(rename_all = "camelCase")]
pub fn confirm_stripe_key(state: State<'_, Db>, key: String) -> Result<MoneyStatus, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    money::confirm_stripe_key(&conn, &key)
}

/// INV-A: the farm's display name for the invoice header. Config, not a Kind.
#[tauri::command]
pub fn farm_display_name(state: State<'_, Db>) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::invoice::farm_display_name(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_farm_display_name(state: State<'_, Db>, name: String) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::invoice::set_farm_display_name(&conn, &name)
}

/// INV-A: render-only bills. Refusals are the invoice sentences, verbatim.
#[tauri::command(rename_all = "camelCase")]
pub fn wholesale_invoice_bill(
    state: State<'_, Db>,
    order_id: String,
) -> Result<crate::invoice::InvoiceBillView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::invoice::wholesale_bill(&conn, &order_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn leftover_invoice_bill(
    state: State<'_, Db>,
    listing_id: String,
) -> Result<crate::invoice::InvoiceBillView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::invoice::leftover_bill(&conn, &listing_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_checkout_endpoint_url(state: State<'_, Db>, url: String) -> Result<MoneyStatus, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    money::set_checkout_endpoint_url(&conn, &url)
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_offers(state: State<'_, Db>, harvest_date: String) -> Result<Vec<OfferView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    offers::list_offers(&conn, &harvest_date)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_offer(
    state: State<'_, Db>,
    harvest_date: String,
    crop_id: String,
    price_cents: i64,
) -> Result<OfferView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    offers::set_offer(&mut conn, &harvest_date, &crop_id, price_cents)
}

#[tauri::command(rename_all = "camelCase")]
pub fn remove_offer(state: State<'_, Db>, offer_id: String) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    offers::remove_offer(&mut conn, &offer_id)
}

#[tauri::command]
pub fn generate_shop_page(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<ShopPage, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    shop::generate_shop_page(&mut conn, &paths.folder_path)
}

#[tauri::command]
pub fn open_shop_page_folder(app: AppHandle, paths: State<'_, FarmPaths>) -> Result<(), String> {
    let dir = shop::shop_dir(&paths.folder_path);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener()
        .reveal_item_in_dir(&dir)
        .map_err(|e| e.to_string())
}

/// SHOP DOOR Job A (WRITE A: viewer only). Composes the page of minted, unpaid
/// leftover lots and writes <farm folder>/shop/index.html — the cart page's
/// path, reused. Read-only on the farm: no event, no offer, no harvest link,
/// no checkout address, no Stripe call, no flush. Refusals are INV-A's farm
/// name sentence and the cart page's budget / key sentences, verbatim.
#[tauri::command]
pub fn write_leftover_shop_page(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<ShopPage, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    shop::write_leftover_shop_page(&conn, &paths.folder_path)
}

/// Poll Stripe: paid sessions, then refunds, then disputes. Never blocks the UI
/// caller should fire-and-forget; never panics.
#[tauri::command]
pub fn poll_stripe(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<PollResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = poll::run_poll_from_db(&mut conn);
    flush_ok(&conn, &paths, result)
}

/// Orders arrived since the grower last opened the app. Call after poll on open.
#[tauri::command]
pub fn take_new_paid_orders(state: State<'_, Db>) -> Result<NewPaidOrders, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    poll::take_new_paid_orders(&conn)
}

/// Read-only capacity vs orders per harvest date. Capacity stays computed.
#[tauri::command]
pub fn reconciliation(state: State<'_, Db>) -> Result<Vec<ReconciliationDate>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    poll::reconciliation(&conn)
}

#[cfg(debug_assertions)]
#[tauri::command(rename_all = "camelCase")]
pub fn dev_backdate_tray(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    tray_id: String,
    days: i64,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::dev_backdate_tray(&mut conn, &tray_id, days);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn list_snapshots(paths: State<'_, FarmPaths>) -> Result<Vec<SnapshotInfo>, String> {
    snapshots::list_snapshots(&paths.snapshots_dir)
}

#[tauri::command]
pub fn take_snapshot(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<SnapshotInfo, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = snapshots::take_snapshot(&mut conn, &paths.snapshots_dir);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn restore_snapshot(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    path: String,
) -> Result<(), String> {
    snapshots::restore_snapshot(
        &state.0,
        &paths.farm_db_path,
        &paths.snapshots_dir,
        path.as_ref(),
    )
}

#[tauri::command]
pub fn farm_location(paths: State<'_, FarmPaths>) -> Result<FarmLocation, String> {
    let farm_db_path = paths
        .farm_db_path
        .to_str()
        .ok_or_else(|| "farm path is not valid UTF-8".to_string())?
        .to_string();
    let folder_path = paths
        .folder_path
        .to_str()
        .ok_or_else(|| "folder path is not valid UTF-8".to_string())?
        .to_string();
    let last_snapshot_at = snapshots::last_snapshot_at(&paths.snapshots_dir)?;
    Ok(FarmLocation {
        farm_db_path,
        folder_path,
        last_snapshot_at,
    })
}

#[tauri::command]
pub fn open_farm_folder(app: AppHandle, paths: State<'_, FarmPaths>) -> Result<(), String> {
    app.opener()
        .reveal_item_in_dir(&paths.folder_path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_bundle(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<ExportResult, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    export::export_bundle(&conn, &paths.folder_path)
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_export_folder(
    app: AppHandle,
    paths: State<'_, FarmPaths>,
    path: String,
) -> Result<(), String> {
    let candidate = PathBuf::from(&path);
    let farm = paths.folder_path.as_path();
    if !path_is_inside(farm, &candidate) {
        return Err("that is not an export folder".into());
    }
    app.opener()
        .reveal_item_in_dir(&candidate)
        .map_err(|e| e.to_string())
}

fn path_is_inside(root: &Path, candidate: &Path) -> bool {
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let cand_canon = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.to_path_buf());
    cand_canon.starts_with(&root_canon)
}

#[tauri::command(rename_all = "camelCase")]
pub fn preview_import(state: State<'_, Db>, bundle_path: String) -> Result<ImportPlan, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    import::preview_import(&conn, Path::new(&bundle_path))
}

#[tauri::command(rename_all = "camelCase")]
pub fn apply_import(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    bundle_path: String,
) -> Result<ImportResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = import::apply_import(&mut conn, Path::new(&bundle_path));
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn apply_cutover_import(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    bundle_path: String,
    confirmation: String,
) -> Result<ImportResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = import::apply_cutover_import(&mut conn, Path::new(&bundle_path), &confirmation);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn recount_state(state: State<'_, Db>) -> Result<Vec<RecountCrop>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    trays::recount_state(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn apply_recount(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    entries: Vec<RecountEntry>,
) -> Result<RecountResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = trays::apply_recount(&mut conn, &entries);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn check_attention(state: State<'_, Db>) -> Result<Vec<AttentionItem>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    attention::check_attention(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn resolve_attention(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    id: String,
    action: String,
) -> Result<ResolveResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    if action == "try_now" {
        let kind: Option<String> = conn
            .query_row(
                "SELECT kind FROM attention WHERE id = ?1 AND resolved_at IS NULL",
                [&id],
                |r| r.get(0),
            )
            .ok();
        if kind.as_deref() == Some("poll.failed") {
            let result = poll::run_poll_from_db(&mut conn)?;
            if result.ok {
                let resolved = attention::resolve_attention(&mut conn, &id, "try_now");
                return flush_ok(&conn, &paths, resolved);
            }
            // Leave the item open — still can't reach Stripe. Poll may have written.
            event_file::try_flush_after_commit(&conn, &paths.folder_path);
            return Ok(ResolveResult {
                tray_ids: vec![],
                open_url: None,
            });
        }
        snapshots::take_snapshot(&mut conn, &paths.snapshots_dir)?;
        let resolved = attention::resolve_attention(&mut conn, &id, "try_now");
        return flush_ok(&conn, &paths, resolved);
    }
    let result = attention::resolve_attention(&mut conn, &id, &action);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn dismiss_attention(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    id: String,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = attention::dismiss_attention(&mut conn, &id);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn list_cost_categories() -> Result<Vec<CostCategoryView>, String> {
    Ok(categories::list_categories())
}

#[tauri::command(rename_all = "camelCase")]
pub fn receipt_source_info(path: String) -> Result<ReceiptSourceInfo, String> {
    costs::receipt_source_info(&path)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)] // H-7 Class A: permanent. This arg list is the IPC contract - the frontend invokes these exact names.
pub fn record_cost(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    amount_cents: i64,
    payee: String,
    category_id: String,
    date_paid: String,
    descriptor: Option<String>,
    receipt_source_path: Option<String>,
) -> Result<CostEventView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = costs::record_cost(
        &mut conn,
        &paths.folder_path,
        RecordCostInput {
            amount_cents,
            payee,
            category_id,
            date_paid,
            descriptor,
            receipt_source_path,
        },
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn list_expenses(state: State<'_, Db>) -> Result<Vec<CostEventView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    costs::list_expenses(&conn)
}

#[tauri::command]
pub fn list_money_corrections(state: State<'_, Db>) -> Result<Vec<MoneyCorrectionView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    costs::list_money_corrections(&conn)
}

#[tauri::command]
pub fn list_unapplied_facts(state: State<'_, Db>) -> Result<Vec<money::UnappliedFactView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    money::list_unapplied_facts(&conn)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)] // H-7 Class A: permanent. This arg list is the IPC contract - the frontend invokes these exact names.
pub fn correct_expense(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    target_event_id: String,
    amount_cents: i64,
    payee: String,
    category_id: String,
    date_paid: String,
    descriptor: Option<String>,
    reason: Option<String>,
) -> Result<CostEventView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = costs::correct_expense(
        &mut conn,
        CorrectExpenseInput {
            target_event_id,
            amount_cents,
            payee,
            category_id,
            date_paid,
            descriptor,
            reason,
        },
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn void_expense(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    target_event_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = costs::void_expense(&mut conn, &target_event_id, reason);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn cost_per_tray(
    state: State<'_, Db>,
    window: String,
    from: Option<String>,
    to: Option<String>,
    category_ids: Option<Vec<String>>,
) -> Result<CostPerTrayOutcome, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::cost_per_tray::cost_per_tray(
        &conn,
        CostPerTrayRequest {
            window,
            from,
            to,
            category_ids,
        },
    )
}

#[tauri::command]
pub fn list_mileage_trips(state: State<'_, Db>) -> Result<Vec<MileageTripView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    mileage::list_trips(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn record_mileage_trip(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    trip_date: String,
    miles: f64,
    purpose: Option<String>,
) -> Result<MileageTripView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = mileage::record_trip(
        &mut conn,
        RecordMileageTripInput {
            trip_date,
            miles,
            purpose,
        },
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn correct_mileage_trip(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    trip_id: String,
    trip_date: String,
    miles: f64,
    purpose: Option<String>,
) -> Result<MileageTripView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = mileage::correct_trip(
        &mut conn,
        CorrectMileageTripInput {
            trip_id,
            trip_date,
            miles,
            purpose,
        },
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn void_mileage_trip(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    trip_id: String,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = mileage::void_trip(&mut conn, &trip_id);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn list_assets(state: State<'_, Db>) -> Result<Vec<AssetView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    assets::list_assets(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn record_asset(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    description: String,
    placed_in_service_on: String,
    cost_cents: i64,
    disposal_date: Option<String>,
) -> Result<AssetView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = assets::record_asset(
        &mut conn,
        RecordAssetInput {
            description,
            placed_in_service_on,
            cost_cents,
            disposal_date,
        },
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn correct_asset(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    asset_id: String,
    description: String,
    placed_in_service_on: String,
    cost_cents: i64,
    disposal_date: Option<String>,
) -> Result<AssetView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = assets::correct_asset(
        &mut conn,
        CorrectAssetInput {
            asset_id,
            description,
            placed_in_service_on,
            cost_cents,
            disposal_date,
        },
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn void_asset(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    asset_id: String,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = assets::void_asset(&mut conn, &asset_id);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn list_income_categories() -> Result<Vec<IncomeCategoryView>, String> {
    Ok(categories::list_income_categories())
}

#[tauri::command]
pub fn list_income(state: State<'_, Db>) -> Result<Vec<IncomeView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    income::list_income(&conn)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)] // H-7 Class A: permanent. This arg list is the IPC contract - the frontend invokes these exact names.
pub fn record_income(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    amount_cents: i64,
    source: String,
    category_id: String,
    date_received: String,
    descriptor: Option<String>,
    receipt_source_path: Option<String>,
    duplicate_ack: bool,
) -> Result<IncomeView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = income::record_income(
        &mut conn,
        &paths.folder_path,
        RecordIncomeInput {
            amount_cents,
            source,
            category_id,
            date_received,
            descriptor,
            receipt_source_path,
        },
        duplicate_ack,
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)] // H-7 Class A: permanent. This arg list is the IPC contract - the frontend invokes these exact names.
pub fn correct_income(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    income_id: String,
    amount_cents: i64,
    source: String,
    category_id: String,
    date_received: String,
    descriptor: Option<String>,
    receipt_source_path: Option<String>,
) -> Result<IncomeView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = income::correct_income(
        &mut conn,
        &paths.folder_path,
        CorrectIncomeInput {
            income_id,
            amount_cents,
            source,
            category_id,
            date_received,
            descriptor,
            receipt_source_path,
        },
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn void_income(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    income_id: String,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = income::void_income(&mut conn, &income_id);
    flush_ok(&conn, &paths, result)
}

/// Cache read only — never hits the network.
#[tauri::command]
pub fn storefront_reading(state: State<'_, Db>) -> Result<Option<Observed<Vec<Variety>>>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    storefront::latest_ok_reading(&conn)
}

/// Fetch, record one observation row, return the newest ok reading (may be older).
#[tauri::command]
pub fn refresh_storefront(state: State<'_, Db>) -> Result<Option<Observed<Vec<Variety>>>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let now = db::utc_now_rfc3339();
    let reading = storefront::fetch_and_record(&conn, &now)?;
    let _ = health::record_h1_from_reading(&conn, &now, &reading);
    storefront::latest_ok_reading(&conn)
}

/// Computed at call time — status is never stored.
#[tauri::command]
pub fn health_status(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<Vec<CheckStatus>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let now = db::utc_now_rfc3339();
    let today = chrono::Local::now().date_naive();
    health::compute_status(&conn, &paths.folder_path, &paths.snapshots_dir, &now, today)
}

/// The three folds. Read-only: grouping, worst-of, surface+rank. No writes.
#[tauri::command]
pub fn dock_folds(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<crate::dock_folds::DockFoldsView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    // FI-1 - one wall-clock read, same instant for both.
    let now = db::utc_now_rfc3339();
    let today = crate::dock_folds::local_date_of(&now)
        .ok_or_else(|| "served instant is not readable".to_string())?;
    crate::dock_folds::dock_folds(&conn, &paths.folder_path, &paths.snapshots_dir, &now, today)
}

/// LAN read door. Does not auto-start. Reaches the farm file from the
/// listener thread through the existing managed `Db`.
#[tauri::command]
pub fn dock_port_start(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<crate::dock_port::DockPortView, String> {
    crate::dock_port::start(
        state.0.clone(),
        paths.folder_path.clone(),
        paths.snapshots_dir.clone(),
        crate::dock_port::DOCK_BIND,
    )
}

#[tauri::command]
pub fn dock_port_stop() -> Result<crate::dock_port::DockPortView, String> {
    Ok(crate::dock_port::stop())
}

#[tauri::command]
pub fn dock_port_status() -> Result<crate::dock_port::DockPortView, String> {
    Ok(crate::dock_port::status())
}

/// One-button full verify-replay; records H4 evidence.
#[tauri::command]
pub fn run_full_verify(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<CheckStatus, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let now = db::utc_now_rfc3339();
    health::run_full_verify(&conn, &paths.farm_db_path, &paths.folder_path, &now)
}

#[tauri::command]
pub fn marketing_summary(
    state: State<'_, Db>,
) -> Result<crate::marketing::MarketingSummary, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::marketing_summary(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_venues(
    state: State<'_, Db>,
    include_archived: Option<bool>,
) -> Result<Vec<crate::marketing::VenueView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::list_venues(&conn, include_archived.unwrap_or(false))
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)] // H-7 Class A: permanent. This arg list is the IPC contract - the frontend invokes these exact names.
pub fn record_venue(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    name: String,
    venue_type: String,
    contact: Option<String>,
    phone: Option<String>,
    address: Option<String>,
    note: Option<String>,
) -> Result<crate::marketing::VenueView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::record_venue(
        &mut conn,
        &name,
        &venue_type,
        contact,
        phone,
        address,
        note,
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn drop_sample(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    venue_id: String,
    dropped_on: String,
    varieties: Vec<String>,
    pack_count: i64,
    note: Option<String>,
) -> Result<crate::marketing::SampleView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::drop_sample(
        &mut conn,
        &venue_id,
        &dropped_on,
        varieties,
        pack_count,
        note,
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)] // H-7 Class A: permanent. This arg list is the IPC contract - the frontend invokes these exact names.
pub fn correct_venue(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    venue_id: String,
    name: String,
    venue_type: String,
    contact: Option<String>,
    phone: Option<String>,
    address: Option<String>,
    note: Option<String>,
) -> Result<crate::marketing::VenueView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::correct_venue(
        &mut conn,
        &venue_id,
        &name,
        &venue_type,
        contact,
        phone,
        address,
        note,
    );
    flush_ok(&conn, &paths, result)
}

/// ONE set of bytes for the sample-drop gate (marketing::QR_FIELDS_REFUSAL) —
/// the unpriced_settlement_line precedent.
#[tauri::command]
pub fn sample_gate_line() -> Result<String, String> {
    Ok(crate::marketing::QR_FIELDS_REFUSAL.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn decide_standing_request(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    request_id: String,
    outcome: String,
) -> Result<crate::marketing::StandingRequestDecision, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::decide_standing_request(&mut conn, &request_id, &outcome);
    flush_ok(&conn, &paths, result)
}

/// GT-D18: read-only link composition. Never writes, never pushes.
#[tauri::command(rename_all = "camelCase")]
pub fn qr_link_for_token(state: State<'_, Db>, token: String) -> Result<String, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::qr_link_for_token(&conn, &token)
}

#[tauri::command]
pub fn standing_pull_view(
    state: State<'_, Db>,
) -> Result<crate::standing_pull::StandingPullView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::standing_pull::latest_view(&conn)
}

/// Pull, ingest through marketing::ingest_standing_request, then return the
/// evaluator's view — never a second composition. Writes marketing events, so
/// the partition flushes.
#[tauri::command]
pub fn pull_standing_requests(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<crate::standing_pull::StandingPullView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let now = db::utc_now_rfc3339();
    let result = crate::standing_pull::pull(&mut conn, &now);
    flush_ok(&conn, &paths, result)
}

/// R3: read-only commitments for one crop, today.
#[tauri::command(rename_all = "camelCase")]
pub fn harvest_commitments(
    state: State<'_, Db>,
    crop_id: String,
) -> Result<crate::marketing::HarvestCommitmentsView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::harvest_commitments(&conn, &crop_id)
}
/// R3: the durable mark (GT-D19). Full current list each call.
#[tauri::command(rename_all = "camelCase")]
pub fn record_harvest_coverage(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    crop_id: String,
    covers: Vec<crate::marketing::CoverageRef>,
) -> Result<crate::marketing::HarvestCommitmentsView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::record_harvest_coverage(&mut conn, &crop_id, covers);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn phone_captures(state: State<'_, Db>) -> Result<Vec<crate::phone::PhoneCaptureView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::phone::phone_captures(&conn)
}
/// GT-D20: the Confirm gate. Writes grow events, so the partition flushes.
#[tauri::command(rename_all = "camelCase")]
pub fn confirm_phone_captures(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    accepted: Vec<crate::phone::AcceptedCapture>,
) -> Result<crate::phone::ConfirmResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::phone::confirm_phone_captures(&mut conn, &accepted);
    flush_ok(&conn, &paths, result)
}
#[tauri::command(rename_all = "camelCase")]
pub fn discard_phone_capture(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    proposal_id: String,
) -> Result<crate::phone::PhoneCaptureDecision, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::phone::discard_phone_capture(&mut conn, &proposal_id);
    flush_ok(&conn, &paths, result)
}
/// Debug builds only (rack-side fence 1, ruling 7); absent from release.
#[cfg(debug_assertions)]
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)] // H-7 Class A: permanent. This arg list is the IPC contract - the frontend invokes these exact names.
pub fn dev_seed_phone_proposal(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    verb: String,
    crop_id: String,
    quantity: i64,
    actual_yield_oz: Option<f64>,
    captured_days_ago: i64,
    note: Option<String>,
) -> Result<crate::phone::PhoneCaptureView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::phone::dev_seed_phone_proposal(
        &mut conn,
        &verb,
        &crop_id,
        quantity,
        actual_yield_oz,
        captured_days_ago,
        note,
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn admin_phone_status(
    state: State<'_, Db>,
) -> Result<crate::field_devices::AdminPhoneView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::field_devices::admin_status(&conn)
}
#[tauri::command]
pub fn pair_admin_phone(state: State<'_, Db>) -> Result<crate::field_devices::PairingView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::field_devices::pair_admin(&mut conn)
}
#[tauri::command]
pub fn retire_admin_phone(
    state: State<'_, Db>,
) -> Result<crate::field_devices::RetireView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::field_devices::retire_admin(&conn)
}
#[tauri::command]
pub fn phone_pull_view(state: State<'_, Db>) -> Result<crate::phone_pull::PhonePullView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::phone_pull::latest_view(&conn)
}
/// Manual pull. Writes grow events (phone.proposed) through the ingest, so the partition flushes.
#[tauri::command]
pub fn pull_phone_captures(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<crate::phone_pull::PhonePullView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let now = db::utc_now_rfc3339();
    let result = crate::phone_pull::pull(&mut conn, &now);
    flush_ok(&conn, &paths, result)
}
/// Automatic pull (ruling 3): no request unless a live Admin exists.
#[tauri::command]
pub fn auto_pull_phone_captures(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
) -> Result<Option<crate::phone_pull::PhonePullView>, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let now = db::utc_now_rfc3339();
    let result = crate::phone_pull::auto_pull(&mut conn, &now);
    flush_ok(&conn, &paths, result)
}

/// Debug builds only (signed 2026-08-17 fence 3 item 3a); absent from release.
#[cfg(debug_assertions)]
#[tauri::command(rename_all = "camelCase")]
pub fn dev_seed_standing_request(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    token: String,
    bags_per_cycle: i64,
) -> Result<crate::marketing::StandingRequestView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::dev_seed_standing_request(&mut conn, &token, bags_per_cycle);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn log_touch(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    venue_id: String,
    touched_on: String,
    channel: String,
    outcome: Option<String>,
    note: Option<String>,
) -> Result<crate::marketing::TouchView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result =
        crate::marketing::log_touch(&mut conn, &venue_id, &touched_on, &channel, outcome, note);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn list_open_followups(
    state: State<'_, Db>,
) -> Result<Vec<crate::marketing::FollowupView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::list_open_followups(&conn)
}

#[tauri::command]
pub fn list_samples(state: State<'_, Db>) -> Result<Vec<crate::marketing::SampleView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::list_samples(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn resolve_followup(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    attention_id: String,
    channel: String,
    note: Option<String>,
) -> Result<crate::marketing::TouchView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::resolve_followup(&mut conn, &attention_id, &channel, note);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)] // H-7 Class A: permanent. This arg list is the IPC contract - the frontend invokes these exact names.
pub fn change_stage(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    venue_id: String,
    stage: String,
    changed_on: String,
    trays_week: Option<i64>,
    varieties: Option<Vec<String>>,
    variety_targets: Option<std::collections::BTreeMap<String, i64>>,
    note: Option<String>,
) -> Result<crate::marketing::StageView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::change_stage(
        &mut conn,
        &venue_id,
        &stage,
        &changed_on,
        trays_week,
        varieties,
        variety_targets,
        note,
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn observe_reviews(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    observed_on: String,
    count: i64,
    source: String,
) -> Result<crate::marketing::ReviewObservationView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::observe_reviews(&mut conn, &observed_on, count, &source);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn list_stages(state: State<'_, Db>) -> Result<Vec<crate::marketing::StageView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::list_stages(&conn)
}

#[tauri::command]
pub fn weekly_actions(state: State<'_, Db>) -> Result<crate::marketing::WeeklyActionsView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::weekly_actions(&conn)
}

#[tauri::command]
pub fn reputation_counts(
    state: State<'_, Db>,
) -> Result<crate::marketing::ReputationCounts, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::reputation_counts(&conn)
}

#[tauri::command]
pub fn capacity_sight(
    state: State<'_, Db>,
) -> Result<crate::observed::Observed<crate::capacity_gate::CapacitySight>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::capacity_gate::capacity_observed(&conn)
}

#[tauri::command]
pub fn latest_review(
    state: State<'_, Db>,
) -> Result<Option<crate::marketing::ReviewObservationView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::latest_review(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn record_review_request(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    venue_id: String,
    outcome: String,
) -> Result<crate::marketing::ReviewRequestView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::marketing::record_review_request(&mut conn, &venue_id, &outcome);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn list_review_requests(
    state: State<'_, Db>,
) -> Result<Vec<crate::marketing::ReviewRequestView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::list_review_requests(&conn)
}

#[tauri::command]
pub fn gbp_verified_on(state: State<'_, Db>) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::gbp_verified_on(&conn)
}

#[tauri::command]
pub fn scan_view(state: State<'_, Db>) -> Result<crate::scans::ScanView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::scans::latest_view(&conn)
}

#[tauri::command]
pub fn scan_config(state: State<'_, Db>) -> Result<crate::scans::ScanConfigView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::scans::config(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_scan_config(
    state: State<'_, Db>,
    endpoint_url: Option<String>,
    pull_token: Option<String>,
) -> Result<crate::scans::ScanConfigView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::scans::set_config(&conn, endpoint_url.as_deref(), pull_token.as_deref())
}

/// Pull, then return the evaluator's view — never a second composition.
#[tauri::command]
pub fn pull_scans(state: State<'_, Db>) -> Result<crate::scans::ScanView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let now = db::utc_now_rfc3339();
    crate::scans::pull(&conn, &now)?;
    crate::scans::latest_view(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_gbp_verified(
    state: State<'_, Db>,
    on: Option<String>,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::set_gbp_verified(&conn, on.as_deref())
}

#[tauri::command]
pub fn standing_demand(
    state: State<'_, Db>,
) -> Result<crate::marketing::StandingDemandView, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::marketing::standing_demand(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn record_wholesale_order(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    venue_id: String,
    harvest_date: String,
    lines: Vec<crate::wholesale::OrderLine>,
    overcommit_ack: bool,
) -> Result<crate::wholesale::WholesaleOrderView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result =
        crate::wholesale::record_order(&mut conn, &venue_id, &harvest_date, lines, overcommit_ack);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn deliver_wholesale_order(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    order_id: String,
    delivered_on: Option<String>,
) -> Result<crate::wholesale::WholesaleOrderView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::wholesale::deliver_order(&mut conn, &order_id, delivered_on);
    flush_ok(&conn, &paths, result)
}

/// TILL-A (GT-D22): mint this farm's Payment Link for one delivered, priced
/// wholesale order. Refusals are the sentences in wholesale.rs, verbatim.
#[tauri::command(rename_all = "camelCase")]
pub fn mint_wholesale_payment_link(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    order_id: String,
) -> Result<crate::wholesale::WholesaleOrderView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::wholesale::mint_payment_link(&mut conn, &order_id);
    flush_ok(&conn, &paths, result)
}

/// TILL-B (GT-D22-B): read-only QR modules of one order's stored payment
/// link. Never writes, never pushes; the URL is the column string, unchanged.
#[tauri::command(rename_all = "camelCase")]
pub fn wholesale_payment_link_qr(
    state: State<'_, Db>,
    order_id: String,
) -> Result<Vec<Vec<bool>>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::payment_link_qr_modules(&conn, &order_id)
}

/// LO-C (GT-D24-C): read-only QR modules of one listing's stored payment
/// link. Never writes, never pushes; the URL is the column string, unchanged.
#[tauri::command(rename_all = "camelCase")]
pub fn leftover_payment_link_qr(
    state: State<'_, Db>,
    listing_id: String,
) -> Result<Vec<Vec<bool>>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::leftover::payment_link_qr_modules(&conn, &listing_id)
}

/// LO-A (GT-D24): list leftover ounces for one harvested crop-day. Refusals
/// are the four sentences in leftover.rs, verbatim. Capacity-free: no tray,
/// no order, no capacity figure moves.
#[tauri::command(rename_all = "camelCase")]
pub fn record_leftover_listing(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    crop_id: String,
    harvested_on: String,
    listed_oz: f64,
) -> Result<crate::leftover::LeftoverListingView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::leftover::list_leftover(&mut conn, &crop_id, &harvested_on, listed_oz);
    flush_ok(&conn, &paths, result)
}

/// LO-A (GT-D24): read-only. harvested_oz is computed at read, never stored.
#[tauri::command]
pub fn list_leftover_listings(
    state: State<'_, Db>,
) -> Result<Vec<crate::leftover::LeftoverListingView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::leftover::listings(&conn)
}

/// LO-B (GT-D24-B): mint a Stripe Payment Link for one leftover listing at an
/// operator-typed price. Refusals are the four mint sentences in leftover.rs,
/// verbatim. Books no money; income.received is the poll's.
#[tauri::command(rename_all = "camelCase")]
pub fn mint_leftover_payment_link(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    listing_id: String,
    price_cents: i64,
) -> Result<crate::leftover::LeftoverListingView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::leftover::mint_payment_link(&mut conn, &listing_id, price_cents);
    flush_ok(&conn, &paths, result)
}

/// LO-B-CASH (GT-D24-B): the desk close for money that arrived outside
/// Stripe. Refusals are the leftover sentences, verbatim; no write-off.
#[tauri::command(rename_all = "camelCase")]
pub fn pay_leftover_listing_cash(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    listing_id: String,
    amount_cents: i64,
    date_received: String,
    descriptor: Option<String>,
) -> Result<crate::leftover::LeftoverListingView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::leftover::pay_listing_cash(
        &mut conn,
        &listing_id,
        amount_cents,
        &date_received,
        descriptor,
    );
    flush_ok(&conn, &paths, result)
}

/// SEED-A (GT-D25): record seed of one crop arriving at the farm, in ounces.
/// Refusals are the two sentences in seed.rs, verbatim. Moves no tray, books
/// no money; the sow path's oz-out row is untouched.
#[tauri::command(rename_all = "camelCase")]
pub fn record_seed_received(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    crop_id: String,
    received_oz: f64,
) -> Result<crate::seed::SeedReceiptView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::seed::receive_seed(&mut conn, &crop_id, received_oz);
    flush_ok(&conn, &paths, result)
}

/// JAR-READER Job A: read-only. What is still in each crop's jar, computed
/// at read from the receipts and the sow path's oz-out rows joined by id;
/// never stored. No clock, no event, no flush.
#[tauri::command]
pub fn seed_on_hand(state: State<'_, Db>) -> Result<Vec<crate::seed::SeedOnHandRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::seed::seed_on_hand(&conn)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)] // H-7 Class A: permanent. This arg list is the IPC contract - the frontend invokes these exact names.
pub fn pay_wholesale_order(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    order_id: String,
    amount_cents: i64,
    date_received: String,
    descriptor: Option<String>,
    amount_ack: bool,
    duplicate_ack: bool,
    write_off: Option<crate::wholesale::WriteOffInput>,
) -> Result<crate::wholesale::WholesaleOrderView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::wholesale::pay_order(
        &mut conn,
        &order_id,
        amount_cents,
        &date_received,
        descriptor,
        amount_ack,
        duplicate_ack,
        write_off,
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn settle_order_with_income(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    order_id: String,
    income_event_id: String,
    amount_ack: bool,
    write_off: Option<crate::wholesale::WriteOffInput>,
) -> Result<crate::wholesale::WholesaleOrderView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::wholesale::settle_order_with_income(
        &mut conn,
        &order_id,
        &income_event_id,
        amount_ack,
        write_off,
    );
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn write_off_summary(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<crate::wholesale::WriteOffSummary, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::write_off_summary_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn cash_collected(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<crate::income::CashCollected, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::income::cash_collected_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn cash_rows(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<Vec<crate::income::CashRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::income::cash_rows_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn write_off_rows(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<Vec<crate::wholesale::WriteOffRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::write_off_rows_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn unpriced_exposure(
    state: State<'_, Db>,
) -> Result<crate::wholesale::UnpricedExposure, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::unpriced_exposure(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn unpriced_orders(
    state: State<'_, Db>,
) -> Result<Vec<crate::wholesale::UnpricedOrderRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::unpriced_open_orders(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn cash_out(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<crate::costs::CashOut, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::costs::cash_out_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn expense_rows(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<Vec<CostEventView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::costs::expense_rows_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn net_cash(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<crate::income::NetCash, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::income::net_cash_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn bad_debt_summary(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<crate::wholesale::BadDebtSummary, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::bad_debt_summary_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn bad_debt_rows(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<Vec<crate::wholesale::BadDebtRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::bad_debt_rows_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn cash_by_category(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<Vec<crate::income::CategoryTotal>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::income::cash_by_category_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn income_corrections_count(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<crate::income::IncomeCorrectionCount, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::income::income_correction_count_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn income_correction_rows(
    state: State<'_, Db>,
    from_date: Option<String>,
    to_date: Option<String>,
) -> Result<Vec<crate::income::IncomeCorrectionRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::income::income_correction_rows_between(&conn, from_date.as_deref(), to_date.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub fn void_wholesale_order(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    order_id: String,
    reason: Option<String>,
) -> Result<crate::wholesale::WholesaleOrderView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::wholesale::void_order(&mut conn, &order_id, reason);
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn reverse_wholesale_payment(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    order_id: String,
    reason: Option<String>,
) -> Result<crate::wholesale::WholesaleOrderView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result =
        crate::wholesale::reverse_payment(&mut conn, &order_id, reason.as_deref().unwrap_or(""));
    flush_ok(&conn, &paths, result)
}

#[tauri::command(rename_all = "camelCase")]
pub fn bad_debt_confirm_line(
    state: State<'_, Db>,
    order_id: String,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::bad_debt_confirm_line(&conn, &order_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn bad_debt_trail_line(
    state: State<'_, Db>,
    order_id: String,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::bad_debt_trail_line(&conn, &order_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn write_off_bad_debt(
    state: State<'_, Db>,
    paths: State<'_, FarmPaths>,
    order_id: String,
    confirm_line: String,
) -> Result<crate::wholesale::WholesaleOrderView, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let result = crate::wholesale::write_off_bad_debt(&mut conn, &order_id, &confirm_line);
    flush_ok(&conn, &paths, result)
}

#[tauri::command]
pub fn list_wholesale_orders(
    state: State<'_, Db>,
) -> Result<Vec<crate::wholesale::WholesaleOrderView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::list_orders(&conn)
}

/// C-2 (SOP-2): read-only. CUT-DATE (COMMAND B): an optional harvest
/// date on the same command name -- null / omitted is today, the
/// engine's pin (the `list_orders` date-arg form above).
#[tauri::command(rename_all = "camelCase")]
pub fn pack_by_customer(
    state: State<'_, Db>,
    harvest_date: Option<String>,
) -> Result<Vec<crate::wholesale::VenuePackView>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    match harvest_date.as_deref() {
        Some(date) => crate::wholesale::pack_by_customer_on(&conn, date),
        None => crate::wholesale::pack_by_customer(&conn),
    }
}

#[tauri::command]
pub fn unpriced_settlement_line() -> Result<String, String> {
    Ok(crate::wholesale::UNPRICED_SETTLEMENT_LINE.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn deliver_refusal_line(
    state: State<'_, Db>,
    order_id: String,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::deliver_refusal_line(&conn, &order_id)
}

#[tauri::command]
pub fn owed_summary(state: State<'_, Db>) -> Result<crate::wholesale::OwedSummary, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::wholesale::owed_summary(&conn)
}

#[tauri::command]
pub fn cover_plan(state: State<'_, Db>) -> Result<Vec<crate::reachability::CoverDate>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::reachability::cover_plan(&conn)
}

#[tauri::command(rename_all = "camelCase")]
pub fn reachability_for_date(
    state: State<'_, Db>,
    harvest_date: String,
    crop_id: String,
) -> Result<crate::reachability::DateReachability, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    crate::reachability::for_date_for_crop(&conn, &harvest_date, &crop_id)
}
