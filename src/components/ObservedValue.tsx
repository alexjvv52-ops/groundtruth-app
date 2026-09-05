import type { Observed } from "@/farm/types";
import type { ReactNode } from "react";

function ageLabel(fetchedAt: string, now: Date): string {
  const fetched = new Date(fetchedAt);
  if (Number.isNaN(fetched.getTime())) {
    return `read at ${fetchedAt}`;
  }
  const seconds = Math.max(
    0,
    Math.floor((now.getTime() - fetched.getTime()) / 1000),
  );
  if (seconds < 60) {
    return "read just now";
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `read ${minutes} ${minutes === 1 ? "minute" : "minutes"} ago`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 48) {
    return `read ${hours} ${hours === 1 ? "hour" : "hours"} ago`;
  }
  const days = Math.floor(hours / 24);
  return `read ${days} ${days === 1 ? "day" : "days"} ago`;
}

type Props<T> = {
  observed: Observed<T>;
  children: (value: T) => ReactNode;
};

/**
 * The ONLY component permitted to render a remote-derived value.
 * Takes the whole Observed&lt;T&gt; — never a bare value prop — so the value
 * cannot appear without its age.
 */
export function ObservedValue<T>({ observed, children }: Props<T>) {
  const age = ageLabel(observed.fetchedAt, new Date());
  return (
    <div className="flex flex-col gap-1">
      <div>{children(observed.value)}</div>
      <p className="text-sm text-muted-foreground">{age}</p>
    </div>
  );
}
