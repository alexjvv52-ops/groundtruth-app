import { useEffect, useMemo, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { CostCategory } from "@/farm/types";
import {
  listCostCategories,
  receiptSourceInfo,
  recordCost,
} from "@/farm/api";
import { localToday } from "@/farm/dates";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";

/** UI-only moment hint. Never reaches RecordCostInput / payload / DB. */
export type CostMoment =
  | "money_just_left"
  | "sow"
  | "harvest"
  | "delivery";

/** Real category ids from categories.rs — closed capital, reference only. */
const MOMENT_SHORTLISTS: Record<CostMoment, readonly string[]> = {
  money_just_left: [],
  sow: ["growing_medium", "seed", "trays_domes_racks"],
  harvest: ["packaging_labels"],
  // tolls has no canonical category — shortlist is fuel only (reported).
  delivery: ["delivery_fuel"],
};

type MoneyJustLeftSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onRecorded?: () => void;
  moment?: CostMoment;
  /** Stack above an already-open sheet (sow / harvest). */
  stacked?: boolean;
};

type PickedReceipt = {
  path: string;
  fileName: string;
  sizeBytes: number;
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

/** Parse operator dollars into positive integer cents. */
function parseDollarsToCents(raw: string): number | null {
  const cleaned = raw.trim().replace(/[^0-9.]/g, "");
  if (!cleaned) return null;
  const parts = cleaned.split(".");
  if (parts.length > 2) return null;
  const dollars = Number(parts[0] || "0");
  if (!Number.isFinite(dollars) || dollars < 0) return null;
  let centsPart = parts[1] ?? "";
  if (centsPart.length > 2) centsPart = centsPart.slice(0, 2);
  const cents = Number((centsPart + "00").slice(0, 2));
  if (!Number.isFinite(cents)) return null;
  const total = dollars * 100 + cents;
  if (!Number.isInteger(total) || total <= 0) return null;
  return total;
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function orderCategories(
  categories: CostCategory[],
  moment: CostMoment,
): CostCategory[] {
  const short = MOMENT_SHORTLISTS[moment];
  if (short.length === 0) return categories;
  const byId = new Map(categories.map((c) => [c.id, c]));
  const head: CostCategory[] = [];
  for (const id of short) {
    const c = byId.get(id);
    if (c) head.push(c);
  }
  const headIds = new Set(head.map((c) => c.id));
  const rest = categories.filter((c) => !headIds.has(c.id));
  return [...head, ...rest];
}

export function MoneyJustLeftSheet({
  open,
  onOpenChange,
  onRecorded,
  moment = "money_just_left",
  stacked = false,
}: MoneyJustLeftSheetProps) {
  const [categories, setCategories] = useState<CostCategory[]>([]);
  const [amount, setAmount] = useState("");
  const [payee, setPayee] = useState("");
  const [categoryId, setCategoryId] = useState<string | null>(null);
  const [datePaid, setDatePaid] = useState(toYyyyMmDd(localToday()));
  const [descriptor, setDescriptor] = useState("");
  const [receipt, setReceipt] = useState<PickedReceipt | null>(null);
  const [showAllCategories, setShowAllCategories] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const shortlist = MOMENT_SHORTLISTS[moment];
  const ordered = useMemo(
    () => orderCategories(categories, moment),
    [categories, moment],
  );
  const visibleCategories =
    shortlist.length === 0 || showAllCategories
      ? ordered
      : ordered.filter((c) => shortlist.includes(c.id));

  const selected = categories.find((c) => c.id === categoryId) ?? null;
  const needsDescriptor = selected?.descriptorRequired ?? false;

  useEffect(() => {
    if (!open) return;
    void listCostCategories()
      .then(setCategories)
      .catch((err) => setError(errMessage(err)));
  }, [open]);

  function reset() {
    setAmount("");
    setPayee("");
    setCategoryId(null);
    setDatePaid(toYyyyMmDd(localToday()));
    setDescriptor("");
    setReceipt(null);
    setShowAllCategories(false);
    setError(null);
    setSaving(false);
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  async function handleAttachReceipt() {
    setError(null);
    try {
      const selectedPath = await openDialog({
        multiple: false,
        filters: [
          {
            name: "Receipt",
            extensions: ["jpg", "jpeg", "png", "webp", "gif", "pdf", "heic"],
          },
        ],
      });
      if (selectedPath === null) return;
      const path = Array.isArray(selectedPath)
        ? selectedPath[0]
        : selectedPath;
      if (!path) return;
      const info = await receiptSourceInfo(path);
      setReceipt({
        path,
        fileName: info.fileName,
        sizeBytes: info.sizeBytes,
      });
    } catch (err) {
      setError(errMessage(err));
    }
  }

  async function handleSave() {
    setError(null);
    const cents = parseDollarsToCents(amount);
    if (cents === null) {
      setError("Enter how much left.");
      return;
    }
    if (!payee.trim()) {
      setError("Who did you pay?");
      return;
    }
    if (!categoryId) {
      setError("Pick what it was for.");
      return;
    }
    if (needsDescriptor && !descriptor.trim()) {
      setError("Add a short note for this one.");
      return;
    }
    setSaving(true);
    try {
      await recordCost({
        amountCents: cents,
        payee: payee.trim(),
        categoryId,
        datePaid,
        descriptor: needsDescriptor ? descriptor.trim() : null,
        receiptSourcePath: receipt?.path ?? null,
      });
      onRecorded?.();
      reset();
      onOpenChange(false);
    } catch (err) {
      setError(errMessage(err));
      setSaving(false);
    }
  }

  return (
    <Sheet open={open} onOpenChange={handleOpenChange} modal>
      <SheetContent
        side="bottom"
        showCloseButton={false}
        stacked={stacked}
        aria-describedby={undefined}
        className="max-h-[90vh] overflow-y-auto"
      >
        <div className="mx-auto flex w-full max-w-md flex-col gap-5 p-4 pb-8">
          <SheetTitle className="text-xl font-semibold tracking-tight">
            Money just left
          </SheetTitle>

          <label className="flex flex-col gap-1.5">
            <span className="text-sm text-muted-foreground">Amount</span>
            <input
              type="text"
              inputMode="decimal"
              autoComplete="off"
              placeholder="0.00"
              value={amount}
              onChange={(e) => setAmount(e.target.value)}
              className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            />
          </label>

          <label className="flex flex-col gap-1.5">
            <span className="text-sm text-muted-foreground">Paid to</span>
            <input
              type="text"
              autoComplete="organization"
              value={payee}
              onChange={(e) => setPayee(e.target.value)}
              className="h-12 rounded-xl border bg-card px-4 text-base focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            />
          </label>

          <fieldset className="flex flex-col gap-2">
            <legend className="text-sm text-muted-foreground">What for</legend>
            <div className="grid max-h-48 grid-cols-1 gap-2 overflow-y-auto">
              {visibleCategories.map((c) => (
                <button
                  key={c.id}
                  type="button"
                  onClick={() => setCategoryId(c.id)}
                  className={
                    categoryId === c.id
                      ? "min-h-11 rounded-xl border border-foreground bg-accent px-4 py-2 text-left text-base font-medium"
                      : "min-h-11 rounded-xl border bg-card px-4 py-2 text-left text-base transition-colors hover:bg-accent"
                  }
                >
                  {c.name}
                </button>
              ))}
            </div>
            {shortlist.length > 0 && !showAllCategories && (
              <button
                type="button"
                onClick={() => setShowAllCategories(true)}
                className="min-h-11 text-left text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                Show all
              </button>
            )}
          </fieldset>

          <label className="flex flex-col gap-1.5">
            <span className="text-sm text-muted-foreground">Date paid</span>
            <input
              type="date"
              value={datePaid}
              max={toYyyyMmDd(localToday())}
              onChange={(e) => setDatePaid(e.target.value)}
              className="h-12 rounded-xl border bg-card px-4 text-base focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            />
          </label>

          {needsDescriptor && (
            <label className="flex flex-col gap-1.5">
              <span className="text-sm text-muted-foreground">
                Short note (required)
              </span>
              <input
                type="text"
                value={descriptor}
                onChange={(e) => setDescriptor(e.target.value)}
                className="h-12 rounded-xl border bg-card px-4 text-base focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              />
            </label>
          )}

          <div className="flex flex-col gap-2">
            {receipt ? (
              <div className="flex items-center justify-between gap-3 rounded-xl border px-4 py-3">
                <div className="min-w-0">
                  <p className="truncate text-base">{receipt.fileName}</p>
                  <p className="text-sm text-muted-foreground">
                    {formatSize(receipt.sizeBytes)}
                  </p>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  className="h-11 shrink-0 text-sm"
                  onClick={() => setReceipt(null)}
                  disabled={saving}
                >
                  Remove
                </Button>
              </div>
            ) : (
              <button
                type="button"
                onClick={() => void handleAttachReceipt()}
                disabled={saving}
                className="flex min-h-11 items-center text-left text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:opacity-50"
              >
                Attach receipt
              </button>
            )}
          </div>

          {error && (
            <p className="text-sm text-destructive" role="alert">
              {error}
            </p>
          )}

          <div className="flex gap-3">
            <Button
              type="button"
              variant="ghost"
              className="h-12 flex-1 text-base"
              onClick={() => handleOpenChange(false)}
              disabled={saving}
            >
              Cancel
            </Button>
            <Button
              type="button"
              className="h-12 flex-1 text-base"
              onClick={() => void handleSave()}
              disabled={saving}
            >
              {saving ? "Saving…" : "Save"}
            </Button>
          </div>
        </div>
      </SheetContent>
    </Sheet>
  );
}
