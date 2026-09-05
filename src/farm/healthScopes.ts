import type { CheckStatus, Severity } from "@/farm/types";

/**
 * Health's two sibling scopes, mirroring the Today / Reality split.
 * System = the ledger itself. Farm = the farm against the real world.
 * Grouping, titles and worst-of live on the PC (dock_folds). Titles, blurbs,
 * recovery and the scan sentences stay here — they are not folds.
 */
export type HealthScope = "farm" | "system";

export const SCOPE_TITLE: Record<HealthScope, string> = {
  farm: "Farm",
  system: "System",
};

export const SCOPE_BLURB: Record<HealthScope, string> = {
  farm: "What you owe, what you promised, what is on the shelf — the farm against the real world.",
  system: "The record itself: the event log, the backups, the replay, the correction trail.",
};

export type Recovery = { forced: boolean; steps: string[] };

/** C1 (INT-001, D3). The exact phrase every read-failure sentence carries.
 *  The Rust side holds the identical string in health::READ_FAILURE_PREFIX. */
export const READ_FAILURE_PREFIX = "could not read";

/** The H2/H3 scan step, unchanged, now one string in two places. */
const SYSTEM_SCAN_STEP =
  "If the sentence has not changed, run System Scan below and take the evidence block to the diagnosis step.";

/**
 * Severity-gated recovery (locked principle 2).
 * Unhealthy -> forced, exact next steps. Degraded -> honest guidance, no panic.
 * Every step names a verb that already exists in this app.
 */
export function recoveryFor(
  id: string,
  severity: Severity,
  sentence: string,
): Recovery | null {
  if (severity === "Healthy") return null;
  const forced = severity === "Unhealthy";

  // C1 (INT-001, D3). A check whose reader failed confirmed no debt, tray or
  // log fact, so the id's own steps would presume one. The scan step already
  // exists; it is reused verbatim and nothing new is said.
  if (sentence.includes(READ_FAILURE_PREFIX)) {
    return { forced, steps: [SYSTEM_SCAN_STEP] };
  }

  if (id === "M1") {
    return forced
      ? {
          forced,
          steps: [
            "Open the Money tab.",
            "Find the oldest order marked delivered.",
            "If its lines are unpriced, price them first — an unpriced order cannot be collected honestly.",
            "Tap Paid… when the money is actually in your hands, not before.",
            "If it will never be paid, void the order with a reason rather than leaving it open.",
          ],
        }
      : { forced, steps: ["Collect the oldest one on the Money tab."] };
  }
  if (id === "M2") {
    return forced
      ? {
          forced,
          steps: [
            "Open Today and read the COVER card for that date.",
            "If it says the date cannot be fixed by sowing, call the venue — nothing sown now reaches it.",
            "Then either shrink the promise (void the order on Money and re-record a smaller one) or deliver what you actually have.",
            "If it says there is no room, harvest early or void the order. The shelf, not the calendar, is the limit.",
          ],
        }
      : {
          forced,
          steps: [
            "Sow by the date the sentence names. The sow door is on Today's card and on Farm.",
          ],
        };
  }
  if (id === "M3") {
    return forced
      ? {
          forced,
          steps: [
            "Open the Money tab.",
            "Mark the order Delivered if it went out.",
            "If it did not go out, void it with a reason and tell the venue. A late promise left open is a lie in the register.",
          ],
        }
      : { forced, steps: ["Deliver it today, then mark it Delivered on Money."] };
  }
  if (id === "F1") {
    return {
      forced,
      steps: [
        "Harvest them from Today.",
        "If they are already gone, open Farm and Count the shelf so the record matches what is really there.",
      ],
    };
  }
  if (id === "F2") {
    return {
      forced,
      steps: [
        "Move them to light from Today.",
        "If they are already under light on the bench, tap Move to light anyway — the record still has them under cover, and that gap is what this check is naming.",
      ],
    };
  }
  if (id === "M4" || id === "H4") {
    return forced
      ? {
          forced,
          steps: [
            "Open the farm folder and read last-verify-replay.txt — it names the outcome, and every diverging row when there is one.",
            "If it says events are pending flush, close the app normally, reopen it, and run System Scan again — nothing has diverged.",
            "Do not record new events while the log and the database disagree.",
            "A restore rolls the database back but not the log, so a verify after a restore can fail exactly like this. That disagreement is real and needs a decision, not a retry.",
          ],
        }
      : id === "M4"
        ? {
            forced,
            steps: [
              "Export the farm and read events.jsonl for the income records named. The detail exists — it just never reached a visible row.",
            ],
          }
        : {
            forced,
            steps: ["Run System Scan below. It runs VERIFY-REPLAY and reports the result."],
          };
  }
  if (id === "H2" || id === "H3") {
    return forced
      ? {
          forced,
          steps: [
            "Close the app and reopen it. A clean close flushes pending events and takes a snapshot.",
            SYSTEM_SCAN_STEP,
            "Do not record new events until it clears.",
          ],
        }
      : { forced, steps: ["Act on the sentence above, then look again."] };
  }
  return { forced, steps: ["Act on the sentence above, then look again."] };
}

/** Locked principle 5. This exact sentence, or none. */
export const SCAN_CLEAN_SENTENCE =
  "I looked and found no signs of the classes of drift I know how to detect.";

/** The classes. Named out loud so the sentence above is checkable. */
export const SCAN_CLASSES: string[] = [
  "the event log against the database (VERIFY-REPLAY) — trays, the money registers, the wholesale order book and the marketing projections; the run prints the tables it excludes",
  "events written but not yet flushed to events.jsonl",
  "a log and a database that have forked after a restore",
  "missing or stale backups, and retention",
  "money owed to you, deliveries due, and promises not covered",
  "income corrections whose detail never reached a visible row",
  "trays past their harvest date, still on the shelf",
  "trays still under cover past their cover-check date",
];

/** Carried by every packet, wherever it came from. Signed FI-9 (E-3). The Rust
 *  side holds the identical string in dock_folds::AUTHORITY_LINE; f9a proves it. */
export const AUTHORITY_LINE =
  "AUTHORITY: PC sole writer. Snapshot only. Do not invent numbers.";

/** Plain text an operator can paste into any AI tool they already use. */
export function evidenceText(
  statuses: CheckStatus[],
  scan: { at: string; verify: CheckStatus } | null,
): string {
  const lines: string[] = ["Groundtruth — health evidence"];
  lines.push(AUTHORITY_LINE);
  if (scan) {
    lines.push(`system scan at ${scan.at}`);
    lines.push(`VERIFY-REPLAY [${scan.verify.severity}] ${scan.verify.sentence}`);
  }
  for (const s of statuses) {
    lines.push(
      `${s.checkId} [${s.severity}] ${s.sentence} (ran_at ${s.ranAt ?? "never reported"})`,
    );
  }
  return lines.join("\n");
}
