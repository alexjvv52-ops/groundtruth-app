import { useEffect, useState } from "react";
import { dockFolds, healthStatus, listVenues, runFullVerify, todayView } from "@/farm/api";
import type { CheckStatus, DockFoldsView } from "@/farm/types";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  evidenceText,
  recoveryFor,
  SCAN_CLASSES,
  SCAN_CLEAN_SENTENCE,
  SCOPE_BLURB,
  SCOPE_TITLE,
  type HealthScope,
} from "@/farm/healthScopes";

type Props = {
  onStatusesChange?: (statuses: CheckStatus[]) => void;
};

type ScanResult = {
  at: string;
  verify: CheckStatus;
  problems: CheckStatus[];
  clean: boolean;
};

export function Health({ onStatusesChange }: Props) {
  const [statuses, setStatuses] = useState<CheckStatus[]>([]);
  const [folds, setFolds] = useState<DockFoldsView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [firstRun, setFirstRun] = useState<boolean | null>(null);
  /**
   * Which scopes have their healthy checks expanded. Collapsed on every open: a
   * Healthy check is the system saying there is nothing to do, and it must not make
   * the operator scroll past it to reach one that is raising. Raising checks are
   * never behind this — they render directly, with their recovery box, as before.
   */
  const [healthyOpen, setHealthyOpen] = useState<Record<HealthScope, boolean>>({
    farm: false,
    system: false,
  });

  async function load() {
    const next = await healthStatus();
    setStatuses(next);
    onStatusesChange?.(next);
    try {
      setFolds(await dockFolds());
    } catch {
      // folds stays null until the first answer — never a guessed default
    }
  }

  async function probeFirstRun() {
    try {
      const [v, venues] = await Promise.all([todayView(), listVenues()]);
      setFirstRun(v.activeTrayCount === 0 && venues.length === 0);
    } catch {
      setFirstRun(null); // claim nothing
    }
  }

  useEffect(() => {
    void load().catch((e: unknown) =>
      setError(e instanceof Error ? e.message : String(e)),
    );
    void probeFirstRun();
  }, []);

  async function onScan() {
    setBusy(true);
    setError(null);
    setScan(null);
    try {
      const verify = await runFullVerify();
      const next = await healthStatus();
      setStatuses(next);
      onStatusesChange?.(next);
      try {
        setFolds(await dockFolds());
      } catch {
        // keep the last folds; never invent a title or scope
      }
      const problems = next.filter((s) => s.severity !== "Healthy");
      setScan({
        at: verify.ranAt ?? new Date().toISOString(),
        verify,
        problems,
        clean: verify.severity === "Healthy" && problems.length === 0,
      });
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  function severityClass(s: CheckStatus): string {
    return s.severity === "Unhealthy"
      ? "text-sm text-red-700"
      : s.severity === "Degraded"
        ? "text-sm text-amber-700"
        : "text-sm text-muted-foreground";
  }

  function renderCheck(s: CheckStatus) {
    const recovery = recoveryFor(s.checkId, s.severity, s.sentence);
    const title = folds?.checks.find((c) => c.checkId === s.checkId)?.title;
    return (
      <section key={s.checkId} className="flex flex-col gap-2 border-b border-border pb-4">
        <h3 className="text-base font-medium">
          {title}{" "}
          <span className="font-mono text-xs text-muted-foreground">{s.checkId}</span>
        </h3>
        <p className="text-base">{s.sentence}</p>
        <p className={severityClass(s)}>severity: {s.severity}</p>
        <p className="font-mono text-xs text-muted-foreground">
          ran_at: {s.ranAt ?? "never reported"}
        </p>
        {recovery && (
          <div
            role={recovery.forced ? "alert" : undefined}
            className={
              recovery.forced
                ? "flex flex-col gap-2 rounded-md border border-red-300 bg-red-50 p-3"
                : "flex flex-col gap-2 rounded-md border border-amber-300 p-3"
            }
          >
            <p className="text-sm font-medium">
              {recovery.forced ? "Do this now" : "What to do"}
            </p>
            {recovery.forced ? (
              <ol className="ml-5 flex list-decimal flex-col gap-1 text-sm">
                {recovery.steps.map((t) => (
                  <li key={t}>{t}</li>
                ))}
              </ol>
            ) : (
              <ul className="ml-5 flex list-disc flex-col gap-1 text-sm">
                {recovery.steps.map((t) => (
                  <li key={t}>{t}</li>
                ))}
              </ul>
            )}
          </div>
        )}
      </section>
    );
  }

  function renderScope(scope: HealthScope) {
    if (!folds) return null;
    const inScope = new Set(
      folds.checks.filter((c) => c.scope === scope).map((c) => c.checkId),
    );
    const rows = statuses.filter((s) => inScope.has(s.checkId));
    const unhealthy = rows.filter((s) => s.severity === "Unhealthy").length;
    const degraded = rows.filter((s) => s.severity === "Degraded").length;
    const worst = folds.byScope[scope];
    // Raising first, healthy behind one control. Each group keeps its REPORTED_CHECKS
    // order (health.rs:107-108, nine checks, H1 not reported per ruling 6.1). Derived
    // per render, so a check that changes severity after a System Scan moves between
    // the groups on its own.
    const raising = rows.filter((s) => s.severity !== "Healthy");
    const healthy = rows.filter((s) => s.severity === "Healthy");
    const healthyShown = healthyOpen[scope];
    const count =
      rows.length === 0
        ? "No checks reported."
        : unhealthy + degraded === 0
          ? `${rows.length} checks, read just now. None raising.`
          : `${unhealthy} unhealthy · ${degraded} degraded, of ${rows.length} read just now.`;
    return (
      <Card>
        <CardHeader>
          <CardTitle>{SCOPE_TITLE[scope]}</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="flex flex-col gap-1">
            <p className="text-sm text-muted-foreground">{SCOPE_BLURB[scope]}</p>
            <p
              className={
                worst === "Unhealthy"
                  ? "text-sm font-medium text-red-700"
                  : worst === "Degraded"
                    ? "text-sm font-medium text-amber-700"
                    : "text-sm font-medium"
              }
            >
              {count}
            </p>
          </div>
          {raising.map(renderCheck)}
          {healthy.length > 0 && (
            <button
              type="button"
              aria-expanded={healthyShown}
              onClick={() =>
                setHealthyOpen((m) => ({ ...m, [scope]: !m[scope] }))
              }
              className="flex min-h-11 items-center gap-2 text-left text-sm text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            >
              <span aria-hidden="true">{healthyShown ? "▾" : "▸"}</span>
              {healthy.length} healthy {healthy.length === 1 ? "check" : "checks"}
            </button>
          )}
          {healthyShown && healthy.map(renderCheck)}
        </CardContent>
      </Card>
    );
  }

  const anyProblem = statuses.some((s) => s.severity !== "Healthy");
  const showDiagnosis = anyProblem || scan !== null;

  return (
    <main className="mx-auto flex w-full max-w-md flex-col gap-8 px-6 py-8">
      <h1 className="text-3xl font-semibold tracking-tight">Health</h1>
      <p className="text-sm text-muted-foreground">
        Status is computed when you look. Timestamps are raw so you can check the
        machine&apos;s claims by eye.
      </p>

      {firstRun === true && (
        <div className="flex flex-col gap-2 rounded-md border p-4">
          <p className="text-base font-medium">
            Nothing has been recorded on this farm yet.
          </p>
          <p className="text-sm text-muted-foreground">
            Health is the surface that tells you when this app&apos;s record and
            your real farm have drifted apart. Some checks below will say they
            haven&apos;t reported yet — that is not damage. It is Health saying it
            has not seen the evidence it needs. They clear on their own as you
            record work and as the app opens and closes.
          </p>
        </div>
      )}

      {renderScope("farm")}
      {renderScope("system")}

      <Card>
        <CardHeader>
          <CardTitle>System Scan</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-sm text-muted-foreground">
            Runs only when you ask. It replays the event log against the database and
            re-reads every check.
          </p>
          <Button
            type="button"
            className="h-12 text-base"
            disabled={busy}
            onClick={() => void onScan()}
          >
            {busy ? "Scanning…" : "Run System Scan"}
          </Button>

          {scan && (
            <div className="flex flex-col gap-3">
              <p className="font-mono text-xs text-muted-foreground">
                scanned_at: {scan.at}
              </p>
              <p
                role={scan.verify.severity === "Unhealthy" ? "alert" : undefined}
                className={
                  scan.verify.severity === "Unhealthy"
                    ? "text-sm text-red-700"
                    : scan.verify.severity === "Degraded"
                      ? "text-sm text-amber-700"
                      : "text-sm"
                }
              >
                {scan.verify.sentence}
              </p>

              {scan.clean ? (
                <>
                  <p className="text-base font-medium">{SCAN_CLEAN_SENTENCE}</p>
                  <p className="text-sm text-muted-foreground">
                    That is not the same as &quot;everything is fine&quot;. It is this
                    list, looked at just now, with nothing found:
                  </p>
                  <ul className="ml-5 flex list-disc flex-col gap-1 text-sm text-muted-foreground">
                    {SCAN_CLASSES.map((c) => (
                      <li key={c}>{c}</li>
                    ))}
                  </ul>
                </>
              ) : (
                <>
                  <p className="text-base font-medium">
                    {scan.problems.length === 0
                      ? "The replay itself is the finding — read the sentence above."
                      : `${scan.problems.length} check${scan.problems.length === 1 ? "" : "s"} raising. Each one is above, with its next steps.`}
                  </p>
                  <ul className="ml-5 flex list-disc flex-col gap-1 text-sm">
                    {scan.problems.map((p) => (
                      <li key={p.checkId}>
                        {p.checkId} — {p.severity}
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </div>
          )}
        </CardContent>
      </Card>

      {showDiagnosis && (
        <div className="flex flex-col gap-3 rounded-md border p-4">
          <h2 className="text-xl font-semibold">Take this further</h2>
          <p className="text-sm">
            Copy the block below and paste it into whichever AI tool you already
            use. Ask it what the sentences mean and what to check next.
          </p>
          <p className="text-sm text-muted-foreground">
            Two rules. The AI has no access to your farm and must never be given
            any — it reads this text and nothing else. And nothing it says changes
            a number here: this app stays the only writer of farm truth.
          </p>
          <pre className="max-h-64 overflow-auto rounded bg-muted p-3 font-mono text-xs whitespace-pre-wrap select-all">
            {evidenceText(statuses, scan ? { at: scan.at, verify: scan.verify } : null)}
          </pre>
          <p className="text-sm text-muted-foreground">
            If a check names a file (last-verify-replay.txt, events.jsonl), open the
            farm folder and take that file too.
          </p>
        </div>
      )}

      {error && (
        <p className="text-sm text-red-700" role="alert">
          {error}
        </p>
      )}
    </main>
  );
}
