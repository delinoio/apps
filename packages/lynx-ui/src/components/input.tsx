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
  const environmentRef = useRef(environment);
  environmentRef.current = environment;
  const nativeValue = useRef<string | null>(null);
  const pendingWrite = useRef<{ id: number; value: string } | null>(null);
  const nextWriteId = useRef(0);
  const inputVersion = useRef(0);
  const [edit, setEdit] = useState(0);
  // Lynx input is initialized by default-value. Later controlled updates require
  // setValue; avoid resetting selection when the parent accepts a native edit.
  // Keep this bridge until Lynx exposes a controlled value attribute on all hosts.
  useEffect(() => {
    const node = focus.node.current;
    const pending = pendingWrite.current;
    const hasConflictingPendingWrite =
      pending !== null && pending.value !== value;
    if (!node || (nativeValue.current === value && !hasConflictingPendingWrite))
      return;
    let active = true;
    const version = inputVersion.current;
    const writeId = ++nextWriteId.current;
    pendingWrite.current = { id: writeId, value };
    invoke(node, "setValue", { value }, environmentRef.current, () => {
      // A failed or superseded request must not mark the current host as synced.
      if (
        active &&
        focus.node.current === node &&
        inputVersion.current === version &&
        pendingWrite.current?.id === writeId
      ) {
        nativeValue.current = value;
        pendingWrite.current = null;
      }
    });
    return () => {
      active = false;
    };
  }, [value, edit]);
  const onInput: NativeInputProps["bindinput"] = (event) => {
    inputVersion.current += 1;
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
