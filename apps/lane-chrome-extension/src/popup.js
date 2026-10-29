// SPDX-License-Identifier: Apache-2.0
const status = document.querySelector("#status"),
  profile = document.querySelector("#profile");
status.textContent =
  (await chrome.storage.session.get("status")).status || status.textContent;
profile.value =
  (await chrome.storage.local.get("profileName")).profileName || "";
for (const action of ["connect", "capture", "disconnect"])
  document.querySelector(`#${action}`).addEventListener("click", async () => {
    status.textContent = "Working…";
    try {
      const result = await chrome.runtime.sendMessage({
        type: action,
        profileName: profile.value,
      });
      status.textContent = result.message;
      if (result.ok && action === "capture") window.close();
    } catch {
      status.textContent = "Unable to connect. Start Lane and try again.";
    }
  });
