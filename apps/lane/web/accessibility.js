// SPDX-License-Identifier: Apache-2.0
// web-core preserves Lynx accessibility attributes but does not map them to
// browser semantics. Remove this adapter when that mapping exists upstream.
export function installAccessibility(view) {
  let previousDialog;
  let previousApp;
  let previousTab;
  let restoreFocus;
  // Dialog fields can take focus before the mutation observer runs, so remember
  // the last control outside any dialog as the focus-return target.
  let lastOutside;
  let previousSettingsTarget;
  let revealedSettingsTarget;
  let previousCommand;
  const pendingAnnouncements = new WeakSet();
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
    if (target?.id === "inner-box" || target?.id === "lane-workspace")
      target.tabIndex = -1;
    target?.focus();
  };
  const update = () => {
    const root = view.shadowRoot;
    if (!root) return;
    for (const panel of root.querySelectorAll(".tab-panel")) {
      const hidden = panel.getAttribute("data-active-tab") !== "true";
      panel.inert = hidden;
      panel.setAttribute("aria-hidden", String(hidden));
    }
    for (const icon of root.querySelectorAll(
      '[accessibility-elements-hidden="true"]',
    )) {
      icon.setAttribute("aria-hidden", "true");
    }
    // web-core does not map Lynx touch hit-testing flags to browser CSS.
    for (const tooltip of root.querySelectorAll(".tab-tooltip")) {
      tooltip.style.pointerEvents = "none";
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
      if (button.classList.contains("command-result")) {
        button.setAttribute(
          "aria-pressed",
          String(button.classList.contains("command-selected")),
        );
      }
      // Toggle chips and collapsible regions expose their state to screen readers.
      // The web runtime removes aria-*="false", so only the active state is observable.
      for (const [source, aria] of [
        ["data-pressed", "aria-pressed"],
        ["data-expanded", "aria-expanded"],
      ]) {
        const state = button.getAttribute(source);
        if (state !== null) button.setAttribute(aria, state);
      }
      const tab = button.hasAttribute("data-workspace-tab");
      const hidden = !!button.closest('[data-active-tab="false"]');
      button.tabIndex =
        disabled ||
        hidden ||
        (tab && button.getAttribute("data-tab-selected") !== "true")
          ? -1
          : 0;
      if (tab) {
        const selected = button.getAttribute("data-tab-selected") === "true";
        button.setAttribute("role", "tab");
        button.setAttribute("aria-selected", String(selected));
        button.setAttribute(
          "aria-controls",
          `panel-${button.getAttribute("data-workspace-tab")}`,
        );
        button.title = button.getAttribute("accessibility-label") || "";
      }
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
    const tabList = root.querySelector(".tab-scroll");
    tabList?.setAttribute("role", "tablist");
    tabList?.setAttribute("aria-orientation", "vertical");
    if (tabList && !wired.has(tabList)) {
      wired.add(tabList);
      // The pinned web runtime resets the scroll offset on resize instead
      // of reapplying native scroll-to-index. Keep the selected tab visible
      // until web-core implements that native behavior on viewport changes.
      new ResizeObserver(() =>
        requestAnimationFrame(() => {
          root
            .querySelector('[data-tab-selected="true"]')
            ?.scrollIntoView({ block: "nearest", inline: "nearest" });
        }),
      ).observe(tabList);
    }
    for (const panel of root.querySelectorAll(".tab-panel")) {
      const id =
        panel.getAttribute("data-tab-id") ??
        panel.querySelector("[data-tab-id]")?.getAttribute("data-tab-id");
      if (id) {
        panel.id = `panel-${id}`;
        panel.setAttribute("role", "tabpanel");
        panel.setAttribute("aria-labelledby", `tab-${id}`);
      }
    }
    const selectedTab = `${root.querySelector("#lane-root")?.getAttribute("data-selected-tab")}:${root.querySelector("#lane-root")?.getAttribute("data-tab-generation")}`;
    if (selectedTab !== previousTab) {
      if (previousTab)
        requestAnimationFrame(() => {
          const selected = root.querySelector('[data-tab-selected="true"]');
          selected?.scrollIntoView({ block: "nearest", inline: "nearest" });
          if (!previousDialog) focus(selected);
        });
      previousTab = selectedTab;
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
        target.style.boxSizing = "border-box";
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
        target.setAttribute(
          "aria-invalid",
          String(
            field.closest(".field")?.getAttribute("data-invalid") === "true",
          ),
        );
      }
    }
    for (const message of root.querySelectorAll(".validation")) {
      message.setAttribute("role", "alert");
    }
    for (const notice of root.querySelectorAll(".notice")) {
      notice.setAttribute("role", "status");
      notice.setAttribute("aria-live", "polite");
    }
    const dialogs = [...root.querySelectorAll(".dialog")].filter(
      (dialog) => !dialog.closest('[data-active-tab="false"]'),
    );
    const dialog = dialogs[dialogs.length - 1];
    if (dialog !== previousDialog) {
      if (dialog) {
        if (!previousDialog)
          restoreFocus = lastOutside?.isConnected
            ? lastOutside
            : root.activeElement;
        dialog.setAttribute("role", "dialog");
        dialog.setAttribute("aria-modal", "true");
        queueMicrotask(() =>
          focus(
            dialog.querySelector('[data-dialog-default="true"]') ||
              focusables(dialog)[0],
          ),
        );
      } else if (previousDialog) {
        if (restoreFocus?.isConnected) focus(restoreFocus);
        else focus(root.querySelector("#lane-workspace"));
      }
      previousDialog = dialog;
    }
    const command = root.querySelector(".command-selected");
    if (command !== previousCommand) {
      previousCommand = command;
      command?.scrollIntoView({ block: "nearest" });
    }
    const palette = root.querySelector(".command-palette");
    if (palette) {
      let status = palette.querySelector(".command-announcement");
      if (!status) {
        // The real input lives in x-input's own shadow root, so an ARIA ID
        // reference to a result in the outer root would not cross that boundary.
        // Keep this live region until web-core exposes cross-root relationships.
        status = document.createElement("span");
        status.className = "command-announcement";
        status.setAttribute("role", "status");
        status.setAttribute("aria-live", "polite");
        status.setAttribute("aria-atomic", "true");
        status.style.cssText =
          "position:absolute;width:1px;height:1px;padding:0;overflow:hidden;clip-path:inset(50%);white-space:nowrap";
        palette.append(status);
      }
      if (!pendingAnnouncements.has(status)) {
        pendingAnnouncements.add(status);
        // Mount the empty live region before writing its first announcement.
        requestAnimationFrame(() => {
          pendingAnnouncements.delete(status);
          if (!status.isConnected) return;
          const selected = palette.querySelector(".command-selected");
          const announcement = selected
            ? `Selected: ${selected.getAttribute("accessibility-label")}`
            : "No matching destinations";
          if (status.textContent !== announcement)
            status.textContent = announcement;
        });
      }
    }
    const app = root.querySelector("#lane-root")?.getAttribute("data-app");
    if (app && app !== previousApp) {
      if (previousApp) {
        // Select after the runtime finishes replacing the previous app's nodes.
        requestAnimationFrame(() => {
          if (app !== previousApp || previousDialog) return;
          if (
            root
              .querySelector("[data-settings-target]")
              ?.getAttribute("data-settings-target")
          )
            return;
          const heading =
            root.querySelector(
              '[data-active-tab="true"] .workspace .heading, .coming-soon-title',
            ) || root.querySelector("#lane-workspace");
          if (heading) {
            heading.setAttribute("tabindex", "-1");
            focus(heading);
          }
        });
      }
      previousApp = app;
    }
    const settingsTarget = root
      .querySelector('[data-active-tab="true"] [data-settings-target]')
      ?.getAttribute("data-settings-target");
    if (settingsTarget !== previousSettingsTarget) {
      previousSettingsTarget = settingsTarget;
      if (settingsTarget && settingsTarget !== revealedSettingsTarget) {
        revealedSettingsTarget = settingsTarget;
        requestAnimationFrame(() => {
          if (previousDialog || settingsTarget !== previousSettingsTarget)
            return;
          const heading = root.querySelector(
            `#settings-${settingsTarget.split(":")[0]}`,
          );
          if (heading) {
            heading.setAttribute("tabindex", "-1");
            heading.scrollIntoView({ block: "start" });
            focus(heading);
          }
        });
      }
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
        "data-active-tab",
        "data-selected-tab",
        "data-tab-generation",
        "data-tab-selected",
        "data-settings-target",
        "data-pressed",
        "data-expanded",
        "data-invalid",
        "disabled",
      ],
    });
    document.addEventListener(
      "keydown",
      (event) => {
        if (
          view.hidden ||
          !root.querySelector("#lane-root") ||
          event.isComposing
        )
          return;
        if (
          (event.metaKey || event.ctrlKey) &&
          !event.altKey &&
          !event.shiftKey &&
          event.key.toLowerCase() === "k"
        ) {
          if (
            previousDialog &&
            !previousDialog.classList.contains("command-palette")
          )
            return;
          event.preventDefault();
          event.stopImmediatePropagation();
          if (!event.repeat) view.sendGlobalEvent("lane.commandPalette", [{}]);
          return;
        }
        if (
          previousDialog?.classList.contains("command-palette") &&
          ["ArrowDown", "ArrowUp", "Enter", "Escape"].includes(event.key)
        ) {
          // Keep native input/confirm and the runtime's global key listener from
          // handling the same key a second time while pinned to web-core 0.26.5.
          if (
            event.key === "Enter" &&
            root.activeElement?.classList.contains("button")
          )
            return;
          event.preventDefault();
          event.stopImmediatePropagation();
          view.sendGlobalEvent("lane.commandKey", [{ key: event.key }]);
        }
      },
      true,
    );
    root.addEventListener("focusin", (event) => {
      const target = event.composedPath()[0];
      if (target instanceof Element && !target.closest(".dialog"))
        lastOutside = target.closest("x-view.button") || target;
    });
    // Activation (pointer or keyboard) names the trigger precisely, even when the
    // button is not the element that received focus.
    root.addEventListener(
      "click",
      (event) => {
        const trigger = event
          .composedPath()
          .find(
            (node) => node instanceof Element && node.matches("x-view.button"),
          );
        if (trigger && !trigger.closest(".dialog")) lastOutside = trigger;
      },
      true,
    );
    root.addEventListener("keydown", (event) => {
      const tab = event
        .composedPath()
        .find(
          (node) =>
            node instanceof Element && node.hasAttribute("data-workspace-tab"),
        );
      if (tab && ["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
        const tabs = [...root.querySelectorAll("[data-workspace-tab]")];
        const index = tabs.indexOf(tab);
        const next =
          event.key === "Home"
            ? 0
            : event.key === "End"
              ? tabs.length - 1
              : (index + (event.key === "ArrowUp" ? -1 : 1) + tabs.length) %
                tabs.length;
        event.preventDefault();
        event.stopPropagation();
        focus(tabs[next]);
        tabs[next]?.scrollIntoView({ block: "nearest", inline: "nearest" });
        return;
      }
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
