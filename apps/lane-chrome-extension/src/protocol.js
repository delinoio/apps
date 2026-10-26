// SPDX-License-Identifier: Apache-2.0
export const HOST = "io.delino.lane.realqa";
export function safeMetadata(value) {
  const url = new URL(value.pageUrl);
  if (
    !["https:", "http:"].includes(url.protocol) ||
    url.username ||
    url.password
  )
    throw new Error("UNSUPPORTED_PAGE");
  return {
    pageUrl: url.href,
    pageTitle: String(value.pageTitle || ""),
    cssSelector: String(value.cssSelector || ""),
  };
}
export function visibleRect(rect, viewport) {
  const left = Math.max(0, rect.x),
    top = Math.max(0, rect.y);
  const right = Math.min(viewport.width, rect.x + rect.width),
    bottom = Math.min(viewport.height, rect.y + rect.height);
  if (
    ![left, top, right, bottom].every(Number.isFinite) ||
    right <= left ||
    bottom <= top
  )
    throw new Error("ELEMENT_NOT_VISIBLE");
  return {
    x: left,
    y: top,
    width: right - left,
    height: bottom - top,
    clipped:
      left !== rect.x ||
      top !== rect.y ||
      right !== rect.x + rect.width ||
      bottom !== rect.y + rect.height,
  };
}
export const remedies = {
  LANE_NOT_RUNNING:
    "Start Lane, then try again. RealQA does not start Lane automatically.",
  APPROVAL_REQUIRED: "Approve this Chrome profile in Lane, then try again.",
  REVOKED:
    "This profile was disconnected. Connect again and approve it in Lane.",
  UNSUPPORTED_PAGE:
    "Open a normal HTTP or HTTPS page. Chrome internal pages, the Web Store, files and incognito are not supported.",
  PAGE_CHANGED:
    "The page moved or changed during capture. Select the element again.",
  ACCESS_UNAVAILABLE:
    "Chrome could not secure RealQA's local storage. Reload the extension, then try again.",
  PERMISSION_REQUIRED:
    "Click the RealQA extension on this page, then select an element.",
};
