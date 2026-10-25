// SPDX-License-Identifier: Apache-2.0
import { afterEach, beforeEach, expect, it, rstest } from "@rstest/core";
import {
  cleanup,
  fireEvent,
  render,
  waitFor,
} from "@lynx-js/react/testing-library";
import { createRef } from "@lynx-js/react";
import { Button, Dialog, Input, Switch, Tabs, type FocusHandle } from "../src";
import * as theme from "../src/theme";
import { FocusContext, type FocusScope } from "../src/focus";

// The ReactLynx fixture has no native input or focus implementation. Assert the
// host-method boundary here; the real web gallery validates host integration.
beforeEach(() => {
  rstest.spyOn(theme, "invoke").mockImplementation(() => {});
  // The ReactLynx fixture does not implement the native NodesRef.path method.
  // Replay its documented sibling-index path from the fixture's rendered tree.
  const prototype = Object.getPrototypeOf(
    lynx.createSelectorQuery().select("body"),
  );
  rstest.spyOn(prototype, "path").mockImplementation(function (
    this: { _nodeSelectToken: { type: number; identifier: string } },
    callback: unknown,
  ) {
    const complete = callback as (
      result: { path: { index: number }[] } | null,
      status: { code: number; data: string },
    ) => void;
    const token = this._nodeSelectToken;
    return {
      exec() {
        const node =
          token.type === 0
            ? document.querySelector(token.identifier)
            : Array.from(document.querySelectorAll("*")).find(
                (element) =>
                  (element as Element & { $$uiSign?: number }).$$uiSign ===
                  Number(token.identifier),
              );
        if (!node) {
          complete(null, { code: 1, data: "node not found" });
          return;
        }
        const data: { index: number }[] = [];
        for (
          let current: Node | null = node;
          current?.parentNode;
          current = current.parentNode
        ) {
          data.push({
            index: Array.prototype.indexOf.call(
              current.parentNode.childNodes,
              current,
            ),
          });
        }
        complete({ path: data }, { code: 0, data: "succeed" });
      },
    };
  });
});
afterEach(() => {
  cleanup();
  rstest.restoreAllMocks();
});

it("preserves scope order across loading and callback ref changes", async () => {
  const scope: FocusScope = { entries: new Map(), active: null };
  const oldRef = rstest.fn();
  const newRef = rstest.fn();
  const controls = (loading: boolean, focusRef: typeof oldRef) => (
    <FocusContext.Provider value={scope}>
      <Input value="" onValueChange={() => {}} accessibilityLabel="Name" />
      <Button loading={loading} focusRef={focusRef}>
        Submit
      </Button>
      <Button>Close</Button>
    </FocusContext.Provider>
  );
  const { rerender, unmount } = render(controls(false, oldRef));
  await waitFor(() => expect(scope.entries.size).toBe(3));
  const order = Array.from(scope.entries.keys());
  rerender(controls(true, oldRef));
  await waitFor(() => expect(scope.entries.get(order[1])!.disabled).toBe(true));
  expect(Array.from(scope.entries.keys())).toEqual(order);
  rerender(controls(false, newRef));
  await waitFor(() =>
    expect(newRef).toHaveBeenCalledWith(
      expect.objectContaining({ focus: expect.any(Function) }),
    ),
  );
  expect(oldRef).toHaveBeenLastCalledWith(null);
  expect(scope.entries.get(order[1])!.disabled).toBe(false);
  expect(Array.from(scope.entries.keys())).toEqual(order);
  unmount();
  await waitFor(() => expect(scope.entries.size).toBe(0));
});

