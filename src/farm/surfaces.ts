/**
 * Today / Reality split — sow-door and money-intent helpers stay here.
 * Surface membership and Today's ranks live on the PC (dock_folds). The desk
 * reads them; it does not recompute them.
 */
import type { DockFoldsView } from "@/farm/types";

export type Surface = "today" | "upcoming" | "reality" | "marketing";

export function surfaceForKind(folds: DockFoldsView, kind: string): Surface {
  return (folds.surfaces.map[kind] ?? folds.surfaces.default) as Surface;
}
export const MONEY_DEBT_KINDS = [
  "money.delivered_unpaid",
  "money.delivery_due",
  "money.capacity_short",
] as const;
export function isMoneyDebtKind(kind: string): boolean {
  return (MONEY_DEBT_KINDS as readonly string[]).includes(kind);
}
/**
 * The single sow door's close rule, shared by every surface that opens it, so
 * an unreachable gap can never hold the sheet open on one screen and close it
 * on another.
 */
export function sowableGapsRemain(
  demand: { shortfall: number } | null,
  plan: { reachability: { sowCanServe: boolean } }[],
): boolean {
  return (demand?.shortfall ?? 0) > 0 || plan.some((c) => c.reachability.sowCanServe);
}

/**
 * RB2 — Today names the two cash-converting facts; Money performs them.
 * This is the deep link between them: placement, not a second write path.
 * Only kinds whose entity_id IS a wholesale order id get an intent
 * (attention.rs:539-560). money.capacity_short is keyed on harvest_date and
 * keeps its own Sow / Open the order buttons — it is not listed here.
 */
export type MoneyIntent = "collect" | "deliver";
export type MoneyFocus = { orderId: string; intent: MoneyIntent };
export function moneyIntentForKind(kind: string): MoneyIntent | null {
  if (kind === "money.delivered_unpaid") return "collect";
  if (kind === "money.delivery_due") return "deliver";
  return null;
}
