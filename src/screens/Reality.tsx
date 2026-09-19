import { useEffect, useState } from "react";
import type {
  AdminPhoneView,
  AttentionItem,
  CoverDate,
  Crop,
  HarvestCommitmentsView,
  PhoneCaptureView,
  PhonePullView,
  RecountResult,
  SeedOnHandRow,
  SeedReceiptView,
  StandingDemandView,
  TodayView,
  DockFoldsView,
} from "@/farm/types";
import {
  adminPhoneStatus,
  checkAttention,
  confirmPhoneCaptures,
  coverPlan,
  devSeedPhoneProposal,
  discardPhoneCapture,
  dismissAttention,
  farmLocation,
  farmUnits,
  harvestCommitments,
  listCrops,
  phoneCaptures,
  phonePullView,
  pullPhoneCaptures,
  seedOnHand,
  sowTray,
  standingDemand,
  todayView,
  undoLast,
  dockFolds,
} from "@/farm/api";
import { estWeekday, monthDayLabel, snapshotLabel } from "@/farm/dates";
import { DEFAULT_UNITS, grams, massFigure, unitWord } from "@/farm/mass";
import { typedOunces } from "@/farm/typed";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { ErrorLine } from "@/components/ErrorLine";
import { SowSheet } from "@/components/SowSheet";
import { RecountSheet, recountResultMessage } from "@/components/RecountSheet";
import { CropsSheet } from "@/components/CropsSheet";
import { SeedInSheet } from "@/components/SeedInSheet";
import { FarmBackupSheet } from "@/components/FarmBackupSheet";
import { MoneyCaptureControls } from "@/components/MoneyCaptureControls";
import { sowableGapsRemain, surfaceForKind } from "@/farm/surfaces";
function trayCountLabel(quantity: number): string {
  return `${quantity} ${quantity === 1 ? "tray" : "trays"}`;
}
// JAR-READER Job B — the window opens at the stamp stored on the row (the
// crop's first receipt), never at the clock. The same date label the other
// tabs print for a day; an unparseable stamp is printed as stored.
function sinceLabel(stamp: string): string {
  const d = new Date(stamp);
  return Number.isNaN(d.getTime()) ? stamp : monthDayLabel(d);
}
// JAR-READER Job B — one muted line per crop with a receipt (SCOPE A). Every
// figure is the reader's (seedOnHand); nothing is computed here. Printed in the
// desk's units (GT-D27 UNITS), display only. BLANK A: onHandOz null is printed
// as unknown, never as a number. NEGATIVE A: short is printed, never clamped.
function jarLine(row: SeedOnHandRow, system: string): string {
  const since = sinceLabel(row.since);
  if (row.onHandOz === null) {
    return `${row.cropName} · on hand unknown · ${row.unweighedSows} sows since ${since} recorded no seed weight`;
  }
  const u = unitWord(system);
  const inOut = `${massFigure(row.receivedOz, system)} in, ${massFigure(row.sownOz, system)} sown since ${since}`;
  if (row.onHandOz < 0) {
    return `${row.cropName} · ${massFigure(Math.abs(row.onHandOz), system)} ${u} short · ${inOut}`;
  }
  return `${row.cropName} · ${massFigure(row.onHandOz, system)} ${u} on hand · ${inOut}`;
}
const actionCardClass =
  "flex min-h-24 cursor-pointer items-center justify-center p-8 text-center text-xl font-medium transition-colors active:translate-y-px hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";
const confirmCardClass =
  "flex min-h-24 items-center justify-between gap-4 p-8 text-xl font-medium";
type LastAction =
  | { kind: "restored"; label: string }
  | { kind: "recounted"; message: string; canUndo: boolean }
  | { kind: "attention_dismissed" }
  | { kind: "phone_confirmed"; lines: string[] }
  | { kind: "phone_discarded" }
  | { kind: "seed_received"; line: string };
