// SPDX-License-Identifier: Apache-2.0
// Session tokens use the platform secure store: the macOS Keychain and the
// Windows Credential Locker (Credential Manager). Secrets travel over stdin,
// never through command arguments, so other processes cannot read them.
import { spawn } from "node:child_process";

export const SERVICE = "io.delino.anydash.session";

function run(command, args, input) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      stdio: ["pipe", "pipe", "ignore"],
      windowsHide: true,
    });
    const chunks = [];
    child.stdout.on("data", (chunk) => chunks.push(chunk));
    child.on("error", reject);
    child.on("exit", (code) =>
      resolve({ code, output: Buffer.concat(chunks).toString("utf8") }),
    );
    child.stdin.end(input);
  });
}

const quote = (value) => `"${String(value).replace(/["\\]/g, "\\$&")}"`;
const psString = (value) => `'${String(value).replace(/'/g, "''")}'`;

export function createSecureStore(platform = process.platform, exec = run) {
  if (platform === "darwin")
    return {
      async get(account) {
        const { code, output } = await exec(
          "/usr/bin/security",
          ["-i"],
          `find-generic-password -s ${quote(SERVICE)} -a ${quote(account)} -w\n`,
        );
        const value = output.trim();
        return code === 0 && value && !value.startsWith("security:")
          ? value
          : undefined;
      },
      async set(account, secret) {
        const { code } = await exec(
          "/usr/bin/security",
          ["-i"],
          `add-generic-password -U -s ${quote(SERVICE)} -a ${quote(account)} -w ${quote(secret)}\n`,
        );
        if (code !== 0) throw new Error("The Keychain rejected the session.");
      },
      async remove(account) {
        await exec(
          "/usr/bin/security",
          ["-i"],
          `delete-generic-password -s ${quote(SERVICE)} -a ${quote(account)}\n`,
        );
      },
    };
  if (platform === "win32") {
    const vault =
      "[Windows.Security.Credentials.PasswordVault,Windows.Security.Credentials,ContentType=WindowsRuntime] | Out-Null; $v = New-Object Windows.Security.Credentials.PasswordVault;";
    const ps = (script) =>
      exec(
        "powershell.exe",
        ["-NoProfile", "-NonInteractive", "-Command", "-"],
        `${vault}${script}\n`,
      );
    return {
      async get(account) {
        const { code, output } = await ps(
          `try { $c = $v.Retrieve(${psString(SERVICE)}, ${psString(account)}); $c.RetrievePassword(); [Console]::Out.Write($c.Password) } catch { exit 1 }`,
        );
        return code === 0 && output ? output : undefined;
      },
      async set(account, secret) {
        const { code } = await ps(
          `$v.Add((New-Object Windows.Security.Credentials.PasswordCredential(${psString(SERVICE)}, ${psString(account)}, ${psString(secret)})))`,
        );
        if (code !== 0)
          throw new Error("The Credential Manager rejected the session.");
      },
      async remove(account) {
        await ps(
          `try { $v.Remove($v.Retrieve(${psString(SERVICE)}, ${psString(account)})) } catch { }`,
        );
      },
    };
  }
  throw new Error("AnyDash desktop supports macOS and Windows.");
}

/** In-memory store for tests and native-testing builds. */
export function memoryStore() {
  const values = new Map();
  return {
    values,
    async get(account) {
      return values.get(account);
    },
    async set(account, secret) {
      values.set(account, secret);
    },
    async remove(account) {
      values.delete(account);
    },
  };
}
