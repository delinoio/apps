// SPDX-License-Identifier: Apache-2.0
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { platformPackage } from "../bin/verify.mjs";
import { npmCommand } from "./npm.mjs";

// Serve only local candidates so npm exercises normal optional dependency selection
// without relying on published versions or an existing npm cache.
export async function installCandidate({
  dist,
  version,
  keys,
  prefix,
  omitOptional = false,
  targetKey,
  global = false,
}) {
  const packages = new Map(
    keys.map((key) => {
      const info = platformPackage(key);
      const filename = `delinoio-lane-${key}-${version}.tgz`;
      return [
        info.name,
        { info, filename, bytes: readFileSync(resolve(dist, filename)) },
      ];
    }),
  );
  const server = createServer((request, response) => {
    const path = decodeURIComponent(request.url.slice(1));
    const tarball = [...packages.values()].find(
      (entry) => entry.filename === path,
    );
    if (tarball) return response.end(tarball.bytes);
    const entry = packages.get(path);
    if (!entry) {
      response.writeHead(404);
      return response.end();
    }
    response.setHeader("Content-Type", "application/json");
    response.end(
      JSON.stringify({
        name: path,
        versions: {
          [version]: {
            name: path,
            version,
            os: [entry.info.os],
            cpu: [entry.info.cpu],
            dist: {
              tarball: `http://127.0.0.1:${server.address().port}/${entry.filename}`,
            },
          },
        },
      }),
    );
  });
  await new Promise((done, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", done);
  });
  try {
    const registry = `http://127.0.0.1:${server.address().port}`;
    await new Promise((done, reject) => {
      const command = npmCommand([
        "install",
        "--ignore-scripts",
        "--no-audit",
        "--no-fund",
        "--fetch-retries=0",
        ...(global ? ["--global"] : []),
        ...(targetKey
          ? [
              `--os=${platformPackage(targetKey).os}`,
              `--cpu=${platformPackage(targetKey).cpu}`,
            ]
          : []),
        "--registry",
        registry,
        "--cache",
        resolve(prefix, "cache"),
        "--userconfig",
        resolve(prefix, "empty.npmrc"),
        ...(omitOptional ? ["--omit=optional"] : []),
        "--prefix",
        prefix,
        resolve(dist, `delinoio-lane-${version}.tgz`),
      ]);
      const child = spawn(command.file, command.args, {
        env: {
          ...process.env,
          npm_config_registry: registry,
          "npm_config_@delinoio:registry": registry,
        },
        stdio: ["ignore", "pipe", "pipe"],
        timeout: 30000,
      });
      let output = "";
      child.stdout.on("data", (data) => {
        output += data;
      });
      child.stderr.on("data", (data) => {
        output += data;
      });
      child.once("error", reject);
      child.once("exit", (code) =>
        code === 0 ? done() : reject(new Error(output)),
      );
    });
  } finally {
    await new Promise((done) => server.close(done));
  }
}
