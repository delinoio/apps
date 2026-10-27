// SPDX-License-Identifier: Apache-2.0
import { win32 } from "node:path";

export function desktopTarCommand(
  platform = process.platform,
  env = process.env,
) {
  if (platform !== "win32") return "tar";
  // Git Bash can put GNU tar ahead of Windows bsdtar. GNU tar interprets drive
  // letters as remote hosts and cannot read the SDK/Node ZIP archives. Keep the
  // native executable explicit while desktop dependencies include ZIP files.
  const systemRoot = env.SystemRoot;
  if (typeof systemRoot !== "string" || !win32.isAbsolute(systemRoot))
    throw new Error(
      "Windows archive extraction requires an absolute SystemRoot.",
    );
  return win32.join(systemRoot, "System32", "tar.exe");
}
