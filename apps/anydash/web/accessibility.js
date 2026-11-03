// SPDX-License-Identifier: Apache-2.0
// web-core keeps Lynx accessibility attributes but does not map them to ARIA.
// lynx-ui's installWebSupport covers its own controls; this adapter maps the
// app's roles, labels, hidden subtrees and live regions, and routes editor
// keys on the web. Remove each mapping when web-core provides it upstream.
const roles = new Set([
  "group",
  "heading",
  "status",
  "alert",
  "link",
  "region",
  "list",
  "listitem",
  "img",
  "banner",
  "note",
  "dialog",
]);
const editorKeys = {
  ArrowLeft: ["move-left", "narrower"],
  ArrowRight: ["move-right", "wider"],
  ArrowUp: ["move-up", "shorter"],
  ArrowDown: ["move-down", "taller"],
};

export function installAccessibility(view) {
  const root = view.shadowRoot;
  if (!root) return () => {};
  const set = (element, name, value) => {
    if (element.getAttribute(name) !== value) element.setAttribute(name, value);
  };
  const sync = () => {
    for (const element of root.querySelectorAll(
      "[accessibility-role-description]",
    )) {
      if (
        element.matches(
          ".lui-button, .lui-toggle, .lui-tab, .lui-tab-panel, .lui-input, .lui-progress, .lui-tabs, .lui-alert",
        )
      )
        continue;
      const role = element.getAttribute("accessibility-role-description");
      if (!roles.has(role)) continue;
      set(element, "role", role);
      if (role === "heading") set(element, "aria-level", "2");
      if (role === "dialog") set(element, "aria-modal", "true");
      if (role === "link") {
        set(element, "tabindex", "0");
      }
      if (role === "status") set(element, "aria-live", "polite");
      const label = element.getAttribute("accessibility-label");
      if (label) set(element, "aria-label", label);
      else element.removeAttribute("aria-label");
      // Editor cards are focusable; arrow keys move and Shift+arrows resize.
      if (element.classList.contains("ad-widget")) {
        if (element.getAttribute("accessibility-element") === "true")
          set(element, "tabindex", "0");
        else element.removeAttribute("tabindex");
      }
    }
    for (const element of root.querySelectorAll(
      '[accessibility-elements-hidden="true"]',
    ))
      set(element, "aria-hidden", "true");
    for (const element of root.querySelectorAll("[accessibility-live-region]"))
      set(
        element,
        "aria-live",
        element.getAttribute("accessibility-live-region"),
      );
    for (const element of root.querySelectorAll("x-input:not(.lui-input)")) {
      const input = element.shadowRoot?.querySelector("input");
      const label = element.getAttribute("accessibility-label");
      if (input && label) set(input, "aria-label", label);
    }
  };
  const onKey = (event) => {
    const target = event
      .composedPath()
      .find(
        (node) =>
          node instanceof HTMLElement &&
          (node.matches(".ad-widget") || node.matches('[role="link"]')),
      );
    if (!target || !root.contains(target)) return;
    if (target.matches('[role="link"]')) {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        target.click();
      }
      return;
    }
    const actions = editorKeys[event.key];
    if (!actions || target.getAttribute("accessibility-element") !== "true")
      return;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (!target.classList.contains("ad-widget-selected")) target.click();
    // The editor bar renders after the background thread commits the
    // selection; wait a bounded number of frames for the selected widget's
    // matching action, then press it and return focus to the widget.
    const id = target.id;
    const action = `#editor-${actions[event.shiftKey ? 1 : 0]}`;
    let frames = 30;
    const press = () => {
      const selected = root.getElementById(id);
      const button = root.querySelector(action);
      if (button && selected?.classList.contains("ad-widget-selected")) {
        button.click();
        requestAnimationFrame(() => {
          (root.getElementById(id) ?? target).focus();
        });
      } else if (frames-- > 0) requestAnimationFrame(press);
    };
    requestAnimationFrame(press);
  };
  const observer = new MutationObserver(sync);
  observer.observe(root, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: [
      "class",
      "accessibility-label",
      "accessibility-role-description",
      "accessibility-element",
      "accessibility-elements-hidden",
    ],
  });
  root.addEventListener("keydown", onKey, true);
  sync();
  return () => {
    observer.disconnect();
    root.removeEventListener("keydown", onKey, true);
  };
}
