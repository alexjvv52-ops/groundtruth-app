export type Crop = {
  id: string;
  name: string;
  growthDays: number;
  blackoutDays: number;
  expectedYieldOz: number;
  sortOrder: number;
  /** Oz of seed per tray. null = no pre-fill proposal. */
  seedRateOzPerTray: number | null;
};

export type TrayView = {
  id: string;
  cropId: string;
  cropName: string;
  state: string;
  quantity: number;
  growthDaysAtSow: number | null;
  blackoutDaysAtSow: number | null;
  plannedOn: string | null;
  sownOn: string | null;
  blackoutOn: string | null;
  lightOn: string | null;
  harvestedOn: string | null;
  discardedOn: string | null;
  actualYieldOz: number | null;
  expectedHarvestDate: string | null;
  coverCheckDate: string | null;
  createdAt: string;
  updatedAt: string;
};

export type CapacityRow = {
  harvestDate: string;
  cropId: string;
  cropName: string;
  trays: number;
  expectedYieldOz: number;
  soldTrays: number;
  remainingTrays: number;
  harvestedTrays: number;
  coverSupply: number;
  coverPromised: number;
  coverRemaining: number;
};

export type MoneyStatus = {
  configured: boolean;
  mode: string | null;
  accountName: string | null;
  lastPollOk: string | null;
  lastPollErr: string | null;
  openOrderCount: number;
  checkoutEndpointUrl: string | null;
};

export type StripeAccountPreview = {
  accountId: string;
  accountName: string;
  mode: string;
};

export type OfferView = {
  id: string | null;
  harvestDate: string;
  cropId: string;
  cropName: string;
  priceCents: number | null;
  stripePriceId: string | null;
  stripeLinkUrl: string | null;
  available: number;
  sold: number;
  remaining: number;
};

export type ShopPage = {
  filePath: string;
  sizeBytes: number;
  generatedAt: string;
  harvestDates: string[];
};

export type OrderView = {
  id: string;
  stripeSessionId: string;
  stripePaymentIntent: string | null;
  harvestDate: string;
  cropId: string;
  cropName: string;
  quantity: number;
  amountCents: number;
  currency: string;
  customerEmail: string | null;
  state: string;
  capacityConsumed: number;
  clientReference: string | null;
  paidAt: string;
  createdAt: string;
  updatedAt: string;
};

export type UndoResult = {
  undoesSeq: number;
  undoneKind: string;
};

export type MoveToLight = {
  trayIds: string[];
  trayCount: number;
};

export type HarvestGroup = {
  cropId: string;
  cropName: string;
  trayIds: string[];
  trayCount: number;
  estimatedYieldOz: number;
};

export type HarvestSummary = {
  trayCount: number;
  varietyCount: number;
  estimatedYieldOz: number;
  singleCropName: string | null;
};

export type HarvestInput = {
  trayIds: string[];
  actualYieldOz: number;
};

export type NextEvent = {
  kind: "light" | "harvest" | string;
  date: string;
  trayCount: number;
  cropName: string;
};

export type TodayView = {
  moveToLight: MoveToLight | null;
  harvests: HarvestGroup[];
  harvestSummary: HarvestSummary | null;
  nextEvents: NextEvent[];
  activeTrayCount: number;
  /** Distinguishes Script A idle copy (B) from the generic next-event line (C). */
  sownToday: boolean;
};

/** Operator-facing cost category — no tax line numbers. */
export type CostCategory = {
  id: string;
  name: string;
  descriptorRequired: boolean;
};

export type CostEvent = {
  eventId: string;
  origin: string;
  datePaid: string;
  amountCents: number;
  payee: string;
  canonicalCategory: string;
  descriptor: string;
  receiptFileRef: string | null;
  lastEventId: string;
  createdAt: string;
  updatedAt: string;
};

/** Permanent money correction trail row — display only. */
export type MoneyCorrection = {
  correctionEventId: string;
  targetEventId: string;
  track: string;
  action: string;
  beforeJson: string;
  afterJson: string | null;
  beforeAmountCents: number;
  afterAmountCents: number | null;
  beforeDate: string;
  afterDate: string | null;
  beforePayee: string;
  afterPayee: string | null;
  reason: string | null;
  correctedAt: string;
};

/** A Stripe fact the register observed and could not apply. Display only. */
export type UnappliedFact = {
  eventId: string;
  stripeObject: string;
  stripeId: string;
  status: string;
  amountCents: number | null;
  currency: string | null;
  stripeCreated: number;
  observedAt: string;
};

