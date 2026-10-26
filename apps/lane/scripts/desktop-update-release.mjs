// SPDX-License-Identifier: Apache-2.0
import {
  createHash,
  createPrivateKey,
  createPublicKey,
  sign,
} from "node:crypto";
import { createReadStream } from "node:fs";
import {
  access,
  lstat,
  mkdir,
  readFile,
  stat,
  writeFile,
} from "node:fs/promises";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { basename, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import {
  archiveName,
  assetURL,
  maximumArchiveSize,
  targetName,
  versionParts,
  verifyManifest,
} from "../native/desktop/update-contract.mjs";

const execute = promisify(execFile);
const psQuote = (value) => `'${value.replaceAll("'", "''")}'`;

// This command prepares candidates only. Signing keys and release publication
// remain outside the checkout and require the maintainer's release environment.
export async function prepareDesktopUpdate({
  application,
  output,
  privateKeyFile,
  platform = process.platform,
  arch = process.arch,
  executeCommand = execute,
}) {
  const target = targetName(platform, arch);
  application = resolve(application);
  output = resolve(output);
  const outputRelative = relative(application, output);
  if (
    outputRelative === "" ||
    (!isAbsolute(outputRelative) &&
      outputRelative !== ".." &&
      !outputRelative.startsWith(`..${sep}`))
  )
    throw new Error("Release output must be outside the application.");
  const appName = platform === "darwin" ? "Lane.app" : "application";
  if (basename(application) !== appName)
    throw new Error(`Expected ${appName}.`);
  if (!(await lstat(application)).isDirectory())
    throw new Error(
      "The application must be a directory, not a symbolic link.",
    );
  const resources = join(
    application,
    platform === "darwin" ? "Contents/Resources/app" : "resources/app",
  );
  const config = JSON.parse(
    await readFile(join(resources, "package.json"), "utf8"),
  );
  versionParts(config.version);
  if (
    config.name !== "lane" ||
    config.scheme !== "lane" ||
    config.testing ||
    config.updates?.target !== target
  )
    throw new Error(
      "An update-enabled production package for this host is required.",
    );
  const key = createPrivateKey(await readFile(privateKeyFile));
  if (key.asymmetricKeyType !== "ed25519")
    throw new Error("An Ed25519 private key is required.");
  const publicKey = createPublicKey(key)
    .export({ format: "der", type: "spki" })
    .toString("base64");
  if (publicKey !== config.updates?.publicKey)
    throw new Error("The release key does not match the packaged public key.");
  await access(
    join(
      application,
      platform === "darwin" ? "Contents/MacOS/lynxtron" : "lynxtron.exe",
    ),
  );
  if (platform === "darwin")
    await executeCommand("/usr/bin/codesign", [
      "--verify",
      "--deep",
      "--strict",
      application,
    ]);
  await mkdir(output, { recursive: true });
  const archive = join(output, archiveName(config.version, target));
  const metadata = join(output, `update-${target}.json`);
  // Refuse to overwrite an existing candidate for an immutable release.
  for (const path of [archive, metadata]) {
    try {
      await lstat(path);
      throw new Error("The release candidate already exists.");
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  if (platform === "darwin")
    await executeCommand("/usr/bin/ditto", [
      "-c",
      "-k",
      "--sequesterRsrc",
      "--keepParent",
      application,
      archive,
    ]);
  else
    await executeCommand("powershell.exe", [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      `Compress-Archive -LiteralPath ${psQuote(application)} -DestinationPath ${psQuote(archive)}`,
    ]);
  const size = (await stat(archive)).size;
  if (size <= 0 || size > maximumArchiveSize)
    throw new Error("Invalid release archive size.");
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(archive)) hash.update(chunk);
  const manifest = {
    schema: 1,
    version: config.version,
    target,
    url: assetURL(config.version, basename(archive)),
    sha256: hash.digest("hex"),
    size,
  };
  const payload = Buffer.from(JSON.stringify(manifest));
  const envelope = {
    payload: payload.toString("base64"),
    signature: sign(null, payload, key).toString("base64"),
  };
  verifyManifest(envelope, {
    publicKey,
    version: config.version,
    platform,
    arch,
  });
  await writeFile(metadata, JSON.stringify(envelope) + "\n", { flag: "wx" });
  return manifest;
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    const [application, output] = process.argv.slice(2);
    if (
      !application ||
      !output ||
      !process.env.LANE_DESKTOP_UPDATE_PRIVATE_KEY_FILE
    )
      throw new Error(
        "Usage: desktop-update-release.mjs <application> <output>; set LANE_DESKTOP_UPDATE_PRIVATE_KEY_FILE.",
      );
    const result = await prepareDesktopUpdate({
      application,
      output,
      privateKeyFile: process.env.LANE_DESKTOP_UPDATE_PRIVATE_KEY_FILE,
    });
    console.info(
      JSON.stringify({ event: "lane_update_candidate_prepared", ...result }),
    );
  } catch {
    console.error(JSON.stringify({ event: "lane_update_candidate_failed" }));
    process.exitCode = 1;
  }
}
