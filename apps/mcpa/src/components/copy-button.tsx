// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { Button } from "@packages/ui/components/button";

export function CopyButton({
  value,
  label = "Copy",
  copiedLabel = "Copied",
  accessibleLabel,
  variant = "outline",
}: {
  value: string;
  label?: string;
  copiedLabel?: string;
  accessibleLabel?: string;
  variant?: "outline" | "default" | "secondary";
}) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  useEffect(() => {
    if (state === "idle") return;
    const timer = setTimeout(() => setState("idle"), 2000);
    return () => clearTimeout(timer);
  }, [state]);
  return (
    <Button
      type="button"
      size="sm"
      variant={variant}
      aria-label={accessibleLabel}
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(value);
          setState("copied");
        } catch {
          setState("failed");
        }
      }}
    >
      <span aria-live="polite">
        {state === "copied"
          ? copiedLabel
          : state === "failed"
            ? "Select and copy manually"
            : label}
      </span>
    </Button>
  );
}
