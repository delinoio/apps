// SPDX-License-Identifier: Apache-2.0
import { expect, it, rstest } from "@rstest/core";
import { createInvokeUIMethod } from "../node_modules/@lynx-js/web-core/dist/client/mainthread/elementAPIs/createInvokeUIMethod.js";
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
  const errors = rstest.spyOn(console, "error").mockImplementation(() => {});

  try {
    const initial = createInput("Account name");
    initial.setAttribute("default-value", "Alice");
    const updated = createInput("Old name");
    updated.setAttribute("default-value", "Alice");
    const cleared = createInput("Remove me");
    cleared.setAttribute("default-value", "");
    const removed = createInput("Removed");
    removed.setAttribute("default-value", "stale");
    const outside = createInput("Other view");
    const disposedInput = createInput("Disposed");
    disposedInput.setAttribute("default-value", "stale");
    const failing = createInput("Failure");
    failing.setAttribute("default-value", "private entry");
    Object.defineProperty(failing, "setValue", {
      configurable: true,
      value: () => {
        throw new Error("host failure");
      },
    });
    root.append(initial, updated, cleared, removed, failing);
    otherRoot.append(outside);
    disposedRoot.append(disposedInput);
    await settle();

    expect(initial.getAttribute("aria-label")).toBe("Account name");
    expect(initial.getAttribute("aria-invalid")).toBe("true");
    expect(innerInput(initial)).toBeUndefined();
    expect(
      (initial as HTMLElement & { setValue?: unknown }).setValue,
    ).toBeUndefined();
    // setValue does not use the geometry service in the pinned web-core dispatcher.
    const invokeResult = await new Promise<{ code: number }>((resolve) =>
      createInvokeUIMethod(
        null as unknown as Parameters<typeof createInvokeUIMethod>[0],
      )(initial, "setValue", { value: "Alice" }, resolve),
    );
    expect(invokeResult.code).toBe(3);

    updated.setAttribute("accessibility-label", "Updated name");
    updated.setAttribute("default-value", "Bob");
    updated.classList.remove("lui-invalid");
    cleared.removeAttribute("accessibility-label");
    cleared.classList.remove("lui-invalid");
    initial.classList.add("unrelated");
    await settle();
    expect(whenDefined).toHaveBeenCalledTimes(2);

    removed.remove();
    const replacement = createInput("Replacement");
    replacement.setAttribute("default-value", "Current");
    root.append(replacement);
    disposeEarly();

    // This import registers the actual pinned web element after the first pass.
    await import("@lynx-js/web-elements/XInput");
    await settle();

    expect(innerInput(initial)?.getAttribute("aria-label")).toBe(
      "Account name",
    );
    expect(innerInput(initial)?.value).toBe("Alice");
    expect(innerInput(updated)?.value).toBe("Bob");
    expect(innerInput(cleared)?.value).toBe("");
    expect(innerInput(initial)?.getAttribute("aria-invalid")).toBe("true");
    expect(innerInput(updated)?.getAttribute("aria-label")).toBe(
      "Updated name",
    );
    expect(innerInput(updated)?.getAttribute("aria-invalid")).toBe("false");
    expect(innerInput(cleared)?.hasAttribute("aria-label")).toBe(false);
    expect(innerInput(cleared)?.getAttribute("aria-invalid")).toBe("false");
    expect(innerInput(removed)).toBeUndefined();
    expect(innerInput(replacement)?.value).toBe("Current");
    expect(errors).toHaveBeenCalledWith("lynx-ui.host-method-failed", {
      method: "setValue",
      code: 4,
    });
    expect(JSON.stringify(errors.mock.calls)).not.toContain("private entry");
    expect(innerInput(disposedInput)?.hasAttribute("aria-label")).toBe(false);
    expect(innerInput(disposedInput)?.hasAttribute("aria-invalid")).toBe(false);
    expect(innerInput(disposedInput)?.value).toBe("");
    expect(innerInput(outside)?.hasAttribute("aria-label")).toBe(false);
    expect(innerInput(outside)?.hasAttribute("aria-invalid")).toBe(false);

    const defined = createInput("Defined input");
    defined.setAttribute("default-value", "Alice");
    root.append(defined);
    await settle();
    const write = rstest.spyOn(
      defined as HTMLElement & {
        setValue: (params: { value: string }) => void;
      },
      "setValue",
    );
    const setValue = (
      defined as HTMLElement & { setValue: (params: { value: string }) => void }
    ).setValue.bind(defined);
    setValue({ value: "Alice" });
    expect(innerInput(defined)?.value).toBe("Alice");
    setValue({ value: "Bob" });
    expect(innerInput(defined)?.value).toBe("Bob");
    setValue({ value: "" });
    expect(innerInput(defined)?.value).toBe("");
    expect(write).toHaveBeenCalledTimes(3);
    expect(innerInput(defined)?.getAttribute("aria-label")).toBe(
      "Defined input",
    );
    expect(innerInput(defined)?.getAttribute("aria-invalid")).toBe("true");

    defined.setAttribute("accessibility-label", "Changed input");
    defined.classList.remove("lui-invalid");
    await settle();
    expect(write).toHaveBeenCalledTimes(3);
    expect(innerInput(defined)?.getAttribute("aria-label")).toBe(
      "Changed input",
    );
    expect(innerInput(defined)?.getAttribute("aria-invalid")).toBe("false");

    defined.setAttribute("accessibility-label", "");
    await settle();
    expect(innerInput(defined)?.hasAttribute("aria-label")).toBe(false);
  } finally {
    errors.mockRestore();
    whenDefined.mockRestore();
    dispose();
    disposeEarly();
    view.remove();
    otherView.remove();
    disposedView.remove();
  }
});
