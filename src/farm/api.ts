import { invoke } from "@tauri-apps/api/core";
import type {
  Asset,
  AttentionItem,
  BadDebtRow,
  BadDebtSummary,
  CapacityRow,
  CashCollected,
  CashOut,
  CashRow,
  CategoryTotal,
  CheckStatus,
  CostCategory,
  CostEvent,
  CostPerTrayOutcome,
  CoverDate,
  Crop,
  DateReachability,
  ExportResult,
  FarmCurrencyView,
  FarmLocation,
  ImportPlan,
  ImportResult,
  HarvestGroup,
  HarvestInput,
  IncomeCategory,
  IncomeCorrectionCount,
  IncomeCorrectionRow,
  IncomeRecord,
  LeftoverListingView,
  SeedReceiptView,
  SeedOnHandRow,
  InvoiceBillView,
  NetCash,
  MileageTrip,
  MoneyCorrection,
  MoneyStatus,
  UnappliedFact,
  NewPaidOrders,
  Observed,
  OfferView,
  OrderView,
  PollResult,
  ReconciliationDate,
  ShelfCapacity,
  ShopPage,
  StripeAccountPreview,
  RecountCrop,
  RecountEntry,
  RecountResult,
  ResolveResult,
  SnapshotInfo,
  TodayView,
  TrayView,
  UndoResult,
  UnpricedExposure,
  UnpricedOrderRow,
  Variety,
  VenuePackView,
  WholesaleOrderView,
  WriteOffInput,
  WriteOffRow,
  WriteOffSummary,
  PhoneCaptureView,
  AcceptedCapture,
  PhoneConfirmResult,
  PhoneCaptureDecision,
  AdminPhoneView,
  PairingView,
  RetireView,
  PhonePullView,
  DockFoldsView,
  DockPortView,
} from "./types";

export function listCrops(): Promise<Crop[]> {
  return invoke("list_crops");
}

export function updateCropSeedRate(
  cropId: string,
  seedRateOzPerTray: number | null,
): Promise<Crop> {
  return invoke("update_crop_seed_rate", {
    cropId,
    seedRateOzPerTray,
  });
}

export function addCrop(
  name: string,
  growthDays: number,
  blackoutDays: number,
  expectedYieldOz: number,
): Promise<Crop> {
  return invoke("add_crop", {
    name,
    growthDays,
    blackoutDays,
    expectedYieldOz,
  });
}

export function renameCrop(cropId: string, newName: string): Promise<Crop> {
  return invoke("rename_crop", { cropId, newName });
}

export function updateCropGrowthDays(
  cropId: string,
  growthDays: number,
  blackoutDays: number,
): Promise<Crop> {
  return invoke("update_crop_growth_days", {
    cropId,
    growthDays,
    blackoutDays,
  });
}

export function shelfCapacity(): Promise<ShelfCapacity> {
  return invoke("shelf_capacity");
}

export function setShelfCapacity(
  lightSlots: number | null,
  blackoutSlots: number | null,
): Promise<ShelfCapacity> {
  return invoke("set_shelf_capacity", { lightSlots, blackoutSlots });
}

export function shelfPressure(): Promise<string | null> {
  return invoke("shelf_pressure");
}

export function overcommitWarning(
  harvestDate: string,
  cropId: string,
  trays: number,
): Promise<string | null> {
  return invoke("overcommit_warning", { harvestDate, cropId, trays });
}

export function payAmountWarning(
  orderId: string,
  amountCents: number,
): Promise<string | null> {
  return invoke("pay_amount_warning", { orderId, amountCents });
}

export function duplicateIncomeWarning(
  source: string,
  amountCents: number,
  dateReceived: string,
): Promise<string | null> {
  return invoke("duplicate_income_warning", {
    source,
    amountCents,
    dateReceived,
  });
}

export function listTrays(): Promise<TrayView[]> {
  return invoke("list_trays");
}

export function todayView(): Promise<TodayView> {
  return invoke("today_view");
}

export function sowTray(
  cropId: string,
  quantity: number,
  seedOz?: number | null,
): Promise<TrayView> {
  return invoke("sow_tray", { cropId, quantity, seedOz: seedOz ?? null });
}

