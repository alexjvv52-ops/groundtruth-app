/**
 * GT-D27 UNITS (J6-R7, HOME B). INPUT A: the one place a typed figure becomes
 * ounces before an invoke. mass.ts stays the face printer and hands no writer a
 * figure; every desk input that types a mass parses here.
 */
import { OZ_TO_G } from "@/farm/mass";
/** UNITS J5 INPUT A - the typed figure, in ounces. Metric types whole grams.
 *  No snap to 0.1: the operator's grams are the operator's grams. */
export function typedOunces(raw: string, system: string): number {
  const n = Number.parseFloat(raw);
  return Number.isFinite(n) && system === "metric" ? n / OZ_TO_G : n;
}
