import { useEffect, useState } from "react";
import type { CostCategory, CostPerTrayOutcome } from "@/farm/types";
import { costPerTray, listCostCategories } from "@/farm/api";
import { formatCents } from "@/farm/dollars";
import { localToday } from "@/farm/dates";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";

type CostPerTraySheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

type WindowChoice = "last_30" | "last_90" | "ytd" | "all" | "custom";

function errMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return "Could not work it out. Try again.";
}

function toYyyyMmDd(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

const WINDOW_CHIPS: { id: WindowChoice; label: string }[] = [
  { id: "last_30", label: "Last 30 days" },
  { id: "last_90", label: "Last 90 days" },
  { id: "ytd", label: "This year so far" },
  { id: "all", label: "Everything recorded" },
  { id: "custom", label: "Pick dates" },
];

function MethodBlock({ method }: { method: CostPerTrayOutcome["method"] }) {
  return (
    <div className="flex flex-col gap-3">
      <h3 className="text-base font-semibold">How this was worked out</h3>
      <p className="text-sm text-muted-foreground">{method.windowLabel}</p>
      <p className="text-sm">{method.paymentRule}</p>
      <p className="text-sm">{method.physicalRule}</p>
      <p className="text-sm">{method.joinRule}</p>
      <p className="text-sm">{method.exclusionRule}</p>
      <p className="text-sm">{method.completenessNote}</p>

      <div className="flex flex-col gap-2 pt-2">
        <h4 className="text-sm font-medium">
          Payments included ({method.paymentCount})
        </h4>
        {method.payments.length === 0 ? (
          <p className="text-sm text-muted-foreground">None</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {method.payments.map((p) => (
              <li
                key={p.eventId}
                className="text-sm tabular-nums text-muted-foreground"
              >
                {p.datePaid} · {p.payee} · {formatCents(p.amountCents)}
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="flex flex-col gap-2 pt-2">
        <h4 className="text-sm font-medium">
          Sow records included ({method.trayRecordCount})
        </h4>
        {method.trayRecords.length === 0 ? (
          <p className="text-sm text-muted-foreground">None</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {method.trayRecords.map((t) => (
              <li key={t.eventId} className="text-sm text-muted-foreground">
                {t.occurredOn} · {t.varietyOrItem} · {t.quantity}
                {!t.seedQuantityRecorded ? (
                  <span className="ml-2 text-xs text-muted-foreground/80">
                    seed quantity not recorded
                  </span>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

export function CostPerTraySheet({
  open,
  onOpenChange,
}: CostPerTraySheetProps) {
  const [outcome, setOutcome] = useState<CostPerTrayOutcome | null>(null);
  const [windowChoice, setWindowChoice] = useState<WindowChoice>("last_90");
  const [customFrom, setCustomFrom] = useState("");
  const [customTo, setCustomTo] = useState("");
  const [narrowOpen, setNarrowOpen] = useState(false);
  const [categories, setCategories] = useState<CostCategory[]>([]);
  const [selectedCategories, setSelectedCategories] = useState<string[]>([]);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const today = toYyyyMmDd(localToday());

  function resetAll() {
    setOutcome(null);
    setWindowChoice("last_90");
    setCustomFrom("");
    setCustomTo("");
    setNarrowOpen(false);
    setSelectedCategories([]);
    setWorking(false);
    setError(null);
  }

  useEffect(() => {
    if (!open) return;
    resetAll();
    void listCostCategories()
      .then(setCategories)
      .catch((err) => setError(errMessage(err)));
  }, [open]);

  function handleOpenChange(next: boolean) {
    if (!next) {
      resetAll();
    }
    onOpenChange(next);
  }

  function toggleCategory(id: string) {
    setSelectedCategories((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );
  }

  async function handleWorkItOut() {
    if (working) return;
    setWorking(true);
    setError(null);
    try {
      const result = await costPerTray({
        window: windowChoice,
        from: windowChoice === "custom" ? customFrom || null : null,
        to: windowChoice === "custom" ? customTo || null : null,
        categoryIds:
          selectedCategories.length > 0 ? selectedCategories : null,
      });
      setOutcome(result);
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setWorking(false);
    }
  }

  return (
    <Sheet open={open} onOpenChange={handleOpenChange}>
      <SheetContent
        side="bottom"
        showCloseButton={false}
        aria-describedby={undefined}
        className="max-h-[90vh] overflow-y-auto"
      >
        <div className="mx-auto flex w-full max-w-md flex-col gap-5 p-4 pb-8">
          <div>
            <SheetTitle className="text-center text-xl font-semibold">
              What a tray costs
            </SheetTitle>
            <p className="mt-2 text-center text-sm text-muted-foreground">
              Nothing is worked out until you ask. This number is never saved.
            </p>
          </div>

          {!outcome ? (
            <>
              <fieldset className="flex flex-col gap-2">
                <legend className="text-sm text-muted-foreground">
                  Window
                </legend>
                <div className="flex flex-col gap-2">
                  {WINDOW_CHIPS.map((chip) => (
                    <button
                      key={chip.id}
                      type="button"
                      onClick={() => setWindowChoice(chip.id)}
                      className={
                        windowChoice === chip.id
                          ? "min-h-11 rounded-xl border border-foreground bg-accent px-4 py-2 text-left text-base font-medium"
                          : "min-h-11 rounded-xl border bg-card px-4 py-2 text-left text-base transition-colors hover:bg-accent"
                      }
                    >
                      {chip.label}
                    </button>
                  ))}
                </div>
              </fieldset>

              {windowChoice === "custom" ? (
                <div className="flex flex-col gap-3">
                  <label className="flex flex-col gap-1.5">
                    <span className="text-sm text-muted-foreground">From</span>
                    <input
                      type="date"
                      value={customFrom}
                      max={today}
                      onChange={(e) => setCustomFrom(e.target.value)}
                      className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    />
                  </label>
                  <label className="flex flex-col gap-1.5">
                    <span className="text-sm text-muted-foreground">To</span>
                    <input
                      type="date"
                      value={customTo}
                      max={today}
                      onChange={(e) => setCustomTo(e.target.value)}
                      className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    />
                  </label>
                </div>
              ) : null}

              <div className="flex flex-col gap-2">
                <button
                  type="button"
                  onClick={() => setNarrowOpen((v) => !v)}
                  className="min-h-11 text-left text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  Narrow to certain payments
                </button>
                {narrowOpen ? (
                  <div className="flex flex-col gap-2">
                    <p className="text-xs text-muted-foreground">
                      Not saved — this applies to this calculation only.
                    </p>
                    <div className="grid max-h-48 grid-cols-1 gap-2 overflow-y-auto">
                      {categories.map((c) => {
                        const on = selectedCategories.includes(c.id);
                        return (
                          <button
                            key={c.id}
                            type="button"
                            onClick={() => toggleCategory(c.id)}
                            className={
                              on
                                ? "min-h-11 rounded-xl border border-foreground bg-accent px-4 py-2 text-left text-base font-medium"
                                : "min-h-11 rounded-xl border bg-card px-4 py-2 text-left text-base transition-colors hover:bg-accent"
                            }
                          >
                            {c.name}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                ) : null}
              </div>

              {error ? (
                <p role="alert" className="text-center text-sm text-destructive">
                  {error}
                </p>
              ) : null}

              <Button
                type="button"
                onClick={() => void handleWorkItOut()}
                disabled={working}
                className="h-14 w-full text-lg"
              >
                Work it out
              </Button>
            </>
          ) : (
            <>
              {outcome.kind === "computed" ? (
                <div className="flex flex-col items-center gap-2 text-center">
                  <p className="text-3xl font-semibold tabular-nums tracking-tight">
                    {formatCents(Math.round(outcome.figure.centsPerTray))} per
                    tray
                  </p>
                  <p className="text-base text-muted-foreground tabular-nums">
                    {formatCents(outcome.figure.totalPaidCents)} ÷{" "}
                    {outcome.figure.totalTrays} trays
                  </p>
                </div>
              ) : (
                <p
                  role="status"
                  className="text-center text-lg font-medium leading-snug"
                >
                  {outcome.reason}
                </p>
              )}

              <MethodBlock method={outcome.method} />

              <Button
                type="button"
                variant="ghost"
                onClick={() => {
                  setOutcome(null);
                  setError(null);
                }}
                className="h-12 w-full text-base text-muted-foreground"
              >
                Clear
              </Button>
            </>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