export function advanceTray(trayId: string): Promise<TrayView> {
  return invoke("advance_tray", { trayId });
}

export function advanceTrays(trayIds: string[]): Promise<void> {
  return invoke("advance_trays", { trayIds });
}

export function harvestTray(
  trayId: string,
  actualYieldOz: number,
): Promise<TrayView> {
  return invoke("harvest_tray", { trayId, actualYieldOz });
}

export function harvestTrays(
  trayIds: string[],
  actualYieldOz: number,
): Promise<void> {
  return invoke("harvest_trays", { trayIds, actualYieldOz });
}

export function harvestGroups(groups: HarvestInput[]): Promise<void> {
  return invoke("harvest_groups", { groups });
}

export function earlyHarvestGroups(
  harvestDate: string,
  cropId: string,
): Promise<HarvestGroup[]> {
  return invoke("early_harvest_groups", { harvestDate, cropId });
}

export function discardTray(trayId: string): Promise<TrayView> {
  return invoke("discard_tray", { trayId });
}

export function discardFromGroup(
  trayIds: string[],
  quantity: number,
): Promise<HarvestGroup | null> {
  return invoke("discard_from_group", { trayIds, quantity });
}

export function undoLast(): Promise<UndoResult | null> {
  return invoke("undo_last");
}

export function capacityByHarvestDate(): Promise<CapacityRow[]> {
  return invoke("capacity_by_harvest_date");
}

export function moneyStatus(): Promise<MoneyStatus> {
  return invoke("money_status");
}

export function listOrders(harvestDate?: string | null): Promise<OrderView[]> {
  return invoke("list_orders", { harvestDate: harvestDate ?? null });
}

export function previewStripeKey(key: string): Promise<StripeAccountPreview> {
  return invoke("preview_stripe_key", { key });
}

export function confirmStripeKey(key: string): Promise<MoneyStatus> {
  return invoke("confirm_stripe_key", { key });
}

export function setCheckoutEndpointUrl(url: string): Promise<MoneyStatus> {
  return invoke("set_checkout_endpoint_url", { url });
}

export function listOffers(harvestDate: string): Promise<OfferView[]> {
  return invoke("list_offers", { harvestDate });
}

export function setOffer(
  harvestDate: string,
  cropId: string,
  priceCents: number,
): Promise<OfferView> {
  return invoke("set_offer", { harvestDate, cropId, priceCents });
}

export function removeOffer(offerId: string): Promise<void> {
  return invoke("remove_offer", { offerId });
}

export function generateShopPage(): Promise<ShopPage> {
  return invoke("generate_shop_page");
}

export function openShopPageFolder(): Promise<void> {
  return invoke("open_shop_page_folder");
}

/** SHOP DOOR Job A: read-only. Writes <farm folder>/shop/index.html from the minted, unpaid leftover lots; mints nothing. Refusals verbatim. */
export function writeLeftoverShopPage(): Promise<ShopPage> {
  return invoke("write_leftover_shop_page");
}

export function pollStripe(): Promise<PollResult> {
  return invoke("poll_stripe");
}

export function takeNewPaidOrders(): Promise<NewPaidOrders> {
  return invoke("take_new_paid_orders");
}

export function reconciliation(): Promise<ReconciliationDate[]> {
  return invoke("reconciliation");
}

/** Debug-only. Absent from release builds. */
export function devBackdateTray(trayId: string, days: number): Promise<void> {
  return invoke("dev_backdate_tray", { trayId, days });
}

export function listSnapshots(): Promise<SnapshotInfo[]> {
  return invoke("list_snapshots");
}

export function takeSnapshot(): Promise<SnapshotInfo> {
  return invoke("take_snapshot");
}

export function restoreSnapshot(path: string): Promise<void> {
  return invoke("restore_snapshot", { path });
}

export function farmLocation(): Promise<FarmLocation> {
  return invoke("farm_location");
}

export function openFarmFolder(): Promise<void> {
  return invoke("open_farm_folder");
}

export function exportBundle(): Promise<ExportResult> {
  return invoke("export_bundle");
}

export function openExportFolder(path: string): Promise<void> {
  return invoke("open_export_folder", { path });
}

export function previewImport(bundlePath: string): Promise<ImportPlan> {
  return invoke("preview_import", { bundlePath });
}

