import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type {
  AttentionItem,
  CommitmentLine,
  CoverDate,
  CoverageRef,
  Crop,
  FarmCurrencyView,
  HarvestCommitmentsView,
  HarvestGroup,
  HarvestInput,
  OwedSummary,
  PhoneCaptureView,
  SeedOnHandRow,
  ShelfCapacity,
  StageView,
  StandingDemandView,
  TodayView,
  TrayView,
  VenuePackView,
  WholesaleOrderView,
  DockFoldsView,
} from "@/farm/types";
import {
  advanceTrays,
  checkAttention,
  confirmPhoneCaptures,
  coverPlan,
  discardPhoneCapture,
  farmCurrency,
  dismissAttention,
  farmUnits,
  harvestCommitments,
  earlyHarvestGroups,
  harvestGroups,
  listCrops,
  listStages,
  listTrays,
  listVenues,
  listWholesaleOrders,
  owedSummary,
  packByCustomer,
  phoneCaptures,
  recordHarvestCoverage,
  recordLeftoverListing,
  resolveAttention,
  scanConfig,
  seedOnHand,
  shelfCapacity,
  sowTray,
  standingDemand,
  todayView,
  undoLast,
  dockFolds,
} from "@/farm/api";
import { monthDayLabel, parseLocalDate } from "@/farm/dates";
import { formatCents } from "@/farm/dollars";
import { DEFAULT_UNITS, massFigure, perTrayFigure, perTrayUnit, unitWord } from "@/farm/mass";
import { typedOunces } from "@/farm/typed";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { ErrorLine } from "@/components/ErrorLine";
import { SowSheet } from "@/components/SowSheet";
import { WeightPad } from "@/components/WeightPad";
import { FarmBackupSheet } from "@/components/FarmBackupSheet";
import { MoneyCaptureControls } from "@/components/MoneyCaptureControls";
import {
  isMoneyDebtKind,
  moneyIntentForKind,
  sowableGapsRemain,
  surfaceForKind,
} from "@/farm/surfaces";
import type { MoneyFocus } from "@/farm/surfaces";
function trayCountLabel(quantity: number): string {
  return `${quantity} ${quantity === 1 ? "tray" : "trays"}`;
}
/**
 * CUT-1 — the cut list is the pack list inverted: one line per crop, trays
 * summed across every venue pack for the same harvest date, then which venue
 * packs those trays fill. Read from the VenuePackView[] the pack card already
 * holds (pack_by_customer): no command, no write, no clock — the date printed
 * is the packs' own. A venue's concatenated lines for one crop (two orders,
 * same crop) sum into one venue count. Crops sort by crops.sortOrder, cropId
 * breaking ties — the order load_lines already gives a pack.
 */
type CutLine = {
  cropId: string;
  cropName: string;
  trays: number;
  venues: { venueName: string; trays: number }[];
};
function cutLinesFor(packs: VenuePackView[], crops: Crop[]): CutLine[] {
  const order = new Map(crops.map((c) => [c.id, c.sortOrder] as const));
  const byCrop = new Map<string, CutLine>();
  for (const pack of packs) {
    for (const line of pack.lines) {
      let cut = byCrop.get(line.cropId);
      if (cut == null) {
        cut = { cropId: line.cropId, cropName: line.cropName, trays: 0, venues: [] };
        byCrop.set(line.cropId, cut);
      }
      cut.trays += line.trays;
      const venue = cut.venues.find((v) => v.venueName === pack.venueName);
      if (venue != null) venue.trays += line.trays;
      else cut.venues.push({ venueName: pack.venueName, trays: line.trays });
    }
  }
  const rank = (id: string) => order.get(id) ?? Number.MAX_SAFE_INTEGER;
  return [...byCrop.values()].sort(
    (a, b) =>
      rank(a.cropId) - rank(b.cropId) ||
      (a.cropId < b.cropId ? -1 : a.cropId > b.cropId ? 1 : 0),
  );
}
/** One cut line's bytes: {crop} · {n} trays · {venue} {n} · {venue} {n} */
function cutLineText(cut: CutLine): string {
  return [
    `${cut.cropName} · ${trayCountLabel(cut.trays)}`,
    ...cut.venues.map((v) => `${v.venueName} ${v.trays}`),
  ].join(" · ");
}
/**
 * ROUTE (CONTENTS B / JOIN id) — the run page's second line for a pack:
 * address · phone, read off the VenuePackView the pack card already holds
 * (pack_by_customer carries venueId, address and phone, joined on venue_id
 * in the engine — never on the venue name). A null or blank segment drops
 * with its dot; neither present, no line at all. Nothing is invented here:
 * no day of the week, no sequence, no map — the venue row's own words.
 */
function runContactLine(pack: VenuePackView): string | null {
  const segments = [pack.address, pack.phone]
    .map((s) => s?.trim() ?? "")
    .filter((s) => s.length > 0);
  return segments.length > 0 ? segments.join(" · ") : null;
}
/**
 * CUT-DATE (DATE A) — the chips: every distinct harvest date still carrying
 * an ordered wholesale row, ascending, read from the rows Today already loads
 * (list_wholesale_orders). No clock here: "today" is the engine's pin (a null
 * harvest date to pack_by_customer), never a JS date. Unreadable rows, no chips.
 */
function orderedDatesFor(rows: WholesaleOrderView[] | null): string[] {
  const dates = new Set<string>();
  for (const row of rows ?? []) {
    if (row.state === "ordered") dates.add(row.harvestDate);
  }
  return [...dates].sort();
}
/**
 * CUT-DATE (STANDING A) — the standing block on the cut page: a weekly rate
 * printed beside the date's cut, never divided by seven, never pinned to a
 * day of the week, never netted against or added into the crop lines
 * (DOUBLE-COUNT A: both print). Read from the StageView[] the probe already
 * loads (list_stages): one line per venue x crop in varietyTargets; an
 * unsplit venue (no targets) is one line at its traysWeek. Venues in the order
 * list_stages gives (venue name); within a venue, crops in crops.sortOrder by
 * name, names no crop carries last, A-Z. The same block prints for every date
 * that prints a cut.
 */
type StandingLine = { key: string; text: string };
function standingLinesFor(stages: StageView[], crops: Crop[]): StandingLine[] {
  const rank = new Map(crops.map((c) => [c.name, c.sortOrder] as const));
  const rankOf = (name: string) => rank.get(name) ?? Number.MAX_SAFE_INTEGER;
  const lines: StandingLine[] = [];
  for (const s of stages) {
    if (s.stage !== "standing") continue;
    if (s.varietyTargets == null) {
      lines.push({ key: s.venueId, text: `${s.venueName} · ${s.traysWeek ?? 0}/week` });
      continue;
    }
    const split = Object.entries(s.varietyTargets).sort(
      ([a], [b]) => rankOf(a) - rankOf(b) || (a < b ? -1 : a > b ? 1 : 0),
    );
    for (const [name, n] of split) {
      lines.push({ key: `${s.venueId}|${name}`, text: `${s.venueName} · ${name} · ${n}/week` });
    }
  }
  return lines;
}
/**
 * YIELD-MEMORY (A) — this crop's last harvests as weighed, on the harvest
 * receipt only. Read from the TrayView[] the probe already loads (list_trays,
 * the same rows leftover.rs caps from): a day key is one harvestedOn among the
 * crop's harvested rows; its figure is the sum of actualYieldOz over the sum
 * of quantity across the rows that carry a weight, at 0.1 (leftover.rs
 * round_tenth). Today is the receipt's own harvestedOn — the engine day the
 * coverage view carries — never a clock read here. Three earlier keys
 * strictly before today, newest first; a key with no weighed row still takes
 * one of the three seats and says so. No estimate enters: only weights that
 * were recorded. Display only — nothing here feeds standing, sow, seed or
 * leftover, and the leftover ounces below stay the operator's to type.
 */
const YIELD_MEMORY_EARLIER = 3;
/** The unrounded mean ounces per tray over the rows that carry a weight; null
 *  when none does. GT-D27 UNITS (PRECISION G): the figure is rounded once, on
 *  the face, by mass.ts perTrayFigure - imperial to the shipped 0.1, metric to
 *  whole grams from this mean, so nothing is rounded twice. */
function ozPerTray(rows: TrayView[]): number | null {
  let oz = 0;
  let trays = 0;
  for (const t of rows) {
    if (t.actualYieldOz == null) continue;
    oz += t.actualYieldOz;
    trays += t.quantity;
  }
  if (trays === 0) return null;
  return oz / trays;
}
/** The line's bytes, or null while today's key has no weighed row to speak from. */
function yieldMemoryFor(
  trays: TrayView[],
  cropId: string,
  cropName: string,
  todayKey: string,
  system: string,
): string | null {
  const harvested = trays.filter(
    (t) => t.cropId === cropId && t.state === "harvested" && t.harvestedOn != null,
  );
  const todayMean = ozPerTray(harvested.filter((t) => t.harvestedOn === todayKey));
  if (todayMean == null) return null;
  const today = perTrayFigure(todayMean, system);
  const keys = new Set<string>();
  for (const t of harvested) {
    if (t.harvestedOn != null && t.harvestedOn < todayKey) keys.add(t.harvestedOn);
  }
  const earlier = [...keys]
    .sort()
    .reverse()
    .slice(0, YIELD_MEMORY_EARLIER)
    .map((k) => {
      const label = monthDayLabel(parseLocalDate(k));
      const mean = ozPerTray(harvested.filter((t) => t.harvestedOn === k));
      return mean == null
        ? `${label} weight not recorded`
        : `${label} ${perTrayFigure(mean, system)}`;
    });
  if (earlier.length === 0) {
    return `Yield today ${today} ${perTrayUnit(system)} · first harvest of ${cropName} on the books.`;
  }
  return `Yield today ${today} ${perTrayUnit(system)} · earlier ${earlier.join(" · ")}.`;
}
/**
 * YIELD-STANDING (A) — the SOP-4 standing-week card's quiet second line:
 * the newest weighed harvest day of the variety's crop, as weighed, dated.
 * Read from the same TrayView[] the receipt's yield line reads (list_trays,
 * already in trayRows); the variety name resolves to a crop on the crops
 * Today already holds, so a name no crop carries prints nothing. Day keys
 * newest first; a key with no weighed row is skipped for the next-newest
 * weighed key (ozPerTray's mean, rounded once on the face); no weighed key,
 * no line. No clock: the newest key is the greatest harvestedOn string on
 * the rows. Display only — nothing here feeds standing_demand, the sow door,
 * seed or leftover, and the SOP-4 sentence above it is untouched.
 */
