import { useEffect, useState } from "react";
import type {
  AdminPhoneView,
  DockPortView,
  PairingView,
  ScanConfigView,
} from "@/farm/types";
import {
  adminPhoneStatus,
  dockPortStart,
  dockPortStatus,
  dockPortStop,
  farmLocation,
  farmDisplayName,
  setFarmDisplayName,
  pairAdminPhone,
  retireAdminPhone,
  scanConfig,
  setScanConfig,
  setShelfCapacity,
  shelfCapacity,
} from "@/farm/api";
import { snapshotLabel } from "@/farm/dates";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { ErrorLine } from "@/components/ErrorLine";
import { FarmBackupSheet } from "@/components/FarmBackupSheet";

/**
 * SCAN-PAIR MOVE 2 (decouple). A stranger must not read "no phone paired" as
 * "the farm is broken". Pairing no longer needs a scan endpoint: field_devices.rs
 * returns `link: None` when no worker is configured, and the phone's key is the
 * token. The endpoint stays required for standing requests, pack scans, the
 * customer QR, and both Farm pulls.
 * One physical line per signed sentence so grep -F proves it verbatim.
 */
const FIELD_TERMINAL_COPY =
  "The farm runs on this PC without a phone. Pairing a phone does not need a scan endpoint. A scan endpoint is a small worker you deploy for standing requests, pack scans, and pulls. This app does not host one. Dock below is a different door (this PC on your Wi-Fi).";
const CONNECTIONS_COPY =
  "Scan endpoint is your worker URL, not the Dock address. Leave both fields blank if you only work on this computer.";
const CONNECTIONS_BLANK = "Left blank — scan endpoint not configured.";
const DOCK_HOW_COPY =
  "On the phone: join the same Wi-Fi as this PC, turn off cellular data, then open the address below. Pair the Admin phone first — the Dock uses that token. This is not the scan endpoint. This address is not encrypted. Use it only on Wi-Fi you trust.";

