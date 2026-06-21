import React from "react";
import ReactDOM from "react-dom/client";
import "../index.css";
import { SessionPanel } from "@/components/session-panel";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SessionPanel />
  </React.StrictMode>,
);
