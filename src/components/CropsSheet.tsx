import { useEffect, useState } from "react";
import type { Crop } from "@/farm/types";
import {
  addCrop,
  listCrops,
  renameCrop,
  updateCropGrowthDays,
  updateCropSeedRate,
} from "@/farm/api";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import { grams, perTrayUnit } from "@/farm/mass";
import { typedOunces } from "@/farm/typed";

type CropsSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved?: () => void;
  /** GT-D27 UNITS (J2) - the desk's display system, for the rate column only.
   *  Both editor fields are typed in the desk's units and converted to ounces before the command. */
  unitSystem: string;
};

type Draft =
  | {
      mode: "add";
      name: string;
      growthDays: string;
      blackoutDays: string;
      expectedYieldOz: string;
    }
  | {
      mode: "edit";
      crop: Crop;
      name: string;
      growthDays: string;
      blackoutDays: string;
      seedRate: string;
      /** UNITS J6-R6 REOPEN A - true once the operator has typed in the field. */
      seedRateDirty: boolean;
    };

function errMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return "Could not save. Try again.";
}

function formatRate(rate: number | null, system: string): string {
  if (rate == null) return "—";
  const figure = system === "metric" ? String(grams(rate)) : String(rate);
  return `${figure} ${perTrayUnit(system)}`;
}

/** UNITS J6-FOLD, FOLD B. Blank → null. Non-blank → typedOunces (Rust owns validation). */
export function rateFieldToValue(raw: string, system: string): number | null {
  const trimmed = raw.trim();
  if (trimmed === "") return null;
  return typedOunces(trimmed, system);
}

const FIELD =
  "h-14 w-full rounded-xl border bg-card px-4 text-base focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";

