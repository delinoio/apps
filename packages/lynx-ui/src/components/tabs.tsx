// SPDX-License-Identifier: Apache-2.0
import { useRef, type ReactNode } from "@lynx-js/react";
import type { KeyEvent } from "@lynx-js/types";
import { classes, type FocusHandle } from "../theme";
import { useControlFocus } from "../focus";

export interface TabItem {
  value: string;
  label: string;
  content: ReactNode;
  disabled?: boolean;
}
export interface TabsProps {
  value: string;
  onValueChange: (value: string) => void;
  label: string;
  items: readonly TabItem[];
}
function Tab({
  item,
  selected,
  onPress,
  onKey,
  handle,
}: {
  item: TabItem;
  selected: boolean;
  onPress: () => void;
  onKey: (event: KeyEvent) => void;
  handle: (value: FocusHandle | null) => void;
}) {
  const focus = useControlFocus(Boolean(item.disabled), undefined, handle);
  return (
    <view
      ref={focus.node}
      className={classes(
        "lui-tab",
        selected && "lui-tab-selected",
        item.disabled && "lui-disabled",
        focus.focused && "lui-focused",
      )}
      focusable={!item.disabled}
      accessibility-element
      accessibility-label={item.label}
      accessibility-role-description="tab"
      accessibility-traits={
        item.disabled ? "disabled" : selected ? "selected" : "button"
      }
      bindtap={() => {
        if (!item.disabled) onPress();
      }}
      bindkeydown={onKey}
      bindfocus={focus.onFocus}
      bindblur={focus.onBlur}
    >
      <text className="lui-text">{item.label}</text>
    </view>
  );
}
export function Tabs({ value, onValueChange, label, items }: TabsProps) {
  const handles = useRef(new Map<string, FocusHandle>());
  const enabled = items.filter((item) => !item.disabled);
  const onKey = (event: KeyEvent, item: TabItem) => {
    if (item.disabled || event.repeat) return;
    const index = enabled.findIndex((entry) => entry.value === item.value);
    let next: TabItem | undefined;
    switch (event.key) {
      case "ArrowRight":
        next = enabled[(index + 1) % enabled.length];
        break;
      case "ArrowLeft":
        next = enabled[(index - 1 + enabled.length) % enabled.length];
        break;
      case "Home":
        next = enabled[0];
        break;
      case "End":
        next = enabled.at(-1);
        break;
      case "Enter":
      case " ":
        next = item;
        break;
      default:
        return;
    }
    event.preventDefault?.();
    event.stopPropagation?.();
    if (next) {
      onValueChange(next.value);
      handles.current.get(next.value)?.focus();
    }
  };
  const selected = items.find((item) => item.value === value && !item.disabled);
  return (
    <view className="lui-stack lui-column lui-gap-16">
      <view
        className="lui-tabs"
        accessibility-label={label}
        accessibility-role-description="tablist"
      >
        {items.map((item) => (
          <Tab
            key={item.value}
            item={item}
            selected={item.value === value}
            onPress={() => onValueChange(item.value)}
            onKey={(event) => onKey(event, item)}
            handle={(handle) => {
              if (handle) handles.current.set(item.value, handle);
              else handles.current.delete(item.value);
            }}
          />
        ))}
      </view>
      {selected ? (
        <view
          accessibility-role-description="tabpanel"
          accessibility-label={selected.label}
        >
          {selected.content}
        </view>
      ) : null}
    </view>
  );
}
