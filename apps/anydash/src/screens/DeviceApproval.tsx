// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "@lynx-js/react";
import { timestampDate } from "@bufbuild/protobuf/wkt";
import { useMutation, useQuery } from "@connectrpc/connect-query";
import type { NodesRef } from "@lynx-js/types";
import { Button, Variant } from "@packages/lynx-ui";
import {
  CpuArchitecture,
  DevicePlatform,
  DeviceService,
  EnrollmentState,
  type EnrollmentSummary,
} from "@repo/protos/anydash/v1/anydash_pb";
import { Heading, T } from "../components/primitives";
import { ErrorState, LimitReached } from "../components/States";
import { Copy, ExtraCopy } from "../copy";
import { ProblemKind, toProblem, type Problem } from "../errors";
import { formatCountdown } from "../format";
import { hostCall, HostMethod } from "../host";
import { logError } from "../log";
import { useNow } from "../visibility";

/** Server alphabet for user codes; comparison ignores case, spaces and "-". */
export const CODE_ALPHABET = "BCDFGHJKLMNPQRSTVWXZ";
export const CODE_LENGTH = 8;
export function normalizeCode(raw: string) {
  return raw
    .toUpperCase()
    .split("")
    .filter((char) => CODE_ALPHABET.includes(char))
    .join("")
    .slice(0, CODE_LENGTH);
}
export const displayCode = (code: string) =>
  code.length > 4 ? `${code.slice(0, 4)}-${code.slice(4)}` : code;

export const platformLabel = (
  platform: DevicePlatform,
  architecture: CpuArchitecture,
) =>
  `${
    {
      [DevicePlatform.MACOS]: "macOS",
      [DevicePlatform.WINDOWS]: "Windows",
      [DevicePlatform.LINUX]: "Linux",
    }[platform as DevicePlatform.MACOS] ?? "Unknown platform"
  } ${
    {
      [CpuArchitecture.ARM64]: "arm64",
      [CpuArchitecture.X64]: "x64",
    }[architecture as CpuArchitecture.ARM64] ?? ""
  }`.trim();

function CodeBoxes({
  code,
  onChange,
  onConfirm,
}: {
  code: string;
  onChange: (code: string) => void;
  onConfirm: () => void;
}) {
  const input = useRef<NodesRef>(null);
  const [focused, setFocused] = useState(false);
  // The single critical input receives focus when the page is shown.
  useEffect(() => {
    input.current
      ?.invoke({
        method: "focus",
        params: {},
        fail: ({ code }) => logError("anydash_focus_failed", { code }),
      })
      .exec();
  }, []);
  const boxes: string[] = [];
  for (let index = 0; index < CODE_LENGTH; index++)
    boxes.push(code[index] ?? "");
  return (
    <view className="ad-code">
      {boxes.map((char, index) => (
        <view key={index} className="lui-stack lui-row lui-gap-0">
          {index === 4 ? (
            <view className="ad-code-dash" accessibility-element={false}>
              <text className="ad-code-char">–</text>
            </view>
          ) : null}
          <view
            className={`ad-code-box${
              focused && index === Math.min(code.length, CODE_LENGTH - 1)
                ? " ad-code-box-focus"
                : ""
            }`}
            accessibility-element={false}
          >
            <text className="ad-code-char">{char}</text>
          </view>
        </view>
      ))}
      <input
        ref={input}
        className="ad-code-input"
        accessibility-element
        accessibility-label={Copy.enterCode}
        default-value=""
        maxlength={9}
        bindinput={(event) => onChange(normalizeCode(event.detail.value))}
        bindfocus={() => setFocused(true)}
        bindblur={() => setFocused(false)}
        bindconfirm={onConfirm}
      />
    </view>
  );
}

