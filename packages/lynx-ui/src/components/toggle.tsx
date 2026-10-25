// SPDX-License-Identifier: Apache-2.0
import type { Ref } from "@lynx-js/react";
import type { KeyEvent } from "@lynx-js/types";
import { classes, type FocusHandle } from "../theme";
import { useControlFocus } from "../focus";

export interface ToggleProps {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  label: string;
  checkedLabel: string;
  uncheckedLabel: string;
  disabled?: boolean;
  focusRef?: Ref<FocusHandle>;
  id?: string;
}
enum ToggleKind {
  Checkbox = "checkbox",
  Switch = "switch",
}
function Toggle({
  checked,
  onCheckedChange,
  label,
  checkedLabel,
  uncheckedLabel,
  disabled = false,
  focusRef,
  id,
  kind,
}: ToggleProps & { kind: ToggleKind }) {
  const focus = useControlFocus(disabled, undefined, focusRef);
  const toggle = () => {
    if (!disabled) onCheckedChange(!checked);
  };
  const onKey = (event: KeyEvent) => {
    if (event.key !== " " && event.key !== "Enter") return;
    event.preventDefault?.();
    event.stopPropagation?.();
    if (!event.repeat) toggle();
  };
  return (
    <view
      ref={focus.node}
      id={id}
      className={classes(
        "lui-toggle",
        disabled && "lui-disabled",
        focus.focused && "lui-focused",
      )}
      focusable={!disabled}
      accessibility-element
      accessibility-label={label}
      accessibility-role-description={kind}
      accessibility-traits={
        disabled ? "disabled" : checked ? "selected" : "button"
      }
      accessibility-value={checked ? checkedLabel : uncheckedLabel}
      bindtap={toggle}
      bindkeydown={onKey}
      bindfocus={focus.onFocus}
      bindblur={focus.onBlur}
    >
      <view
        className={classes(`lui-${kind}`, checked && "lui-checked")}
        accessibility-element={false}
      >
        {kind === ToggleKind.Switch ? (
          <view className="lui-switch-thumb" />
        ) : (
          <text className="lui-checkmark">{checked ? "✓" : ""}</text>
        )}
      </view>
      <text className="lui-text">{label}</text>
    </view>
  );
}
export function Checkbox(props: ToggleProps) {
  return <Toggle {...props} kind={ToggleKind.Checkbox} />;
}
export function Switch(props: ToggleProps) {
  return <Toggle {...props} kind={ToggleKind.Switch} />;
}
