/** Parse operator dollars into positive integer cents. */
export function parseDollarsToCents(raw: string): number | null {
  const cleaned = raw.trim().replace(/[^0-9.]/g, "");
  if (!cleaned) return null;
  const parts = cleaned.split(".");
  if (parts.length > 2) return null;
  const dollars = Number(parts[0] || "0");
  if (!Number.isFinite(dollars) || dollars < 0) return null;
  let centsPart = parts[1] ?? "";
  if (centsPart.length > 2) centsPart = centsPart.slice(0, 2);
  const cents = Number((centsPart + "00").slice(0, 2));
  if (!Number.isFinite(cents)) return null;
  const total = dollars * 100 + cents;
  if (!Number.isInteger(total) || total <= 0) return null;
  return total;
}

/** Format integer cents as `$X.YY`. */
export function formatCents(cents: number): string {
  const sign = cents < 0 ? "-" : "";
  const abs = Math.abs(cents);
  const dollars = Math.floor(abs / 100);
  const rem = abs % 100;
  return `${sign}$${dollars}.${String(rem).padStart(2, "0")}`;
}
