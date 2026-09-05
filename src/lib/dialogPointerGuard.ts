import { useEffect } from "react";

const SHEET_NODES = '[data-slot="sheet-content"],[data-slot="sheet-overlay"]';

/**
 * Radix's DismissableLayer sets `document.body.style.pointerEvents = "none"` while a
 * modal Sheet is open and restores it when the last layer unmounts. The saved "original"
 * value is module-level state shared by every layer, so a stacked pair
 * (SowSheet / WeightPad -> MoneyJustLeftSheet) unmounting out of order can leave "none"
 * on <body> after the last sheet has closed. The window then renders normally and text
 * stays selectable, but nothing is clickable.
 *
 * This guard is a repair, not a policy. It only ever clears "none" from <body>, and only
 * while no sheet content or overlay is mounted. It never sets pointer-events and never
 * touches a sheet, so open-sheet behaviour is unchanged.
 */
export function useDialogPointerGuard(): void {
  useEffect(() => {
    const body = document.body;

    function release() {
      if (body.style.pointerEvents !== "none") return;
      if (document.querySelector(SHEET_NODES) !== null) return;
      body.style.pointerEvents = "";
    }

    const observer = new MutationObserver(release);
    observer.observe(body, {
      attributes: true,
      attributeFilter: ["style"],
      childList: true,
      subtree: true,
    });
    release();

    return () => observer.disconnect();
  }, []);
}
