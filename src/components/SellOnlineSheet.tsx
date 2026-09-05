import { useEffect, useState } from "react";
import type {
  MoneyStatus,
  OfferView,
  ReconciliationDate,
  ShopPage,
  StripeAccountPreview,
} from "@/farm/types";
import {
  capacityByHarvestDate,
  confirmStripeKey,
  generateShopPage,
  listOffers,
  moneyStatus,
  openShopPageFolder,
  previewStripeKey,
  reconciliation,
  removeOffer,
  setCheckoutEndpointUrl,
  setOffer,
} from "@/farm/api";
import { parseLocalDate, weekdayName } from "@/farm/dates";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import { PricePad } from "@/components/PricePad";

type SellOnlineSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

type Step = "status" | "paste" | "confirm";

function errMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return "Something went wrong. Try again.";
}

function formatPrice(cents: number): string {
  return `$${(cents / 100).toFixed(2)}`;
}

function formatHarvestLabel(yyyyMmDd: string): string {
  const d = parseLocalDate(yyyyMmDd);
  return `${weekdayName(d)} ${d.getDate()} ${d.toLocaleString("en-CA", { month: "short" })}`;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} bytes`;
  return `${(n / 1024).toFixed(1)} KB`;
}

export function SellOnlineSheet({ open, onOpenChange }: SellOnlineSheetProps) {
  const [status, setStatus] = useState<MoneyStatus | null>(null);
  const [step, setStep] = useState<Step>("status");
  const [key, setKey] = useState("");
  const [preview, setPreview] = useState<StripeAccountPreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // The sale sheet sells a DATE. It deliberately does NOT carry cropId,
  // cropName, coverSupply, coverPromised or coverRemaining: cover math is
  // not a sale answer (D21a).
  type SaleDate = { harvestDate: string; remainingTrays: number };
  const [dates, setDates] = useState<SaleDate[]>([]);
  const [harvestDate, setHarvestDate] = useState<string | null>(null);
  const [offers, setOffers] = useState<OfferView[]>([]);
  const [priceCrop, setPriceCrop] = useState<OfferView | null>(null);
  const [shop, setShop] = useState<ShopPage | null>(null);
  const [recon, setRecon] = useState<ReconciliationDate[]>([]);
  const [checkoutUrl, setCheckoutUrl] = useState("");

  async function loadStatus() {
    try {
      const s = await moneyStatus();
      setStatus(s);
      setCheckoutUrl(s.checkoutEndpointUrl ?? "");
      setStep(s.configured ? "status" : "paste");
      setError(null);
      if (s.configured) {
        await loadCapacity();
        await loadReconciliation();
      }
    } catch (err) {
      console.error(err);
      setError(errMessage(err));
    }
  }

  async function loadReconciliation() {
    try {
      const rows = await reconciliation();
      setRecon(rows);
    } catch (err) {
      console.error(err);
    }
  }

  async function loadCapacity() {
    const rows = await capacityByHarvestDate();
    // D3: the engine emits one row per (date, crop). Sum FIRST, then gate on
    // the date's own total. Gating per crop row would drop an oversold crop's
    // negative remainder and publish a number higher than the date can serve.
    // D21a: remainingTrays is the exact-date sale figure. Never the cover one.
    const byDate = new Map<string, { trays: number; remainingTrays: number }>();
    for (const r of rows) {
      const acc = byDate.get(r.harvestDate) ?? { trays: 0, remainingTrays: 0 };
      acc.trays += r.trays;
      acc.remainingTrays += r.remainingTrays;
      byDate.set(r.harvestDate, acc);
    }
    const openDates: SaleDate[] = [...byDate.entries()]
      .filter(([, v]) => v.trays > 0)
      .map(([harvestDate, v]) => ({ harvestDate, remainingTrays: v.remainingTrays }))
      .sort((a, b) => a.harvestDate.localeCompare(b.harvestDate));
    setDates(openDates);
    const next = openDates[0]?.harvestDate ?? null;
    setHarvestDate((prev) => {
      if (prev && openDates.some((r) => r.harvestDate === prev)) return prev;
      return next;
    });
  }

  async function loadOffers(date: string) {
    try {
      const rows = await listOffers(date);
      setOffers(rows);
    } catch (err) {
      console.error(err);
      setError(errMessage(err));
    }
  }

  useEffect(() => {
    if (!open) return;
    setKey("");
    setPreview(null);
    setBusy(false);
    setError(null);
    setShop(null);
    void loadStatus();
  }, [open]);

  useEffect(() => {
    if (!open || !status?.configured || !harvestDate) return;
    void loadOffers(harvestDate);
  }, [open, status?.configured, harvestDate]);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setKey("");
      setPreview(null);
      setError(null);
      setBusy(false);
      setPriceCrop(null);
    }
    onOpenChange(next);
  }

  async function handlePreview() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const p = await previewStripeKey(key.trim());
      setPreview(p);
      setStep("confirm");
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleConfirm() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const s = await confirmStripeKey(key.trim());
      setStatus(s);
      setKey("");
      setPreview(null);
      setStep("status");
      await loadCapacity();
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setBusy(false);
    }
  }

  function startReplace() {
    setKey("");
    setPreview(null);
    setError(null);
    setStep("paste");
  }

  async function handleSavePrice(cents: number) {
    if (!priceCrop || !harvestDate) return;
    setBusy(true);
    setError(null);
    try {
      await setOffer(harvestDate, priceCrop.cropId, cents);
      setPriceCrop(null);
      await loadOffers(harvestDate);
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove(offerId: string) {
    setBusy(true);
    setError(null);
    try {
      await removeOffer(offerId);
      if (harvestDate) await loadOffers(harvestDate);
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleSaveCheckoutUrl() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const s = await setCheckoutEndpointUrl(checkoutUrl.trim());
      setStatus(s);
      setCheckoutUrl(s.checkoutEndpointUrl ?? "");
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleGenerate() {
    setBusy(true);
    setError(null);
    try {
      const page = await generateShopPage();
      setShop(page);
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Sheet open={open} onOpenChange={handleOpenChange}>
        <SheetContent
          side="bottom"
          className="max-h-[90vh] gap-5 overflow-y-auto px-6 pt-6 pb-10"
        >
          <SheetTitle className="text-xl font-medium">Sell online</SheetTitle>

          {step === "status" && status?.configured && (
            <div className="flex flex-col gap-5">
              <div className="flex flex-col gap-2">
                <p className="text-base text-muted-foreground">
                  Connected to {status.accountName ?? "Stripe"} ·{" "}
                  {status.mode === "live" ? "live mode" : "test mode"}
                </p>
                <button
                  type="button"
                  className="text-left text-sm text-muted-foreground underline-offset-4 hover:underline"
                  onClick={startReplace}
                >
                  Replace key
                </button>
              </div>

              <div className="flex flex-col gap-2">
                <label
                  htmlFor="checkout-endpoint"
                  className="text-sm font-medium"
                >
                  Checkout address
                </label>
                <input
                  id="checkout-endpoint"
                  type="url"
                  inputMode="url"
                  autoComplete="off"
                  placeholder="https://…"
                  value={checkoutUrl}
                  onChange={(e) => setCheckoutUrl(e.target.value)}
                  className="min-h-11 rounded-md border border-border bg-card px-3 text-sm"
                />
                <p className="text-sm text-muted-foreground">
                  The Worker URL from your checkout deploy. Not a secret — it
                  appears on the shop page.
                </p>
                <Button
                  type="button"
                  variant="outline"
                  className="min-h-11 w-fit"
                  disabled={busy || !checkoutUrl.trim()}
                  onClick={() => void handleSaveCheckoutUrl()}
                >
                  Save checkout address
                </Button>
              </div>

              {dates.length === 0 ? (
                <p className="text-sm text-muted-foreground">
                  Sow trays first — harvest dates appear here when there is
                  capacity to sell.
                </p>
              ) : (
                <>
                  <div className="flex flex-col gap-2">
                    <p className="text-sm font-medium">Harvest date</p>
                    <div className="flex flex-col gap-1">
                      {dates.map((d) => (
                        <button
                          key={d.harvestDate}
                          type="button"
                          onClick={() => setHarvestDate(d.harvestDate)}
                          className={`min-h-11 rounded-md px-3 text-left text-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none ${
                            harvestDate === d.harvestDate
                              ? "bg-accent text-accent-foreground"
                              : "text-muted-foreground hover:bg-accent/50"
                          }`}
                        >
                          {formatHarvestLabel(d.harvestDate)} · {d.remainingTrays}{" "}
                          remaining
                        </button>
                      ))}
                    </div>
                  </div>

                  <div className="flex flex-col gap-3">
                    {offers.map((o) => (
                      <div
                        key={o.cropId}
                        className="flex flex-col gap-1 border-t border-border pt-3"
                      >
                        <div className="flex items-start justify-between gap-3">
                          <div>
                            <p className="font-medium">{o.cropName}</p>
                            <p className="text-sm text-muted-foreground">
                              {o.available} available · {o.sold} sold ·{" "}
                              {o.remaining} remaining
                            </p>
                          </div>
                          <Button
                            type="button"
                            variant="outline"
                            className="min-h-11 shrink-0"
                            onClick={() => setPriceCrop(o)}
                          >
                            {o.priceCents != null
                              ? formatPrice(o.priceCents)
                              : "Set price"}
                          </Button>
                        </div>
                        {o.id && o.priceCents != null && (
                          <button
                            type="button"
                            className="text-left text-sm text-muted-foreground underline-offset-4 hover:underline"
                            onClick={() => void handleRemove(o.id!)}
                            disabled={busy}
                          >
                            Remove from shop
                          </button>
                        )}
                      </div>
                    ))}
                  </div>

                  <Button
                    type="button"
                    className="min-h-12"
                    disabled={busy}
                    onClick={() => void handleGenerate()}
                  >
                    {busy ? "Updating…" : "Update shop page"}
                  </Button>

                  {shop && (
                    <div className="flex flex-col gap-2 text-sm text-muted-foreground">
                      <p>
                        {formatBytes(shop.sizeBytes)} ·{" "}
                        {shop.harvestDates.length} harvest date
                        {shop.harvestDates.length === 1 ? "" : "s"}
                      </p>
                      <p className="break-all">{shop.filePath}</p>
                      <Button
                        type="button"
                        variant="outline"
                        className="min-h-11 justify-start"
                        onClick={() => void openShopPageFolder()}
                      >
                        Open folder
                      </Button>
                    </div>
                  )}

                  {recon.length > 0 && (
                    <div className="flex flex-col gap-4 border-t border-border pt-5">
                      <p className="text-sm font-medium">Reconciliation</p>
                      <p className="text-sm text-muted-foreground">
                        Read-only. Capacity is computed from trays and paid
                        orders — never typed in.
                      </p>
                      {recon.map((row) => (
                        <div key={row.harvestDate} className="flex flex-col gap-2">
                          <p className="text-sm font-medium">
                            {formatHarvestLabel(row.harvestDate)}
                          </p>
                          <p className="text-sm text-muted-foreground">
                            {row.available} available · {row.sold} sold ·{" "}
                            {row.remaining} remaining
                          </p>
                          {row.orders.length === 0 ? (
                            <p className="text-sm text-muted-foreground">
                              No orders yet.
                            </p>
                          ) : (
                            <ul className="flex flex-col gap-1 text-sm text-muted-foreground">
                              {row.orders.map((o) => (
                                <li key={o.id}>
                                  {o.quantity}{" "}
                                  {o.quantity === 1 ? "tray" : "trays"} of{" "}
                                  {o.cropName} · {formatPrice(o.amountCents)} ·{" "}
                                  {o.state}
                                  {o.capacityConsumed === 0
                                    ? " · capacity released"
                                    : ""}
                                </li>
                              ))}
                            </ul>
                          )}
                        </div>
                      ))}
                    </div>
                  )}
                </>
              )}
            </div>
          )}

          {step === "paste" && (
            <div className="flex flex-col gap-4">
              <p className="text-base text-muted-foreground">
                Farm OS can take paid orders through Stripe and set capacity aside
                automatically. You create a restricted key once; nothing else to
                manage day to day.
              </p>
              <ol className="list-decimal space-y-2 pl-5 text-sm text-muted-foreground">
                <li>
                  In the Stripe Dashboard (test mode), open Developers → API keys
                  → Restricted keys → Create restricted key.
                </li>
                <li>
                  Turn on <span className="text-foreground">write</span> for
                  Products, Prices, and Payment Links.
                </li>
                <li>
                  Turn on <span className="text-foreground">read</span> for
                  Checkout Sessions, Refunds, Disputes, and Account. Leave
                  everything else off.
                </li>
                <li>Create the key and paste it below.</li>
              </ol>
              <label className="flex flex-col gap-2">
                <span className="text-sm font-medium">Restricted key</span>
                <input
                  type="password"
                  autoComplete="off"
                  spellCheck={false}
                  value={key}
                  onChange={(e) => setKey(e.target.value)}
                  placeholder="rk_test_…"
                  className="min-h-12 rounded-md border border-input bg-card px-3 text-base outline-none focus-visible:ring-2 focus-visible:ring-ring"
                />
              </label>
              <Button
                type="button"
                className="min-h-12"
                disabled={busy || key.trim().length === 0}
                onClick={() => void handlePreview()}
              >
                {busy ? "Checking…" : "Continue"}
              </Button>
            </div>
          )}

          {step === "confirm" && preview && (
            <div className="flex flex-col gap-4">
              <div className="flex flex-col gap-1">
                <p className="text-lg font-medium">{preview.accountName}</p>
                <p className="text-sm text-muted-foreground">{preview.accountId}</p>
                <p className="text-sm text-muted-foreground">
                  {preview.mode === "live" ? "Live mode" : "Test mode"}
                </p>
              </div>
              <p className="text-base text-muted-foreground">
                This must be a different Stripe account from the one your
                commercial farm uses. Farm OS must never record a sale that also
                exists in your other system.
              </p>
              <Button
                type="button"
                className="min-h-12"
                disabled={busy}
                onClick={() => void handleConfirm()}
              >
                {busy ? "Connecting…" : "Connect this account"}
              </Button>
              <button
                type="button"
                className="text-left text-sm text-muted-foreground underline-offset-4 hover:underline"
                onClick={() => {
                  setPreview(null);
                  setStep("paste");
                }}
              >
                Use a different key
              </button>
            </div>
          )}

          {error && (
            <p className="text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </SheetContent>
      </Sheet>

      <PricePad
        open={priceCrop != null}
        onOpenChange={(next) => {
          if (!next) setPriceCrop(null);
        }}
        cropName={priceCrop?.cropName ?? ""}
        initialCents={priceCrop?.priceCents}
        onSave={(cents) => void handleSavePrice(cents)}
      />
    </>
  );
}
