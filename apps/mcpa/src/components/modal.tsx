// SPDX-License-Identifier: Apache-2.0
import type { ReactNode } from "react";
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
 * returns focus to its trigger. Forms put `autoFocus` on their single
 * critical input so it receives focus when the dialog opens.
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
