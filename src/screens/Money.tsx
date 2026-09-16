import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  CostCategory,
  CostEvent,
  Crop,
  IncomeRecord,
  MoneyCorrection,
  MoneyStatus,
  StripeAccountPreview,
  DateReachability,
  UnappliedFact,
  OrderView,
  OwedSummary,
  StageView,
  VenueView,
  WholesaleOrderView,
  LeftoverListingView,
  InvoiceBillView,
  ShopPage,
  WriteOffCategory,
} from "@/farm/types";
import type { MoneyFocus } from "@/farm/surfaces";
import {
  correctExpense,
  deliverRefusalLine,
  deliverWholesaleOrder,
  mintWholesalePaymentLink,
  wholesalePaymentLinkQr,
  moneyStatus,
  previewStripeKey,
  confirmStripeKey,
  listLeftoverListings,
  recordLeftoverListing,
  mintLeftoverPaymentLink,
  leftoverPaymentLinkQr,
  payLeftoverListingCash,
  writeLeftoverShopPage,
  openShopPageFolder,
  wholesaleInvoiceBill,
  leftoverInvoiceBill,
  farmDisplayName,
  healthStatus,
  listCostCategories,
  listCrops,
  listExpenses,
  listIncome,
  listMoneyCorrections,
  listUnappliedFacts,
  listOrders,
  listStages,
  listVenues,
  listWholesaleOrders,
  unpricedSettlementLine,
  payWholesaleOrder,
  reverseWholesalePayment,
  badDebtConfirmLine,
  badDebtTrailLine,
  writeOffBadDebt,
  settleOrderWithIncome,
  payAmountWarning,
  duplicateIncomeWarning,
  overcommitWarning,
  owedSummary,
  reachabilityForDate,
  recordWholesaleOrder,
  voidExpense,
  voidWholesaleOrder,
} from "@/farm/api";
import { localToday, monthDayLabel, parseLocalDate } from "@/farm/dates";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { MoneyCaptureControls } from "@/components/MoneyCaptureControls";

function cents(n: number): string {
  return `$${(n / 100).toFixed(2)}`;
}

function unappliedAmount(amountCents: number | null): string {
  return amountCents != null ? cents(amountCents) : "amount not captured";
}

function unappliedSentence(status: string, stripeObject: string): string {
  if (status === "unmatched") {
    return "A payment arrived that Groundtruth couldn't match to a crop and harvest date.";
  }
  if (status === "unrecorded") {
    return "A payment couldn't be recorded.";
  }
  if (status === "no_paid_order") {
    return stripeObject === "dispute"
      ? "A dispute arrived for an order that was never paid."
      : "A refund arrived for an order that was never paid.";
  }
  // C2 (INT-002) - the refund gate's four reasons. Each leaves the order paid.
  if (status === "amount_partial") {
    return "A partial refund was issued. The order stays paid and its capacity stays sold.";
  }
  if (status === "not_terminal") {
    return "A refund is still pending at Stripe. Nothing changes until it settles.";
  }
  if (status === "terminal_failed") {
    return "A refund failed at Stripe. The order stays paid.";
  }
  if (status === "not_comparable") {
    return "A refund arrived that Groundtruth couldn't measure against the order. The order stays paid.";
  }
  if (status === "wholesale_not_delivered") {
    return "A venue paid a payment link before its order was marked delivered. Nothing was recorded. Mark it delivered, then record the payment with Paid…";
  }
  if (status === "wholesale_already_settled") {
    return "A venue paid a payment link on an order that is no longer owed. Nothing was recorded; the money is at Stripe.";
  }
  if (status === "wholesale_amount_mismatch") {
    return "A payment link was paid for an amount that is not the order's total. Nothing was recorded. Record it with Paid…";
  }
  if (status === "wholesale_payment") {
    return stripeObject === "dispute"
      ? "A dispute arrived on a wholesale payment. The order was not changed."
      : "A refund arrived on a wholesale payment. The order was not changed — use Reverse payment if the money went back.";
  }
  // R-10 (PACK-RF-PI, signed law 5): a refund Stripe listed with no payment
  // intent and no session id. There is no order to name, so the sentence
  // names none — and never the raw token as the primary line.
  if (status === "no_payment_intent") {
    return "A refund arrived that Farm OS couldn't match to a payment. Nothing was changed — use Reverse payment if the money went back.";
  }
  // PACK-LO-SPEAK (audit R-3, ruling 3): the two leftover refusal tokens speak
  // leftover — never "order", never the raw Stripe token as the primary line.
  // The trace rows themselves are untouched (standing: unapplied facts are
  // never retired here).
  if (status === "leftover_already_paid") {
    return "A payment link was paid on a leftover listing that is already paid. Nothing was recorded; the money is at Stripe.";
  }
  if (status === "leftover_amount_mismatch") {
    return "A payment link was paid for an amount that is not the leftover listing's total. Nothing was recorded. Record it with Paid…";
  }
  // J4 REF-DUP-FACT (SENTENCE A): a second Stripe session under a cart
  // reference Farm OS already recorded. The first order stands; the twin is
  // named here, never booked, and never as the raw token.
  if (status === "duplicate_reference") {
    return "A payment arrived for a cart Farm OS had already recorded. The first order stands; nothing else was recorded, and the money is at Stripe.";
  }
  return status;
}

function stripeObjectWord(stripeObject: string): string {
  if (stripeObject === "checkout_session") return "session";
  if (stripeObject === "refund") return "refund";
  if (stripeObject === "dispute") return "dispute";
  return stripeObject;
}

function observedDate(observedAt: string): string {
  return observedAt.length >= 10 ? observedAt.slice(0, 10) : observedAt;
}

function errMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return "Could not save. Try again.";
}

function shortfallFor(order: WholesaleOrderView, raw: string): number {
  const cents = parseDollarsToCents(raw);
  if (order.pricedTotalCents == null || cents == null) return 0;
  return cents < order.pricedTotalCents ? order.pricedTotalCents - cents : 0;
}

function parseDollarsToCents(raw: string): number | null {
  const cleaned = raw.trim().replace(/[^0-9.]/g, "");
  if (!cleaned) return null;
  const parts = cleaned.split(".");
  if (parts.length > 2) return null;
  const dollars = Number(parts[0] || "0");
  if (!Number.isFinite(dollars) || dollars < 0) return null;
  let centsPart = parts[1] ?? "";
  if (centsPart.length > 2) centsPart = centsPart.slice(0, 2);
  const centsVal = Number((centsPart + "00").slice(0, 2));
  if (!Number.isFinite(centsVal)) return null;
  const total = dollars * 100 + centsVal;
  if (!Number.isInteger(total) || total <= 0) return null;
  return total;
}

function toYyyyMmDd(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

function deliveredUnpaidAge(days: number): string {
  if (days === 1) return "delivered 1 day ago, unpaid";
  return `delivered ${days} days ago, unpaid`;
}

/**
 * INTEGRITY-RECEIPT (STAMP A) — the bill's verify stamp is the stored last
 * pass Health already reports, read through the same health_status command.
 * H4's Healthy sentence is this desk prefix followed by the pass time, so the
 * remainder IS the when. Anything else — Degraded, Unhealthy, a read that
 * fails — is no fact, and the footer drops the segment rather than invent a
 * pass. Money never runs a verify; that write stays on Health.
 */
const VERIFY_PASS_PREFIX = "H4 Core data intact — quick_check ok, verify passed ";

async function verifyPassWhen(): Promise<string | null> {
  try {
    const h4 = (await healthStatus()).find((s) => s.checkId === "H4");
    if (h4 == null || h4.severity !== "Healthy") return null;
    if (!h4.sentence.startsWith(VERIFY_PASS_PREFIX)) return null;
    const when = h4.sentence.slice(VERIFY_PASS_PREFIX.length).trim();
    return when === "" ? null : when;
  } catch {
    return null;
  }
}

/** INTEGRITY-RECEIPT (LINE A) — segments joined by " · "; a segment with no fact drops with its separator. */
function receiptLine(segments: (string | null)[]): string {
  return segments.filter((s): s is string => s != null && s !== "").join(" · ");
}
/**
 * SEND-THE-BILL — the human title: the venue (a leftover bill says Leftover)
 * and the harvest month day. The UUID stays in the mono receipt line.
 */
function billTitle(bill: InvoiceBillView): string {
  const day = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric" }).format(
    parseLocalDate(bill.harvestDate),
  );
  const who = bill.venue?.name ?? (bill.parent === "leftover" ? "Leftover" : null);
  return who != null ? `Invoice — ${who} · ${day}` : `Invoice — ${day}`;
}
/**
 * SEND-THE-BILL — the bill as plain text for Copy bill and the mail body: the
 * sheet's lines in the sheet's order, the receipt line included, then
 * "Pay online: {url}" last when a link exists. Nothing the sheet does not print.
 */
function billText(bill: InvoiceBillView, farmName: string | null, receipt: string): string {
  return [
    farmName,
    billTitle(bill),
    bill.venue?.name ?? null,
    bill.venue?.contact ?? null,
    bill.venue?.phone ?? null,
    bill.venue?.address ?? null,
    `Harvest ${bill.harvestDate}${bill.deliveredOn != null ? ` · delivered ${bill.deliveredOn}` : ""}`,
    ...bill.orderLines.map(
      (line) =>
        `${line.cropName} · ${line.trays} trays × ${cents(line.priceCentsPerTray)} = ${cents(line.lineTotalCents)}`,
    ),
    bill.leftoverLine != null
      ? `Leftover ${bill.leftoverLine.cropName} · ${bill.leftoverLine.harvestedOn} · ${bill.leftoverLine.listedOz.toFixed(1)} oz`
      : null,
    `Total ${cents(bill.totalCents)}`,
    bill.paidOn != null ? `Paid ${bill.paidOn}` : null,
    receipt,
    bill.paymentLinkUrl != null ? `Pay online: ${bill.paymentLinkUrl}` : null,
  ]
    .filter((line): line is string => line != null && line !== "")
    .join("\n");
}

/**
 * Owed wording, mirrored from Today.tsx (owedLine — owed_lo_tests pins the
 * mirror). Both screens read the SAME OwedSummary from wholesale.rs
 * owed_summary, so the NUMBER can never drift; only this prose could.
 * `total_cents` is deliberately null whenever anything is unpriced
 * (wholesale.rs — "never a partial sum wearing a total's clothes"). This
 * function must state that in words and must never invent a partial sum.
 * OWED-LO (audit R-2): priced-unpaid leftover (leftoverCount /
 * leftoverCents, from leftover::owed_leftover through owed_summary) is owed
 * money — it blocks the empty line and joins the count and the total.
 * Unpriced leftover carries no cents and never appears here.
 */
function owedLine(o: OwedSummary | null): string {
  if (o == null) return "Owed to you: not readable right now.";
  if (o.deliveries === 0 && o.leftoverCount === 0) return "Nothing owed to you.";
  const items: string[] = [];
  if (o.deliveries > 0) {
    items.push(`${o.deliveries} ${o.deliveries === 1 ? "delivery" : "deliveries"}`);
  }
  if (o.leftoverCount > 0) {
    items.push(`${o.leftoverCount} leftover ${o.leftoverCount === 1 ? "listing" : "listings"}`);
  }
  const across = items.join(" and ");
  const totalCents =
    o.deliveries > 0 && o.totalCents == null ? null : (o.totalCents ?? 0) + o.leftoverCents;
  const what =
    totalCents != null
      ? `Owed to you: ${cents(totalCents)} across ${across}`
      : `Owed to you: ${across}, value partly unpriced`;
  const age =
    o.oldestDays == null
      ? null
      : o.oldestDays === 0
        ? "delivered today"
        : `oldest ${o.oldestDays} ${o.oldestDays === 1 ? "day" : "days"}`;
  return age == null ? `${what}.` : `${what} — ${age}.`;
}

function lineSummary(order: WholesaleOrderView): string {
  return order.lines.map((l) => `${l.trays} ${l.cropName}`).join(" · ");
}

/// Derived, with the raw state as the fallback so a state this code has never
/// seen is shown rather than swallowed (Health's checkTitle pattern,
/// healthScopes.ts:32-40).
function stateLabel(state: string): string {
  if (state === "delivered") return "delivered, unpaid";
  if (state === "written_off") return "written off";
  // PACK-LIST-STATE (audit R-20): the word the Void control already uses —
  // never the raw state token as the primary label.
  if (state === "voided") return "Void";
  return state;
}

function reverseT1(amount: string, venue: string, d: string): string {
  return `Reverse ${amount} from ${venue}, recorded ${d}? The order returns to unpaid and that money leaves the income register. The delivery stands and the venue still owes you.`;
}

function reverseT2(amount: string, venue: string, d: string, today: string): string {
  return `Payment of ${amount} from ${venue} reversed - recorded ${d}, reversed ${today}.`;
}

