// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@packages/ui/components/dialog";

/**
 * A dialog that closes with Esc, the close button or an outside click and
 * returns focus to the control that opened it. Forms put `autoFocus` on
 * their single critical input so it receives focus when the dialog opens.
 */
export function Modal({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  dismissible = true,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  description?: ReactNode;
  children?: ReactNode;
  footer?: ReactNode;
  /** Set to false while a request is running so it is not abandoned. */
  dismissible?: boolean;
}) {
  // Radix returns focus only to its own trigger element. Console dialogs are
  // opened by ordinary buttons, so remember the focused control instead.
  const [opener] = useState(() =>
    typeof document === "undefined" ? null : document.activeElement,
  );
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && dismissible) onClose();
      }}
    >
      <DialogContent
        showCloseButton={dismissible}
        onInteractOutside={(event) => {
          if (!dismissible) event.preventDefault();
        }}
        onEscapeKeyDown={(event) => {
          if (!dismissible) event.preventDefault();
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          if (opener instanceof HTMLElement && opener.isConnected)
            opener.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription className={description ? undefined : "sr-only"}>
            {description ?? title}
          </DialogDescription>
        </DialogHeader>
        {children}
        {footer && <DialogFooter>{footer}</DialogFooter>}
      </DialogContent>
    </Dialog>
  );
}

/**
 * Focuses an input that appears inside an already open dialog, for example
 * when a multi-step form moves to its next step. `autoFocus` covers inputs
 * present when the dialog opens.
 */
export function useFocusOnMount<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  useEffect(() => {
    ref.current?.focus();
  }, []);
  return ref;
}
