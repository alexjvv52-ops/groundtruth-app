/**
 * GT-D27 UNITS (J2 UNITS-FACE). PRINT B, INTERIM B: the one mass printer the
 * desk faces read. SCOPE A - mass only, never currency, never pack unit, never
 * tray counts. CANON A - ounces on the books become whole grams on a face, and
 * only on a face: nothing here runs on the way into an event, and no writer is
 * ever handed a figure from this file.
 *
 * Twin of src-tauri/src/units.rs: OZ_TO_G, grams() and the unit words of the
 * sealed two-row table. A change on one side without the other is a failed chip.
 */
/** PRECISION G. A pound is 453.59237 g and sixteen ounces, so an ounce is this. */
export const OZ_TO_G = 28.349523125;
/** STORE B. The system a face prints before the desk has answered. */
export const DEFAULT_UNITS = "imperial";
/** CANON A. Whole grams. Math.round; every mass on the tree is positive. */
export function grams(oz: number): number {
  if (!Number.isFinite(oz)) return 0;
  return Math.round(oz * OZ_TO_G);
}
/** The short unit word this system puts after a figure. Twin of unit_for. */
export function unitWord(system: string): string {
  return system === "metric" ? "g" : "oz";
}
/** The per-tray unit word. One table - no face spells its own. */
export function perTrayUnit(system: string): string {
  return system === "metric" ? "g/tray" : "oz/tray";
}
/**
 * A figure already on the file's 0.1-oz grid (a weighed harvest, a receipt, a
 * jar, a listed leftover). Imperial keeps the shipped bytes; metric prints
 * whole grams.
 */
export function massFigure(oz: number, system: string): string {
  return system === "metric" ? String(grams(oz)) : oz.toFixed(1);
}
/**
 * A mean over trays, which is not on the grid. Imperial snaps to 0.1 exactly as
 * the shipped line did; metric rounds the unrounded mean once, so no face
 * rounds twice.
 */
export function perTrayFigure(oz: number, system: string): string {
  return system === "metric"
    ? String(grams(oz))
    : (Math.round(oz * 10) / 10).toFixed(1);
}
