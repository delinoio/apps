// SPDX-License-Identifier: Apache-2.0
import { useRef, useState } from "@lynx-js/react";
import { Button, Dialog, Field } from "./components";
import { LaneError } from "./adapter";

// Typed by the user so a reset is never a single accidental activation.
export const RESET_CONFIRMATION = "reset development data";

/** Typed-confirmation dialog for the explicit pre-release development reset. */
export function ResetDevelopmentDialog({
  onConfirm,
  onClose,
}: {
  onConfirm: () => Promise<void>;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const lock = useRef(false);
  const failure = (e: unknown) => {
    if (e instanceof LaneError) {
      const reason = e.detail.reason.replace(/^ERROR_REASON_/, "");
      if (reason === "STORAGE_BUSY")
        return "Storage is busy. Close other Lane windows, CLI or MCP handles, or stop the sync worker before you retry.";
      if (reason === "STORAGE_PERMISSION")
        return "Storage permission denied. Restore access before you retry.";
      return /^[A-Z_]+$/.test(reason)
        ? `Reset failed (${reason}). Retry explicitly.`
        : "Reset failed. Retry explicitly.";
    }
    if (
      e instanceof Error &&
      e.message === "Data reset cancelled. Your input is retained."
    )
      return e.message;
    return "Reset failed. Retry explicitly.";
  };
  return (
    <Dialog
      title="Reset development data?"
      className="reset-dialog"
      closeDisabled={busy}
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <view className="reset-content">
        <text>Only reset disposable development data.</text>
        <view
          className="reset-warning"
          accessibility-element
          accessibility-label="Permanent local deletion. This removes every repository, task, prompt, label, milestone, cycle, history, attachment and pending change in this local data space. There is no recovery."
        >
          <text className="bold">⚠ Permanent local deletion</text>
          <text>
            This removes every repository, task, prompt, label, milestone,
            cycle, history, attachment and pending change in this local data
            space.
          </text>
          <text className="bold">There is no recovery.</text>
        </view>
        <text>
          Server data is not deleted. This does not contact the server or sign
          you out.
        </text>
        <text>
          An account-linked space will need explicit reconnection before sync
          resumes.
        </text>
        <Field
          label="Type the following phrase to confirm"
          value={text}
          onChange={setText}
          disabled={busy}
          focus
          error={error}
          prefix={<text className="reset-phrase">{RESET_CONFIRMATION}</text>}
        />
        {busy && (
          <text accessibility-element accessibility-live-region="polite">
            Resetting…
          </text>
        )}
        <view className="reset-actions">
          <Button disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          <Button
            primary
            disabled={busy || text.trim() !== RESET_CONFIRMATION}
            onClick={() => {
              if (lock.current) return;
              lock.current = true;
              setBusy(true);
              setError("");
              void onConfirm()
                .catch((e) => setError(failure(e)))
                .finally(() => {
                  lock.current = false;
                  setBusy(false);
                });
            }}
          >
            {busy ? "Resetting…" : "Reset development data"}
          </Button>
        </view>
      </view>
    </Dialog>
  );
}
