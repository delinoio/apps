// SPDX-License-Identifier: Apache-2.0
import { afterEach, beforeEach, expect, it, rstest } from "@rstest/core";
import {
  cleanup,
  fireEvent,
  render,
  waitFor,
} from "@lynx-js/react/testing-library";
import { createRef } from "@lynx-js/react";
import { Button, Dialog, Input, Tabs, type FocusHandle } from "../src";
import * as theme from "../src/theme";

// The ReactLynx fixture has no native input or focus implementation. Assert the
// host-method boundary here; the real web gallery validates host integration.
beforeEach(() => {
  rstest.spyOn(theme, "invoke").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  rstest.restoreAllMocks();
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
  expect(theme.invoke).toHaveBeenCalledWith(
    expect.anything(),
    "setFocus",
    { focus: true },
    expect.anything(),
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
