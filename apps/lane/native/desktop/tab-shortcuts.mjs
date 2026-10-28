// SPDX-License-Identifier: Apache-2.0
// Application menu accelerators are local to Lane's focused window.
import { TabCommand } from "../../src/tab-commands.ts";
export { TabCommand } from "../../src/tab-commands.ts";

export function tabShortcutItems(platform, dispatch) {
  const mac = platform === "darwin";
  const items = [];
  const add = (label, accelerator, command, index) => {
    items.push({
      id: `tab-${items.length}`,
      label,
      accelerator,
      command,
      index,
      click: (_item, focusedWindow, event) =>
        dispatch(
          { command, ...(index === undefined ? {} : { index }) },
          focusedWindow,
          event,
        ),
    });
  };
  add("New tab", mac ? "Command+T" : "Control+T", TabCommand.New);
  add("Close tab", mac ? "Command+W" : "Control+W", TabCommand.Close);
  if (!mac) add("Close tab", "Control+F4", TabCommand.Close);
  add(
    "Reopen closed tab",
    mac ? "Command+Shift+T" : "Control+Shift+T",
    TabCommand.Reopen,
  );
  for (const [label, accelerator] of mac
    ? [
        ["Next tab", "Control+Tab"],
        ["Next tab", "Control+PageDown"],
        ["Next tab", "Command+Alt+Right"],
        ["Next tab", "Command+Shift+]"],
        ["Next tab", "Command+PageDown"],
      ]
    : [
        ["Next tab", "Control+Tab"],
        ["Next tab", "Control+PageDown"],
      ])
    add(label, accelerator, TabCommand.Next);
  for (const [label, accelerator] of mac
    ? [
        ["Previous tab", "Control+Shift+Tab"],
        ["Previous tab", "Control+PageUp"],
        ["Previous tab", "Command+Alt+Left"],
        ["Previous tab", "Command+Shift+["],
        ["Previous tab", "Command+PageUp"],
      ]
    : [
        ["Previous tab", "Control+Shift+Tab"],
        ["Previous tab", "Control+PageUp"],
      ])
    add(label, accelerator, TabCommand.Previous);
  for (let index = 0; index < 8; index++)
    add(
      `Select tab ${index + 1}`,
      `${mac ? "Command" : "Control"}+${index + 1}`,
      TabCommand.SelectIndex,
      index,
    );
  add(
    "Select last tab",
    `${mac ? "Command" : "Control"}+9`,
    TabCommand.SelectLast,
  );
  add("Move tab earlier", "Control+Shift+PageUp", TabCommand.MoveEarlier);
  add("Move tab later", "Control+Shift+PageDown", TabCommand.MoveLater);
  add(
    "Move focused tab first",
    `${mac ? "Command" : "Control"}+Shift+Home`,
    TabCommand.MoveFirst,
  );
  add(
    "Move focused tab last",
    `${mac ? "Command" : "Control"}+Shift+End`,
    TabCommand.MoveLast,
  );
  return items;
}

export function tabCommandAvailable(request, state) {
  if (!state || state.count < 1) return false;
  const { command, index } = request;
  if (command === TabCommand.Reopen) return state.history > 0;
  if (command === TabCommand.SelectIndex) return index < state.count;
  if (command === TabCommand.MoveEarlier) return state.selectedIndex > 0;
  if (command === TabCommand.MoveLater)
    return state.selectedIndex < state.count - 1;
  if (command === TabCommand.MoveFirst) return state.focusedIndex > 0;
  if (command === TabCommand.MoveLast)
    return state.focusedIndex >= 0 && state.focusedIndex < state.count - 1;
  if (command === TabCommand.New || command === TabCommand.Close) return true;
  return state.count > 1;
}

export function dispatchReadyTabCommand(
  request,
  focusedWindow,
  event,
  owner,
  session,
  state,
  stopping,
) {
  if (
    stopping ||
    !owner ||
    owner.isDestroyed() ||
    !owner.isFocused() ||
    (focusedWindow && focusedWindow !== owner) ||
    !session?.ready ||
    event?.repeat ||
    event?.isComposing ||
    !tabCommandAvailable(request, state)
  )
    return false;
  owner.sendGlobalEvent("lane.workspace.tabCommand", request);
  return true;
}
