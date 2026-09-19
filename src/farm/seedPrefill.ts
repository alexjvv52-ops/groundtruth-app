/**
 * Seed-weight pre-fill for sow. Rate × tray count only — never dollars.
 * Mirrors src-tauri/src/seed_prefill.rs for the oz arithmetic and the dirty
 * ownership; the string shape is mass.ts's, and under imperial it is
 * byte-identical to the Rust format_seed_oz the mirror pins.
 */

import { massFigure, unitWord } from "@/farm/mass";
import { typedOunces } from "@/farm/typed";

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
  system: string,
): SeedFieldState {
  const oz = proposedSeedOz(rateOzPerTray, trayCount);
  return {
    value: oz == null ? "" : massFigure(oz, system),
    dirty: false,
  };
}

/** Recompute proposal only when the field is not operator-owned. */
export function onProposalInputsChanged(
  state: SeedFieldState,
  rateOzPerTray: number | null | undefined,
  trayCount: number,
  system: string,
): SeedFieldState {
  if (state.dirty) return state;
  return freshSeedProposal(rateOzPerTray, trayCount, system);
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
  system: string,
): { ok: true; quantity: number | null } | { ok: false; error: string } {
  const trimmed = state.value.trim();
  if (trimmed === "") {
    return { ok: true, quantity: null };
  }
  const n = typedOunces(trimmed, system);
  if (!Number.isFinite(n)) {
    return { ok: false, error: `Seed weight must be a positive number (${unitWord(system)}).` };
  }
  if (n <= 0) {
    return { ok: false, error: "Seed weight must be greater than zero." };
  }
  return { ok: true, quantity: n };
}