export function applyImport(bundlePath: string): Promise<ImportResult> {
  return invoke("apply_import", { bundlePath });
}

export function applyCutoverImport(
  bundlePath: string,
  confirmation: string,
): Promise<ImportResult> {
  return invoke("apply_cutover_import", { bundlePath, confirmation });
}

export function recountState(): Promise<RecountCrop[]> {
  return invoke("recount_state");
}

export function applyRecount(entries: RecountEntry[]): Promise<RecountResult> {
  return invoke("apply_recount", { entries });
}

export function checkAttention(): Promise<AttentionItem[]> {
  return invoke("check_attention");
}

export function resolveAttention(
  id: string,
  action: string,
): Promise<ResolveResult> {
  return invoke("resolve_attention", { id, action });
}

export function dismissAttention(id: string): Promise<void> {
  return invoke("dismiss_attention", { id });
}

export function listCostCategories(): Promise<CostCategory[]> {
  return invoke("list_cost_categories");
}

export function receiptSourceInfo(path: string): Promise<{
  fileName: string;
  sizeBytes: number;
}> {
  return invoke("receipt_source_info", { path });
}

export function recordCost(input: {
  amountCents: number;
  payee: string;
  categoryId: string;
  datePaid: string;
  descriptor?: string | null;
  receiptSourcePath?: string | null;
}): Promise<CostEvent> {
  return invoke("record_cost", {
    amountCents: input.amountCents,
    payee: input.payee,
    categoryId: input.categoryId,
    datePaid: input.datePaid,
    descriptor: input.descriptor ?? null,
    receiptSourcePath: input.receiptSourcePath ?? null,
  });
}

export function listExpenses(): Promise<CostEvent[]> {
  return invoke("list_expenses");
}

export function listMoneyCorrections(): Promise<MoneyCorrection[]> {
  return invoke("list_money_corrections");
}

export function listUnappliedFacts(): Promise<UnappliedFact[]> {
  return invoke("list_unapplied_facts");
}

export function correctExpense(input: {
  targetEventId: string;
  amountCents: number;
  payee: string;
  categoryId: string;
  datePaid: string;
  descriptor?: string | null;
  reason?: string | null;
}): Promise<CostEvent> {
  return invoke("correct_expense", {
    targetEventId: input.targetEventId,
    amountCents: input.amountCents,
    payee: input.payee,
    categoryId: input.categoryId,
    datePaid: input.datePaid,
    descriptor: input.descriptor ?? null,
    reason: input.reason ?? null,
  });
}

export function voidExpense(
  targetEventId: string,
  reason?: string | null,
): Promise<void> {
  return invoke("void_expense", {
    targetEventId,
    reason: reason ?? null,
  });
}

export function listMileageTrips(): Promise<MileageTrip[]> {
  return invoke("list_mileage_trips");
}

export function recordMileageTrip(input: {
  tripDate: string;
  miles: number;
  purpose?: string | null;
}): Promise<MileageTrip> {
  return invoke("record_mileage_trip", {
    tripDate: input.tripDate,
    miles: input.miles,
    purpose: input.purpose ?? null,
  });
}

export function correctMileageTrip(input: {
  tripId: string;
  tripDate: string;
  miles: number;
  purpose?: string | null;
}): Promise<MileageTrip> {
  return invoke("correct_mileage_trip", {
    tripId: input.tripId,
    tripDate: input.tripDate,
    miles: input.miles,
    purpose: input.purpose ?? null,
  });
}

export function voidMileageTrip(tripId: string): Promise<void> {
  return invoke("void_mileage_trip", { tripId });
}

export function listAssets(): Promise<Asset[]> {
  return invoke("list_assets");
}

export function recordAsset(input: {
  description: string;
  placedInServiceOn: string;
  costCents: number;
  disposalDate?: string | null;
}): Promise<Asset> {
  return invoke("record_asset", {
    description: input.description,
    placedInServiceOn: input.placedInServiceOn,
    costCents: input.costCents,
    disposalDate: input.disposalDate ?? null,
  });
}

