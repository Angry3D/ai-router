import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import type { AppView } from "./App";
import { AppRoot } from "./AppRoot";
import "./styles.css";

const root = document.getElementById("root");
const view = document.body.dataset.view as AppView | undefined;

if (!root || !view) {
  throw new Error("AI Router view bootstrap is incomplete");
}

createRoot(root).render(
  <StrictMode>
    <AppRoot view={view} />
  </StrictMode>,
);
