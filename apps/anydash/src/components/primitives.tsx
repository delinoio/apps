// SPDX-License-Identifier: Apache-2.0
import type { ReactNode, Ref } from "@lynx-js/react";
import { Button, Variant, type FocusHandle } from "@packages/lynx-ui";
import { BadgeTone, type ViewBadge } from "../model/widget-view";

export enum Role {
  Group = "group",
  Heading = "heading",
  Status = "status",
  Alert = "alert",
  Link = "link",
  Region = "region",
  List = "list",
  ListItem = "listitem",
  Image = "img",
  Banner = "banner",
  Note = "note",
}

/**
 * Lynx exposes `accessibility-role-description`; the web host maps it to an
 * ARIA role (see web/accessibility.js) because web-core does not yet do so.
 */
export function A11y({
  role,
  label,
  children,
  className,
  id,
  live,
}: {
  role: Role;
  label?: string;
  children?: ReactNode;
  className?: string;
  id?: string;
  live?: boolean;
}) {
  return (
    <view
      id={id}
      className={className}
      accessibility-element={!!label}
      accessibility-label={label}
      accessibility-role-description={role}
      accessibility-live-region={live ? "polite" : undefined}
    >
      {children}
    </view>
  );
}

export function T({
  children,
  className = "ad-text",
  muted,
  strong,
}: {
  children: ReactNode;
  className?: string;
  muted?: boolean;
  strong?: boolean;
}) {
  return (
    <text
      className={[className, muted && "ad-muted", strong && "ad-strong"]
        .filter(Boolean)
        .join(" ")}
    >
      {children}
    </text>
  );
}

export function Heading({ children }: { children: string }) {
  return (
    <view
      accessibility-element
      accessibility-label={children}
      accessibility-role-description={Role.Heading}
    >
      <text className="ad-heading">{children}</text>
    </view>
  );
}

const toneClass = (tone: BadgeTone) => `ad-badge ad-badge-${tone}`;
export function StateBadge({ badge }: { badge: ViewBadge }) {
  return (
    <view className={toneClass(badge.tone)}>
      <text className="ad-badge-label">{badge.label}</text>
    </view>
  );
}

export enum NoticeTone {
  Info = "info",
  Warning = "warning",
  Danger = "danger",
}
export function Notice({
  tone,
  title,
  children,
}: {
  tone: NoticeTone;
  title: string;
  children?: string;
}) {
  return (
    <view
      className={`ad-notice ad-notice-${tone}`}
      accessibility-element
      accessibility-label={children ? `${title} ${children}` : title}
      accessibility-role-description={
        tone === NoticeTone.Info ? Role.Note : Role.Alert
      }
    >
      <text className="ad-notice-icon">
        {tone === NoticeTone.Info ? "ⓘ" : "!"}
      </text>
      <text className="ad-notice-body">
        <text className="ad-strong">{title}</text>
        {children ? ` ${children}` : ""}
      </text>
    </view>
  );
}

export function SmallButton({
  children,
  onPress,
  disabled,
  accessibilityLabel,
  focusRef,
  id,
}: {
  children: string;
  onPress?: () => void;
  disabled?: boolean;
  accessibilityLabel?: string;
  focusRef?: Ref<FocusHandle>;
  id?: string;
}) {
  return (
    <Button
      id={id}
      className="ad-small-button"
      variant={Variant.Outline}
      onPress={onPress}
      disabled={disabled}
      accessibilityLabel={accessibilityLabel}
      focusRef={focusRef}
    >
      {children}
    </Button>
  );
}

/** Header picker, rendered as the record's pill with a "▾" affordance. */
export function Pill({
  label,
  onPress,
  accessibilityLabel,
}: {
  label: string;
  onPress: () => void;
  accessibilityLabel: string;
}) {
  return (
    <Button
      className="ad-pill"
      variant={Variant.Outline}
      onPress={onPress}
      accessibilityLabel={accessibilityLabel}
    >
      {`${label} ▾`}
    </Button>
  );
}

export function Skeleton({ width, height }: { width: string; height: number }) {
  return (
    <view
      className="ad-sk"
      style={{ width, height: `${height}px` }}
      accessibility-element={false}
    />
  );
}

/** Link-styled inline action. */
export function LinkText({
  children,
  onPress,
}: {
  children: string;
  onPress: () => void;
}) {
  return (
    <text
      className="ad-text ad-link"
      bindtap={onPress}
      accessibility-element
      accessibility-label={children}
      accessibility-role-description={Role.Link}
      focusable
    >
      {children}
    </text>
  );
}