/** One dated trip, stored in miles. There is no dollar value on this type. */
export type MileageTrip = {
  tripId: string;
  origin: string;
  tripDate: string;
  miles: number;
  purpose: string | null;
  lastEventId: string;
  createdAt: string;
  updatedAt: string;
};

/** Asset register row — four operator fields. Nothing computed. */
export type Asset = {
  assetId: string;
  origin: string;
  description: string;
  placedInServiceOn: string;
  costCents: number;
  disposalDate: string | null;
  lastEventId: string;
  createdAt: string;
  updatedAt: string;
};

export type IncomeCategory = {
  id: string;
  name: string;
  descriptorRequired: boolean;
};

/** Money-in register row — amount, source, category, date. Nothing computed. */
export type IncomeRecord = {
  incomeId: string;
  origin: string;
  dateReceived: string;
  amountCents: number;
  source: string;
  canonicalCategory: string;
  descriptor: string;
  receiptFileRef: string | null;
  lastEventId: string;
  createdAt: string;
  updatedAt: string;
};

export type WholesaleOrderLine = {
  cropId: string;
  cropName: string;
  trays: number;
  priceCentsPerTray: number | null;
};

export type OwedSummary = {
  deliveries: number;
  totalCents: number | null;
  anyUnpriced: boolean;
  oldestDays: number | null;
  /** OWED-LO: priced-unpaid leftover listings (leftover::owed_leftover). */
  leftoverCount: number;
  leftoverCents: number;
};

export type WholesaleOrderView = {
  id: string;
  venueId: string;
  venueName: string;
  harvestDate: string;
  state: string;
  orderedOn: string;
  deliveredOn: string | null;
  paidOn: string | null;
  incomeEventId: string | null;
  voidedAt: string | null;
  voidReason: string | null;
  createdAt: string;
  updatedAt: string;
  lines: WholesaleOrderLine[];
  pricedTotalCents: number | null;
  deliveredAgeDays: number | null;
  paymentLinkId: string | null;
  paymentLinkUrl: string | null;
  paymentLinkMintedAt: string | null;
};

/** C-2 (SOP-2): read-only. One pack per venue for that harvest date.
 *  ROUTE (ENGINE C / JOIN id): venueId, address and phone ride along for the
 *  run page — joined on venue_id in the engine, never on the venue name. */
export type VenuePackView = {
  venueId: string;
  venueName: string;
  harvestDate: string;
  lines: WholesaleOrderLine[];
  trayTotal: number;
  address: string | null;
  phone: string | null;
};

/** LO-A (GT-D24). harvestedOz is computed at read from the trays; never stored.
 *  LO-B (GT-D24-B): the six money fields are null until minted / paid. */
export type LeftoverListingView = {
  listingId: string;
  cropId: string;
  cropName: string;
  harvestedOn: string;
  harvestedOz: number;
  listedOz: number;
  createdAt: string;
  paymentLinkId: string | null;
  paymentLinkUrl: string | null;
  paymentLinkMintedAt: string | null;
  pricedTotalCents: number | null;
  paidSessionId: string | null;
  paidAt: string | null;
};

/** SEED-A (GT-D25). One seed.received row: crop id + ounces at 0.1, the event's createdAt.
 *  cropName is joined at read; never in the payload or the row. */
export type SeedReceiptView = {
  receiptId: string;
  cropId: string;
  cropName: string;
  receivedOz: number;
  createdAt: string;
};

/** JAR-READER Job A. One crop's jar, computed at read; never stored. Only crops
 *  with a receipt have a row. Ounces out are joined to the crop by id (never by
 *  name); the window opens at the first receipt (`since`). onHandOz is null while
 *  any sow in the window is unweighed, and negative when the jar is short.
 *  unattributedOz is farm-wide (ounces no id join can place) and repeats on every row. */
export type SeedOnHandRow = {
  cropId: string;
  cropName: string;
  receivedOz: number;
  since: string;
  sownOz: number;
  unweighedSows: number;
  unweighedTrays: number;
  undoneSownOz: number;
  beforeFirstReceiptOz: number;
  unattributedOz: number;
  onHandOz: number | null;
};

export type WriteOffCategory =
  | "sales_discount" | "quality_spoilage"
  | "pricing_or_billing_error" | "customer_goodwill" | "other";
export type WriteOffInput = { category: WriteOffCategory; reason: string | null };

