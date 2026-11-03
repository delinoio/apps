// SPDX-License-Identifier: Apache-2.0
import type { KeyEvent } from "@lynx-js/types";
import { Copy } from "../copy";
import {
  LineStyle,
  WidgetState,
  type ViewLine,
  type WidgetView,
} from "../model/widget-view";
import { Role, Skeleton, SmallButton, StateBadge } from "./primitives";

function Line({ line }: { line: ViewLine }) {
  const className = `ad-small${line.style === LineStyle.Stale ? " ad-stale" : ""}`;
  const fragment = line.code ?? line.strong;
  if (!fragment || !line.text.includes(fragment))
    return <text className={className}>{line.text}</text>;
  const at = line.text.indexOf(fragment);
  return (
    <text className={className}>
      {line.text.slice(0, at)}
      <text className={line.code ? "ad-mono ad-muted" : "ad-small ad-strong"}>
        {fragment}
      </text>
      {line.text.slice(at + fragment.length)}
    </text>
  );
}

export interface EditingProps {
  selected: boolean;
  description: string;
  onSelect: () => void;
  onKey: (event: KeyEvent) => void;
}

export function WidgetCard({
  view,
  compact = false,
  focused = false,
  editing,
  onRemove,
}: {
  view: WidgetView;
  compact?: boolean;
  focused?: boolean;
  editing?: EditingProps;
  onRemove?: () => void;
}) {
  // Values read value → detail → badge; other states lead with the badge.
  const valueFirst =
    view.state === WidgetState.Partial || view.state === WidgetState.Value;
  const before = valueFirst
    ? view.lines.filter((line) => !line.afterBadges)
    : [];
  const after = view.lines.filter((line) => !before.includes(line));
  const className = [
    "ad-widget",
    compact && "ad-widget-compact",
    focused && "ad-widget-focused",
    editing?.selected && "ad-widget-selected",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <view
      id={`widget-${view.id}`}
      className={className}
      // role="group" labelled by the title; the hidden summary below gives
      // screen readers the state label, then the value, then the time.
      accessibility-element={!!editing}
      accessibility-label={
        editing ? `${view.title}. ${editing.description}` : view.title
      }
      accessibility-role-description={Role.Group}
      focusable={!!editing}
      bindtap={editing?.onSelect}
      bindkeydown={editing?.onKey}
      data-state={view.state}
    >
      {editing ? null : (
        <text
          className="ad-sr-only"
          accessibility-element
          accessibility-label={view.accessibilityLabel}
        >
          {view.accessibilityLabel}
        </text>
      )}
      <view className="ad-widget-body" accessibility-elements-hidden>
        <text className="ad-title">{view.title}</text>
        {view.state === WidgetState.Loading ? (
          <view className="lui-stack lui-column lui-gap-8">
            <Skeleton width="60%" height={26} />
            <Skeleton width="85%" height={10} />
            <text className="ad-small">{Copy.loading}</text>
          </view>
        ) : null}
        {view.primary ? (
          <text
            className={[
              view.primary.muted ? "ad-np" : "ad-big",
              view.primary.stale && "ad-stale",
            ]
              .filter(Boolean)
              .join(" ")}
          >
            {view.primary.text}
          </text>
        ) : null}
        {view.usedPercent !== undefined ? (
          <view className="ad-bar">
            <view
              className="ad-bar-fill"
              style={{ width: `${view.usedPercent}%` }}
            />
          </view>
        ) : null}
        {before.map((line, index) => (
          <Line key={`b${index}`} line={line} />
        ))}
        {view.badges.map((badge) => (
          <StateBadge key={badge.label} badge={badge} />
        ))}
        {after.map((line, index) => (
          <Line key={`a${index}`} line={line} />
        ))}
        {view.bars.length > 1 && !compact ? (
          <view className="ad-chart">
            {view.bars.map((value, index) => (
              <view
                key={index}
                className="ad-chart-bar"
                style={{ height: `${Math.max(2, Math.round(value * 100))}%` }}
              />
            ))}
          </view>
        ) : null}
        {view.rows.slice(0, compact ? 3 : 8).map((row) => (
          <view key={row.label} className="ad-row">
            <text className="ad-small">{row.label}</text>
            <text className={`ad-small${row.muted ? "" : " ad-strong"}`}>
              {row.value}
            </text>
          </view>
        ))}
      </view>
      {view.state === WidgetState.SharingEnded && onRemove && !editing ? (
        <SmallButton onPress={onRemove}>{Copy.removeWidget}</SmallButton>
      ) : null}
    </view>
  );
}
