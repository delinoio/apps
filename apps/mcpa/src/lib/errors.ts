// SPDX-License-Identifier: Apache-2.0
import { Code, ConnectError } from "@connectrpc/connect";
import { ErrorDetailSchema, ErrorReason } from "@repo/protos/mcpa/v1/mcpa_pb";

export enum ErrorKind {
  Unauthenticated = "unauthenticated",
  MfaRequired = "mfa-required",
  PermissionDenied = "permission-denied",
  NotFound = "not-found",
  Invalid = "invalid",
  Conflict = "conflict",
  Unavailable = "unavailable",
  Blocked = "blocked",
  Unknown = "unknown",
}

export type ErrorDescription = {
  kind: ErrorKind;
  reason: ErrorReason;
  title: string;
  message: string;
  traceId?: string;
  reauthenticationUrl?: string;
};

type Copy = { kind: ErrorKind; title: string; message: string };

const reasonCopy: Record<ErrorReason, Copy | undefined> = {
  [ErrorReason.UNSPECIFIED]: undefined,
  [ErrorReason.INVALID_INPUT]: {
    kind: ErrorKind.Invalid,
    title: "Check what you entered",
    message: "Some of the details are not valid. Fix them and try again.",
  },
  [ErrorReason.UNAUTHENTICATED]: {
    kind: ErrorKind.Unauthenticated,
    title: "Sign in again",
    message: "Your session has ended. Sign in to continue.",
  },
  [ErrorReason.PERMISSION_DENIED]: {
    kind: ErrorKind.PermissionDenied,
    title: "You don't have access",
    message:
      "Your account doesn't have permission for this. Ask a resource manager or a tenant admin for access.",
  },
  [ErrorReason.NOT_FOUND]: {
    kind: ErrorKind.NotFound,
    title: "Not found",
    message: "We couldn't find this. It may have been deleted.",
  },
  [ErrorReason.ALREADY_EXISTS]: {
    kind: ErrorKind.Conflict,
    title: "Already exists",
    message: "Something with these details already exists.",
  },
  [ErrorReason.FAILED_PRECONDITION]: {
    kind: ErrorKind.Conflict,
    title: "This change isn't possible right now",
    message:
      "The current state doesn't allow this change. For example, a resource needs at least one manager.",
  },
  [ErrorReason.MFA_REQUIRED]: {
    kind: ErrorKind.MfaRequired,
    title: "Two-step verification required",
    message:
      "Managed CPA requires two-step verification. Set it up or sign in again with it, then come back.",
  },
  [ErrorReason.DEPENDENCY_UNAVAILABLE]: {
    kind: ErrorKind.Unavailable,
    title: "Temporarily unavailable",
    message:
      "A service Managed CPA depends on isn't responding. Try again in a moment.",
  },
  [ErrorReason.TENANT_INACTIVE]: {
    kind: ErrorKind.Blocked,
    title: "This tenant isn't active",
    message:
      "Delcore reports this tenant as blocked or being deleted, or its billing isn't set up. Check the tenant in Delcore.",
  },
  [ErrorReason.BUDGET_EXHAUSTED]: {
    kind: ErrorKind.Blocked,
    title: "Budget reached",
    message:
      "Spending reached the budget for this billing period. Raise or remove the limit to allow new calls.",
  },
  [ErrorReason.MODEL_NOT_ALLOWED]: {
    kind: ErrorKind.Blocked,
    title: "Model not allowed",
    message: "This model isn't on the resource's model allowlist.",
  },
  [ErrorReason.IDEMPOTENCY_CONFLICT]: {
    kind: ErrorKind.Conflict,
    title: "Request already used",
    message:
      "This request was already sent with different details. Close the form and start again.",
  },
  [ErrorReason.TENANT_SERVICE_KEY_REJECTED]: {
    kind: ErrorKind.PermissionDenied,
    title: "Tenant service keys can't be used",
    message:
      "Managed CPA management accepts only personal Delcore keys. Use a personal key instead.",
  },
};

const codeReason: Partial<Record<Code, ErrorReason>> = {
  [Code.InvalidArgument]: ErrorReason.INVALID_INPUT,
  [Code.Unauthenticated]: ErrorReason.UNAUTHENTICATED,
  [Code.PermissionDenied]: ErrorReason.PERMISSION_DENIED,
  [Code.NotFound]: ErrorReason.NOT_FOUND,
  [Code.AlreadyExists]: ErrorReason.ALREADY_EXISTS,
  [Code.FailedPrecondition]: ErrorReason.FAILED_PRECONDITION,
  [Code.Unavailable]: ErrorReason.DEPENDENCY_UNAVAILABLE,
};

/** Maps a Connect failure to plain-language copy using ErrorDetail.reason. */
export function describeError(error: unknown): ErrorDescription {
  const connectError = ConnectError.from(error);
  const [detail] = connectError.findDetails(ErrorDetailSchema);
  const reason =
    detail?.reason || codeReason[connectError.code] || ErrorReason.UNSPECIFIED;
  const traceId =
    detail?.traceId ||
    connectError.metadata.get("X-Trace-Id") ||
    connectError.metadata.get("X-Mcpa-Request-Id") ||
    undefined;
  const copy = reasonCopy[reason];
  if (!copy) {
    const network =
      connectError.code === Code.Unknown &&
      /fetch|network|load failed/i.test(connectError.rawMessage);
    return {
      kind: network ? ErrorKind.Unavailable : ErrorKind.Unknown,
      reason,
      title: network ? "Can't reach Managed CPA" : "Something went wrong",
      message: network
        ? "Check your connection and try again."
        : "Try again. If it keeps happening, contact support with the trace ID.",
      traceId,
    };
  }
  return {
    ...copy,
    reason,
    traceId,
    reauthenticationUrl:
      reason === ErrorReason.MFA_REQUIRED
        ? detail?.reauthenticationUrl || undefined
        : undefined,
  };
}

export function isPermissionDenied(error: unknown): boolean {
  return describeError(error).kind === ErrorKind.PermissionDenied;
}
