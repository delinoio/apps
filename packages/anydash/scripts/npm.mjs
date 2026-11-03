// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";

export function npmCommand(
  args,
  { platform = process.platform, node = process.execPath, commands } = {},
) {
  if (platform !== "win32") return { file: "npm", args };
  // Windows npm.cmd cannot be executed without a shell. Use its JavaScript
  // entry with Node so paths and arguments never go through cmd.exe parsing.
  const candidates =
    commands ??
    execFileSync("where.exe", ["npm.cmd"], { encoding: "utf8" })
      .trim()
      .split(/\r?\n/);
  const cli = candidates
    .map((command) => join(dirname(command), "node_modules/npm/bin/npm-cli.js"))
    .find(existsSync);
  if (!cli) throw new Error("Cannot locate npm CLI beside npm.cmd");
  return { file: node, args: [cli, ...args] };
}

export function npm(args, options = {}) {
  const command = npmCommand(args);
  return execFileSync(command.file, command.args, options);
}