export function correctAsset(input: {
  assetId: string;
  description: string;
  placedInServiceOn: string;
  costCents: number;
  disposalDate?: string | null;
}): Promise<Asset> {
  return invoke("correct_asset", {
    assetId: input.assetId,
    description: input.description,
    placedInServiceOn: input.placedInServiceOn,
    costCents: input.costCents,
    disposalDate: input.disposalDate ?? null,
  });
}

export function voidAsset(assetId: string): Promise<void> {
  return invoke("void_asset", { assetId });
}

export function listIncomeCategories(): Promise<IncomeCategory[]> {
  return invoke("list_income_categories");
}

export function listIncome(): Promise<IncomeRecord[]> {
  return invoke("list_income");
}

export function recordIncome(input: {
  amountCents: number;
  source: string;
  categoryId: string;
  dateReceived: string;
  descriptor?: string | null;
  receiptSourcePath?: string | null;
  duplicateAck: boolean;
}): Promise<IncomeRecord> {
  return invoke("record_income", {
    amountCents: input.amountCents,
    source: input.source,
    categoryId: input.categoryId,
    dateReceived: input.dateReceived,
    descriptor: input.descriptor ?? null,
    receiptSourcePath: input.receiptSourcePath ?? null,
    duplicateAck: input.duplicateAck,
  });
}

export function correctIncome(input: {
  incomeId: string;
  amountCents: number;
  source: string;
  categoryId: string;
  dateReceived: string;
  descriptor?: string | null;
  receiptSourcePath?: string | null;
}): Promise<IncomeRecord> {
  return invoke("correct_income", {
    incomeId: input.incomeId,
    amountCents: input.amountCents,
    source: input.source,
    categoryId: input.categoryId,
    dateReceived: input.dateReceived,
    descriptor: input.descriptor ?? null,
    receiptSourcePath: input.receiptSourcePath ?? null,
  });
}

export function voidIncome(incomeId: string): Promise<void> {
  return invoke("void_income", { incomeId });
}

export function costPerTray(input: {
  window: string;
  from?: string | null;
  to?: string | null;
  categoryIds?: string[] | null;
}): Promise<CostPerTrayOutcome> {
  return invoke("cost_per_tray", {
    window: input.window,
    from: input.from ?? null,
    to: input.to ?? null,
    categoryIds: input.categoryIds ?? null,
  });
}

export function storefrontReading(): Promise<Observed<Variety[]> | null> {
  return invoke("storefront_reading");
}

export function refreshStorefront(): Promise<Observed<Variety[]> | null> {
  return invoke("refresh_storefront");
}

export function healthStatus(): Promise<CheckStatus[]> {
  return invoke("health_status");
}

export function dockFolds(): Promise<DockFoldsView> {
  return invoke("dock_folds");
}

export function dockPortStart(): Promise<DockPortView> {
  return invoke("dock_port_start");
}

export function dockPortStop(): Promise<DockPortView> {
  return invoke("dock_port_stop");
}

export function dockPortStatus(): Promise<DockPortView> {
  return invoke("dock_port_status");
}

export function runFullVerify(): Promise<CheckStatus> {
  return invoke("run_full_verify");
}

export function marketingSummary(): Promise<import("./types").MarketingSummary> {
  return invoke("marketing_summary");
}

export function listVenues(
  includeArchived?: boolean,
): Promise<import("./types").VenueView[]> {
  return invoke("list_venues", { includeArchived: includeArchived ?? false });
}

export function recordVenue(input: {
  name: string;
  venueType: string;
  contact?: string | null;
  phone?: string | null;
  address?: string | null;
  note?: string | null;
}): Promise<import("./types").VenueView> {
  return invoke("record_venue", {
    name: input.name,
    venueType: input.venueType,
    contact: input.contact ?? null,
    phone: input.phone ?? null,
    address: input.address ?? null,
    note: input.note ?? null,
  });
}

export function correctVenue(input: {
  venueId: string;
  name: string;
  venueType: string;
  contact?: string | null;
  phone?: string | null;
  address?: string | null;
  note?: string | null;
}): Promise<import("./types").VenueView> {
  return invoke("correct_venue", {
    venueId: input.venueId,
    name: input.name,
    venueType: input.venueType,
    contact: input.contact ?? null,
    phone: input.phone ?? null,
    address: input.address ?? null,
    note: input.note ?? null,
  });
}

