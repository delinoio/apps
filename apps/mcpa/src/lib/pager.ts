// SPDX-License-Identifier: Apache-2.0
import { useCallback, useState } from "react";

/** Keeps the cursor stack for APIs that page with `after_id`. */
export function useCursorPager() {
  const [stack, setStack] = useState<string[]>([""]);
  const afterId = stack[stack.length - 1];
  const next = useCallback(
    (nextAfterId: string) => setStack((current) => [...current, nextAfterId]),
    [],
  );
  const previous = useCallback(
    () =>
      setStack((current) =>
        current.length > 1 ? current.slice(0, -1) : current,
      ),
    [],
  );
  const reset = useCallback(() => setStack([""]), []);
  return {
    afterId,
    page: stack.length,
    hasPrevious: stack.length > 1,
    next,
    previous,
    reset,
  };
}
