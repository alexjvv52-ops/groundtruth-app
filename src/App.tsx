import { useEffect, useState } from "react";
import { useDialogPointerGuard } from "@/lib/dialogPointerGuard";
import { Today } from "@/screens/Today";
import { Reality } from "@/screens/Reality";
import { Health } from "@/screens/Health";
import { Marketing } from "@/screens/Marketing";
import { Money } from "@/screens/Money";
import { Settings } from "@/screens/Settings";
import { Books } from "@/screens/Books";
import { StatusMark } from "@/components/StatusMark";
import { autoPullPhoneCaptures, dockFolds, healthStatus, pollStripe, takeNewPaidOrders } from "@/farm/api";
import type { CheckStatus, DockFoldsView, Severity } from "@/farm/types";
import type { MoneyFocus } from "@/farm/surfaces";
/**
 * Five top-level tabs (operator to-do / Farm tab separation, shell fence 1):
 * Today · Farm · Marketing · Money · Health. Today is the operator to-do surface
 * and the app opens on it, always — no memory of the last tab. Farm renders the
 * physical-farm state screen (src/screens/Reality.tsx keeps its file name).
 *
 * Five loop tabs plus Books, which sits in the row between Money and Health but
 * stays muted because it is a read-only reporting lens and not part of the daily
 * loop (B-7, B-5(b)). Settings stays last and right-aligned. When the labels no
 * longer fit, the row wraps rather than colliding.
 *
 * DESK-LIFE — the tab row is a strip on a hairline; the selected tab is a bottom
 * border in ink; the healthy mark sits at the row's end. Sentences unchanged.
 */
