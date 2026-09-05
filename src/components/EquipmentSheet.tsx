import { useEffect, useState } from "react";
import type { Asset } from "@/farm/types";
import { correctAsset, listAssets, recordAsset, voidAsset } from "@/farm/api";
import { formatCents, parseDollarsToCents } from "@/farm/dollars";
import { localToday } from "@/farm/dates";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";

type EquipmentSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onRecorded?: () => void;
};

function errMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return "Could not save. Try again.";
}

function toYyyyMmDd(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

function formatServiceDate(yyyyMmDd: string): string {
  const [y, m, d] = yyyyMmDd.split("-").map(Number);
  if (!y || !m || !d) return yyyyMmDd;
  return new Date(y, m - 1, d).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

export function EquipmentSheet({
  open,
  onOpenChange,
  onRecorded,
}: EquipmentSheetProps) {
  const [assets, setAssets] = useState<Asset[]>([]);
  const [editing, setEditing] = useState<Asset | null>(null);
  const [description, setDescription] = useState("");
  const [placedInServiceOn, setPlacedInServiceOn] = useState(
    toYyyyMmDd(localToday()),
  );
  const [cost, setCost] = useState("");
  const [disposalDate, setDisposalDate] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const today = toYyyyMmDd(localToday());

  async function load() {
    try {
      const rows = await listAssets();
      setAssets(rows);
    } catch (err) {
      setError(errMessage(err));
    }
  }

  function resetForm() {
    setDescription("");
    setPlacedInServiceOn(toYyyyMmDd(localToday()));
    setCost("");
    setDisposalDate("");
    setError(null);
    setSaving(false);
  }

  useEffect(() => {
    if (!open) return;
    setEditing(null);
    resetForm();
    void load();
  }, [open]);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setEditing(null);
      resetForm();
    }
    onOpenChange(next);
  }

  function startEdit(asset: Asset) {
    setEditing(asset);
    setDescription(asset.description);
    setPlacedInServiceOn(asset.placedInServiceOn);
    setCost((asset.costCents / 100).toFixed(2));
    setDisposalDate(asset.disposalDate ?? "");
    setError(null);
  }

  async function handleSaveNew() {
    if (saving) return;
    const costCents = parseDollarsToCents(cost);
    if (costCents === null) {
      setError("Enter a cost.");
      return;
    }
    if (!description.trim()) {
      setError("description is required");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await recordAsset({
        description: description.trim(),
        placedInServiceOn,
        costCents,
        disposalDate: null,
      });
      resetForm();
      await load();
      onRecorded?.();
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleSaveEdit() {
    if (!editing || saving) return;
    const costCents = parseDollarsToCents(cost);
    if (costCents === null) {
      setError("Enter a cost.");
      return;
    }
    if (!description.trim()) {
      setError("description is required");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await correctAsset({
        assetId: editing.assetId,
        description: description.trim(),
        placedInServiceOn,
        costCents,
        disposalDate: disposalDate.trim() || null,
      });
      setEditing(null);
      resetForm();
      await load();
      onRecorded?.();
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleRemove() {
    if (!editing || saving) return;
    setSaving(true);
    setError(null);
    try {
      await voidAsset(editing.assetId);
      setEditing(null);
      resetForm();
      await load();
      onRecorded?.();
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
        showCloseButton={false}
        aria-describedby={undefined}
        className="max-h-[90vh] overflow-y-auto"
      >
        <div className="mx-auto flex w-full max-w-md flex-col gap-5 p-4 pb-8">
          {editing ? (
            <>
              <div>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => {
                    setEditing(null);
                    resetForm();
                  }}
                  className="h-14 -ml-2 px-3 text-base text-muted-foreground"
                >
                  ← Back
                </Button>
              </div>
              <SheetTitle className="text-center text-xl font-semibold">
                Edit equipment
              </SheetTitle>

              <label className="flex flex-col gap-1.5">
                <span className="text-sm text-muted-foreground">
                  Description
                </span>
                <input
                  type="text"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                />
              </label>

              <label className="flex flex-col gap-1.5">
                <span className="text-sm text-muted-foreground">
                  Date placed in service
                </span>
                <input
                  type="date"
                  value={placedInServiceOn}
                  max={today}
                  onChange={(e) => setPlacedInServiceOn(e.target.value)}
                  className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                />
              </label>

              <label className="flex flex-col gap-1.5">
                <span className="text-sm text-muted-foreground">Cost</span>
                <input
                  type="text"
                  inputMode="decimal"
                  placeholder="0.00"
                  value={cost}
                  onChange={(e) => setCost(e.target.value)}
                  className="h-12 rounded-xl border bg-card px-4 text-lg tabular-nums focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                />
              </label>

              <label className="flex flex-col gap-1.5">
                <span className="text-sm text-muted-foreground">
                  Disposal date
                </span>
                <input
                  type="date"
                  value={disposalDate}
                  max={today}
                  onChange={(e) => setDisposalDate(e.target.value)}
                  className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                />
              </label>

              {error && (
                <p role="alert" className="text-center text-sm text-destructive">
                  {error}
                </p>
              )}

              <Button
                type="button"
                onClick={() => void handleSaveEdit()}
                disabled={saving}
                className="h-14 w-full text-lg"
              >
                Save
              </Button>
              <Button
                type="button"
                variant="ghost"
                onClick={() => void handleRemove()}
                disabled={saving}
                className="h-12 w-full text-base text-muted-foreground"
              >
                Remove this equipment
              </Button>
            </>
          ) : (
            <>
              <SheetTitle className="text-center text-xl font-semibold">
                Equipment
              </SheetTitle>

              <div className="flex flex-col gap-4">
                <p className="text-center text-base text-muted-foreground">
                  Add equipment
                </p>

                <label className="flex flex-col gap-1.5">
                  <span className="text-sm text-muted-foreground">
                    Description
                  </span>
                  <input
                    type="text"
                    value={description}
                    onChange={(e) => setDescription(e.target.value)}
                    className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  />
                </label>

                <label className="flex flex-col gap-1.5">
                  <span className="text-sm text-muted-foreground">
                    Date placed in service
                  </span>
                  <input
                    type="date"
                    value={placedInServiceOn}
                    max={today}
                    onChange={(e) => setPlacedInServiceOn(e.target.value)}
                    className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  />
                </label>

                <label className="flex flex-col gap-1.5">
                  <span className="text-sm text-muted-foreground">Cost</span>
                  <input
                    type="text"
                    inputMode="decimal"
                    placeholder="0.00"
                    value={cost}
                    onChange={(e) => setCost(e.target.value)}
                    className="h-12 rounded-xl border bg-card px-4 text-lg tabular-nums focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  />
                </label>

                {error && (
                  <p
                    role="alert"
                    className="text-center text-sm text-destructive"
                  >
                    {error}
                  </p>
                )}

                <Button
                  type="button"
                  onClick={() => void handleSaveNew()}
                  disabled={saving}
                  className="h-14 w-full text-lg"
                >
                  Save
                </Button>
              </div>

              <ul className="flex flex-col gap-2">
                {assets.map((asset) => (
                  <li key={asset.assetId}>
                    <button
                      type="button"
                      onClick={() => startEdit(asset)}
                      className="flex min-h-14 w-full flex-col gap-1 rounded-xl border px-4 py-3 text-left text-base transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      <span className="font-medium">{asset.description}</span>
                      <span className="flex justify-between gap-3 text-sm text-muted-foreground">
                        <span>{formatServiceDate(asset.placedInServiceOn)}</span>
                        <span className="tabular-nums">
                          {formatCents(asset.costCents)}
                        </span>
                        <span>
                          {asset.disposalDate
                            ? formatServiceDate(asset.disposalDate)
                            : "—"}
                        </span>
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
