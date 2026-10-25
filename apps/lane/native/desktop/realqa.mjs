// SPDX-License-Identifier: Apache-2.0
import { randomBytes, createHash } from "node:crypto";
import { readFileSync, mkdirSync, writeFileSync, renameSync } from "node:fs";
import { dirname } from "node:path";

const modes = ["screen", "window", "region", "dom"];
const fail = (
  message,
  reason = "ERROR_REASON_INVALID_INPUT",
  retryable = false,
) => {
  const error = new Error(message);
  error.detail = { message, reason, retryable };
  throw error;
};
// The host allocates sortable UUID v7 image identities without persisting drafts.
export function uuid() {
  const bytes = randomBytes(16);
  bytes.writeUIntBE(Date.now(), 0, 6);
  bytes[6] = (bytes[6] & 15) | 112;
  bytes[8] = (bytes[8] & 63) | 128;
  const hex = bytes.toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
export class RealQA {
  draft = null;
  originals = new Map();
  shortcuts = Object.fromEntries(modes.map((mode) => [mode, ""]));
  capturePending = false;
  saving = false;
  stopped = false;
  extension;
  constructor({
    native,
    settingsPath,
    emit,
    showComposer,
    closeComposer,
    selectSource,
    beforeCapture,
    afterCapture,
    onSaved,
  }) {
    Object.assign(this, {
      native,
      settingsPath,
      emit,
      showComposer,
      closeComposer,
      selectSource,
      beforeCapture,
      afterCapture,
      onSaved,
    });
  }
  async os(request) {
    const raw = await new Promise((resolve) =>
      this.native.capture(JSON.stringify(request), resolve),
    );
    const reply = JSON.parse(raw);
    if (!reply.ok)
      fail(reply.error.message, reply.error.reason, reply.error.retryable);
    return reply.value;
  }
  async start() {
    let saved;
    try {
      saved = JSON.parse(readFileSync(this.settingsPath, "utf8"));
    } catch {
      return;
    }
    for (const mode of modes)
      if (typeof saved[mode] === "string" && saved[mode]) {
        try {
          await this.os({ op: "shortcut", mode, accelerator: saved[mode] });
          this.shortcuts[mode] = saved[mode];
        } catch {
          this.emit("lane.realqa.error", {
            message: `The ${mode} shortcut is already in use. Assign another shortcut in RealQA.`,
            reason: "ERROR_REASON_CONFLICT",
            retryable: false,
          });
        }
      }
  }
  changed() {
    this.emit("lane.realqa.changed", {});
  }
  image(id) {
    const image = this.draft?.images.find((image) => image.id === id);
    if (!image)
      fail("This capture is no longer in the draft.", "ERROR_REASON_NOT_FOUND");
    return image;
  }
  metadata(value = {}) {
    const metadata = {};
    for (const key of ["pageUrl", "pageTitle", "cssSelector"])
      if (typeof value[key] === "string" && value[key])
        metadata[key] = value[key];
    return metadata;
  }
  appendCapture(value) {
    if (this.stopped)
      fail("Lane is stopping.", "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
    if (this.saving)
      fail(
        "Wait for the current save to finish, then capture again.",
        "ERROR_REASON_CONFLICT",
        true,
      );
    if (
      typeof value.png !== "string" ||
      !Number.isSafeInteger(value.width) ||
      !Number.isSafeInteger(value.height) ||
      value.width < 1 ||
      value.height < 1
    )
      fail("The captured image is invalid.");
    if (!this.draft)
      this.draft = {
        id: uuid(),
        requestId: uuid(),
        images: [],
        title: "",
        description: "",
        repositoryId: "",
      };
    const image = {
      id: uuid(),
      png: value.png,
      width: value.width,
      height: value.height,
      layers: [],
      metadata: this.metadata(value.metadata),
      canUndo: false,
      canRedo: false,
    };
    this.originals.set(image.id, {
      png: value.png,
      width: value.width,
      height: value.height,
      history: [{ layers: [], crop: null }],
      position: 0,
    });
    this.draft.images.push(image);
    this.showComposer();
    this.changed();
    return image;
  }
  async capture(request) {
    if (!modes.includes(request.mode)) fail("Choose a capture mode.");
    if (this.capturePending)
      fail(
        "Finish or cancel the current capture first.",
        "ERROR_REASON_CONFLICT",
      );
    if (request.mode === "dom") {
      if (!this.extension)
        fail(
          "Install and connect the RealQA Chrome extension, then retry.",
          "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
          true,
        );
      return this.extension.startDOMCapture();
    }
    if (request.mode !== "region" && !request.sourceId) {
      this.selectSource(request.mode);
      return { selecting: true };
    }
    this.capturePending = true;
    try {
      await this.beforeCapture?.();
      const value = await this.os({
        op: "capture",
        mode: request.mode,
        sourceId: request.sourceId,
      });
      if (!value.cancelled) this.appendCapture(value);
      return value.cancelled ? { cancelled: true } : { captured: true };
    } finally {
      this.capturePending = false;
      this.afterCapture?.();
    }
  }
  async render(image, state) {
    const original = this.originals.get(image.id);
    const rendered = await this.os({
      op: "render",
      png: original.png,
      layers: state.layers,
      crop: state.crop,
    });
    Object.assign(image, rendered, {
      layers: structuredClone(state.layers),
      crop: state.crop,
    });
  }
  async stop() {
    this.stopped = true;
    await this.os({ op: "cancel" });
    await this.extension?.close();
    this.draft = null;
    this.originals.clear();
  }
  async dispatch(request, window = "main") {
    if (this.stopped)
      fail("Lane is stopping.", "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
    if (
      this.saving &&
      ![
        "state",
        "export",
        "saveFailed",
        "complete",
        "permission",
        "close",
      ].includes(request.op)
    )
      fail(
        "Wait for the current save to finish.",
        "ERROR_REASON_CONFLICT",
        true,
      );
    switch (request.op) {
      case "state":
        return {
          window,
          saving: this.saving,
          draft: this.draft,
          shortcuts: this.shortcuts,
          ...(await this.os({ op: "permission" })),
          ...(this.extension ? { chrome: await this.extension.state() } : {}),
        };
      case "sources":
        if (!["screen", "window"].includes(request.mode))
          fail("Choose screen or window capture.");
        return this.os(request);
      case "permission":
      case "openPermissionSettings":
        return this.os(request);
      case "capture":
        return this.capture(request);
      case "shortcut": {
        if (
          !modes.includes(request.mode) ||
          typeof request.accelerator !== "string"
        )
          fail("Choose a valid shortcut.");
        if (this.shortcuts[request.mode] === request.accelerator) return {};
        const previous = this.shortcuts[request.mode];
        await this.os(request);
        const saved = {
          ...this.shortcuts,
          [request.mode]: request.accelerator,
        };
        try {
          mkdirSync(dirname(this.settingsPath), {
            recursive: true,
            mode: 0o700,
          });
          writeFileSync(`${this.settingsPath}.new`, JSON.stringify(saved), {
            mode: 0o600,
          });
          renameSync(`${this.settingsPath}.new`, this.settingsPath);
        } catch {
          await this.os({
            op: "shortcut",
            mode: request.mode,
            accelerator: previous,
          });
          fail(
            "The shortcut could not be saved. The previous assignment is retained.",
            "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
            true,
          );
        }
        this.shortcuts = saved;
        this.changed();
        return {};
      }
      case "updateDraft": {
        if (!this.draft) fail("Capture an image first.");
        for (const key of ["title", "description", "repositoryId"])
          if (typeof request[key] === "string") this.draft[key] = request[key];
        if (request.imageId && request.metadata)
          this.image(request.imageId).metadata = this.metadata(
            request.metadata,
          );
        this.changed();
        return {};
      }
      case "edit": {
        const image = this.image(request.imageId),
          original = this.originals.get(image.id);
        const state = { layers: request.layers, crop: request.crop ?? null };
        await this.render(image, state);
        original.history.splice(original.position + 1);
        original.history.push(structuredClone(state));
        original.position++;
        image.canUndo = true;
        image.canRedo = false;
        this.changed();
        return {};
      }
      case "undo":
      case "redo": {
        const image = this.image(request.imageId),
          original = this.originals.get(image.id);
        const position = original.position + (request.op === "undo" ? -1 : 1);
        if (position < 0 || position >= original.history.length) return {};
        await this.render(image, original.history[position]);
        original.position = position;
        image.canUndo = position > 0;
        image.canRedo = position < original.history.length - 1;
        this.changed();
        return {};
      }
      case "removeImage":
        this.image(request.imageId);
        this.originals.delete(request.imageId);
        this.draft.images = this.draft.images.filter(
          (image) => image.id !== request.imageId,
        );
        this.changed();
        return {};
      case "reorderImages": {
        if (
          !this.draft ||
          !Array.isArray(request.ids) ||
          request.ids.length !== this.draft.images.length ||
          new Set(request.ids).size !== request.ids.length
        )
          fail("Choose each image once.");
        this.draft.images = request.ids.map((id) => this.image(id));
        this.changed();
        return {};
      }
      case "export":
        if (
          !this.draft?.images.length ||
          !this.draft.title.trim() ||
          !this.draft.repositoryId
        )
          fail(
            "Select a repository, enter a title, and add at least one capture.",
          );
        this.saving = true;
        return {
          ...this.draft,
          images: this.draft.images.map(
            ({ id, png, width, height, metadata }) => ({
              id,
              png,
              width,
              height,
              metadata,
              sha256: createHash("sha256")
                .update(Buffer.from(png, "base64"))
                .digest("hex"),
            }),
          ),
        };
      case "close":
        if (this.draft) this.emit("lane.realqa.closeRequested", {});
        else this.closeComposer();
        return {};
      case "saveFailed":
        this.saving = false;
        this.changed();
        return {};
      case "complete":
      case "discard":
        this.saving = false;
        this.draft = null;
        this.originals.clear();
        this.closeComposer();
        this.changed();
        if (request.op === "complete" && typeof request.taskId === "string")
          this.onSaved?.(request.taskId, request.spaceId);
        return {};
      default:
        if (request.op?.startsWith("chrome.") && this.extension)
          return this.extension.invoke(request);
        fail("Unknown RealQA operation.");
    }
  }
  // Serialize mutations and renders so a slow render cannot restore a removed
  // image, overwrite a newer edit, or release a draft while export is in flight.
  queue = Promise.resolve();
  enqueue(action) {
    const operation = this.queue.then(action);
    this.queue = operation.catch(() => {});
    return operation;
  }
  invoke(raw, window) {
    const action = async () => {
      try {
        return JSON.stringify({
          ok: true,
          value: await this.dispatch(JSON.parse(raw), window),
        });
      } catch (error) {
        return JSON.stringify({
          ok: false,
          error: error.detail ?? {
            reason: "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
            message:
              "RealQA could not complete this operation. Your draft is retained.",
            retryable: true,
          },
        });
      }
    };
    // Revocation waits for accepted captures in the Chrome bridge. It must not
    // hold the draft queue that those captures need to finish or cancel.
    let disconnect = false;
    try {
      disconnect = JSON.parse(raw).op === "chrome.disconnect";
    } catch {}
    return disconnect ? action() : this.enqueue(action);
  }
}
