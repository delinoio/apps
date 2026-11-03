// SPDX-License-Identifier: Apache-2.0
import type { ReactNode } from "@lynx-js/react";
import {
  GRID_COLUMNS,
  gridRows,
  LayoutMode,
  ordered,
  ROW_HEIGHT,
  type Box,
} from "../model/grid";

/**
 * Wide layouts place widgets on the stored 12-column grid. Tablet layouts flow
 * two across and phone layouts one column, both in stored order.
 */
export function Grid({
  mode,
  boxes,
  render,
}: {
  mode: LayoutMode;
  boxes: Box[];
  render: (box: Box) => ReactNode;
}) {
  if (mode === LayoutMode.Wide)
    return (
      <view
        className="ad-grid"
        style={{ height: `${gridRows(boxes) * ROW_HEIGHT}px` }}
      >
        {ordered(boxes).map((box) => (
          <view
            key={box.id}
            className="ad-cell"
            style={{
              left: `${(box.x / GRID_COLUMNS) * 100}%`,
              top: `${box.y * ROW_HEIGHT}px`,
              width: `${(box.width / GRID_COLUMNS) * 100}%`,
              height: `${box.height * ROW_HEIGHT}px`,
            }}
          >
            {render(box)}
          </view>
        ))}
      </view>
    );
  return (
    <view className="ad-flow">
      {ordered(boxes).map((box) => (
        <view
          key={box.id}
          className={
            mode === LayoutMode.Tablet && box.width < 7
              ? "ad-flow-half"
              : "ad-flow-full"
          }
          style={
            mode === LayoutMode.Tablet
              ? { minHeight: `${box.height * ROW_HEIGHT}px` }
              : undefined
          }
        >
          {render(box)}
        </view>
      ))}
    </view>
  );
}
