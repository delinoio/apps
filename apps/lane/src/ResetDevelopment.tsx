// SPDX-License-Identifier: Apache-2.0
import { useRef, useState } from "@lynx-js/react";
import { Button, Dialog, Field, Notice } from "./components";
import { describeError } from "./planning";

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
  return (
    <Dialog
      title="Reset development data?"
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <Notice>
        This permanently removes every repository, task, prompt, label,
        milestone, cycle, history, attachment and pending change in this local
        data space. It does not contact the server, delete server data or sign
        you out. Use it only for disposable development data. There is no
        recovery.
      </Notice>
      <text>
        Type <text className="bold">{RESET_CONFIRMATION}</text> to confirm.
      </text>
      <Field
        label={`Type ${RESET_CONFIRMATION} to confirm`}
        value={text}
        onChange={setText}
        disabled={busy}
        focus
      />
      {error && <Notice>{error}</Notice>}
      <Button
        primary
        disabled={busy || text.trim() !== RESET_CONFIRMATION}
        onClick={() => {
          if (lock.current) return;
          lock.current = true;
          setBusy(true);
          setError("");
          void onConfirm()
            .catch((e) => setError(describeError(e)))
            .finally(() => {
              lock.current = false;
              setBusy(false);
            });
        }}
      >
        {busy ? "Resetting…" : "Reset development data"}
      </Button>
      <Button disabled={busy} onClick={onClose}>
        Cancel
      </Button>
    </Dialog>
  );
}
