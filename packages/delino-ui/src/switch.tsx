// SPDX-License-Identifier: Apache-2.0
import { Field, FieldDescription, FieldLabel } from "@ui-sdk/react/field";
import { Switch as SwitchPart, SwitchThumb } from "@ui-sdk/react/switch";
import type * as React from "react";
import { mergeClassName } from "./class-name.ts";

export type SwitchProps = Omit<
  React.ComponentProps<typeof SwitchPart>,
  "children"
> & {
  /** The visible name of the setting. */
  label: React.ReactNode;
  /** More detail about the setting, shown below the label. */
  description?: React.ReactNode;
};

/**
 * A labeled on/off setting built from UI SDK `Field` and `Switch`. The field
 * connects the label and description to the switch.
 */
export function Switch({
  label,
  description,
  className,
  ...props
}: SwitchProps) {
  return (
    <Field className="dui-switch-field">
      <div className="dui-switch-text">
        <FieldLabel className="dui-switch-label">{label}</FieldLabel>
        {description === undefined ? null : (
          <FieldDescription className="dui-switch-description">
            {description}
          </FieldDescription>
        )}
      </div>
      <SwitchPart
        className={mergeClassName("dui-switch", className)}
        {...props}
      >
        <SwitchThumb className="dui-switch-thumb" />
      </SwitchPart>
    </Field>
  );
}
