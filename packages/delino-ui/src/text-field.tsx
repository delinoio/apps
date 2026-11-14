// SPDX-License-Identifier: Apache-2.0
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
} from "@ui-sdk/react/field";
import { Input } from "@ui-sdk/react/input";
import type * as React from "react";
import { mergeClassName } from "./class-name.ts";

export type TextFieldProps = React.ComponentProps<typeof Input> & {
  /** The visible name of the field. */
  label: React.ReactNode;
  /** Help text shown below the input. */
  description?: React.ReactNode;
  /**
   * An error message to show. Setting it marks the field as invalid. Without
   * it, the field shows the browser's message after a failed submit.
   */
  error?: React.ReactNode;
};

/**
 * A labeled text input built from UI SDK `Field` and `Input`. The field
 * connects the label, description and error to the input.
 *
 * When this is the only critical input of a form, pass `autoFocus` so the
 * input has focus when the form is shown.
 */
export function TextField({
  label,
  description,
  error,
  className,
  disabled,
  ...props
}: TextFieldProps) {
  const hasError = error !== undefined && error !== null && error !== false;

  return (
    <Field
      className="dui-text-field"
      disabled={disabled}
      invalid={hasError ? true : undefined}
    >
      <FieldLabel className="dui-text-field-label">{label}</FieldLabel>
      <Input
        className={mergeClassName("dui-text-field-input", className)}
        disabled={disabled}
        {...props}
      />
      {description === undefined ? null : (
        <FieldDescription className="dui-text-field-description">
          {description}
        </FieldDescription>
      )}
      {hasError ? (
        <FieldError className="dui-text-field-error" match>
          {error}
        </FieldError>
      ) : (
        <FieldError className="dui-text-field-error" />
      )}
    </Field>
  );
}
