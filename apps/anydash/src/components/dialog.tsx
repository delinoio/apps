// SPDX-License-Identifier: Apache-2.0
import {
  useEffect,
  useRef,
  type ReactNode,
  type RefObject,
} from "@lynx-js/react";
import type { KeyEvent } from "@lynx-js/types";
import {
  Button,
  FocusContainment,
  useFocusContainment,
  Variant,
  type FocusHandle,
} from "@packages/lynx-ui";
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
 * background; native hosts use the key handler below, which moves Tab focus
 * through every enabled library control in the body and the button row in
 * their rendered order.
 */
export function Modal({
  title,
  children,
  action,
  onClose,
  cancelLabel = Copy.cancel,
  initialFocusRef,
  label,
}: {
  title: string;
  children?: ReactNode;
  action?: DialogAction;
  onClose: () => void;
  cancelLabel?: string;
  /** Focus target on open; defaults to Cancel. */
  initialFocusRef?: RefObject<FocusHandle | null>;
  /** Accessible description; defaults to the title. */
  label?: string;
}) {
  const cancel = useRef<FocusHandle>(null);
  const { scope, move } = useFocusContainment();
  useEffect(() => {
    (initialFocusRef ?? cancel).current?.focus();
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
    move(!!event.shiftKey);
  };
  return (
    <FocusContainment scope={scope}>
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
              >
                {action.label}
              </Button>
            ) : null}
          </view>
        </view>
      </view>
    </FocusContainment>
  );
}
