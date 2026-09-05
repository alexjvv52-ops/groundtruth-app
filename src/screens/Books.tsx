/**
 * Books v1.1 — a read-only lens on the single source of truth. Nine cards, each
 * with its method in plain language and a "Show the rows" disclosure onto the
 * exact records the morning loop already trusts. It never writes and it derives
 * nothing: every figure comes from a Books-a or Books-c command already
 * registered. Books-c landed every reader this surface needs (5eb41ba).
 *
 * B-6(a) signed 2026-08-23: no new palette. Card, text-muted-foreground and the
 * amount-first ordering already used on Money are the whole visual budget.
 */
import { useEffect, useState } from "react";
import type {
  BadDebtRow,
  BadDebtSummary,
  CashCollected,
  CashOut,
  CashRow,
  CategoryTotal,
  CostEvent,
  IncomeCorrectionCount,
  IncomeCorrectionRow,
  NetCash,
  OwedSummary,
  UnpricedExposure,
  UnpricedOrderRow,
  WholesaleOrderView,
  WriteOffRow,
  WriteOffSummary,
} from "@/farm/types";
import {
  badDebtRows,
  badDebtSummary,
  cashByCategory,
  cashCollected,
  cashOut,
  cashRows,
  expenseRows,
  incomeCorrectionRows,
  incomeCorrectionsCount,
  listWholesaleOrders,
  netCash,
  owedSummary,
  unpricedExposure,
  unpricedOrders,
  writeOffRows,
  writeOffSummary,
} from "@/farm/api";
import { formatCents } from "@/farm/dollars";
import {
  localToday,
  monthDayLabel,
  monthStart,
  parseLocalDate,
  toYyyyMmDd,
  weekStartMonday,
} from "@/farm/dates";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

type Mode = "week" | "month" | "custom" | "all";

function caughtMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

function deliveredOldestFirst(
  orders: WholesaleOrderView[],
): WholesaleOrderView[] {
  return orders
    .filter((o) => o.state === "delivered")
    .slice()
    .sort((a, b) => {
      if (a.deliveredOn == null && b.deliveredOn == null) {
        return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
      }
      if (a.deliveredOn == null) return 1;
      if (b.deliveredOn == null) return -1;
      if (a.deliveredOn !== b.deliveredOn) {
        return a.deliveredOn < b.deliveredOn ? -1 : 1;
      }
      return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
    });
}