/**
 * Farm — the physical-farm state surface (the Farm tab; the file
 * keeps its name).
 * Growth timeline, shelf facts, the unforced sow door, the farm's records.
 * No money debt is evaluated here; forced work lives on Today.
 */
export function Reality({ pollTick = 0 }: { pollTick?: number }) {
  const [crops, setCrops] = useState<Crop[]>([]);
  // GT-D27 UNITS (J2) - display only; Farm's write paths still carry ounces.
  // Imperial until the desk answers, and imperial if it never does (STORE B).
  const [unitSystem, setUnitSystem] = useState<string>(DEFAULT_UNITS);
  const [view, setView] = useState<TodayView | null>(null);
  const [attention, setAttention] = useState<AttentionItem[]>([]);
  const [demand, setDemand] = useState<StandingDemandView | null>(null);
  const [cover, setCover] = useState<CoverDate[]>([]);
  const [folds, setFolds] = useState<DockFoldsView | null>(null);
  // JAR-READER Job B — the jar, per crop, as seedOnHand() answers it. Null
  // until the first answer; no receipts, no rows, no lines.
  const [jar, setJar] = useState<SeedOnHandRow[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [lastError, setLastError] = useState<string | null>(null);
  const [lastAction, setLastAction] = useState<LastAction | null>(null);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [recountOpen, setRecountOpen] = useState(false);
  const [cropsOpen, setCropsOpen] = useState(false);
  const [seedInOpen, setSeedInOpen] = useState(false);
  const [backupOpen, setBackupOpen] = useState(false);
  const [nextLightOpen, setNextLightOpen] = useState(false);
  const [lastBackupAt, setLastBackupAt] = useState<Date | null>(null);
  const [captures, setCaptures] = useState<PhoneCaptureView[]>([]);
  const [acceptedIds, setAcceptedIds] = useState<Record<string, true>>({});
  const [drafts, setDrafts] = useState<Record<string, { quantity: number; actualYieldOz: number | null }>>({});
  const [editing, setEditing] = useState<Record<string, boolean>>({});
  const [blockedLines, setBlockedLines] = useState<Record<string, string>>({});
  const [commitments, setCommitments] = useState<Record<string, HarvestCommitmentsView>>({});
  const [captureBusy, setCaptureBusy] = useState(false);
  // Settings fence 1 (S3): Pair / Copy link / Retire moved to Settings › Field
  // terminal. Farm keeps the status line (same reader), Pull, Confirm / Discard.
  const [adminPhone, setAdminPhone] = useState<AdminPhoneView | null>(null);
  const [pullView, setPullView] = useState<PhonePullView | null>(null);
  const [pullBusy, setPullBusy] = useState(false);
  async function refresh() {
    // FARM-PAIR Job 1 — the pair reader runs before the three unguarded awaits
    // below, so a failed todayView / checkAttention / phoneCaptures can no longer
    // abort refresh() and leave Farm printing the pair state of an earlier pass.
    try { setAdminPhone(await adminPhoneStatus()); } catch { setAdminPhone(null); }
    try { setPullView(await phonePullView()); } catch { setPullView(null); }
    setView(await todayView());
    setAttention(await checkAttention());
    const next = await phoneCaptures();
    setCaptures(next);
    const live = new Set(next.map((v) => v.proposalId));
    setAcceptedIds((m) => Object.fromEntries(Object.entries(m).filter(([id]) => live.has(id))) as Record<string, true>);
    setDrafts((m) => Object.fromEntries(Object.entries(m).filter(([id]) => live.has(id))));
    setEditing((m) => Object.fromEntries(Object.entries(m).filter(([id]) => live.has(id))));
    setBlockedLines((m) => Object.fromEntries(Object.entries(m).filter(([id]) => live.has(id))));
    setCommitments((m) => Object.fromEntries(Object.entries(m).filter(([id]) => live.has(id))));
    try {
      setDemand(await standingDemand());
    } catch {
      setDemand(null);
    }
    try {
      setCover(await coverPlan());
    } catch {
      setCover([]);
    }
    try {
      setFolds(await dockFolds());
    } catch {
      // folds stays null until the first answer — never a guessed default
    }
    try {
      setJar(await seedOnHand());
    } catch {
      // jar stays null until the first answer — never a guessed default
    }
  }
  async function refreshBackupLine() {
    try {
      const loc = await farmLocation();
      setLastBackupAt(loc.lastSnapshotAt ? new Date(loc.lastSnapshotAt) : null);
    } catch (err) {
      console.error(err);
    }
  }
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const c = await listCrops();
        if (!cancelled) setCrops(c);
        try {
          const u = await farmUnits();
          if (!cancelled) setUnitSystem(u.system);
        } catch (e) {
          console.error(e);
        }
        await refresh();
        await refreshBackupLine();
      } catch (err) {
        console.error(err);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);
  useEffect(() => { if (pollTick === 0) return; void refresh().catch(console.error); }, [pollTick]);
  async function handleDismiss(id: string) {
    try {
      setLastError(null);
      await dismissAttention(id);
      setLastAction({ kind: "attention_dismissed" });
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
    }
  }
  async function handleUndo() {
    try {
      setLastError(null);
      const undone = await undoLast();
      if (undone == null) {
        setLastError("Nothing to undo.");
        await refresh();
        return;
      }
      setLastAction(null);
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
    }
  }
  async function handleSow(crop: Crop, quantity: number, seedOz: number | null) {
    try {
      setLastError(null);
      await sowTray(crop.id, quantity, seedOz);
      await refresh();
      try {
        const [d, plan] = await Promise.all([standingDemand(), coverPlan()]);
        setCover(plan);
        if (!sowableGapsRemain(d, plan)) setSheetOpen(false);
      } catch {
        setSheetOpen(false);
      }
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
      try {
        await refresh();
      } catch (e) {
        setLastError(e instanceof Error ? e.message : String(e));
      }
    }
  }
  async function handleRecountDone(result: RecountResult, cropCount: number) {
    const { matched, text } = recountResultMessage(result, cropCount);
    setLastAction({ kind: "recounted", message: text, canUndo: !matched });
    try {
      setLastError(null);
      await refresh();
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
    }
  }
  async function handleRestored(label: string) {
    setLastAction({ kind: "restored", label });
    try {
      await refresh();
      await refreshBackupLine();
    } catch (err) {
      console.error(err);
    }
  }
  // SEED-A (GT-D25) — the seed-in door's receipt. No Undo: seed.received is
  // inverse none; a wrong receipt is answered by a later record.
  // JAR-READER Job B — the jar lines under Records read the receipts, so that
  // one read is re-run here; nothing else on Farm moved, so no refresh().
  async function handleSeedRecorded(receipt: SeedReceiptView) {
    setLastAction({
      kind: "seed_received",
      line: `Seed in — ${massFigure(receipt.receivedOz, unitSystem)} ${unitWord(unitSystem)} of ${receipt.cropName}.`,
    });
    try {
      setLastError(null);
      setJar(await seedOnHand());
    } catch (err) {
      setLastError(err instanceof Error ? err.message : String(err));
    }
  }
  function draftFor(v: PhoneCaptureView) { return drafts[v.proposalId] ?? { quantity: v.quantity, actualYieldOz: v.actualYieldOz }; }
  function setDraft(v: PhoneCaptureView, patch: Partial<{ quantity: number; actualYieldOz: number | null }>) {
    setDrafts((m) => ({ ...m, [v.proposalId]: { ...draftFor(v), ...patch } }));
  }
  async function toggleAccept(v: PhoneCaptureView) {
    if (acceptedIds[v.proposalId]) {
      setAcceptedIds((m) => { const n = { ...m }; delete n[v.proposalId]; return n; });
      return;
    }
    setAcceptedIds((m) => ({ ...m, [v.proposalId]: true }));
    if (v.verb === "harvest") {
      // Ruling 4: the GT-D19 read-only commitments list, same reader, at Confirm.
      try { const c = await harvestCommitments(v.cropId); setCommitments((m) => ({ ...m, [v.proposalId]: c })); }
      catch (err) { setLastError(err instanceof Error ? err.message : String(err)); }
    }
  }
  // B2-F1 (B2-D3) — one tap accepts every pending capture, through the same
  // per-row accept path (drafts untouched: captured count and weight stand).
  // Component state only. Confirm remains the only door that runs the gate and
  // writes; a row blocked before re-gates there and stays pending with its
  // sentence.
  async function acceptAll() {
    for (const v of captures) {
      if (!acceptedIds[v.proposalId]) await toggleAccept(v);
    }
  }
  async function handleDiscard(proposalId: string) {
    setCaptureBusy(true); setLastError(null);
    try { await discardPhoneCapture(proposalId); setLastAction({ kind: "phone_discarded" }); await refresh(); }
    catch (err) { setLastError(err instanceof Error ? err.message : String(err)); }
    finally { setCaptureBusy(false); }
  }
  async function handleConfirm() {
    const rows = captures.filter((v) => acceptedIds[v.proposalId]).map((v) => {
      const d = draftFor(v);
      return { proposalId: v.proposalId, quantity: d.quantity, actualYieldOz: v.verb === "harvest" ? d.actualYieldOz : null };
    });
    if (rows.length === 0) return;
    setCaptureBusy(true); setLastError(null);
    try {
      const res = await confirmPhoneCaptures(rows);
      const nextBlocked: Record<string, string> = {};
      for (const b of res.blocked) nextBlocked[b.proposalId] = b.sentence;
      setBlockedLines(nextBlocked);
      if (res.written.length > 0) setLastAction({ kind: "phone_confirmed", lines: res.written.map((w) => w.line) });
      await refresh();
    } catch (err) { setLastError(err instanceof Error ? err.message : String(err)); }
    finally { setCaptureBusy(false); }
  }
  async function handlePull() { setPullBusy(true); setLastError(null);
    try { setPullView(await pullPhoneCaptures()); await refresh(); }
    catch (err) { setLastError(err instanceof Error ? err.message : String(err)); } finally { setPullBusy(false); } }
  const realityAttention = folds
    ? attention.filter((a) => surfaceForKind(folds, a.kind) === "reality" && a.kind !== "phone.proposal")
    : [];
  const nextLightEvents = view?.nextEvents.filter((ne) => ne.kind === "light") ?? [];
  const nextHarvestEvents =
    view?.nextEvents.filter((ne) => ne.kind === "harvest") ?? [];
  const nextLightTrayTotal = nextLightEvents.reduce((s, ne) => s + ne.trayCount, 0);
  const backupLine = lastBackupAt
    ? `Farm saved automatically · last backup ${snapshotLabel(lastBackupAt)}`
    : "Farm saved automatically · last backup —";
  const sownTodaySingleLight =
    !!view &&
    view.sownToday &&
    view.nextEvents.length === 1 &&
    view.nextEvents[0].kind === "light";
  return (
    <main className="mx-auto flex min-h-screen w-full max-w-md flex-col gap-8 px-6 py-12">
      <h1 className="text-2xl font-semibold tracking-tight">Farm</h1>
      <ErrorLine message={lastError} />
      {/* STATES A - the loading face: the count, the instrument lines and the sow
          card as blocks of var(--border) in the flow they will take. Gated on
          `loading` alone: a failed probe leaves view null with loading false, and
          gating on view too would hold this skeleton forever on an unreadable farm. */}
      {loading && (
        <div className="flex flex-col gap-8" aria-hidden="true">
          <div className="skeleton-block h-9 w-2/3" />
          <div className="skeleton-block h-16" />
          <div className="skeleton-block h-24" />
        </div>
      )}
      {!loading && view && (
        <>
          <p className="text-3xl font-medium tabular-nums">
            {view.activeTrayCount === 0
              ? "Nothing growing right now."
              : `${trayCountLabel(view.activeTrayCount)} growing.`}
          </p>
          {lastAction?.kind === "restored" && (
            <Card className={confirmCardClass}>
              <span>Farm restored from {lastAction.label}.</span>
            </Card>
          )}
          {lastAction?.kind === "recounted" && (
            <Card className={confirmCardClass}>
              <span>{lastAction.message}</span>
              {lastAction.canUndo && (
                <Button
                  type="button"
                  variant="ghost"
                  onClick={handleUndo}
                  className="h-14 px-3 text-base"
                >
                  Undo
                </Button>
              )}
            </Card>
          )}
          {lastAction?.kind === "attention_dismissed" && (
            <Card className={confirmCardClass}>
              <span>Dismissed.</span>
              <Button
                type="button"
                variant="ghost"
                onClick={handleUndo}
                className="h-14 px-3 text-base"
              >
                Undo
              </Button>
            </Card>
          )}
          {lastAction?.kind === "seed_received" && (
            <Card className={confirmCardClass}>
              <span>{lastAction.line}</span>
            </Card>
          )}
          {realityAttention.length > 0 && (
            <div className="flex flex-col gap-6">
              {realityAttention.map((item) => (
                <Card key={item.id} className="flex flex-col gap-4 p-6">
                  <p className="text-base font-medium leading-snug">{item.message}</p>
                  <div className="flex flex-wrap gap-2">
                    <Button
                      type="button"
                      variant="ghost"
                      className="h-11 px-4 text-base"
                      onClick={() => void handleDismiss(item.id)}
                    >
                      Dismiss
                    </Button>
                  </div>
                </Card>
              ))}
            </div>
          )}
          {/* Growth timeline — ruling 6.5 moved it here whole. */}
          {sownTodaySingleLight ? (
            <Card className="flex flex-col gap-2 p-6">
              <p className="text-xl font-medium">
                {trayCountLabel(view.nextEvents[0].trayCount)} of{" "}
                {view.nextEvents[0].cropName}
              </p>
              <p className="text-base text-muted-foreground">
                {trayCountLabel(view.nextEvents[0].trayCount)} of{" "}
                {view.nextEvents[0].cropName} · sown today · cover check{" "}
                {estWeekday(view.nextEvents[0].date)}
              </p>
            </Card>
          ) : view.nextEvents.length === 0 ? (
            <p className="text-xl font-medium">Nothing growing right now.</p>
          ) : (
            <div className="flex flex-col">
              {nextLightEvents.length >= 2 ? (
                <>
                  <button
                    type="button"
                    aria-expanded={nextLightOpen}
                    onClick={() => setNextLightOpen((v) => !v)}
                    className="flex min-h-11 items-center gap-2 border-t border-border py-2 text-left text-xl font-medium focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  >
                    <span aria-hidden="true">{nextLightOpen ? "▾" : "▸"}</span>
                    Move to light ({trayCountLabel(nextLightTrayTotal)})
                  </button>
                  {nextLightOpen &&
                    nextLightEvents.map((ne) => (
                      <p
                        key={`light-${ne.date}-${ne.cropName}`}
                        className="border-t border-border py-2 pl-6 text-xl font-medium"
                      >
                        {`Next: move ${trayCountLabel(ne.trayCount)} of ${ne.cropName} to light on ${estWeekday(ne.date)}.`}
                      </p>
                    ))}
                </>
              ) : (
                nextLightEvents.map((ne) => (
                  <p
                    key={`light-${ne.date}-${ne.cropName}`}
                    className="border-t border-border py-2 text-xl font-medium"
                  >
                    {`Next: move ${trayCountLabel(ne.trayCount)} of ${ne.cropName} to light on ${estWeekday(ne.date)}.`}
                  </p>
                ))
              )}
              {nextHarvestEvents.map((ne) => (
                <p
                  key={`harvest-${ne.date}-${ne.cropName}`}
                  className="border-t border-border py-2 text-xl font-medium"
                >
                  {`Next: harvest ${trayCountLabel(ne.trayCount)} of ${ne.cropName} on ${estWeekday(ne.date)}.`}
                </p>
              ))}
            </div>
          )}
          <Card
            role="button"
            tabIndex={0}
            onClick={() => setSheetOpen(true)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                setSheetOpen(true);
              }
            }}
            className={actionCardClass}
          >
            {view.activeTrayCount === 0 ? "Sow your first tray" : "Sow more trays"}
          </Card>
          {/* Farm opens on physical truth. The trays line, the growth timeline and the Sow
              door are what this tab is named for; the capture queue can be empty and must
              not hold the top of the surface. Position only — this section is byte-identical
              to what stood above, and Confirm remains the only write door (phone::gate). */}
          <Card>
            <CardHeader>
              <CardTitle>
                {captures.length > 0 ? `Phone captures (${captures.length})` : "Phone captures"}
              </CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              {lastAction?.kind === "phone_confirmed" &&
                lastAction.lines.map((line, i) => (
                  <Card key={`pc-${i}`} className={confirmCardClass}><span>{line}</span></Card>
                ))}
              {lastAction?.kind === "phone_discarded" && (
                <Card className={confirmCardClass}><span>Discarded.</span></Card>
              )}
              {captures.length === 0 ? (
                <p className="text-sm text-muted-foreground">No phone captures waiting.</p>
              ) : (
                <ul className="flex flex-col gap-3">
                  {captures.map((v) => {
                    const isAccepted = !!acceptedIds[v.proposalId];
                    const d = draftFor(v);
                    const c = commitments[v.proposalId];
                    return (
                      <li key={v.proposalId} className="flex flex-col gap-2 border-b border-border pb-3">
                        <p className="text-sm">{v.message}</p>
                        {editing[v.proposalId] && (
                          <div className="flex flex-wrap items-center gap-4">
                            <div className="flex items-center gap-3">
                              <button type="button" aria-label="Fewer trays" disabled={d.quantity <= 1}
                                onClick={() => setDraft(v, { quantity: Math.max(1, d.quantity - 1) })}
                                className="flex size-11 items-center justify-center rounded-full border text-2xl leading-none disabled:opacity-40">−</button>
                              <span className="w-10 text-center text-2xl font-semibold tabular-nums">{d.quantity}</span>
                              <button type="button" aria-label="More trays"
                                onClick={() => setDraft(v, { quantity: d.quantity + 1 })}
                                className="flex size-11 items-center justify-center rounded-full border text-2xl leading-none">+</button>
                            </div>
                            {v.verb === "harvest" && (
                              <label className="flex items-center gap-2 text-sm">
                                <input type="number" inputMode="decimal" min="0" step={unitSystem === "metric" ? "1" : "0.1"}
                                  value={d.actualYieldOz == null
                                    ? ""
                                    : unitSystem === "metric" ? String(grams(d.actualYieldOz)) : String(d.actualYieldOz)}
                                  onChange={(e) => setDraft(v, { actualYieldOz: e.target.value === "" ? null : typedOunces(e.target.value, unitSystem) })}
                                  className="h-11 w-24 rounded-md border px-2 text-base tabular-nums" />
                                {unitWord(unitSystem)}
                              </label>
                            )}
                          </div>
                        )}
                        {isAccepted && v.verb === "harvest" && c && (
                          <div className="flex flex-col gap-1">
                            <p className="text-sm font-medium">{c.header}</p>
                            {c.lines.length === 0 ? (
                              <p className="text-sm text-muted-foreground">{c.emptyLine}</p>
                            ) : (
                              c.lines.map((l) => (
                                <p key={`${l.kind}-${l.id}`} className="text-sm text-muted-foreground">{l.text}</p>
                              ))
                            )}
                          </div>
                        )}
                        {blockedLines[v.proposalId] && (
                          <p className="text-sm text-amber-700">{blockedLines[v.proposalId]}</p>
                        )}
                        <div className="flex flex-wrap gap-2">
                          <Button type="button" aria-pressed={isAccepted} variant={isAccepted ? "default" : "outline"}
                            className="h-11 px-4" disabled={captureBusy} onClick={() => void toggleAccept(v)}>Accept</Button>
                          <Button type="button" variant="outline" className="h-11 px-4" disabled={captureBusy}
                            onClick={() => setEditing((m) => ({ ...m, [v.proposalId]: !m[v.proposalId] }))}>Edit</Button>
                          <Button type="button" variant="ghost" className="h-11 px-4" disabled={captureBusy}
                            onClick={() => void handleDiscard(v.proposalId)}>Discard</Button>
                        </div>
                      </li>
                    );
                  })}
                </ul>
              )}
              {captures.length > 0 && (
                <div className="flex flex-wrap items-center gap-2">
                  {/* B2-F1 (B2-D3) — Accept all beside Confirm. Accept is
                      acknowledgement; Confirm is the door (phone::gate). */}
                  <Button type="button" variant="outline" className="h-12 px-4 text-base"
                    disabled={captureBusy || captures.every((v) => !!acceptedIds[v.proposalId])}
                    onClick={() => void acceptAll()}>Accept all</Button>
                  <Button type="button" className="h-12 self-start text-base"
                    disabled={captureBusy || Object.keys(acceptedIds).length === 0}
                    onClick={() => void handleConfirm()}>Confirm</Button>
                </div>
              )}
              <div className="flex flex-col gap-2">
                <p className="text-sm text-muted-foreground">{adminPhone?.status ?? ""}</p>
                <div className="flex flex-wrap gap-2">
                  <Button type="button" variant="outline" className="h-11 px-4" disabled={pullBusy} onClick={() => void handlePull()}>Pull phone captures</Button>
                </div>
                {pullView && (
                  <div className="flex flex-col gap-1">
                    <p className="text-sm text-muted-foreground">{pullView.message}</p>
                    {pullView.lastOkMessage && <p className="text-sm text-muted-foreground">{pullView.lastOkMessage}</p>}
                    {pullView.refusalMessage && <p className="text-sm text-amber-700">{pullView.refusalMessage}</p>}
                    {pullView.gapMessage && <p className="text-sm text-amber-700">{pullView.gapMessage}</p>}
                  </div>
                )}
              </div>
              {import.meta.env.DEV && (
                <DevSeedPhoneCapture crops={crops} onSeeded={() => void refresh()} onError={setLastError} />
              )}
            </CardContent>
          </Card>
          <MoneyCaptureControls collapsible />
          <div className="border-t border-border" aria-hidden="true" />
          <Card>
            <CardHeader>
              <CardTitle>Records</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
            {view.activeTrayCount > 0 && (
              <button
                type="button"
                onClick={() => setRecountOpen(true)}
                className="flex min-h-11 items-center text-left text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                Count the shelf
              </button>
            )}
            <button
              type="button"
              onClick={() => setCropsOpen(true)}
              className="flex min-h-11 items-center text-left text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            >
              Crops
            </button>
            {/* SEED-A (GT-D25) — the record-seed-in door, beside Crops. Hidden
                until the crop list has answered with at least one crop. */}
            {crops.length > 0 && (
              <button
                type="button"
                onClick={() => setSeedInOpen(true)}
                className="flex min-h-11 items-center text-left text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                Record seed in
              </button>
            )}
            {/* JAR-READER Job B (SURFACE A · SCOPE A) — what is still in each
                crop's jar, one muted line per crop with a receipt, straight from
                seedOnHand(). unattributedOz is farm-wide and repeats on every
                row, so it prints once, under the list, only when above zero. */}
            {jar && jar.length > 0 && (
              <div className="flex flex-col gap-1">
                {jar.map((row) => (
                  <p key={row.cropId} className="text-sm text-muted-foreground">
                    {jarLine(row, unitSystem)}
                  </p>
                ))}
                {jar[0].unattributedOz > 0 && (
                  <p className="text-sm text-muted-foreground">
                    {`${massFigure(jar[0].unattributedOz, unitSystem)} ${unitWord(unitSystem)} not tied to a crop`}
                  </p>
                )}
              </div>
            )}
            </CardContent>
          </Card>
          <div className="border-t border-border" aria-hidden="true" />
          <Card>
            <CardContent className="flex flex-col gap-3">
            <button
              type="button"
              onClick={() => setBackupOpen(true)}
              className="text-left text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            >
              {backupLine}
            </button>
            </CardContent>
          </Card>
        </>
      )}
      <SowSheet
        open={sheetOpen}
        onOpenChange={setSheetOpen}
        crops={crops}
        onSow={handleSow}
        demand={demand}
        coverDates={cover}
        unitSystem={unitSystem}
      />
      <RecountSheet
        open={recountOpen}
        onOpenChange={setRecountOpen}
        onDone={handleRecountDone}
      />
      <CropsSheet
        open={cropsOpen}
        onOpenChange={setCropsOpen}
        unitSystem={unitSystem}
        onSaved={() => {
          void listCrops().then(setCrops).catch(console.error);
        }}
      />
      <SeedInSheet
        open={seedInOpen}
        onOpenChange={setSeedInOpen}
        crops={crops}
        onRecorded={handleSeedRecorded}
        unitSystem={unitSystem}
      />
      <FarmBackupSheet
        open={backupOpen}
        onOpenChange={setBackupOpen}
        onRestored={handleRestored}
        recoveryMode={false}
        onImported={() => {
          void refresh();
        }}
      />
    </main>
  );
}
function DevSeedPhoneCapture({ crops, onSeeded, onError }: { crops: Crop[]; onSeeded: () => void; onError: (m: string) => void }) {
  const [verb, setVerb] = useState<"move_to_light" | "harvest">("move_to_light");
  const [cropId, setCropId] = useState(crops[0]?.id ?? "");
  const [quantity, setQuantity] = useState(1);
  const [oz, setOz] = useState("");
  const [daysAgo, setDaysAgo] = useState(0);
  const [busy, setBusy] = useState(false);
  useEffect(() => { if (!cropId && crops[0]) setCropId(crops[0].id); }, [crops, cropId]);
  async function seed() {
    setBusy(true);
    try {
      await devSeedPhoneProposal({ verb, cropId, quantity,
        actualYieldOz: verb === "harvest" ? (oz === "" ? null : Number(oz)) : null,
        capturedDaysAgo: daysAgo, note: null });
      onSeeded();
    } catch (e) { onError(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  }
  return (
    <div className="flex flex-wrap items-end gap-2 text-sm">
      <label className="flex flex-col">Dev verb
        <select value={verb} onChange={(e) => setVerb(e.target.value as "move_to_light" | "harvest")} className="h-9 rounded-md border px-2">
          <option value="move_to_light">Move to light</option><option value="harvest">Harvest</option>
        </select></label>
      <label className="flex flex-col">Dev crop
        <select value={cropId} onChange={(e) => setCropId(e.target.value)} className="h-9 rounded-md border px-2">
          {crops.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
        </select></label>
      <label className="flex flex-col">Dev trays
        <input type="number" min={1} value={quantity} onChange={(e) => setQuantity(Math.max(1, Number(e.target.value)))} className="h-9 w-20 rounded-md border px-2" /></label>
      {verb === "harvest" && (<label className="flex flex-col">Dev oz
        <input type="number" step="0.1" value={oz} onChange={(e) => setOz(e.target.value)} className="h-9 w-24 rounded-md border px-2" /></label>)}
      <label className="flex flex-col">Dev captured days ago (negative = future)
        <input type="number" value={daysAgo} onChange={(e) => setDaysAgo(Number(e.target.value))} className="h-9 w-24 rounded-md border px-2" /></label>
      <Button type="button" variant="outline" className="self-start text-sm" disabled={busy || !cropId} onClick={() => void seed()}>
        Dev: seed a phone capture
      </Button>
    </div>
  );
}
