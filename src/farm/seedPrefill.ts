/**
 * Seed-weight pre-fill for sow. Rate × tray count only — never dollars.
 * Mirrors src-tauri/src/seed_prefill.rs (1 decimal, dirty ownership).
 */

/** Same precision as WeightPad formatOz. */
export function formatSeedOz(oz: number): string {
  return oz.toFixed(1);
}

export function proposedSeedOz(
  rateOzPerTray: number | null | undefined,
  trayCount: number,
): number | null {
  if (rateOzPerTray == null || !Number.isFinite(rateOzPerTray) || rateOzPerTray <= 0) {
    return null;
  }
  if (trayCount < 1) return null;
  return rateOzPerTray * trayCount;
}

export type SeedFieldState = {
  value: string;
  dirty: boolean;
};

export function freshSeedProposal(
  rateOzPerTray: number | null | undefined,
  trayCount: number,
): SeedFieldState {
  const oz = proposedSeedOz(rateOzPerTray, trayCount);
  return {
    value: oz == null ? "" : formatSeedOz(oz),
    dirty: false,
  };
}

/** Recompute proposal only when the field is not operator-owned. */
export function onProposalInputsChanged(
  state: SeedFieldState,
  rateOzPerTray: number | null | undefined,
  trayCount: number,
): SeedFieldState {
  if (state.dirty) return state;
  return freshSeedProposal(rateOzPerTray, trayCount);
}

/**
 * Operator edit. Clearing the field drops dirty so a fresh proposal may return.
 */
export function onOperatorSeedEdit(
  _state: SeedFieldState,
  next: string,
): SeedFieldState {
  if (next.trim() === "") {
    return { value: "", dirty: false };
  }
  return { value: next, dirty: true };
}

/** Blank → null (no seed record). Zero/negative → error. */
export function confirmSeedQuantity(
  state: SeedFieldState,
): { ok: true; quantity: number | null } | { ok: false; error: string } {
  const trimmed = state.value.trim();
  if (trimmed === "") {
    return { ok: true, quantity: null };
  }
  const n = Number.parseFloat(trimmed);
  if (!Number.isFinite(n)) {
    return { ok: false, error: "Seed weight must be a positive number (oz)." };
  }
  if (n <= 0) {
    return { ok: false, error: "Seed weight must be greater than zero." };
  }
  return { ok: true, quantity: n };
}
