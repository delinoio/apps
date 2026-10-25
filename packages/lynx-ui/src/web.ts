// SPDX-License-Identifier: Apache-2.0
/**
 * Install on a LynxView's shadow root in the browser host, before loading a bundle.
 * Web elements do not yet map focusable/accessibility props or Clay's setFocus.
 * Keep this scoped bridge until the pinned web runtime implements those APIs.
 * It never runs in a Lynx background thread and never patches global prototypes.
 */
export function installWebSupport(root: ShadowRoot | HTMLElement): () => void {
  const patched = new Map<HTMLElement, PropertyDescriptor | undefined>();
  const blocked = new Map<HTMLElement, boolean>();
  const registry = root.ownerDocument.defaultView?.customElements;
  const pendingInputs = new Set<HTMLElement>();
  let waitingForInputDefinition = false;
  let disposed = false;
  const set = (element: Element, name: string, value: string) => {
    if (element.getAttribute(name) !== value) element.setAttribute(name, value);
  };
  const synchronize = () => {
    if (disposed) return;
    for (const element of pendingInputs) {
      if (!root.contains(element) || !element.classList.contains("lui-input"))
        pendingInputs.delete(element);
    }
    const modal = root.querySelector<HTMLElement>(".lui-dialog");
    const outsideNodes = new Set<HTMLElement>();
    let ancestor: Element | null = modal;
    while (ancestor?.parentElement && root.contains(ancestor.parentElement)) {
      for (const sibling of ancestor.parentElement.children) {
        if (sibling !== ancestor && sibling instanceof HTMLElement)
          outsideNodes.add(sibling);
      }
      ancestor = ancestor.parentElement;
    }
    for (const [element, original] of blocked) {
      if (!outsideNodes.has(element)) {
        element.inert = original;
        blocked.delete(element);
      }
    }
    for (const element of outsideNodes) {
      if (!blocked.has(element)) blocked.set(element, element.inert);
      element.inert = true;
    }
    for (const element of root.querySelectorAll<HTMLElement>(
      ".lui-button, .lui-toggle, .lui-tab, .lui-tab-panel, .lui-input, .lui-progress, .lui-dialog, .lui-tabs, .lui-alert",
    )) {
      const disabled = element.classList.contains("lui-disabled");
      const isInput = element.classList.contains("lui-input");
      const isControl = element.matches(".lui-button, .lui-toggle, .lui-tab");
      const outside = Boolean(
        modal && !modal.contains(element) && !element.contains(modal),
      );
      if (isControl) {
        set(
          element,
          "tabindex",
          disabled ||
            outside ||
            (element.classList.contains("lui-tab") &&
              !element.classList.contains("lui-tab-selected"))
            ? "-1"
            : "0",
        );
        if (!patched.has(element)) {
          patched.set(
            element,
            Object.getOwnPropertyDescriptor(element, "setFocus"),
          );
          Object.defineProperty(element, "setFocus", {
            configurable: true,
            value: ({ focus }: { focus: boolean }) => {
              if (focus) element.focus();
              else element.blur();
            },
          });
        }
      }
      const role = element.classList.contains("lui-button")
        ? "button"
        : element.classList.contains("lui-toggle")
          ? element.querySelector(".lui-switch")
            ? "switch"
            : "checkbox"
          : element.classList.contains("lui-tab")
            ? "tab"
            : element.classList.contains("lui-tab-panel")
              ? "tabpanel"
              : element.classList.contains("lui-tabs")
                ? "tablist"
                : element.classList.contains("lui-progress")
                  ? "progressbar"
                  : element.classList.contains("lui-dialog")
                    ? "dialog"
                    : element.classList.contains("lui-alert")
                      ? "alert"
                      : "textbox";
      // XInput owns the actual focusable HTML input inside its shadow tree.
      if (
        isInput &&
        element.localName === "x-input" &&
        !element.shadowRoot?.querySelector("input")
      ) {
        if (registry?.get("x-input")) {
          registry.upgrade(element);
        } else if (registry) {
          // Web-core can attach XInput after the outer tree is ready. Recheck
          // only live hosts here until upstream forwards these semantics on upgrade.
          pendingInputs.add(element);
          if (!waitingForInputDefinition) {
            waitingForInputDefinition = true;
            void registry.whenDefined("x-input").then(() => {
              if (disposed) return;
              let hasLiveInput = false;
              for (const pending of pendingInputs) {
                if (
                  root.contains(pending) &&
                  pending.classList.contains("lui-input")
                ) {
                  registry.upgrade(pending);
                  // The pinned XInput does not consume default-value on upgrade.
                  // Replay the current rendered value only for this live host.
                  const value = pending.getAttribute("default-value");
                  const input = pending.shadowRoot?.querySelector("input");
                  if (value !== null && input && input.value !== value) {
                    try {
                      const setValue = (
                        pending as HTMLElement & {
                          setValue?: (params: { value: string }) => void;
                        }
                      ).setValue;
                      if (typeof setValue === "function")
                        setValue.call(pending, { value });
                      else
                        console.error("lynx-ui.host-method-failed", {
                          method: "setValue",
                          code: 3,
                        });
                    } catch {
                      console.error("lynx-ui.host-method-failed", {
                        method: "setValue",
                        code: 4,
                      });
                    }
                  }
                  hasLiveInput = true;
                }
              }
              pendingInputs.clear();
              if (hasLiveInput) synchronize();
            });
          }
        }
      } else {
        pendingInputs.delete(element);
      }
      const target = isInput
        ? (element.shadowRoot?.querySelector("input") ?? element)
        : element;
      set(target, "role", role);
      const label = element.getAttribute("accessibility-label");
      if (label) set(target, "aria-label", label);
      else target.removeAttribute("aria-label");
      if (isControl || isInput) set(target, "aria-disabled", String(disabled));
      if (isInput)
        set(
          target,
          "aria-invalid",
          String(element.classList.contains("lui-invalid")),
        );
      if (role === "switch" || role === "checkbox")
        set(
          target,
          "aria-checked",
          String(Boolean(element.querySelector(".lui-checked"))),
        );
      if (role === "tab")
        set(
          target,
          "aria-selected",
          String(element.classList.contains("lui-tab-selected")),
        );
      if (role === "dialog") set(target, "aria-modal", "true");
      if (role === "progressbar") {
        set(target, "aria-valuemin", "0");
        set(target, "aria-valuemax", "100");
        set(
          target,
          "aria-valuenow",
          element.getAttribute("accessibility-value")?.replace("%", "") ?? "0",
        );
      }
    }
    for (const [element, descriptor] of patched) {
      if (!root.contains(element)) {
        if (descriptor) Object.defineProperty(element, "setFocus", descriptor);
        else Reflect.deleteProperty(element, "setFocus");
        patched.delete(element);
      }
    }
  };
  const onKey = (event: Event) => {
    const key = event as KeyboardEvent;
    const target = event
      .composedPath()
      .find(
        (node): node is HTMLElement =>
          node instanceof HTMLElement &&
          node.matches(".lui-button, .lui-toggle, .lui-tab, .lui-input"),
      );
    if (!target || !root.contains(target)) return;
    const modal = target.closest(".lui-dialog");
    const cancel = () => {
      key.preventDefault();
      key.stopImmediatePropagation();
    };
    // The pinned runtime listens for keys on document, where shadow DOM retargets
    // event.target to lynx-view. Handle library keys here using the composed path
    // until upstream preserves the original target for document-level events.
    if (key.key === "Escape" && modal) {
      cancel();
      modal.querySelector<HTMLElement>(".lui-dialog-close")?.click();
      return;
    }
    if (key.key === "Tab" && modal) {
      cancel();
      const controls = Array.from(
        modal.querySelectorAll<HTMLElement>(
          ".lui-button, .lui-toggle, .lui-tab-selected, .lui-input",
        ),
      ).filter((entry) => !entry.classList.contains("lui-disabled"));
      const index = controls.indexOf(target);
      controls[
        (index + (key.shiftKey ? -1 : 1) + controls.length) % controls.length
      ]?.focus();
      return;
    }
    if (
      target.matches(".lui-tab") &&
      ["ArrowLeft", "ArrowRight", "Home", "End"].includes(key.key)
    ) {
      cancel();
      const tabs = Array.from(
        target.parentElement!.querySelectorAll<HTMLElement>(
          ".lui-tab:not(.lui-disabled)",
        ),
      );
      const index = tabs.indexOf(target);
      const next =
        key.key === "Home"
          ? tabs[0]
          : key.key === "End"
            ? tabs.at(-1)
            : tabs[
                (index + (key.key === "ArrowLeft" ? -1 : 1) + tabs.length) %
                  tabs.length
              ];
      next?.click();
      next?.focus();
      return;
    }
    if (
      target.matches(".lui-button, .lui-toggle, .lui-tab") &&
      [" ", "Enter"].includes(key.key)
    ) {
      cancel();
      if (!key.repeat && !target.classList.contains("lui-disabled"))
        target.click();
    }
  };
  const observer = new MutationObserver(synchronize);
  observer.observe(root, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["class", "accessibility-label", "accessibility-value"],
  });
  root.addEventListener("keydown", onKey, true);
  synchronize();
  return () => {
    disposed = true;
    pendingInputs.clear();
    observer.disconnect();
    root.removeEventListener("keydown", onKey, true);
    for (const [element, descriptor] of patched) {
      if (descriptor) Object.defineProperty(element, "setFocus", descriptor);
      else Reflect.deleteProperty(element, "setFocus");
    }
    patched.clear();
    for (const [element, original] of blocked) element.inert = original;
    blocked.clear();
  };
}
