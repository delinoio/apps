// SPDX-License-Identifier: Apache-2.0
import { expect, it, rstest } from "@rstest/core";
import { installWebSupport } from "../src/web";

it("maps web semantics, suppresses repeated activation and cleans up methods", () => {
  const host = document.createElement("div");
  host.innerHTML =
    '<div class="lui-button" accessibility-label="Save"></div><div class="lui-toggle"><div class="lui-switch lui-checked"></div></div>';
  document.body.append(host);
  const dispose = installWebSupport(host);
  const button = host.firstElementChild as HTMLElement;
  const click = rstest.fn();
  button.addEventListener("click", click);
  expect(button.getAttribute("role")).toBe("button");
  expect(button.tabIndex).toBe(0);
  expect(host.querySelector('[aria-checked="true"]')).not.toBeNull();
  button.dispatchEvent(
    new KeyboardEvent("keydown", {
      key: "Enter",
      bubbles: true,
      composed: true,
    }),
  );
  button.dispatchEvent(
    new KeyboardEvent("keydown", {
      key: "Enter",
      repeat: true,
      bubbles: true,
      composed: true,
    }),
  );
  expect(click).toHaveBeenCalledTimes(1);
  expect(Reflect.has(button, "setFocus")).toBe(true);
  dispose();
  expect(Reflect.has(button, "setFocus")).toBe(false);
  button.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
  );
  expect(click).toHaveBeenCalledTimes(1);
  host.remove();
});

it("contains modal focus, hides background content, and handles Escape", async () => {
  const host = document.createElement("div");
  host.innerHTML =
    '<div><main><p>Background</p><div class="lui-button">Open</div></main><section class="lui-dialog"><div class="lui-button">First</div><div class="lui-button lui-disabled">Disabled</div><div class="lui-button lui-dialog-close">Close</div></section></div>';
  document.body.append(host);
  const dispose = installWebSupport(host);
  const modal = host.querySelector<HTMLElement>(".lui-dialog")!;
  const controls = modal.querySelectorAll<HTMLElement>(".lui-button");
  const close = rstest.fn();
  controls[2].addEventListener("click", close);
  expect(host.querySelector("main")!.inert).toBe(true);
  controls[0].focus();
  controls[0].dispatchEvent(
    new KeyboardEvent("keydown", {
      key: "Tab",
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    }),
  );
  expect(document.activeElement).toBe(controls[2]);
  controls[2].dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );
  expect(close).toHaveBeenCalledTimes(1);
  modal.remove();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(host.querySelector("main")!.inert).not.toBe(true);
  dispose();
  host.remove();
});

it("moves web tab selection across disabled items", () => {
  const host = document.createElement("div");
  host.innerHTML =
    '<div class="lui-tabs"><div class="lui-tab lui-tab-selected">First</div><div class="lui-tab lui-disabled">Disabled</div><div class="lui-tab">Last</div></div>';
  document.body.append(host);
  const dispose = installWebSupport(host);
  const tabs = host.querySelectorAll<HTMLElement>(".lui-tab");
  const select = rstest.fn();
  tabs[2].addEventListener("click", select);
  tabs[0].dispatchEvent(
    new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
  );
  expect(select).toHaveBeenCalledTimes(1);
  expect(document.activeElement).toBe(tabs[2]);
  expect(tabs[1].getAttribute("aria-disabled")).toBe("true");
  dispose();
  host.remove();
});