export function CropsSheet({
  open,
  onOpenChange,
  onSaved,
  unitSystem,
}: CropsSheetProps) {
  const [crops, setCrops] = useState<Crop[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Settings fence 1 (S2): the shelf ceiling ("Space") left this sheet. Its
  // single editor is Settings › Farm — Shelf space. This sheet is crops only.
  async function load() {
    try {
      const rows = await listCrops();
      setCrops(rows);
    } catch (err) {
      console.error(err);
    }
  }

  useEffect(() => {
    if (!open) return;
    setDraft(null);
    setError(null);
    void load();
  }, [open]);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setDraft(null);
      setError(null);
    }
    onOpenChange(next);
  }

  function startAdd() {
    setDraft({
      mode: "add",
      name: "",
      growthDays: "",
      blackoutDays: "",
      expectedYieldOz: "",
    });
    setError(null);
  }

  function startEdit(crop: Crop) {
    setDraft({
      mode: "edit",
      crop,
      name: crop.name,
      growthDays: String(crop.growthDays),
      blackoutDays: String(crop.blackoutDays),
      seedRate:
        crop.seedRateOzPerTray == null
          ? ""
          : unitSystem === "metric"
            ? String(grams(crop.seedRateOzPerTray))
            : String(crop.seedRateOzPerTray),
      seedRateDirty: false,
    });
    setError(null);
  }

  async function handleSave() {
    if (!draft || saving) return;
    setSaving(true);
    setError(null);
    try {
      if (draft.mode === "add") {
        await addCrop(
          draft.name,
          Number(draft.growthDays),
          Number(draft.blackoutDays),
          typedOunces(draft.expectedYieldOz, unitSystem),
        );
        await load();
        setDraft(null);
        onSaved?.();
        return;
      }

      const { crop } = draft;
      if (draft.name !== crop.name) {
        await renameCrop(crop.id, draft.name);
        await load();
      }
      if (
        Number(draft.growthDays) !== crop.growthDays ||
        Number(draft.blackoutDays) !== crop.blackoutDays
      ) {
        await updateCropGrowthDays(
          crop.id,
          Number(draft.growthDays),
          Number(draft.blackoutDays),
        );
        await load();
      }
      // UNITS J6-R6 REOPEN A - an untouched field is never parsed and never
      // sent. The metric prefill is String(grams(oz)); typedOunces(that) is
      // not the stored ounce, so only a keystroke earns update_crop_seed_rate.
      // INPUT A stands inside the branch: a typed gram converts as typed.
      if (draft.seedRateDirty) {
        const value = rateFieldToValue(draft.seedRate, unitSystem);
        if (value !== null && !Number.isFinite(value)) {
          setError("seed_rate_oz_per_tray must be a finite number");
          return;
        }
        if (value !== crop.seedRateOzPerTray) {
          await updateCropSeedRate(crop.id, value);
          await load();
        }
      }
      setDraft(null);
      onSaved?.();
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Sheet open={open} onOpenChange={handleOpenChange}>
      <SheetContent
        side="bottom"
        aria-describedby={undefined}
      >
        <div className="mx-auto flex w-full max-w-md flex-col gap-6 p-4 pb-8">
          {draft ? (
            <>
              <div>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => {
                    setDraft(null);
                    setError(null);
                  }}
                  className="h-14 -ml-2 px-3 text-base text-muted-foreground"
                >
                  ← Back
                </Button>
              </div>
              <SheetTitle className="text-center text-xl font-semibold">
                {draft.mode === "add" ? "Add a crop" : draft.crop.name}
              </SheetTitle>
              <label className="flex flex-col gap-2">
                <span className="text-sm text-muted-foreground">Name</span>
                <input
                  type="text"
                  value={draft.name}
                  onChange={(e) =>
                    setDraft({ ...draft, name: e.target.value })
                  }
                  aria-label="Crop name"
                  className={FIELD}
                />
              </label>
              <label className="flex flex-col gap-2">
                <span className="text-sm text-muted-foreground">
                  Growth days
                </span>
                <input
                  type="text"
                  inputMode="numeric"
                  value={draft.growthDays}
                  onChange={(e) =>
                    setDraft({ ...draft, growthDays: e.target.value })
                  }
                  aria-label="Growth days"
                  className={`${FIELD} text-center text-2xl tabular-nums`}
                />
              </label>
              <label className="flex flex-col gap-2">
                <span className="text-sm text-muted-foreground">
                  Blackout days
                </span>
                <input
                  type="text"
                  inputMode="numeric"
                  value={draft.blackoutDays}
                  onChange={(e) =>
                    setDraft({ ...draft, blackoutDays: e.target.value })
                  }
                  aria-label="Blackout days"
                  className={`${FIELD} text-center text-2xl tabular-nums`}
                />
              </label>
              {draft.mode === "add" ? (
                <label className="flex flex-col gap-2">
                  <span className="text-sm text-muted-foreground">
                    {`Expected yield ${perTrayUnit(unitSystem)}`}
                  </span>
                  <input
                    type="text"
                    inputMode="decimal"
                    value={draft.expectedYieldOz}
                    onChange={(e) =>
                      setDraft({ ...draft, expectedYieldOz: e.target.value })
                    }
                    aria-label={`Expected yield ${perTrayUnit(unitSystem)}`}
                    className={`${FIELD} text-center text-2xl tabular-nums`}
                  />
                </label>
              ) : (
                <label className="flex flex-col gap-2">
                  <span className="text-sm text-muted-foreground">
                    {`Seed rate (${perTrayUnit(unitSystem)}). Leave blank for no proposal.`}
                  </span>
                  <input
                    type="text"
                    inputMode="decimal"
                    value={draft.seedRate}
                    onChange={(e) =>
                      setDraft({
                        ...draft,
                        seedRate: e.target.value,
                        seedRateDirty: true,
                      })
                    }
                    aria-label={`Seed rate ${perTrayUnit(unitSystem)}`}
                    className={`${FIELD} text-center text-2xl tabular-nums`}
                  />
                </label>
              )}
              <p className="text-center text-sm text-muted-foreground">
                Growth days are an estimate. Trays already sown keep the days they were
                sown with.
              </p>
              {error && (
                <p className="text-center text-sm text-destructive">{error}</p>
              )}
              <Button
                type="button"
                onClick={() => void handleSave()}
                disabled={saving}
                className="h-14 w-full text-lg"
              >
                Save
              </Button>
            </>
          ) : (
            <>
              <SheetTitle className="text-center text-xl font-semibold">
                Crops
              </SheetTitle>
              <p className="text-center text-base text-muted-foreground">
                Name, growth days and seed rate. Growth days are an estimate.
              </p>
              <Button
                type="button"
                variant="outline"
                onClick={startAdd}
                className="h-14 w-full text-base"
              >
                Add a crop
              </Button>
              <ul className="flex flex-col gap-2">
                {crops.map((crop) => (
                  <li key={crop.id}>
                    <button
                      type="button"
                      onClick={() => startEdit(crop)}
                      className="flex min-h-14 w-full items-center justify-between gap-4 rounded-xl border px-4 py-3 text-left text-base transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      <span className="flex flex-col">
                        <span className="font-medium">{crop.name}</span>
                        <span className="text-sm text-muted-foreground">
                          {crop.growthDays}d growth · {crop.blackoutDays}d
                          blackout
                        </span>
                      </span>
                      <span className="tabular-nums text-muted-foreground">
                        {formatRate(crop.seedRateOzPerTray, unitSystem)}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