/** ONE set of bytes with the backend gate (marketing::QR_FIELDS_REFUSAL). */
export function sampleGateLine(): Promise<string> {
  return invoke("sample_gate_line");
}
export function decideStandingRequest(
  requestId: string,
  outcome: "accepted" | "dismissed",
): Promise<import("./types").StandingRequestDecision> {
  return invoke("decide_standing_request", { requestId, outcome });
}
/** Debug builds only — the command does not exist in release. */
export function devSeedStandingRequest(
  token: string,
  bagsPerCycle: number,
): Promise<import("./types").StandingRequestView> {
  return invoke("dev_seed_standing_request", { token, bagsPerCycle });
}
export function qrLinkForToken(token: string): Promise<string> {
  return invoke("qr_link_for_token", { token });
}
export function standingPullView(): Promise<import("./types").StandingPullView> {
  return invoke("standing_pull_view");
}
export function pullStandingRequests(): Promise<import("./types").StandingPullView> {
  return invoke("pull_standing_requests");
}
export function harvestCommitments(cropId: string): Promise<import("./types").HarvestCommitmentsView> {
  return invoke("harvest_commitments", { cropId });
}
export function recordHarvestCoverage(
  cropId: string,
  covers: import("./types").CoverageRef[],
): Promise<import("./types").HarvestCommitmentsView> {
  return invoke("record_harvest_coverage", { cropId, covers });
}

export function dropSample(input: {
  venueId: string;
  droppedOn: string;
  varieties: string[];
  packCount: number;
  note?: string | null;
}): Promise<import("./types").SampleView> {
  return invoke("drop_sample", {
    venueId: input.venueId,
    droppedOn: input.droppedOn,
    varieties: input.varieties,
    packCount: input.packCount,
    note: input.note ?? null,
  });
}

export function logTouch(input: {
  venueId: string;
  touchedOn: string;
  channel: string;
  outcome?: string | null;
  note?: string | null;
}): Promise<import("./types").TouchView> {
  return invoke("log_touch", {
    venueId: input.venueId,
    touchedOn: input.touchedOn,
    channel: input.channel,
    outcome: input.outcome ?? null,
    note: input.note ?? null,
  });
}

export function listOpenFollowups(): Promise<
  import("./types").FollowupView[]
> {
  return invoke("list_open_followups");
}

export function listSamples(): Promise<import("./types").SampleView[]> {
  return invoke("list_samples");
}

export function resolveFollowup(input: {
  attentionId: string;
  channel: string;
  note?: string | null;
}): Promise<import("./types").TouchView> {
  return invoke("resolve_followup", {
    attentionId: input.attentionId,
    channel: input.channel,
    note: input.note ?? null,
  });
}

export function changeStage(input: {
  venueId: string;
  stage: string;
  changedOn: string;
  traysWeek?: number | null;
  varieties?: string[] | null;
  varietyTargets?: Record<string, number> | null;
  note?: string | null;
}): Promise<import("./types").StageView> {
  return invoke("change_stage", {
    venueId: input.venueId,
    stage: input.stage,
    changedOn: input.changedOn,
    traysWeek: input.traysWeek ?? null,
    varieties: input.varieties ?? null,
    varietyTargets: input.varietyTargets ?? null,
    note: input.note ?? null,
  });
}

export function observeReviews(input: {
  observedOn: string;
  count: number;
  source: string;
}): Promise<import("./types").ReviewObservationView> {
  return invoke("observe_reviews", {
    observedOn: input.observedOn,
    count: input.count,
    source: input.source,
  });
}

export function listStages(): Promise<import("./types").StageView[]> {
  return invoke("list_stages");
}

export function weeklyActions(): Promise<import("./types").WeeklyActionsView> {
  return invoke("weekly_actions");
}

export function reputationCounts(): Promise<
  import("./types").ReputationCounts
> {
  return invoke("reputation_counts");
}

export function capacitySight(): Promise<
  Observed<import("./types").CapacitySight>
> {
  return invoke("capacity_sight");
}

export function latestReview(): Promise<
  import("./types").ReviewObservationView | null
> {
  return invoke("latest_review");
}

export function recordReviewRequest(input: {
  venueId: string;
  outcome: "asked" | "skipped";
}): Promise<import("./types").ReviewRequestView> {
  return invoke("record_review_request", {
    venueId: input.venueId,
    outcome: input.outcome,
  });
}

