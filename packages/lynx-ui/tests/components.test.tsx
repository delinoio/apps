// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, it, rstest } from "@rstest/core";
import { cleanup, fireEvent, render } from "@lynx-js/react/testing-library";
import {
  Button,
  Checkbox,
  Input,
  Progress,
  Switch,
  Tabs,
  Theme,
  UIProvider,
} from "../src";

afterEach(cleanup);
describe("actions", () => {
  it("activates with tap and suppresses disabled and loading actions", () => {
    const action = rstest.fn();
    const { container, rerender } = render(
      <Button onPress={action}>Save</Button>,
    );
    fireEvent.tap(container.firstChild!);
    expect(action).toHaveBeenCalledTimes(1);
    rerender(
      <Button onPress={action} disabled>
        Save
      </Button>,
    );
    fireEvent.tap(container.firstChild!);
    rerender(
      <Button onPress={action} loading loadingLabel="Saving">
        Save
      </Button>,
    );
    fireEvent.tap(container.firstChild!);
    expect(action).toHaveBeenCalledTimes(1);
    expect(container.textContent).toBe("Saving");
  });
  it.each([Checkbox, Switch])(
    "reports controlled toggle changes",
    (Component) => {
      const change = rstest.fn();
      const props = {
        label: "Option",
        checkedLabel: "On",
        uncheckedLabel: "Off",
        onCheckedChange: change,
      };
      const { container, rerender } = render(
        <Component {...props} checked={false} />,
      );
      fireEvent.tap(container.firstChild!);
      expect(change).toHaveBeenLastCalledWith(true);
      expect(
        container.firstElementChild?.getAttribute("accessibility-value"),
      ).toBe("Off");
      rerender(<Component {...props} checked disabled />);
      fireEvent.tap(container.firstChild!);
      expect(change).toHaveBeenCalledTimes(1);
    },
  );
});
it("selects enabled tabs and replaces the visible panel", () => {
  const change = rstest.fn();
  const items = [
    { value: "a", label: "First", content: <text>Panel A</text> },
    {
      value: "b",
      label: "Blocked",
      disabled: true,
      content: <text>Panel B</text>,
    },
    { value: "c", label: "Last", content: <text>Panel C</text> },
  ];
  const { container, rerender } = render(
    <Tabs value="a" onValueChange={change} label="Sections" items={items} />,
  );
  const tabs = container.querySelectorAll(
    '[accessibility-role-description="tab"]',
  );
  fireEvent.tap(tabs[1]!);
  expect(change).not.toHaveBeenCalled();
  fireEvent.tap(tabs[2]!);
  expect(change).toHaveBeenCalledWith("c");
  rerender(
    <Tabs value="c" onValueChange={change} label="Sections" items={items} />,
  );
  expect(container.textContent).toContain("Panel C");
  expect(container.textContent).not.toContain("Panel A");
});
it("changes theme without losing child state and clamps progress", () => {
  const { container, rerender } = render(
    <UIProvider>
      <Progress value={120} label="Upload" />
    </UIProvider>,
  );
  expect(
    container.querySelector('[accessibility-value="100%"]'),
  ).not.toBeNull();
  rerender(
    <UIProvider theme={Theme.Dark}>
      <Progress value={-20} label="Upload" />
    </UIProvider>,
  );
  expect(container.querySelector(".lui-dark")).not.toBeNull();
  expect(container.querySelector('[accessibility-value="0%"]')).not.toBeNull();
});