export function DeviceApproval({
  enrollmentId,
  onDone,
}: {
  enrollmentId: string;
  onDone: () => void;
}) {
  const enrollment = useQuery(DeviceService.method.getEnrollment, {
    enrollmentId,
  });
  const [code, setCode] = useState("");
  const [problem, setProblem] = useState<Problem>();
  const [outcome, setOutcome] = useState<"approved" | "rejected">();
  const approve = useMutation(DeviceService.method.approveEnrollment, {
    onSuccess: () => setOutcome("approved"),
    onError: (error) => {
      setProblem(toProblem(error));
      void enrollment.refetch();
    },
  });
  const reject = useMutation(DeviceService.method.rejectEnrollment, {
    onSuccess: () => setOutcome("rejected"),
    onError: (error) => setProblem(toProblem(error)),
  });
  const summary: EnrollmentSummary | undefined = enrollment.data?.enrollment;
  const expires = summary?.expiresAt
    ? timestampDate(summary.expiresAt).getTime()
    : 0;
  const now = useNow(1000, !!summary && !outcome);

  if (enrollment.isPending)
    return (
      <view className="ad-screen">
        <view className="ad-center">
          <T className="ad-small">{Copy.loading}</T>
        </view>
      </view>
    );
  if (outcome)
    return (
      <view className="ad-screen">
        <view className="ad-center">
          <Heading>
            {outcome === "approved"
              ? ExtraCopy.deviceApproved
              : ExtraCopy.requestRejected}
          </Heading>
          <T muted>
            {outcome === "approved"
              ? ExtraCopy.deviceApprovedDetail
              : ExtraCopy.requestRejectedDetail}
          </T>
          <Button variant={Variant.Outline} onPress={onDone}>
            {Copy.goToDashboards}
          </Button>
        </view>
      </view>
    );
  const failure = enrollment.isError ? toProblem(enrollment.error) : undefined;
  const invalid =
    problem?.kind === ProblemKind.EnrollmentInvalid ||
    failure?.kind === ProblemKind.EnrollmentInvalid ||
    failure?.kind === ProblemKind.NotFound ||
    (summary &&
      (summary.state !== EnrollmentState.PENDING ||
        summary.remainingAttempts === 0 ||
        (expires > 0 && expires <= now)));
  if (invalid)
    return (
      <view className="ad-screen">
        <view className="ad-center">
          <view
            accessibility-element
            accessibility-label={Copy.codeInvalid}
            accessibility-role-description="heading"
          >
            <text className="ad-heading ad-danger-text">
              {Copy.codeInvalid}
            </text>
          </view>
          <text className="ad-text ad-muted">
            {Copy.codeInvalidBefore}
            <text className="ad-mono">{Copy.codeInvalidCommand}</text>
            {Copy.codeInvalidAfter}
          </text>
        </view>
      </view>
    );
  if (failure || !summary)
    return (
      <ErrorState
        ids={{ correlationId: failure?.correlationId }}
        onRetry={() => void enrollment.refetch()}
        onCopy={(text) => void hostCall(HostMethod.CopyText, { text })}
      />
    );
  const submit = () => {
    if (code.length !== CODE_LENGTH || approve.isPending) return;
    setProblem(undefined);
    approve.mutate({ enrollmentId, userCode: displayCode(code) });
  };
  return (
    <view className="ad-screen">
      <view className="ad-center">
        <Heading>{Copy.approveDevice}</Heading>
        <text className="ad-text ad-muted">
          <text className="ad-text ad-strong">{summary.deviceName}</text>
          {` · ${platformLabel(summary.platform, summary.architecture)} · ${Copy.collector} ${summary.collectorVersion}`}
        </text>
        <T className="ad-small">{Copy.enterCode}</T>
        <CodeBoxes code={code} onChange={setCode} onConfirm={submit} />
        <T className="ad-small">
          {Copy.expiresIn(
            formatCountdown(expires - now),
            summary.remainingAttempts,
          )}
        </T>
        {problem?.kind === ProblemKind.CodeMismatch ? (
          <T className="ad-text ad-danger-text">{ExtraCopy.codeMismatch}</T>
        ) : problem?.kind === ProblemKind.QuotaExceeded ? (
          <LimitReached problem={problem} />
        ) : problem ? (
          <T className="ad-text ad-danger-text">{problem.message}</T>
        ) : null}
        <view className="ad-actions">
          <Button
            onPress={submit}
            disabled={code.length !== CODE_LENGTH}
            loading={approve.isPending}
          >
            {Copy.approve}
          </Button>
          <Button
            variant={Variant.Outline}
            onPress={() => reject.mutate({ enrollmentId })}
            loading={reject.isPending}
          >
            {Copy.notMyDevice}
          </Button>
        </view>
      </view>
    </view>
  );
}
