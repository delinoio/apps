// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";

for (const action of ["install", "uninstall"]) {
  test(`Windows scratch ${action} refuses before filesystem or registry changes`, async (t) => {
    const root = await mkdtemp(join(tmpdir(), "lane-windows-install-"));
    t.after(() => rm(root, { recursive: true, force: true }));
    const directory = join(root, "Programs/Lane/resources/app");
    await mkdir(directory, { recursive: true });
    const manifest = join(directory, "package.json");
    const original = JSON.stringify({
      name: "lane",
      testing: true,
      chromeExtensionIds: ["a".repeat(32)],
    });
    await writeFile(manifest, original);
    const script = new URL("../../lane/scripts/desktop.mjs", import.meta.url)
      .href;
    const child = spawnSync(
      process.execPath,
      [
        "--input-type=module",
        "-e",
        `
      import fs from 'node:fs';
      import cp from 'node:child_process';
      import {syncBuiltinESMExports} from 'node:module';
      // Keep this portable fixture from invoking any real registry command.
      cp.execFileSync = cp.spawnSync = () => { throw new Error('UNEXPECTED_EXTERNAL_COMMAND'); };
      for (const name of ['mkdirSync','cpSync','rmSync','writeFileSync'])
        fs[name] = () => { throw new Error('UNEXPECTED_FILESYSTEM_WRITE'); };
      syncBuiltinESMExports();
      Object.defineProperty(process, 'platform', {value: 'win32'});
      process.argv[2] = ${JSON.stringify(action)};
      await import(${JSON.stringify(script)});
    `,
      ],
      {
        env: {
          ...process.env,
          LANE_NATIVE_INSTALL_TEST_ROOT: root,
          LANE_NATIVE_TESTING: "1",
        },
        encoding: "utf8",
        timeout: 10000,
      },
    );
    assert.equal(child.error, undefined);
    assert.equal(child.status, 1);
    assert.match(child.stderr, /Windows scratch installation is unavailable/);
    assert.doesNotMatch(child.stderr, /UNEXPECTED_/);
    assert.equal(await readFile(manifest, "utf8"), original);
  });
}
