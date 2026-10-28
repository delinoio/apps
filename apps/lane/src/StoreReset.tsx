// SPDX-License-Identifier: Apache-2.0
import { useState } from "@lynx-js/react";
import type { LaneAdapter } from "./adapter";
import { Button } from "./components";
import { ResetDevelopmentDialog } from "./ResetDevelopment";

/**
 * Shown when the host refuses the local store format. Lane never migrates or
 * resets implicitly; the reset below is the explicit, typed-confirmation path
 * and works although the store could not be opened.
 */
export function StoreReset({
  message,
  reset,
  ready,
}: {
  message: string;
  reset: NonNullable<LaneAdapter["resetUnopened"]>;
  ready: () => void;
}) {
  const [open, setOpen] = useState(false);
  const diagnostic =
    message ===
    "Unsupported store format. Run the explicit development reset (lane data reset)."
      ? message
      : "The local data format is unsupported.";
  return (
    <view className="section store-recovery">
      <text className="heading">This data space cannot be opened</text>
      <text className="bold">Your data has not been changed.</text>
      <view
        className="recovery-diagnostic"
        accessibility-element
        accessibility-label={`Host diagnostic: ${diagnostic}`}
      >
        <text className="bold">Host diagnostic</text>
        <text>{diagnostic}</text>
      </view>
      <text>
        Lane cannot open this data format. If this data matters, use a
        compatible Lane version. Reset only disposable development data.
      </text>
      <text>
        For a specific data space, use the existing CLI reset procedure.
      </text>
      <Button onClick={() => setOpen(true)}>Reset development data</Button>
      {open && (
        <ResetDevelopmentDialog
          onClose={() => setOpen(false)}
          onConfirm={async () => {
            await reset();
            setOpen(false);
            ready();
          }}
        />
      )}
    </view>
  );
}
