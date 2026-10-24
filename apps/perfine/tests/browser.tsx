// SPDX-License-Identifier: Apache-2.0
// Separate test entry. Production never imports this mock transport or identity.
import { createRoot } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { TransportProvider } from "@connectrpc/connect-query";
import { App } from "../src/app";
import { queryClient } from "../src/core";
import { fixture } from "./fixture";
import {
  ExecutionState,
  PerformanceVerdict,
  Permission,
} from "@delino/protos/perfine/v1/perfine_pb";
import "../src/style.css";
const scenario = new URLSearchParams(location.search).get("scenario");
const f = fixture({
  deny: scenario === "denied",
  empty: scenario === "empty",
  incomplete: scenario === "incomplete",
  execution: scenario === "cancelled" ? ExecutionState.CANCELLED : undefined,
  verdict:
    scenario === "inconclusive" ? PerformanceVerdict.INCONCLUSIVE : undefined,
  permissions: scenario === "manager" ? [Permission.MANAGE] : undefined,
  publicationUncertain: scenario === "uncertain",
});
createRoot(document.getElementById("root")!).render(
  <>
    <div
      role="note"
      style={{ background: "#fff0c2", padding: 8, textAlign: "center" }}
    >
      MOCK RPC FIXTURE — illustrative data, no live account or execution
      acceptance
    </div>
    <QueryClientProvider client={queryClient()}>
      <TransportProvider transport={f.transport}>
        <App delcore={f.transport} signOut={() => location.reload()} />
      </TransportProvider>
    </QueryClientProvider>
  </>,
);
