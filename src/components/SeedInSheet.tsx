import { useState } from "react";
import type { Crop, SeedReceiptView } from "@/farm/types";
import { recordSeedReceived } from "@/farm/api";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";

type SeedInSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  crops: Crop[];
  onRecorded: (receipt: SeedReceiptView) => void;
};

/**
 * SEED-A (GT-D25) — the one seed-in door: pick a crop, type the ounces that
 * arrived, Record. Writes seed.received through recordSeedReceived and
 * nothing else: no rate, no tray, no dollars, no jar total. A blank or
 * non-numeric field reaches the door as 0 exactly as the leftover door does
 * (Today.tsx handleListLeftover) and the door refuses it with its own
 * sentence; nothing here pre-empts or rewrites those sentences.
 */
export function SeedInSheet({
  open,
  onOpenChange,
  crops,
  onRecorded,
}: SeedInSheetProps) {
  const [selectedCrop, setSelectedCrop] = useState<Crop | null>(null);
  const [oz, setOz] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setSelectedCrop(null);
    setOz("");
    setError(null);
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function pickCrop(crop: Crop) {
    setSelectedCrop(crop);
    setOz("");
    setError(null);
  }

  async function handleRecord() {
    if (!selectedCrop || busy) return;
    setBusy(true);
    setError(null);
    try {
      const parsed = Number.parseFloat(oz);
      const receipt = await recordSeedReceived({
        cropId: selectedCrop.id,
        receivedOz: Number.isFinite(parsed) ? parsed : 0,
      });
      reset();
      onRecorded(receipt);
      onOpenChange(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet open={open} onOpenChange={handleOpenChange}>
      <SheetContent
        side="bottom"
        aria-describedby={undefined}
      >
        <div className="mx-auto flex w-full max-w-md flex-col gap-6 p-4 pb-8">
          {selectedCrop === null ? (
            <>
              <SheetTitle className="text-center text-xl font-semibold">
                Seed in
              </SheetTitle>
              <p className="text-center text-base text-muted-foreground">
                Seed that arrived. Pick the crop, then weigh the bag.
              </p>
              <div className="grid grid-cols-2 gap-3">
                {crops.map((crop) => (
                  <button
                    key={crop.id}
                    type="button"
                    onClick={() => pickCrop(crop)}
                    className="flex min-h-24 flex-col items-start justify-center gap-1 rounded-xl border bg-card p-4 text-left transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  >
                    <span className="text-lg font-medium leading-tight">
                      {crop.name}
                    </span>
                  </button>
                ))}
              </div>
            </>
          ) : (
            <>
              <SheetTitle className="sr-only">
                Record seed in for {selectedCrop.name}
              </SheetTitle>
              <div>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => {
                    setSelectedCrop(null);
                    setError(null);
                  }}
                  className="h-14 -ml-2 px-3 text-base text-muted-foreground"
                >
                  ← Back
                </Button>
              </div>
              <h2 className="text-2xl font-semibold">{selectedCrop.name}</h2>
              <label className="flex flex-col gap-2">
                <span className="text-sm text-muted-foreground">
                  Seed received (oz)
                </span>
                <input
                  type="text"
                  inputMode="decimal"
                  value={oz}
                  onChange={(e) => {
                    setError(null);
                    setOz(e.target.value);
                  }}
                  placeholder="Weigh and enter"
                  aria-label="Seed received ounces"
                  className="h-14 w-full rounded-xl border bg-card px-4 text-lg tabular-nums focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                />
              </label>
              {error && (
                <p className="text-center text-sm text-destructive">{error}</p>
              )}
              <Button
                type="button"
                onClick={() => void handleRecord()}
                disabled={busy}
                className="h-14 w-full text-lg"
              >
                Record
              </Button>
            </>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
