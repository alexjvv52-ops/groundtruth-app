import type { CheckStatus, DockFoldsView } from "@/farm/types";

type Props = {
  statuses: CheckStatus[] | null;
  folds: DockFoldsView | null;
  onOpenHealth?: () => void;
};

/**
 * Shell header mark on every screen.
 * Healthy: the life-green dot. Degraded: one amber sentence in a quiet tinted container.
 * Unhealthy: one red line with a severity dot, in a quiet tinted container — no dismiss control, and no loud filled
 * block. The shell states the failure; it does not own the first visual slot. Today
 * opens on the forced morning action and an integrity pointer must not outrank it.
 */
export function StatusMark({ statuses, folds, onOpenHealth }: Props) {
  if (statuses === null || folds === null) return null; // first fetch in flight — claim nothing

  // S2a: the PC owns the all-nine worst-of AND its absent state. An overall of null
  // is the one source of "no check has reported", so this sentence cannot drift from
  // what Health and the later dock port say. The rule is not repeated here.
  const worst = folds.overall;
  if (worst === null) {
    return (
      <p className="text-sm text-amber-700">
        Health checks haven&apos;t reported yet.
      </p>
    );
  }
  if (worst === "Healthy") {
    return (
      <span
        aria-label="Healthy"
        className="inline-block h-2 w-2 rounded-full bg-emerald-600"
      />
    );
  }

  const failed =
    statuses.find((s) => s.severity === worst) ?? statuses[0];
  // A failed refresh sets statuses to [] while folds keeps its last answer, so worst
  // can be non-null with no row to name. Claim nothing, the same rule as the guard
  // above, rather than render an undefined sentence.
  if (failed === undefined) return null;
  const others = statuses.filter(
    (s) => s.severity !== "Healthy" && s.checkId !== failed.checkId,
  ).length;
  const more =
    others > 0
      ? ` · ${others} more check${others === 1 ? "" : "s"} not healthy — open Health`
      : "";

  if (worst === "Degraded") {
    return (
      <button
        type="button"
        onClick={onOpenHealth}
        className="flex min-h-11 items-center rounded-r-md border-l-2 border-amber-500 bg-amber-50 px-3 py-2 text-left text-sm text-amber-700 underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        {failed.sentence}
        {more}
      </button>
    );
  }

  // Banner demotion. The signal is unchanged: same check, same sentence, same tap to
  // Health, same condition. What changed is altitude. This was a full-width filled and
  // bordered two-line red panel in the shell header, above every tab's h1 and above
  // Today's first card, which made an integrity pointer outrank the forced morning
  // action (surface audit, TOP 10 #10). min-h-11 keeps it a 44 px tap target now that
  // it is one line. `more` already ends in "— open Health" when other checks are
  // raising; when it is empty the invitation is appended so the tap is never unlabelled.
  return (
    <button
      type="button"
      onClick={onOpenHealth}
      className="flex min-h-11 items-center gap-2 rounded-r-md border-l-2 border-red-600 bg-red-50 px-3 py-2 text-left text-sm font-medium text-red-700 underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
    >
      <span
        aria-hidden="true"
        className="inline-block h-2.5 w-2.5 shrink-0 rounded-full bg-red-600"
      />
      <span>
        {failed.checkId} — {failed.sentence}
        {more || " — open Health"}
      </span>
    </button>
  );
}
