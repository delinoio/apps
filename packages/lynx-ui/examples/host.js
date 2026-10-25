// SPDX-License-Identifier: Apache-2.0
import "./runtime/static/js/client.js";
import { installWebSupport } from "./web.js";

await customElements.whenDefined("lynx-view");
const view = document.createElement("lynx-view");
document.body.append(view);
const dispose = installWebSupport(
  view.shadowRoot ?? view.attachShadow({ mode: "open" }),
);
window.addEventListener("pagehide", dispose, { once: true });
view.setAttribute("url", "./gallery.web.bundle");
