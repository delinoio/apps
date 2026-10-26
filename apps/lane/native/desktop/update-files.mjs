// SPDX-License-Identifier: Apache-2.0
import { writeFile, rename } from "node:fs/promises";
import { join } from "node:path";

export async function writeResult(path, value) {
  const temporary = `${path}.tmp`;
  await writeFile(temporary, JSON.stringify(value), { mode: 0o600 });
  await rename(temporary, path);
}
export function executablePath(app, platform) {
  return join(
    app,
    platform === "darwin" ? "Contents/MacOS/lynxtron" : "lynxtron.exe",
  );
}