function lastWeighedFor(trays: TrayView[], cropId: string, system: string): string | null {
  const harvested = trays.filter(
    (t) => t.cropId === cropId && t.state === "harvested" && t.harvestedOn != null,
  );
  const keys = new Set<string>();
  for (const t of harvested) {
    if (t.harvestedOn != null) keys.add(t.harvestedOn);
  }
  for (const k of [...keys].sort().reverse()) {
    const mean = ozPerTray(harvested.filter((t) => t.harvestedOn === k));
    if (mean != null) {
      return `last weighed ${perTrayFigure(mean, system)} ${perTrayUnit(system)} · ${monthDayLabel(parseLocalDate(k))}`;
    }
  }
  return null;
}
/**
 * TODAY-JAR (MOVE 2) — the SOP-4 standing-week card's third muted line: the
 * variety's jar, as seedOnHand() answers it (the same reader Farm prints),
 * only while the variety resolves to a crop and that crop has a receipt.
 * The sentence is said only when the ledger alone proves it — trays still
 * short and the jar at or below zero, because no seed sows no tray. With
 * seed still in the jar the line is the figure and nothing more: "enough
 * for N trays" would take an ounces-per-tray this app does not hold as a
 * fact, so nothing is multiplied here. The desk's units, on the face only.
 * Display only — nothing here feeds standing_demand, the sow door, seed or
 * leftover, and the SOP-4 sentence above it is untouched.
 */
function seedLineFor(
  v: StandingDemandView["varieties"][number],
  row: SeedOnHandRow,
  system: string,
): string {
  if (row.onHandOz === null) return "seed on hand unknown";
  const x = massFigure(Math.abs(row.onHandOz), system);
  const u = unitWord(system);
  if (v.shortfall > 0 && row.onHandOz <= 0) {
    return row.onHandOz === 0
      ? `you promised ${v.traysWeek} trays of ${v.name} and the jar is empty`
      : `you promised ${v.traysWeek} trays of ${v.name} and the jar is short ${x} ${u}`;
  }
  return row.onHandOz < 0 ? `jar short ${x} ${u}` : `${x} ${u} of seed on hand`;
}
/**
 * OWED-LO (audit R-2): mirrored byte-for-byte with Money.tsx owedLine
 * (owed_lo_tests pins the mirror); only the cents formatter differs.
 * Priced-unpaid leftover blocks the empty line and joins the count and the
 * total. Unpriced leftover carries no cents and never appears here.
 */
function owedLine(o: OwedSummary | null, symbol = "$"): string {
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
      ? `Owed to you: ${formatCents(totalCents, symbol)} across ${across}`
      : `Owed to you: ${across}, value partly unpriced`;
  const age =
    o.oldestDays == null
      ? null
      : o.oldestDays === 0
        ? "delivered today"
        : `oldest ${o.oldestDays} ${o.oldestDays === 1 ? "day" : "days"}`;
  return age == null ? `${what}.` : `${what} — ${age}.`;
}
function deliveryWord(n: number): string {
  return `${n} ${n === 1 ? "delivery" : "deliveries"}`;
}
/**
 * B1-F1 (D2) — two card grammars, assigned by POSITION in the queue, never by
 * kind. The first row of the sorted queue is the only row rendered large; every
 * other row takes the attention-card grammar. Neither grammar is new: "large" is
 * the pre-split action / confirm row, "card" is the attention card. Ranks are
 * untouched (surfaces.ts).
 */
/** Large, single-action: the whole card is the tap. Unchanged from the pre-split screen. */
const actionCardClass =
  "flex min-h-24 cursor-pointer items-center justify-center p-8 text-center text-xl font-medium transition-colors active:translate-y-px hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";
/** Card, single-action: the whole card is still the tap (D2 — tap targets do not shrink). */
const tapCardClass =
  "flex min-h-16 cursor-pointer items-center p-6 text-left text-base font-medium transition-colors active:translate-y-px hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";
function tapCardClassFor(large: boolean): string {
  return large ? actionCardClass : tapCardClass;
}
/** Confirmation with its Undo ("Moved…", "Discarded…", "Dismissed.", "Farm restored…"). */
function confirmCardClassFor(large: boolean): string {
  return large
    ? "flex min-h-24 items-center justify-between gap-4 p-8 text-xl font-medium"
    : "flex items-center justify-between gap-4 p-6 text-base font-medium";
}
/** The harvested confirmation, which also carries the GT-D19 coverage lines. */
function harvestConfirmClassFor(large: boolean): string {
  return large
    ? "flex min-h-24 flex-col gap-4 p-8 text-xl font-medium"
    : "flex flex-col gap-4 p-6 text-base font-medium";
}
/** Attention card wrapper and message. */
function cardClassFor(large: boolean): string {
  return large ? "flex flex-col gap-4 p-8" : "flex flex-col gap-4 p-6";
}
function messageClassFor(large: boolean): string {
  return large
    ? "text-xl font-medium leading-snug"
    : "text-base font-medium leading-snug";
}
/** D3 — on a money card the converting tap is the primary, full-width control. */
function primaryClassFor(large: boolean): string {
  return large ? "h-14 w-full text-base" : "h-11 w-full text-base";
}
/** D3 — "Not today" is a quiet text control on the same card; still a 44 px tap target. */
const quietTextClass =
  "flex min-h-11 items-center self-start text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";
/** First-run verbs ("Start here") — DESK-LIFE: rows 2-5 as a numbered list; the first
 *  undone verb takes actionCardClass. Card's base is flex-col, so the row says flex-row. */
const verbRowClass =
  "flex min-h-12 cursor-pointer flex-row items-center gap-3 px-4 py-3 text-left text-base font-medium transition-colors active:translate-y-px hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";
/** DESK-LIFE — 150 ms reveal for a block a disclosure just opened (tw-animate-css, index.css:2).
 *  motion-reduce and the index.css reduced-motion rule both still it. */
const revealClass =
  "animate-in fade-in-0 slide-in-from-top-1 duration-150 motion-reduce:animate-none";
/**
 * CAPACITY-FACE (FACE A / PACKET A / EMPTY A) — the rack: one cell per live tray,
 * read from the TrayView[] the probe already loads (list_trays, trayRows): the
 * rows today_view sums into activeTrayCount (trays.rs — planned, sown, blackout,
 * light; quantity per row). Cells sit light first, then blackout. sow_tray lands
 * a tray in blackout and no production writer lands planned or sown (the one
 * 'sown' insert is a cfg(test) helper), so those two states, should a row ever
 * carry one, count with blackout (not yet under the lights) rather than vanish
 * from the rack. Harvest-due is the engine's own harvestSummary.trayCount — no
 * date is compared here, no clock. CEILING — the ceiling is Settings' shelf
 * space (shelf_capacity; the probe reads it through shelfCapacity beside
 * scan_config): N = lightSlots + blackoutSlots only when both are set; one
 * blank, or an unreadable read, and N is unknown — the face says
 * "ceiling not set" and never invents N. Known N: hollow cells fill the rack
 * to N after the live cells (none when live ≥ N — every live cell still paints
 * and the caption still counts them). Unknown N: an empty farm draws
 * EMPTY_RACK_CELLS hollow cells, a live farm draws none. Display only —
 * nothing here feeds the sow door, standing, seed, leftover or money.
 */
const EMPTY_RACK_CELLS = 6;
type RackFace = {
  light: number;
  blackout: number;
  due: number;
  ceiling: number | null;
  hollow: number;
};
function rackFaceFor(
  trays: TrayView[],
  view: TodayView | null,
  shelf: ShelfCapacity | null,
): RackFace {
  let light = 0;
  let blackout = 0;
  for (const t of trays) {
    if (t.state === "light") light += t.quantity;
    else if (t.state === "blackout" || t.state === "sown" || t.state === "planned") {
      blackout += t.quantity;
    }
  }
  const live = light + blackout;
  const ceiling =
    shelf != null && shelf.lightSlots != null && shelf.blackoutSlots != null
      ? shelf.lightSlots + shelf.blackoutSlots
      : null;
  const hollow =
    ceiling == null ? (live === 0 ? EMPTY_RACK_CELLS : 0) : Math.max(0, ceiling - live);
  return { light, blackout, due: view?.harvestSummary?.trayCount ?? 0, ceiling, hollow };
}
/**
 * Receipts sort as one block strictly below every live row (see the sort below), so
 * this value only orders receipts among themselves. Deliberately above every rank in
 * surfaces.ts so a receipt can never tie with a live row's rank. No rank moved.
 */
const RECEIPT_TAIL_RANK = 100;
/**
 * SOP-7 (C-6) — the Health tail. Strictly above RECEIPT_TAIL_RANK so the one
 * Health card sorts after every receipt: the last card on Today. No rank moved.
 */
