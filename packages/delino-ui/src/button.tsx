// SPDX-License-Identifier: Apache-2.0
import { Button as ButtonPart } from "@ui-sdk/react/button";
import type * as React from "react";
import { mergeClassName } from "./class-name.ts";

/** The visual emphasis of a button. */
export enum ButtonVariant {
  /** The main action of a screen. Use one per view. */
  Primary = "primary",
  /** A supporting action next to a primary action. */
  Secondary = "secondary",
  /** An action that deletes data or cannot be undone. */
  Danger = "danger",
}

/** The height and text size of a button. */
export enum ButtonSize {
  Small = "small",
  Medium = "medium",
  Large = "large",
}

export type ButtonProps = React.ComponentProps<typeof ButtonPart> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Stretches the button to the width of its container. */
  fullWidth?: boolean;
};

/**
 * A styled UI SDK `Button`. A Promise returned from `onClick` keeps the
 * button pending until it settles, so an action cannot run twice.
 */
export function Button({
  variant = ButtonVariant.Primary,
  size = ButtonSize.Medium,
  fullWidth = false,
  className,
  ...props
}: ButtonProps) {
  return (
    <ButtonPart
      data-variant={variant}
      data-size={size}
      data-full-width={fullWidth ? "" : undefined}
      className={mergeClassName("dui-button", className)}
      {...props}
    />
  );
}
