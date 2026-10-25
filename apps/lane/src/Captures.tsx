// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import {
  AttachmentState,
  CaptureService,
  type CaptureReference,
} from "@delino/protos/lane/v1/lane_pb";
import type { UI } from "./serializable";
import { LaneError, type LaneAdapter } from "./adapter";
import { Button, Notice } from "./components";
import { attachmentSource } from "./captures-model";
export function Captures({
  adapter,
  taskId,
  captures,
}: {
  adapter: LaneAdapter;
  taskId: string;
  captures: UI<CaptureReference>[];
}) {
  return (
    <view className="captures">
      {captures.map((capture, index) => (
        <Capture
          key={capture.attachmentId}
          adapter={adapter}
          taskId={taskId}
          capture={capture}
          label={`Capture ${index + 1} of ${captures.length}`}
        />
      ))}
    </view>
  );
}
function Capture({
  adapter,
  taskId,
  capture,
  label,
}: {
  adapter: LaneAdapter;
  taskId: string;
  capture: UI<CaptureReference>;
  label: string;
}) {
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<{
    source?: string;
    message: string;
    retry?: boolean;
  }>({ message: "Loading capture…" });
  useEffect(() => {
    let active = true;
    setResult({ message: "Loading capture…" });
    void adapter
      .call(CaptureService.method.getAttachment, {
        taskId,
        attachmentId: capture.attachmentId,
      })
      .then((reply) => {
        if (!active) return;
        if (reply.state === AttachmentState.DELETED)
          setResult({ message: "This capture was deleted." });
        else if (reply.state === AttachmentState.WAITING_FOR_SYNC)
          setResult({
            message:
              "Waiting for synchronization. The capture is saved on its originating device.",
            retry: true,
          });
        else if (reply.state === AttachmentState.AVAILABLE) {
          const source = attachmentSource(reply.png, reply.downloadUrl);
          setResult(
            source
              ? { source, message: "" }
              : { message: "Capture bytes are unavailable.", retry: true },
          );
        } else setResult({ message: "Capture is unavailable.", retry: true });
      })
      .catch((error) => {
        if (!active) return;
        const denied =
          error instanceof LaneError &&
          /PERMISSION|UNAUTH|FORBIDDEN/.test(error.detail.reason);
        setResult({
          message: denied
            ? "Permission denied. Use Lane Settings to sign in or reconnect this account."
            : "Could not retrieve this capture. Check your connection and retry.",
          retry: !denied,
        });
      });
    return () => {
      active = false;
    };
  }, [adapter, taskId, capture.attachmentId, attempt]);
  return (
    <view className="card capture-result">
      <text className="label">{label}</text>
      {result.source ? (
        <image
          className="capture-image"
          src={result.source}
          mode="aspectFit"
          accessibility-label={`${label}${capture.pageTitle ? `: ${capture.pageTitle}` : ""}`}
          binderror={() =>
            setResult({
              message: "Could not load this capture. Retry to renew access.",
              retry: true,
            })
          }
        />
      ) : (
        <Notice>{result.message}</Notice>
      )}
      {result.retry && (
        <Button onClick={() => setAttempt((value) => value + 1)}>Retry</Button>
      )}
      {capture.pageUrl !== undefined && (
        <text className="capture-metadata" text-selection>
          URL: {capture.pageUrl}
        </text>
      )}
      {capture.pageTitle !== undefined && (
        <text className="capture-metadata" text-selection>
          Page title: {capture.pageTitle}
        </text>
      )}
      {capture.cssSelector !== undefined && (
        <text className="capture-metadata" text-selection>
          CSS selector: {capture.cssSelector}
        </text>
      )}
    </view>
  );
}