/**
 * B1-F2 (D10) — the Wholesale list, presented debt-first. The SAME rows the
 * reader returns (wholesale.rs list_orders, newest ordered_on first), re-sorted
 * for collection: delivered-unpaid oldest first (by delivered_on), then ordered
 * by harvest date, then paid in the reader's own newest-first order.
 * PACK-LIST-STATE (audit R-20): voided rows are no longer dropped before this
 * sort — they take the STATE_ORDER fallback rank (after paid) and land under
 * Settled via SETTLED_STATES. Presentation only: no query, cap or write
 * changes.
 */
const STATE_ORDER: Record<string, number> = { delivered: 0, ordered: 1, paid: 2 };
function stateRank(state: string): number {
  return STATE_ORDER[state] ?? 3;
}
function cmpDates(a: string | null, b: string | null): number {
  if (a == null && b == null) return 0;
  if (a == null) return 1;
  if (b == null) return -1;
  return a < b ? -1 : a > b ? 1 : 0;
}
function sortForCollection(rows: WholesaleOrderView[]): WholesaleOrderView[] {
  return [...rows].sort((a, b) => {
    const byState = stateRank(a.state) - stateRank(b.state);
    if (byState !== 0) return byState;
    if (a.state === "delivered") return cmpDates(a.deliveredOn, b.deliveredOn);
    if (a.state === "ordered") return cmpDates(a.harvestDate, b.harvestDate);
    return 0; // paid (and anything unknown) keep the reader's newest-first order
  });
}

type DraftLine = { cropId: string; trays: string; price: string };

function emptyDraftLine(): DraftLine {
  return { cropId: "", trays: "1", price: "" };
}

/// Settled = paid, written off, and voided — and ONLY those. Any state this
/// code does not know lands in To collect, never in Settled: an unclassified
/// obligation must never be filed as history (same principle as surfaces.ts
/// UNCLASSIFIED_TODAY_RANK). PACK-LIST-STATE (audit R-20): voided is history,
/// not a live debt — it renders under Settled with the Void label, never in
/// To collect, and joins no count and no money sum.
const SETTLED_STATES = new Set(["paid", "written_off", "voided"]);
const LIVE_VISIBLE = 5;
const SETTLED_VISIBLE = 5;
/// SEND-THE-BILL — the two places in the Wholesale card that are not an order
/// row: the New order form and the bill. A row's place is its order id.
const NEW_ORDER_FORM = "new-order";
const BILL_SHEET = "bill";
/// Cash in is a history, not a queue: it only grows, and unbounded it pushes
/// Cash out and the corrections trail off the screen. Newest 7, matching the
/// backup list (FarmBackupSheet.tsx:400).
const INCOME_VISIBLE = 7;
/// Cash out and the corrections trail are histories like Cash in: they only
/// grow. Same cut-off, kept as its own constant because INCOME_VISIBLE belongs
/// to the Cash-in collapse and that collapse is out of scope here.
const MONEY_LIST_VISIBLE = 7;
const PRICE_REQUIRED_LINE =
  "Every variety needs a price per tray. An order with no price has no total, and an order with no total cannot be settled.";

