// SPDX-License-Identifier: Apache-2.0
// web-core preserves Lynx accessibility attributes but does not map them to
// browser semantics. Remove this adapter when that mapping exists upstream.
export function installAccessibility(view) {
  let previousDialog;
  let restoreFocus;
  const wired = new WeakSet();
  const focusables = (root) =>
    [
      ...root.querySelectorAll('x-view[role="button"], x-input, x-textarea'),
    ].filter(
      (e) =>
        e.getAttribute("aria-disabled") !== "true" &&
        !e.hasAttribute("disabled"),
    );
  const focus = (node) => {
    const target = node?.shadowRoot?.querySelector("input,textarea") || node;
    target?.focus();
  };
  const update = () => {
    const root = view.shadowRoot;
    if (!root) return;
    for (const element of root.querySelectorAll("[accessibility-label]")) {
      const target =
        element.shadowRoot?.querySelector("input,textarea") || element;
      target.setAttribute(
        "aria-label",
        element.getAttribute("accessibility-label"),
      );
    }
    for (const button of root.querySelectorAll("x-view.button")) {
      button.setAttribute("role", "button");
      const disabled = button.classList.contains("disabled");
      button.setAttribute("aria-disabled", String(disabled));
      button.tabIndex = disabled ? -1 : 0;
      if (!wired.has(button)) {
        wired.add(button);
        button.addEventListener("keydown", (e) => {
          if (
            (e.key === "Enter" || e.key === " ") &&
            !button.classList.contains("disabled")
          ) {
            e.preventDefault();
            e.stopPropagation();
            button.click();
          }
        });
      }
    }
    for (const field of root.querySelectorAll("x-input,x-textarea")) {
      const target = field.shadowRoot?.querySelector("input,textarea");
      if (target) {
        target.setAttribute(
          "aria-label",
          field.getAttribute("accessibility-label") || "",
        );
        target.disabled =
          field.hasAttribute("disabled") &&
          field.getAttribute("disabled") !== "false";
      }
    }
    const dialogs = root.querySelectorAll(".dialog");
    const dialog = dialogs[dialogs.length - 1];
    if (dialog !== previousDialog) {
      if (dialog) {
        restoreFocus = root.activeElement;
        dialog.setAttribute("role", "dialog");
        dialog.setAttribute("aria-modal", "true");
        queueMicrotask(() => focus(focusables(dialog)[0]));
      } else if (previousDialog) focus(restoreFocus);
      previousDialog = dialog;
    }
  };
  const attach = () => {
    if (!view.shadowRoot) {
      requestAnimationFrame(attach);
      return;
    }
    const root = view.shadowRoot;
    const style = document.createElement("style");
    style.textContent =
      ".button:focus-visible{outline:3px solid #075aff;outline-offset:2px}.notice{role:status}";
    root.append(style);
    new MutationObserver(update).observe(root, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["class", "accessibility-label", "disabled"],
    });
    root.addEventListener("keydown", (event) => {
      if (!previousDialog) return;
      if (event.key === "Escape") {
        const close = [...previousDialog.querySelectorAll(".button")].find(
          (e) => e.getAttribute("accessibility-label") === "Close",
        );
        close?.click();
        event.stopPropagation();
      }
      if (event.key === "Tab") {
        const list = focusables(previousDialog);
        if (!list.length) return;
        const active = root.activeElement;
        const index = list.indexOf(active);
        if (event.shiftKey && index <= 0) {
          event.preventDefault();
          focus(list.at(-1));
        } else if (
          !event.shiftKey &&
          (index === list.length - 1 || index < 0)
        ) {
          event.preventDefault();
          focus(list[0]);
        }
      }
    });
    update();
  };
  attach();
}
