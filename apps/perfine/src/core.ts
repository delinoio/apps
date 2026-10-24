// SPDX-License-Identifier: Apache-2.0
import { Code, ConnectError } from "@connectrpc/connect";
import { QueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  CompletionState,
  ExecutionState,
  PerformanceVerdict,
  ErrorDetailSchema,
} from "@delino/protos/perfine/v1/perfine_pb";

export function uuid7() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  let time = BigInt(Date.now());
  for (let i = 5; i >= 0; i--) {
    bytes[i] = Number(time & 255n);
    time >>= 8n;
  }
  bytes[6] = (bytes[6] & 15) | 112;
  bytes[8] = (bytes[8] & 63) | 128;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join(
    "",
  );
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
export function useRequestId() {
  return useState(uuid7)[0];
}
export const queryClient = () =>
  new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 0,
        gcTime: 0,
        retry: (count, error) =>
          count < 1 && ConnectError.from(error).code === Code.Unavailable,
        refetchOnWindowFocus: true,
      },
      mutations: { retry: false },
    },
  });
export const executionLabel = (v: ExecutionState) =>
  ({
    [ExecutionState.UNSPECIFIED]: "Unknown",
    [ExecutionState.QUEUED]: "Queued",
    [ExecutionState.RUNNING]: "Running",
    [ExecutionState.COMPLETED]: "Completed",
    [ExecutionState.FAILED]: "Failed",
    [ExecutionState.CANCELLED]: "Cancelled",
  })[v] || "Unknown";
export const verdictLabel = (v: PerformanceVerdict) =>
  ({
    [PerformanceVerdict.UNSPECIFIED]: "Not evaluated",
    [PerformanceVerdict.PASS]: "Pass",
    [PerformanceVerdict.REGRESSION]: "Regression",
    [PerformanceVerdict.INCONCLUSIVE]: "Inconclusive",
    [PerformanceVerdict.NOT_EVALUATED]: "Not evaluated",
  })[v] || "Not evaluated";
export const completenessLabel = (v: CompletionState) =>
  v === CompletionState.COMPLETE ? "Complete" : "Incomplete";
export const percent = (n: number) =>
  `${(n * 100).toLocaleString("en", { maximumFractionDigits: 3 })}%`;
export function safeURL(value: string, githubOnly = false) {
  try {
    const u = new URL(value);
    return u.protocol === "https:" &&
      !u.username &&
      !u.password &&
      (!githubOnly || u.hostname === "github.com")
      ? u.href
      : undefined;
  } catch {
    return undefined;
  }
}
export function errorInfo(error: unknown) {
  const e = ConnectError.from(error);
  const detail = e.findDetails(ErrorDetailSchema)[0];
  return {
    denied: [Code.Unauthenticated, Code.PermissionDenied].includes(e.code),
    message: detail?.message || e.rawMessage,
    requestId:
      detail?.requestId || e.metadata.get("x-request-id") || "unavailable",
  };
}