export function Books() {
  const [mode, setMode] = useState<Mode>("month");
  const [customFrom, setCustomFrom] = useState("");
  const [customTo, setCustomTo] = useState("");
  const [showCash, setShowCash] = useState(false);
  const [showOwed, setShowOwed] = useState(false);
  const [showWriteOffs, setShowWriteOffs] = useState(false);
  const [showUnpriced, setShowUnpriced] = useState(false);
  const [showOut, setShowOut] = useState(false);
  const [showNet, setShowNet] = useState(false);
  const [showBadDebt, setShowBadDebt] = useState(false);
  const [showCategory, setShowCategory] = useState(false);
  const [showCorrections, setShowCorrections] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cash, setCash] = useState<CashCollected | null>(null);
  const [cashList, setCashList] = useState<CashRow[]>([]);
  const [owed, setOwed] = useState<OwedSummary | null>(null);
  const [orders, setOrders] = useState<WholesaleOrderView[]>([]);
  const [summary, setSummary] = useState<WriteOffSummary | null>(null);
  const [writeOffs, setWriteOffs] = useState<WriteOffRow[]>([]);
  const [exposure, setExposure] = useState<UnpricedExposure | null>(null);
  const [unpriced, setUnpriced] = useState<UnpricedOrderRow[]>([]);
  const [out, setOut] = useState<CashOut | null>(null);
  const [expenses, setExpenses] = useState<CostEvent[]>([]);
  const [net, setNet] = useState<NetCash | null>(null);
  const [badDebt, setBadDebt] = useState<BadDebtSummary | null>(null);
  const [badDebts, setBadDebts] = useState<BadDebtRow[]>([]);
  const [categories, setCategories] = useState<CategoryTotal[]>([]);
  const [corrections, setCorrections] = useState<IncomeCorrectionCount | null>(
    null,
  );
  const [correctionList, setCorrectionList] = useState<IncomeCorrectionRow[]>(
    [],
  );

  const today = localToday();
  const from =
    mode === "week"
      ? toYyyyMmDd(weekStartMonday(today))
      : mode === "month"
        ? toYyyyMmDd(monthStart(today))
        : mode === "custom"
          ? customFrom || null
          : null;
  const to =
    mode === "week" || mode === "month"
      ? toYyyyMmDd(today)
      : mode === "custom"
        ? customTo || null
        : null;

  useEffect(() => {
    let cancelled = false;
    async function load() {
      try {
        const [
          nextCash,
          nextCashRows,
          nextSummary,
          nextWriteOffs,
          nextExposure,
          nextUnpriced,
          nextOwed,
          nextOrders,
          nextOut,
          nextExpenses,
          nextNet,
          nextBadDebt,
          nextBadDebts,
          nextCategories,
          nextCorrections,
          nextCorrectionList,
        ] = await Promise.all([
          cashCollected(from, to),
          cashRows(from, to),
          writeOffSummary(from, to),
          writeOffRows(from, to),
          unpricedExposure(),
          unpricedOrders(),
          owedSummary(),
          listWholesaleOrders(),
          cashOut(from, to),
          expenseRows(from, to),
          netCash(from, to),
          badDebtSummary(from, to),
          badDebtRows(from, to),
          cashByCategory(from, to),
          incomeCorrectionsCount(from, to),
          incomeCorrectionRows(from, to),
        ]);
        if (cancelled) return;
        setCash(nextCash);
        setCashList(nextCashRows);
        setSummary(nextSummary);
        setWriteOffs(nextWriteOffs);
        setExposure(nextExposure);
        setUnpriced(nextUnpriced);
        setOwed(nextOwed);
        setOrders(nextOrders);
        setOut(nextOut);
        setExpenses(nextExpenses);
        setNet(nextNet);
        setBadDebt(nextBadDebt);
        setBadDebts(nextBadDebts);
        setCategories(nextCategories);
        setCorrections(nextCorrections);
        setCorrectionList(nextCorrectionList);
        setError(null);
      } catch (e: unknown) {
        if (cancelled) return;
        setError(caughtMessage(e));
      }
    }
    void load();
    return () => {
      cancelled = true;
    };
  }, [from, to]);

  const periodBtn = (id: Mode, label: string) => (
    <button
      type="button"
      className={
        mode === id
          ? "text-sm text-muted-foreground underline underline-offset-4"
          : "text-sm text-muted-foreground"
      }
      onClick={() => setMode(id)}
    >
      {label}
    </button>
  );

  const delivered = deliveredOldestFirst(orders);
  // PACK-LO-SPEAK (audit R-4, ruling 4): the Cash collected caption names
  // leftover only when leftover income exists in the period. Keyed on the
  // machine-written income source "Leftover {crop} · {day}" (leftover.rs:538,
  // 607 — pinned by lo_b_cash_tests). Zero leftover in the period renders the
  // caption byte-identical to the pre-existing sentence.
  const leftoverInPeriod = cashList.some(
    (row) => row.recordType === "recorded" && row.source.startsWith("Leftover "),
  );
  const owedFigure =
    owed == null
      ? null
      : owed.totalCents != null
        ? `${formatCents(owed.totalCents)} across ${owed.deliveries} ${owed.deliveries === 1 ? "delivery" : "deliveries"}`
        : `${owed.deliveries} ${owed.deliveries === 1 ? "delivery" : "deliveries"}, value partly unpriced`;

  return (
    <main className="flex flex-col gap-4 px-6 py-6">
      <div className="flex flex-wrap gap-x-3 gap-y-1">
        {periodBtn("week", "This week")}
        {periodBtn("month", "This month")}
        {periodBtn("custom", "Custom")}
        {periodBtn("all", "All time")}
      </div>
      {mode === "custom" && (
        <div className="flex flex-wrap gap-3">
          <label className="flex flex-col gap-1 text-sm text-muted-foreground">
            From
            <input
              type="date"
              value={customFrom}
              onChange={(e) => setCustomFrom(e.target.value)}
              className="text-sm"
            />
          </label>
          <label className="flex flex-col gap-1 text-sm text-muted-foreground">
            To
            <input
              type="date"
              value={customTo}
              onChange={(e) => setCustomTo(e.target.value)}
              className="text-sm"
            />
          </label>
        </div>
      )}
      {error != null && (
        <p className="text-sm text-muted-foreground">{error}</p>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Cash collected</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {cash != null && (
            <p className="text-2xl font-medium">
              {formatCents(cash.totalCents)}
            </p>
          )}
          <p className="text-sm text-muted-foreground">
            {leftoverInPeriod
              ? "Every payment dated in this period — wholesale payments recorded on Money, paid online orders, and leftover listings paid on Money. Each dollar counted once."
              : "Every payment dated in this period — wholesale payments recorded on Money, and paid online orders. Each dollar counted once."}
          </p>
          <button
            type="button"
            className="text-sm text-muted-foreground"
            onClick={() => setShowCash((v) => !v)}
          >
            Show the rows
          </button>
          {showCash && (
            <ul className="flex flex-col gap-2">
              {cashList.map((row) => (
                <li
                  key={`${row.recordType}:${row.incomeId}`}
                  className="flex flex-col gap-0.5 text-sm"
                >
                  <span>{row.dateReceived}</span>
                  <span className="text-muted-foreground">{row.source}</span>
                  <span>{formatCents(row.amountCents)}</span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Still owed to us</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {owedFigure != null && (
            <p className="text-2xl font-medium">{owedFigure}</p>
          )}
          <p className="text-sm text-muted-foreground">
            Delivered orders that have not been paid, as of right now. Not filtered by
            period. Oldest first.
          </p>
          <button
            type="button"
            className="text-sm text-muted-foreground"
            onClick={() => setShowOwed((v) => !v)}
          >
            Show the rows
          </button>
          {showOwed && (
            <ul className="flex flex-col gap-2">
              {delivered.map((o) => (
                <li key={o.id} className="flex flex-col gap-0.5 text-sm">
                  <span>{o.venueName}</span>
                  {o.deliveredOn != null && (
                    <span className="text-muted-foreground">
                      {monthDayLabel(parseLocalDate(o.deliveredOn))}
                    </span>
                  )}
                  {o.pricedTotalCents != null ? (
                    <span>{formatCents(o.pricedTotalCents)}</span>
                  ) : null}
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Write-offs this period</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {summary != null && (
            <p className="text-2xl font-medium">
              {formatCents(summary.totalShortfallCents)}
            </p>
          )}
          <p className="text-sm text-muted-foreground">
            Allowances recorded when an order was settled for less than its priced
            total, dated in this period. A reversed payment's allowance is not counted.
          </p>
          <button
            type="button"
            className="text-sm text-muted-foreground"
            onClick={() => setShowWriteOffs((v) => !v)}
          >
            Show the rows
          </button>
          {showWriteOffs && (
            <ul className="flex flex-col gap-2">
              {writeOffs.map((row) => (
                <li key={row.eventId} className="flex flex-col gap-0.5 text-sm">
                  <span>{row.writtenOffOn}</span>
                  <span className="text-muted-foreground">{row.venueName}</span>
                  <span>{formatCents(row.shortfallCents)}</span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Unpriced exposure</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {exposure != null && (
            <p className="text-2xl font-medium">{exposure.count}</p>
          )}
          <p className="text-sm text-muted-foreground">
            Open orders with at least one line that has no price. There is no total for
            these, so only the count is shown.
          </p>
          <button
            type="button"
            className="text-sm text-muted-foreground"
            onClick={() => setShowUnpriced((v) => !v)}
          >
            Show the rows
          </button>
          {showUnpriced && (
            <ul className="flex flex-col gap-2">
              {unpriced.map((row) => (
                <li key={row.id} className="flex flex-col gap-0.5 text-sm">
                  <span>{row.venueName}</span>
                  <span className="text-muted-foreground">{row.harvestDate}</span>
                  <span className="text-muted-foreground">{row.state}</span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Cash out this period</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {out != null && (
            <p className="text-2xl font-medium">
              {formatCents(out.totalCents)}
            </p>
          )}
          <p className="text-sm text-muted-foreground">
            Costs and expenses dated in this period. Each dollar counted once.
          </p>
          <button
            type="button"
            className="text-sm text-muted-foreground"
            onClick={() => setShowOut((v) => !v)}
          >
            Show the rows
          </button>
          {showOut && (
            <ul className="flex flex-col gap-2">
              {expenses.map((row) => (
                <li key={row.eventId} className="flex flex-col gap-0.5 text-sm">
                  <span>{row.datePaid}</span>
                  <span className="text-muted-foreground">{row.payee}</span>
                  <span>{formatCents(row.amountCents)}</span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Net cash this period</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {net != null && (
            <p className="text-2xl font-medium">
              {formatCents(net.netCents)}
            </p>
          )}
          <p className="text-sm text-muted-foreground">
            Cash collected minus cash out for this period. Both sides come from
            the same honest trails.
          </p>
          <button
            type="button"
            className="text-sm text-muted-foreground"
            onClick={() => setShowNet((v) => !v)}
          >
            Show the rows
          </button>
          {showNet && net != null && (
            <ul className="flex flex-col gap-2">
              <li className="flex flex-col gap-0.5 text-sm">
                <span>{formatCents(net.collectedCents)}</span>
              </li>
              <li className="flex flex-col gap-0.5 text-sm">
                <span>{formatCents(net.outCents)}</span>
              </li>
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Bad debts this period</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {badDebt != null && (
            <p className="text-2xl font-medium">
              {formatCents(badDebt.totalCents)}
            </p>
          )}
          <p className="text-sm text-muted-foreground">
            Orders written off as uncollectible, dated in this period. Separate
            from settlement allowances.
          </p>
          <button
            type="button"
            className="text-sm text-muted-foreground"
            onClick={() => setShowBadDebt((v) => !v)}
          >
            Show the rows
          </button>
          {showBadDebt && (
            <ul className="flex flex-col gap-2">
              {badDebts.map((row) => (
                <li key={row.eventId} className="flex flex-col gap-0.5 text-sm">
                  <span>{row.writtenOffOn}</span>
                  <span className="text-muted-foreground">{row.venueName}</span>
                  <span>{formatCents(row.amountCents)}</span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Income by category</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-sm text-muted-foreground">
            Cash collected this period, grouped by category. Same dollars as Cash
            collected, only grouped.
          </p>
          <button
            type="button"
            className="text-sm text-muted-foreground"
            onClick={() => setShowCategory((v) => !v)}
          >
            Show the rows
          </button>
          {showCategory && (
            <ul className="flex flex-col gap-2">
              {categories.map((row) => (
                <li
                  key={row.categoryId}
                  className="flex flex-col gap-0.5 text-sm"
                >
                  <span>{row.name}</span>
                  <span>{formatCents(row.totalCents)}</span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Income corrections this period</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {corrections != null && (
            <p className="text-2xl font-medium">{corrections.count}</p>
          )}
          <p className="text-sm text-muted-foreground">
            Income that was voided or corrected in this period. The original cash
            is no longer counted.
          </p>
          <button
            type="button"
            className="text-sm text-muted-foreground"
            onClick={() => setShowCorrections((v) => !v)}
          >
            Show the rows
          </button>
          {showCorrections && (
            <ul className="flex flex-col gap-2">
              {correctionList.map((row) => (
                <li
                  key={row.correctionEventId}
                  className="flex flex-col gap-0.5 text-sm"
                >
                  <span>{row.correctedOn}</span>
                  <span className="text-muted-foreground">{row.action}</span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>
    </main>
  );
}
