import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import "./index.css";

if (import.meta.env.DEV) {
  (
    window as Window & {
      __farmDev?: {
        trays: () => Promise<unknown>;
        backdate: (trayId: string, days: number) => Promise<void>;
      };
    }
  ).__farmDev = {
    trays: () => invoke("list_trays"),
    backdate: (trayId: string, days: number) =>
      invoke("dev_backdate_tray", { trayId, days }),
  };
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
