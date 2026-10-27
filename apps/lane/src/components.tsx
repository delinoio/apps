// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState, type ReactNode } from "@lynx-js/react";
import { parseMarkdown, type MarkdownSpan } from "./markdown";
import { hostAction } from "./runtime";
export function Button({
  id: suppliedId,
  children,
  onClick,
  disabled = false,
  primary = false,
  pressed,
  expanded,
  label,
  swatch,
  focusOnShow = false,
  focusRequest = 0,
}: {
  id?: string;
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  primary?: boolean;
  // Toggle state for chips; exposed to assistive technology, not only by color.
  pressed?: boolean;
  expanded?: boolean;
  // Overrides the accessible name when the visible text is not a plain string.
  label?: string;
  // Decorative color next to the text; the name always stays visible.
  swatch?: string;
  focusOnShow?: boolean;
  focusRequest?: number;
}) {
  const [generatedId] = useState(
    () => "button-" + Math.random().toString(36).slice(2),
  );
  const buttonId = suppliedId ?? generatedId;
  useEffect(() => {
    "background only";
    if (focusOnShow)
      lynx
        .createSelectorQuery()
        .select(`#${buttonId}`)
        .invoke({ method: "setFocus", params: { focus: true } })
        .exec();
  }, [focusOnShow, buttonId]);
  const selected = pressed ?? primary;
  return (
    <view
      id={buttonId}
      data-dialog-default={focusOnShow ? "true" : undefined}
      data-focus-request={focusRequest || undefined}
      className={`button ${primary || pressed ? "primary" : ""} ${disabled ? "disabled" : ""}`}
      data-pressed={pressed === undefined ? undefined : String(pressed)}
      data-expanded={expanded === undefined ? undefined : String(expanded)}
      accessibility-element
      accessibility-traits="button"
      accessibility-label={
        label ?? (typeof children === "string" ? children : undefined)
      }
      accessibility-value={
        disabled
          ? "Disabled"
          : pressed !== undefined && selected
            ? "Selected"
            : undefined
      }
      focusable={!disabled}
      bindtap={() => {
        if (!disabled) onClick?.();
      }}
      bindkeydown={(e) => {
        if (!disabled && (e.key === "Enter" || e.key === " ")) onClick?.();
      }}
    >
      {swatch && (
        <view
          className="swatch"
          accessibility-elements-hidden
          style={{ backgroundColor: swatch }}
        />
      )}
      <text>{children}</text>
    </view>
  );
}
export function CompactSelect<T extends string | number>({
  label,
  value,
  options,
  onChange,
  disabled = false,
  active = true,
}: {
  label: string;
  value: T;
  options: readonly (readonly [T, string])[];
  onChange: (value: T) => void;
  disabled?: boolean;
  active?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [focusRequest, setFocusRequest] = useState(0);
  const [triggerId] = useState(
    () => "select-" + Math.random().toString(36).slice(2),
  );
  const selected = options.find(([option]) => option === value)?.[1] ?? "";
  useEffect(() => {
    "background only";
    if (focusRequest)
      lynx
        .createSelectorQuery()
        .select(`#${triggerId}`)
        .invoke({ method: "setFocus", params: { focus: true } })
        .exec();
  }, [focusRequest, triggerId]);
  const close = () => {
    setOpen(false);
    setFocusRequest((request) => request + 1);
  };
  return (
    <view
      className="compact-select"
      global-bindkeydown={(event) => {
        if (active && open && event.key === "Escape") close();
      }}
    >
      <text className="label">{label}</text>
      <Button
        id={triggerId}
        focusRequest={focusRequest}
        disabled={disabled}
        expanded={open}
        label={`${label}: ${selected}`}
        onClick={() => (open ? close() : setOpen(true))}
      >
        {selected}
      </Button>
      {open && (
        <view className="compact-select-options" accessibility-label={label}>
          {options.map(([option, title]) => (
            <Button
              key={option}
              pressed={option === value}
              label={`${label}: ${title}`}
              onClick={() => {
                onChange(option);
                close();
              }}
            >
              {title}
            </Button>
          ))}
        </view>
      )}
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
  error,
  help,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  multiline?: boolean;
  disabled?: boolean;
  focus?: boolean;
  // Validation message shown in text next to the field, never by color alone.
  error?: string;
  help?: string;
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
    "accessibility-label": error ? `${label}. ${error}` : label,
    disabled,
    maxlength: 2147483647,
    bindinput: (e: { detail: { value: string } }) => onChange(e.detail.value),
  };
  return (
    <view className="field" data-invalid={error ? "true" : undefined}>
      <text className="label">{label}</text>
      {multiline ? <textarea {...props} /> : <input {...props} />}
      {help && !error && <text className="muted">{help}</text>}
      {error && <text className="validation">{error}</text>}
    </view>
  );
}
export function Choices<T extends string | number>({
  label,
  value,
  options,
  onChange,
  disabled = false,
  exclusive = false,
}: {
  label: string;
  value: T;
  options: readonly (readonly [T, string])[];
  onChange: (v: T) => void;
  disabled?: boolean;
  // Expose the pressed state to assistive technology for exclusive choices.
  exclusive?: boolean;
}) {
  return (
    <view className="field">
      <text className="label">{label}</text>
      <view className="row wrap">
        {options.map(([v, title]) => (
          <Button
            key={v}
            disabled={disabled}
            primary={exclusive ? undefined : v === value}
            pressed={exclusive ? v === value : undefined}
            label={exclusive ? `${label}: ${title}` : undefined}
            onClick={() => onChange(v)}
          >
            {title}
          </Button>
        ))}
      </view>
    </view>
  );
}
// Multi-select chips: every option exposes its own pressed state.
export function MultiChoices({
  label,
  selected,
  options,
  onToggle,
  disabled = false,
  extra,
}: {
  label: string;
  selected: readonly string[];
  options: readonly { id: string; title: string; swatch?: string }[];
  onToggle: (id: string) => void;
  disabled?: boolean;
  // Field-specific "Unassigned" style choice that ORs with the selection.
  extra?: { title: string; pressed: boolean; onToggle: () => void };
}) {
  return (
    <view className="field">
      <text className="label">{label}</text>
      <view className="row wrap">
        {extra && (
          <Button
            disabled={disabled}
            pressed={extra.pressed}
            label={`${label}: ${extra.title}`}
            onClick={extra.onToggle}
          >
            {extra.title}
          </Button>
        )}
        {options.map((option) => (
          <Button
            key={option.id}
            disabled={disabled}
            pressed={selected.includes(option.id)}
            swatch={option.swatch}
            label={`${label}: ${option.title}`}
            onClick={() => onToggle(option.id)}
          >
            {option.title}
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
let openDialogs = 0;
export function hasOpenDialog() {
  "background only";
  return openDialogs > 0;
}
export function Dialog({
  title,
  children,
  onClose,
  focusHeading = false,
  footer,
  returnFocusId,
  className = "",
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  focusHeading?: boolean;
  footer?: ReactNode;
  returnFocusId?: string;
  className?: string;
}) {
  const [headingId] = useState(
    () => "dialog-heading-" + Math.random().toString(36).slice(2),
  );
  useEffect(() => {
    "background only";
    openDialogs++;
    if (focusHeading)
      lynx
        .createSelectorQuery()
        .select(`#${headingId}`)
        .invoke({ method: "setFocus", params: { focus: true } })
        .exec();
    return () => {
      openDialogs--;
      if (returnFocusId)
        lynx
          .createSelectorQuery()
          .select(`#${returnFocusId}`)
          .invoke({ method: "setFocus", params: { focus: true } })
          .exec();
    };
  }, [focusHeading, headingId, returnFocusId]);
  return (
    <view
      className="overlay"
      global-bindkeydown={(e) => {
        if (e.key === "Escape") onClose();
      }}
    >
      <view
        className={className ? `dialog ${className}` : "dialog"}
        accessibility-label={title}
      >
        <view className="row between">
          {focusHeading ? (
            <text
              id={headingId}
              className="heading"
              accessibility-heading
              focusable
              data-dialog-default="true"
            >
              {title}
            </text>
          ) : (
            <text className="heading">{title}</text>
          )}
          <Button
            focusOnShow={!focusHeading && !!returnFocusId}
            onClick={onClose}
          >
            Close
          </Button>
        </view>
        <scroll-view className="dialog-scroll" scroll-orientation="vertical">
          {children}
        </scroll-view>
        {footer}
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
