import { useEffect, useState } from "react";
import type { RecountCrop, RecountEntry, RecountResult } from "@/farm/types";
import { applyRecount, recountState } from "@/farm/api";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";

type RecountSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDone: (result: RecountResult, cropCount: number) => void;
};

function trayWord(n: number): string {
  return `${n} ${n === 1 ? "tray" : "trays"}`;
}

export function RecountSheet({ open, onOpenChange, onDone }: RecountSheetProps) {
  const [crops, setCrops] = useState<RecountCrop[]>([]);
  const [counts, setCounts] = useState<number[]>([]);
  const [step, setStep] = useState(0);
  const [submitting, setSubmitting] = useState(false);

  const count = crops.length;
  const current = crops[step];

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    (async () => {
      try {
        const rows = await recountState();
        if (cancelled) return;
        setCrops(rows);
        setCounts(rows.map((r) => r.appQuantity));
        setStep(0);
      } catch (err) {
        console.error(err);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open]);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setCrops([]);
      setCounts([]);
      setStep(0);
    }
    onOpenChange(next);
  }

  function setCount(n: number) {
    setCounts((prev) => {
      const next = [...prev];
      next[step] = Math.max(0, n);
      return next;
    });
  }

  function goBack() {
    if (step > 0) setStep(step - 1);
  }

  async function goNextOrDone() {
    if (step + 1 < count) {
      setStep(step + 1);
      return;
    }
    if (submitting || crops.length === 0) return;
    setSubmitting(true);
    try {
      const entries: RecountEntry[] = crops.map((c, i) => ({
        cropId: c.cropId,
        countedQuantity: counts[i] ?? c.appQuantity,
      }));
      const result = await applyRecount(entries);
      onOpenChange(false);
      onDone(result, crops.length);
    } catch (err) {
      console.error(err);
    } finally {
      setSubmitting(false);
    }
  }

  const isLast = count === 0 || step >= count - 1;
  const primaryLabel = isLast ? "Done" : "Next";
  const counted = current ? (counts[step] ?? current.appQuantity) : 0;

  const header =
    current == null
      ? "Count the shelf"
      : count > 1
        ? `${current.cropName} · ${step + 1} of ${count}`
        : current.cropName;

  return (
    <Sheet open={open} onOpenChange={handleOpenChange}>
      <SheetContent
        side="bottom"
        showCloseButton={false}
        aria-describedby={undefined}
      >
        <div className="mx-auto flex w-full max-w-md flex-col gap-6 p-4 pb-8">
          {step > 0 && (
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

          <SheetTitle className="text-center text-xl font-semibold">
            {header}
          </SheetTitle>

          {current && (
            <>
              <p className="text-center text-base text-muted-foreground">
                The app says {trayWord(current.appQuantity)}.
              </p>
              <p className="text-center text-base">How many are on the shelf?</p>

              <div className="flex items-center justify-center gap-6">
                <button
                  type="button"
                  aria-label="Fewer trays"
                  onClick={() => setCount(counted - 1)}
                  disabled={counted <= 0}
                  className="flex size-14 items-center justify-center rounded-full border text-2xl leading-none transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:opacity-40"
                >
                  −
                </button>
                <span className="w-12 text-center text-3xl font-semibold tabular-nums">
                  {counted}
                </span>
                <button
                  type="button"
                  aria-label="More trays"
                  onClick={() => setCount(counted + 1)}
                  className="flex size-14 items-center justify-center rounded-full border text-2xl leading-none transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  +
                </button>
              </div>

              <Button
                type="button"
                onClick={() => void goNextOrDone()}
                disabled={submitting}
                className="h-14 w-full text-lg"
              >
                {primaryLabel}
              </Button>
            </>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}

/** Build the lastAction line for a recount result. */
export function recountResultMessage(
  result: RecountResult,
  cropCount: number,
): { matched: boolean; text: string } {
  const down = result.adjustedDown;
  const up = result.adjustedUp;
  if (down.length === 0 && up.length === 0) {
    return {
      matched: true,
      text: `Counted ${cropCount} ${cropCount === 1 ? "crop" : "crops"}. Everything matched.`,
    };
  }
  const parts: string[] = [];
  for (const c of down) {
    parts.push(
      `${trayWord(c.quantity)} of ${c.cropName} removed`,
    );
  }
  for (const c of up) {
    parts.push(`${trayWord(c.quantity)} of ${c.cropName} added`);
  }
  return { matched: false, text: `Recount: ${parts.join(", ")}.` };
}
