// SPDX-License-Identifier: Apache-2.0
export const GRID_COLUMNS = 12;
export const MAX_ROW = 999;
export const MAX_HEIGHT = 12;
/** Pixel height of one grid row; a 2-row widget fits the five-line states. */
export const ROW_HEIGHT = 84;
export const GRID_GAP = 10;
export const DEFAULT_WIDTH = 4;
export const DEFAULT_HEIGHT = 2;

export enum LayoutMode {
  /** 1280px and up: the stored 12-column layout, 3 default widgets across. */
  Wide = "wide",
  /** 768–1279px: 2 widgets across in stored order. */
  Tablet = "tablet",
  /** Below 768px: 1 column in stored order. */
  Phone = "phone",
}

export function layoutMode(width: number): LayoutMode {
  if (width >= 1280) return LayoutMode.Wide;
  if (width >= 768) return LayoutMode.Tablet;
  return LayoutMode.Phone;
}

export interface Box {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export function clampBox(box: Box): Box {
  const width = Math.max(1, Math.min(GRID_COLUMNS, Math.round(box.width)));
  const x = Math.max(0, Math.min(GRID_COLUMNS - width, Math.round(box.x)));
  const height = Math.max(1, Math.min(MAX_HEIGHT, Math.round(box.height)));
  const y = Math.max(0, Math.min(MAX_ROW, Math.round(box.y)));
  return { ...box, x, y, width, height };
}

export const overlaps = (a: Box, b: Box) =>
  a.id !== b.id &&
  a.x < b.x + b.width &&
  b.x < a.x + a.width &&
  a.y < b.y + b.height &&
  b.y < a.y + a.height;

/** Stored order: top to bottom, then left to right. */
export const ordered = (boxes: Box[]) =>
  [...boxes].sort((a, b) => a.y - b.y || a.x - b.x || (a.id < b.id ? -1 : 1));

/**
 * The server accepts overlapping widgets, so the client resolves placement:
 * every widget that overlaps a placed one moves down below it, in order.
 */
export function resolveCollisions(boxes: Box[], pinned: string): Box[] {
  const result = boxes.map((box) => ({ ...box }));
  const placed = result.filter((box) => box.id === pinned);
  const rest = ordered(result.filter((box) => box.id !== pinned));
  for (const box of rest) {
    let guard = 0;
    while (guard++ <= MAX_ROW) {
      const blocker = placed.find((other) => overlaps(box, other));
      if (!blocker) break;
      box.y = Math.min(MAX_ROW, blocker.y + blocker.height);
      if (box.y === MAX_ROW) break;
    }
    placed.push(box);
  }
  return result;
}

export enum EditKey {
  Left = "ArrowLeft",
  Right = "ArrowRight",
  Up = "ArrowUp",
  Down = "ArrowDown",
}

export const isEditKey = (key: string): key is EditKey =>
  (Object.values(EditKey) as string[]).includes(key);

/** Arrow keys move one cell; Shift with arrows resizes, as the record requires. */
export function applyKey(
  boxes: Box[],
  id: string,
  key: EditKey,
  resize: boolean,
): Box[] {
  const target = boxes.find((box) => box.id === id);
  if (!target) return boxes;
  const delta = {
    [EditKey.Left]: [-1, 0],
    [EditKey.Right]: [1, 0],
    [EditKey.Up]: [0, -1],
    [EditKey.Down]: [0, 1],
  }[key];
  const next = clampBox(
    resize
      ? {
          ...target,
          width: target.width + delta[0],
          height: target.height + delta[1],
        }
      : { ...target, x: target.x + delta[0], y: target.y + delta[1] },
  );
  return resolveCollisions(
    boxes.map((box) => (box.id === id ? next : box)),
    id,
  );
}

/**
 * Incorporates a server dashboard returned by an add or remove during editing
 * into the unsaved draft: surviving widgets keep their draft positions, new
 * widgets take the server position and removed widgets leave the draft. The
 * pending moves and resizes are still saved together with Done.
 */
export const keepDraftPositions = (draft: Box[], next: Box[]): Box[] => {
  const drafted = new Map(draft.map((box) => [box.id, box]));
  return next.map((box) => drafted.get(box.id) ?? box);
};

/** First free position for a new widget, scanning rows top to bottom. */
export function placeNew(
  boxes: Box[],
  id: string,
  width = DEFAULT_WIDTH,
  height = DEFAULT_HEIGHT,
): Box {
  for (let y = 0; y <= MAX_ROW; y++)
    for (let x = 0; x + width <= GRID_COLUMNS; x++) {
      const candidate = { id, x, y, width, height };
      if (!boxes.some((box) => overlaps(candidate, box))) return candidate;
    }
  return { id, x: 0, y: MAX_ROW, width, height };
}

export const gridRows = (boxes: Box[]) =>
  boxes.reduce((rows, box) => Math.max(rows, box.y + box.height), 0);

/** Screen-reader announcement after a keyboard move or resize. */
export const describeBox = (title: string, box: Box) =>
  `${title}: column ${box.x + 1}, row ${box.y + 1}, ${box.width} ${
    box.width === 1 ? "column" : "columns"
  } wide, ${box.height} ${box.height === 1 ? "row" : "rows"} tall.`;

export const changedBoxes = (before: Box[], after: Box[]) =>
  after.filter((box) => {
    const previous = before.find((entry) => entry.id === box.id);
    return (
      !previous ||
      previous.x !== box.x ||
      previous.y !== box.y ||
      previous.width !== box.width ||
      previous.height !== box.height
    );
  });
