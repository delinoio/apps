// SPDX-License-Identifier: Apache-2.0
import type { ReactNode } from "@lynx-js/react";
import type { CSSProperties } from "@lynx-js/types";
import { classes, Direction, TextVariant, Tone } from "../theme";

export interface LayoutProps {
  children?: ReactNode;
  className?: string;
  style?: CSSProperties;
  id?: string;
}

export function Stack({
  children,
  className,
  direction = Direction.Column,
  gap = 16,
  wrap = false,
  ...props
}: LayoutProps & {
  direction?: Direction;
  gap?: 0 | 4 | 8 | 16 | 24 | 32;
  wrap?: boolean;
}) {
  return (
    <view
      {...props}
      className={classes(
        "lui-stack",
        `lui-gap-${gap}`,
        `lui-${direction}`,
        wrap && "lui-wrap",
        className,
      )}
    >
      {children}
    </view>
  );
}
export function Text({
  children,
  variant = TextVariant.Body,
  muted = false,
  className,
  ...props
}: LayoutProps & { variant?: TextVariant; muted?: boolean }) {
  return (
    <text
      {...props}
      className={classes(
        "lui-text",
        `lui-text-${variant}`,
        muted && "lui-muted",
        className,
      )}
    >
      {children}
    </text>
  );
}
export function Card({ children, className, ...props }: LayoutProps) {
  return (
    <view {...props} className={classes("lui-card", className)}>
      {children}
    </view>
  );
}
export function Badge({
  children,
  tone = Tone.Neutral,
  className,
  ...props
}: LayoutProps & { tone?: Tone }) {
  return (
    <view
      {...props}
      className={classes("lui-badge", `lui-tone-${tone}`, className)}
    >
      <text className="lui-badge-label">{children}</text>
    </view>
  );
}
export function Separator() {
  return <view className="lui-separator" accessibility-element={false} />;
}
export function Alert({
  title,
  children,
  tone = Tone.Danger,
}: {
  title: string;
  children?: ReactNode;
  tone?: Tone;
}) {
  return (
    <view
      className={classes("lui-alert", `lui-tone-${tone}`)}
      accessibility-label={title}
    >
      <Text variant={TextVariant.Title}>{title}</Text>
      {children}
    </view>
  );
}
export function Progress({ value, label }: { value: number; label: string }) {
  const percent = Number.isFinite(value)
    ? Math.max(0, Math.min(100, value))
    : 0;
  return (
    <view
      className="lui-progress"
      accessibility-element
      accessibility-role-description="progressbar"
      accessibility-label={label}
      accessibility-value={`${percent}%`}
    >
      <view className="lui-progress-fill" style={{ width: `${percent}%` }} />
    </view>
  );
}
