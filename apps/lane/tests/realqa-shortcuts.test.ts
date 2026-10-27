// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { RealQA } from "../native/desktop/realqa.mjs";

const modes = ["screen", "window", "region", "dom"];

function fixture(t: { after: (cleanup: () => void) => void }) {
  const directory = mkdtempSync(join(tmpdir(), "lane-realqa-shortcuts-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const settingsPath = join(directory, "settings.json");
  const active = new Map<string, string>();
  const events: { name: string; detail: any }[] = [];
  const native = {
    capture(raw: string, callback: (response: string) => void) {
      const { mode, accelerator } = JSON.parse(raw);
      if (/^(Command|Cmd|Meta)\+K$|^(Ctrl|Control)\+K$/.test(accelerator)) {
        callback(
          JSON.stringify({
            ok: false,
            error: {
              reason: "ERROR_REASON_CONFLICT",
              message: "Lane already uses that shortcut.",
              retryable: false,
            },
          }),
        );
        return;
      }
      if (accelerator) active.set(mode, accelerator);
      else active.delete(mode);
      callback(JSON.stringify({ ok: true, value: {} }));
    },
  };
  const host = new RealQA({
    native,
    settingsPath,
    emit: (name: string, detail: any) => events.push({ name, detail }),
  });
  return { host, active, events, settingsPath };
}

test("rejected capture shortcuts preserve the native and persisted assignment", async (t) => {
  const { host, active, settingsPath } = fixture(t);
  for (const mode of modes) {
    for (const accelerator of [
      "Command+K",
      "Cmd+K",
      "Meta+K",
      "Ctrl+K",
      "Control+K",
    ]) {
      await assert.rejects(
        host.dispatch({ op: "shortcut", mode, accelerator }),
        (error: any) => error.detail?.reason === "ERROR_REASON_CONFLICT",
      );
      assert.equal(host.shortcuts[mode], "");
      assert.equal(active.has(mode), false);
    }
    const previous = `Ctrl+Shift+${modes.indexOf(mode) + 1}`;
    await host.dispatch({ op: "shortcut", mode, accelerator: previous });
    for (const accelerator of [
      "Command+K",
      "Cmd+K",
      "Meta+K",
      "Ctrl+K",
      "Control+K",
    ]) {
      await assert.rejects(
        host.dispatch({ op: "shortcut", mode, accelerator }),
        (error: any) => error.detail?.reason === "ERROR_REASON_CONFLICT",
      );
      assert.equal(host.shortcuts[mode], previous);
      assert.equal(active.get(mode), previous);
      assert.equal(
        JSON.parse(readFileSync(settingsPath, "utf8"))[mode],
        previous,
      );
    }
  }
});

test("startup reports a conflicting stored shortcut and restores other modes", async (t) => {
  const { host, active, events, settingsPath } = fixture(t);
  writeFileSync(
    settingsPath,
    JSON.stringify({ screen: "Command+K", window: "Ctrl+Shift+8" }),
  );
  await host.start();
  assert.equal(host.shortcuts.screen, "");
  assert.equal(active.has("screen"), false);
  assert.equal(host.shortcuts.window, "Ctrl+Shift+8");
  assert.equal(active.get("window"), "Ctrl+Shift+8");
  assert.deepEqual(events, [
    {
      name: "lane.realqa.error",
      detail: {
        message:
          "The screen shortcut is already in use. Assign another shortcut in RealQA.",
        reason: "ERROR_REASON_CONFLICT",
        retryable: false,
      },
    },
  ]);
  assert.equal(
    JSON.parse(readFileSync(settingsPath, "utf8")).screen,
    "Command+K",
  );
});
