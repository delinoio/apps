// SPDX-License-Identifier: Apache-2.0
import { Code, ConnectError } from "@connectrpc/connect";
import { QueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  CompletionState,
  ExecutionState,
  PerformanceVerdict,
  MetricObjective,
  PolicyKind,
  ExperimentSpecificationSchema,
  ErrorDetailSchema,
  type ExperimentSpecification,
} from "@delino/protos/perfine/v1/perfine_pb";
import { fromJsonString } from "@bufbuild/protobuf";

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

// The CLI owns the canonical fingerprint algorithm. Import its complete, frozen
// specification rather than rehashing a browser-specific representation.
export function parseSpecification(json: string): ExperimentSpecification {
  const s = fromJsonString(ExperimentSpecificationSchema, json);
  const sha = /^[a-f0-9]{40}$/i,
    hash = /^[a-f0-9]{64}$/i;
  if (
    s.schemaVersion !== 1 ||
    !sha.test(s.baseSha) ||
    !sha.test(s.initialBaseSha) ||
    !sha.test(s.harnessSha) ||
    !hash.test(s.fingerprintSha256)
  )
    throw new Error(
      "A v1 specification with exact base, initial base, harness SHAs and fingerprint is required.",
    );
  const l = s.limits;
  if (
    !l ||
    !l.candidateCount ||
    !l.simultaneousCandidates ||
    l.simultaneousCandidates > 5 ||
    l.simultaneousCandidates > l.candidateCount ||
    !l.startDeadline ||
    Number(l.startDeadline.seconds) * 1000 <= Date.now() ||
    !l.candidateTimeout ||
    Number(l.candidateTimeout.seconds) <= 0
  )
    throw new Error(
      "Declare a positive candidate limit, concurrency from 1 to 5, a future start deadline and a positive per-candidate timeout.",
    );
  const p = s.policy;
  if (
    !p ||
    p.schemaVersion !== 1 ||
    !p.kind ||
    !p.metric?.name ||
    !p.metric.unit ||
    ![MetricObjective.MINIMIZE, MetricObjective.MAXIMIZE].includes(
      p.metric.objective,
    ) ||
    !hash.test(p.policySha256)
  )
    throw new Error(
      "Declare the metric, original unit, objective and frozen v1 policy hash.",
    );
  if (
    p.kind !== PolicyKind.MEASUREMENTS_ONLY &&
    (!p.repetitions ||
      p.minimumImprovement <= 0 ||
      p.maximumCaseRegression < 0 ||
      !p.uncertainty ||
      p.uncertainty.confidenceLevel <= 0 ||
      p.uncertainty.confidenceLevel >= 1 ||
      p.uncertainty.maximumRelativeIntervalWidth <= 0 ||
      p.uncertainty.nearThresholdMargin < 0)
  )
    throw new Error(
      "Declare minimum improvement, allowed regression, repetitions and all uncertainty criteria before evaluation.",
    );
  if (
    p.kind === PolicyKind.CALLGRIND_IR &&
    (p.metric.name !== "Ir" || p.metric.objective !== MetricObjective.MINIMIZE)
  )
    throw new Error(
      "The Callgrind preset requires Ir instruction count with a minimize objective.",
    );
  if (
    !s.language ||
    !s.environment?.operatingSystem ||
    !s.environment.architecture ||
    !s.environment.runner ||
    !hash.test(s.environment.environmentSha256) ||
    !s.environment.tools.length ||
    s.environment.tools.some(
      (t) => !t.name || !t.version || !hash.test(t.configurationSha256),
    ) ||
    !s.cases.length ||
    s.cases.some(
      (c) =>
        !c.id ||
        !c.family ||
        c.frequency <= 0 ||
        !c.frequencySource ||
        !c.inputs.length ||
        c.inputs.some((i) => !i.name || !hash.test(i.sha256)),
    ) ||
    new Set(s.cases.map((c) => c.id)).size !== s.cases.length
  )
    throw new Error(
      "Include language, reproducible environment/tools, unique workload cases, frequencies and input hashes.",
    );
  if (!s.workloadRepresentativenessConfirmed)
    throw new Error(
      "Confirm workload representativeness locally before freezing the specification.",
    );
  return s;
}
