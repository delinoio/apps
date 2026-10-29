// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import { CompletedSelection } from "@repo/protos/lane/v1/lane_pb";
import { Button } from "./components";
import { completedOptions, TaskLayout } from "./completedDisplay";

export function DisplayOptions({
  layout,
  setLayout,
  completed,
  setCompleted,
}: {
  layout: TaskLayout;
  setLayout: (layout: TaskLayout) => void;
  completed: CompletedSelection;
  setCompleted: (selection: CompletedSelection) => void;
}) {
  const [open, setOpen] = useState(false);
  const [periodOpen, setPeriodOpen] = useState(false);
  const [id] = useState(() => `display-${Math.random().toString(36).slice(2)}`);
  const triggerId = `${id}-trigger`;
  const periodId = `${id}-period`;
  const layoutIds = [TaskLayout.List, TaskLayout.Board];
  const periodIds = completedOptions.map(([value]) => value);
  const focus = (target: string) => {
    "background only";
    lynx
      .createSelectorQuery()
      .select(`#${target}`)
      .invoke({ method: "setFocus", params: { focus: true } })
      .exec();
  };
  useEffect(() => {
    "background only";
    if (open && !periodOpen) focus(`${id}-layout-${layout}`);
  }, [open]);
  useEffect(() => {
    "background only";
    if (periodOpen) focus(`${id}-completed-${completed}`);
  }, [periodOpen]);
  const move = (
    key: string,
    values: readonly (string | number)[],
    current: string | number,
    prefix: string,
  ) => {
    if (key !== "ArrowDown" && key !== "ArrowUp") return;
    const offset = key === "ArrowDown" ? 1 : -1;
    const next =
      (values.indexOf(current) + offset + values.length) % values.length;
    focus(`${id}-${prefix}-${values[next]}`);
  };
  const close = () => {
    if (periodOpen) {
      setPeriodOpen(false);
      focus(periodId);
    } else if (open) {
      setOpen(false);
      focus(triggerId);
    }
  };
  return (
    <view
      className="display-options"
      global-bindkeydown={(event) => {
        if (event.key === "Escape") close();
      }}
    >
      <Button
        id={triggerId}
        expanded={open}
        onClick={() => {
          if (open) {
            setPeriodOpen(false);
            setOpen(false);
          } else setOpen(true);
        }}
      >
        Display Options
      </Button>
      {open && (
        <view
          className="display-options-menu"
          accessibility-label="Display Options"
        >
          <text className="label">Layout</text>
          <view className="display-options-layout">
            {layoutIds.map((value) => (
              <Button
                id={`${id}-layout-${value}`}
                key={value}
                pressed={layout === value}
                label={`Layout: ${value === TaskLayout.List ? "List" : "Board"}`}
                onClick={() => setLayout(value)}
                onKeyDown={(key) => move(key, layoutIds, value, "layout")}
              >
                {value === TaskLayout.List ? "List" : "Board"}
              </Button>
            ))}
          </view>
          <view className="display-options-divider" />
          <view className="display-options-period-row">
            <text className="label">Completed items</text>
            <Button
              id={periodId}
              expanded={periodOpen}
              label={`Completed items: ${completedOptions.find(([value]) => value === completed)?.[1]}`}
              onClick={() => setPeriodOpen(!periodOpen)}
              onKeyDown={(key) => {
                if (key === "ArrowDown") setPeriodOpen(true);
              }}
            >
              {completedOptions.find(([value]) => value === completed)?.[1]} ▾
            </Button>
          </view>
          {periodOpen && (
            <view
              className="display-options-period-menu"
              accessibility-label="Completed items"
            >
              {completedOptions.map(([value, label]) => (
                <Button
                  id={`${id}-completed-${value}`}
                  key={value}
                  pressed={completed === value}
                  label={`Select completed items: ${label}`}
                  onClick={() => {
                    setCompleted(value);
                    setPeriodOpen(false);
                    focus(periodId);
                  }}
                  onKeyDown={(key) => move(key, periodIds, value, "completed")}
                >
                  {label}
                </Button>
              ))}
            </view>
          )}
        </view>
      )}
    </view>
  );
}
