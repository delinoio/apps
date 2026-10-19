import { spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);

function git(directory, args, optional = false) {
  const result = spawnSync("git", args, {
    cwd: directory,
    encoding: "utf8",
    shell: false,
  });
  if (result.error || result.status !== 0) {
    if (optional) return "";
    throw (
      result.error ?? new Error(result.stderr.trim() || "Git command failed.")
    );
  }
  return result.stdout.trim();
}

function canonicalDirectory(directory) {
  try {
    return realpathSync(directory);
  } catch {
    return directory;
  }
}

export function hookInstallArguments(directory) {
  if (git(directory, ["rev-parse", "--is-inside-work-tree"], true) !== "true")
    return null;
  const root = git(directory, ["rev-parse", "--show-toplevel"]);
  const common = git(directory, [
    "rev-parse",
    "--path-format=absolute",
    "--git-common-dir",
  ]);
  const configured = git(
    directory,
    ["config", "--path", "--get", "core.hooksPath"],
    true,
  );
  if (!configured) return ["install"];
  const hooks = canonicalDirectory(resolve(root, configured));
  const shared = canonicalDirectory(join(common, "hooks"));
  // Lefthook rejects configured shared hooks in linked worktrees. Force only
  // that repository-owned path until upstream accepts this safe configuration.
  return hooks === shared ? ["install", "--force"] : ["install"];
}

export function installGitHooks(directory, run = spawnSync) {
  const args = hookInstallArguments(directory);
  if (args === null) {
    process.stdout.write(
      "Git metadata is unavailable; skipping Lefthook installation.\n",
    );
    return 0;
  }
  const result = run(process.execPath, [require.resolve("lefthook"), ...args], {
    cwd: directory,
    stdio: "inherit",
    shell: false,
  });
  if (result.error) throw result.error;
  return result.status ?? 1;
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    process.exitCode = installGitHooks(process.cwd());
  } catch (error) {
    process.stderr.write(error.message + "\n");
    process.exitCode = 1;
  }
}