const HEALTH_TAIL_RANK = RECEIPT_TAIL_RANK + 1;

/**
 * Live rows visible before the fold. Same number as Money's LIVE_VISIBLE (db3f8c4),
 * so both loop surfaces bound live work identically.
 */
const QUEUE_VISIBLE = 5;
type LastAction =
  | { kind: "moved"; trayCount: number }
  | {
      kind: "harvested";
      trayCount: number;
      varietyCount: number;
      cropName: string | null;
      yieldOz: number;
      crops: { cropId: string; cropName: string }[];
    }
  | { kind: "discarded"; trayCount: number; cropName: string }
  | { kind: "restored"; label: string }
  | { kind: "attention_dismissed" };
/**
 * DAY-TAPE (HARVEST-PAPER A) — the harvested receipt's sentence, one builder
 * for the screen card and the printed harvest page, so the paper carries the
 * bytes the screen shows. Two forms, unchanged: the one crop named, or the
 * count of varieties. No clock, no farm name, no estimate: the receipt's own
 * count and weighed ounces.
 */
function harvestReceiptText(
  a: Extract<LastAction, { kind: "harvested" }>,
  system: string,
): string {
  const weight = `${massFigure(a.yieldOz, system)} ${unitWord(system)}`;
  return a.varietyCount === 1 && a.cropName
    ? `Harvested ${trayCountLabel(a.trayCount)} of ${a.cropName} — ${weight}.`
    : `Harvested ${trayCountLabel(a.trayCount)} across ${a.varietyCount} varieties — ${weight}.`;
}
type FarmShape = {
  venues: number;
  stagesBeyondLead: boolean;
  scanConfigured: boolean;
  noRecords: boolean;
};
/** A row decides its content; its grammar is decided by position at render (D2). */
type QueueRow = {
  rank: number;
  key: string;
  /**
   * A settled confirmation, not forced work. Receipts sort as one block strictly
   * below every live row and never receive the large grammar — Today is the forced
   * morning surface and must not be occupied by a receipt.
   */
  receipt?: true;
  render: (large: boolean) => ReactNode;
};
/**
 * Today — the ranked action queue. Only what is forced right now.
 * Physical farm state lives on Farm; relationship items live on Marketing.
 */
