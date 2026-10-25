// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState, type ReactNode } from "@lynx-js/react";
import { parseMarkdown, type MarkdownSpan } from "./markdown";
import { hostAction } from "./runtime";
export function Button({
  children,
  onClick,
  disabled = false,
  primary = false,
}: {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  primary?: boolean;
}) {
  return (
    <view
      className={`button ${primary ? "primary" : ""} ${disabled ? "disabled" : ""}`}
      accessibility-element
      accessibility-traits="button"
      accessibility-label={typeof children === "string" ? children : undefined}
      accessibility-value={disabled ? "Disabled" : undefined}
      focusable={!disabled}
      bindtap={() => {
        if (!disabled) onClick?.();
      }}
      bindkeydown={(e) => {
        if (!disabled && (e.key === "Enter" || e.key === " ")) onClick?.();
      }}
    >
      <text>{children}</text>
    </view>
  );
}
export function Field({
  label,
  value,
  onChange,
  multiline = false,
  disabled = false,
  focus = false,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  multiline?: boolean;
  disabled?: boolean;
  focus?: boolean;
}) {
  const [id] = useState(() => "field-" + Math.random().toString(36).slice(2));
  useEffect(() => {
    "background only";
    lynx
      .createSelectorQuery()
      .select(`#${id}`)
      .invoke({ method: "setValue", params: { value } })
      .exec();
  }, [id, value]);
  useEffect(() => {
    "background only";
    if (focus)
      lynx
        .createSelectorQuery()
        .select(`#${id}`)
        .invoke({ method: "focus" })
        .exec();
  }, [id, focus]);
  const props = {
    id,
    className: `input ${multiline ? "multiline" : ""}`,
    "accessibility-label": label,
    disabled,
    maxlength: 2147483647,
    bindinput: (e: { detail: { value: string } }) => onChange(e.detail.value),
  };
  return (
    <view className="field">
      <text className="label">{label}</text>
      {multiline ? <textarea {...props} /> : <input {...props} />}
    </view>
  );
}
export function Choices<T extends string | number>({
  label,
  value,
  options,
  onChange,
  disabled = false,
}: {
  label: string;
  value: T;
  options: readonly (readonly [T, string])[];
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
  return (
    <view className="field">
      <text className="label">{label}</text>
      <view className="row wrap">
        {options.map(([v, title]) => (
          <Button
            key={v}
            disabled={disabled}
            primary={v === value}
            onClick={() => onChange(v)}
          >
            {title}
          </Button>
        ))}
      </view>
    </view>
  );
}
export function Notice({ children }: { children: ReactNode }) {
  return (
    <view className="notice" accessibility-element accessibility-traits="text">
      <text>{children}</text>
    </view>
  );
}
export function Dialog({
  title,
  children,
  onClose,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
}) {
  return (
    <view
      className="overlay"
      global-bindkeydown={(e) => {
        if (e.key === "Escape") onClose();
      }}
    >
      <view className="dialog" accessibility-label={title}>
        <view className="row between">
          <text className="heading">{title}</text>
          <Button onClick={onClose}>Close</Button>
        </view>
        <scroll-view className="dialog-scroll" scroll-orientation="vertical">
          {children}
        </scroll-view>
      </view>
    </view>
  );
}
export function Markdown({ source }: { source: string }) {
  const span = (s: MarkdownSpan, j: number) => (
    <text
      key={j}
      className={`${s.code ? "code" : ""} ${s.bold ? "bold" : ""} ${s.italic ? "italic" : ""} ${s.href ? "link" : ""}`}
    >
      {s.blocked ? `[Image blocked: ${s.text}]` : s.text}
      {s.href ? ` (${s.href})` : ""}
    </text>
  );
  return (
    <view className="markdown">
      {parseMarkdown(source).map((block, i) => {
        const groups: { image?: MarkdownSpan; spans: MarkdownSpan[] }[] = [];
        for (const part of block.spans) {
          if (part.image) groups.push({ image: part, spans: [] });
          else {
            if (!groups.length || groups[groups.length - 1].image)
              groups.push({ spans: [] });
            groups[groups.length - 1].spans.push(part);
          }
        }
        return (
          <view className="paragraph" key={i}>
            {groups.map((group, j) =>
              group.image ? (
                <image
                  key={j}
                  className="markdown-image"
                  src={group.image.image!}
                  accessibility-label={group.image.text}
                />
              ) : (
                <text
                  key={j}
                  text-selection
                  className={
                    block.kind === "heading"
                      ? "heading"
                      : block.kind === "code"
                        ? "code"
                        : ""
                  }
                >
                  {group.spans.map(span)}
                </text>
              ),
            )}
          </view>
        );
      })}
    </view>
  );
}