function FarmNameSection() {
  const [name, setName] = useState("");
  const [saved, setSaved] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    void (async () => {
      try {
        const current = await farmDisplayName();
        setSaved(current);
        setName(current ?? "");
      } catch (e) {
        setError(errMessage(e));
      }
    })();
  }, []);
  async function onSave() {
    setBusy(true);
    setError(null);
    try {
      const next = await setFarmDisplayName(name);
      setSaved(next);
      setName(next ?? "");
    } catch (e) {
      setError(errMessage(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Card>
      <CardHeader>
        <CardTitle>Farm name</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <p className="text-sm text-muted-foreground">
          Printed at the top of every invoice. Reference data on this machine —
          not an event, not farm truth. Empty means invoices refuse to show.
        </p>
        <label className="flex flex-col gap-1 text-sm">
          Display name
          <input
            className="h-12 rounded-md border border-input bg-card px-3"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        <Button
          type="button"
          className="h-12 self-start px-4 text-base active:translate-y-px"
          disabled={busy}
          onClick={() => void onSave()}
        >
          {busy ? "Saving…" : "Save farm name"}
        </Button>
        {saved != null ? (
          <p className="text-sm text-muted-foreground">Saved: {saved}</p>
        ) : (
          <p className="text-sm text-muted-foreground">No farm name saved yet.</p>
        )}
        {error != null && (
          <p className="text-sm text-destructive" role="alert">
            {error}
          </p>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * Settings — how this farm is configured and governed (Settings fence 1).
 *
 * The second light to Health: Health says the system cannot lie; Settings says
 * you own the system. Fence 1 is relocation-only — every control here already
 * existed elsewhere and writes only reference data outside the replay ledger
 * (shelf_capacity, scan_config, field_devices; projection/verify.rs
 * EXCLUSION_LIST). Nothing here touches event_log, an evaluator, or a rank.
 *
 * Settings raises nothing: no severity, no attention card, no Health sentence.
 * Real problems still surface through Health (H3 already names a missing
 * snapshot). The sparseness is deliberate — the engines that are not listed
 * here cannot be turned off.
 *
 * Section order is signed (S6 / P2): Trust & Ownership sits high and is
 * load-bearing; Backup & Export is the concrete proof that you can leave.
 */

const FIELD =
  "h-14 w-full rounded-xl border bg-card px-4 text-base focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";

function errMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return String(err);
}

/** Blank → null (unknown, not zero). Non-blank → number (Rust owns validation). */
function slotsFieldToValue(raw: string): number | null {
  const trimmed = raw.trim();
  if (trimmed === "") return null;
  return Number(trimmed);
}

export function Settings() {
  const [error, setError] = useState<string | null>(null);

  // Farm — Shelf space (moved from CropsSheet "Space", S2). Explicit Save.
  const [lightSlots, setLightSlots] = useState("");
  const [blackoutSlots, setBlackoutSlots] = useState("");
  const [spaceSaved, setSpaceSaved] = useState(false);
  const [spaceBusy, setSpaceBusy] = useState(false);

  // Field terminal (moved from Farm, S3). Same reader Farm keeps.
  const [adminPhone, setAdminPhone] = useState<AdminPhoneView | null>(null);
  const [pairing, setPairing] = useState<PairingView | null>(null);
  const [retiredLine, setRetiredLine] = useState<string | null>(null);
  const [pairBusy, setPairBusy] = useState(false);
  const [dock, setDock] = useState<DockPortView | null>(null);
  const [dockBusy, setDockBusy] = useState(false);

  // Connections (moved from Marketing, S4). Token is never re-shown.
  const [cfg, setCfg] = useState<ScanConfigView | null>(null);
  const [scanUrl, setScanUrl] = useState("");
  const [scanToken, setScanToken] = useState("");
  const [connectionsSaved, setConnectionsSaved] = useState(false);
  const [connectionsBlank, setConnectionsBlank] = useState(false);
  const [connectionsBusy, setConnectionsBusy] = useState(false);

  // Backup & Export (door to the existing sheet, S5).
  const [backupOpen, setBackupOpen] = useState(false);
  const [lastBackupAt, setLastBackupAt] = useState<Date | null>(null);
  const [restoredLabel, setRestoredLabel] = useState<string | null>(null);

  async function loadSpace() {
    const space = await shelfCapacity();
    setLightSlots(space.lightSlots == null ? "" : String(space.lightSlots));
    setBlackoutSlots(
      space.blackoutSlots == null ? "" : String(space.blackoutSlots),
    );
  }

  async function loadPhone() {
    setAdminPhone(await adminPhoneStatus());
  }

  async function loadDock() {
    setDock(await dockPortStatus());
  }

  async function loadConnections() {
    const next = await scanConfig();
    setCfg(next);
    setScanUrl(next.endpointUrl ?? "");
    setScanToken("");
  }

  async function loadBackupLine() {
    const loc = await farmLocation();
    setLastBackupAt(loc.lastSnapshotAt ? new Date(loc.lastSnapshotAt) : null);
  }

  async function loadAll() {
    // Each section loads on its own so one failure cannot blank the others.
    try { await loadSpace(); } catch (err) { console.error(err); }
    try { await loadPhone(); } catch (err) { setAdminPhone(null); console.error(err); }
    try { await loadDock(); } catch (err) { setDock(null); console.error(err); }
    try { await loadConnections(); } catch (err) { setCfg(null); console.error(err); }
    try { await loadBackupLine(); } catch (err) { console.error(err); }
  }

  useEffect(() => {
    void loadAll();
  }, []);

  async function handleSaveSpace() {
    const light = slotsFieldToValue(lightSlots);
    const blackout = slotsFieldToValue(blackoutSlots);
    if (light !== null && !Number.isFinite(light)) {
      setError("slots must be a number");
      return;
    }
    if (blackout !== null && !Number.isFinite(blackout)) {
      setError("slots must be a number");
      return;
    }
    setSpaceBusy(true);
    setError(null);
    setSpaceSaved(false);
    try {
      const next = await setShelfCapacity(light, blackout);
      setLightSlots(next.lightSlots == null ? "" : String(next.lightSlots));
      setBlackoutSlots(
        next.blackoutSlots == null ? "" : String(next.blackoutSlots),
      );
      setSpaceSaved(true);
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setSpaceBusy(false);
    }
  }

  async function handlePair() {
    setPairBusy(true);
    setError(null);
    setRetiredLine(null);
    try {
      const p = await pairAdminPhone();
      setPairing(p);
      await loadPhone();
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setPairBusy(false);
    }
  }

  async function handleDockStart() {
    setDockBusy(true);
    setError(null);
    try {
      setDock(await dockPortStart());
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setDockBusy(false);
    }
  }

  async function handleDockStop() {
    setDockBusy(true);
    setError(null);
    try {
      setDock(await dockPortStop());
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setDockBusy(false);
    }
  }

  async function handleCopyLink() {
    if (!pairing?.link) return;
    try {
      await navigator.clipboard.writeText(pairing.link);
    } catch (err) {
      setError(errMessage(err));
    }
  }

  // Option A: the token beside the link, both alive only while `pairing` is.
  // Nothing stored it, so there is nothing to copy for a phone paired earlier.
  async function handleCopyToken() {
    if (!pairing) return;
    try {
      await navigator.clipboard.writeText(pairing.token);
    } catch (err) {
      setError(errMessage(err));
    }
  }

  async function handleRetire() {
    setPairBusy(true);
    setError(null);
    try {
      const r = await retireAdminPhone();
      setRetiredLine(r.line);
      setPairing(null);
      await loadPhone();
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setPairBusy(false);
    }
  }

  async function handleSaveConnections() {
    setConnectionsBusy(true);
    setError(null);
    setConnectionsSaved(false);
    setConnectionsBlank(false);
    try {
      const url = scanUrl.trim();
      const token = scanToken.trim();
      // Same semantics Marketing had: a blank field means "leave as is".
      await setScanConfig({
        endpointUrl: url ? url : null,
        pullToken: token ? token : null,
      });
      await loadConnections();
      // Copy-only: both blank sent nothing, so do not claim a save.
      if (!url && !token) {
        setConnectionsBlank(true);
      } else {
        setConnectionsSaved(true);
      }
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setConnectionsBusy(false);
    }
  }

  async function handleRestored(label: string) {
    setRestoredLabel(label);
    try {
      await loadAll();
    } catch (err) {
      console.error(err);
    }
  }

  const backupLine = lastBackupAt
    ? `Farm saved automatically · last backup ${snapshotLabel(lastBackupAt)}`
    : "Farm saved automatically · last backup —";

  return (
    <main className="mx-auto flex w-full max-w-md flex-col gap-8 px-6 py-8">
      <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>
      <p className="text-sm text-muted-foreground">
        How this farm is configured and governed. Nothing here is a daily
        action, and nothing here raises an alarm — problems still surface on
        Health.
      </p>
      <ErrorLine message={error} />

      <Card>
        <CardHeader>
          <CardTitle>Trust &amp; Ownership</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-base">
            This software is free and open source, licensed Apache-2.0.
          </p>
          <p className="text-base">
            You own the data folder on this machine. No farm truth lives anywhere
            else.
          </p>
          <p className="text-base">
            You can export everything and leave at any time. Backup &amp; Export,
            below, is the door.
          </p>
          <p className="text-base font-medium">This system will never:</p>
          <ul className="ml-5 flex list-disc flex-col gap-1 text-base">
            <li>let a second writer touch farm truth</li>
            <li>hide the oldest unpaid debt</li>
            <li>accept a soft number</li>
            <li>let history be rewritten</li>
          </ul>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Farm — Shelf space</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-sm text-muted-foreground">
            How many trays fit under the lights, and how many in blackout. Leave
            blank if you have not counted — blank means unknown, not zero.
          </p>
          <label className="flex flex-col gap-2">
            <span className="text-sm text-muted-foreground">Light slots</span>
            <input
              type="text"
              inputMode="numeric"
              value={lightSlots}
              onChange={(e) => {
                setLightSlots(e.target.value);
                setSpaceSaved(false);
              }}
              aria-label="Light slots"
              className={`${FIELD} text-center text-2xl tabular-nums`}
            />
          </label>
          <label className="flex flex-col gap-2">
            <span className="text-sm text-muted-foreground">Blackout slots</span>
            <input
              type="text"
              inputMode="numeric"
              value={blackoutSlots}
              onChange={(e) => {
                setBlackoutSlots(e.target.value);
                setSpaceSaved(false);
              }}
              aria-label="Blackout slots"
              className={`${FIELD} text-center text-2xl tabular-nums`}
            />
          </label>
          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="button"
              className="h-12 px-6 text-base active:translate-y-px"
              disabled={spaceBusy}
              onClick={() => void handleSaveSpace()}
            >
              Save
            </Button>
            {spaceSaved && <p className="text-sm text-muted-foreground">Saved.</p>}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Field terminal</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-sm text-muted-foreground">{FIELD_TERMINAL_COPY}</p>
          <p className="text-sm">{adminPhone?.status ?? ""}</p>
          <p className="text-sm text-muted-foreground">Mode: Solo</p>
          {/* FARM-PAIR Job 2 — the minted token is a receipt, not a pair state. It
              renders only while the live reader says the farm is paired, so a token
              can never sit above the unpaired sentence printed at the status line. */}
          {adminPhone?.paired && pairing && (
            <>
              <p className="text-sm">{pairing.pairingText}</p>
              {pairing.link && (
                <p className="text-sm break-all">{pairing.link}</p>
              )}
              <p className="font-mono text-sm break-all select-all">{pairing.token}</p>
            </>
          )}
          {retiredLine && <p className="text-sm">{retiredLine}</p>}
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              variant="outline"
              className="h-11 px-4 active:translate-y-px"
              disabled={pairBusy}
              onClick={() => void handlePair()}
            >
              Pair the Admin phone
            </Button>
            {adminPhone?.paired && pairing?.link && (
              <Button
                type="button"
                variant="outline"
                className="h-11 px-4 active:translate-y-px"
                disabled={pairBusy}
                onClick={() => void handleCopyLink()}
              >
                Copy link
              </Button>
            )}
            {adminPhone?.paired && pairing && (
              <Button
                type="button"
                variant="outline"
                className="h-11 px-4 active:translate-y-px"
                disabled={pairBusy}
                onClick={() => void handleCopyToken()}
              >
                Copy token
              </Button>
            )}
            {adminPhone?.paired && (
              <Button
                type="button"
                variant="ghost"
                className="h-11 px-4 active:translate-y-px"
                disabled={pairBusy}
                onClick={() => void handleRetire()}
              >
                Retire the Admin phone
              </Button>
            )}
          </div>
          <p className="text-sm text-muted-foreground">
            Phone captures are confirmed on Farm.
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Dock</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-sm">{dock?.running ? "Running." : "Off."}</p>
          <p className="text-sm text-muted-foreground">{DOCK_HOW_COPY}</p>
          {dock?.reachUrl ? (
            <>
              <p className="text-sm break-all select-all">{dock.reachUrl}</p>
              {(() => {
                const qr = dock.reachQr;
                if (!qr) return null;
                const side = qr.length + 8;
                return (
                  <svg
                    role="img"
                    aria-label="QR code for the Dock address"
                    width={200}
                    height={200}
                    viewBox={`0 0 ${side} ${side}`}
                    shapeRendering="crispEdges"
                    className="self-start"
                  >
                    <rect x={0} y={0} width={side} height={side} fill="#fff" />
                    {qr.map((row, y) =>
                      row.map((dark, x) =>
                        dark ? <rect key={`${y}-${x}`} x={x + 4} y={y + 4} width={1} height={1} fill="#000" /> : null,
                      ),
                    )}
                  </svg>
                );
              })()}
            </>
          ) : dock?.port != null ? (
            <p className="text-sm">Port {dock.port}</p>
          ) : null}
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              variant="outline"
              className="h-11 px-4 active:translate-y-px"
              disabled={dockBusy}
              onClick={() => void (dock?.running ? handleDockStop() : handleDockStart())}
            >
              {dock?.running ? "Stop" : "Start"}
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Connections</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-sm text-muted-foreground">{CONNECTIONS_COPY}</p>
          <label className="flex flex-col gap-1 text-sm">
            Scan endpoint URL
            <input
              type="url"
              className="border border-border bg-card px-3 py-2"
              value={scanUrl}
              onChange={(e) => {
                setScanUrl(e.target.value);
                setConnectionsSaved(false);
                setConnectionsBlank(false);
              }}
            />
          </label>
          <label className="flex flex-col gap-1 text-sm">
            Pull token ({cfg?.tokenSet ? "token set" : "no token"})
            <input
              type="password"
              className="border border-border bg-card px-3 py-2"
              value={scanToken}
              autoComplete="off"
              onChange={(e) => {
                setScanToken(e.target.value);
                setConnectionsSaved(false);
                setConnectionsBlank(false);
              }}
            />
          </label>
          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="button"
              variant="outline"
              disabled={connectionsBusy}
              onClick={() => void handleSaveConnections()}
            >
              Save
            </Button>
            {connectionsSaved && (
              <p className="text-sm text-muted-foreground">Saved.</p>
            )}
            {connectionsBlank && (
              <p className="text-sm text-muted-foreground">{CONNECTIONS_BLANK}</p>
            )}
          </div>
          {cfg?.configuredAt && (
            <p className="font-mono text-xs text-muted-foreground">
              configured_at: {cfg.configuredAt}
            </p>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Backup &amp; Export</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-sm text-muted-foreground">{backupLine}</p>
          {restoredLabel && (
            <p className="text-sm">Farm restored from {restoredLabel}.</p>
          )}
          <Button
            type="button"
            variant="outline"
            className="h-12 self-start text-base active:translate-y-px"
            onClick={() => setBackupOpen(true)}
          >
            Open Backup &amp; Export
          </Button>
          <p className="text-sm text-muted-foreground">
            The farm folder and Open folder, the snapshots and Restore, I moved
            computers, Bring in a bundle, and Export everything.
          </p>
        </CardContent>
      </Card>
      <FarmNameSection />
      <FarmBackupSheet
        open={backupOpen}
        onOpenChange={setBackupOpen}
        onRestored={(label) => void handleRestored(label)}
        recoveryMode={false}
        onImported={() => {
          void loadAll();
        }}
      />
    </main>
  );
}