export function Today({
  newPaidCount,
  pollTick,
  onOpenMoney,
  onOpenMarketing,
  onOpenSettings,
  onOpenFarm,
  onOpenHealth,
}: {
  newPaidCount: number;
  pollTick: number;
  onOpenMoney: (focus?: MoneyFocus) => void;
  /** FIRST-15 — "Add your first venue" lands in Marketing's new-venue mode. Navigation only. */
  onOpenMarketing: (focus?: "new-venue") => void;
  /** Settings fence 1 (S4): the optional scan-endpoint first-run verb opens Settings. */
  onOpenSettings: () => void;
  /** B2-F1 (B2-D4): the phone-captures pointer taps through to Farm. Navigation only. */
  onOpenFarm: () => void;
  /** SOP-7 (C-6): the Health tail card's Open Health. Navigation only. */
  onOpenHealth: () => void;
}) {
  const [crops, setCrops] = useState<Crop[]>([]);
  // GT-D27 UNITS (J2) - the display system this desk prints mass in. Display
  // only: every write on Today still carries ounces. Imperial until the desk
  // answers, and imperial if it never does (STORE B).
  const [unitSystem, setUnitSystem] = useState<string>(DEFAULT_UNITS);
  // WORLD-PAY PRINT-C J2 (FACE A TS A) - the farm symbol the owed line prints.
  // Read-only here - Settings owns the write. Read before refresh(), so the
  // line never renders a "$" ahead of the pick. Never a boot default.
  const [currency, setCurrency] = useState<FarmCurrencyView | null>(null);
  const [view, setView] = useState<TodayView | null>(null);
  const [attention, setAttention] = useState<AttentionItem[]>([]);
  const [demand, setDemand] = useState<StandingDemandView | null>(null);
  const [cover, setCover] = useState<CoverDate[]>([]);
  const [earlyHarvest, setEarlyHarvest] = useState<
    Record<string, HarvestGroup[]>
  >({});
  const [owed, setOwed] = useState<OwedSummary | null>(null);
  // B1-F1 (D6, D9) — the wholesale rows Money lists, read here read-only so
  // same-rank money cards can sort by the debt's own age and the first-order
  // verb knows whether "first" is still true. null = not readable: the sort
  // falls back to attention order and the verb stays quiet. Never written here.
  const [wholesale, setWholesale] = useState<WholesaleOrderView[] | null>(null);
  const [folds, setFolds] = useState<DockFoldsView | null>(null);
  // TODAY-JAR (MOVE 2) — the jar, per crop, as seedOnHand() answers it. Null
  // until the first answer; no receipts, no rows, no line.
  const [jar, setJar] = useState<SeedOnHandRow[] | null>(null);
  // SOP-3 — one pack per venue for today's harvest date (C-2), read here
  // read-only. [] until the read answers; an unreadable list shows no card.
  const [packs, setPacks] = useState<VenuePackView[]>([]);
  // CUT-DATE (DATE A) — the chosen harvest date. null = nothing chosen: the
  // engine packs today (COMMAND B, a null harvestDate). The ref mirrors the
  // state so refresh() — called from every door on Today — packs the date the
  // chips show, not the date of the render that started the call.
  const [chosenDate, setChosenDate] = useState<string | null>(null);
  const chosenDateRef = useRef<string | null>(null);
  // CUT-DATE (STANDING A) — the stage rows the probe already loads, kept for
  // the cut page's standing block. [] until the probe answers.
  const [stageRows, setStageRows] = useState<StageView[]>([]);
  // YIELD-MEMORY (A) — the tray rows the same probe already loads, kept for
  // the harvest receipt's yield line. [] until the probe answers.
  const [trayRows, setTrayRows] = useState<TrayView[]>([]);
  // SOP-5 — the phone captures Farm lists, read here through the same reader
  // (phone_captures). [] until the read answers; an unreadable list shows no
  // card. captureBusy holds both doors shut while one decision is in flight.
  const [captures, setCaptures] = useState<PhoneCaptureView[]>([]);
  const [captureBusy, setCaptureBusy] = useState(false);
  // SOP-6 (amended C-5 + C-5+) — the leftover prompt on the harvest receipt.
  // Ounces are typed per crop; List leftover is the existing GT-D24 door
  // (recordLeftoverListing -> record_leftover_listing -> leftover::list_leftover)
  // and every refusal sentence is the door's, verbatim. Gone is receipt-local:
  // component state only — no event, no command, harvested stays harvested.
  // All of it resets with lastAction, so the next receipt asks again.
  const [leftoverOz, setLeftoverOz] = useState<Record<string, string>>({});
  const [leftoverListed, setLeftoverListed] = useState<Record<string, number>>({});
  const [leftoverGone, setLeftoverGone] = useState<Record<string, true>>({});
  const [leftoverBusy, setLeftoverBusy] = useState(false);
  const [shape, setShape] = useState<FarmShape | null>(null);
  const [loading, setLoading] = useState(true);
  const [lastError, setLastError] = useState<string | null>(null);
  const [lastAction, setLastAction] = useState<LastAction | null>(null);
  const [moving, setMoving] = useState(false);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [harvestOpen, setHarvestOpen] = useState(false);
  const [harvestGroupsForPad, setHarvestGroupsForPad] = useState<
    HarvestGroup[] | null
  >(null);
  const [backupOpen, setBackupOpen] = useState(false);
  const [startedFresh, setStartedFresh] = useState(false);
  const [upcomingOpen, setUpcomingOpen] = useState(false);
  // The ranked queue is bounded: the rest of the live work sits behind this control,
  // collapsed on every load. Receipts are never behind it — there is at most one.
  const [queueOpen, setQueueOpen] = useState(false);
  // CAPACITY-FACE (PACKET A) — the packet is open or closed; nothing persists.
  const [rackOpen, setRackOpen] = useState(false);
  // CEILING — Settings' shelf space as the probe last read it; null until it answers.
  const [shelf, setShelf] = useState<ShelfCapacity | null>(null);
  const [coverage, setCoverage] = useState<Record<string, HarvestCommitmentsView>>({});
  async function probeFarmShape(v: TodayView) {
    try {
      const [trays, venues, stages] = await Promise.all([
        listTrays(),
        listVenues(),
        listStages(),
      ]);
      setStageRows(stages);
      setTrayRows(trays);
      let scanConfigured = false;
      try {
        scanConfigured = (await scanConfig()).endpointUrl != null;
      } catch {
        scanConfigured = false;
      }
      // CEILING — same grammar as scan_config: an unreadable ceiling is unknown, never a probe failure.
      try {
        setShelf(await shelfCapacity());
      } catch {
        setShelf(null);
      }
      setShape({
        venues: venues.length,
        stagesBeyondLead: stages.some((s: StageView) => s.stage !== "lead"),
        scanConfigured,
        noRecords:
          v.activeTrayCount === 0 && trays.length === 0 && venues.length === 0,
      });
    } catch {
      setShape(null); // probe failed — claim nothing, show nothing extra
      setStageRows([]);
      setTrayRows([]);
      setShelf(null);
    }
  }
  async function refresh() {
    const v = await todayView();
    setView(v);
    void probeFarmShape(v);
    setAttention(await checkAttention());
    try {
      setDemand(await standingDemand());
    } catch {
      setDemand(null);
    }
    try {
      const plan = await coverPlan();
      setCover(plan);
      const pairs = await Promise.all(
        plan.map(async (c) => {
          const groups = await earlyHarvestGroups(c.harvestDate, c.cropId);
          return [`${c.harvestDate}|${c.cropId}`, groups] as const;
        }),
      );
      const next: Record<string, HarvestGroup[]> = {};
      for (const [k, groups] of pairs) {
        if (groups.length > 0) next[k] = groups;
      }
      setEarlyHarvest(next);
    } catch {
      setCover([]);
      setEarlyHarvest({});
    }
    try {
      setOwed(await owedSummary());
    } catch {
      setOwed(null);
    }
    try {
      setWholesale(await listWholesaleOrders());
    } catch {
      setWholesale(null);
    }
    try {
      setFolds(await dockFolds());
    } catch {
      // folds stays null until the first answer — never a guessed default
    }
    try {
      setJar(await seedOnHand());
    } catch {
      // jar stays null until the first answer — never a guessed default
    }
    await loadPacks(chosenDateRef.current);
    try {
      setCaptures(await phoneCaptures());
    } catch {
      setCaptures([]);
    }
  }
  // CUT-DATE (COMMAND B) — the one reader of pack_by_customer. null = today
  // by the engine's pin; a chip passes its date and the packs echo it. An
  // unreadable list shows no packs for that date; the chip row stays.
  async function loadPacks(harvestDate: string | null) {
    try {
      setPacks(await packByCustomer(harvestDate));
    } catch {
      setPacks([]);
    }
  }
  // CUT-DATE (DATE A) — a chip tap: remember the date, then read its packs.
  function handleChooseDate(harvestDate: string) {
    chosenDateRef.current = harvestDate;
    setChosenDate(harvestDate);
    void loadPacks(harvestDate);
  }
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const c = await listCrops();
        if (!cancelled) setCrops(c);
        try {
          const u = await farmUnits();
          if (!cancelled) setUnitSystem(u.system);
        } catch (e) {
          console.error(e);
        }
        try {
          const cur = await farmCurrency();
          if (!cancelled) setCurrency(cur);
        } catch (e) {
          console.error(e);
        }
        await refresh();
      } catch (err) {
        console.error(err);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);
  useEffect(() => {
    if (pollTick === 0) return;
    void refresh().catch(console.error);
  }, [pollTick]);
  useEffect(() => {
    setLeftoverOz({});
    setLeftoverListed({});
    setLeftoverGone({});
    if (lastAction?.kind !== "harvested") {
      setCoverage({});
      return;
    }
    let cancelled = false;
    const crops = lastAction.crops;
    Promise.all(
      crops.map((c) =>
        harvestCommitments(c.cropId)
          .then((v) => [c.cropId, v] as const)
          .catch(() => null),
      ),
    ).then((results) => {
      if (cancelled) return;
      const next: Record<string, HarvestCommitmentsView> = {};
      for (const r of results) {
        if (r) next[r[0]] = r[1];
      }
      setCoverage(next);
    });
    return () => {
      cancelled = true;
    };
  }, [lastAction]);
  async function handleAttentionDismiss(id: string) {
    try {
      setLastError(null);
      await dismissAttention(id);
      setLastAction({ kind: "attention_dismissed" });
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      console.error(err);
    }
  }
  async function handleAttentionAction(item: AttentionItem, action: string) {
    if (action === "dismiss") {
      await handleAttentionDismiss(item.id);
      return;
    }
    try {
      setLastError(null);
      const result = await resolveAttention(item.id, action);
      if (action === "try_now") {
        await refresh();
        return;
      }
      if (action === "open_in_stripe" && result.openUrl) {
        window.open(result.openUrl, "_blank", "noopener,noreferrer");
        await refresh();
        return;
      }
      if (action === "move_now" && result.trayIds.length > 0) {
        const trayCount =
          view?.moveToLight &&
          result.trayIds.every((id) => view.moveToLight!.trayIds.includes(id))
            ? view.moveToLight.trayCount
            : result.trayIds.length;
        await advanceTrays(result.trayIds);
        setLastAction({ kind: "moved", trayCount });
        await refresh();
        return;
      }
      if (action === "harvest_now") {
        const v = await todayView();
        setView(v);
        setAttention(await checkAttention());
        const idSet = new Set(result.trayIds);
        const groups = v.harvests.filter((g) =>
          g.trayIds.some((id) => idSet.has(id)),
        );
        setHarvestGroupsForPad(groups.length > 0 ? groups : v.harvests);
        setHarvestOpen(true);
        return;
      }
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      console.error(err);
      try {
        await refresh();
      } catch (e) {
        setLastError(e instanceof Error ? e.message : String(e));
      }
    }
  }
  async function handleSow(crop: Crop, quantity: number, seedOz: number | null) {
    try {
      setLastError(null);
      await sowTray(crop.id, quantity, seedOz);
      await refresh();
      try {
        const [d, plan] = await Promise.all([standingDemand(), coverPlan()]);
        setCover(plan);
        // An unreachable gap must never hold the sow door open — sowing cannot
        // close it, and pretending otherwise is the lie B3 kills.
        if (!sowableGapsRemain(d, plan)) setSheetOpen(false);
      } catch {
        setSheetOpen(false);
      }
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      console.error(err);
      try {
        await refresh();
      } catch (e) {
        setLastError(e instanceof Error ? e.message : String(e));
      }
    }
  }
  async function handleMoveToLight(trayIds: string[], trayCount: number) {
    if (moving) return;
    setMoving(true);
    try {
      setLastError(null);
      await advanceTrays(trayIds);
      setLastAction({ kind: "moved", trayCount });
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      try {
        await refresh();
      } catch (e) {
        setLastError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      setMoving(false);
    }
  }
  async function handleHarvestDone(
    groups: HarvestInput[],
    meta: {
      trayCount: number;
      varietyCount: number;
      cropName: string | null;
      crops: { cropId: string; cropName: string }[];
    },
  ) {
    try {
      setLastError(null);
      await harvestGroups(groups);
      const yieldOz = groups.reduce((s, g) => s + g.actualYieldOz, 0);
      setLastAction({
        kind: "harvested",
        trayCount: meta.trayCount,
        varietyCount: meta.varietyCount,
        cropName: meta.cropName,
        yieldOz,
        crops: meta.crops,
      });
      setHarvestOpen(false);
      setHarvestGroupsForPad(null);
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      try {
        await refresh();
      } catch (e) {
        setLastError(e instanceof Error ? e.message : String(e));
      }
    }
  }
  async function onToggleCoverage(cropId: string, line: CommitmentLine) {
    const view = coverage[cropId];
    if (!view) return;
    const next: CoverageRef[] = view.lines
      .filter((l) => (l.kind === line.kind && l.id === line.id ? !l.covered : l.covered))
      .map((l) => ({ kind: l.kind, id: l.id }));
    try {
      const updated = await recordHarvestCoverage(cropId, next);
      setCoverage((prev) => ({ ...prev, [cropId]: updated }));
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
    }
  }
  function handleDiscarded(info: { trayCount: number; cropName: string }) {
    setLastAction({
      kind: "discarded",
      trayCount: info.trayCount,
      cropName: info.cropName,
    });
  }
  async function handleHarvestOpenChange(open: boolean) {
    setHarvestOpen(open);
    if (!open) {
      setHarvestGroupsForPad(null);
      try {
        await refresh();
      } catch (err) {
        console.error(err);
      }
    }
  }
  async function handleUndo() {
    try {
      setLastError(null);
      const undone = await undoLast();
      if (undone == null) {
        setLastError("Nothing to undo.");
        await refresh();
        return;
      }
      setLastAction(null);
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      try {
        await refresh();
      } catch (e) {
        setLastError(e instanceof Error ? e.message : String(e));
      }
    }
  }
  // SOP-5 (C-4) — Accept and Discard for a phone capture, on Today. Accept is
  // ONE row through the same Confirm door Farm's Confirm uses
  // (confirmPhoneCaptures -> confirm_phone_captures, phone::gate on the PC);
  // Discard is the same discardPhoneCapture -> discard_phone_capture. The
  // captured count and weight go through unedited — Edit stays on Farm.
  // GT-D20 holds: Confirm is the only door and it runs on the PC; no phone
  // write, no new command, no new Kind. A blocked row is not a thrown error:
  // it stays pending and its sentence (the PC's) lands on the same line every
  // other Today action uses. After either door, refresh() clears the card.
  async function handleCaptureAccept(v: PhoneCaptureView) {
    if (captureBusy) return;
    setCaptureBusy(true);
    try {
      setLastError(null);
      const res = await confirmPhoneCaptures([
        {
          proposalId: v.proposalId,
          quantity: v.quantity,
          actualYieldOz: v.verb === "harvest" ? v.actualYieldOz : null,
        },
      ]);
      const blocked = res.blocked.find((b) => b.proposalId === v.proposalId);
      if (blocked) setLastError(blocked.sentence);
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      try {
        await refresh();
      } catch (e) {
        setLastError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      setCaptureBusy(false);
    }
  }
  async function handleCaptureDiscard(proposalId: string) {
    if (captureBusy) return;
    setCaptureBusy(true);
    try {
      setLastError(null);
      await discardPhoneCapture(proposalId);
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      try {
        await refresh();
      } catch (e) {
        setLastError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      setCaptureBusy(false);
    }
  }
  // SOP-6 — List leftover goes through the one GT-D24 door, then refresh().
  // A blank or non-numeric field reaches the door as 0 exactly as Money does
  // (Money.tsx onListLeftover) and the door refuses it with its own sentence;
  // nothing here pre-empts or rewrites those sentences. Gone writes nothing.
  async function handleListLeftover(cropId: string, harvestedOn: string) {
    if (leftoverBusy) return;
    setLeftoverBusy(true);
    try {
      setLastError(null);
      const oz = typedOunces(leftoverOz[cropId] ?? "", unitSystem);
      const listed = await recordLeftoverListing({
        cropId,
        harvestedOn,
        listedOz: Number.isFinite(oz) ? oz : 0,
      });
      setLeftoverListed((prev) => ({ ...prev, [cropId]: listed.listedOz }));
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      try {
        await refresh();
      } catch (e) {
        setLastError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      setLeftoverBusy(false);
    }
  }
  function handleLeftoverGone(cropId: string) {
    setLeftoverGone((prev) => ({ ...prev, [cropId]: true }));
  }
  // DESK-LABEL-1 (2026-08-25). The desk's action vocabulary is a closed set.
  // ATTENTION_ACTIONS is the one list and AttentionAction is derived from it,
  // so there is nothing to keep in sync by hand. The switch is exhaustive over
  // that union: a new member without a case arm fails tsc here rather than
  // reaching the operator as a raw key.
  //
  // AttentionItem.actions stays string[] on purpose. A row raised by an older
  // build can still carry a key this build does not know - attention.actions
  // is TEXT with no CHECK and has never been migrated - so typing the wire as
  // AttentionAction would be a claim the table does not support.
  // isKnownAction is where that reality is handled.
  const ATTENTION_ACTIONS = [
    "try_now",
    "harvest_now",
    "move_now",
    "open_in_stripe",
    "dismiss",
  ] as const;
  type AttentionAction = (typeof ATTENTION_ACTIONS)[number];
  function isKnownAction(action: string): action is AttentionAction {
    return (ATTENTION_ACTIONS as readonly string[]).includes(action);
  }
  function attentionActionLabel(action: AttentionAction): string {
    switch (action) {
      case "try_now":
        return "Try now";
      case "harvest_now":
        return "Harvest";
      case "move_now":
        return "Move to light";
      case "open_in_stripe":
        return "Open in Stripe";
      case "dismiss":
        return "Dismiss";
      default: {
        const unreachable: never = action;
        return unreachable;
      }
    }
  }
  function coverFor(item: AttentionItem): CoverDate | null {
    if (
      item.kind !== "money.capacity_short" &&
      item.kind !== "wholesale.overcommitted"
    ) {
      return null;
    }
    const id = item.entityId ?? "";
    const pipe = id.indexOf("|");
    if (pipe < 0) {
      return cover.find((c) => c.harvestDate === id) ?? null;
    }
    const date = id.slice(0, pipe);
    const crop = id.slice(pipe + 1);
    return (
      cover.find((c) => c.harvestDate === date && c.cropId === crop) ?? null
    );
  }
  function attentionCard(item: AttentionItem, large: boolean): ReactNode {
    const c = coverFor(item);
    // RB2 — COLLECT and DELIVER carried only "Not today". The card that
    // names the money now carries the tap that converts it. Today does not
    // write money: this deep-links to the Money row that owns the write.
    const orderId = item.entityId;
    const intent = orderId == null ? null : moneyIntentForKind(item.kind);
    const money = isMoneyDebtKind(item.kind);
    // DESK-LABEL-1 - an action this build has no label for is not rendered.
    // The card keeps its message and its own Dismiss below, so nothing is
    // hidden from the operator; what is removed is a control that would have
    // closed the ask without moving the farm.
    const extra = item.actions.filter(
      (a): a is AttentionAction => a !== "dismiss" && isKnownAction(a),
    );
    // B1-F1 (D3) — on a money card the converting tap (Collect / Deliver /
    // Sow / Open the order) is the primary, full-width control and "Not today"
    // is a quiet text control on the same card. Same dismissAttention call,
    // same one-day episode (attention.rs raise_or_refresh). Every listed
    // action still renders; nothing is dropped.
    return (
      <Card key={item.id} className={cardClassFor(large)}>
        <p className={messageClassFor(large)}>{item.message}</p>
        {c != null && c.orders.length > 0 && (
          <p className="text-sm text-muted-foreground">
            {c.orders
              .map(
                (o) =>
                  `${o.venueName} · ${o.trays} ${o.trays === 1 ? "tray" : "trays"} · ${o.state}`,
              )
              .join(" · ")}
          </p>
        )}
        {intent != null && orderId != null && (
          <Button
            type="button"
            className={primaryClassFor(large)}
            onClick={() => onOpenMoney({ orderId, intent })}
          >
            {intent === "collect" ? "Collect" : "Deliver"}
          </Button>
        )}
        {c != null && (
          <div className="flex flex-wrap items-center gap-3">
            {c.reachability.sowCanServe ? (
              <Button
                type="button"
                className={primaryClassFor(large)}
                onClick={() => setSheetOpen(true)}
              >
                Sow
              </Button>
            ) : (
              <Button
                type="button"
                className={primaryClassFor(large)}
                onClick={() => onOpenMoney()}
              >
                Open the order
              </Button>
            )}
            {earlyHarvest[`${c.harvestDate}|${c.cropId}`] != null && (
              <Button
                type="button"
                className={primaryClassFor(large)}
                onClick={() => {
                  const groups =
                    earlyHarvest[`${c.harvestDate}|${c.cropId}`];
                  if (groups == null || groups.length === 0) return;
                  setHarvestGroupsForPad(groups);
                  setHarvestOpen(true);
                }}
              >
                Harvest early
              </Button>
            )}
          </div>
        )}
        {(extra.length > 0 || !money) && (
          <div className="flex flex-wrap gap-2">
            {extra.map((action) => (
              <Button
                key={action}
                type="button"
                className="h-11 px-4 text-base"
                onClick={() => void handleAttentionAction(item, action)}
              >
                {attentionActionLabel(action)}
              </Button>
            ))}
            {!money && (
              <Button
                type="button"
                variant="ghost"
                className="h-11 px-4 text-base"
                onClick={() => void handleAttentionDismiss(item.id)}
              >
                Dismiss
              </Button>
            )}
          </div>
        )}
        {money && (
          <button
            type="button"
            className={quietTextClass}
            onClick={() => void handleAttentionDismiss(item.id)}
          >
            Not today
          </button>
        )}
      </Card>
    );
  }
  const byId = new Map(attention.map((a) => [a.id, a] as const));
  const todayAttention = folds
    ? folds.todayAttentionOrder
        .map((id) => byId.get(id))
        .filter((a): a is AttentionItem => a != null)
    : [];
  const upcomingAttention = folds
    ? attention.filter((a) => surfaceForKind(folds, a.kind) === "upcoming")
    : [];
  const unallocated = demand?.unallocatedVenues ?? [];
  const upcomingCount = upcomingAttention.length + (unallocated.length > 0 ? 1 : 0);
  // CUT-1 — derived from the packs already in state: empty packs, empty cut.
  const cutLines = cutLinesFor(packs, crops);
  const cutDate = packs.length > 0 ? packs[0].harvestDate : null;
  // CUT-DATE — the chips, the underlined one and the standing block, all
  // derived from state already loaded: no clock, no second read.
  const orderedDates = orderedDatesFor(wholesale);
  const selectedDate = packs.length > 0 ? packs[0].harvestDate : chosenDate;
  const standingLines = standingLinesFor(stageRows, crops);
  // CAPACITY-FACE — the rack's face, derived from rows already in state: no second read.
  const rack = rackFaceFor(trayRows, view, shelf);
  const rackLive = rack.light + rack.blackout;
  // B1-F1 (D2) — every row is pushed with a render function; the grammar is
  // chosen at render by position (first row large, all others card).
  const rows: QueueRow[] = [];
  if (folds) {
  for (const item of todayAttention) {
    rows.push({
      rank: folds.ranks.map[item.kind] ?? folds.ranks.unclassified,
      key: item.id,
      render: (large) => attentionCard(item, large),
    });
  }
  // SOP-3 — pack sits with beat 5 (C-1): the money.delivery_due rank, read
  // from the folds map; no dock_folds key added. Pushed after the attention
  // loop so the Deliver card keeps the rank-5 tie (the sort is stable). One
  // card lists every venue pack; its one button prints the sheet below.
  // Read-only — no Delivered here, that write stays on Money.
  // CUT-1 — the same card now opens with the cut lines (crop -> trays -> which
  // venue packs), above the venue lines; the same button prints the cut page
  // first and the venue pages after it (the .pack-print section below).
  // CUT-DATE (DATE A / EMPTY A / SURFACE A) — the card mounts while any
  // ordered wholesale row exists on any date, opening with one quiet row of
  // date chips; the crop lines, venue lines and button render only when the
  // chosen date has packs. The underlined chip is the packs' own date, or the
  // chosen date while it has no packs. No picker, no typed date, no clock.
  if (orderedDates.length > 0 || packs.length > 0) {
    const p = packs;
    rows.push({
      rank: folds.ranks.map["money.delivery_due"] ?? folds.ranks.unclassified,
      key: "pack-sheet",
      render: (large) => (
        <Card className={cardClassFor(large)}>
          {orderedDates.length > 0 && (
            <div className="flex flex-wrap gap-x-4">
              {orderedDates.map((d) => (
                <button
                  key={d}
                  type="button"
                  aria-pressed={d === selectedDate}
                  className={
                    d === selectedDate ? `${quietTextClass} underline` : quietTextClass
                  }
                  onClick={() => handleChooseDate(d)}
                >
                  {monthDayLabel(parseLocalDate(d))}
                </button>
              ))}
            </div>
          )}
          {p.length > 0 && (
            <>
              {cutLines.map((cut) => (
                <p key={`cut-${cut.cropId}`} className={messageClassFor(large)}>
                  {cutLineText(cut)}
                </p>
              ))}
              {p.map((pack) => (
                <p key={pack.venueName} className={messageClassFor(large)}>
                  Pack for {pack.venueName} — {trayCountLabel(pack.trayTotal)}
                </p>
              ))}
              <Button
                type="button"
                className={primaryClassFor(large)}
                onClick={() => window.print()}
              >
                Print cut and pack sheets
              </Button>
            </>
          )}
        </Card>
      ),
    });
  }
  if (demand != null && demand.shortfall > 0) {
    const d = demand;
    rows.push({
      rank: folds.ranks.standingShortfall,
      key: "standing-shortfall",
      render: (large) => (
        <Card
          role="button"
          tabIndex={0}
          onClick={() => setSheetOpen(true)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              setSheetOpen(true);
            }
          }}
          className={tapCardClassFor(large)}
        >
          <span className="text-amber-700">
            Standing orders short:{" "}
            {d.varieties
              .filter((v) => v.shortfall > 0)
              .map((v) => `${v.shortfall} ${v.name}`)
              .join(" · ")}{" "}
            (last 7 days)
          </span>
        </Card>
      ),
    });
  }
  // SOP-4 — this week's standing lines, read-only. "This week" is the
  // trailing 7-day window standing_demand already uses (marketing.rs:2049,
  // today-6): the view carries no dates and none are computed here (C-3).
  // Sits with beat 4 (C-1): the standingShortfall rank, read from the folds
  // map; no dock_folds key added. Pushed after the short-to-sow card so that
  // card keeps the rank tie (the sort is stable). Confirm is a tab switch
  // only — the standing card and its decide door stay on Marketing (GT-D17).
  // Nothing is written here.
  const standingWeek = demand?.varieties.filter((v) => v.traysWeek > 0) ?? [];
  if (standingWeek.length > 0) {
    rows.push({
      rank: folds.ranks.standingShortfall,
      key: "standing-week",
      render: (large) => (
        <Card className={cardClassFor(large)}>
          <p className={messageClassFor(large)}>
            Standing orders this week (last 7 days)
          </p>
          {standingWeek.map((v) => {
            // YIELD-STANDING (A) — one muted line under the sentence, only while
            // a weighed harvest day exists for the variety's crop. Read-only.
            const crop = crops.find((c) => c.name === v.name);
            const weighed = crop == null ? null : lastWeighedFor(trayRows, crop.id, unitSystem);
            // TODAY-JAR (MOVE 2) — the jar line, only while the crop has a jar
            // row (a receipt). The row is found by crop id, never by name.
            const jarRow =
              crop == null || jar == null
                ? null
                : (jar.find((r) => r.cropId === crop.id) ?? null);
            return (
              <div key={v.name} className="flex flex-col gap-1">
                <p className="text-sm text-muted-foreground">
                  {`${v.name} · ${v.traysWeek}/week · sown ${v.sownLast7Days} · short ${v.shortfall}`}
                </p>
                {weighed != null && (
                  <p className="text-sm text-muted-foreground">{weighed}</p>
                )}
                {jarRow != null && (
                  <p className="text-sm text-muted-foreground">
                    {seedLineFor(v, jarRow, unitSystem)}
                  </p>
                )}
              </div>
            );
          })}
          <Button
            type="button"
            className={primaryClassFor(large)}
            onClick={() => onOpenMarketing()}
          >
            Confirm
          </Button>
        </Card>
      ),
    });
  }
  // SOP-5 — the pending phone captures, one card. Sits with beat 5 (C-1,
  // cover / harvest): the tray.overdue_harvest rank, read from the folds map;
  // no dock_folds key added. Pushed after the attention loop so an overdue-
  // harvest card keeps the rank tie (the sort is stable). phone.proposal stays
  // a Farm attention kind (REALITY_KINDS): Today reads the list Farm lists,
  // never that kind, so the loop above cannot render a capture twice. Each
  // capture carries its own Accept and Discard. Empty list, no card.
  if (captures.length > 0) {
    const cs = captures;
    rows.push({
      rank: folds.ranks.map["tray.overdue_harvest"] ?? folds.ranks.unclassified,
      key: "phone-captures",
      render: (large) => (
        <Card className={cardClassFor(large)}>
          {cs.map((v) => (
            <div key={v.proposalId} className="flex flex-col gap-3">
              <p className={messageClassFor(large)}>{v.message}</p>
              <div className="flex flex-wrap gap-2">
                <Button
                  type="button"
                  className="h-11 px-4 text-base"
                  disabled={captureBusy}
                  onClick={() => void handleCaptureAccept(v)}
                >
                  Accept
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  className="h-11 px-4 text-base"
                  disabled={captureBusy}
                  onClick={() => void handleCaptureDiscard(v.proposalId)}
                >
                  Discard
                </Button>
              </div>
            </div>
          ))}
        </Card>
      ),
    });
  }
  if (lastAction?.kind === "moved") {
    const a = lastAction;
    rows.push({
      rank: folds.ranks.move,
      key: "move-confirm",
      receipt: true,
      render: (large) => (
        <Card className={confirmCardClassFor(large)}>
          <span>Moved {trayCountLabel(a.trayCount)} to light.</span>
          <Button
            type="button"
            variant="ghost"
            onClick={handleUndo}
            className="h-14 px-3 text-base"
          >
            Undo
          </Button>
        </Card>
      ),
    });
  }
  if (view?.moveToLight) {
    const move = view.moveToLight;
    rows.push({
      rank: folds.ranks.move,
      key: "move-due",
      render: (large) => (
        <Card
          role="button"
          tabIndex={0}
          aria-disabled={moving}
          onClick={
            moving
              ? undefined
              : () => handleMoveToLight(move.trayIds, move.trayCount)
          }
          onKeyDown={
            moving
              ? undefined
              : (e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    handleMoveToLight(move.trayIds, move.trayCount);
                  }
                }
          }
          className={tapCardClassFor(large)}
        >
          Move to light — {trayCountLabel(move.trayCount)}
        </Card>
      ),
    });
  }
  if (lastAction?.kind === "harvested") {
    const a = lastAction;
    rows.push({
      rank: folds.ranks.harvest,
      key: "harvest-confirm",
      receipt: true,
      render: (large) => (
        <Card className={harvestConfirmClassFor(large)}>
          <div className="flex items-center justify-between gap-4">
            <span>{harvestReceiptText(a, unitSystem)}</span>
            <Button
              type="button"
              variant="ghost"
              onClick={handleUndo}
              className="h-14 px-3 text-base"
            >
              Undo
            </Button>
          </div>
          {a.crops.map((c) => {
            const v = coverage[c.cropId];
            if (!v) return null;
            const listedOz = leftoverListed[c.cropId] as number | undefined;
            // YIELD-MEMORY (A / CAP A) — display only, above the leftover row;
            // the leftover input below stays empty and its door unchanged.
            const yieldLine = yieldMemoryFor(trayRows, c.cropId, c.cropName, v.harvestedOn, unitSystem);
            return (
              <div key={c.cropId} className="flex flex-col gap-1">
                <p className="text-sm font-medium">{v.header}</p>
                {v.lines.length === 0 ? (
                  <p className="text-sm text-muted-foreground">{v.emptyLine}</p>
                ) : (
                  v.lines.map((l) => (
                    <div key={`${l.kind}-${l.id}`} className="flex items-center justify-between gap-2">
                      <span className="text-sm">{l.covered ? `Covering — ${l.text}` : l.text}</span>
                      <Button
                        type="button"
                        variant="outline"
                        className="h-9 px-3 text-sm"
                        onClick={() => void onToggleCoverage(c.cropId, l)}
                      >
                        {l.covered ? "Undo mark" : "Covered by this harvest"}
                      </Button>
                    </div>
                  ))
                )}
                {yieldLine != null && (
                  <p className="text-sm text-muted-foreground">{yieldLine}</p>
                )}
                {listedOz != null ? (
                  <p className="text-sm text-muted-foreground">
                    Leftover listed — {massFigure(listedOz, unitSystem)} {unitWord(unitSystem)}.
                  </p>
                ) : leftoverGone[c.cropId] === true ? null : (
                  <div className="flex items-end gap-2">
                    <label className="flex flex-col gap-1 text-sm">
                      Leftover ({unitWord(unitSystem)})
                      <input
                        className="h-12 w-28 rounded-md border border-input bg-card px-3"
                        inputMode="decimal"
                        value={leftoverOz[c.cropId] ?? ""}
                        onChange={(e) => {
                          const next = e.target.value;
                          setLeftoverOz((prev) => ({ ...prev, [c.cropId]: next }));
                        }}
                      />
                    </label>
                    <Button
                      type="button"
                      variant="outline"
                      className="h-12 px-4 text-sm"
                      disabled={leftoverBusy}
                      onClick={() => void handleListLeftover(c.cropId, v.harvestedOn)}
                    >
                      {leftoverBusy ? "Saving…" : "List leftover"}
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      className="h-12 px-3 text-sm"
                      disabled={leftoverBusy}
                      onClick={() => handleLeftoverGone(c.cropId)}
                    >
                      Gone
                    </Button>
                  </div>
                )}
              </div>
            );
          })}
        </Card>
      ),
    });
  } else if (lastAction?.kind === "discarded") {
    const a = lastAction;
    rows.push({
      rank: folds.ranks.harvest,
      key: "harvest-confirm",
      receipt: true,
      render: (large) => (
        <Card className={confirmCardClassFor(large)}>
          <span>
            Discarded {trayCountLabel(a.trayCount)} of {a.cropName}.
          </span>
          <Button
            type="button"
            variant="ghost"
            onClick={handleUndo}
            className="h-14 px-3 text-base"
          >
            Undo
          </Button>
        </Card>
      ),
    });
  }
  if (lastAction?.kind === "attention_dismissed") {
    rows.push({
      rank: RECEIPT_TAIL_RANK,
      key: "attention-dismissed",
      receipt: true,
      render: (large) => (
        <Card className={confirmCardClassFor(large)}>
          <span>Dismissed.</span>
          <Button
            type="button"
            variant="ghost"
            onClick={handleUndo}
            className="h-14 px-3 text-base"
          >
            Undo
          </Button>
        </Card>
      ),
    });
  } else if (lastAction?.kind === "restored") {
    const a = lastAction;
    rows.push({
      rank: RECEIPT_TAIL_RANK,
      key: "farm-restored",
      receipt: true,
      render: (large) => (
        <Card className={confirmCardClassFor(large)}>
          <span>Farm restored from {a.label}.</span>
        </Card>
      ),
    });
  }
  if (view?.harvestSummary) {
    const hs = view.harvestSummary;
    rows.push({
      rank: folds.ranks.harvest,
      key: "harvest-due",
      render: (large) => (
        <Card
          role="button"
          tabIndex={0}
          onClick={() => setHarvestOpen(true)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              setHarvestOpen(true);
            }
          }}
          className={tapCardClassFor(large)}
        >
          {hs.varietyCount === 1 && hs.singleCropName
            ? `Harvest today — ${trayCountLabel(hs.trayCount)} of ${hs.singleCropName}, est. ${massFigure(hs.estimatedYieldOz, unitSystem)} ${unitWord(unitSystem)}`
            : `Harvest today — ${trayCountLabel(hs.trayCount)}, ${hs.varietyCount} varieties, est. ${massFigure(hs.estimatedYieldOz, unitSystem)} ${unitWord(unitSystem)}`}
        </Card>
      ),
    });
  }
  // SOP-7 (C-6) — the Health tail. Unhealthy only: Degraded, Healthy and an
  // overall of null (no check has reported) push nothing. The sentence is the
  // raising check's own, verbatim, off the folds Today already reads
  // (folds.checks, REPORTED_CHECKS order — the same first-Unhealthy pick
  // StatusMark makes), so the header line and this card name one check.
  // receipt: true is the tail-block mechanism, not a claim of a confirmation:
  // below every live row, never the large grammar, never behind the queue
  // fold. Open Health is the existing tab switch (App.tsx onOpenHealth).
  // Nothing is computed or persisted here; Health stays the one dashboard.
  if (folds.overall === "Unhealthy") {
    const raising = folds.checks.find((c) => c.severity === "Unhealthy");
    if (raising != null) {
      rows.push({
        rank: HEALTH_TAIL_RANK,
        key: "health-tail",
        receipt: true,
        render: (large) => (
          <Card className={cardClassFor(large)}>
            <p className={messageClassFor(large)}>
              <span className="text-red-700">{raising.sentence}</span>
            </p>
            <Button
              type="button"
              className={primaryClassFor(large)}
              onClick={onOpenHealth}
            >
              Open Health
            </Button>
          </Card>
        ),
      });
    }
  }
  }
  // B1-F1 (D2) revisited — a receipt is a settled outcome, not forced work. Receipts
  // sort as ONE BLOCK strictly below every live row; rank still orders each block
  // internally, and no rank in surfaces.ts moved. This is what stops a confirmation
  // from tying with the live row it confirms and winning on insertion order.
  rows.sort(
    (a, b) =>
      Number(a.receipt ?? false) - Number(b.receipt ?? false) || a.rank - b.rank,
  );
  const liveRows = rows.filter((r) => !r.receipt);
  const receiptRows = rows.filter((r) => r.receipt);
  const shownLive = liveRows.slice(0, QUEUE_VISIBLE);
  const restLive = liveRows.slice(QUEUE_VISIBLE);
  const venueDone = (shape?.venues ?? 0) > 0;
  const demandDone = shape?.stagesBeyondLead === true;
  const sowDone = (view?.activeTrayCount ?? 0) > 0;
  // The optional scan verb never keeps the block alive on its own.
  const firstRunVisible =
    shape !== null && !(venueDone && demandDone && sowDone);
  const recoveryVisible = shape?.noRecords === true && !startedFresh;
  const phoneCapturesPending = attention.filter((a) => a.kind === "phone.proposal").length;
  // DESK-LIFE — the same five verbs, same conditions, same handlers, same order, same
  // bytes; rendered as one desk: the first undone verb large, the rest numbered rows.
  // The step number is static (venue 1 … scan 5) so a done step leaves its number behind.
  const firstRunVerbs: { n: number; text: string; onTap: () => void }[] = [
    ...(!venueDone
      ? [{ n: 1, text: "Add your first venue", onTap: () => onOpenMarketing("new-venue") }]
      : []),
    ...(!demandDone
      ? [{ n: 2, text: "Record a standing order or drop a sample", onTap: () => onOpenMarketing() }]
      : []),
    ...(!sowDone
      ? [{ n: 3, text: "Sow trays to cover that demand", onTap: () => setSheetOpen(true) }]
      : []),
    ...(wholesale != null && wholesale.length === 0
      ? [{ n: 4, text: "Record your first wholesale order", onTap: () => onOpenMoney() }]
      : []),
    ...(shape?.scanConfigured === false
      ? [{ n: 5, text: "Configure scan endpoint (optional)", onTap: () => onOpenSettings() }]
      : []),
  ];
  return (
    <main className="mx-auto flex min-h-screen w-full max-w-md flex-col gap-8 px-6 py-12">
      <h1 className="text-2xl font-semibold tracking-tight">Today</h1>
      {/* STATES A — the loading face. While the probe has not answered, the
          fact, the rack and the first card stand as blocks of var(--border) in
          the flow they will take (index.css .skeleton-block). No spinner word,
          no motion: the sentences arrive with their own paint. */}
      {loading && (
        <div className="flex flex-col gap-8" aria-hidden="true">
          <div className="skeleton-block h-9 w-2/3" />
          <div className="skeleton-block h-12 w-full" />
          <div className="skeleton-block h-24 w-full" />
        </div>
      )}
      {/* B1-F1 (D4) — the owed line renders when something is owed or when the
          register is unreadable. "Nothing owed to you." no longer renders: on a
          clean morning Today is one sentence and Health M1 carries the
          affirmative. Source unchanged: owedSummary from the register, never
          the attention table, so "Not today" can never quiet this line.
          OWED-LO (audit R-2): priced-unpaid leftover is owed — the gate
          counts it, so leftover-only owed prints here too. */}
      {!loading && (owed == null || owed.deliveries > 0 || owed.leftoverCount > 0) && (
        <p className="text-3xl font-medium tabular-nums">{owedLine(owed, currency?.symbol ?? "$")}</p>
      )}
      <ErrorLine message={lastError} />
      {/* CAPACITY-FACE (WIDTH A) — the rack. It sits here in the 28rem column
          as a flow block at every window size (index.css .rack-rail; the 900 px
          absolute rail hung off this <main> is retired, and the App shell stays
          max-w-md). One control, the tree's own disclosure grammar: tap opens the
          packet (under the lights / in blackout / to harvest today) on this
          screen, a second tap or Close dismisses it. Cells are aria-hidden; the
          caption carries the count and, while N is unknown, the mute ceiling
          line. Rendered only once the probe has answered (shape !== null) — an
          unreadable farm claims no rack. */}
      {!loading && shape !== null && (
        <aside className="rack-rail flex flex-col gap-2" aria-label="Rack">
          <button
            type="button"
            aria-expanded={rackOpen}
            onClick={() => setRackOpen((v) => !v)}
            className="flex min-h-11 flex-col items-start gap-2 text-left focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
          >
            <span className="rack" aria-hidden="true">
              {Array.from({ length: rack.light }, (_, i) => (
                <span key={`light-${i}`} className="rack-cell" data-stage="light" />
              ))}
              {Array.from({ length: rack.blackout }, (_, i) => (
                <span key={`blackout-${i}`} className="rack-cell" data-stage="blackout" />
              ))}
              {Array.from({ length: rack.hollow }, (_, i) => (
                <span key={`hollow-${i}`} className="rack-cell" />
              ))}
            </span>
            <span className="flex items-center gap-2 text-sm text-muted-foreground">
              <span aria-hidden="true">{rackOpen ? "▾" : "▸"}</span>
              {rackLive === 0 ? "Nothing on the rack" : `${trayCountLabel(rackLive)} on the rack`}
            </span>
            {rack.ceiling == null && (
              <span className="text-sm text-muted-foreground">ceiling not set</span>
            )}
          </button>
          {rackOpen && (
            <div className={`flex flex-col gap-1 ${revealClass}`}>
              <p className="text-sm">{trayCountLabel(rack.light)} under the lights</p>
              <p className="text-sm">{trayCountLabel(rack.blackout)} in blackout</p>
              <p className="text-sm">{trayCountLabel(rack.due)} to harvest today</p>
              <button type="button" className={quietTextClass} onClick={() => setRackOpen(false)}>
                Close
              </button>
            </div>
          )}
        </aside>
      )}
      {!loading && folds != null && rows.length > 0 && (
        <div className="flex flex-col gap-6">
          {shownLive.map((r, i) => (
            <div key={r.key}>{r.render(i === 0)}</div>
          ))}
          {restLive.length > 0 && (
            <button
              type="button"
              aria-expanded={queueOpen}
              onClick={() => setQueueOpen((v) => !v)}
              className="flex min-h-11 items-center gap-2 text-left text-base font-medium focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            >
              <span aria-hidden="true">{queueOpen ? "▾" : "▸"}</span>
              {restLive.length} more forced right now
            </button>
          )}
          {queueOpen &&
            restLive.map((r) => (
              <div key={r.key} className={revealClass}>
                {r.render(false)}
              </div>
            ))}
          {receiptRows.map((r) => (
            <div key={r.key}>{r.render(false)}</div>
          ))}
        </div>
      )}
      {/* B1-F1 (D5) — the deferred state is named. If money is owed and the
          queue is empty, every collect card was set aside with "Not today"
          today (raise_or_refresh re-raises anything not dismissed today), so
          the sentence says so instead of claiming nothing. */}
      {!loading && folds != null && liveRows.length === 0 && !firstRunVisible && (
        <p className="text-3xl font-medium tabular-nums">
          {owed != null && owed.deliveries > 0
            ? `Nothing forced right now — ${deliveryWord(owed.deliveries)} set aside for today.`
            : "Nothing forced right now."}
        </p>
      )}
      {/* B1-F1 (D8, D9) — first open: "Start here" above the recovery door.
          The four existing verbs are unchanged; the fifth is the one door into
          the money loop (a wholesale order is what raises DELIVER and COLLECT).
          It says "first" only while that is true — it hides once any wholesale
          order row exists — and, like the scan verb, never keeps the block
          alive on its own. */}
      {!loading && firstRunVisible && (
        <div className={`flex flex-col gap-3 ${revealClass}`}>
          <p className="text-sm text-muted-foreground">Start here</p>
          {firstRunVerbs.map(({ n, text, onTap }, i) => (
            <Card
              key={n}
              role="button"
              tabIndex={0}
              onClick={() => onTap()}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onTap();
                }
              }}
              className={i === 0 ? actionCardClass : verbRowClass}
            >
              {i === 0 ? (
                text
              ) : (
                <>
                  <span className="w-5 shrink-0 text-sm text-muted-foreground">{n}</span>
                  <span>{text}</span>
                </>
              )}
            </Card>
          ))}
        </div>
      )}
      {/* B1-F1 (D8) — the recovery card is now a one-line door with the same
          two actions. The full explanation lives behind Restore a backup
          (FarmBackupSheet, recoveryMode). Start fresh still writes nothing. */}
      {!loading && recoveryVisible && (
        <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
          <span>No tray or venue records on this machine.</span>
          <button
            type="button"
            className="min-h-11 underline underline-offset-4 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            onClick={() => setBackupOpen(true)}
          >
            Restore a backup
          </button>
          <button
            type="button"
            className="min-h-11 underline underline-offset-4 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            onClick={() => setStartedFresh(true)}
          >
            Start fresh on this machine
          </button>
        </p>
      )}
      {/* B1-F1 (D7) — the phone-captures pointer sits below the queue, above
          "Upcoming & later". Wording unchanged. Confirm stays on the PC (GT-D20);
          SOP-5 (C-4) reaches the same door from Today. Farm keeps Edit and Accept all.
          B2-F1 (B2-D4) — the same sentence is now the tap that lands on Farm. */}
      {phoneCapturesPending > 0 && (
        <button
          type="button"
          onClick={onOpenFarm}
          className="flex min-h-11 items-center text-left text-base text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        >
          {phoneCapturesPending === 1
            ? "1 phone capture waiting — confirm it on Farm."
            : `${phoneCapturesPending} phone captures waiting — confirm them on Farm.`}
        </button>
      )}
      {!loading && folds != null && upcomingCount > 0 && (
        <div className="flex flex-col gap-4">
          <button
            type="button"
            aria-expanded={upcomingOpen}
            onClick={() => setUpcomingOpen((v) => !v)}
            className="flex min-h-11 items-center gap-2 text-left text-base font-medium focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
          >
            <span aria-hidden="true">{upcomingOpen ? "▾" : "▸"}</span>
            Upcoming &amp; later ({upcomingCount})
          </button>
          {upcomingOpen && (
            <div className={`flex flex-col gap-4 ${revealClass}`}>
              {upcomingAttention.map((item) => (
                <Card key={item.id} className="flex flex-col gap-4 p-6">
                  <p className="text-base font-medium leading-snug">
                    {item.message}
                  </p>
                  <div className="flex flex-wrap gap-2">
                    {item.actions
                      .filter(
                        (a): a is AttentionAction =>
                          a !== "dismiss" && isKnownAction(a),
                      )
                      .map((action) => (
                        <Button
                          key={action}
                          type="button"
                          className="h-11 px-4 text-base"
                          onClick={() => void handleAttentionAction(item, action)}
                        >
                          {attentionActionLabel(action)}
                        </Button>
                      ))}
                    <Button
                      type="button"
                      variant="ghost"
                      className="h-11 px-4 text-base"
                      onClick={() => void handleAttentionDismiss(item.id)}
                    >
                      Dismiss
                    </Button>
                  </div>
                </Card>
              ))}
              {unallocated.length > 0 && (
                <p className="text-sm text-amber-700">
                  Standing order at {unallocated.join(", ")} needs re-stating by
                  variety (on the Marketing page).
                </p>
              )}
            </div>
          )}
        </div>
      )}
      {newPaidCount > 0 && (
        <p className="text-base text-muted-foreground">
          {newPaidCount === 1
            ? "1 new paid order — capacity already set aside"
            : `${newPaidCount} new paid orders — capacity already set aside`}
        </p>
      )}
      {/* SOP-3 — the pack sheet. display: none on screen; in print it is the
          only visible root (index.css .pack-print, sibling of .invoice-print),
          one page per venue. Heading bytes: Pack for {venue} · {harvestDate}
          CUT-1 — one cut page first, in the same root under the same print
          rules (.pack-page + .pack-page breaks the page, so the venue pages
          follow unchanged). Heading bytes: Cut for {harvestDate} — the packs'
          own date, never a clock read here.
          CUT-DATE (STANDING A / DOUBLE-COUNT A) — under the crop lines on that
          first page, the standing block: heading bytes Standing — per week,
          the weekly rate per venue (x crop when split), printed beside the cut
          and never summed into it. Print-only: the card never shows it.
          DAY-TAPE (HARVEST-PAPER A) — last, one harvest page, only while the
          harvested receipt is on screen: the receipt's own sentence
          (harvestReceiptText) under the date the cut page carries. No
          receipt, no page. Nothing else rides along — none of the receipt's
          controls or rows, no yield line, no standing, no farm name.
          ROUTE (SURFACE A / HEADING B / CONTENTS B / PAGE A) — between the
          venue pages and the harvest page, one run page: heading bytes
          Delivery run · {harvestDate} (the cut page's own date), then one
          line per pack — {venue} · {n trays}, the pack card's own label —
          and under it the venue row's address · phone when present
          (runContactLine). No crop lines, no owed, no Delivered, no
          standing, no farm name, no day of the week. No packs, no page. */}
      {packs.length > 0 && (
        <section className="pack-print">
          <article className="pack-page flex flex-col gap-2">
            <h2 className="text-lg font-medium">Cut for {cutDate}</h2>
            {cutLines.map((cut) => (
              <p key={cut.cropId} className="text-sm">
                {cutLineText(cut)}
              </p>
            ))}
            {standingLines.length > 0 && (
              <>
                <h3 className="mt-2 text-base font-medium">Standing — per week</h3>
                {standingLines.map((s) => (
                  <p key={s.key} className="text-sm">
                    {s.text}
                  </p>
                ))}
              </>
            )}
          </article>
          {packs.map((pack) => (
            <article key={pack.venueName} className="pack-page flex flex-col gap-2">
              <h2 className="text-lg font-medium">
                Pack for {pack.venueName} · {pack.harvestDate}
              </h2>
              {pack.lines.map((line, i) => (
                <p key={i} className="text-sm">
                  {line.cropName} · {trayCountLabel(line.trays)}
                </p>
              ))}
            </article>
          ))}
          <article className="pack-page flex flex-col gap-2">
            <h2 className="text-lg font-medium">Delivery run · {cutDate}</h2>
            {packs.map((pack) => {
              const contact = runContactLine(pack);
              return (
                <div key={pack.venueId} className="flex flex-col">
                  <p className="text-sm">
                    {pack.venueName} · {trayCountLabel(pack.trayTotal)}
                  </p>
                  {contact != null && <p className="text-sm">{contact}</p>}
                </div>
              );
            })}
          </article>
          {lastAction?.kind === "harvested" && (
            <article className="pack-page flex flex-col gap-2">
              <h2 className="text-lg font-medium">{cutDate}</h2>
              <p className="text-sm">{harvestReceiptText(lastAction, unitSystem)}</p>
            </article>
          )}
        </section>
      )}
      <MoneyCaptureControls collapsible />
      <SowSheet
        open={sheetOpen}
        onOpenChange={setSheetOpen}
        crops={crops}
        onSow={handleSow}
        demand={demand}
        coverDates={cover}
        unitSystem={unitSystem}
      />
      <WeightPad
        open={harvestOpen}
        onOpenChange={handleHarvestOpenChange}
        groups={harvestGroupsForPad ?? view?.harvests ?? []}
        onDone={handleHarvestDone}
        onDiscarded={handleDiscarded}
        unitSystem={unitSystem}
      />
      <FarmBackupSheet
        open={backupOpen}
        onOpenChange={setBackupOpen}
        onRestored={(label) => {
          setLastAction({ kind: "restored", label });
          void refresh();
        }}
        recoveryMode={recoveryVisible}
        onImported={() => {
          void refresh();
        }}
      />
    </main>
  );
}
