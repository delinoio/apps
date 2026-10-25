// SPDX-License-Identifier: Apache-2.0
import { expect, it, rstest } from "@rstest/core";
import { installWebSupport } from "../src/web";

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const createInput = (label: string, invalid = true) => {
  const input = document.createElement("x-input");
  input.className = `lui-input${invalid ? " lui-invalid" : ""}`;
  input.setAttribute("accessibility-label", label);
  return input;
};

const innerInput = (host: HTMLElement) =>
  host.shadowRoot?.querySelector("input");

it("synchronizes real XInput semantics after delayed definition within each live bridge", async () => {
  expect(customElements.get("x-input")).toBeUndefined();

  const view = document.createElement("div");
  const otherView = document.createElement("div");
  const disposedView = document.createElement("div");
  document.body.append(view, otherView, disposedView);
  const root = view.attachShadow({ mode: "open" });
  const otherRoot = otherView.attachShadow({ mode: "open" });
  const disposedRoot = disposedView.attachShadow({ mode: "open" });
  const dispose = installWebSupport(root);
  const disposeEarly = installWebSupport(disposedRoot);
  const whenDefined = rstest.spyOn(customElements, "whenDefined");

  try {
    const initial = createInput("Account name");
    const updated = createInput("Old name");
    const cleared = createInput("Remove me");
    const removed = createInput("Removed");
    const outside = createInput("Other view");
    const disposedInput = createInput("Disposed");
    root.append(initial, updated, cleared, removed);
    otherRoot.append(outside);
    disposedRoot.append(disposedInput);
    await settle();

    expect(initial.getAttribute("aria-label")).toBe("Account name");
    expect(initial.getAttribute("aria-invalid")).toBe("true");
    expect(innerInput(initial)).toBeUndefined();

    updated.setAttribute("accessibility-label", "Updated name");
    updated.classList.remove("lui-invalid");
    cleared.removeAttribute("accessibility-label");
    cleared.classList.remove("lui-invalid");
    initial.classList.add("unrelated");
    await settle();
    expect(whenDefined).toHaveBeenCalledTimes(2);

    removed.remove();
    disposeEarly();

    // This import registers the actual pinned web element after the first pass.
    await import("@lynx-js/web-elements/XInput");
    await settle();

    expect(innerInput(initial)?.getAttribute("aria-label")).toBe(
      "Account name",
    );
    expect(innerInput(initial)?.getAttribute("aria-invalid")).toBe("true");
    expect(innerInput(updated)?.getAttribute("aria-label")).toBe(
      "Updated name",
    );
    expect(innerInput(updated)?.getAttribute("aria-invalid")).toBe("false");
    expect(innerInput(cleared)?.hasAttribute("aria-label")).toBe(false);
    expect(innerInput(cleared)?.getAttribute("aria-invalid")).toBe("false");
    expect(innerInput(removed)).toBeUndefined();
    expect(innerInput(disposedInput)?.hasAttribute("aria-label")).toBe(false);
    expect(innerInput(disposedInput)?.hasAttribute("aria-invalid")).toBe(false);
    expect(innerInput(outside)?.hasAttribute("aria-label")).toBe(false);
    expect(innerInput(outside)?.hasAttribute("aria-invalid")).toBe(false);

    const defined = createInput("Defined input");
    root.append(defined);
    await settle();
    expect(innerInput(defined)?.getAttribute("aria-label")).toBe(
      "Defined input",
    );
    expect(innerInput(defined)?.getAttribute("aria-invalid")).toBe("true");

    defined.setAttribute("accessibility-label", "Changed input");
    defined.classList.remove("lui-invalid");
    await settle();
    expect(innerInput(defined)?.getAttribute("aria-label")).toBe(
      "Changed input",
    );
    expect(innerInput(defined)?.getAttribute("aria-invalid")).toBe("false");

    defined.setAttribute("accessibility-label", "");
    await settle();
    expect(innerInput(defined)?.hasAttribute("aria-label")).toBe(false);
  } finally {
    whenDefined.mockRestore();
    dispose();
    disposeEarly();
    view.remove();
    otherView.remove();
    disposedView.remove();
  }
});
