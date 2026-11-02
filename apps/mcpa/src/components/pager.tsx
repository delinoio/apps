// SPDX-License-Identifier: Apache-2.0
import { Button } from "@packages/ui/components/button";
import type { useCursorPager } from "../lib/pager";

export function PagerControls({
  pager,
  nextAfterId,
}: {
  pager: ReturnType<typeof useCursorPager>;
  nextAfterId: string | undefined;
}) {
  if (!pager.hasPrevious && !nextAfterId) return null;
  return (
    <nav className="pager" aria-label="Pages">
      <Button
        variant="outline"
        size="sm"
        disabled={!pager.hasPrevious}
        onClick={pager.previous}
      >
        Previous
      </Button>
      <span className="muted small">Page {pager.page}</span>
      <Button
        variant="outline"
        size="sm"
        disabled={!nextAfterId}
        onClick={() => nextAfterId && pager.next(nextAfterId)}
      >
        Next
      </Button>
    </nav>
  );
}