it("synchronizes controlled input and focuses the critical input", async () => {
  const change = rstest.fn();
  const { container, rerender } = render(
    <Input
      id="name"
      value="first"
      accessibilityLabel="Name"
      autoFocus
      onValueChange={change}
    />,
  );
  await waitFor(() =>
    expect(theme.invoke).toHaveBeenCalledWith(
      expect.anything(),
      "setValue",
      { value: "first" },
      expect.anything(),
    ),
  );
  expect(theme.invoke).toHaveBeenCalledWith(
    expect.anything(),
    "focus",
    {},
    expect.anything(),
  );
  fireEvent(
    lynx.createSelectorQuery().select("#name") as unknown as Node,
    Object.assign(
      new CustomEvent("bindEvent:input", { detail: { value: "edited" } }),
      { eventType: "bindEvent", eventName: "input" },
    ),
  );
  expect(change).toHaveBeenCalledWith("edited");
  await waitFor(() =>
    expect(theme.invoke).toHaveBeenLastCalledWith(
      expect.anything(),
      "setValue",
      { value: "first" },
      expect.anything(),
    ),
  );
  rerender(
    <Input
      id="name"
      value="reset"
      accessibilityLabel="Name"
      onValueChange={change}
    />,
  );
  await waitFor(() =>
    expect(theme.invoke).toHaveBeenLastCalledWith(
      expect.anything(),
      "setValue",
      { value: "reset" },
      expect.anything(),
    ),
  );
});

it("uses Enter and Space without activating repeated keys", () => {
  const action = rstest.fn();
  const { container } = render(<Button onPress={action}>Save</Button>);
  fireEvent.keydown(container.firstChild!, { key: "Enter" });
  fireEvent.keydown(container.firstChild!, { key: " ", repeat: true });
  expect(action).toHaveBeenCalledTimes(1);
});

it("skips disabled tabs and wraps keyboard navigation", async () => {
  const change = rstest.fn();
  const items = [
    { value: "a", label: "First", content: null },
    { value: "b", label: "Disabled", content: null, disabled: true },
    { value: "c", label: "Last", content: null },
  ];
  const { container } = render(
    <Tabs value="a" onValueChange={change} items={items} label="Tabs" />,
  );
  await waitFor(() =>
    expect(container.querySelectorAll(".lui-tab").length).toBe(3),
  );
  const first = container.querySelector(".lui-tab")!;
  fireEvent.keydown(first, { key: "ArrowRight" });
  expect(change).toHaveBeenLastCalledWith("c");
  fireEvent.keydown(first, { key: "ArrowLeft" });
  expect(change).toHaveBeenLastCalledWith("c");
});

it("focuses dialog input, closes on Escape and restores trigger focus", async () => {
  const initial = createRef<FocusHandle>();
  const restore = { current: { focus: rstest.fn() } };
  const change = rstest.fn();
  const props = {
    onOpenChange: change,
    title: "Edit",
    description: "Change name",
    closeLabel: "Cancel",
    returnFocusRef: restore,
    initialFocusRef: initial,
  };
  const { container, rerender } = render(
    <Dialog {...props} open>
      <Input
        value=""
        onValueChange={() => {}}
        accessibilityLabel="Name"
        focusRef={initial}
      />
    </Dialog>,
  );
  await waitFor(() =>
    expect(theme.invoke).toHaveBeenCalledWith(
      expect.anything(),
      "focus",
      {},
      expect.anything(),
    ),
  );
  fireEvent.keydown(container.querySelector(".lui-dialog-backdrop")!, {
    key: "Tab",
  });
  await waitFor(() =>
    expect(theme.invoke).toHaveBeenCalledWith(
      expect.anything(),
      "setFocus",
      { focus: true },
      expect.anything(),
    ),
  );
  fireEvent.keydown(container.querySelector(".lui-dialog-backdrop")!, {
    key: "Escape",
  });
  expect(change).toHaveBeenCalledWith(false);
  rerender(
    <Dialog {...props} open={false}>
      {null}
    </Dialog>,
  );
  await waitFor(() => expect(restore.current.focus).toHaveBeenCalledTimes(1));
});

