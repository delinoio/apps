// SPDX-License-Identifier: Apache-2.0
// web-core preserves Lynx accessibility attributes but does not map them to
// browser semantics. Remove this adapter when that mapping exists upstream.
export function installAccessibility(view) {
  let previousDialog;
  let previousApp;
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
    // x-text delegates rendering to an inner box; its host cannot take focus.
    const target =
      node?.shadowRoot?.querySelector("input,textarea,#inner-box") || node;
    if (target?.id === "inner-box") target.tabIndex = -1;
    target?.focus();
  };
  const update = () => {
    const root = view.shadowRoot;
    if (!root) return;
    for (const icon of root.querySelectorAll(
      '[accessibility-elements-hidden="true"]',
    )) {
      icon.setAttribute("aria-hidden", "true");
    }
    for (const heading of root.querySelectorAll(
      '[accessibility-heading="true"]',
    )) {
      heading.setAttribute("role", "heading");
      heading.setAttribute("aria-level", "2");
    }
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
      if (button.classList.contains("app-switch")) {
        button.setAttribute(
          "aria-pressed",
          String(button.classList.contains("selected")),
        );
      }
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
        // Give each editor a full-size form box; the runtime defaults to display:contents.
        const form = field.shadowRoot.querySelector("form");
        if (form) {
          form.style.display = "block";
          form.style.width = "100%";
          form.style.height = "100%";
        }
        target.style.width = "100%";
        target.style.height = "100%";
        target.setAttribute("role", "textbox");
        target.setAttribute("tabindex", "0");
        target.setAttribute(
          "aria-multiline",
          String(field.tagName === "X-TEXTAREA"),
        );
        target.setAttribute(
          "aria-label",
          field.getAttribute("accessibility-label") || "",
        );
        target.disabled =
          field.hasAttribute("disabled") &&
          field.getAttribute("disabled") !== "false";
      }
    }
    for (const notice of root.querySelectorAll(".notice")) {
      notice.setAttribute("role", "status");
      notice.setAttribute("aria-live", "polite");
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
    const app = root.querySelector("#lane-root")?.getAttribute("data-app");
    if (app && app !== previousApp) {
      if (previousApp) {
        // Select after the runtime finishes replacing the previous app's nodes.
        requestAnimationFrame(() => {
          if (app !== previousApp || previousDialog) return;
          const heading =
            root.querySelector(
              ".workspace .coming-soon-title, .workspace .heading",
            ) || root.querySelector("#lane-workspace");
          if (heading) {
            heading.setAttribute("tabindex", "-1");
            focus(heading);
          }
        });
      }
      previousApp = app;
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
      ".button:focus-visible{outline:3px solid #075aff;outline-offset:2px}x-input:focus-within,x-textarea:focus-within{outline:3px solid #075aff;outline-offset:2px}";
    root.append(style);
    new MutationObserver(update).observe(root, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: [
        "class",
        "accessibility-label",
        "accessibility-value",
        "data-app",
        "disabled",
      ],
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
