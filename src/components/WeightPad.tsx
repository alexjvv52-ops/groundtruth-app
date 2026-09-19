import { useEffect, useState } from "react";
import type { HarvestCommitmentsView, HarvestGroup, HarvestInput } from "@/farm/types";
import { discardFromGroup, harvestCommitments } from "@/farm/api";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import { ErrorLine } from "@/components/ErrorLine";
import { MoneyJustLeftSheet } from "@/components/MoneyJustLeftSheet";
import { massFigure, unitWord } from "@/farm/mass";
import { typedOunces } from "@/farm/typed";

export type HarvestDoneMeta = {
  trayCount: number;
  varietyCount: number;
  cropName: string | null;
  crops: { cropId: string; cropName: string }[];
};

type WeightPadProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  groups: HarvestGroup[];
  onDone: (groups: HarvestInput[], meta: HarvestDoneMeta) => void;
  onDiscarded: (info: { trayCount: number; cropName: string }) => void;
  unitSystem: string;
};

function trayWord(n: number): string {
  return `${n} ${n === 1 ? "tray" : "trays"}`;
}

export function WeightPad({
  open,
  onOpenChange,
  groups,
  onDone,
  onDiscarded,
  unitSystem,
}: WeightPadProps) {
  const [liveGroups, setLiveGroups] = useState<HarvestGroup[]>([]);
  const [step, setStep] = useState(0);
  const [values, setValues] = useState<string[]>([]);
  const [display, setDisplay] = useState("0");
  const [replaced, setReplaced] = useState(false);
  const [mode, setMode] = useState<"weight" | "discard">("weight");
  const [discardQty, setDiscardQty] = useState(1);
  const [costOpen, setCostOpen] = useState(false);
  const [padError, setPadError] = useState<string | null>(null);

  const count = liveGroups.length;
  const current = liveGroups[step];
  const [commitments, setCommitments] = useState<HarvestCommitmentsView | null>(null);
  useEffect(() => {
    let cancelled = false;
    setCommitments(null);
    if (!open || !current) return;
    harvestCommitments(current.cropId)
      .then((v) => { if (!cancelled) setCommitments(v); })
      .catch(() => { if (!cancelled) setCommitments(null); });
    return () => { cancelled = true; };
  }, [open, current?.cropId]);   // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (open && groups.length > 0) {
      const initial = groups.map((g) => massFigure(g.estimatedYieldOz, unitSystem));
      setLiveGroups(groups);
      setValues(initial);
      setStep(0);
      setDisplay(initial[0]);
      setReplaced(false);
      setMode("weight");
      setDiscardQty(1);
    }
  }, [open, groups]);

  function handleOpenChange(next: boolean) {
    if (!next && costOpen) return;
    onOpenChange(next);
  }

  function loadStep(i: number, stored: string[], nextGroups: HarvestGroup[]) {
    setStep(i);
    setDisplay(stored[i] ?? massFigure(nextGroups[i]?.estimatedYieldOz ?? 0, unitSystem));
    setReplaced(false);
    setMode("weight");
  }

  function pressDigit(d: string) {
    if (!replaced) {
      setDisplay(d);
      setReplaced(true);
      return;
    }
    if (d === "." && display.includes(".")) return;
    if (display === "0" && d !== ".") {
      setDisplay(d);
      return;
    }
    setDisplay((prev) => prev + d);
  }

  function backspace() {
    if (!replaced) {
      setDisplay("");
      setReplaced(true);
      return;
    }
    setDisplay((prev) => prev.slice(0, -1));
  }

  function parsedValue(): number | null {
    const n = Number.parseFloat(display);
    if (display === "" || !Number.isFinite(n) || n <= 0) return null;
    return n;
  }

  function storeCurrent(): string[] | null {
    const n = parsedValue();
    if (n === null) return null;
    const next = [...values];
    next[step] = display;
    setValues(next);
    return next;
  }

  function goNext() {
    const stored = storeCurrent();
    if (!stored) return;
    if (step + 1 < count) {
      loadStep(step + 1, stored, liveGroups);
    }
  }

  function goBack() {
    const next = [...values];
    next[step] = display;
    setValues(next);
    loadStep(step - 1, next, liveGroups);
  }

  function goDone() {
    const stored = storeCurrent();
    if (!stored) return;
    const inputs: HarvestInput[] = liveGroups.map((g, i) => ({
      trayIds: g.trayIds,
      actualYieldOz: typedOunces(stored[i], unitSystem),
    }));
    onDone(inputs, {
      trayCount: liveGroups.reduce((s, g) => s + g.trayCount, 0),
      varietyCount: liveGroups.length,
      cropName: liveGroups.length === 1 ? liveGroups[0].cropName : null,
      crops: liveGroups.map((g) => ({ cropId: g.cropId, cropName: g.cropName })),
    });
  }

  function openDiscard() {
    if (!current) return;
    setDiscardQty(1);
    setMode("discard");
  }

  async function confirmDiscard() {
    setPadError(null);
    if (!current) return;
    const cropName = current.cropName;
    const qty = discardQty;
    try {
      const remaining = await discardFromGroup(current.trayIds, qty);
      onDiscarded({ trayCount: qty, cropName });

      if (remaining) {
        const nextGroups = [...liveGroups];
        nextGroups[step] = remaining;
        const nextValues = [...values];
        const pref = massFigure(remaining.estimatedYieldOz, unitSystem);
        nextValues[step] = pref;
        setLiveGroups(nextGroups);
        setValues(nextValues);
        setDisplay(pref);
        setReplaced(false);
        setMode("weight");
        return;
      }

      // Whole crop discarded — drop from sequence.
      const nextGroups = liveGroups.filter((_, i) => i !== step);
      const nextValues = values.filter((_, i) => i !== step);
      if (nextGroups.length === 0) {
        setLiveGroups([]);
        setValues([]);
        setMode("weight");
        handleOpenChange(false);
        return;
      }
      const nextStep = Math.min(step, nextGroups.length - 1);
      setLiveGroups(nextGroups);
      setValues(nextValues);
      loadStep(nextStep, nextValues, nextGroups);
    } catch (err) {
      // TODO(stage-3:attention): failed writes and divergence become Attention items.
      setPadError(err instanceof Error ? err.message : String(err));
      console.error(err);
    }
  }

  const keys = ["1", "2", "3", "4", "5", "6", "7", "8", "9", ".", "0", "⌫"] as const;
  const valid = parsedValue() !== null;
  const isLast = step >= count - 1;
  const primaryLabel = count === 1 ? "Confirm" : isLast ? "Done" : "Next";

  const header =
    current == null
      ? ""
      : count > 1
        ? `${current.cropName} — ${trayWord(current.trayCount)} · ${step + 1} of ${count}`
        : `${current.cropName} · ${trayWord(current.trayCount)}`;

  return (
    <>
      <Sheet open={open} onOpenChange={handleOpenChange}>
        <SheetContent
          side="bottom"
          showCloseButton={false}
          aria-describedby={undefined}
        >
          <div className="mx-auto flex w-full max-w-md flex-col gap-6 p-4 pb-8">
            {mode === "discard" && current ? (
              <>
                <div>
                  <Button
                    type="button"
                    variant="ghost"
                    onClick={() => setMode("weight")}
                    className="h-14 -ml-2 px-3 text-base text-muted-foreground"
                  >
                    ← Back
                  </Button>
                </div>

                <SheetTitle className="text-center text-xl font-semibold">
                  {current.cropName} — {trayWord(current.trayCount)}
                </SheetTitle>

                <p className="text-center text-base text-muted-foreground">
                  How many failed?
                </p>

                <div className="flex items-center justify-center gap-6">
                  <button
                    type="button"
                    aria-label="Fewer trays"
                    onClick={() => setDiscardQty((q) => Math.max(1, q - 1))}
                    disabled={discardQty <= 1}
                    className="flex size-14 items-center justify-center rounded-full border text-2xl leading-none transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:opacity-40"
                  >
                    −
                  </button>
                  <span className="w-12 text-center text-3xl font-semibold tabular-nums">
                    {discardQty}
                  </span>
                  <button
                    type="button"
                    aria-label="More trays"
                    onClick={() =>
                      setDiscardQty((q) => Math.min(current.trayCount, q + 1))
                    }
                    disabled={discardQty >= current.trayCount}
                    className="flex size-14 items-center justify-center rounded-full border text-2xl leading-none transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:opacity-40"
                  >
                    +
                  </button>
                </div>

                <ErrorLine message={padError} />
                <Button
                  type="button"
                  onClick={confirmDiscard}
                  className="h-14 w-full text-lg"
                >
                  Discard
                </Button>
              </>
            ) : (
              <>
                {count > 1 && step > 0 && (
                  <div>
                    <Button
                      type="button"
                      variant="ghost"
                      onClick={goBack}
                      className="h-14 -ml-2 px-3 text-base text-muted-foreground"
                    >
                      ← Back
                    </Button>
                  </div>
                )}

                <div className="relative">
                  <SheetTitle className="px-14 text-center text-xl font-semibold">
                    {header}
                  </SheetTitle>
                  {current && (
                    <Button
                      type="button"
                      variant="ghost"
                      onClick={openDiscard}
                      className="absolute top-1/2 right-0 min-h-11 min-w-11 -translate-y-1/2 px-3 text-sm font-normal text-muted-foreground"
                    >
                      Discard
                    </Button>
                  )}
                </div>

                {commitments && (
                  <div className="flex flex-col gap-1 text-left">
                    <p className="text-sm font-medium">{commitments.header}</p>
                    {commitments.lines.length === 0 ? (
                      <p className="text-sm text-muted-foreground">{commitments.emptyLine}</p>
                    ) : (
                      commitments.lines.map((l) => (
                        <p key={`${l.kind}-${l.id}`} className="text-sm text-muted-foreground">
                          {l.text}
                        </p>
                      ))
                    )}
                  </div>
                )}

                <div
                  className="text-center text-5xl font-semibold tabular-nums tracking-tight"
                  aria-live="polite"
                >
                  {display || "0"}
                </div>
                <p className="text-center text-sm text-muted-foreground">
                  {replaced
                    ? unitWord(unitSystem)
                    : `estimate — type what the scale says (${unitWord(unitSystem)})`}
                </p>

                <div className="grid grid-cols-3 gap-3">
                  {keys.map((key) => (
                    <button
                      key={key}
                      type="button"
                      onClick={() => {
                        if (key === "⌫") backspace();
                        else pressDigit(key);
                      }}
                      className="flex min-h-14 items-center justify-center rounded-xl border text-2xl font-medium transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      {key}
                    </button>
                  ))}
                </div>

                <Button
                  type="button"
                  onClick={() => {
                    if (isLast) goDone();
                    else goNext();
                  }}
                  disabled={!valid}
                  className="h-14 w-full text-lg"
                >
                  {primaryLabel}
                </Button>

                <button
                  type="button"
                  onClick={() => setCostOpen(true)}
                  className="flex min-h-11 w-full items-center justify-center rounded-xl border px-4 text-sm font-medium transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  Money just left
                </button>
              </>
            )}
          </div>
        </SheetContent>
      </Sheet>

      <MoneyJustLeftSheet
        open={costOpen}
        onOpenChange={setCostOpen}
        moment="harvest"
        stacked
      />
    </>
  );
}
