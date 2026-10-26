// SPDX-License-Identifier: Apache-2.0
import { uuid } from "./realqa.mjs";
// This opt-in test requires a scratch native data root. Raw screenshots remain
// in memory; only a fully covered 32x32 final fixture is saved to the scratch DB.
// It does not grant permission or establish human selector/UI acceptance.
export async function realQASmoke(realqa) {
  const permission = await realqa.os({ op: "permission" });
  if (permission.permission !== "granted") {
    console.log("LANE_NATIVE_CAPTURE_PERMISSION_REQUIRED");
    return;
  }
  const { sources } = await realqa.os({ op: "sources", mode: "screen" });
  if (!sources.length) throw new Error("No monitor is available.");
  const screen = await realqa.os({
    op: "capture",
    mode: "screen",
    sourceId: sources[0].id,
  });
  if (!screen.png || screen.width < 1 || screen.height < 1)
    throw new Error("Monitor capture failed.");
  const edited = await realqa.os({
    op: "render",
    png: screen.png,
    layers: [
      { type: "cover", x: 0, y: 0, width: 32, height: 32, color: "#000000" },
    ],
    crop: { x: 0, y: 0, width: 32, height: 32 },
  });
  if (edited.width !== 32 || edited.height !== 32)
    throw new Error("Captured pixels could not be edited.");
  const region = await realqa.os({
    op: "capture",
    mode: "region",
    sourceId: sources[0].id,
    rect: { x: 0, y: 0, width: 32, height: 32 },
  });
  if (!region.png) throw new Error("Region capture failed.");
  const windows = await realqa.os({ op: "sources", mode: "window" });
  if (windows.sources.length) {
    const image = await realqa.os({
      op: "capture",
      mode: "window",
      sourceId: windows.sources[0].id,
    });
    if (!image.png) throw new Error("Window capture failed.");
    console.log("LANE_NATIVE_WINDOW_CAPTURE_PASSED");
  } else console.log("LANE_NATIVE_WINDOW_CAPTURE_NO_SOURCE");
  console.log("LANE_NATIVE_MONITOR_REGION_EDIT_PASSED");
  const bridge = async (method, ...args) => {
    const reply = JSON.parse(
      await new Promise((resolve) =>
        realqa.native.store[method](...args, resolve),
      ),
    );
    if (!reply.ok)
      throw new Error(reply.error?.reason || "Store operation failed.");
    return reply.response;
  };
  const opened = await bridge(
    "open",
    JSON.stringify({ formatVersion: 3, spaceId: "" }),
  );
  const meta = (requestId) => ({
    requestId: requestId || uuid(),
    origin: {
      kind: "ORIGIN_KIND_APP",
      deviceId: opened.deviceId,
      deviceName: "Native RealQA smoke",
    },
  });
  const call = (method, request) =>
    bridge(
      "invoke",
      method,
      JSON.stringify({ spaceId: opened.spaceId, request }),
    );
  const repositoryId = uuid();
  await call("CreateRepository", {
    meta: meta(),
    id: repositoryId,
    url: `https://example.test/realqa/${repositoryId}`,
  });
  realqa.appendCapture(edited);
  await realqa.dispatch({
    op: "updateDraft",
    repositoryId,
    title: "Native RealQA smoke",
    description: "Fully covered capture fixture.",
  });
  const draft = await realqa.dispatch({ op: "export" });
  const request = {
    meta: meta(draft.requestId),
    id: draft.id,
    repositoryId,
    title: draft.title,
    description: draft.description,
    images: draft.images.map(({ id, png, width, height, sha256 }) => ({
      png,
      capture: {
        attachmentId: id,
        width,
        height,
        sha256,
        byteSize: String(Buffer.from(png, "base64").length),
      },
    })),
  };
  const first = await call("CreateCaptureTask", request);
  const replay = await call("CreateCaptureTask", request);
  if (first.result.version.versionId !== replay.result.version.versionId)
    throw new Error("Save replay created another version.");
  const attachment = await call("GetAttachment", {
    taskId: draft.id,
    attachmentId: draft.images[0].id,
  });
  if (attachment.png !== edited.png)
    throw new Error("Saved final PNG changed.");
  await realqa.dispatch({
    op: "complete",
    taskId: draft.id,
    spaceId: opened.spaceId,
  });
  if (realqa.draft || realqa.originals.size)
    throw new Error("Saved draft memory was retained.");
  console.log("LANE_NATIVE_CAPTURE_SAVE_RETRIEVE_REPLAY_PASSED");
}
