// SPDX-License-Identifier: Apache-2.0
import "./runtime/static/js/client.js";
import { installWebSupport } from "./web.js";

await customElements.whenDefined("lynx-view");
const view = document.createElement("lynx-view");
document.body.append(view);
const dispose = installWebSupport(
  view.shadowRoot ?? view.attachShadow({ mode: "open" }),
);
const onPageHide = (event) => {
  // A persisted pagehide suspends this same document for BFCache restoration.
  // Keep its bridge alive until the document is leaving permanently.
  if (event.persisted) return;
  window.removeEventListener("pagehide", onPageHide);
  dispose();
};
window.addEventListener("pagehide", onPageHide);
view.setAttribute("url", "./gallery.web.bundle");
