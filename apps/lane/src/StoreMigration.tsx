// SPDX-License-Identifier: Apache-2.0
import { useRef, useState } from "@lynx-js/react";
import type { LaneAdapter } from "./adapter";
import { Button, Dialog, Field, Notice } from "./components";
export function StoreMigration({
  adapter,
  ready,
}: {
  adapter: LaneAdapter;
  ready: () => void;
}) {
  const [space, setSpace] = useState("");
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const lock = useRef(false);
  return (
    <view className="section">
      <text className="heading">Update an existing Lane data space</text>
      <Notice>
        Close other Lane windows and stop CLI or MCP writers before migrating.
        This explicitly updates an existing space to the capture-capable format.
        Older Lane versions cannot open the updated space. Migration does not
        create an empty replacement.
      </Notice>
      <Field
        label="Existing data space ID"
        value={space}
        onChange={setSpace}
        focus
        disabled={busy}
      />
      <Button disabled={!space.trim() || busy} onClick={() => setConfirm(true)}>
        Review migration
      </Button>
      {error && <Notice>{error}</Notice>}
      {confirm && (
        <Dialog
          title="Migrate this data space?"
          onClose={() => {
            if (!busy) setConfirm(false);
          }}
        >
          <text text-selection>Existing space: {space}</text>
          <Button disabled={busy} onClick={() => setConfirm(false)}>
            Cancel
          </Button>
          <Button
            primary
            disabled={busy}
            onClick={() => {
              if (lock.current) return;
              lock.current = true;
              setBusy(true);
              setError("");
              void adapter.migrate!(space)
                .then(ready)
                .catch((e) => setError(e.message))
                .finally(() => {
                  lock.current = false;
                  setBusy(false);
                  setConfirm(false);
                });
            }}
          >
            {busy ? "Migrating…" : "Migrate existing space"}
          </Button>
        </Dialog>
      )}
    </view>
  );
}
