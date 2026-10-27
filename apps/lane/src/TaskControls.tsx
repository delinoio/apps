// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import { Button, Field } from "./components";

type Option<T> = readonly [T, string];
type LabelOption = { id: string; title: string; swatch?: string };

function focus(id: string) {
  "background only";
  lynx
    .createSelectorQuery()
    .select(`#${id}`)
    .invoke({ method: "setFocus", params: { focus: true } })
    .exec();
}

export function TaskSelect<T extends string | number>({
  control,
  openControl,
  setOpenControl,
  label,
  value,
  options,
  onChange,
  disabled = false,
}: {
  control: string;
  openControl: string | undefined;
  setOpenControl: (control: string | undefined) => void;
  label: string;
  value: T;
  options: readonly Option<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  const [triggerId] = useState(
    () => `task-select-${Math.random().toString(36).slice(2)}`,
  );
  const [index, setIndex] = useState(0);
  const open = openControl === control;
  const selectedIndex = Math.max(
    0,
    options.findIndex(([item]) => item === value),
  );
  const current =
    options.find(([item]) => item === value)?.[1] ?? "Unavailable";
  const close = () => {
    setOpenControl(undefined);
    focus(triggerId);
  };
  const show = (first: number) => {
    if (disabled) return;
    setIndex(first);
    setOpenControl(control);
  };
  const move = (step: number) => {
    if (options.length)
      setIndex((index + step + options.length) % options.length);
  };
  const key = (pressed: string): boolean => {
    if (pressed === "Escape" && open) {
      close();
      return true;
    }
    if (pressed === "ArrowDown" || pressed === "ArrowUp") {
      if (open) move(pressed === "ArrowDown" ? 1 : -1);
      else show(selectedIndex);
      return true;
    }
    if (pressed === "Home" && open) {
      setIndex(0);
      return true;
    }
    if (pressed === "End" && open) {
      setIndex(options.length - 1);
      return true;
    }
    return false;
  };
  useEffect(() => {
    "background only";
    if (disabled && open) setOpenControl(undefined);
  }, [disabled, open, setOpenControl]);
  return (
    <view className="field task-select">
      <text className="label">{label}</text>
      <Button
        elementId={triggerId}
        label={`${label}: ${current}`}
        expanded={open}
        disabled={disabled}
        onKeyDown={key}
        onClick={() => (open ? close() : show(selectedIndex))}
      >
        {current} {open ? "▴" : "▾"}
      </Button>
      {open && (
        <scroll-view
          className="task-select-options"
          scroll-orientation="vertical"
        >
          {options.map(([option, title], optionIndex) => (
            <Button
              key={String(option)}
              label={`${label}: ${title}`}
              pressed={option === value}
              focusOnShow={optionIndex === index}
              disabled={disabled}
              onKeyDown={key}
              onClick={() => {
                if (disabled) return;
                onChange(option);
                close();
              }}
            >
              {title}
            </Button>
          ))}
        </scroll-view>
      )}
    </view>
  );
}

export function TaskLabelSelect({
  openControl,
  setOpenControl,
  selected,
  options,
  onToggle,
  limit,
  disabled,
}: {
  openControl: string | undefined;
  setOpenControl: (control: string | undefined) => void;
  selected: readonly string[];
  options: readonly LabelOption[];
  onToggle: (id: string) => void;
  limit: number;
  disabled: boolean;
}) {
  const [triggerId] = useState(
    () => `task-labels-${Math.random().toString(36).slice(2)}`,
  );
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(-1);
  const open = openControl === "labels";
  const needle = query.trim().toLowerCase();
  const visible = options.filter((option) =>
    option.title.toLowerCase().includes(needle),
  );
  const titleOf = (id: string) =>
    options.find((option) => option.id === id)?.title ?? "(unavailable label)";
  const close = () => {
    setOpenControl(undefined);
    focus(triggerId);
  };
  const key = (pressed: string): boolean => {
    if (pressed === "Escape" && open) {
      close();
      return true;
    }
    if (
      (pressed === "ArrowDown" || pressed === "ArrowUp") &&
      open &&
      visible.length
    ) {
      setIndex(
        (index + (pressed === "ArrowDown" ? 1 : -1) + visible.length) %
          visible.length,
      );
      return true;
    }
    return false;
  };
  useEffect(() => {
    "background only";
    if (disabled && open) setOpenControl(undefined);
  }, [disabled, open, setOpenControl]);
  return (
    <view className="field task-label-select">
      <text className="label">Labels</text>
      <view className="row wrap task-selected-labels">
        {selected.map((id) => (
          <Button
            key={id}
            label={`Remove ${titleOf(id)} from Labels`}
            disabled={disabled}
            swatch={options.find((option) => option.id === id)?.swatch}
            onClick={() => onToggle(id)}
          >
            {titleOf(id)} ×
          </Button>
        ))}
      </view>
      <Button
        elementId={triggerId}
        label={`Labels: ${selected.length} selected`}
        expanded={open}
        disabled={disabled}
        onKeyDown={(pressed) => {
          if (key(pressed)) return true;
          if (pressed === "ArrowDown" && !open && !disabled) {
            setQuery("");
            setIndex(-1);
            setOpenControl("labels");
            return true;
          }
          return false;
        }}
        onClick={() => {
          if (open) close();
          else {
            setQuery("");
            setIndex(-1);
            setOpenControl("labels");
          }
        }}
      >
        Select labels {open ? "▴" : "▾"}
      </Button>
      {open && (
        <scroll-view
          className="task-select-options"
          scroll-orientation="vertical"
        >
          <Field
            label="Search labels"
            value={query}
            onChange={(value) => {
              setQuery(value);
              setIndex(-1);
            }}
            onKeyDown={key}
            disabled={disabled}
            focus
          />
          {visible.length === 0 && (
            <text className="muted">No matching labels.</text>
          )}
          {visible.map((option, optionIndex) => (
            <Button
              key={option.id}
              label={`Labels: ${option.title}`}
              pressed={selected.includes(option.id)}
              swatch={option.swatch}
              focusOnShow={optionIndex === index}
              disabled={
                disabled ||
                (!selected.includes(option.id) && selected.length >= limit)
              }
              onKeyDown={key}
              onClick={() => onToggle(option.id)}
            >
              {option.title}
            </Button>
          ))}
        </scroll-view>
      )}
    </view>
  );
}
