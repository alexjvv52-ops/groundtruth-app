import { useEffect, useMemo, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { IncomeCategory, IncomeRecord } from "@/farm/types";
import {
  correctIncome,
  duplicateIncomeWarning,
  listIncome,
  listIncomeCategories,
  receiptSourceInfo,
  recordIncome,
  voidIncome,
} from "@/farm/api";
import { formatCents, parseDollarsToCents } from "@/farm/dollars";
import { localToday } from "@/farm/dates";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";

type MoneyCameInSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onRecorded?: () => void;
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

function formatReceivedDate(yyyyMmDd: string): string {
  const [y, m, d] = yyyyMmDd.split("-").map(Number);
  if (!y || !m || !d) return yyyyMmDd;
  return new Date(y, m - 1, d).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function MoneyCameInSheet({
  open,
  onOpenChange,
  onRecorded,
}: MoneyCameInSheetProps) {
  const [categories, setCategories] = useState<IncomeCategory[]>([]);
  const [records, setRecords] = useState<IncomeRecord[]>([]);
  const [editing, setEditing] = useState<IncomeRecord | null>(null);
  const [amount, setAmount] = useState("");
  const [source, setSource] = useState("");
  const [categoryId, setCategoryId] = useState<string | null>(null);
  const [dateReceived, setDateReceived] = useState(toYyyyMmDd(localToday()));
  const [descriptor, setDescriptor] = useState("");
  const [receipt, setReceipt] = useState<PickedReceipt | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingDuplicate, setPendingDuplicate] = useState<string | null>(null);
  const today = toYyyyMmDd(localToday());

  const selected = categories.find((c) => c.id === categoryId) ?? null;
  const needsDescriptor = selected?.descriptorRequired ?? false;

  const listTotalCents = useMemo(
    () => records.reduce((sum, r) => sum + r.amountCents, 0),
    [records],
  );

  async function load() {
    try {
      const [cats, rows] = await Promise.all([
        listIncomeCategories(),
        listIncome(),
      ]);
      setCategories(cats);
      setRecords(rows);
    } catch (err) {
      setError(errMessage(err));
    }
  }

  function resetForm() {
    setAmount("");
    setSource("");
    setCategoryId(null);
    setDateReceived(toYyyyMmDd(localToday()));
    setDescriptor("");
    setReceipt(null);
    setError(null);
    setSaving(false);
    setPendingDuplicate(null);
  }

  useEffect(() => {
    if (!open) return;
    setEditing(null);
    resetForm();
    void load();
  }, [open]);

  useEffect(() => {
    setPendingDuplicate(null);
  }, [amount, source, dateReceived]);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setEditing(null);
      resetForm();
    }
    onOpenChange(next);
  }

  function startEdit(row: IncomeRecord) {
    setEditing(row);
    setAmount((row.amountCents / 100).toFixed(2));
    setSource(row.source);
    setCategoryId(row.canonicalCategory);
    setDateReceived(row.dateReceived);
    setDescriptor(row.descriptor);
    setReceipt(null);
    setError(null);
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

  async function handleSaveNew() {
    if (saving) return;
    setError(null);
    const cents = parseDollarsToCents(amount);
    if (cents === null) {
      setError("Enter how much came in.");
      return;
    }
    if (!source.trim()) {
      setError("Who did it come from?");
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
    if (pendingDuplicate === null) {
      const warning = await duplicateIncomeWarning(
        source.trim(),
        cents,
        dateReceived,
      );
      if (warning !== null) {
        setPendingDuplicate(warning);
        return;
      }
    }
    setSaving(true);
    try {
      await recordIncome({
        amountCents: cents,
        source: source.trim(),
        categoryId,
        dateReceived,
        descriptor: needsDescriptor ? descriptor.trim() : null,
        receiptSourcePath: receipt?.path ?? null,
        duplicateAck: pendingDuplicate !== null,
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
    setError(null);
    const cents = parseDollarsToCents(amount);
    if (cents === null) {
      setError("Enter how much came in.");
      return;
    }
    if (!source.trim()) {
      setError("Who did it come from?");
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
      await correctIncome({
        incomeId: editing.incomeId,
        amountCents: cents,
        source: source.trim(),
        categoryId,
        dateReceived,
        descriptor: needsDescriptor ? descriptor.trim() : null,
        receiptSourcePath: receipt?.path ?? null,
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
      await voidIncome(editing.incomeId);
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

  const formFields = (
    <>
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
        <span className="text-sm text-muted-foreground">Received from</span>
        <input
          type="text"
          autoComplete="organization"
          value={source}
          onChange={(e) => setSource(e.target.value)}
          className="h-12 rounded-xl border bg-card px-4 text-base focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        />
      </label>

      <fieldset className="flex flex-col gap-2">
        <legend className="text-sm text-muted-foreground">What it was for</legend>
        <div className="grid max-h-48 grid-cols-1 gap-2 overflow-y-auto">
          {categories.map((c) => (
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
      </fieldset>

      <label className="flex flex-col gap-1.5">
        <span className="text-sm text-muted-foreground">Date received</span>
        <input
          type="date"
          value={dateReceived}
          max={today}
          onChange={(e) => setDateReceived(e.target.value)}
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
    </>
  );

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
              <SheetTitle className="text-xl font-semibold tracking-tight">
                Edit record
              </SheetTitle>

              {formFields}

              {error && (
                <p className="text-sm text-destructive" role="alert">
                  {error}
                </p>
              )}

              <Button
                type="button"
                onClick={() => void handleSaveEdit()}
                disabled={saving}
                className="h-12 w-full text-base"
              >
                {saving ? "Saving…" : "Save"}
              </Button>
              <Button
                type="button"
                variant="ghost"
                onClick={() => void handleRemove()}
                disabled={saving}
                className="h-12 w-full text-base text-muted-foreground"
              >
                Remove this record
              </Button>
            </>
          ) : (
            <>
              <SheetTitle className="text-xl font-semibold tracking-tight">
                Money came in
              </SheetTitle>

              <p className="text-sm text-muted-foreground" role="note">
                Online orders paid through Stripe are already recorded. Do not
                enter them here.
              </p>
              {/* B1-F2 (D16) — one steering line: a wholesale payment settles
                  the order (Collect on Today / Paid… on Money), so the debt
                  closes and the income row is linked, not orphaned. */}
              <p className="text-sm text-muted-foreground" role="note">
                Payment for a delivered wholesale order: use Collect on Today or
                Paid… on Money — not this sheet.
              </p>

              {formFields}

              {pendingDuplicate != null && (
                <p className="text-sm font-medium text-destructive">
                  {pendingDuplicate}
                </p>
              )}

              {error && (
                <p className="text-sm text-destructive" role="alert">
                  {error}
                </p>
              )}

              <Button
                type="button"
                onClick={() => void handleSaveNew()}
                disabled={saving}
                className="h-12 w-full text-base"
              >
                {saving
                  ? "Saving…"
                  : pendingDuplicate != null
                    ? "Record anyway"
                    : "Save"}
              </Button>

              <p className="text-sm text-muted-foreground">
                {records.length}{" "}
                {records.length === 1 ? "record" : "records"} ·{" "}
                {formatCents(listTotalCents)}
              </p>

              <ul className="flex flex-col gap-2">
                {records.map((row) => (
                  <li key={row.incomeId}>
                    <button
                      type="button"
                      onClick={() => startEdit(row)}
                      className="flex min-h-14 w-full items-center justify-between gap-3 rounded-xl border px-4 py-3 text-left text-base transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      <span className="min-w-0">
                        <span className="block text-sm text-muted-foreground">
                          {formatReceivedDate(row.dateReceived)}
                        </span>
                        <span className="block truncate font-medium">
                          {row.source}
                        </span>
                      </span>
                      <span className="shrink-0 tabular-nums">
                        {formatCents(row.amountCents)}
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
