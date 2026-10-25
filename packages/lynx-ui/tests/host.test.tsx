// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from "node:fs";
import { expect, it, rstest } from "@rstest/core";
import { installWebSupport } from "../src/web";

// The runtime asset imports are unavailable in jsdom. Execute the remaining
// production host module with its real bridge and browser globals.
const hostSource = readFileSync(
  new URL("../examples/host.js", import.meta.url),
  "utf8",
).replace(/^import .*;\n/gm, "");
const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor;

it("keeps the gallery bridge through BFCache cycles and disposes on departure", async () => {
  const dispose = rstest.fn();
  const install = rstest.fn((root: ShadowRoot) => {
    const cleanup = installWebSupport(root);
    return () => {
      dispose();
      cleanup();
    };
  });
  const registerHost = () =>
    new AsyncFunction(
      "customElements",
      "document",
      "window",
      "installWebSupport",
      hostSource,
    )(
      { whenDefined: () => Promise.resolve() },
      document,
      window,
      install,
    ) as Promise<void>;
  const pagehide = (persisted: boolean) => {
    const event = new Event("pagehide");
    Object.defineProperty(event, "persisted", { value: persisted });
    window.dispatchEvent(event);
  };
  const pageshow = () => {
    const event = new Event("pageshow");
    Object.defineProperty(event, "persisted", { value: true });
    window.dispatchEvent(event);
  };
  const flushMutations = () => new Promise((resolve) => setTimeout(resolve, 0));

  try {
    await registerHost();
    expect(install).toHaveBeenCalledTimes(1);
    const view = document.querySelector("lynx-view")!;
    const root = view.shadowRoot!;
    expect(view.getAttribute("url")).toBe("./gallery.web.bundle");
    root.innerHTML =
      '<div><main>Background</main><section class="lui-dialog"><div class="lui-button" accessibility-label="Open">Open</div></section></div>';
    const background = root.querySelector<HTMLElement>("main")!;
    background.inert = false;
    await flushMutations();
    const button = root.querySelector<HTMLElement>(".lui-button")!;
    const click = rstest.fn();
    button.addEventListener("click", click);
    const focus = (
      button as HTMLElement & {
        setFocus: (options: { focus: boolean }) => void;
      }
    ).setFocus;
    expect(background.inert).toBe(true);

    for (let cycle = 0; cycle < 3; cycle++) {
      pagehide(true);
      pageshow();
      expect(dispose).not.toHaveBeenCalled();
      expect(install).toHaveBeenCalledTimes(1);
      expect(Reflect.get(button, "setFocus")).toBe(focus);
      focus({ focus: true });
      expect(root.activeElement).toBe(button);
      button.setAttribute("accessibility-label", `Open ${cycle}`);
      await flushMutations();
      expect(button.getAttribute("aria-label")).toBe(`Open ${cycle}`);
      for (const key of ["Enter", " "]) {
        button.dispatchEvent(
          new KeyboardEvent("keydown", {
            key,
            bubbles: true,
            composed: true,
            cancelable: true,
          }),
        );
      }
      expect(click).toHaveBeenCalledTimes((cycle + 1) * 2);
    }

    pagehide(false);
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(Reflect.has(button, "setFocus")).toBe(false);
    expect(background.inert).toBe(false);
    button.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
    expect(click).toHaveBeenCalledTimes(6);
    button.setAttribute("accessibility-label", "After departure");
    await flushMutations();
    expect(button.getAttribute("aria-label")).toBe("Open 2");
    pagehide(false);
    expect(dispose).toHaveBeenCalledTimes(1);

    view.remove();
    await registerHost();
    expect(install).toHaveBeenCalledTimes(2);
    expect(document.querySelectorAll("lynx-view")).toHaveLength(1);
    const freshRoot = document.querySelector("lynx-view")!.shadowRoot!;
    freshRoot.innerHTML = '<div class="lui-button">Fresh</div>';
    await flushMutations();
    const freshButton = freshRoot.querySelector<HTMLElement>(".lui-button")!;
    const freshClick = rstest.fn();
    freshButton.addEventListener("click", freshClick);
    freshButton.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
    expect(freshClick).toHaveBeenCalledTimes(1);
  } finally {
    pagehide(false);
    document.querySelectorAll("lynx-view").forEach((view) => view.remove());
  }
  expect(dispose).toHaveBeenCalledTimes(2);
});
