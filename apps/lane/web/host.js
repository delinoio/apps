// SPDX-License-Identifier: Apache-2.0
import "./runtime/static/js/client.js";
await customElements.whenDefined("lynx-view");
const view = document.createElement("lynx-view");
view.addEventListener("error", () => {
  console.error({ event: "lane_template_failed", platform: "web" });
});
document.body.append(view);
view.setAttribute("url", "./main.web.bundle");
console.info({ event: "lane_started", platform: "web" });
