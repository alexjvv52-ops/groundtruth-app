import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type {
  ExportResult,
  FarmLocation,
  ImportPlan,
  ImportResult,
  SnapshotInfo,
} from "@/farm/types";
import {
  applyImport,
  exportBundle,
  farmLocation,
  listSnapshots,
  openExportFolder,
  openFarmFolder,
  previewImport,
  restoreSnapshot,
} from "@/farm/api";
import { snapshotLabel } from "@/farm/dates";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";

type FarmBackupSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onRestored: (label: string) => void;
  onImported?: () => void;
  /** True when Today's no-records probe found zero tray and venue records
   *  and the operator has not chosen Start fresh. */
  recoveryMode?: boolean;
};

type ConfirmTarget = {
  path: string;
  label: string;
  /** When true, confirm copy refers to a picked file rather than a dated snapshot. */
  fromFile?: boolean;
};

export function FarmBackupSheet({
  open,
  onOpenChange,
  onRestored,
  onImported,
  recoveryMode = false,
}: FarmBackupSheetProps) {
  const [location, setLocation] = useState<FarmLocation | null>(null);
  const [snapshots, setSnapshots] = useState<SnapshotInfo[]>([]);
  const [confirm, setConfirm] = useState<ConfirmTarget | null>(null);
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [exportResult, setExportResult] = useState<ExportResult | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);
  const [importPlan, setImportPlan] = useState<ImportPlan | null>(null);
  const [importResult, setImportResult] = useState<ImportResult | null>(null);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [showAllSnapshots, setShowAllSnapshots] = useState(false);

  async function load() {
    try {
      const [loc, snaps] = await Promise.all([farmLocation(), listSnapshots()]);
      setLocation(loc);
      setSnapshots(snaps);
      setError(null);
    } catch (err) {
      console.error(err);
    }
  }

  useEffect(() => {
    if (!open) return;
    setConfirm(null);
    setError(null);
    setExportResult(null);
    setExportError(null);
    setExporting(false);
    setImportPlan(null);
    setImportResult(null);
    setImportError(null);
    setImporting(false);
    setShowAllSnapshots(false);
    void load();
  }, [open]);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setConfirm(null);
      setError(null);
      setExportResult(null);
      setExportError(null);
      setExporting(false);
      setImportPlan(null);
      setImportResult(null);
      setImportError(null);
      setImporting(false);
      setShowAllSnapshots(false);
    }
    onOpenChange(next);
  }

  async function handleExport() {
    if (exporting) return;
    setExporting(true);
    setExportError(null);
    try {
      const result = await exportBundle();
      setExportResult(result);
    } catch (err) {
      const message =
        typeof err === "string"
          ? err
          : err instanceof Error
            ? err.message
            : "Export failed.";
      setExportError(message);
      console.error(err);
    } finally {
      setExporting(false);
    }
  }

  async function handleRestore() {
    if (!confirm || restoring) return;
    setRestoring(true);
    setError(null);
    try {
      await restoreSnapshot(confirm.path);
      const label = confirm.label;
      setConfirm(null);
      onOpenChange(false);
      onRestored(label);
    } catch (err) {
      const message =
        typeof err === "string"
          ? err
          : err instanceof Error
            ? err.message
            : "Restore failed.";
      setError(message);
      console.error(err);
    } finally {
      setRestoring(false);
    }
  }

  async function handleMovedComputers() {
    setError(null);
    try {
      const selected = await openDialog({
        multiple: false,
        filters: [
          { name: "Farm database", extensions: ["db"] },
          { name: "All files", extensions: ["*"] },
        ],
      });
      if (selected === null) return;
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (!path) return;
      const name = path.split(/[/\\]/).pop() ?? path;
      const fromName = snapshotLabelFromFileName(name);
      if (fromName) {
        setConfirm({ path, label: fromName });
      } else {
        setConfirm({ path, label: "this file", fromFile: true });
      }
    } catch (err) {
      console.error(err);
      setError("Could not open the file picker.");
    }
  }

  async function handleBringInBundle() {
    setImportError(null);
    setImportResult(null);
    try {
      const selected = await openDialog({
        multiple: false,
        filters: [{ name: "Bundle manifest", extensions: ["json"] }],
      });
      if (selected === null) return;
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (!path) return;
      const name = path.split(/[/\\]/).pop() ?? path;
      if (name !== "manifest.json") {
        setImportError("Pick the manifest.json inside the bundle folder.");
        return;
      }
      const bundleDir = path.replace(/[/\\][^/\\]+$/, "");
      const plan = await previewImport(bundleDir);
      setImportPlan(plan);
    } catch (err) {
      const message =
        typeof err === "string"
          ? err
          : err instanceof Error
            ? err.message
            : "Could not read that bundle.";
      setImportError(message);
      console.error(err);
    }
  }

  async function handleApplyImport() {
    if (!importPlan || !importPlan.canApply || importing) return;
    setImporting(true);
    setImportError(null);
    try {
      const result = await applyImport(importPlan.bundlePath);
      setImportResult(result);
      setImportPlan(null);
      onImported?.();
    } catch (err) {
      const message =
        typeof err === "string"
          ? err
          : err instanceof Error
            ? err.message
            : "Import failed.";
      setImportError(message);
      console.error(err);
    } finally {
      setImporting(false);
    }
  }

  // Settings fence 1 (S5): the "Bring in Farm OS at cutover" door is gone.
  // Cutover is closed (identity::CUTOVER_LIVE = true); "Bring in a bundle" is
  // the one import door. The apply_cutover_import command is untouched.

  const folderBlock = (
    <>
      <div className="flex flex-col gap-3">
        <p className="break-all font-mono text-sm text-muted-foreground">
          {location?.folderPath ?? "…"}
        </p>
        <Button
          type="button"
          variant="outline"
          className="h-12 self-start text-base"
          onClick={() => {
            void openFarmFolder().catch((err) => {
              console.error(err);
            });
          }}
        >
          Open folder
        </Button>
      </div>
      <p className="text-sm text-muted-foreground">
        Backups are on this computer. To protect against losing the
        computer itself, copy this folder to a USB stick or a cloud
        folder.
      </p>
    </>
  );

  const exportBlock = (
    <div className="flex flex-col gap-3">
      <p className="text-base font-medium">Take everything with you</p>
      <p className="text-sm text-muted-foreground">
        Everything the farm knows, in one folder you own — the
        database, the full log, your receipts, and your costs ready
        for whoever does your taxes. Works with the internet off. No
        account, no fee.
      </p>
      {exportResult ? (
        <div className="flex flex-col gap-3">
          <p className="text-sm">
            Exported {exportResult.fileCount} files to{" "}
            <span className="break-all font-mono">
              {exportResult.bundlePath}
            </span>
          </p>
          <Button
            type="button"
            variant="ghost"
            className="h-12 text-base"
            onClick={() => {
              void openExportFolder(exportResult.bundlePath).catch(
                (err) => {
                  console.error(err);
                },
              );
            }}
          >
            Open folder
          </Button>
          <Button
            type="button"
            variant="ghost"
            className="h-12 text-base"
            onClick={() => {
              setExportResult(null);
              setExportError(null);
            }}
          >
            Export again
          </Button>
        </div>
      ) : (
        <Button
          type="button"
          variant={recoveryMode ? "outline" : "default"}
          className={
            recoveryMode
              ? "h-12 self-start text-base"
              : "h-14 w-full text-base"
          }
          disabled={exporting}
          onClick={() => void handleExport()}
        >
          {exporting ? "Exporting…" : "Export everything"}
        </Button>
      )}
      {exportError && (
        <p className="text-sm text-destructive" role="alert">
          {exportError}
        </p>
      )}
    </div>
  );

  return (
    <Sheet open={open} onOpenChange={handleOpenChange}>
      <SheetContent
        side="bottom"
        showCloseButton={false}
        aria-describedby={undefined}
        className="max-h-[85vh] overflow-y-auto"
      >
        <div className="mx-auto flex w-full max-w-md flex-col gap-6 p-4 pb-8">
          {confirm ? (
            <>
              <SheetTitle className="text-xl font-medium leading-snug">
                {confirm.fromFile
                  ? "Restore the farm from this file?"
                  : `Restore the farm as it was ${confirm.label}?`}
              </SheetTitle>
              <div className="flex flex-col gap-3">
                <Button
                  type="button"
                  className="h-14 text-base"
                  disabled={restoring}
                  onClick={handleRestore}
                >
                  Restore
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  className="h-12 text-base"
                  disabled={restoring}
                  onClick={() => {
                    setConfirm(null);
                    setError(null);
                  }}
                >
                  Back
                </Button>
              </div>
              <p className="text-sm text-muted-foreground">
                Your farm right now will be saved first, so this can be undone.
              </p>
              {error && (
                <p className="text-sm text-destructive">{error}</p>
              )}
            </>
          ) : (
            <>
              <SheetTitle className="text-xl font-medium">
                {recoveryMode
                  ? "Restore this machine"
                  : "Where your farm lives"}
              </SheetTitle>
              {recoveryMode ? (
                <p className="text-sm text-muted-foreground">
                  No tray or venue records on this machine. If they should exist,
                  restore a backup made by this app — Farm OS files and other apps'
                  databases are refused. Brand-new farm with nothing to restore?
                  Close this and start fresh; no backup is needed.
                </p>
              ) : (
                folderBlock
              )}
              {!recoveryMode && exportBlock}
              {recoveryMode && (
                <p className="text-sm text-muted-foreground">
                  A snapshot is taken every launch, so the newest ones here may be
                  snapshots of this empty farm. Sizes help tell them apart.
                </p>
              )}
              <div className="flex flex-col gap-1">
                {(showAllSnapshots ? snapshots : snapshots.slice(0, 7)).map(
                  (snap) => {
                    const label = snapshotLabel(new Date(snap.takenAt));
                    return (
                      <button
                        key={snap.path}
                        type="button"
                        onClick={() =>
                          setConfirm({ path: snap.path, label })
                        }
                        className="rounded-md px-2 py-3 text-left text-base transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                      >
                        {recoveryMode
                          ? `${label} · ${Math.max(1, Math.round(snap.sizeBytes / 1024))} KB`
                          : label}
                      </button>
                    );
                  },
                )}
                {!showAllSnapshots && snapshots.length > 7 && (
                  <button
                    type="button"
                    onClick={() => setShowAllSnapshots(true)}
                    className="self-start px-2 py-3 text-left text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  >
                    Show older backups ({snapshots.length - 7})
                  </button>
                )}
                {snapshots.length === 0 && (
                  <p className="text-sm text-muted-foreground">
                    No backups yet.
                  </p>
                )}
              </div>
              {recoveryMode ? (
                <Button
                  type="button"
                  className="h-14 w-full text-base"
                  onClick={() => void handleMovedComputers()}
                >
                  Restore from a backup file
                </Button>
              ) : (
                <button
                  type="button"
                  onClick={() => void handleMovedComputers()}
                  className="mt-2 self-start text-sm text-muted-foreground underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  I moved computers
                </button>
              )}
              {importPlan ? (
                <div className="flex flex-col gap-3">
                  <p className="text-sm">
                    {importPlan.eventsInBundle} events in this bundle, exported{" "}
                    {importPlan.bundleExportedAt}
                  </p>
                  <p className="text-sm">
                    {importPlan.sharedEventIds} of them are already in this farm
                  </p>
                  <p className="text-sm">
                    {importPlan.wouldBeAdded} would be added
                  </p>
                  {importPlan.foreignRecordsInBundle > 0 && (
                    <p className="text-sm">
                      {importPlan.foreignRecordsInBundle} from another system —
                      these come in marked and are never counted in your totals
                    </p>
                  )}
                  {importPlan.explanations.length > 0 && (
                    <div className="flex flex-col gap-2" role="alert">
                      {importPlan.explanations.map((line, i) => (
                        <p key={i} className="text-sm text-destructive">
                          {line}
                        </p>
                      ))}
                    </div>
                  )}
                  {importPlan.canApply ? (
                    <div className="flex flex-col gap-3">
                      <Button
                        type="button"
                        className="h-14 text-base"
                        disabled={importing}
                        onClick={() => void handleApplyImport()}
                      >
                        {importing ? "Bringing in…" : "Bring it in"}
                      </Button>
                      <Button
                        type="button"
                        variant="ghost"
                        className="h-12 text-base"
                        disabled={importing}
                        onClick={() => {
                          setImportPlan(null);
                          setImportError(null);
                        }}
                      >
                        Cancel
                      </Button>
                    </div>
                  ) : (
                    <Button
                      type="button"
                      variant="ghost"
                      className="h-12 text-base"
                      onClick={() => {
                        setImportPlan(null);
                        setImportError(null);
                      }}
                    >
                      Back
                    </Button>
                  )}
                </div>
              ) : importResult ? (
                <div className="flex flex-col gap-3">
                  <p className="text-sm">
                    Brought in {importResult.eventsAdded} events
                    {importResult.eventsSkippedIdentical > 0
                      ? `, skipped ${importResult.eventsSkippedIdentical} already present`
                      : ""}
                    {importResult.foreignRecordsAdded > 0
                      ? `, ${importResult.foreignRecordsAdded} from another system`
                      : ""}
                    .
                  </p>
                  <Button
                    type="button"
                    variant="ghost"
                    className="h-12 self-start text-base"
                    onClick={() => setImportResult(null)}
                  >
                    Done
                  </Button>
                </div>
              ) : (
                <div className="flex flex-col gap-3 self-start">
                  <Button
                    type="button"
                    variant="secondary"
                    className="h-12 self-start text-base"
                    onClick={() => void handleBringInBundle()}
                  >
                    Bring in a bundle
                  </Button>
                </div>
              )}
              {recoveryMode && folderBlock}
              {recoveryMode && exportBlock}
              {importError && (
                <p className="text-sm text-destructive" role="alert">
                  {importError}
                </p>
              )}
              {error && (
                <p className="text-sm text-destructive">{error}</p>
              )}
            </>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}

function snapshotLabelFromFileName(name: string): string | null {
  // farm-YYYY-MM-DD-HHMMSS.db
  const m = /^farm-(\d{4})-(\d{2})-(\d{2})-(\d{2})(\d{2})(\d{2})(?:-\d+)?\.db$/.exec(
    name,
  );
  if (!m) return null;
  const [, y, mo, d, h, mi, s] = m;
  const date = new Date(
    Number(y),
    Number(mo) - 1,
    Number(d),
    Number(h),
    Number(mi),
    Number(s),
  );
  return snapshotLabel(date);
}
