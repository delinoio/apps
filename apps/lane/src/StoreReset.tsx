// SPDX-License-Identifier: Apache-2.0
import { useState } from "@lynx-js/react";
import type { LaneAdapter } from "./adapter";
import { Button, Notice } from "./components";
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
  return (
    <view className="section">
      <text className="heading">This data space cannot be opened</text>
      <Notice>{message}</Notice>
      <text>
        Lane is pre-release and cannot migrate this older format. If its data is
        disposable, reset the active data space explicitly. Otherwise update
        Lane or use the CLI reset procedure for a specific space.
      </text>
      <Button primary onClick={() => setOpen(true)}>
        Reset development data
      </Button>
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
