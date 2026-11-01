// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "@rstest/core";
import { Code, ConnectError } from "@connectrpc/connect";
import { ErrorDetailSchema, ErrorReason } from "@repo/protos/mcpa/v1/mcpa_pb";
import { describeError, ErrorKind } from "../src/lib/errors";

const failure = (
  code: Code,
  reason?: ErrorReason,
  extra: { traceId?: string; reauthenticationUrl?: string } = {},
) =>
  new ConnectError(
    "internal server text",
    code,
    undefined,
    reason === undefined
      ? undefined
      : [{ desc: ErrorDetailSchema, value: { reason, ...extra } }],
  );

describe("ErrorDetail.reason mapping", () => {
  it("maps every reason to plain-language copy", () => {
    const reasons = Object.values(ErrorReason).filter(
      (value): value is ErrorReason =>
        typeof value === "number" && value !== ErrorReason.UNSPECIFIED,
    );
    for (const reason of reasons) {
      const description = describeError(failure(Code.Unknown, reason));
      expect(description.reason).toBe(reason);
      expect(description.title).not.toBe("Something went wrong");
      expect(description.message).not.toContain("internal server text");
    }
  });

  it("prefers the detail reason over the Connect code", () => {
    const description = describeError(
      failure(Code.FailedPrecondition, ErrorReason.BUDGET_EXHAUSTED, {
        traceId: "trace-1",
      }),
    );
    expect(description.title).toBe("Budget reached");
    expect(description.kind).toBe(ErrorKind.Blocked);
    expect(description.traceId).toBe("trace-1");
  });

  it("carries the reauthentication URL for MFA failures", () => {
    const description = describeError(
      failure(Code.PermissionDenied, ErrorReason.MFA_REQUIRED, {
        reauthenticationUrl: "https://id.example/mfa",
      }),
    );
    expect(description.kind).toBe(ErrorKind.MfaRequired);
    expect(description.reauthenticationUrl).toBe("https://id.example/mfa");
  });

  it("falls back to the Connect code without a detail", () => {
    expect(describeError(failure(Code.PermissionDenied)).kind).toBe(
      ErrorKind.PermissionDenied,
    );
    expect(describeError(failure(Code.Unavailable)).title).toBe(
      "Temporarily unavailable",
    );
    expect(describeError(new Error("Failed to fetch")).kind).toBe(
      ErrorKind.Unavailable,
    );
    expect(describeError(failure(Code.Internal)).kind).toBe(ErrorKind.Unknown);
  });

  it("explains tenant service key rejection", () => {
    expect(
      describeError(
        failure(Code.PermissionDenied, ErrorReason.TENANT_SERVICE_KEY_REJECTED),
      ).message,
    ).toContain("personal");
  });
});
