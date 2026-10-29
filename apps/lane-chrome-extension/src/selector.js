// SPDX-License-Identifier: Apache-2.0
(() => {
  if (globalThis.__laneRealQASelector) {
    globalThis.__laneRealQASelector();
    return;
  }
  const box = document.createElement("div");
  box.style.cssText =
    "position:fixed;pointer-events:none;z-index:2147483647;border:2px solid #3278f6;background:#3278f622;box-sizing:border-box";
  const tip = document.createElement("div");
  tip.textContent =
    "RealQA: click an element · Tab selects, Enter captures, Esc cancels · Only the visible portion is captured. Scroll and capture again for more.";
  tip.style.cssText =
    "position:fixed;bottom:16px;left:16px;right:16px;z-index:2147483647;pointer-events:none;padding:12px;background:#172033;color:white;font:14px system-ui;border-radius:8px";
  tip.setAttribute("role", "status");
  tip.setAttribute("aria-live", "polite");
  document.documentElement.append(box, tip);
  let selected = null;
  function cleanup() {
    box.remove();
    tip.remove();
    document.removeEventListener("pointermove", move, true);
    document.removeEventListener("click", click, true);
    document.removeEventListener("keydown", key, true);
    delete globalThis.__laneRealQASelector;
  }
  globalThis.__laneRealQASelector = cleanup;
  // Page JavaScript shares this DOM and can dispatch synthetic events into the
  // capture-phase listeners below. Only genuine user input (isTrusted === true)
  // may select or capture; anything else is ignored before any side effect so
  // the session stays intact for the user's next real action.
  function trusted(e) {
    return e.isTrusted === true;
  }
  function move(e) {
    if (!trusted(e)) return;
    selected = e.target;
    highlight();
  }
  function highlight() {
    const r = selected.getBoundingClientRect();
    Object.assign(box.style, {
      left: `${r.x}px`,
      top: `${r.y}px`,
      width: `${r.width}px`,
      height: `${r.height}px`,
    });
  }
  function key(e) {
    if (!trusted(e)) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      cleanup();
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      e.stopImmediatePropagation();
      const candidates = [
        document.body,
        ...document.body.querySelectorAll("*"),
      ].filter((el) => {
        const r = el.getBoundingClientRect();
        return (
          r.width > 0 &&
          r.height > 0 &&
          r.right > 0 &&
          r.bottom > 0 &&
          r.left < innerWidth &&
          r.top < innerHeight
        );
      });
      const index = candidates.indexOf(selected),
        step = e.shiftKey ? -1 : 1;
      selected =
        candidates[(index + step + candidates.length) % candidates.length];
      if (selected) {
        highlight();
        tip.textContent = `RealQA: ${selected.localName}${selected.id ? " #" + selected.id : ""} selected. Tab selects, Enter captures, Esc cancels. Only visible pixels are captured.`;
      }
    } else if (e.key === "Enter" && selected) {
      e.preventDefault();
      e.stopImmediatePropagation();
      void capture(selected);
    }
  }
  function selector(element) {
    const parts = [];
    for (let el = element; el && el.nodeType === 1; el = el.parentElement) {
      if (el.id) {
        parts.unshift(`#${CSS.escape(el.id)}`);
        break;
      }
      let part = el.localName;
      const siblings = el.parentElement
        ? [...el.parentElement.children].filter(
            (s) => s.localName === el.localName,
          )
        : [];
      if (siblings.length > 1)
        part += `:nth-of-type(${siblings.indexOf(el) + 1})`;
      parts.unshift(part);
    }
    return parts.join(" > ");
  }
  async function click(e) {
    if (!trusted(e)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    await capture(e.target);
  }
  async function capture(el) {
    const bounds = el.getBoundingClientRect();
    let left = bounds.left,
      top = bounds.top,
      right = bounds.right,
      bottom = bounds.bottom;
    for (let parent = el.parentElement; parent; parent = parent.parentElement) {
      const style = getComputedStyle(parent),
        r = parent.getBoundingClientRect();
      const sx = parent.offsetWidth ? r.width / parent.offsetWidth : 1,
        sy = parent.offsetHeight ? r.height / parent.offsetHeight : 1;
      if (/hidden|clip|scroll|auto/.test(style.overflowX)) {
        left = Math.max(left, r.left + parent.clientLeft * sx);
        right = Math.min(
          right,
          r.left + (parent.clientLeft + parent.clientWidth) * sx,
        );
      }
      if (/hidden|clip|scroll|auto/.test(style.overflowY)) {
        top = Math.max(top, r.top + parent.clientTop * sy);
        bottom = Math.min(
          bottom,
          r.top + (parent.clientTop + parent.clientHeight) * sy,
        );
      }
    }
    const r = {
      x: left,
      y: top,
      width: Math.max(0, right - left),
      height: Math.max(0, bottom - top),
    };
    const message = {
      type: "selection",
      rect: { x: r.x, y: r.y, width: r.width, height: r.height },
      viewport: { width: innerWidth, height: innerHeight, scrollX, scrollY },
      clipped: r.width !== bounds.width || r.height !== bounds.height,
      metadata: {
        pageUrl: location.href,
        pageTitle: document.title,
        cssSelector: selector(el),
      },
    };
    cleanup();
    await new Promise((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(resolve)),
    );
    chrome.runtime.sendMessage(message).catch(() => {});
  }
  document.addEventListener("pointermove", move, true);
  document.addEventListener("click", click, true);
  document.addEventListener("keydown", key, true);
})();
