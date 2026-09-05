import { useEffect, useState } from "react";
import type { Crop } from "@/farm/types";
import type { CoverDate } from "@/farm/types";
import { addDays, localToday, parseLocalDate, readyLabel, monthDayLabel } from "@/farm/dates";
import {
  confirmSeedQuantity,
  freshSeedProposal,
  onOperatorSeedEdit,
  onProposalInputsChanged,
  type SeedFieldState,
} from "@/farm/seedPrefill";
import { shelfPressure } from "@/farm/api";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import { MoneyJustLeftSheet } from "@/components/MoneyJustLeftSheet";

type SowSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  crops: Crop[];
  onSow: (crop: Crop, quantity: number, seedOz: number | null) => void;
  demand?: import("@/farm/types").StandingDemandView | null;
  coverDates?: CoverDate[] | null;
};

export function SowSheet({
  open,
  onOpenChange,
  crops,
  onSow,
  demand = null,
  coverDates = null,
}: SowSheetProps) {
  const [selectedCrop, setSelectedCrop] = useState<Crop | null>(null);
  const [quantity, setQuantity] = useState(1);
  const [seedField, setSeedField] = useState<SeedFieldState>({
    value: "",
    dirty: false,
  });
  const [seedError, setSeedError] = useState<string | null>(null);
  const [costOpen, setCostOpen] = useState(false);
  const [pressureLine, setPressureLine] = useState<string | null>(null);

  function reset() {
    setSelectedCrop(null);
    setQuantity(1);
    setSeedField({ value: "", dirty: false });
    setSeedError(null);
  }

  function handleOpenChange(next: boolean) {
    if (!next && costOpen) return;
    if (!next) reset();
    onOpenChange(next);
  }

  function pickCrop(crop: Crop) {
    const need =
      demand != null
        ? demand.varieties.find((v) => v.name === crop.name)?.shortfall ?? 0
        : 0;
    // B2-F1 (B2-D2) — the cover gap wins for a crop that reaches the sow
    // target: pre-fill the date's short (the plan's own number, the same one
    // the header sentence prints), capped by this crop's free slots when the
    // ceiling is known, never below 1. Otherwise the standing-need rule as
    // before. A proposal only: the write still happens on Sow.
    const own =
      (coverDates ?? []).find(
        (c) => c.cropId === crop.id && c.reachability.sowCanServe,
      ) ?? null;
    const reach =
      own?.reachability.crops.find(
        (c) => c.cropId === crop.id && c.canSowNow,
      ) ?? null;
    const q =
      own != null && reach != null
        ? Math.max(
            1,
            reach.slotsFree != null
              ? Math.min(own.shortTrays, reach.slotsFree)
              : own.shortTrays,
          )
        : need > 0
          ? need
          : 1;
    setQuantity(q);
    setSelectedCrop(crop);
    setSeedField(freshSeedProposal(crop.seedRateOzPerTray, q));
    setSeedError(null);
  }

  useEffect(() => {
    if (!open) {
      setPressureLine(null);
      return;
    }
    let cancelled = false;
    void shelfPressure()
      .then((line) => {
        if (!cancelled) setPressureLine(line);
      })
      .catch((err) => {
        console.error(err);
        if (!cancelled) setPressureLine(null);
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  useEffect(() => {
    if (!selectedCrop) return;
    setSeedField((prev) =>
      onProposalInputsChanged(
        prev,
        selectedCrop.seedRateOzPerTray,
        quantity,
      ),
    );
  }, [quantity, selectedCrop]);

  function confirmSow() {
    if (!selectedCrop) return;
    const parsed = confirmSeedQuantity(seedField);
    if (!parsed.ok) {
      setSeedError(parsed.error);
      return;
    }
    setSeedError(null);
    onSow(selectedCrop, quantity, parsed.quantity);
    reset();
  }

  const standingNames = demand?.standingVarieties ?? [];
  const isStandingCrop = (name: string) => standingNames.includes(name);
  const varietyNeed = (name: string) =>
    demand?.varieties.find((v) => v.name === name)?.shortfall ?? 0;
  const money = coverDates ?? [];
  const unreachableDates = money.filter((c) => !c.reachability.reachable);
  const reachableDates = money.filter((c) => c.reachability.reachable);
  // The soonest date sowing can still serve - what a crop tile is measured
  // against. Unreachable and shelf-blocked dates are never a sow target.
  const target =
    money.find((c) => c.reachability.sowCanServe) ?? null;
  const targetLabel =
    target != null ? monthDayLabel(parseLocalDate(target.harvestDate)) : "";
  // A crop must reach the target date in time AND fit on the shelf for the
  // whole span it needs. canSowNow carries both; see CropReach.
  const reaches = (crop: Crop) =>
    target == null ||
    target.reachability.crops.some(
      (c) => c.cropId === crop.id && c.canSowNow,
    );
  const standingGap =
    demand != null && demand.varieties.some((v) => v.shortfall > 0);
  // Free sow stays demoted while ANY money gap is open, unreachable included.
  const anyGap = standingGap || money.length > 0;
  const displayCrops = anyGap
    ? [...crops].sort((a, b) => {
        if (target != null) {
          const ra = Number(reaches(a));
          const rb = Number(reaches(b));
          if (rb !== ra) return rb - ra;
        }
        const na = varietyNeed(a.name);
        const nb = varietyNeed(b.name);
        if (nb !== na) return nb - na;
        return (
          Number(isStandingCrop(b.name)) - Number(isStandingCrop(a.name)) ||
          a.sortOrder - b.sortOrder
        );
      })
    : crops;

  return (
    <>
      <Sheet open={open} onOpenChange={handleOpenChange}>
        <SheetContent
          side="bottom"
          showCloseButton={false}
          aria-describedby={undefined}
        >
          <div className="mx-auto w-full max-w-md p-4">
            {selectedCrop === null ? (
              <>
                <SheetTitle className="sr-only">Pick a crop to sow</SheetTitle>
                {unreachableDates.map((c) => (
                  <p
                    key={`${c.harvestDate}|${c.cropId}`}
                    className="mb-1 text-base font-medium text-red-700"
                  >
                    {c.message}
                  </p>
                ))}
                {reachableDates.map((c) => (
                  <p
                    key={`${c.harvestDate}|${c.cropId}`}
                    className="mb-1 text-base font-medium text-amber-700"
                  >
                    {c.message}
                  </p>
                ))}
                {standingGap && demand != null && (
                  <p className="mb-1 text-base font-medium text-amber-700">
                    Still needed for standing orders:{" "}
                    {demand.varieties
                      .filter((v) => v.shortfall > 0)
                      .map((v) => `${v.shortfall} ${v.name}`)
                      .join(" · ")}
                  </p>
                )}
                {unreachableDates.length > 0 && (
                  <p className="mb-1 text-sm text-muted-foreground">
                    Nothing sown today reaches{" "}
                    {unreachableDates.length === 1
                      ? "that date"
                      : "those dates"}
                    . Anything you sow now is for later dates.
                  </p>
                )}
                {pressureLine != null && (
                  <p className="mb-1 text-sm text-muted-foreground">
                    {pressureLine}
                  </p>
                )}
                {(standingGap || reachableDates.length > 0) && (
                  <p className="mb-3 text-sm text-muted-foreground">
                    Sowing anything else is fine — these are the open gaps.
                  </p>
                )}
                {demand != null && demand.traysWeek > 0 && !standingGap && (
                  <p className="mb-3 text-base font-medium">
                    Standing orders covered for the last 7 days.
                  </p>
                )}
                <div className="grid grid-cols-2 gap-3">
                  {displayCrops.map((crop) => (
                    <button
                      key={crop.id}
                      type="button"
                      onClick={() => pickCrop(crop)}
                      className="flex min-h-24 flex-col items-start justify-center gap-1 rounded-xl border bg-card p-4 text-left transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      <span className="text-lg font-medium leading-tight">
                        {crop.name}
                      </span>
                      <span className="text-sm text-muted-foreground">
                        ~{crop.growthDays} days
                      </span>
                      {anyGap && varietyNeed(crop.name) > 0 && (
                        <span className="text-xs text-muted-foreground">
                          needs {varietyNeed(crop.name)}
                        </span>
                      )}
                      {anyGap &&
                        isStandingCrop(crop.name) &&
                        varietyNeed(crop.name) === 0 && (
                        <span className="text-xs text-muted-foreground">
                          standing
                        </span>
                      )}
                      {target != null && (
                        <span className="text-xs text-muted-foreground">
                          {reaches(crop)
                            ? `reaches ${targetLabel}`
                            : `does not reach ${targetLabel}`}
                        </span>
                      )}
                    </button>
                  ))}
                </div>
                {demand != null && demand.traysWeek > 0 && (
                  <p className="mt-4 text-sm text-muted-foreground">
                    Standing orders: {demand.traysWeek}{" "}
                    {demand.traysWeek === 1 ? "tray" : "trays"}/week ·{" "}
                    {demand.sownLast7Days} sown in the last 7 days.
                  </p>
                )}
                <button
                  type="button"
                  onClick={() => setCostOpen(true)}
                  className="mt-4 flex min-h-11 w-full items-center justify-center rounded-xl border px-4 text-sm font-medium transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  Money just left
                </button>
              </>
            ) : (
              <>
                <SheetTitle className="sr-only">
                  Confirm sowing {selectedCrop.name}
                </SheetTitle>
                <div className="flex flex-col gap-6">
                  <div>
                    <Button
                      type="button"
                      variant="ghost"
                      onClick={() => setSelectedCrop(null)}
                      className="h-14 -ml-2 px-3 text-base text-muted-foreground"
                    >
                      ← Back
                    </Button>
                  </div>

                  <h2 className="text-2xl font-semibold">{selectedCrop.name}</h2>

                  <div className="flex items-center justify-center gap-6">
                    <button
                      type="button"
                      aria-label="Fewer trays"
                      onClick={() => setQuantity((q) => Math.max(1, q - 1))}
                      disabled={quantity <= 1}
                      className="flex size-14 items-center justify-center rounded-full border text-2xl leading-none transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:opacity-40"
                    >
                      −
                    </button>
                    <span className="w-12 text-center text-3xl font-semibold tabular-nums">
                      {quantity}
                    </span>
                    <button
                      type="button"
                      aria-label="More trays"
                      onClick={() => setQuantity((q) => q + 1)}
                      className="flex size-14 items-center justify-center rounded-full border text-2xl leading-none transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      +
                    </button>
                  </div>

                  <p className="text-center text-base text-muted-foreground">
                    {readyLabel(
                      addDays(localToday(), selectedCrop.growthDays),
                    )}
                  </p>
                  {target != null && (
                    <p
                      className={
                        reaches(selectedCrop)
                          ? "text-center text-sm text-muted-foreground"
                          : "text-center text-sm text-amber-700"
                      }
                    >
                      {reaches(selectedCrop)
                        ? `Covers ${targetLabel}.`
                        : `Does not reach ${targetLabel} — that gap needs a faster crop.`}
                    </p>
                  )}

                  <label className="flex flex-col gap-2">
                    <span className="text-sm text-muted-foreground">
                      Seed weight (oz)
                    </span>
                    <input
                      type="text"
                      inputMode="decimal"
                      value={seedField.value}
                      onChange={(e) => {
                        setSeedError(null);
                        setSeedField((prev) =>
                          onOperatorSeedEdit(prev, e.target.value),
                        );
                      }}
                      placeholder="Weigh and enter"
                      className="h-14 w-full rounded-xl border bg-card px-4 text-lg tabular-nums focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    />
                    {seedError ? (
                      <span className="text-sm text-destructive">{seedError}</span>
                    ) : null}
                  </label>

                  <Button
                    type="button"
                    onClick={confirmSow}
                    className="h-14 w-full text-lg"
                  >
                    Sow
                  </Button>

                  <button
                    type="button"
                    onClick={() => setCostOpen(true)}
                    className="flex min-h-11 w-full items-center justify-center rounded-xl border px-4 text-sm font-medium transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  >
                    Money just left
                  </button>
                </div>
              </>
            )}
          </div>
        </SheetContent>
      </Sheet>

      <MoneyJustLeftSheet
        open={costOpen}
        onOpenChange={setCostOpen}
        moment="sow"
        stacked
      />
    </>
  );
}