export function listReviewRequests(): Promise<
  import("./types").ReviewRequestView[]
> {
  return invoke("list_review_requests");
}

export function gbpVerifiedOn(): Promise<string | null> {
  return invoke("gbp_verified_on");
}

export function scanView(): Promise<import("./types").ScanView> {
  return invoke("scan_view");
}

export function scanConfig(): Promise<import("./types").ScanConfigView> {
  return invoke("scan_config");
}

export function setScanConfig(input: {
  endpointUrl: string | null;
  pullToken: string | null;
}): Promise<import("./types").ScanConfigView> {
  return invoke("set_scan_config", {
    endpointUrl: input.endpointUrl,
    pullToken: input.pullToken,
  });
}

export function pullScans(): Promise<import("./types").ScanView> {
  return invoke("pull_scans");
}

export function setGbpVerified(on: string | null): Promise<string | null> {
  return invoke("set_gbp_verified", { on });
}

export function standingDemand(): Promise<
  import("./types").StandingDemandView
> {
  return invoke("standing_demand");
}

export function listWholesaleOrders(): Promise<WholesaleOrderView[]> {
  return invoke("list_wholesale_orders");
}

/** C-2 (SOP-2): read-only. One pack per venue for a harvest date.
 *  CUT-DATE (COMMAND B): null / omitted = today, the engine's pin
 *  (the listOrders harvestDate form). */
export function packByCustomer(
  harvestDate?: string | null,
): Promise<VenuePackView[]> {
  return invoke("pack_by_customer", { harvestDate: harvestDate ?? null });
}

export function unpricedSettlementLine(): Promise<string> {
  return invoke("unpriced_settlement_line");
}

export function deliverRefusalLine(orderId: string): Promise<string | null> {
  return invoke("deliver_refusal_line", { orderId });
}

export function owedSummary(): Promise<import("./types").OwedSummary> {
  return invoke("owed_summary");
}

export function cashCollected(
  fromDate: string | null,
  toDate: string | null,
): Promise<CashCollected> {
  return invoke("cash_collected", { fromDate, toDate });
}

export function cashRows(
  fromDate: string | null,
  toDate: string | null,
): Promise<CashRow[]> {
  return invoke("cash_rows", { fromDate, toDate });
}

export function writeOffSummary(
  fromDate: string | null,
  toDate: string | null,
): Promise<WriteOffSummary> {
  return invoke("write_off_summary", { fromDate, toDate });
}

export function writeOffRows(
  fromDate: string | null,
  toDate: string | null,
): Promise<WriteOffRow[]> {
  return invoke("write_off_rows", { fromDate, toDate });
}

export function unpricedExposure(): Promise<UnpricedExposure> {
  return invoke("unpriced_exposure");
}

export function unpricedOrders(): Promise<UnpricedOrderRow[]> {
  return invoke("unpriced_orders");
}

export function cashOut(
  fromDate: string | null,
  toDate: string | null,
): Promise<CashOut> {
  return invoke("cash_out", { fromDate, toDate });
}

export function expenseRows(
  fromDate: string | null,
  toDate: string | null,
): Promise<CostEvent[]> {
  return invoke("expense_rows", { fromDate, toDate });
}

export function netCash(
  fromDate: string | null,
  toDate: string | null,
): Promise<NetCash> {
  return invoke("net_cash", { fromDate, toDate });
}

export function badDebtSummary(
  fromDate: string | null,
  toDate: string | null,
): Promise<BadDebtSummary> {
  return invoke("bad_debt_summary", { fromDate, toDate });
}

export function badDebtRows(
  fromDate: string | null,
  toDate: string | null,
): Promise<BadDebtRow[]> {
  return invoke("bad_debt_rows", { fromDate, toDate });
}

export function cashByCategory(
  fromDate: string | null,
  toDate: string | null,
): Promise<CategoryTotal[]> {
  return invoke("cash_by_category", { fromDate, toDate });
}

export function incomeCorrectionsCount(
  fromDate: string | null,
  toDate: string | null,
): Promise<IncomeCorrectionCount> {
  return invoke("income_corrections_count", { fromDate, toDate });
}