type Destination = "today" | "farm" | "marketing" | "money" | "health" | "books" | "settings";
function Badge({ severity, label }: { severity: Severity | null; label: string }) {
  if (severity !== "Unhealthy" && severity !== "Degraded") return null;
  return (
    <span
      aria-label={`${label} — ${severity}`}
      className={
        severity === "Unhealthy"
          ? "ml-1 inline-block h-2.5 w-2.5 rounded-full bg-red-600 align-middle"
          : "ml-1 inline-block h-2.5 w-2.5 rounded-full bg-amber-500 align-middle"
      }
    />
  );
}
function App() {
  useDialogPointerGuard();
  // The app always opens on Today — the operator to-do surface. Never the last tab.
  const [destination, setDestination] = useState<Destination>("today");
  const [statuses, setStatuses] = useState<CheckStatus[] | null>(null);
  const [folds, setFolds] = useState<DockFoldsView | null>(null);
  const [newPaidCount, setNewPaidCount] = useState(0);
  const [pollTick, setPollTick] = useState(0);
  const [moneyFocus, setMoneyFocus] = useState<MoneyFocus | null>(null);
  const [marketingFocus, setMarketingFocus] = useState<"new-venue" | null>(null);
  async function loadHealth() {
    try {
      const next = await healthStatus();
      setStatuses(next);
    } catch {
      setStatuses([]);
    }
    try {
      setFolds(await dockFolds());
    } catch {
      // A stale fold presented as current is the thing S2c forbids; a brief blank is honest.
      setFolds(null);
    }
  }
  useEffect(() => {
    void loadHealth().catch(() => undefined);
    const interval = window.setInterval(() => {
      void loadHealth();
    }, 60_000);
    const onFocus = () => {
      void loadHealth(); void autoPullPhoneCaptures().catch(console.error).finally(() => setPollTick((t) => t + 1));
    };
    window.addEventListener("focus", onFocus);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", onFocus);
    };
  }, []);
  useEffect(() => {
    let cancelled = false;
    async function pollCycle() {
      try {
        await pollStripe();
        const n = await takeNewPaidOrders();
        if (!cancelled && n.count > 0) {
          setNewPaidCount((prev) => prev + n.count);
        }
      } catch (err) {
        console.error(err);
      }
      try { await autoPullPhoneCaptures(); } catch (err) { console.error(err); }
      if (!cancelled) setPollTick((t) => t + 1);
    }
    void pollCycle();
    const id = window.setInterval(() => {
      void pollCycle();
    }, 60_000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);
  const moneySeverity =
    folds?.cards.find((c) => c.card === "money")?.severity ?? null;
  // DESK-LIFE — one base for the seven tabs; the seven className lines below are untouched.
  const navBase =
    "-mb-px min-h-11 border-b-2 transition-colors duration-150 motion-reduce:transition-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";
  const navActive = `${navBase} border-foreground font-medium text-foreground`;
  const navIdle = `${navBase} border-transparent text-muted-foreground hover:text-foreground`;
  return (
    <div className="mx-auto flex min-h-screen w-full flex-col">
      <header className="flex flex-col gap-3 pt-6">
        <nav className="flex flex-wrap items-end gap-x-3 border-b border-border px-6 text-sm min-[440px]:gap-x-4">
          <button
            type="button"
            className={destination === "today" ? navActive : navIdle}
            onClick={() => setDestination("today")}
          >
            Today
          </button>
          <button
            type="button"
            className={destination === "farm" ? navActive : navIdle}
            onClick={() => setDestination("farm")}
          >
            Farm
          </button>
          <button
            type="button"
            className={destination === "marketing" ? navActive : navIdle}
            onClick={() => { setMarketingFocus(null); setDestination("marketing"); }}
          >
            Marketing
          </button>
          <button
            type="button"
            className={destination === "money" ? navActive : navIdle}
            onClick={() => {
              setMoneyFocus(null);
              setDestination("money");
            }}
          >
            Money
            <Badge severity={moneySeverity} label="Money" />
          </button>
          <button
            type="button"
            className={destination === "books" ? navActive : navIdle}
            onClick={() => setDestination("books")}
          >
            Books
          </button>
          <button
            type="button"
            className={destination === "health" ? navActive : navIdle}
            onClick={() => setDestination("health")}
          >
            Health
          </button>
          <button
            type="button"
            className={
              destination === "settings"
                ? `ml-auto ${navActive}`
                : `ml-auto ${navIdle}`
            }
            onClick={() => setDestination("settings")}
          >
            Settings
          </button>
          <StatusMark statuses={statuses} folds={folds} inline />
        </nav>
        <div className="mx-auto w-full max-w-md px-6 empty:hidden">
          <StatusMark
            statuses={statuses}
            folds={folds}
            onOpenHealth={() => setDestination("health")}
          />
        </div>
      </header>
      <div key={destination} className="animate-in fade-in-0 duration-150 motion-reduce:animate-none">
        {destination === "today" ? (
          <Today
            newPaidCount={newPaidCount}
            pollTick={pollTick}
            onOpenMoney={(focus) => {
              setMoneyFocus(focus ?? null);
              setDestination("money");
            }}
            onOpenMarketing={(focus) => { setMarketingFocus(focus ?? null); setDestination("marketing"); }}
            onOpenSettings={() => setDestination("settings")}
            // B2-F1 (B2-D4) — the phone-captures pointer taps through to Farm. Navigation only.
            onOpenFarm={() => setDestination("farm")}
            // SOP-7 (C-6) — the Health tail card's Open Health. Navigation only; the same closure StatusMark gets.
            onOpenHealth={() => setDestination("health")}
          />
        ) : destination === "farm" ? (
          <Reality pollTick={pollTick} />
        ) : destination === "marketing" ? (
          // FIRST-15 — the new-venue deep link. Navigation only; Marketing writes nothing on arrival.
          <Marketing focus={marketingFocus} onFocusHandled={() => setMarketingFocus(null)} />
        ) : destination === "money" ? (
          // B1-F2 (D14) — "Back to Today" after a deep-linked write. Navigation only.
          <Money
            focus={moneyFocus}
            onFocusHandled={() => setMoneyFocus(null)}
            onBackToToday={() => setDestination("today")}
          />
        ) : destination === "books" ? (
          <Books />
        ) : destination === "settings" ? (
          <Settings />
        ) : (
          <Health
            onStatusesChange={(next) => {
              setStatuses(next);
            }}
          />
        )}
      </div>
    </div>
  );
}
export default App;
