// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "@lynx-js/react";
import { Button } from "./components";
import {
  UpdateAction,
  UpdatePhase,
  updateAction,
  updateBusy,
  type UpdateStatus,
} from "./updates";

export function AppUpdates({
  settingsDirty,
  settingsBusy,
  onBusyChange,
}: {
  settingsDirty: () => boolean;
  settingsBusy: boolean;
  onBusyChange: (busy: boolean) => void;
}) {
  const [status, setStatus] = useState<UpdateStatus>();
  const [error, setError] = useState("");
  const [starting, setStarting] = useState(false);
  const mounted = useRef(false);
  const startingRef = useRef(false);
  useEffect(() => {
    "background only";
    mounted.current = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const result = await updateAction(UpdateAction.Status);
        if (mounted.current) setStatus(result);
      } catch {
        if (mounted.current)
          setStatus((previous) => ({
            phase: UpdatePhase.Error,
            installedRelease: previous?.installedRelease || "Unknown",
            message:
              "The update service is unavailable. Reopen Settings to retry.",
          }));
      }
      if (mounted.current) timer = setTimeout(() => void poll(), 500);
    };
    void poll();
    return () => {
      mounted.current = false;
      clearTimeout(timer);
      void updateAction(UpdateAction.Cancel).catch(() => {});
    };
  }, []);
  const busy = starting || (!!status && updateBusy(status.phase));
  useEffect(() => {
    "background only";
    onBusyChange(busy);
    return () => onBusyChange(false);
  }, [busy, onBusyChange]);
  if (!status) return null;
  const start = async () => {
    "background only";
    if (startingRef.current || busy || settingsBusy) return;
    if (settingsDirty()) {
      setError("Save your notification changes before updating Lane.");
      return;
    }
    startingRef.current = true;
    setStarting(true);
    setError("");
    try {
      const result = await updateAction(UpdateAction.Start);
      if (mounted.current) setStatus(result);
    } catch {
      if (mounted.current) setError("Could not start the update. Try again.");
    } finally {
      startingRef.current = false;
      if (mounted.current) setStarting(false);
    }
  };
  const message = error || status.message;
  return (
    <view className="card app-updates">
      <text className="heading" accessibility-heading>
        App updates
      </text>
      <text className="muted">
        Installed release: {status.installedRelease.replace("delino@", "")}
      </text>
      <text accessibility-element accessibility-label={message}>
        {message}
      </text>
      {status.phase === UpdatePhase.Downloading &&
        typeof status.progress === "number" && (
          <view
            accessibility-element
            accessibility-label={`Download progress: ${status.progress}%`}
          >
            <text>{status.progress}%</text>
            <view className="update-progress">
              <view
                className="update-progress-fill"
                style={{ width: `${status.progress}%` }}
              />
            </view>
          </view>
        )}
      <view className="row wrap">
        <Button
          primary
          disabled={
            busy || settingsBusy || status.phase === UpdatePhase.Unavailable
          }
          onClick={() => void start()}
        >
          {busy
            ? status.phase === UpdatePhase.Checking
              ? "Checking…"
              : "Updating…"
            : status.phase === UpdatePhase.Error
              ? "Retry update"
              : "Update Lane"}
        </Button>
      </view>
    </view>
  );
}