export function incomeCorrectionRows(
  fromDate: string | null,
  toDate: string | null,
): Promise<IncomeCorrectionRow[]> {
  return invoke("income_correction_rows", { fromDate, toDate });
}

export function coverPlan(): Promise<CoverDate[]> {
  return invoke("cover_plan");
}

export function reachabilityForDate(
  harvestDate: string,
  cropId: string,
): Promise<DateReachability> {
  return invoke("reachability_for_date", { harvestDate, cropId });
}

export function recordWholesaleOrder(input: {
  venueId: string;
  harvestDate: string;
  lines: {
    cropId: string;
    trays: number;
    priceCentsPerTray?: number | null;
  }[];
  overcommitAck: boolean;
}): Promise<WholesaleOrderView> {
  return invoke("record_wholesale_order", {
    venueId: input.venueId,
    harvestDate: input.harvestDate,
    lines: input.lines,
    overcommitAck: input.overcommitAck,
  });
}

export function deliverWholesaleOrder(
  orderId: string,
  deliveredOn?: string | null,
): Promise<WholesaleOrderView> {
  return invoke("deliver_wholesale_order", {
    orderId,
    deliveredOn: deliveredOn ?? null,
  });
}

export function mintWholesalePaymentLink(orderId: string): Promise<WholesaleOrderView> {
  return invoke("mint_wholesale_payment_link", { orderId });
}

/** TILL-B (GT-D22-B): read-only. The stored payment link as QR modules, row by row; true is a dark module. */
export function wholesalePaymentLinkQr(orderId: string): Promise<boolean[][]> {
  return invoke("wholesale_payment_link_qr", { orderId });
}

/** LO-A (GT-D24): read-only. */
export function listLeftoverListings(): Promise<LeftoverListingView[]> {
  return invoke("list_leftover_listings");
}

/** LO-A (GT-D24): the one leftover write. Refusals are the four sentences, verbatim. */
export function recordLeftoverListing(input: {
  cropId: string;
  harvestedOn: string;
  listedOz: number;
}): Promise<LeftoverListingView> {
  return invoke("record_leftover_listing", input);
}

/** LO-B (GT-D24-B): mint at an operator-typed price. Refusals are the four mint sentences, verbatim. */
export function mintLeftoverPaymentLink(input: {
  listingId: string;
  priceCents: number;
}): Promise<LeftoverListingView> {
  return invoke("mint_leftover_payment_link", input);
}

/** LO-C (GT-D24-C): read-only. The stored leftover payment link as QR modules, row by row; true is a dark module. */
export function leftoverPaymentLinkQr(listingId: string): Promise<boolean[][]> {
  return invoke("leftover_payment_link_qr", { listingId });
}

/** LO-B-CASH (GT-D24-B): the desk close — cash / Venmo / check ride in the descriptor. Refusals verbatim. */
export function payLeftoverListingCash(input: {
  listingId: string;
  amountCents: number;
  dateReceived: string;
  descriptor?: string | null;
}): Promise<LeftoverListingView> {
  return invoke("pay_leftover_listing_cash", {
    listingId: input.listingId,
    amountCents: input.amountCents,
    dateReceived: input.dateReceived,
    descriptor: input.descriptor ?? null,
  });
}

/** SEED-A (GT-D25): the one seed-in write. Refusals are the two sentences, verbatim. */
export function recordSeedReceived(input: {
  cropId: string;
  receivedOz: number;
}): Promise<SeedReceiptView> {
  return invoke("record_seed_received", input);
}

/** JAR-READER Job A: read-only. What is still in each crop's jar, computed at read; never stored. */
export function seedOnHand(): Promise<SeedOnHandRow[]> {
  return invoke("seed_on_hand");
}

export function payWholesaleOrder(input: {
  orderId: string;
  amountCents: number;
  dateReceived: string;
  descriptor?: string | null;
  amountAck: boolean;
  duplicateAck: boolean;
  writeOff?: WriteOffInput | null;
}): Promise<WholesaleOrderView> {
  return invoke("pay_wholesale_order", {
    orderId: input.orderId,
    amountCents: input.amountCents,
    dateReceived: input.dateReceived,
    descriptor: input.descriptor ?? null,
    amountAck: input.amountAck,
    duplicateAck: input.duplicateAck,
    writeOff: input.writeOff ?? null,
  });
}