export type CashRow = {
  recordType: string; incomeId: string; dateReceived: string;
  amountCents: number; source: string; canonicalCategory: string;
  scheduleFLine: string; scheduleCLine: string;
  descriptor: string; receiptFileRef: string;
};
export type CashCollected = { totalCents: number; count: number };
export type WriteOffRow = {
  eventId: string; orderId: string; venueName: string;
  shortfallCents: number; category: string;
  reason: string | null; writtenOffOn: string;
};
export type WriteOffCategoryTotal = { category: string; count: number; totalCents: number };
export type WriteOffSummary = { totalShortfallCents: number; byCategory: WriteOffCategoryTotal[] };
export type UnpricedOrderRow = { id: string; venueName: string; harvestDate: string; state: string };
export type UnpricedExposure = { count: number };

export type CashOut = { totalCents: number; count: number };
export type NetCash = { collectedCents: number; outCents: number; netCents: number };
export type BadDebtRow = {
  eventId: string; orderId: string; venueName: string;
  amountCents: number; writtenOffOn: string;
};
export type BadDebtSummary = { totalCents: number; count: number };
export type CategoryTotal = {
  categoryId: string; name: string; totalCents: number; count: number;
};
export type IncomeCorrectionRow = {
  correctionEventId: string; targetIncomeId: string;
  action: string; correctedOn: string;
};
export type IncomeCorrectionCount = { count: number };

export type IncludedPayment = {
  eventId: string;
  datePaid: string;
  payee: string;
  canonicalCategory: string;
  amountCents: number;
};

export type IncludedTrayRecord = {
  eventId: string;
  occurredOn: string;
  varietyOrItem: string;
  quantity: number;
  seedQuantityRecorded: boolean;
};

export type MethodStatement = {
  windowLabel: string;
  windowFrom: string;
  windowTo: string;
  originFilter: string;
  paymentRule: string;
  physicalRule: string;
  joinRule: string;
  exclusionRule: string;
  payments: IncludedPayment[];
  trayRecords: IncludedTrayRecord[];
  paymentCount: number;
  trayRecordCount: number;
  totalPaidCents: number;
  totalTrays: number;
  trayRecordsWithSeedRecorded: number;
  trayRecordsWithoutSeedRecorded: number;
  completenessNote: string;
};

/** Derived at query time. Never stored.
 *  Do not cache this in component state beyond the current view. */
export type CostPerTrayFigure = {
  totalPaidCents: number;
  totalTrays: number;
  centsPerTray: number;
};

export type CostPerTrayOutcome =
  | { kind: "computed"; figure: CostPerTrayFigure; method: MethodStatement }
  | { kind: "refused"; reason: string; method: MethodStatement };

export type SnapshotInfo = {
  fileName: string;
  path: string;
  takenAt: string;
  sizeBytes: number;
};

export type FarmLocation = {
  farmDbPath: string;
  folderPath: string;
  lastSnapshotAt: string | null;
};

export type ExportResult = {
  bundlePath: string;
  fileCount: number;
  totalBytes: number;
  exportedAt: string;
};

export type ImportRefusal =
  | { kind: "missingEventId"; lineNo: number }
  | {
      kind: "farmOsConflict";
      eventId: string;
      field: string;
      inThisFarm: string;
      inTheBundle: string;
    }
  | { kind: "commercialClaimingFarmOs"; eventId: string; detail: string }
  | { kind: "foreignKindNotCarried"; eventId: string; eventKind: string }
  | {
      kind: "differentFarm";
      farmRecordsHere: number;
      eventsInBundle: number;
    }
  | { kind: "logVersusDatabase"; detail: string }
  | { kind: "manifestMismatch"; path: string; detail: string }
  | { kind: "schemaVersion"; bundle: number; thisApp: number }
  | { kind: "malformed"; lineNo: number; detail: string };

export type ImportPlan = {
  bundlePath: string;
  bundleExportedAt: string;
  eventsInBundle: number;
  sharedEventIds: number;
  alreadyPresentIdentical: number;
  wouldBeAdded: number;
  foreignRecordsInBundle: number;
  refusals: ImportRefusal[];
  canApply: boolean;
  explanations: string[];
};

export type ImportResult = {
  eventsAdded: number;
  eventsSkippedIdentical: number;
  foreignRecordsAdded: number;
};

export type RecountCrop = {
  cropId: string;
  cropName: string;
  appQuantity: number;
  trayIds: string[];
};

export type RecountEntry = {
  cropId: string;
  countedQuantity: number;
};

export type RecountCropChange = {
  cropId: string;
  cropName: string;
  quantity: number;
};

export type RecountResult = {
  adjustedDown: RecountCropChange[];
  adjustedUp: RecountCropChange[];
  unchanged: number;
};

export type AttentionItem = {
  id: string;
  kind: string;
  entityType: string | null;
  entityId: string | null;
  message: string;
  actions: string[];
  createdAt: string;
};

