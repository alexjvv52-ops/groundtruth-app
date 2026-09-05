import { useState } from "react";
import { MoneyJustLeftSheet } from "@/components/MoneyJustLeftSheet";
import { MoneyCameInSheet } from "@/components/MoneyCameInSheet";
import { MilesSheet } from "@/components/MilesSheet";
import { EquipmentSheet } from "@/components/EquipmentSheet";
import { CostPerTraySheet } from "@/components/CostPerTraySheet";
const ROW =
  "flex min-h-11 w-full items-center justify-center rounded-xl border px-4 text-base font-medium transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";
/**
 * Cost capture and money records. Ruling 6.3: reachable from Today and from
 * Reality as overlays, with a permanent home on the Money tab. One component so
 * the six entries can never drift into three different sets.
 */
export function MoneyCaptureControls({
  collapsible = false,
}: {
  collapsible?: boolean;
}) {
  const [open, setOpen] = useState(!collapsible);
  const [moneyLeftOpen, setMoneyLeftOpen] = useState(false);
  const [moneyInOpen, setMoneyInOpen] = useState(false);
  const [deliveryCostOpen, setDeliveryCostOpen] = useState(false);
  const [milesOpen, setMilesOpen] = useState(false);
  const [equipmentOpen, setEquipmentOpen] = useState(false);
  const [costPerTrayOpen, setCostPerTrayOpen] = useState(false);
  return (
    <div className="flex flex-col gap-3">
      {collapsible && (
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
          className="flex min-h-11 items-center gap-2 text-left text-sm text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        >
          <span aria-hidden="true">{open ? "▾" : "▸"}</span>
          Record money
        </button>
      )}
      {open && (
        <>
          <button type="button" className={ROW} onClick={() => setMoneyLeftOpen(true)}>
            Money just left
          </button>
          <button type="button" className={ROW} onClick={() => setMoneyInOpen(true)}>
            Money came in
          </button>
          <button type="button" className={ROW} onClick={() => setDeliveryCostOpen(true)}>
            Money out for a delivery run
          </button>
          <button type="button" className={ROW} onClick={() => setMilesOpen(true)}>
            Log miles
          </button>
          <button type="button" className={ROW} onClick={() => setEquipmentOpen(true)}>
            Equipment
          </button>
          <button type="button" className={ROW} onClick={() => setCostPerTrayOpen(true)}>
            What a tray costs
          </button>
        </>
      )}
      <MoneyJustLeftSheet
        open={moneyLeftOpen}
        onOpenChange={setMoneyLeftOpen}
        moment="money_just_left"
      />
      <MoneyCameInSheet open={moneyInOpen} onOpenChange={setMoneyInOpen} />
      <MoneyJustLeftSheet
        open={deliveryCostOpen}
        onOpenChange={setDeliveryCostOpen}
        moment="delivery"
      />
      <MilesSheet open={milesOpen} onOpenChange={setMilesOpen} />
      <EquipmentSheet open={equipmentOpen} onOpenChange={setEquipmentOpen} />
      <CostPerTraySheet open={costPerTrayOpen} onOpenChange={setCostPerTrayOpen} />
    </div>
  );
}