export function settleOrderWithIncome(input: {
  orderId: string;
  incomeEventId: string;
  amountAck: boolean;
  writeOff?: WriteOffInput | null;
}): Promise<WholesaleOrderView> {
  return invoke("settle_order_with_income", {
    orderId: input.orderId,
    incomeEventId: input.incomeEventId,
    amountAck: input.amountAck,
    writeOff: input.writeOff ?? null,
  });
}

export function voidWholesaleOrder(
  orderId: string,
  reason?: string | null,
): Promise<WholesaleOrderView> {
  return invoke("void_wholesale_order", {
    orderId,
    reason: reason ?? null,
  });
}

export function reverseWholesalePayment(
  orderId: string,
  reason?: string | null,
): Promise<WholesaleOrderView> {
  return invoke("reverse_wholesale_payment", {
    orderId,
    reason: reason ?? null,
  });
}

export function badDebtConfirmLine(orderId: string): Promise<string | null> {
  return invoke("bad_debt_confirm_line", { orderId });
}

export function badDebtTrailLine(orderId: string): Promise<string | null> {
  return invoke("bad_debt_trail_line", { orderId });
}

export function writeOffBadDebt(
  orderId: string,
  confirmLine: string,
): Promise<WholesaleOrderView> {
  return invoke("write_off_bad_debt", { orderId, confirmLine });
}

export function phoneCaptures(): Promise<PhoneCaptureView[]> { return invoke("phone_captures"); }
export function confirmPhoneCaptures(accepted: AcceptedCapture[]): Promise<PhoneConfirmResult> {
  return invoke("confirm_phone_captures", { accepted });
}
export function discardPhoneCapture(proposalId: string): Promise<PhoneCaptureDecision> {
  return invoke("discard_phone_capture", { proposalId });
}
/** Debug builds only — the command does not exist in release. */
export function devSeedPhoneProposal(input: { verb: "move_to_light" | "harvest"; cropId: string; quantity: number;
  actualYieldOz: number | null; capturedDaysAgo: number; note: string | null }): Promise<PhoneCaptureView> {
  return invoke("dev_seed_phone_proposal", input);
}
export function adminPhoneStatus(): Promise<AdminPhoneView> { return invoke("admin_phone_status"); }
export function pairAdminPhone(): Promise<PairingView> { return invoke("pair_admin_phone"); }
export function retireAdminPhone(): Promise<RetireView> { return invoke("retire_admin_phone"); }
export function phonePullView(): Promise<PhonePullView> { return invoke("phone_pull_view"); }
export function pullPhoneCaptures(): Promise<PhonePullView> { return invoke("pull_phone_captures"); }
export function autoPullPhoneCaptures(): Promise<PhonePullView | null> { return invoke("auto_pull_phone_captures"); }
/** INV-A: the farm's display name — config for the invoice header, not an event. */
export function farmDisplayName(): Promise<string | null> {
  return invoke("farm_display_name");
}
export function setFarmDisplayName(name: string): Promise<string | null> {
  return invoke("set_farm_display_name", { name });
}
/** GT-D26 WORLD-PAY: the farm currency — config on farm_config, not an event. */
export function farmCurrency(): Promise<FarmCurrencyView> {
  return invoke("farm_currency");
}
export function setFarmCurrency(code: string): Promise<FarmCurrencyView> {
  return invoke("set_farm_currency", { code });
}
/** GT-D26 WORLD-PAY (ONRAMP B): the farmer's own how-to-pay line — config on farm_config, not an event. */
export function farmPayInstructions(): Promise<string | null> { return invoke("farm_pay_instructions"); }
export function setFarmPayInstructions(text: string): Promise<string | null> { return invoke("set_farm_pay_instructions", { text }); }
/** INV-A: render-only bills of a priced parent. Refusals verbatim. */
export function wholesaleInvoiceBill(orderId: string): Promise<InvoiceBillView> {
  return invoke("wholesale_invoice_bill", { orderId });
}
export function leftoverInvoiceBill(listingId: string): Promise<InvoiceBillView> {
  return invoke("leftover_invoice_bill", { listingId });
}
