import { useEffect, useState } from "react";
import type { MileageTrip } from "@/farm/types";
import {
  correctMileageTrip,
  listMileageTrips,
  recordMileageTrip,
  voidMileageTrip,
} from "@/farm/api";
import { localToday } from "@/farm/dates";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";

type MilesSheetProps = {
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
  return y + "-" + m + "-" + day;
}

function formatTripDate(yyyyMmDd: string): string {
  const [y, m, d] = yyyyMmDd.split("-").map(Number);
  if (!y || !m || !d) return yyyyMmDd;
  return new Date(y, m - 1, d).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

function parseMiles(raw: string): number | null {
  const cleaned = raw.trim();
  if (!cleaned) return null;
  const n = Number(cleaned);
  if (!Number.isFinite(n) || n <= 0) return null;
  return n;
}

export function MilesSheet({
  open,
  onOpenChange,
  onRecorded,
}: MilesSheetProps) {
  const [trips, setTrips] = useState<MileageTrip[]>([]);
  const [editing, setEditing] = useState<MileageTrip | null>(null);
  const [tripDate, setTripDate] = useState(toYyyyMmDd(localToday()));
  const [miles, setMiles] = useState("");
  const [purpose, setPurpose] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const today = toYyyyMmDd(localToday());

  async function load() {
    try {
      const rows = await listMileageTrips();
      setTrips(rows);
    } catch (err) {
      setError(errMessage(err));
    }
  }

  function resetForm() {
    setTripDate(toYyyyMmDd(localToday()));
    setMiles("");
    setPurpose("");
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

  function startEdit(trip: MileageTrip) {
    setEditing(trip);
    setTripDate(trip.tripDate);
    setMiles(String(trip.miles));
    setPurpose(trip.purpose ?? "");
    setError(null);
  }

  async function handleSaveNew() {
    if (saving) return;
    const milesVal = parseMiles(miles);
    if (milesVal === null) {
      setError("miles must be greater than zero");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await recordMileageTrip({
        tripDate,
        miles: milesVal,
        purpose: purpose.trim() || null,
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
    const milesVal = parseMiles(miles);
    if (milesVal === null) {
      setError("miles must be greater than zero");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await correctMileageTrip({
        tripId: editing.tripId,
        tripDate,
        miles: milesVal,
        purpose: purpose.trim() || null,
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
      await voidMileageTrip(editing.tripId);
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
                Edit trip
              </SheetTitle>

              <label className="flex flex-col gap-1.5">
                <span className="text-sm text-muted-foreground">Date</span>
                <input
                  type="date"
                  value={tripDate}
                  max={today}
                  onChange={(e) => setTripDate(e.target.value)}
                  className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                />
              </label>

              <label className="flex flex-col gap-1.5">
                <span className="text-sm text-muted-foreground">Miles</span>
                <input
                  type="text"
                  inputMode="decimal"
                  placeholder="0"
                  value={miles}
                  onChange={(e) => setMiles(e.target.value)}
                  className="h-12 rounded-xl border bg-card px-4 text-lg tabular-nums focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                />
              </label>

              <label className="flex flex-col gap-1.5">
                <span className="text-sm text-muted-foreground">
                  What for (optional)
                </span>
                <input
                  type="text"
                  value={purpose}
                  onChange={(e) => setPurpose(e.target.value)}
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
                Remove this trip
              </Button>
            </>
          ) : (
            <>
              <SheetTitle className="text-center text-xl font-semibold">
                Miles
              </SheetTitle>

              <div className="flex flex-col gap-4">
                <p className="text-center text-base text-muted-foreground">
                  Log a trip
                </p>

                <label className="flex flex-col gap-1.5">
                  <span className="text-sm text-muted-foreground">Date</span>
                  <input
                    type="date"
                    value={tripDate}
                    max={today}
                    onChange={(e) => setTripDate(e.target.value)}
                    className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  />
                </label>

                <label className="flex flex-col gap-1.5">
                  <span className="text-sm text-muted-foreground">Miles</span>
                  <input
                    type="text"
                    inputMode="decimal"
                    placeholder="0"
                    value={miles}
                    onChange={(e) => setMiles(e.target.value)}
                    className="h-12 rounded-xl border bg-card px-4 text-lg tabular-nums focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  />
                </label>

                <label className="flex flex-col gap-1.5">
                  <span className="text-sm text-muted-foreground">
                    What for (optional)
                  </span>
                  <input
                    type="text"
                    value={purpose}
                    onChange={(e) => setPurpose(e.target.value)}
                    className="h-12 rounded-xl border bg-card px-4 text-lg focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
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
                {trips.map((trip) => (
                  <li key={trip.tripId}>
                    <button
                      type="button"
                      onClick={() => startEdit(trip)}
                      className="flex min-h-14 w-full items-center justify-between gap-4 rounded-xl border px-4 text-left text-base transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      <span className="font-medium">
                        {formatTripDate(trip.tripDate)}
                      </span>
                      <span className="tabular-nums text-muted-foreground">
                        {trip.miles} mi
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
