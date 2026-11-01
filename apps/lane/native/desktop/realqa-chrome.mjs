// SPDX-License-Identifier: Apache-2.0
import { createChromeBridge, resetChromeApprovals } from "./chrome-bridge.mjs";
const validId = (value) =>
  typeof value === "string" && /^[a-p]{32}$/.test(value);
// Stable recovery actions the settings view can offer for a Chrome error state.
export const ChromeRecovery = Object.freeze({
  Retry: "retry",
  ResetApprovals: "resetApprovals",
});
// Map bridge startup failures to actionable English guidance. Messages never
// include paths, endpoints, secrets or raw system errors.
function startFailure(error) {
  if (error?.message === "CHROME_APPROVALS_INVALID")
    return {
      code: "CHROME_APPROVALS_INVALID",
      recovery: ChromeRecovery.ResetApprovals,
      message:
        "Lane could not read its saved Chrome profile approvals. Reset Chrome approvals, then connect and approve each Chrome profile again.",
    };
  if (
    error?.message === "CHROME_BRIDGE_ALREADY_RUNNING" ||
    error?.code === "EADDRINUSE"
  )
    return {
      code: "CHROME_BRIDGE_IN_USE",
      recovery: ChromeRecovery.Retry,
      message:
        "Another copy of Lane is using the Chrome connection. Quit the other copy, then retry.",
    };
  return {
    code: "CHROME_BRIDGE_UNAVAILABLE",
    recovery: ChromeRecovery.Retry,
    message:
      "Lane could not start the Chrome connection. Retry. If this continues, restart Lane.",
  };
}
const captureFailures = {
  PAGE_CHANGED:
    "The Chrome page moved or changed during capture. Your draft is unchanged. Select the element again.",
  ELEMENT_NOT_VISIBLE:
    "The selected element is not visible in Chrome. Scroll it into view, then select it again.",
  UNSUPPORTED_PAGE:
    "Chrome does not allow capture on this page. Open an ordinary HTTP or HTTPS page and retry.",
  PERMISSION_REQUIRED:
    "Click the RealQA extension on this Chrome tab to grant tab access, then retry.",
  CHROME_NOT_AVAILABLE:
    "Keep the Chrome window focused until the capture finishes, then select the element again.",
  LANE_SAVING:
    "The Chrome capture was not added because Lane was saving the current task. Wait for the save to finish, then capture again.",
  INVALID_CAPTURE:
    "Lane could not validate the Chrome capture. Your draft is unchanged. Select the element again.",
  INVALID_METADATA:
    "Lane could not validate the Chrome capture. Your draft is unchanged. Select the element again.",
  REVOKED:
    "This Chrome profile is no longer approved. Connect it again from the RealQA extension and approve it in Lane.",
  CAPTURE_FAILED:
    "The Chrome capture could not be added. Your draft is unchanged. Select the element again.",
};
const dependencyError = (message) => {
  const error = new Error(message);
  error.detail = {
    reason: "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
    message,
    retryable: true,
  };
  return error;
};
export async function connectChrome({
  realqa,
  dataRoot,
  extensionIds,
  listingExtensionId,
  showWindow,
  log = () => {},
  createBridge = createChromeBridge,
  resetApprovals = resetChromeApprovals,
}) {
  const pending = new Map();
  let bridge;
  let failure;
  let starting;
  let captureEpoch = 0;
  // Only a configured, allowed published ID produces a listing. Never fabricate one.
  const listingUrl =
    validId(listingExtensionId) &&
    Array.isArray(extensionIds) &&
    extensionIds.includes(listingExtensionId)
      ? `https://chromewebstore.google.com/detail/${listingExtensionId}`
      : undefined;
  const state = () => {
    const listing = listingUrl ? { listingUrl } : {};
    if (!bridge)
      return {
        state: "error",
        message: failure?.message ?? startFailure().message,
        recovery: failure?.recovery ?? ChromeRecovery.Retry,
        pending: [],
        connections: [],
        ...listing,
      };
    const connections = bridge.listConnections?.() ?? [];
    return {
      state: pending.size
        ? "approval_pending"
        : connections.some((item) => item.connected)
          ? "connected"
          : "disconnected",
      pending: [...pending.values()].map(({ resolve, ...item }) => item),
      connections,
      ...listing,
    };
  };
  const options = {
    dataRoot,
    extensionIds,
    requestApproval(identity, signal) {
      return new Promise((resolve) => {
        const id = `${identity.extensionId}:${identity.profileId}`;
        const entry = { ...identity, id, resolve };
        pending.set(id, entry);
        // The extension closed its connection while waiting. Remove the prompt so
        // a late Approve cannot grant a token that no extension receives.
        signal?.addEventListener(
          "abort",
          () => {
            if (pending.get(id) !== entry) return;
            pending.delete(id);
            resolve(false);
            log("lane_chrome_approval_abandoned");
            realqa.changed();
          },
          { once: true },
        );
        showWindow();
        realqa.changed();
      });
    },
    acceptCapture(capture) {
      const epoch = captureEpoch;
      return realqa.enqueue(async () => {
        if (realqa.saving) throw new Error("LANE_SAVING");
        // Decode and encode through the OS renderer before accepting browser bytes.
        // The original browser buffer and the final PNG exist only in memory.
        const decoded = await realqa.os({
          op: "render",
          png: capture.pngBase64,
          layers: [],
        });
        if (
          decoded.width !== capture.width ||
          decoded.height !== capture.height
        )
          throw new Error("INVALID_CAPTURE");
        if (epoch !== captureEpoch) throw new Error("REVOKED");
        const image = realqa.appendCapture({
          ...decoded,
          metadata: capture.metadata,
          clipped: capture.clipped === true,
        });
        // The draft image keeps `clipped` so a composer that was created by this
        // capture can render the explanation from state; this event is advisory.
        if (capture.clipped)
          realqa.emit("lane.realqa.notice", {
            imageId: image?.id,
            message:
              "Only the visible part of this element was captured. Scroll and capture again to include another part.",
          });
      });
    },
    onStatus: () => realqa.changed(),
    onCaptureFailed(reason) {
      const message = captureFailures[reason] ?? captureFailures.CAPTURE_FAILED;
      log("lane_chrome_capture_failed", reason);
      realqa.emit("lane.realqa.error", {
        reason:
          reason === "LANE_SAVING"
            ? "ERROR_REASON_CONFLICT"
            : "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
        message,
        retryable: true,
      });
    },
  };
  const start = () => {
    if (bridge) return Promise.resolve();
    starting ??= (async () => {
      try {
        bridge = await createBridge(options);
        failure = undefined;
        log("lane_chrome_connection_started");
      } catch (error) {
        failure = startFailure(error);
        log("lane_chrome_connection_unavailable", failure.code);
      } finally {
        starting = undefined;
      }
    })();
    return starting;
  };
  await start();
  const requireBridge = () => {
    if (!bridge) throw dependencyError(state().message);
    return bridge;
  };
  return {
    state,
    async startDOMCapture({ shortcut = false } = {}) {
      requireBridge();
      if (shortcut && !(await realqa.os({ op: "activeBrowser" })).chrome)
        throw dependencyError(
          "Activate Google Chrome with the connected RealQA extension, then use the DOM shortcut or select an element from the extension.",
        );
      try {
        await bridge.startSelection({ activate: !shortcut });
        return { selecting: true };
      } catch (failure) {
        const remedies = {
          UNSUPPORTED_PAGE:
            "Chrome does not allow capture on this page. Open an ordinary HTTP or HTTPS page and retry.",
          PERMISSION_REQUIRED:
            "Click the RealQA extension on this Chrome tab to grant tab access, then retry.",
          CHROME_NOT_RESPONDING:
            "The RealQA extension did not respond. Open the extension in Chrome and retry.",
          CHROME_NOT_AVAILABLE:
            "Activate a normal Chrome window. Incognito and other browsers cannot capture DOM elements.",
        };
        throw dependencyError(
          remedies[failure.message] ||
            "Open the RealQA extension in Chrome and connect this profile to Lane, then retry.",
        );
      }
    },
    async invoke(request) {
      if (request.op === "chrome.retry") {
        await start();
        realqa.changed();
        requireBridge();
        return {};
      }
      if (request.op === "chrome.resetApprovals") {
        // Explicit user action only, and only for the unreadable-approvals state.
        if (bridge || failure?.recovery !== ChromeRecovery.ResetApprovals)
          throw dependencyError(
            "Chrome approvals do not need to be reset. Refresh RealQA settings.",
          );
        await resetApprovals(dataRoot);
        log("lane_chrome_approvals_reset");
        await start();
        realqa.changed();
        requireBridge();
        return {};
      }
      if (request.op === "chrome.approve" || request.op === "chrome.reject") {
        const approval = pending.get(request.id);
        if (!approval) throw new Error("Approval is no longer pending.");
        pending.delete(request.id);
        approval.resolve(request.op === "chrome.approve");
        realqa.changed();
        return {};
      }
      if (request.op === "chrome.disconnect") {
        requireBridge();
        // Cancel pending decoded bytes before waiting for bridge acceptance.
        // Conservatively cancel all in-flight profiles at this trust boundary.
        captureEpoch += 1;
        await bridge.revoke(request.profileId);
        realqa.changed();
        return {};
      }
      throw new Error("Unknown Chrome connection operation.");
    },
    async close() {
      captureEpoch += 1;
      for (const approval of pending.values()) approval.resolve(false);
      pending.clear();
      await starting;
      await bridge?.close();
    },
  };
}