export function Money({
  focus = null,
  onFocusHandled,
  onBackToToday,
}: {
  focus?: MoneyFocus | null;
  onFocusHandled?: () => void;
  /** B1-F2 (D14) — the "Back to Today" control after a deep-linked write. Navigation only; no write. */
  onBackToToday?: () => void;
}) {
  const [income, setIncome] = useState<IncomeRecord[]>([]);
  const [expenses, setExpenses] = useState<CostEvent[]>([]);
  const [corrections, setCorrections] = useState<MoneyCorrection[]>([]);
  const [unapplied, setUnapplied] = useState<UnappliedFact[]>([]);
  const [categories, setCategories] = useState<CostCategory[]>([]);
  const [wholesale, setWholesale] = useState<WholesaleOrderView[]>([]);
  const [owed, setOwed] = useState<OwedSummary | null>(null);
  const [retailOrders, setRetailOrders] = useState<OrderView[]>([]);
  const [venues, setVenues] = useState<VenueView[]>([]);
  const [crops, setCrops] = useState<Crop[]>([]);
  const [stages, setStages] = useState<StageView[]>([]);
  const [error, setErrorLine] = useState<string | null>(null);
  // SEND-THE-BILL — where the alert line was raised: an order id, NEW_ORDER_FORM
  // or BILL_SHEET, so the same sentence also renders inside the Wholesale card
  // under that row or form. A plain setError(line) belongs to no place and
  // renders on the bottom line alone. One error, one sentence.
  const [errorAt, setErrorAt] = useState<string | null>(null);
  function setError(line: string | null, at: string | null = null) {
    setErrorLine(line);
    setErrorAt(at);
  }
  const [correctTarget, setCorrectTarget] = useState<CostEvent | null>(null);
  const [voidTarget, setVoidTarget] = useState<CostEvent | null>(null);
  const [amount, setAmount] = useState("");
  const [payee, setPayee] = useState("");
  const [categoryId, setCategoryId] = useState("");
  const [datePaid, setDatePaid] = useState("");
  const [descriptor, setDescriptor] = useState("");
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [payingId, setPayingId] = useState<string | null>(null);
  const [payAmount, setPayAmount] = useState("");
  const [payDate, setPayDate] = useState("");
  const [payDescriptor, setPayDescriptor] = useState("");
  const [voidingId, setVoidingId] = useState<string | null>(null);
  const [reversingId, setReversingId] = useState<string | null>(null);
  const [linkCopiedId, setLinkCopiedId] = useState<string | null>(null);
  const [linkQr, setLinkQr] = useState<Record<string, boolean[][]>>({});
  const [leftoverQr, setLeftoverQr] = useState<Record<string, boolean[][]>>({});
  const [loPayingId, setLoPayingId] = useState<string | null>(null);
  const [loPayAmount, setLoPayAmount] = useState("");
  const [loPayDate, setLoPayDate] = useState("");
  const [loPayDescriptor, setLoPayDescriptor] = useState("");
  const [invoiceBill, setInvoiceBill] = useState<InvoiceBillView | null>(null);
  // SEND-THE-BILL — the bill whose text was last copied; "Copied" reads for it alone.
  const [billCopiedFor, setBillCopiedFor] = useState<string | null>(null);
  // PACK-FACE (audit R-13) — the invoice header's farm name, read on Money
  // through farmDisplayName(), the same farm_config reader Settings uses.
  // One source: Money keeps no farm-name store and never writes the name.
  const [farmName, setFarmName] = useState<string | null>(null);
  // INTEGRITY-RECEIPT (STAMP A) — the last verify pass time H4 reported when
  // the bill was opened; null is "no pass to print", never a guess.
  const [verifyWhen, setVerifyWhen] = useState<string | null>(null);
  // GT-D23 — the desk key door. money_status carries configured / account /
  // mode and never the key; paste → preview → confirm run the existing
  // commands. Nothing here reaches offers, the checkout address, or the shop.
  const [stripeAccount, setStripeAccount] = useState<MoneyStatus | null>(null);
  const [connectStep, setConnectStep] = useState<"closed" | "paste" | "confirm">("closed");
  const [connectKey, setConnectKey] = useState("");
  const [connectPreview, setConnectPreview] = useState<StripeAccountPreview | null>(null);
  const [connectBusy, setConnectBusy] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  // LO-A (GT-D24) — the leftover listing door. Read from list_leftover_listings;
  // harvestedOz is computed on the PC at read, never stored. The four refusals
  // come back as the command's Err and land on the existing alert line below.
  const [leftover, setLeftover] = useState<LeftoverListingView[]>([]);
  const [leftoverOpen, setLeftoverOpen] = useState(false);
  const [leftoverCropId, setLeftoverCropId] = useState("");
  const [leftoverDay, setLeftoverDay] = useState(toYyyyMmDd(localToday()));
  const [leftoverOz, setLeftoverOz] = useState("");
  // LO-B (GT-D24-B) — per-row dollars for the mint; refusals are the four mint
  // sentences from the door, on the same alert line.
  const [leftoverPrice, setLeftoverPrice] = useState<Record<string, string>>({});
  // SHOP DOOR Job A (WRITE A: viewer only) — the last page written this
  // visit: its path, for the Open folder control. Null until the operator
  // writes one; a refusal clears it and lands on the alert line below.
  const [shopPage, setShopPage] = useState<ShopPage | null>(null);
  const [writingOffId, setWritingOffId] = useState<string | null>(null);
  const [voidReasonWs, setVoidReasonWs] = useState("");
  const [reverseTrail, setReverseTrail] = useState<Record<string, string>>({});
  const [newVenueId, setNewVenueId] = useState("");
  const [newHarvestDate, setNewHarvestDate] = useState(toYyyyMmDd(localToday()));
  const [newLines, setNewLines] = useState<DraftLine[]>([emptyDraftLine()]);
  const [pendingOvercommit, setPendingOvercommit] = useState<string | null>(null);
  const [pendingPayMismatch, setPendingPayMismatch] = useState<string | null>(null);
  const [pendingPayDuplicate, setPendingPayDuplicate] = useState<string | null>(null);
  const [payWriteOffCategory, setPayWriteOffCategory] = useState<WriteOffCategory | "">("");
  const [payWriteOffReason, setPayWriteOffReason] = useState("");
  const [linkingIncomeId, setLinkingIncomeId] = useState<string | null>(null);
  const [linkOrderId, setLinkOrderId] = useState("");
  const [writeOffCategory, setWriteOffCategory] = useState<WriteOffCategory | "">("");
  const [writeOffReason, setWriteOffReason] = useState("");
  const [pendingLinkMismatch, setPendingLinkMismatch] = useState<string | null>(null);
  const [entryReachByLine, setEntryReachByLine] = useState<
    Record<number, DateReachability>
  >({});
  const [showAllLive, setShowAllLive] = useState(false);
  const [showAllSettled, setShowAllSettled] = useState(false);
  const [showAllIncome, setShowAllIncome] = useState(false);
  const [showAllExpenses, setShowAllExpenses] = useState(false);
  const [showAllCorrections, setShowAllCorrections] = useState(false);
  const [showAllUnapplied, setShowAllUnapplied] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [focusedOrderId, setFocusedOrderId] = useState<string | null>(null);
  const [focusMissing, setFocusMissing] = useState(false);
  // B1-F2 (D14) — the row whose deep-linked write just landed; it carries the
  // "Back to Today" control where the form was. Cleared by the next arrival.
  const [backToTodayId, setBackToTodayId] = useState<string | null>(null);
  // B1-F2 (D12) — New order sits behind a disclosure, collapsed by default.
  const [newOrderOpen, setNewOrderOpen] = useState(false);
  // B2-F2 (B2-D5a) — the order the current draft lines were copied from, so
  // the form can say so. Cleared when the venue changes or the order is
  // recorded. A proposal only: Record order is still the write.
  const [copiedFrom, setCopiedFrom] = useState<WholesaleOrderView | null>(null);
  // FROM-STANDING — the draft lines were drafted from the venue's standing
  // line, so the form can say so. Cleared when the venue changes, when Same as
  // last order replaces the draft, or when the order is recorded. A proposal
  // only: Record order is still the write.
  const [draftedFromStanding, setDraftedFromStanding] = useState(false);
  const [unpricedLine, setUnpricedLine] = useState("");
  const [deliverRefusal, setDeliverRefusal] = useState<Record<string, string>>(
    {},
  );
  const [badDebtLine, setBadDebtLine] = useState<Record<string, string>>({});
  const [badDebtTrail, setBadDebtTrail] = useState<Record<string, string>>({});
  const orderRowRefs = useRef<Record<string, HTMLLIElement | null>>({});

  async function load() {
    const [inRows, outRows, trail, cats, ws, retail, venueRows, cropRows, stageRows, unpriced, unappliedRows, owedRow, stripeRow, leftoverRows, farmNameRow] =
      await Promise.all([
        listIncome(),
        listExpenses(),
        listMoneyCorrections(),
        listCostCategories(),
        listWholesaleOrders(),
        listOrders(),
        listVenues(),
        listCrops(),
        listStages(),
        unpricedSettlementLine(),
        listUnappliedFacts(),
        owedSummary(),
        moneyStatus(),
        listLeftoverListings(),
        farmDisplayName(),
      ]);
    setIncome(inRows);
    setExpenses(outRows);
    setCorrections(trail);
    setUnapplied(unappliedRows);
    setCategories(cats);
    setWholesale(ws);
    setOwed(owedRow);
    setStripeAccount(stripeRow);
    setLeftover(leftoverRows);
    setFarmName(farmNameRow);
    setRetailOrders(retail);
    setVenues(venueRows);
    setCrops(cropRows);
    setStages(stageRows);
    setUnpricedLine(unpriced);
    const refusals: Record<string, string> = {};
    await Promise.all(
      ws.map(async (o) => {
        const line = await deliverRefusalLine(o.id);
        if (line) refusals[o.id] = line;
      }),
    );
    setDeliverRefusal(refusals);
    const qrs: Record<string, boolean[][]> = {};
    await Promise.all(
      ws.map(async (o) => {
        if (o.paymentLinkUrl != null) qrs[o.id] = await wholesalePaymentLinkQr(o.id);
      }),
    );
    setLinkQr(qrs);
    const loQrs: Record<string, boolean[][]> = {};
    await Promise.all(
      leftoverRows.map(async (l) => {
        if (l.paymentLinkUrl != null) loQrs[l.listingId] = await leftoverPaymentLinkQr(l.listingId);
      }),
    );
    setLeftoverQr(loQrs);
    const badDebt: Record<string, string> = {};
    await Promise.all(
      ws.map(async (o) => {
        const line = await badDebtConfirmLine(o.id);
        if (line) badDebt[o.id] = line;
      }),
    );
    setBadDebtLine(badDebt);
    const badDebtTrails: Record<string, string> = {};
    await Promise.all(
      ws.map(async (o) => {
        const line = await badDebtTrailLine(o.id);
        if (line) badDebtTrails[o.id] = line;
      }),
    );
    setBadDebtTrail(badDebtTrails);
    if (!newVenueId && venueRows[0]) {
      setNewVenueId(venueRows[0].venueId);
    }
    if (!leftoverCropId && cropRows[0]) {
      setLeftoverCropId(cropRows[0].id);
    }
    // FIRST-15 — an empty register opens on the verb it needs; first load only, never after a write.
    if (!loaded && ws.length === 0) setNewOrderOpen(true);
    setLoaded(true);
  }

  useEffect(() => {
    void load().catch((e: unknown) => setError(errMessage(e)));
  }, []);

  // B3: the sow-by implication of the chosen date, before the operator commits.
  // Attention, not a gate (GT-D14). Re-reads after any order write because
  // capacity moved.
  useEffect(() => {
    let cancelled = false;
    const d = newHarvestDate.trim();
    if (!/^\d{4}-\d{2}-\d{2}$/.test(d)) {
      setEntryReachByLine({});
      return;
    }
    void Promise.all(
      newLines.map((line) =>
        line.cropId
          ? reachabilityForDate(d, line.cropId)
          : Promise.resolve(null),
      ),
    )
      .then((rows) => {
        if (cancelled) return;
        const next: Record<number, DateReachability> = {};
        rows.forEach((r, i) => {
          if (r) next[i] = r;
        });
        setEntryReachByLine(next);
      })
      .catch(() => {
        if (!cancelled) setEntryReachByLine({});
      });
    return () => {
      cancelled = true;
    };
  }, [newHarvestDate, newLines, wholesale]);

  useEffect(() => {
    setPendingOvercommit(null);
  }, [newHarvestDate, newLines, newVenueId]);

  // RB3 — the acknowledgement dies the moment the amount or the order
  // changes. An ack is for one fact, never a mode.
  useEffect(() => {
    setPendingPayMismatch(null);
    setPendingPayDuplicate(null);
    setPayWriteOffCategory("");
    setPayWriteOffReason("");
  }, [payingId, payAmount, payDate]);

  useEffect(() => {
    setPendingLinkMismatch(null);
    setWriteOffCategory("");
    setWriteOffReason("");
  }, [linkingIncomeId, linkOrderId]);

  const cashIn = income.reduce((s, r) => s + r.amountCents, 0);
  const cashOut = expenses.reduce((s, r) => s + r.amountCents, 0);
  const liveWholesale = wholesale.filter((o) => o.state !== "voided");
  // B1-F2 (D10) — the list as shown: debt-first. Counts, the owed figure and
  // "Apply to an order…" keep reading liveWholesale in the reader's order.
  // PACK-LIST-STATE (audit R-20): the list shows voided under Settled, so it
  // sorts the unfiltered reader rows. Counts, the owed figure and the apply /
  // prior-order pickers keep reading liveWholesale above — voided stays out
  // of all of those, and out of every money sum (ruling 7).
  const wholesaleList = sortForCollection(wholesale);
  const liveRows = wholesaleList.filter((o) => !SETTLED_STATES.has(o.state));
  const settledRows = wholesaleList.filter((o) => SETTLED_STATES.has(o.state));
  const firstCollect = liveRows.find((o) => o.state === "delivered") ?? null;
  const orderedCount = liveRows.filter((o) => o.state === "ordered").length;
  const openCount = liveWholesale.filter(
    (o) => o.state === "ordered" || o.state === "delivered",
  ).length;
  const deliveredUnpaid = liveWholesale.filter((o) => o.state === "delivered");
  // RB3b — an income row already named by a live order is spent. The row
  // says so rather than just losing its door.
  const orderForIncome = (incomeId: string) =>
    wholesale.find((o) => o.incomeEventId === incomeId && o.state !== "voided") ??
    null;
  // A line with a crop chosen and no usable price. The button says why rather
  // than going quietly dead.
  const newOrderPriceMissing = newLines
    .filter((l) => l.cropId)
    .some((l) => parseDollarsToCents(l.price.trim()) == null);
  // B2-F2 (B2-D5a) — the chosen venue's most recent prior order (the reader is
  // newest ordered_on first; voided rows are not "prior orders"). Its lines,
  // tray counts and prices are what "Same as last order" proposes. Read only.
  const lastOrderForVenue =
    newVenueId === ""
      ? null
      : (wholesale.find((o) => o.venueId === newVenueId && o.state !== "voided") ??
        null);
  function applyLastOrder(o: WholesaleOrderView) {
    setError(null);
    setNewLines(
      o.lines.map((l) => ({
        cropId: l.cropId,
        trays: String(l.trays),
        price:
          l.priceCentsPerTray != null
            ? (l.priceCentsPerTray / 100).toFixed(2)
            : "",
      })),
    );
    setCopiedFrom(o);
    setDraftedFromStanding(false);
  }
  // FROM-STANDING — the chosen venue's standing split, when it has one. The
  // control exists only for a venue whose standing line is split by variety:
  // no standing row, or a /week total not yet split, is no control. Read from
  // the StageView[] load() already fetches (list_stages). Read only.
  const standingStageForVenue =
    newVenueId === ""
      ? null
      : (stages.find(
          (row) => row.venueId === newVenueId && row.stage === "standing",
        ) ?? null);
  const standingTargetsForVenue =
    standingStageForVenue?.varietyTargets != null &&
    Object.keys(standingStageForVenue.varietyTargets).length > 0
      ? standingStageForVenue.varietyTargets
      : null;
  // FROM-STANDING — the price this venue was last billed for a crop: the
  // newest prior order (the reader is newest ordered_on first; voided rows are
  // not prior orders — the same rows Same as last order reads) that carries a
  // line for the crop. No such order, or a line that was never priced, is
  // blank, so PRICE_REQUIRED_LINE speaks before Record order does. Read only.
  function lastPriceForVenueCrop(cropId: string): string {
    const prior = wholesale.find(
      (o) =>
        o.venueId === newVenueId &&
        o.state !== "voided" &&
        o.lines.some((l) => l.cropId === cropId),
    );
    const line = prior?.lines.find((l) => l.cropId === cropId) ?? null;
    return line?.priceCentsPerTray != null
      ? (line.priceCentsPerTray / 100).toFixed(2)
      : "";
  }
  // FROM-STANDING — draft this week's order from the venue's standing line:
  // one draft line per variety target, trays = the target (the split is
  // trays/week), in the order Today's cut-page block prints the split
  // (crops.sortOrder, then A-Z). A target name resolves to the crop that
  // carries that exact name, the way Today already does
  // (crops.find((c) => c.name === v.name)); a name no crop carries keeps its
  // tray count with the variety left to choose, so nothing is dropped quietly
  // and nothing records until a variety is chosen. Harvest date is never
  // carried. A proposal only — the standing line is not written and Record
  // order remains the only write.
  function applyFromStanding(targets: Record<string, number>) {
    setError(null);
    const rank = new Map(crops.map((c) => [c.name, c.sortOrder] as const));
    const rankOf = (name: string) => rank.get(name) ?? Number.MAX_SAFE_INTEGER;
    const split = Object.entries(targets).sort(
      ([a], [b]) => rankOf(a) - rankOf(b) || (a < b ? -1 : a > b ? 1 : 0),
    );
    setNewLines(
      split.map(([name, n]) => {
        const crop = crops.find((c) => c.name === name) ?? null;
        return {
          cropId: crop?.id ?? "",
          trays: String(n),
          price: crop == null ? "" : lastPriceForVenueCrop(crop.id),
        };
      }),
    );
    setCopiedFrom(null);
    setDraftedFromStanding(true);
  }
  // B2-F2 (B2-D7) — the Apply default: the delivered-unpaid order whose priced
  // total equals this income row's amount (the oldest such when several match);
  // otherwise the oldest delivered-unpaid order. Selection only: the select
  // still lists every open debt, and RB3's preview, the write-off gate and
  // "Apply anyway" are unchanged.
  function defaultApplyTarget(amountCents: number): string {
    const oldestFirst = [...deliveredUnpaid].sort((a, b) =>
      cmpDates(a.deliveredOn, b.deliveredOn),
    );
    const match = oldestFirst.find((o) => o.pricedTotalCents === amountCents);
    return (match ?? oldestFirst[0])?.id ?? "";
  }

  function openPay(order: WholesaleOrderView) {
    setError(null);
    setPayingId(order.id);
    setPayAmount(
      order.pricedTotalCents != null
        ? (order.pricedTotalCents / 100).toFixed(2)
        : "",
    );
    setPayDate(toYyyyMmDd(localToday()));
    setPayDescriptor("");
    setVoidingId(null);
  }

  function openVoidWs(orderId: string) {
    setError(null);
    setVoidingId(orderId);
    setVoidReasonWs("");
    setPayingId(null);
  }

  /** The group a row is rendered in, and its index within that group. */
  function groupOf(orderId: string): { settled: boolean; index: number } | null {
    const l = liveRows.findIndex((o) => o.id === orderId);
    if (l >= 0) return { settled: false, index: l };
    const s = settledRows.findIndex((o) => o.id === orderId);
    if (s >= 0) return { settled: true, index: s };
    return null;
  }

  // RB2 — arriving from a Today money card. Reveal the exact order, and for a
  // collect open the same Paid… form the "Paid…" button opens (openPay). This
  // fires no write: onDeliver / onPayWholesale below remain the only writers,
  // and both still need the operator's tap.
  // B1-F2 (D15) — the form opens only when the row itself would offer Paid…:
  // an unpriced delivered order is focused and ringed, and its amber unpriced
  // line speaks; the backend would refuse the settlement anyway
  // (wholesale.rs UNPRICED_SETTLEMENT_LINE).
  useEffect(() => {
    if (focus == null || !loaded) return;
    setBackToTodayId(null);
    const idx = wholesaleList.findIndex((o) => o.id === focus.orderId);
    if (idx < 0) {
      setFocusedOrderId(null);
      setFocusMissing(true);
      onFocusHandled?.();
      return;
    }
    setFocusMissing(false);
    const g = groupOf(focus.orderId);
    if (g != null) {
      if (g.settled && g.index >= SETTLED_VISIBLE) setShowAllSettled(true);
      if (!g.settled && g.index >= LIVE_VISIBLE) setShowAllLive(true);
    }
    setFocusedOrderId(focus.orderId);
    if (
      focus.intent === "collect" &&
      wholesaleList[idx].state === "delivered" &&
      wholesaleList[idx].pricedTotalCents != null
    ) {
      openPay(wholesaleList[idx]);
    }
    onFocusHandled?.();
  }, [focus, loaded, wholesale]);

  useEffect(() => {
    if (focusedOrderId == null) return;
    orderRowRefs.current[focusedOrderId]?.scrollIntoView({ block: "center" });
  }, [focusedOrderId, showAllLive, showAllSettled]);

  // B1-F2 (D14) — after the deep-linked write the row moves within the
  // debt-first list (paid rows sort last). Keep it in view exactly as arrival
  // does: expand the To collect or Settled group past its cap if the row fell
  // behind that group's Show control, and scroll to
  // it, so the operator sees the row turn paid / delivered before leaving.
  useEffect(() => {
    if (backToTodayId == null) return;
    // B1-F2 (D14) — after a deep-linked Save the row has moved from To collect to
    // Settled (onPayWholesale, line ~616). Expand whichever group now holds it.
    const g = groupOf(backToTodayId);
    if (g == null) return;
    if (g.settled && g.index >= SETTLED_VISIBLE && !showAllSettled) {
      setShowAllSettled(true);
      return;
    }
    if (!g.settled && g.index >= LIVE_VISIBLE && !showAllLive) {
      setShowAllLive(true);
      return;
    }
    orderRowRefs.current[backToTodayId]?.scrollIntoView({ block: "center" });
  }, [backToTodayId, wholesale, showAllLive, showAllSettled]);

  async function onDeliver(orderId: string) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await deliverWholesaleOrder(orderId);
      await load();
      // B1-F2 (D14) — a deep-linked write: the row keeps its ring and gains
      // the "Back to Today" control. Not a write; navigation only.
      if (orderId === focusedOrderId) setBackToTodayId(orderId);
    } catch (e: unknown) {
      setError(errMessage(e), orderId);
    } finally {
      setBusy(false);
    }
  }

  async function onMintLink(orderId: string) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await mintWholesalePaymentLink(orderId);
      await load();
    } catch (e: unknown) {
      setError(errMessage(e), orderId);
    } finally {
      setBusy(false);
    }
  }

  async function onCopyLink(o: WholesaleOrderView) {
    if (!o.paymentLinkUrl) return;
    try {
      await navigator.clipboard.writeText(o.paymentLinkUrl);
      setLinkCopiedId(o.id);
    } catch (e: unknown) {
      setError(errMessage(e), o.id);
    }
  }

  function openConnect() {
    setConnectKey("");
    setConnectPreview(null);
    setConnectError(null);
    setConnectStep("paste");
  }

  function closeConnect() {
    setConnectKey("");
    setConnectPreview(null);
    setConnectError(null);
    setConnectStep("closed");
  }

  async function onConnectPreview() {
    if (connectBusy) return;
    setConnectBusy(true);
    setConnectError(null);
    try {
      const p = await previewStripeKey(connectKey.trim());
      setConnectPreview(p);
      setConnectStep("confirm");
    } catch (e: unknown) {
      setConnectError(errMessage(e));
    } finally {
      setConnectBusy(false);
    }
  }

  async function onConnectConfirm() {
    if (connectBusy) return;
    setConnectBusy(true);
    setConnectError(null);
    try {
      const s = await confirmStripeKey(connectKey.trim());
      setStripeAccount(s);
      closeConnect();
    } catch (e: unknown) {
      setConnectError(errMessage(e));
    } finally {
      setConnectBusy(false);
    }
  }

  async function onListLeftover() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const oz = Number.parseFloat(leftoverOz);
      await recordLeftoverListing({
        cropId: leftoverCropId,
        harvestedOn: leftoverDay.trim(),
        listedOz: Number.isFinite(oz) ? oz : 0,
      });
      setLeftoverOz("");
      await load();
    } catch (e: unknown) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  function openLoPay(l: LeftoverListingView) {
    setError(null);
    setLoPayingId(l.listingId);
    setLoPayAmount(
      l.pricedTotalCents != null ? (l.pricedTotalCents / 100).toFixed(2) : "",
    );
    setLoPayDate(toYyyyMmDd(localToday()));
    setLoPayDescriptor("");
  }
  async function onPayLeftoverCash() {
    if (!loPayingId || busy) return;
    // PACK-LO-SPEAK (audit R-17, ruling 6): one price sentence, owned by the
    // cash door (leftover.rs LEFTOVER_CASH_UNPRICED_LINE). An unparsable or
    // empty amount goes to the door as 0 and the door's sentence comes back —
    // the same shape onMintLeftover already uses for the mint sentence.
    const amountCents = parseDollarsToCents(loPayAmount);
    setBusy(true);
    setError(null);
    try {
      await payLeftoverListingCash({
        listingId: loPayingId,
        amountCents: amountCents ?? 0,
        dateReceived: loPayDate,
        descriptor: loPayDescriptor.trim() || null,
      });
      setLoPayingId(null);
      await load();
    } catch (e) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function onInvoiceWholesale(o: WholesaleOrderView) {
    setError(null);
    try {
      setVerifyWhen(await verifyPassWhen());
      setInvoiceBill(await wholesaleInvoiceBill(o.id));
    } catch (e) {
      setInvoiceBill(null);
      setError(errMessage(e), o.id);
    }
  }
  async function onInvoiceLeftover(l: LeftoverListingView) {
    setError(null);
    try {
      setVerifyWhen(await verifyPassWhen());
      setInvoiceBill(await leftoverInvoiceBill(l.listingId));
    } catch (e) {
      setInvoiceBill(null);
      setError(errMessage(e));
    }
  }

  async function onMintLeftover(l: LeftoverListingView) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const cents = parseDollarsToCents(leftoverPrice[l.listingId] ?? "");
      await mintLeftoverPaymentLink({
        listingId: l.listingId,
        priceCents: cents ?? 0,
      });
      await load();
    } catch (e: unknown) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function onCopyLeftoverLink(l: LeftoverListingView) {
    if (!l.paymentLinkUrl) return;
    try {
      await navigator.clipboard.writeText(l.paymentLinkUrl);
    } catch (e: unknown) {
      setError(errMessage(e));
    }
  }

  // SHOP DOOR Job A — the PC composes the page of minted, unpaid leftover
  // lots from the same listings this screen shows and writes it to the farm
  // folder. Mints nothing, so nothing here reloads; the refusals (farm name,
  // budget, key) come back as the command's Err on the alert line.
  async function onWriteShopPage() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setShopPage(await writeLeftoverShopPage());
    } catch (e: unknown) {
      setShopPage(null);
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function onOpenShopFolder() {
    setError(null);
    try {
      await openShopPageFolder();
    } catch (e: unknown) {
      setError(errMessage(e));
    }
  }

  async function onApplyIncome(row: IncomeRecord) {
    if (busy || !linkOrderId) return;
    // The RB3a preview, on the income row's own amount. Same command, same bytes.
    if (pendingLinkMismatch === null) {
      const warning = await payAmountWarning(linkOrderId, row.amountCents);
      if (warning !== null) {
        setPendingLinkMismatch(warning);
        return;
      }
    }
    setBusy(true);
    setError(null);
    try {
      const target = deliveredUnpaid.find((o) => o.id === linkOrderId) ?? null;
      const shortfallCents =
        target?.pricedTotalCents != null && row.amountCents < target.pricedTotalCents
          ? target.pricedTotalCents - row.amountCents
          : 0;
      await settleOrderWithIncome({
        orderId: linkOrderId,
        incomeEventId: row.incomeId,
        amountAck: pendingLinkMismatch !== null,
        writeOff: shortfallCents > 0
          ? { category: writeOffCategory as WriteOffCategory,
              reason: writeOffCategory === "other" ? writeOffReason.trim() : null }
          : null,
      });
      setLinkingIncomeId(null);
      setPendingLinkMismatch(null);
      await load();
    } catch (e: unknown) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function onPayWholesale() {
    if (!payingId || busy) return;
    const amountCents = parseDollarsToCents(payAmount);
    if (amountCents == null) {
      setError("Amount must be a positive dollars figure.", payingId);
      return;
    }
    if (pendingPayMismatch === null) {
      const warning = await payAmountWarning(payingId, amountCents);
      if (warning !== null) {
        setPendingPayMismatch(warning);
        return;
      }
    }
    if (pendingPayDuplicate === null) {
      const order = wholesale.find((o) => o.id === payingId);
      if (order) {
        const warning = await duplicateIncomeWarning(
          order.venueName,
          amountCents,
          payDate,
        );
        if (warning !== null) {
          setPendingPayDuplicate(warning);
          return;
        }
      }
    }
    setBusy(true);
    setError(null);
    try {
      const order = wholesale.find((o) => o.id === payingId);
      await payWholesaleOrder({
        orderId: payingId,
        amountCents,
        dateReceived: payDate,
        descriptor: payDescriptor.trim() || null,
        amountAck: pendingPayMismatch !== null,
        duplicateAck: pendingPayDuplicate !== null,
        writeOff:
          order != null && shortfallFor(order, payAmount) > 0
            ? { category: payWriteOffCategory as WriteOffCategory,
                reason: payWriteOffCategory === "other" ? payWriteOffReason.trim() : null }
            : null,
      });
      const paidId = payingId;
      setPayingId(null);
      await load();
      // B1-F2 (D14) — a deep-linked write: "Back to Today" appears where the
      // form was. Navigation only; the write above is unchanged.
      if (paidId === focusedOrderId) setBackToTodayId(paidId);
    } catch (e: unknown) {
      setError(errMessage(e), payingId);
    } finally {
      setBusy(false);
    }
  }

  function askReversePayment(o: WholesaleOrderView) {
    if (busy || o.state !== "paid" || !o.paidOn) return;
    const linked = income.find((r) => r.incomeId === o.incomeEventId);
    const amountCents = linked?.amountCents ?? o.pricedTotalCents;
    if (amountCents == null) return;
    setReversingId(o.id);
  }

  async function confirmReversePayment(o: WholesaleOrderView) {
    const linked = income.find((r) => r.incomeId === o.incomeEventId);
    const amountCents = linked?.amountCents ?? o.pricedTotalCents;
    if (amountCents == null || !o.paidOn) return;
    const amount = cents(amountCents);
    const d = monthDayLabel(parseLocalDate(o.paidOn));
    const today = monthDayLabel(localToday());
    setBusy(true);
    setError(null);
    try {
      await reverseWholesalePayment(o.id);
      setReverseTrail((prev) => ({
        ...prev,
        [o.id]: reverseT2(amount, o.venueName, d, today),
      }));
      await load();
    } catch (e: unknown) {
      setError(errMessage(e), o.id);
    } finally {
      setBusy(false);
      setReversingId(null);
    }
  }

  function askWriteOff(o: WholesaleOrderView) {
    if (busy || o.state !== "delivered" || !badDebtLine[o.id]) return;
    setWritingOffId(o.id);
  }

  async function confirmWriteOff(o: WholesaleOrderView) {
    const line = badDebtLine[o.id];
    if (!line) return;
    setBusy(true);
    setError(null);
    try {
      await writeOffBadDebt(o.id, line);
      await load();
    } catch (e: unknown) {
      setError(errMessage(e), o.id);
    } finally {
      setBusy(false);
      setWritingOffId(null);
    }
  }

  async function onVoidWholesale() {
    if (!voidingId || busy) return;
    setBusy(true);
    setError(null);
    try {
      await voidWholesaleOrder(voidingId, voidReasonWs.trim() || null);
      setVoidingId(null);
      await load();
    } catch (e: unknown) {
      setError(errMessage(e), voidingId);
    } finally {
      setBusy(false);
    }
  }

  async function onRecordOrder() {
    if (busy) return;
    const lines = newLines
      .filter((l) => l.cropId)
      .map((l) => {
        const trays = Number(l.trays);
        const priceRaw = l.price.trim();
        const centsVal = parseDollarsToCents(priceRaw);
        if (centsVal == null) {
          return { error: PRICE_REQUIRED_LINE };
        }
        const priceCentsPerTray = centsVal;
        if (!Number.isInteger(trays) || trays < 1) {
          return { error: "Trays must be a whole number of at least 1." };
        }
        return { cropId: l.cropId, trays, priceCentsPerTray };
      });
    const bad = lines.find((l) => "error" in l);
    if (bad && "error" in bad) {
      setError(bad.error ?? "Could not save. Try again.", NEW_ORDER_FORM);
      return;
    }
    const ready = lines.filter(
      (l): l is { cropId: string; trays: number; priceCentsPerTray: number } =>
        "cropId" in l,
    );
    if (!newVenueId) {
      setError("Choose a venue.", NEW_ORDER_FORM);
      return;
    }
    if (ready.length === 0) {
      setError("Add at least one variety.", NEW_ORDER_FORM);
      return;
    }
    if (pendingOvercommit === null) {
      const seen = new Set<string>();
      const warnings: string[] = [];
      for (const l of ready) {
        if (seen.has(l.cropId)) continue;
        seen.add(l.cropId);
        const warning = await overcommitWarning(
          newHarvestDate,
          l.cropId,
          l.trays,
        );
        if (warning !== null) warnings.push(warning);
      }
      if (warnings.length > 0) {
        setPendingOvercommit(warnings.join("\n"));
        return;
      }
    }
    setBusy(true);
    setError(null);
    try {
      await recordWholesaleOrder({
        venueId: newVenueId,
        harvestDate: newHarvestDate,
        lines: ready,
        overcommitAck: pendingOvercommit !== null,
      });
      setNewLines([emptyDraftLine()]);
      setCopiedFrom(null);
      setDraftedFromStanding(false);
      setPendingOvercommit(null);
      await load();
    } catch (e: unknown) {
      setError(errMessage(e), NEW_ORDER_FORM);
    } finally {
      setBusy(false);
    }
  }

  function openCorrect(row: CostEvent) {
    setError(null);
    setCorrectTarget(row);
    setAmount((row.amountCents / 100).toFixed(2));
    setPayee(row.payee);
    setCategoryId(row.canonicalCategory);
    setDatePaid(row.datePaid);
    setDescriptor(row.descriptor);
    setReason("");
  }

  function openVoid(row: CostEvent) {
    setError(null);
    setVoidTarget(row);
    setReason("");
  }

  async function onCorrect() {
    if (!correctTarget || busy) return;
    const amountCents = parseDollarsToCents(amount);
    if (amountCents == null) {
      setError("Amount must be a positive dollars figure.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await correctExpense({
        targetEventId: correctTarget.eventId,
        amountCents,
        payee,
        categoryId,
        datePaid,
        descriptor: descriptor.trim() || null,
        reason: reason.trim() || null,
      });
      setCorrectTarget(null);
      await load();
    } catch (e: unknown) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function onVoid() {
    if (!voidTarget || busy) return;
    setBusy(true);
    setError(null);
    try {
      await voidExpense(voidTarget.eventId, reason.trim() || null);
      setVoidTarget(null);
      await load();
    } catch (e: unknown) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }

  // INTEGRITY-RECEIPT (LINE A) built once: the sheet's mono footer and the
  // copied text read the same string.
  const billReceipt =
    invoiceBill != null
      ? receiptLine([
          farmName,
          `Harvest ${invoiceBill.harvestDate}`,
          verifyWhen != null ? `VERIFY-REPLAY PASS ${verifyWhen}` : null,
          `Invoice ${invoiceBill.number}`,
        ])
      : null;
  // SEND-THE-BILL — Copy bill and Email bill read the bill already on the
  // sheet; neither writes, neither mints, and invoice.issued stays refused.
  // Copy uses the clipboard the payment link already uses. Email opens the
  // operator's own mail app through the opener plugin with the title as
  // subject and the text as body; To stays empty because venues carry no
  // email. A failure lands under the bill, on the same alert line.
  async function onCopyBill() {
    if (invoiceBill == null || billReceipt == null) return;
    setError(null);
    try {
      await navigator.clipboard.writeText(billText(invoiceBill, farmName, billReceipt));
      setBillCopiedFor(invoiceBill.number);
    } catch (e: unknown) {
      setError(errMessage(e), BILL_SHEET);
    }
  }
  async function onEmailBill() {
    if (invoiceBill == null || billReceipt == null) return;
    setError(null);
    try {
      const subject = encodeURIComponent(billTitle(invoiceBill));
      const body = encodeURIComponent(
        billText(invoiceBill, farmName, billReceipt).replace(/\n/g, "\r\n"),
      );
      await openUrl(`mailto:?subject=${subject}&body=${body}`);
    } catch (e: unknown) {
      setError(errMessage(e), BILL_SHEET);
    }
  }
  // SEND-THE-BILL — the alert line where it was raised. The bottom line stays.
  function alertAt(at: string) {
    if (error == null || errorAt !== at) return null;
    return (
      <p className="text-sm text-destructive" role="alert">
        {error}
      </p>
    );
  }
  function orderRow(o: WholesaleOrderView) {
    return (
            <li
              key={o.id}
              ref={(el) => {
                orderRowRefs.current[o.id] = el;
              }}
              className={
                focusedOrderId === o.id
                  ? "flex flex-col gap-2 rounded-md p-3 ring-2 ring-ring"
                  : "flex flex-col gap-2"
              }
            >
              <span>
                {o.venueName} · {o.harvestDate} · {lineSummary(o)} · {stateLabel(o.state)}
              </span>
              {o.state === "delivered" && o.deliveredAgeDays != null && (
                <span className="text-muted-foreground">
                  {deliveredUnpaidAge(o.deliveredAgeDays)}
                </span>
              )}
              {o.pricedTotalCents != null ? (
                <span className="order-first text-base font-semibold tabular-nums">{cents(o.pricedTotalCents)}</span>
              ) : o.lines.some((l) => l.priceCentsPerTray != null) ? (
                <span className="text-muted-foreground">partly unpriced</span>
              ) : null}
              <div className="flex flex-wrap items-center gap-3">
                <Button
                  type="button"
                  variant="outline"
                  className="h-11 px-4 text-base"
                  disabled={busy}
                  onClick={() => void onInvoiceWholesale(o)}
                >
                  Invoice
                </Button>
              </div>
              {/* B1-F2 (D11) — Delivered and Paid… at Button altitude; Void
                  stays a text link. Same handlers, same writes. */}
              {o.state === "ordered" && (
                <div className="flex flex-wrap items-center gap-3">
                  <Button
                    type="button"
                    className="h-11 px-4 text-base"
                    disabled={deliverRefusal[o.id] != null}
                    onClick={() => void onDeliver(o.id)}
                  >
                    Delivered
                  </Button>
                  <button
                    type="button"
                    className="inline-flex min-h-11 items-center rounded-md border border-input px-4 text-sm text-muted-foreground"
                    onClick={() => openVoidWs(o.id)}
                  >
                    Void
                  </button>
                </div>
              )}
              {deliverRefusal[o.id] != null && (
                <p className="text-sm text-amber-700">{deliverRefusal[o.id]}</p>
              )}
              {o.state === "delivered" && o.pricedTotalCents == null && (
                <p className="text-sm text-amber-700">{unpricedLine}</p>
              )}
              {o.state === "delivered" && (
                <div className="flex flex-wrap items-center gap-3">
                  {o.pricedTotalCents != null && (
                    <Button
                      type="button"
                      className="h-11 px-4 text-base"
                      onClick={() => openPay(o)}
                    >
                      Paid…
                    </Button>
                  )}
                  {o.pricedTotalCents != null && o.paymentLinkUrl == null && (
                    <Button type="button" className="h-11 px-4 text-base" disabled={busy} onClick={() => void onMintLink(o.id)}>
                      Payment link
                    </Button>
                  )}
                  <button
                    type="button"
                    className="inline-flex min-h-11 items-center rounded-md border border-input px-4 text-sm text-muted-foreground"
                    onClick={() => openVoidWs(o.id)}
                  >
                    Void
                  </button>
                  {badDebtLine[o.id] != null && (
                    <button
                      type="button"
                      className="inline-flex min-h-11 items-center rounded-md border border-input px-4 text-sm text-muted-foreground"
                      disabled={busy}
                      onClick={() => askWriteOff(o)}
                    >
                      Write off as bad debt
                    </button>
                  )}
                </div>
              )}
              {o.state === "paid" && (
                <button
                  type="button"
                  className="inline-flex min-h-11 items-center rounded-md border border-input px-4 text-sm text-muted-foreground self-start"
                  disabled={busy}
                  onClick={() => askReversePayment(o)}
                >
                  Reverse payment
                </button>
              )}
              {!SETTLED_STATES.has(o.state) && o.paymentLinkUrl != null && (
                <div className="flex flex-col gap-1">
                  <p className="text-sm break-all">Payment link: {o.paymentLinkUrl}</p>
                  <Button type="button" variant="outline" className="self-start text-sm" onClick={() => void onCopyLink(o)}>
                    {linkCopiedId === o.id ? "Copied" : "Copy"}
                  </Button>
                  {(() => {
                    const qr = linkQr[o.id];
                    if (!qr) return null;
                    const side = qr.length + 8;
                    return (
                      <svg
                        role="img"
                        aria-label={`QR code for the ${o.venueName} payment link`}
                        width={200}
                        height={200}
                        viewBox={`0 0 ${side} ${side}`}
                        shapeRendering="crispEdges"
                        className="self-start"
                      >
                        <rect x={0} y={0} width={side} height={side} fill="#fff" />
                        {qr.map((row, y) =>
                          row.map((dark, x) =>
                            dark ? <rect key={`${y}-${x}`} x={x + 4} y={y + 4} width={1} height={1} fill="#000" /> : null,
                          ),
                        )}
                      </svg>
                    );
                  })()}
                  <p className="text-sm text-muted-foreground">A test key takes test cards only. A live key takes real money.</p>
                </div>
              )}
              {reversingId === o.id && (() => {
                const linked = income.find((r) => r.incomeId === o.incomeEventId);
                const amountCents = linked?.amountCents ?? o.pricedTotalCents;
                const amount = amountCents != null ? cents(amountCents) : "";
                const d = o.paidOn
                  ? monthDayLabel(parseLocalDate(o.paidOn))
                  : "";
                return (
                  <div className="flex flex-col gap-2">
                    <p className="text-sm">
                      {reverseT1(amount, o.venueName, d)}
                    </p>
                    <Button
                      type="button"
                      className="h-12 self-start px-4 text-base"
                      disabled={busy}
                      onClick={() => void confirmReversePayment(o)}
                    >
                      Reverse the payment
                    </Button>
                    <button
                      type="button"
                      className="text-sm underline underline-offset-4 self-start"
                      disabled={busy}
                      onClick={() => setReversingId(null)}
                    >
                      Keep it paid
                    </button>
                  </div>
                );
              })()}
              {writingOffId === o.id && (
                <div className="flex flex-col gap-2">
                  <p className="text-sm">{badDebtLine[o.id]}</p>
                  <Button
                    type="button"
                    className="h-12 self-start px-4 text-base"
                    disabled={busy}
                    onClick={() => void confirmWriteOff(o)}
                  >
                    Write it off as bad debt
                  </Button>
                  <button
                    type="button"
                    className="text-sm underline underline-offset-4 self-start"
                    disabled={busy}
                    onClick={() => setWritingOffId(null)}
                  >
                    Keep it owed
                  </button>
                </div>
              )}
              {reverseTrail[o.id] != null && (
                <span className="text-muted-foreground">{reverseTrail[o.id]}</span>
              )}
              {badDebtTrail[o.id] != null && (
                <span className="text-muted-foreground">{badDebtTrail[o.id]}</span>
              )}
              {payingId === o.id && (
                <div className="flex flex-col gap-2">
                  <label className="flex flex-col gap-1">
                    Amount
                    <input
                      className="h-12 rounded-md border border-input bg-card px-3"
                      value={payAmount}
                      onChange={(e) => setPayAmount(e.target.value)}
                    />
                  </label>
                  <label className="flex flex-col gap-1">
                    Date received
                    <input
                      className="h-12 rounded-md border border-input bg-card px-3"
                      value={payDate}
                      onChange={(e) => setPayDate(e.target.value)}
                    />
                  </label>
                  <label className="flex flex-col gap-1">
                    Descriptor
                    <input
                      className="h-12 rounded-md border border-input bg-card px-3"
                      value={payDescriptor}
                      onChange={(e) => setPayDescriptor(e.target.value)}
                    />
                  </label>
                  {pendingPayMismatch != null && (
                    <p className="text-sm font-medium text-destructive">
                      {pendingPayMismatch}
                    </p>
                  )}
                  {pendingPayDuplicate != null && (
                    <p className="text-sm font-medium text-destructive">
                      {pendingPayDuplicate}
                    </p>
                  )}
                  {shortfallFor(o, payAmount) > 0 && (
                    <>
                      <label className="flex flex-col gap-1">
                        Category
                        <select
                          className="h-12 rounded-md border border-input bg-card px-3"
                          value={payWriteOffCategory}
                          onChange={(e) =>
                            setPayWriteOffCategory(e.target.value as WriteOffCategory | "")
                          }
                          required
                        >
                          <option value="">Select a category</option>
                          <option value="sales_discount">Sales discount</option>
                          <option value="quality_spoilage">Quality or spoilage</option>
                          <option value="pricing_or_billing_error">
                            Pricing or billing error
                          </option>
                          <option value="customer_goodwill">Customer goodwill</option>
                          <option value="other">Other</option>
                        </select>
                      </label>
                      {payWriteOffCategory === "other" && (
                        <label className="flex flex-col gap-1">
                          Reason
                          <input
                            type="text"
                            className="h-12 rounded-md border border-input bg-card px-3"
                            value={payWriteOffReason}
                            onChange={(e) => setPayWriteOffReason(e.target.value)}
                            required
                          />
                        </label>
                      )}
                    </>
                  )}
                  {/* B1-F2 (D11) — the collect write at Button altitude, equal
                      to Record order. Same disabled rule, same handler. */}
                  <Button
                    type="button"
                    className="h-12 self-start px-4 text-base"
                    disabled={
                      busy ||
                      (shortfallFor(o, payAmount) > 0 &&
                        (payWriteOffCategory === "" ||
                          (payWriteOffCategory === "other" &&
                            payWriteOffReason.trim() === "")))
                    }
                    onClick={() => void onPayWholesale()}
                  >
                    {busy
                      ? "Saving…"
                      : pendingPayMismatch != null || pendingPayDuplicate != null
                        ? "Record anyway"
                        : "Save"}
                  </Button>
                </div>
              )}
              {/* B1-F2 (D14) — after a deep-linked Save or Delivered the
                  operator stays on Money, sees this row in its new state, and
                  gets one control back to the queue. Navigation only. */}
              {backToTodayId === o.id && onBackToToday != null && (
                <Button
                  type="button"
                  className="h-12 self-start px-4 text-base"
                  onClick={() => {
                    setBackToTodayId(null);
                    onBackToToday();
                  }}
                >
                  Back to Today
                </Button>
              )}
              {voidingId === o.id && (
                <div className="flex flex-col gap-2">
                  <label className="flex flex-col gap-1">
                    Reason
                    <input
                      className="h-12 rounded-md border border-input bg-card px-3"
                      value={voidReasonWs}
                      onChange={(e) => setVoidReasonWs(e.target.value)}
                    />
                  </label>
                  <button
                    type="button"
                    className="text-sm underline underline-offset-4"
                    disabled={busy}
                    onClick={() => void onVoidWholesale()}
                  >
                    {busy ? "Voiding…" : "Confirm void"}
                  </button>
                </div>
              )}
              {alertAt(o.id)}
            </li>
    );
  }

  return (
    <main className="mx-auto flex w-full max-w-md flex-col gap-10 px-6 py-8">
      <h1 className="text-2xl font-semibold tracking-tight">Money</h1>
      <p className="text-sm text-muted-foreground">
        What came in, what went out, what you fixed. Nothing else.
      </p>

      <section className="flex flex-col gap-2">
        {loaded || error != null ? (
          <>
            <p className="text-3xl font-medium tabular-nums">{owedLine(owed)}</p>
            {firstCollect != null ? (
              <button
                type="button"
                onClick={() => setFocusedOrderId(firstCollect.id)}
                className="flex min-h-11 items-center text-left text-base font-medium underline-offset-4 hover:underline active:translate-y-px focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                Collect first: {firstCollect.venueName} —{" "}
                {firstCollect.pricedTotalCents != null
                  ? cents(firstCollect.pricedTotalCents)
                  : "not priced yet"}
                {firstCollect.deliveredAgeDays != null
                  ? `, ${deliveredUnpaidAge(firstCollect.deliveredAgeDays)}`
                  : ""}
                .
              </button>
            ) : orderedCount > 0 ? (
              <p className="text-base font-medium">
                {orderedCount} ordered — nothing delivered to collect yet.
              </p>
            ) : (
              <p className="text-base font-medium">No live wholesale orders.</p>
            )}
            <p className="text-sm text-muted-foreground">
              Wholesale: {openCount} open · {deliveredUnpaid.length} delivered, unpaid
            </p>
          </>
        ) : (
          <div className="flex flex-col gap-2" aria-hidden="true">
            <div className="skeleton-block h-9 w-2/3" />
            <div className="skeleton-block h-11 w-1/2" />
            <div className="skeleton-block h-5 w-2/5" />
          </div>
        )}
      </section>

      <Card>
        <CardHeader>
          <CardTitle>Wholesale</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {focusMissing && (
            <p className="text-sm text-muted-foreground">
              That order is no longer open. The list below is the record.
            </p>
          )}
          <div className="flex flex-col gap-2">
            <h3 className="text-base font-medium">To collect ({liveRows.length})</h3>
            <ul className="flex flex-col gap-4 text-sm">
              {(showAllLive ? liveRows : liveRows.slice(0, LIVE_VISIBLE)).map(orderRow)}
              {!showAllLive && liveRows.length > LIVE_VISIBLE && (
                <li>
                  <button
                    type="button"
                    onClick={() => setShowAllLive(true)}
                    className="px-2 py-3 text-left text-sm text-muted-foreground underline-offset-4 hover:underline active:translate-y-px focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  >
                    Show {liveRows.length - LIVE_VISIBLE} more to collect
                  </button>
                </li>
              )}
              {liveRows.length === 0 && (
                <li className="text-muted-foreground">
                  {wholesaleList.length === 0
                    ? "No wholesale orders."
                    : "Nothing to collect."}
                </li>
              )}
            </ul>
          </div>

          {/* PACK-FACE (audit R-12) — the bill in the face: it renders under
              live To collect and above Settled, so an unpaid row opens it
              without "Show N older settled" first. INV-A fields unchanged. */}
          {invoiceBill != null && (
            <section className="invoice-print flex flex-col gap-2 rounded-md border border-input p-4">
              <h2 className="text-lg font-medium">{farmName}</h2>
              <p className="text-sm">{billTitle(invoiceBill)}</p>
              {invoiceBill.venue != null && (
                <div className="flex flex-col text-sm">
                  <span>{invoiceBill.venue.name}</span>
                  {invoiceBill.venue.contact != null && <span>{invoiceBill.venue.contact}</span>}
                  {invoiceBill.venue.phone != null && <span>{invoiceBill.venue.phone}</span>}
                  {invoiceBill.venue.address != null && <span>{invoiceBill.venue.address}</span>}
                </div>
              )}
              <p className="text-sm text-muted-foreground">
                Harvest {invoiceBill.harvestDate}
                {invoiceBill.deliveredOn != null ? ` · delivered ${invoiceBill.deliveredOn}` : ""}
              </p>
              {invoiceBill.orderLines.map((line, i) => (
                <p key={i} className="text-sm">
                  {line.cropName} · {line.trays} trays × {cents(line.priceCentsPerTray)} = {cents(line.lineTotalCents)}
                </p>
              ))}
              {invoiceBill.leftoverLine != null && (
                <p className="text-sm">
                  Leftover {invoiceBill.leftoverLine.cropName} · {invoiceBill.leftoverLine.harvestedOn} · {invoiceBill.leftoverLine.listedOz.toFixed(1)} oz
                </p>
              )}
              <p className="text-base font-medium">Total {cents(invoiceBill.totalCents)}</p>
              {invoiceBill.paidOn != null && <p className="text-sm">Paid {invoiceBill.paidOn}</p>}
              {invoiceBill.paymentLinkUrl != null && (
                <p className="text-sm break-all text-muted-foreground">Pay online: {invoiceBill.paymentLinkUrl}</p>
              )}
              {/* INTEGRITY-RECEIPT (LINE A / STAMP A / QR A) — one footer line of
                  facts the desk already holds: the farm name Money loaded, the
                  bill's harvest date, the last verify pass H4 reports, the
                  invoice number. A segment with no fact drops with its
                  separator. No code image on the bill; no verify run from here. */}
              <p className="font-mono text-xs text-muted-foreground">{billReceipt}</p>
              <div className="flex items-center gap-3">
                <button
                  type="button"
                  className="text-sm underline underline-offset-4"
                  onClick={() => void onCopyBill()}
                >
                  {billCopiedFor === invoiceBill.number ? "Copied" : "Copy bill"}
                </button>
                <button
                  type="button"
                  className="text-sm underline underline-offset-4"
                  onClick={() => void onEmailBill()}
                >
                  Email bill
                </button>
                <button
                  type="button"
                  className="text-sm underline underline-offset-4"
                  onClick={() => window.print()}
                >
                  Print
                </button>
                <button
                  type="button"
                  className="text-sm underline underline-offset-4"
                  onClick={() => setInvoiceBill(null)}
                >
                  Close
                </button>
              </div>
            </section>
          )}
          {alertAt(BILL_SHEET)}
          {settledRows.length > 0 && (
            <div className="flex flex-col gap-2">
              <h3 className="text-base font-medium text-muted-foreground">
                Settled ({settledRows.length})
              </h3>
              <p className="text-sm text-muted-foreground">
                Paid, written off, and voided. No action here.
              </p>
              <ul className="flex flex-col gap-4 text-sm text-muted-foreground">
                {(showAllSettled
                  ? settledRows
                  : settledRows.slice(0, SETTLED_VISIBLE)
                ).map(orderRow)}
                {!showAllSettled && settledRows.length > SETTLED_VISIBLE && (
                  <li>
                    <button
                      type="button"
                      onClick={() => setShowAllSettled(true)}
                      className="px-2 py-3 text-left text-sm underline-offset-4 hover:underline active:translate-y-px focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      Show {settledRows.length - SETTLED_VISIBLE} older settled
                    </button>
                  </li>
                )}
              </ul>
            </div>
          )}
          {/* B1-F2 (D12) — New order behind a disclosure, collapsed by default.
              The form, its warnings and the overcommit acknowledgement are
              unchanged; only the default visibility moved. */}
          <div className="flex flex-col gap-3">
            <button
              type="button"
              aria-expanded={newOrderOpen}
              onClick={() => setNewOrderOpen((v) => !v)}
              className="flex min-h-11 items-center gap-2 text-left text-sm font-medium focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            >
              <span aria-hidden="true">{newOrderOpen ? "▾" : "▸"}</span>
              New order
            </button>
            {newOrderOpen && (
              <div className="flex flex-col gap-3 animate-in fade-in-0 slide-in-from-top-1 duration-150 motion-reduce:animate-none">
            <label className="flex flex-col gap-1 text-sm">
              Venue
              <select
                className="h-12 rounded-md border border-input bg-card px-3"
                value={newVenueId}
                onChange={(e) => {
                  setNewVenueId(e.target.value);
                  setCopiedFrom(null);
                  setDraftedFromStanding(false);
                }}
              >
                {venues.map((v) => (
                  <option key={v.venueId} value={v.venueId}>
                    {v.name}
                  </option>
                ))}
              </select>
            </label>
                {newVenueId && (() => {
                  const s = stages.find(
                    (row) =>
                      row.venueId === newVenueId && row.stage === "standing",
                  );
                  if (!s) {
                    return (
                      <p className="text-sm text-muted-foreground">Standing: none</p>
                    );
                  }
                  if (s.varietyTargets != null) {
                    const parts = Object.entries(s.varietyTargets).map(
                      ([name, n]) => `${name} ${n}/week`,
                    );
                    return (
                      <p className="text-sm text-muted-foreground">
                        Standing: {parts.join(" · ")}
                      </p>
                    );
                  }
                  return (
                    <p className="text-sm text-muted-foreground">
                      Standing: {s.traysWeek ?? 0}/week total (not yet split by variety)
                    </p>
                  );
                })()}
                {/* B2-F2 (B2-D5a) — a quiet one-tap proposal: copy the venue's
                    most recent prior order (varieties, tray counts, prices) into
                    the draft. Harvest date is never carried. The operator edits
                    anything and Record order remains the only write. */}
                {lastOrderForVenue != null && (
                  <div className="flex flex-col gap-1">
                    <button
                      type="button"
                      className="self-start text-sm underline underline-offset-4"
                      onClick={() => applyLastOrder(lastOrderForVenue)}
                    >
                      Same as last order (
                      {monthDayLabel(parseLocalDate(lastOrderForVenue.harvestDate))})
                    </button>
                    {copiedFrom != null && (
                      <p className="text-sm text-muted-foreground">
                        Copied from the order for{" "}
                        {monthDayLabel(parseLocalDate(copiedFrom.harvestDate))}. Change
                        anything that differs.
                      </p>
                    )}
                  </div>
                )}
                {/* FROM-STANDING — this week's order from the venue's standing
                    line: varieties and tray counts from the split printed
                    above, prices from the venue's last orders. Sits next to
                    Same as last order and shows only for a venue whose
                    standing line is split. The operator edits anything and
                    Record order remains the only write; the standing line
                    itself is never written here. */}
                {standingTargetsForVenue != null && (
                  <div className="flex flex-col gap-1">
                    <button
                      type="button"
                      className="self-start text-sm underline underline-offset-4"
                      onClick={() => applyFromStanding(standingTargetsForVenue)}
                    >
                      From standing
                    </button>
                    {draftedFromStanding && (
                      <p className="text-sm text-muted-foreground">
                        Drafted from standing. Prices are from this venue&apos;s
                        last orders. Change anything that differs.
                      </p>
                    )}
                  </div>
                )}
            <label className="flex flex-col gap-1 text-sm">
              Harvest date
              <input
                className="h-12 rounded-md border border-input bg-card px-3"
                value={newHarvestDate}
                onChange={(e) => setNewHarvestDate(e.target.value)}
              />
            </label>
            {newLines.map((line, idx) => (
              <div key={idx} className="flex flex-col gap-2">
                {entryReachByLine[idx] != null && (
                  <p
                    className={
                      entryReachByLine[idx].reachable
                        ? "text-sm text-muted-foreground"
                        : "text-sm text-amber-700"
                    }
                  >
                    {entryReachByLine[idx].entryLine}
                  </p>
                )}
                <label className="flex flex-col gap-1 text-sm">
                  Variety
                  <select
                    className="h-12 rounded-md border border-input bg-card px-3"
                    value={line.cropId}
                    onChange={(e) => {
                      const next = [...newLines];
                      next[idx] = { ...next[idx], cropId: e.target.value };
                      setNewLines(next);
                    }}
                  >
                    <option value="">Choose variety</option>
                    {crops.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="flex flex-col gap-1 text-sm">
                  Trays
                  <input
                    className="h-12 rounded-md border border-input bg-card px-3"
                    value={line.trays}
                    onChange={(e) => {
                      const next = [...newLines];
                      next[idx] = { ...next[idx], trays: e.target.value };
                      setNewLines(next);
                    }}
                  />
                </label>
                <label className="flex flex-col gap-1 text-sm">
                  Price per tray
                  <input
                    className="h-12 rounded-md border border-input bg-card px-3"
                    value={line.price}
                    onChange={(e) => {
                      const next = [...newLines];
                      next[idx] = { ...next[idx], price: e.target.value };
                      setNewLines(next);
                    }}
                  />
                </label>
              </div>
            ))}
            <button
              type="button"
              className="text-sm underline underline-offset-4 self-start"
              onClick={() => setNewLines([...newLines, emptyDraftLine()])}
            >
              Add variety
            </button>
            {pendingOvercommit != null && (
              <p className="text-sm font-medium text-destructive">
                {pendingOvercommit}
              </p>
            )}
            {newOrderPriceMissing && (
              <p className="text-sm text-amber-700">{PRICE_REQUIRED_LINE}</p>
            )}
            <Button
              type="button"
              className="h-12 self-start px-4 text-base"
              disabled={busy || newOrderPriceMissing}
              onClick={() => void onRecordOrder()}
            >
              {busy
                ? "Saving…"
                : pendingOvercommit != null
                  ? "Record anyway"
                  : "Record order"}
            </Button>
            {alertAt(NEW_ORDER_FORM)}
              </div>
            )}
          </div>
          {/* GT-D23 — the key door sits where Payment link is offered. */}
          {stripeAccount != null && (
            <div className="flex flex-wrap items-center gap-3">
              {stripeAccount.configured ? (
                <>
                  <p className="text-sm text-muted-foreground">
                    Stripe: connected to {stripeAccount.accountName ?? "Stripe"} · {stripeAccount.mode ?? "test"} mode
                  </p>
                  <button type="button" className="text-sm underline underline-offset-4" onClick={openConnect}>
                    Replace key
                  </button>
                </>
              ) : (
                <>
                  <p className="text-sm text-muted-foreground">Stripe: not connected.</p>
                  <Button type="button" variant="outline" className="h-11 px-4 text-base" onClick={openConnect}>
                    Connect Stripe
                  </Button>
                </>
              )}
            </div>
          )}
        </CardContent>
      </Card>
      {/* LO-A (GT-D24) — leftover listings: harvested ounces the operator lists
          for retail. Capacity-free; no Stripe here (income.received is LO-B). */}
      <Card>
        <CardHeader>
          <CardTitle>Leftover</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {leftover.length === 0 ? (
            <p className="text-sm text-muted-foreground">No leftover listed.</p>
          ) : (
            <ul className="flex flex-col gap-2 text-sm">
              {leftover.map((l) => (
                <li key={l.listingId} className="flex flex-col gap-2">
                  <span>
                    {l.cropName} · {l.harvestedOn} · harvested {l.harvestedOz.toFixed(1)} oz · listed {l.listedOz.toFixed(1)} oz
                  </span>
                  {/* PACK-LO-SPEAK (audit R-9, ruling 5): a paid listing shows its
                      total once — the same money span wholesale rows use. Unpriced
                      leftover invents no dollar from ounces. */}
                  {l.paidAt != null && l.pricedTotalCents != null && (
                    <span className="order-first text-base font-semibold tabular-nums">{cents(l.pricedTotalCents)}</span>
                  )}
                  <Button
                    type="button"
                    variant="outline"
                    className="self-start text-sm"
                    disabled={busy}
                    onClick={() => void onInvoiceLeftover(l)}
                  >
                    Invoice
                  </Button>
                  {l.paidAt == null && l.paymentLinkUrl == null && (
                    <div className="flex flex-col gap-2">
                      <label className="flex flex-col gap-1 text-sm">
                        Price
                        <input
                          className="h-12 rounded-md border border-input bg-card px-3"
                          inputMode="decimal"
                          value={leftoverPrice[l.listingId] ?? ""}
                          onChange={(e) =>
                            setLeftoverPrice((m) => ({ ...m, [l.listingId]: e.target.value }))
                          }
                        />
                      </label>
                      <Button
                        type="button"
                        variant="outline"
                        className="self-start text-sm"
                        disabled={busy}
                        onClick={() => void onMintLeftover(l)}
                      >
                        Payment link
                      </Button>
                      <Button
                        type="button"
                        className="h-11 self-start px-4 text-base"
                        disabled={busy}
                        onClick={() => openLoPay(l)}
                      >
                        Paid…
                      </Button>
                      <p className="text-sm text-muted-foreground">A test key takes test cards only. A live key takes real money.</p>
                    </div>
                  )}
                  {l.paidAt == null && l.paymentLinkUrl != null && (
                    <div className="flex flex-col gap-2">
                      <p className="text-sm break-all">Payment link: {l.paymentLinkUrl}</p>
                      <Button
                        type="button"
                        variant="outline"
                        className="self-start text-sm"
                        onClick={() => void onCopyLeftoverLink(l)}
                      >
                        Copy
                      </Button>
                      {(() => {
                        const qr = leftoverQr[l.listingId];
                        if (!qr) return null;
                        const side = qr.length + 8;
                        return (
                          <svg
                            role="img"
                            aria-label={`QR code for the ${l.cropName} · ${l.harvestedOn} payment link`}
                            width={200}
                            height={200}
                            viewBox={`0 0 ${side} ${side}`}
                            shapeRendering="crispEdges"
                            className="self-start"
                          >
                            <rect x={0} y={0} width={side} height={side} fill="#fff" />
                            {qr.map((row, y) =>
                              row.map((dark, x) =>
                                dark ? <rect key={`${y}-${x}`} x={x + 4} y={y + 4} width={1} height={1} fill="#000" /> : null,
                              ),
                            )}
                          </svg>
                        );
                      })()}
                      <Button
                        type="button"
                        className="h-11 self-start px-4 text-base"
                        disabled={busy}
                        onClick={() => openLoPay(l)}
                      >
                        Paid…
                      </Button>
                      <p className="text-sm text-muted-foreground">A test key takes test cards only. A live key takes real money.</p>
                    </div>
                  )}
                  {loPayingId === l.listingId && l.paidAt == null && (
                    <div className="flex flex-col gap-2">
                      <label className="flex flex-col gap-1">
                        Amount
                        <input
                          className="h-12 rounded-md border border-input bg-card px-3"
                          value={loPayAmount}
                          onChange={(e) => setLoPayAmount(e.target.value)}
                        />
                      </label>
                      <label className="flex flex-col gap-1">
                        Date received
                        <input
                          className="h-12 rounded-md border border-input bg-card px-3"
                          value={loPayDate}
                          onChange={(e) => setLoPayDate(e.target.value)}
                        />
                      </label>
                      <label className="flex flex-col gap-1">
                        Descriptor
                        <input
                          className="h-12 rounded-md border border-input bg-card px-3"
                          value={loPayDescriptor}
                          onChange={(e) => setLoPayDescriptor(e.target.value)}
                        />
                      </label>
                      <Button
                        type="button"
                        className="h-12 self-start px-4 text-base"
                        disabled={busy}
                        onClick={() => void onPayLeftoverCash()}
                      >
                        {busy ? "Saving…" : "Save"}
                      </Button>
                      <button
                        type="button"
                        className="text-sm underline underline-offset-4"
                        onClick={() => setLoPayingId(null)}
                      >
                        Cancel
                      </button>
                    </div>
                  )}
                </li>
              ))}
            </ul>
          )}
          <div className="flex flex-col gap-3">
            {/* SHOP DOOR Job A (WHERE A) — one button beside List leftover: the
                PC writes <farm folder>/shop/index.html from the minted, unpaid
                lots above and shows where it landed. Hosting stays by hand;
                nothing here mints, polls, or touches an offer. */}
            <div className="flex flex-wrap items-center gap-3">
              <button
                type="button"
                aria-expanded={leftoverOpen}
                onClick={() => setLeftoverOpen((v) => !v)}
                className="flex min-h-11 items-center gap-2 text-left text-sm font-medium focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                <span aria-hidden="true">{leftoverOpen ? "▾" : "▸"}</span>
                List leftover
              </button>
              <Button
                type="button"
                variant="outline"
                className="text-sm"
                disabled={busy}
                onClick={() => void onWriteShopPage()}
              >
                Write shop page
              </Button>
            </div>
            {shopPage != null && (
              <div className="flex flex-col gap-2">
                <p className="text-sm break-all">Shop page written: {shopPage.filePath}</p>
                <Button
                  type="button"
                  variant="outline"
                  className="self-start text-sm"
                  onClick={() => void onOpenShopFolder()}
                >
                  Open folder
                </Button>
              </div>
            )}
            <p className="text-sm text-muted-foreground">A test key takes test cards only. A live key takes real money.</p>
            {leftoverOpen && (
              <div className="flex flex-col gap-3 animate-in fade-in-0 slide-in-from-top-1 duration-150 motion-reduce:animate-none">
                <label className="flex flex-col gap-1 text-sm">
                  Crop
                  <select
                    className="h-12 rounded-md border border-input bg-card px-3"
                    value={leftoverCropId}
                    onChange={(e) => setLeftoverCropId(e.target.value)}
                  >
                    {crops.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="flex flex-col gap-1 text-sm">
                  Harvest day
                  <input
                    className="h-12 rounded-md border border-input bg-card px-3"
                    value={leftoverDay}
                    onChange={(e) => setLeftoverDay(e.target.value)}
                  />
                </label>
                <label className="flex flex-col gap-1 text-sm">
                  Ounces
                  <input
                    className="h-12 rounded-md border border-input bg-card px-3"
                    inputMode="decimal"
                    value={leftoverOz}
                    onChange={(e) => setLeftoverOz(e.target.value)}
                  />
                </label>
                <Button
                  type="button"
                  className="h-12 self-start px-4 text-base"
                  disabled={busy}
                  onClick={() => void onListLeftover()}
                >
                  {busy ? "Saving…" : "List leftover"}
                </Button>
              </div>
            )}
          </div>
        </CardContent>
      </Card>

      {/* The six capture doors, collapsed by default — the same call Today and Farm
          already make (Today.tsx:1183, Reality.tsx:507). MoneyCaptureControls is not
          edited: same doors, same labels, same handlers, same sheets, same validation.
          Its own disclosure carries the words "Record money" (MoneyCaptureControls.tsx
          :33-37), so the h2 would have printed the label twice. The blurb went with it
          because its only job was explaining why Money kept these open while the other
          two tabs collapsed them — an asymmetry this line removes. */}
      <MoneyCaptureControls collapsible />

      <Card>
        <CardHeader>
          <CardTitle>Cash in</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-3xl font-semibold tabular-nums">{cents(cashIn)}</p>
          <ul className="flex flex-col gap-2 text-sm">
            {(showAllIncome ? income : income.slice(0, INCOME_VISIBLE)).map((r) => {
              const applied = orderForIncome(r.incomeId);
              const target = deliveredUnpaid.find((o) => o.id === linkOrderId) ?? null;
              const shortfallCents =
                target?.pricedTotalCents != null && r.amountCents < target.pricedTotalCents
                  ? target.pricedTotalCents - r.amountCents
                  : 0;
              return (
                <li key={r.incomeId} className="flex flex-col gap-2">
                  <span className="tabular-nums">
                    {r.dateReceived} · {cents(r.amountCents)} · {r.source}
                  </span>
                  {applied != null ? (
                    <span className="text-muted-foreground">
                      applied to {applied.venueName} · {applied.harvestDate}
                    </span>
                  ) : deliveredUnpaid.length > 0 && linkingIncomeId !== r.incomeId ? (
                    <button
                      type="button"
                      className="self-start text-sm underline underline-offset-4"
                      onClick={() => {
                        setError(null);
                        setLinkingIncomeId(r.incomeId);
                        // B2-F2 (B2-D7) — default to the exact-amount match, else the oldest debt.
                        setLinkOrderId(defaultApplyTarget(r.amountCents));
                      }}
                    >
                      Apply to an order…
                    </button>
                  ) : null}
                  {linkingIncomeId === r.incomeId && (
                    <div className="flex flex-col gap-2">
                      <label className="flex flex-col gap-1">
                        Order
                        <select
                          className="h-12 rounded-md border border-input bg-card px-3"
                          value={linkOrderId}
                          onChange={(e) => setLinkOrderId(e.target.value)}
                        >
                          {deliveredUnpaid.map((o) => (
                            <option key={o.id} value={o.id}>
                              {o.venueName} · {o.harvestDate} ·{" "}
                              {o.pricedTotalCents != null
                                ? cents(o.pricedTotalCents)
                                : "unpriced"}
                            </option>
                          ))}
                        </select>
                      </label>
                      {(deliveredUnpaid.find((x) => x.id === linkOrderId)
                        ?.pricedTotalCents ?? null) == null && (
                        <p className="text-sm text-amber-700">{unpricedLine}</p>
                      )}
                      {pendingLinkMismatch != null && (
                        <p className="text-sm font-medium text-destructive">
                          {pendingLinkMismatch}
                        </p>
                      )}
                      {shortfallCents > 0 && (
                        <>
                          <label className="flex flex-col gap-1">
                            Category
                            <select
                              className="h-12 rounded-md border border-input bg-card px-3"
                              value={writeOffCategory}
                              onChange={(e) =>
                                setWriteOffCategory(e.target.value as WriteOffCategory | "")
                              }
                              required
                            >
                              <option value="">Select a category</option>
                              <option value="sales_discount">Sales discount</option>
                              <option value="quality_spoilage">Quality or spoilage</option>
                              <option value="pricing_or_billing_error">
                                Pricing or billing error
                              </option>
                              <option value="customer_goodwill">Customer goodwill</option>
                              <option value="other">Other</option>
                            </select>
                          </label>
                          {writeOffCategory === "other" && (
                            <label className="flex flex-col gap-1">
                              Reason
                              <input
                                type="text"
                                className="h-12 rounded-md border border-input bg-card px-3"
                                value={writeOffReason}
                                onChange={(e) => setWriteOffReason(e.target.value)}
                                required
                              />
                            </label>
                          )}
                        </>
                      )}
                      <div className="flex gap-3">
                        <button
                          type="button"
                          className="text-sm underline underline-offset-4"
                          disabled={
                            busy ||
                            (shortfallCents > 0 &&
                              (writeOffCategory === "" ||
                                (writeOffCategory === "other" &&
                                  writeOffReason.trim() === ""))) ||
                            (deliveredUnpaid.find((x) => x.id === linkOrderId)
                              ?.pricedTotalCents ?? null) == null
                          }
                          onClick={() => void onApplyIncome(r)}
                        >
                          {busy
                            ? "Applying…"
                            : pendingLinkMismatch != null
                              ? "Apply anyway"
                              : "Apply"}
                        </button>
                        <button
                          type="button"
                          className="text-sm underline underline-offset-4"
                          onClick={() => {
                            setLinkingIncomeId(null);
                            setPendingLinkMismatch(null);
                          }}
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  )}
                </li>
              );
            })}
            {!showAllIncome && income.length > INCOME_VISIBLE && (
              <li>
                <button
                  type="button"
                  onClick={() => setShowAllIncome(true)}
                  className="px-2 py-3 text-left text-sm text-muted-foreground underline-offset-4 hover:underline active:translate-y-px focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  Show older ({income.length - INCOME_VISIBLE})
                </button>
              </li>
            )}
            {income.length === 0 && (
              <li className="text-muted-foreground">No income recorded.</li>
            )}
          </ul>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Cash out</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-3xl font-semibold tabular-nums">{cents(cashOut)}</p>
          <ul className="flex flex-col gap-3 text-sm">
            {(showAllExpenses ? expenses : expenses.slice(0, MONEY_LIST_VISIBLE)).map((r) => (
              <li key={r.eventId} className="flex flex-col gap-2">
                <span className="tabular-nums">
                  {r.datePaid} · {cents(r.amountCents)} · {r.payee}
                </span>
                <div className="flex gap-3">
                  <button
                    type="button"
                    className="text-sm underline underline-offset-4"
                    onClick={() => openCorrect(r)}
                  >
                    Correct
                  </button>
                  <button
                    type="button"
                    className="inline-flex min-h-11 items-center rounded-md border border-input px-4 text-sm text-muted-foreground"
                    onClick={() => openVoid(r)}
                  >
                    Void
                  </button>
                </div>
              </li>
            ))}
            {!showAllExpenses && expenses.length > MONEY_LIST_VISIBLE && (
              <li>
                <button
                  type="button"
                  onClick={() => setShowAllExpenses(true)}
                  className="px-2 py-3 text-left text-sm text-muted-foreground underline-offset-4 hover:underline active:translate-y-px focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  Show older ({expenses.length - MONEY_LIST_VISIBLE})
                </button>
              </li>
            )}
            {expenses.length === 0 && (
              <li className="text-muted-foreground">No expenses recorded.</li>
            )}
          </ul>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Expense corrections</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <ul className="flex flex-col gap-3 text-sm">
            {(showAllCorrections ? corrections : corrections.slice(0, MONEY_LIST_VISIBLE)).map((c) => (
              <li key={c.correctionEventId} className="flex flex-col gap-1">
                <span>
                  {c.action} · {c.track} · {c.correctedAt}
                </span>
                <span className="text-muted-foreground">
                  before {cents(c.beforeAmountCents)} on {c.beforeDate} ({c.beforePayee})
                </span>
                {c.afterAmountCents != null && c.afterDate && c.afterPayee != null ? (
                  <span className="text-muted-foreground">
                    after {cents(c.afterAmountCents)} on {c.afterDate} ({c.afterPayee})
                  </span>
                ) : (
                  <span className="text-muted-foreground">after — voided</span>
                )}
                {c.reason && (
                  <span className="text-muted-foreground">reason: {c.reason}</span>
                )}
              </li>
            ))}
            {!showAllCorrections && corrections.length > MONEY_LIST_VISIBLE && (
              <li>
                <button
                  type="button"
                  onClick={() => setShowAllCorrections(true)}
                  className="px-2 py-3 text-left text-sm text-muted-foreground underline-offset-4 hover:underline active:translate-y-px focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  Show older ({corrections.length - MONEY_LIST_VISIBLE})
                </button>
              </li>
            )}
            {corrections.length === 0 && (
              <li className="text-muted-foreground">No corrections yet.</li>
            )}
          </ul>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Stripe money not recorded</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {/* B1-F2 (D13) — the Retail paid-orders line lives with the Stripe
              record it describes, not in the daily summary. */}
          <p className="text-sm text-muted-foreground">
            Retail: {retailOrders.length} paid orders
          </p>
          <ul className="flex flex-col gap-3 text-sm">
            {(showAllUnapplied ? unapplied : unapplied.slice(0, MONEY_LIST_VISIBLE)).map((f) => (
              <li key={f.eventId} className="flex flex-col gap-1">
                <span className="tabular-nums">
                  {observedDate(f.observedAt)} · {unappliedAmount(f.amountCents)} ·{" "}
                  {unappliedSentence(f.status, f.stripeObject)}
                </span>
                <span className="text-muted-foreground">
                  Stripe {stripeObjectWord(f.stripeObject)} {f.stripeId}
                </span>
              </li>
            ))}
            {!showAllUnapplied && unapplied.length > MONEY_LIST_VISIBLE && (
              <li>
                <button
                  type="button"
                  onClick={() => setShowAllUnapplied(true)}
                  className="px-2 py-3 text-left text-sm text-muted-foreground underline-offset-4 hover:underline active:translate-y-px focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  Show older ({unapplied.length - MONEY_LIST_VISIBLE})
                </button>
              </li>
            )}
            {unapplied.length === 0 && (
              <li className="text-muted-foreground">Nothing unrecorded.</li>
            )}
          </ul>
        </CardContent>
      </Card>

      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}

      <Sheet
        open={correctTarget != null}
        onOpenChange={(open) => {
          if (!open) setCorrectTarget(null);
        }}
      >
        <SheetContent side="bottom" showCloseButton={false} aria-describedby={undefined}>
          <div className="mx-auto flex w-full max-w-md flex-col gap-4 p-4 pb-8">
            <SheetTitle className="text-xl font-medium">Correct expense</SheetTitle>
            <label className="flex flex-col gap-1 text-sm">
              Amount
              <input
                className="h-12 rounded-md border border-input bg-card px-3"
                value={amount}
                onChange={(e) => setAmount(e.target.value)}
              />
            </label>
            <label className="flex flex-col gap-1 text-sm">
              Payee
              <input
                className="h-12 rounded-md border border-input bg-card px-3"
                value={payee}
                onChange={(e) => setPayee(e.target.value)}
              />
            </label>
            <label className="flex flex-col gap-1 text-sm">
              Category
              <select
                className="h-12 rounded-md border border-input bg-card px-3"
                value={categoryId}
                onChange={(e) => setCategoryId(e.target.value)}
              >
                {categories.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1 text-sm">
              Date paid
              <input
                className="h-12 rounded-md border border-input bg-card px-3"
                value={datePaid}
                onChange={(e) => setDatePaid(e.target.value)}
              />
            </label>
            <label className="flex flex-col gap-1 text-sm">
              Description
              <input
                className="h-12 rounded-md border border-input bg-card px-3"
                value={descriptor}
                onChange={(e) => setDescriptor(e.target.value)}
              />
            </label>
            <label className="flex flex-col gap-1 text-sm">
              Reason
              <input
                className="h-12 rounded-md border border-input bg-card px-3"
                value={reason}
                onChange={(e) => setReason(e.target.value)}
              />
            </label>
            <Button
              type="button"
              className="h-14 text-base"
              disabled={busy}
              onClick={() => void onCorrect()}
            >
              {busy ? "Saving…" : "Save correction"}
            </Button>
            <Button
              type="button"
              variant="ghost"
              className="h-12 text-base"
              disabled={busy}
              onClick={() => setCorrectTarget(null)}
            >
              Cancel
            </Button>
          </div>
        </SheetContent>
      </Sheet>

      <Sheet
        open={voidTarget != null}
        onOpenChange={(open) => {
          if (!open) setVoidTarget(null);
        }}
      >
        <SheetContent side="bottom" showCloseButton={false} aria-describedby={undefined}>
          <div className="mx-auto flex w-full max-w-md flex-col gap-4 p-4 pb-8">
            <SheetTitle className="text-xl font-medium">Void expense</SheetTitle>
            <p className="text-sm text-muted-foreground">
              The figure stays readable in the Corrections trail. It leaves the
              active list.
            </p>
            <label className="flex flex-col gap-1 text-sm">
              Reason
              <input
                className="h-12 rounded-md border border-input bg-card px-3"
                value={reason}
                onChange={(e) => setReason(e.target.value)}
              />
            </label>
            <Button
              type="button"
              className="h-14 text-base"
              disabled={busy}
              onClick={() => void onVoid()}
            >
              {busy ? "Voiding…" : "Void"}
            </Button>
            <Button
              type="button"
              variant="ghost"
              className="h-12 text-base"
              disabled={busy}
              onClick={() => setVoidTarget(null)}
            >
              Cancel
            </Button>
          </div>
        </SheetContent>
      </Sheet>

      {/* GT-D23 — Connect Stripe. The paste and confirm copy and the scope list
          are the unmounted shop sheet's, reused; that component stays unmounted. */}
      <Sheet
        open={connectStep !== "closed"}
        onOpenChange={(open) => {
          if (!open) closeConnect();
        }}
      >
        <SheetContent side="bottom" showCloseButton={false} aria-describedby={undefined}>
          <div className="mx-auto flex w-full max-w-md flex-col gap-4 p-4 pb-8">
            <SheetTitle className="text-xl font-medium">Connect Stripe</SheetTitle>
            {connectStep === "paste" && (
              <div className="flex flex-col gap-4">
                <p className="text-base text-muted-foreground">
                  Groundtruth mints payment links with a Stripe restricted key. You create a
                  restricted key once; nothing else to manage day to day.
                </p>
                <ol className="list-decimal space-y-2 pl-5 text-sm text-muted-foreground">
                  <li>
                    In the Stripe Dashboard, open Developers → API keys →
                    Restricted keys → Create restricted key. Test mode for a
                    rehearsal key; live mode for one that takes real money.
                  </li>
                  <li>
                    Turn on <span className="text-foreground">write</span> for
                    Products, Prices, and Payment Links.
                  </li>
                  <li>
                    Turn on <span className="text-foreground">read</span> for
                    Checkout Sessions, Refunds, Disputes, and Account. Leave
                    everything else off.
                  </li>
                  <li>Create the key and paste it below.</li>
                </ol>
                <p className="text-base text-muted-foreground">
                  Groundtruth accepts a test key or a live key. A live key moves real
                  money the first time a customer pays.
                </p>
                <p className="text-base text-muted-foreground">
                  Every payment link Groundtruth mints is billed in US dollars.
                </p>
                <label className="flex flex-col gap-2">
                  <span className="text-sm font-medium">Restricted key</span>
                  <input
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    value={connectKey}
                    onChange={(e) => setConnectKey(e.target.value)}
                    placeholder="rk_test_… or rk_live_…"
                    className="min-h-12 rounded-md border border-input bg-card px-3 text-base outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  />
                </label>
                <Button
                  type="button"
                  className="min-h-12"
                  disabled={connectBusy || connectKey.trim().length === 0}
                  onClick={() => void onConnectPreview()}
                >
                  {connectBusy ? "Checking…" : "Continue"}
                </Button>
              </div>
            )}
            {connectStep === "confirm" && connectPreview && (
              <div className="flex flex-col gap-4">
                <div className="flex flex-col gap-1">
                  <p className="text-lg font-medium">{connectPreview.accountName}</p>
                  <p className="text-sm text-muted-foreground">{connectPreview.accountId}</p>
                  <p className="text-sm text-muted-foreground">
                    {connectPreview.mode === "test" ? "Test mode" : `${connectPreview.mode} mode`}
                  </p>
                </div>
                <Button
                  type="button"
                  className="min-h-12"
                  disabled={connectBusy}
                  onClick={() => void onConnectConfirm()}
                >
                  {connectBusy ? "Connecting…" : "Connect this account"}
                </Button>
                <button
                  type="button"
                  className="text-left text-sm text-muted-foreground underline-offset-4 hover:underline"
                  onClick={() => {
                    setConnectPreview(null);
                    setConnectStep("paste");
                  }}
                >
                  Use a different key
                </button>
              </div>
            )}
            {connectError && (
              <p className="text-sm text-destructive" role="alert">
                {connectError}
              </p>
            )}
            <Button
              type="button"
              variant="ghost"
              className="h-12 text-base"
              disabled={connectBusy}
              onClick={closeConnect}
            >
              Cancel
            </Button>
          </div>
        </SheetContent>
      </Sheet>
    </main>
  );
}
