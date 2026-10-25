// SPDX-License-Identifier: Apache-2.0
import { afterEach, beforeEach, expect, it, rstest } from "@rstest/core";
import {
  act,
  cleanup,
  fireEvent,
  render,
  waitFor,
} from "@lynx-js/react/testing-library";
import { createRef, useState, type Ref } from "@lynx-js/react";
import {
  Button,
  Checkbox,
  Dialog,
  Input,
  Switch,
  Tabs,
  type FocusHandle,
} from "../src";
import * as theme from "../src/theme";
import { FocusContext, type FocusScope } from "../src/focus";

// The ReactLynx fixture has no native input or focus implementation. Assert the
// host-method boundary here; the real web gallery validates host integration.
beforeEach(() => {
  rstest.spyOn(theme, "invoke").mockImplementation(() => {});
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

const focusControls = [
  {
    name: "Button",
    render: (focusRef: Ref<FocusHandle>, disabled: boolean) => (
      <Button focusRef={focusRef} disabled={disabled}>
        Save
      </Button>
    ),
  },
  {
    name: "Input",
    render: (focusRef: Ref<FocusHandle>, disabled: boolean) => (
      <Input
        value=""
        onValueChange={() => {}}
        accessibilityLabel="Name"
        focusRef={focusRef}
        disabled={disabled}
      />
    ),
  },
  {
    name: "Checkbox",
    render: (focusRef: Ref<FocusHandle>, disabled: boolean) => (
      <Checkbox
        checked={false}
        onCheckedChange={() => {}}
        label="Select"
        checkedLabel="Selected"
        uncheckedLabel="Not selected"
        focusRef={focusRef}
        disabled={disabled}
      />
    ),
  },
  {
    name: "Switch",
    render: (focusRef: Ref<FocusHandle>, disabled: boolean) => (
      <Switch
        checked={false}
        onCheckedChange={() => {}}
        label="Enable"
        checkedLabel="Enabled"
        uncheckedLabel="Disabled"
        focusRef={focusRef}
        disabled={disabled}
      />
    ),
  },
];

for (const control of focusControls) {
  it(`${control.name} releases each callback ref handle after null notification`, async () => {
    const hostRequests = rstest.spyOn(theme, "invoke");
    const events: string[] = [];
    const registrations = new Set<FocusHandle>();
    const handles: FocusHandle[] = [];
    const ref =
      (name: string): Ref<FocusHandle> =>
      (value) => {
        if (!value) {
          events.push(`${name}:null`);
          return;
        }
        events.push(`${name}:handle`);
        handles.push(value);
        registrations.add(value);
        return () => {
          events.push(`${name}:cleanup`);
          registrations.delete(value);
        };
      };
    const first = ref("first");
    const second = ref("second");
    const { rerender, unmount } = render(control.render(first, false));
    await waitFor(() => expect(registrations.size).toBe(1));

    await act(async () => rerender(control.render(first, false)));
    expect(events).toEqual(["first:handle"]);

    await act(async () => rerender(control.render(second, false)));
    expect(events).toEqual([
      "first:handle",
      "first:null",
      "first:cleanup",
      "second:handle",
    ]);
    expect(registrations.size).toBe(1);
    expect(registrations.has(handles[0])).toBe(false);
    expect(registrations.has(handles[1])).toBe(true);

    await act(async () => rerender(control.render(second, true)));
    expect(events.slice(-3)).toEqual([
      "second:null",
      "second:cleanup",
      "second:handle",
    ]);
    const current = handles.at(-1)!;
    const requestsBefore = hostRequests.mock.calls.length;
    current.focus();
    expect(hostRequests.mock.calls).toHaveLength(requestsBefore);
    expect(registrations.size).toBe(1);

    await act(async () => unmount());
    expect(events.slice(-2)).toEqual(["second:null", "second:cleanup"]);
    expect(registrations.size).toBe(0);
    expect(events.filter((event) => event.endsWith(":cleanup"))).toHaveLength(
      3,
    );
  });
}

it("runs a stable callback ref cleanup once on unmount", async () => {
  const registrations = new Set<FocusHandle>();
  const cleanupHandle = rstest.fn();
  const ref: Ref<FocusHandle> = (value) => {
    if (!value) return;
    registrations.add(value);
    return () => {
      cleanupHandle(value);
      registrations.delete(value);
    };
  };
  const { unmount } = render(<Button focusRef={ref}>Save</Button>);
  await waitFor(() => expect(registrations.size).toBe(1));
  const [published] = registrations;
  await act(async () => unmount());
  expect(cleanupHandle).toHaveBeenCalledTimes(1);
  expect(cleanupHandle).toHaveBeenCalledWith(published);
  expect(registrations.size).toBe(0);
});

it("preserves Input autoFocus triggers for autoFocus, disabled and scope changes", async () => {
  const hostRequests = rstest.spyOn(theme, "invoke");
  const first = createRef<FocusHandle>();
  const second = createRef<FocusHandle>();
  const firstScope: FocusScope = { entries: new Map(), active: null };
  const secondScope: FocusScope = { entries: new Map(), active: null };
  const input = (
    scope: FocusScope,
    focusRef: Ref<FocusHandle>,
    autoFocus: boolean,
    disabled = false,
  ) => (
    <FocusContext.Provider value={scope}>
      <Input
        value=""
        onValueChange={() => {}}
        accessibilityLabel="Name"
        focusRef={focusRef}
        autoFocus={autoFocus}
        disabled={disabled}
      />
    </FocusContext.Provider>
  );
  const focusCount = () =>
    hostRequests.mock.calls.filter(([, method]) => method === "focus").length;
  const { rerender, unmount } = render(input(firstScope, first, false));
  await waitFor(() => expect(first.current).not.toBeNull());
  expect(focusCount()).toBe(0);

  await act(async () => rerender(input(firstScope, first, true)));
  expect(focusCount()).toBe(1);
  await act(async () => rerender(input(firstScope, second, true)));
  expect(first.current).toBeNull();
  expect(second.current).not.toBeNull();
  expect(focusCount()).toBe(1);

  await act(async () => rerender(input(firstScope, second, true, true)));
  expect(focusCount()).toBe(1);
  await act(async () => rerender(input(firstScope, first, true, true)));
  expect(focusCount()).toBe(1);
  await act(async () => rerender(input(firstScope, first, true)));
  expect(focusCount()).toBe(2);
  await act(async () => rerender(input(secondScope, first, true)));
  expect(focusCount()).toBe(3);
  await act(async () => rerender(input(secondScope, first, true)));
  expect(focusCount()).toBe(3);
  await act(async () => unmount());
  expect(first.current).toBeNull();
});

it("keeps void callback and object focus refs synchronized", async () => {
  const calls: Array<FocusHandle | null> = [];
  const first: Ref<FocusHandle> = (value) => {
    calls.push(value);
  };
  const second: Ref<FocusHandle> = (value) => {
    calls.push(value);
  };
  const object = createRef<FocusHandle>();
  const { rerender, unmount } = render(<Button focusRef={first}>Save</Button>);
  await waitFor(() => expect(calls).toHaveLength(1));
  await act(async () => rerender(<Button focusRef={second}>Save</Button>));
  expect(calls).toHaveLength(3);
  expect(calls[1]).toBeNull();
  expect(calls[2]?.focus).toEqual(expect.any(Function));
  await act(async () => rerender(<Button focusRef={object}>Save</Button>));
  expect(calls.at(-1)).toBeNull();
  expect(object.current?.focus).toEqual(expect.any(Function));
  await act(async () => unmount());
  expect(object.current).toBeNull();
});

it("does not restart Input autoFocus when a controlled sibling replaces its inline focus ref", async () => {
  const aRefs: Array<FocusHandle | null> = [];
  const bRef = createRef<FocusHandle>();
  let active: unknown = null;
  let aNode: unknown = null;
  let bNode: unknown = null;
  const hostRequests = rstest
    .spyOn(theme, "invoke")
    .mockImplementation((node, method) => {
      if (method !== "focus") return;
      if (aNode === null) aNode = node;
      else if (node !== aNode) bNode = node;
      active = node;
    });
  function Pair() {
    const [value, setValue] = useState("");
    return (
      <>
        <Input
          id="primary"
          value=""
          onValueChange={() => {}}
          accessibilityLabel="Primary"
          autoFocus
          focusRef={(handle) => {
            aRefs.push(handle);
          }}
        />
        <Input
          id="sibling"
          value={value}
          onValueChange={setValue}
          accessibilityLabel="Sibling"
          focusRef={bRef}
        />
      </>
    );
  }
  const { unmount } = render(<Pair />);
  await waitFor(() => expect(aNode).not.toBeNull());
  expect(theme.invoke).toHaveBeenCalledWith(
    aNode,
    "focus",
    {},
    expect.anything(),
  );
  bRef.current!.focus();
  expect(active).toBe(bNode);
  const beforeEdit = hostRequests.mock.calls.filter(
    ([node, method]) => node === aNode && method === "focus",
  ).length;
  await act(async () => {
    fireEvent(
      lynx.createSelectorQuery().select("#sibling") as unknown as Node,
      Object.assign(
        new CustomEvent("bindEvent:input", { detail: { value: "edited" } }),
        { eventType: "bindEvent", eventName: "input" },
      ),
    );
  });
  await waitFor(() => expect(aRefs).toHaveLength(3));
  expect(aRefs[1]).toBeNull();
  expect(aRefs[2]?.focus).toEqual(expect.any(Function));
  expect(active).toBe(bNode);
  expect(
    hostRequests.mock.calls.filter(
      ([node, method]) => node === aNode && method === "focus",
    ),
  ).toHaveLength(beforeEdit);
  aRefs[2]!.focus();
  expect(active).toBe(aNode);
  await act(async () => unmount());
  expect(aRefs.at(-1)).toBeNull();
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
