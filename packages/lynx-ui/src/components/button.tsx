// SPDX-License-Identifier: Apache-2.0
import type { ReactNode, Ref } from "@lynx-js/react";
import type { KeyEvent } from "@lynx-js/types";
import { classes, Variant, type FocusHandle } from "../theme";
import { useControlFocus } from "../focus";

export interface ButtonProps {
  children: ReactNode;
  onPress?: () => void;
  disabled?: boolean;
  loading?: boolean;
  loadingLabel?: string;
  variant?: Variant;
  accessibilityLabel?: string;
  className?: string;
  id?: string;
  focusRef?: Ref<FocusHandle>;
}
export function Button({
  children,
  onPress,
  disabled = false,
  loading = false,
  loadingLabel,
  variant = Variant.Primary,
  accessibilityLabel,
  className,
  id,
  focusRef,
}: ButtonProps) {
  const blocked = disabled || loading;
  const focus = useControlFocus(blocked, undefined, focusRef);
  const activate = () => {
    if (!blocked) onPress?.();
  };
  const onKey = (event: KeyEvent) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault?.();
    event.stopPropagation?.();
    if (!event.repeat) activate();
  };
  return (
    <view
      id={id}
      ref={focus.node}
      className={classes(
        "lui-button",
        `lui-button-${variant}`,
        blocked && "lui-disabled",
        focus.focused && "lui-focused",
        className,
      )}
      focusable={!blocked}
      accessibility-element
      accessibility-traits={blocked ? "disabled" : "button"}
      accessibility-role-description="button"
      accessibility-label={accessibilityLabel}
      bindtap={activate}
      bindkeydown={onKey}
      bindfocus={focus.onFocus}
      bindblur={focus.onBlur}
    >
      {loading ? (
        <view className="lui-spinner" accessibility-element={false} />
      ) : null}
      <text className="lui-button-label">
        {loading && loadingLabel ? loadingLabel : children}
      </text>
    </view>
  );
}
