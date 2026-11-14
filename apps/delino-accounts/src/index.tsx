// SPDX-License-Identifier: Apache-2.0
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.tsx";
import { ACCOUNTS_API_ORIGIN } from "./api-origin.ts";
import "./index.css";

// Expose the inlined API origin on the document root so the build-time value
// is observable in the running app. This is not user-visible markup; remove it
// when the Connect RPC client consumes `ACCOUNTS_API_ORIGIN` directly.
document.documentElement.dataset.apiOrigin = ACCOUNTS_API_ORIGIN;

const root = document.getElementById("root");
if (!root) {
  throw new Error("Missing #root element");
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
