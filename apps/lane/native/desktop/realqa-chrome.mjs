// SPDX-License-Identifier: Apache-2.0
import { createChromeBridge } from "./chrome-bridge.mjs";
export async function connectChrome({
  realqa,
  dataRoot,
  extensionIds,
  showWindow,
  createBridge = createChromeBridge,
}) {
  const pending = new Map();
  let bridge;
  let captureEpoch = 0;
  const state = () => {
    const connections = bridge?.listConnections() ?? [];
    return {
      state: pending.size
        ? "approval_pending"
        : connections.some((item) => item.connected)
          ? "connected"
          : "disconnected",
      pending: [...pending.values()].map(({ resolve, ...item }) => item),
      connections,
    };
  };
  bridge = await createBridge({
    dataRoot,
    extensionIds,
    requestApproval(identity) {
      return new Promise((resolve) => {
        const id = `${identity.extensionId}:${identity.profileId}`;
        pending.set(id, { ...identity, id, resolve });
        showWindow();
        realqa.changed();
      });
    },
    acceptCapture(capture) {
      const epoch = captureEpoch;
      return realqa.enqueue(async () => {
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
        realqa.appendCapture({ ...decoded, metadata: capture.metadata });
        if (capture.clipped)
          realqa.emit("lane.realqa.notice", {
            message:
              "Only the visible part of this element was captured. Scroll and capture again to include another part.",
          });
      });
    },
    onStatus: () => realqa.changed(),
  });
  return {
    state,
    async startDOMCapture({ shortcut = false } = {}) {
      if (shortcut && !(await realqa.os({ op: "activeBrowser" })).chrome) {
        const error = new Error(
          "Activate Google Chrome with the connected RealQA extension, then use the DOM shortcut or select an element from the extension.",
        );
        error.detail = {
          reason: "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
          message: error.message,
          retryable: true,
        };
        throw error;
      }
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
        const error = new Error(
          remedies[failure.message] ||
            "Open the RealQA extension in Chrome and connect this profile to Lane, then retry.",
        );
        error.detail = {
          reason: "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
          message: error.message,
          retryable: true,
        };
        throw error;
      }
    },
    async invoke(request) {
      if (request.op === "chrome.approve" || request.op === "chrome.reject") {
        const approval = pending.get(request.id);
        if (!approval) throw new Error("Approval is no longer pending.");
        pending.delete(request.id);
        approval.resolve(request.op === "chrome.approve");
        realqa.changed();
        return {};
      }
      if (request.op === "chrome.disconnect") {
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
      await bridge.close();
    },
  };
}