export type ResolveResult = {
  trayIds: string[];
  openUrl?: string | null;
};

export type PollResult = {
  ok: boolean;
  sessionsApplied: number;
  refundsApplied: number;
  disputesApplied: number;
  error: string | null;
};

export type NewPaidOrders = {
  count: number;
};

export type ReconciliationOrder = {
  id: string;
  cropName: string;
  quantity: number;
  state: string;
  capacityConsumed: number;
  amountCents: number;
  paidAt: string;
};

export type ReconciliationDate = {
  harvestDate: string;
  available: number;
  sold: number;
  remaining: number;
  orders: ReconciliationOrder[];
};

export type Observed<T> = { value: T; fetchedAt: string; origin: string };

export type Variety = {
  name: string;
  price: string;
  availability: string;
};

export type Severity = "Healthy" | "Degraded" | "Unhealthy";

export type CheckStatus = {
  checkId: string;
  severity: Severity;
  sentence: string;
  ranAt: string | null;
};

export type MarketingSummary = {
  timeToFirstStandingOrder: string;
};

export type VarietyDemand = {
  name: string;
  traysWeek: number;
  sownLast7Days: number;
  shortfall: number;
};

export type StandingDemandView = {
  standingVenues: number;
  traysWeek: number;
  sownLast7Days: number;
  shortfall: number;
  standingVarieties: string[];
  varieties: VarietyDemand[];
  unallocatedVenues: string[];
  unallocatedTraysWeek: number;
};

export type VenueView = {
  venueId: string;
  name: string;
  venueType: string;
  contact: string | null;
  phone: string | null;
  address: string | null;
  note: string | null;
  archivedAt: string | null;
  createdAt: string;
  updatedAt: string;
  qrReady: boolean;
};

export type SampleView = {
  sampleId: string;
  venueId: string;
  venueName: string;
  droppedOn: string;
  varieties: string[];
  packCount: number;
  note: string | null;
  createdAt: string;
  token: string | null;
};
export type StandingRequestView = {
  requestId: string;
  token: string;
  venueId: string;
  venueName: string;
  varieties: string[];
  bagsPerCycle: number;
  requestedAt: string;
  contact: string | null;
  createdAt: string;
  decidedAt: string | null;
  outcome: string | null;
};
export type StandingRequestDecision = {
  requestId: string;
  outcome: string;
  decidedAt: string;
  stage: StageView | null;
};
export type StandingPullView = {
  message: string;
  lastOkMessage: string | null;
  refusalMessage: string | null;
  gapMessage: string | null;
};
export type CoverageRef = { kind: string; id: string };
export type CommitmentLine = { kind: string; id: string; text: string; covered: boolean };
export type HarvestCommitmentsView = {
  cropId: string;
  cropName: string;
  harvestedOn: string;
  header: string;
  emptyLine: string;
  lines: CommitmentLine[];
};

export type TouchView = {
  touchId: string;
  venueId: string;
  venueName: string;
  touchedOn: string;
  channel: string;
  outcome: string | null;
  note: string | null;
  createdAt: string;
};

export type FollowupView = {
  followupId: string;
  venueId: string;
  venueName: string;
  dueOn: string;
  what: string;
  clearedAt: string | null;
  createdAt: string;
  updatedAt: string;
  attentionId: string | null;
};

export type StageView = {
  venueId: string;
  venueName: string;
  stage: string;
  traysWeek: number | null;
  varieties: string[] | null;
  varietyTargets: Record<string, number> | null;
  changedOn: string;
  note: string | null;
  updatedAt: string;
};

export type ReviewObservationView = {
  observationId: string;
  observedOn: string;
  count: number;
  source: string;
  createdAt: string;
};

export type ReviewRequestView = {
  venueId: string;
  venueName: string;
  decidedOn: string;
  outcome: string;
  createdAt: string;
};

export type WeeklyAction = {
  kind: string;
  title: string;
  venueId: string | null;
  attentionId: string | null;
};

export type WeeklyActionsView = {
  actions: WeeklyAction[];
  dropped: number;
  pitchingAdvice: string | null;
};

export type ReputationCounts = {
  activeWarmVenues: number;
  samplesOutstanding: number;
  standingOrders: number;
  standingTraysWeek: number;
  overdueFollowups: number;
  googleReviewCount: number | null;
  googleReviewObservedOn: string | null;
};

export type ScanView = {
  count: Observed<number> | null;
  message: string;
  gapMessage: string | null;
  printedNote: string;
};

export type ScanConfigView = {
  endpointUrl: string | null;
  tokenSet: boolean;
  configuredAt: string | null;
};

