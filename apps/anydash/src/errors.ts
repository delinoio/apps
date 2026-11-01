// SPDX-License-Identifier: Apache-2.0
import { pick } from "./format";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  ErrorDetailSchema,
  ErrorReason,
  QuotaKind,
  RateLimitScope,
  type ErrorDetail,
} from "@repo/protos/anydash/v1/anydash_pb";
import { CORRELATION_HEADER } from "./transport";

export enum ProblemKind {
  UpdateRequired = "update-required",
  RateLimited = "rate-limited",
  QuotaExceeded = "quota-exceeded",
  VersionConflict = "version-conflict",
  AccessDenied = "access-denied",
  SpaceInactive = "space-inactive",
  OnboardingRequired = "onboarding-required",
  NotFound = "not-found",
  SignedOut = "signed-out",
  AccountNotConnected = "account-not-connected",
  EnrollmentInvalid = "enrollment-invalid",
  CodeMismatch = "code-mismatch",
  DataOwnerRequired = "data-owner-required",
  ServiceManaged = "service-managed",
  CollectorUnavailable = "collector-unavailable",
  Unknown = "unknown",
}

export interface Problem {
  kind: ProblemKind;
  reason: ErrorReason;
  message: string;
  correlationId?: string;
  retryAfterSeconds?: number;
  rateLimitScope?: RateLimitScope;
  quotaKind?: QuotaKind;
  quotaLimit?: number;
  currentVersion?: bigint;
  field?: string;
}

export function errorDetail(error: unknown): ErrorDetail | undefined {
  const connect = ConnectError.from(error);
  try {
    return connect.findDetails(ErrorDetailSchema)[0];
  } catch {
    return undefined;
  }
}

const kindForReason = (reason: ErrorReason, code: Code): ProblemKind => {
  switch (reason) {
    case ErrorReason.UPDATE_REQUIRED:
      return ProblemKind.UpdateRequired;
    case ErrorReason.RATE_LIMITED:
      return ProblemKind.RateLimited;
    case ErrorReason.QUOTA_EXCEEDED:
      return ProblemKind.QuotaExceeded;
    case ErrorReason.VERSION_CONFLICT:
      return ProblemKind.VersionConflict;
    case ErrorReason.PERMISSION_DENIED:
      return ProblemKind.AccessDenied;
    case ErrorReason.DATA_OWNER_REQUIRED:
      return ProblemKind.DataOwnerRequired;
    case ErrorReason.SPACE_INACTIVE:
      return ProblemKind.SpaceInactive;
    case ErrorReason.ONBOARDING_REQUIRED:
      return ProblemKind.OnboardingRequired;
    case ErrorReason.NOT_FOUND:
      return ProblemKind.NotFound;
    case ErrorReason.UNAUTHENTICATED:
      return ProblemKind.SignedOut;
    case ErrorReason.ACCOUNT_NOT_CONNECTED:
      return ProblemKind.AccountNotConnected;
    case ErrorReason.ENROLLMENT_EXPIRED:
    case ErrorReason.ENROLLMENT_INVALIDATED:
    case ErrorReason.ENROLLMENT_ALREADY_USED:
      return ProblemKind.EnrollmentInvalid;
    case ErrorReason.ENROLLMENT_CODE_MISMATCH:
      return ProblemKind.CodeMismatch;
    case ErrorReason.SERVICE_MANAGED:
      return ProblemKind.ServiceManaged;
    default:
      break;
  }
  if (code === Code.Unauthenticated) return ProblemKind.SignedOut;
  if (code === Code.PermissionDenied) return ProblemKind.AccessDenied;
  if (code === Code.Aborted) return ProblemKind.VersionConflict;
  return ProblemKind.Unknown;
};

const durationSeconds = (detail: ErrorDetail) =>
  detail.retryAfter
    ? Math.max(
        1,
        Math.ceil(
          Number(detail.retryAfter.seconds) + detail.retryAfter.nanos / 1e9,
        ),
      )
    : undefined;

/** Classifies an RPC failure into the states the design record defines. */
export function toProblem(error: unknown): Problem {
  if (
    error &&
    typeof error === "object" &&
    "kind" in error &&
    "reason" in error
  )
    return error as Problem;
  const connect = ConnectError.from(error);
  const detail = errorDetail(connect);
  const reason = detail?.reason ?? ErrorReason.UNSPECIFIED;
  return {
    kind: kindForReason(reason, connect.code),
    reason,
    message: connect.rawMessage,
    correlationId:
      detail?.correlationId ||
      connect.metadata.get(CORRELATION_HEADER) ||
      undefined,
    retryAfterSeconds: detail ? durationSeconds(detail) : undefined,
    rateLimitScope: detail?.rateLimitScope,
    quotaKind: detail?.quota?.kind,
    quotaLimit: detail?.quota?.limit,
    currentVersion: detail?.currentVersion,
    field: detail?.field || undefined,
  };
}

export const quotaThings = (kind: QuotaKind | undefined) =>
  pick(
    {
      [QuotaKind.COLLECTOR_DEVICES_PER_USER]: "devices",
      [QuotaKind.EXTERNAL_CONNECTIONS_PER_USER]: "connections",
      [QuotaKind.DASHBOARDS_PER_SPACE]: "dashboards in a space",
      [QuotaKind.WIDGETS_PER_DASHBOARD]: "widgets on a dashboard",
    },
    kind ?? QuotaKind.UNSPECIFIED,
    "items",
  );

export const quotaDefaultLimit = (kind: QuotaKind | undefined) =>
  pick(
    {
      [QuotaKind.COLLECTOR_DEVICES_PER_USER]: 20,
      [QuotaKind.EXTERNAL_CONNECTIONS_PER_USER]: 50,
      [QuotaKind.DASHBOARDS_PER_SPACE]: 50,
      [QuotaKind.WIDGETS_PER_DASHBOARD]: 100,
    },
    kind ?? QuotaKind.UNSPECIFIED,
    0,
  );

/** Never retry quota, permission, conflict or update errors automatically. */
export function shouldRetry(failureCount: number, error: unknown) {
  const problem = toProblem(error);
  if (
    [
      ProblemKind.QuotaExceeded,
      ProblemKind.UpdateRequired,
      ProblemKind.AccessDenied,
      ProblemKind.SpaceInactive,
      ProblemKind.VersionConflict,
      ProblemKind.NotFound,
      ProblemKind.SignedOut,
      ProblemKind.OnboardingRequired,
      ProblemKind.AccountNotConnected,
      ProblemKind.RateLimited,
      ProblemKind.DataOwnerRequired,
    ].includes(problem.kind)
  )
    return false;
  return failureCount < 2;
}
