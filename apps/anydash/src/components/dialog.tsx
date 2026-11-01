// SPDX-License-Identifier: Apache-2.0
import {
  useEffect,
  useRef,
  type ReactNode,
  type RefObject,
} from "@lynx-js/react";
import type { KeyEvent } from "@lynx-js/types";
import { Button, Variant, type FocusHandle } from "@packages/lynx-ui";
import { Copy } from "../copy";

export interface DialogAction {
  label: string;
  onPress: () => void;
  destructive?: boolean;
  disabled?: boolean;
  loading?: boolean;
  loadingLabel?: string;
}

/**
 * Modal dialog in the design record's layout: title, body, then Cancel and the
 * action right-aligned. Escape closes it and initial focus is on Cancel unless
 * a single critical input owns it. The `lui-dialog` and `lui-dialog-close`
 * classes let the lynx-ui web support apply Escape, Tab containment and inert
 * background; native hosts use the key handler below.
 */
export function Modal({
  title,
  children,
  action,
  onClose,
  cancelLabel = Copy.cancel,
  initialFocusRef,
  inputs = [],
  label,
}: {
  title: string;
  children?: ReactNode;
  action?: DialogAction;
  onClose: () => void;
  cancelLabel?: string;
  /** Focus target on open; defaults to Cancel. */
  initialFocusRef?: RefObject<FocusHandle | null>;
  /** Additional focusable inputs, in visual order before the buttons. */
  inputs?: RefObject<FocusHandle | null>[];
  /** Accessible description; defaults to the title. */
  label?: string;
}) {
  const cancel = useRef<FocusHandle>(null);
  const confirm = useRef<FocusHandle>(null);
  const active = useRef(0);
  const order = [...inputs, cancel, ...(action ? [confirm] : [])];
  useEffect(() => {
    const target = initialFocusRef ?? cancel;
    target.current?.focus();
    active.current = Math.max(0, order.indexOf(target));
  }, []);
  const onKey = (event: KeyEvent) => {
    if (event.key === "Escape") {
      event.preventDefault?.();
      event.stopPropagation?.();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    event.preventDefault?.();
    event.stopPropagation?.();
    active.current =
      (active.current + (event.shiftKey ? -1 : 1) + order.length) %
      order.length;
    order[active.current].current?.focus();
  };
  return (
    <view
      className="lui-dialog-backdrop ad-dialog-backdrop"
      accessibility-exclusive-focus
      catchtap={() => {}}
      bindkeydown={onKey}
    >
      <view
        className="lui-dialog ad-dialog"
        accessibility-role-description="dialog"
        accessibility-label={label ?? title}
      >
        <text className="ad-heading ad-dialog-title">{title}</text>
        {children}
        <view className="ad-dialog-row">
          <Button
            className="lui-dialog-close"
            variant={Variant.Outline}
            onPress={onClose}
            focusRef={cancel}
          >
            {cancelLabel}
          </Button>
          {action ? (
            <Button
              variant={action.destructive ? Variant.Danger : Variant.Primary}
              onPress={action.onPress}
              disabled={action.disabled}
              loading={action.loading}
              loadingLabel={action.loadingLabel}
              focusRef={confirm}
            >
              {action.label}
            </Button>
          ) : null}
        </view>
      </view>
    </view>
  );
}