export type CapacitySight = {
  rows: CapacityRow[];
  takenAt: string;
  fileName: string;
};

export type CropReach = {
  cropId: string;
  cropName: string;
  growthDays: number;
  sowBy: string;
  mustSowToday: boolean;
  slotsFree: number | null;
  canSowNow: boolean;
};

export type DateReachability = {
  harvestDate: string;
  cropId: string;
  cropName: string;
  daysUntilHarvest: number;
  reachable: boolean;
  cropGrowthDays: number | null;
  lastSowBy: string | null;
  mustSowToday: boolean;
  crops: CropReach[];
  traysGrowing: number;
  committedTrays: number;
  remainingTrays: number;
  entryLine: string;
  shelfSlotsFree: number | null;
  sowCanServe: boolean;
  overdueOnShelf: number;
};

export type ShelfCapacity = {
  lightSlots: number | null;
  blackoutSlots: number | null;
};

export type CoverOrder = {
  orderId: string;
  venueName: string;
  state: string;
  trays: number;
};

export type CoverDate = {
  harvestDate: string;
  cropId: string;
  cropName: string;
  shortTrays: number;
  message: string;
  reachability: DateReachability;
  orders: CoverOrder[];
};

export type PhoneCaptureView = { proposalId: string; deviceId: string; verb: "move_to_light" | "harvest";
  cropId: string; cropName: string; quantity: number; actualYieldOz: number | null; phoneCapturedAt: string;
  capturedLabel: string; note: string | null; message: string };
export type AcceptedCapture = { proposalId: string; quantity: number; actualYieldOz: number | null };
export type WrittenCapture = { proposalId: string; line: string; appliedEventIds: string[] };
export type BlockedCapture = { proposalId: string; reason: string; sentence: string };
export type PhoneConfirmResult = { written: WrittenCapture[]; blocked: BlockedCapture[] };
export type PhoneCaptureDecision = { proposalId: string; outcome: string; decidedAt: string; gateReason: string | null };
export type AdminPhoneView = { paired: boolean; status: string; pairedAt: string | null };
export type PairingView = { deviceId: string; link: string | null; token: string; pairingText: string; status: string };
export type RetireView = { line: string; status: string };
export type PhonePullView = { message: string; lastOkMessage: string | null; refusalMessage: string | null; gapMessage: string | null };

export type DockPortView = {
  running: boolean;
  port: number | null;
  reachUrl: string | null;
  reachQr: boolean[][] | null;
};

export type DockCardId =
  | "money"
  | "cover"
  | "promise"
  | "rack"
  | "phone_queue"
  | "system";

export type WorstClash = {
  source: "attention" | "standing_shortfall" | "move_due" | "harvest_due";
  kind: string;
  entityId: string | null;
  rank: number;
  sentence: string | null;
  cards: string[] | null;
  owner: string | null;
};

export type DockCard = {
  card: DockCardId;
  severity: Severity | null;
  weight: number | null;
  checkIds: string[];
  sentence: string | null;
  oldestRanAt: string | null;
  oldestRanAtDisplay: string | null;
};

export type DockCheck = {
  checkId: string;
  title: string;
  scope: "farm" | "system";
  card: DockCardId;
  severity: Severity | null;
  sentence: string;
  ranAt: string | null;
};

export type DockFoldsView = {
  overall: Severity | null;
  byScope: { farm: Severity | null; system: Severity | null };
  cards: DockCard[];
  checks: DockCheck[];
  surfaces: { map: Record<string, string>; default: string };
  ranks: {
    map: Record<string, number>;
    unclassified: number;
    standingShortfall: number;
    move: number;
    harvest: number;
  };
  todayAttentionOrder: string[];
  worstClash: WorstClash | null;
};
/** INV-A: a render-only bill from a priced parent + the farm display name. */
export type InvoiceVenueView = {
  name: string;
  contact: string | null;
  phone: string | null;
  address: string | null;
};
export type InvoiceOrderLineView = {
  cropName: string;
  trays: number;
  priceCentsPerTray: number;
  lineTotalCents: number;
};
export type InvoiceLeftoverLineView = {
  cropName: string;
  harvestedOn: string;
  listedOz: number;
};
export type InvoiceBillView = {
  farmName: string;
  number: string;
  parent: string;
  venue: InvoiceVenueView | null;
  orderLines: InvoiceOrderLineView[];
  leftoverLine: InvoiceLeftoverLineView | null;
  harvestDate: string;
  deliveredOn: string | null;
  totalCents: number;
  paidOn: string | null;
  paymentLinkUrl: string | null;
};
