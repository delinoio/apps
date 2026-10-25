// SPDX-License-Identifier: Apache-2.0
import {
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type Ref,
} from "@lynx-js/react";
import type { InputProps as NativeInputProps } from "@lynx-js/types";
import {
  classes,
  FocusKind,
  invoke,
  TextVariant,
  useEnvironment,
  type FocusHandle,
} from "../theme";
import { useControlFocus } from "../focus";
import { Stack, Text } from "./layout";

export function Field({
  label,
  hint,
  error,
  children,
}: {
  label: string;
  hint?: string;
  error?: string;
  children: ReactNode;
}) {
  return (
    <Stack gap={8}>
      <Text>{label}</Text>
      {children}
      {error ? (
        <Text className="lui-error" variant={TextVariant.Caption}>
          {error}
        </Text>
      ) : hint ? (
        <Text muted variant={TextVariant.Caption}>
          {hint}
        </Text>
      ) : null}
    </Stack>
  );
}
export interface InputProps {
  value: string;
  onValueChange: (value: string) => void;
  accessibilityLabel: string;
  placeholder?: string;
  disabled?: boolean;
  readOnly?: boolean;
  invalid?: boolean;
  autoFocus?: boolean;
  focusRef?: Ref<FocusHandle>;
  id?: string;
  maxLength?: number;
  onConfirm?: () => void;
}
export function Input({
  value,
  onValueChange,
  accessibilityLabel,
  placeholder,
  disabled = false,
  readOnly = false,
  invalid = false,
  autoFocus = false,
  focusRef,
  id,
  maxLength,
  onConfirm,
}: InputProps) {
  const focus = useControlFocus(disabled, FocusKind.Input, focusRef, autoFocus);
  const environment = useEnvironment();
  const nativeValue = useRef<string | null>(null);
  const [edit, setEdit] = useState(0);
  // Lynx input is initialized by default-value. Later controlled updates require
  // setValue; avoid resetting selection when the parent accepts a native edit.
  // Keep this bridge until Lynx exposes a controlled value attribute on all hosts.
  useEffect(() => {
    if (nativeValue.current !== value) {
      invoke(focus.node.current, "setValue", { value }, environment);
      nativeValue.current = value;
    }
  }, [value, edit, environment]);
  const onInput: NativeInputProps["bindinput"] = (event) => {
    nativeValue.current = event.detail.value;
    if (!disabled && !readOnly) onValueChange(event.detail.value);
    setEdit((previous) => previous + 1);
  };
  return (
    <input
      id={id}
      ref={focus.node}
      className={classes(
        "lui-input",
        invalid && "lui-invalid",
        disabled && "lui-disabled",
        focus.focused && "lui-focused",
      )}
      default-value={value}
      placeholder={placeholder}
      disabled={disabled}
      readonly={readOnly}
      maxlength={maxLength}
      accessibility-element
      accessibility-label={accessibilityLabel}
      bindinput={onInput}
      bindfocus={focus.onFocus}
      bindblur={focus.onBlur}
      bindconfirm={() => {
        if (!disabled && !readOnly) onConfirm?.();
      }}
    />
  );
}
