// SPDX-License-Identifier: Apache-2.0
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app";
import { resolveApiUrl } from "./lib/config";
import { createSessionClient } from "./lib/session";
import "./style.css";

const apiUrl = resolveApiUrl({
  PUBLIC_MCPA_API_URL: import.meta.env.PUBLIC_MCPA_API_URL,
  PROD: import.meta.env.PROD,
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App apiUrl={apiUrl} sessionClient={createSessionClient(apiUrl)} />
  </StrictMode>,
);