it("uses the current native order for inserted, removed and moved dialog controls", async () => {
  const restore = { current: { focus: rstest.fn() } };
  const switchRef = createRef<FocusHandle>();
  const inputRef = createRef<FocusHandle>();
  const change = rstest.fn();
  const children = (
    showInput: boolean,
    inputFirst = false,
    disabled = false,
  ) => {
    const toggle = (
      <Switch
        key="switch"
        id="dialog-switch"
        checked={false}
        onCheckedChange={() => {}}
        label="Enable"
        checkedLabel="On"
        uncheckedLabel="Off"
        focusRef={switchRef}
      />
    );
    const input = showInput ? (
      <Input
        key="input"
        id="dialog-input"
        value=""
        onValueChange={() => {}}
        accessibilityLabel="Name"
        focusRef={inputRef}
        disabled={disabled}
      />
    ) : null;
    return inputFirst ? [input, toggle] : [toggle, input];
  };
  const dialog = (showInput: boolean, inputFirst = false, disabled = false) => (
    <Dialog
      open
      onOpenChange={change}
      title="Edit"
      description="Change settings"
      closeLabel="Close"
      returnFocusRef={restore}
    >
      {children(showInput, inputFirst, disabled)}
    </Dialog>
  );
  const { container, rerender, unmount } = render(dialog(false));
  const focused = () =>
    rstest
      .mocked(theme.invoke)
      .mock.calls.filter(
        ([, method]) => method === "focus" || method === "setFocus",
      )
      .map(
        ([ref]) =>
          container.querySelector(
            (ref as unknown as { selector: string }).selector,
          )?.className,
      );
  const press = async (key: string, shiftKey = false) => {
    const before = focused().length;
    fireEvent.keydown(container.querySelector(".lui-dialog-backdrop")!, {
      key,
      shiftKey,
    });
    await waitFor(() => expect(focused()).toHaveLength(before + 1));
    return focused().at(-1);
  };
  await waitFor(() => expect(focused()).toHaveLength(1));
  expect(focused()[0]).toContain("lui-toggle");
  const survivingSwitch = switchRef.current;
  expect(await press("Tab")).toContain("lui-dialog-close");

  rerender(dialog(true));
  await waitFor(() => expect(inputRef.current).not.toBeNull());
  expect(switchRef.current).toBe(survivingSwitch);
  expect(focused()).toHaveLength(2);
  expect(await press("Tab", true)).toContain("lui-input");
  expect(await press("Tab", true)).toContain("lui-toggle");
  expect(await press("Tab", true)).toContain("lui-dialog-close");
  expect(await press("Tab")).toContain("lui-toggle");
  expect(await press("Tab")).toContain("lui-input");
  expect(await press("Tab")).toContain("lui-dialog-close");

  const survivingInput = inputRef.current;
  rerender(dialog(true, true));
  expect(switchRef.current).toBe(survivingSwitch);
  expect(inputRef.current).toBe(survivingInput);
  expect(focused()).toHaveLength(8);
  expect(await press("Tab")).toContain("lui-input");
  expect(await press("Tab")).toContain("lui-toggle");
  expect(await press("Tab")).toContain("lui-dialog-close");

  rerender(dialog(true, true, true));
  expect(await press("Tab", true)).toContain("lui-toggle");
  rerender(dialog(false));
  await waitFor(() => expect(inputRef.current).toBeNull());
  expect(await press("Tab")).toContain("lui-dialog-close");
  rerender(dialog(true));
  await waitFor(() => expect(inputRef.current).not.toBeNull());
  expect(switchRef.current).toBe(survivingSwitch);
  expect(await press("Tab")).toContain("lui-toggle");
  expect(await press("Tab")).toContain("lui-input");
  rerender(dialog(false));
  await waitFor(() => expect(inputRef.current).toBeNull());
  expect(await press("Tab", true)).toContain("lui-dialog-close");
  unmount();
  expect(restore.current.focus).toHaveBeenCalledTimes(1);
});
