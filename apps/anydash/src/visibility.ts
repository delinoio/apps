// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import { HostEvent, listenHost } from "./host";

/** True while the host reports the view as visible; data refreshes only then. */
export function useVisible() {
  const [visible, setVisible] = useState(true);
  useEffect(() => {
    const stopVisibility = listenHost(HostEvent.Visibility, (raw) => {
      const value = raw as { visible?: unknown } | undefined;
      if (typeof value?.visible === "boolean") setVisible(value.visible);
    });
    const stopResume = listenHost(HostEvent.Resume, () => setVisible(true));
    return () => {
      stopVisibility();
      stopResume();
    };
  }, []);
  return visible;
}

/** Re-renders every `period` milliseconds; used for countdowns. */
export function useNow(period = 1000, active = true) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), period);
    return () => clearInterval(timer);
  }, [period, active]);
  return now;
}
