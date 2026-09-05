import { useEffect, useState } from "react";
import {
  Sheet,
  SheetContent,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";

type PricePadProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  cropName: string;
  /** Initial price in cents. */
  initialCents?: number | null;
  onSave: (priceCents: number) => void;
};

/** Dollar-and-cent pad — buttons only; the key paste field stays the app's sole text field. */
export function PricePad({
  open,
  onOpenChange,
  cropName,
  initialCents,
  onSave,
}: PricePadProps) {
  const [display, setDisplay] = useState("0");
  const [replaced, setReplaced] = useState(false);

  useEffect(() => {
    if (!open) return;
    if (initialCents != null && initialCents > 0) {
      const dollars = (initialCents / 100).toFixed(2);
      setDisplay(dollars);
    } else {
      setDisplay("0");
    }
    setReplaced(false);
  }, [open, initialCents]);

  function pressDigit(d: string) {
    if (!replaced) {
      setDisplay(d === "." ? "0." : d);
      setReplaced(true);
      return;
    }
    if (d === "." && display.includes(".")) return;
    const parts = display.split(".");
    if (parts[1] && parts[1].length >= 2 && d !== ".") return;
    if (display === "0" && d !== ".") {
      setDisplay(d);
      return;
    }
    setDisplay((prev) => prev + d);
  }

  function backspace() {
    if (!replaced) {
      setDisplay("");
      setReplaced(true);
      return;
    }
    setDisplay((prev) => prev.slice(0, -1));
  }

  const cents = (() => {
    const n = Number.parseFloat(display);
    if (display === "" || !Number.isFinite(n) || n <= 0) return null;
    return Math.round(n * 100);
  })();

  const keys = ["1", "2", "3", "4", "5", "6", "7", "8", "9", ".", "0", "⌫"];

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="bottom"
        showCloseButton={false}
        aria-describedby={undefined}
      >
        <div className="mx-auto flex w-full max-w-md flex-col gap-6 p-4 pb-8">
          <SheetTitle className="text-center text-xl font-semibold">
            Price for {cropName}
          </SheetTitle>
          <div
            className="text-center text-5xl font-semibold tabular-nums tracking-tight"
            aria-live="polite"
          >
            ${display || "0"}
          </div>
          <div className="grid grid-cols-3 gap-3">
            {keys.map((key) => (
              <button
                key={key}
                type="button"
                onClick={() => {
                  if (key === "⌫") backspace();
                  else pressDigit(key);
                }}
                className="flex min-h-14 items-center justify-center rounded-xl border text-2xl font-medium transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                {key}
              </button>
            ))}
          </div>
          <Button
            type="button"
            className="h-14 w-full text-lg"
            disabled={cents == null}
            onClick={() => {
              if (cents == null) return;
              onSave(cents);
            }}
          >
            Set price
          </Button>
        </div>
      </SheetContent>
    </Sheet>
  );
}
