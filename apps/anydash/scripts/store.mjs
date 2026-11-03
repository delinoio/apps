// SPDX-License-Identifier: Apache-2.0
// Signed Android bundle checks and the optional Google Play internal-testing
// upload for the unified release. Store listings, review submission and the
// public release stay with the maintainer.
import { spawnSync } from "node:child_process";
import { X509Certificate, createSign } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const androidPackage = "io.delino.anydash";

/** Confirms that the bundle is signed with the pinned upload certificate. */
export function verifyAndroidSigning(
  artifact,
  expectedFingerprint,
  run = spawnSync,
) {
  const expected = expectedFingerprint
    ?.trim()
    .replaceAll(":", "")
    .toLowerCase();
  if (!/^[a-f0-9]{64}$/.test(expected ?? ""))
    throw new Error(
      "Set ANYDASH_ANDROID_CERTIFICATE_SHA256 to the upload certificate SHA-256 fingerprint",
    );
  const result = run("keytool", ["-printcert", "-rfc", "-jarfile", artifact], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error || result.status !== 0)
    throw new Error("Could not read the Android upload certificate");
  const pem = result.stdout.match(
    /-----BEGIN CERTIFICATE-----[\s\S]*?-----END CERTIFICATE-----/,
  );
  if (!pem) throw new Error("The Android bundle has no signing certificate");
  const actual = new X509Certificate(pem[0]).fingerprint256
    .replaceAll(":", "")
    .toLowerCase();
  if (actual !== expected)
    throw new Error(
      "Android upload certificate does not match ANYDASH_ANDROID_CERTIFICATE_SHA256",
    );
  return actual;
}

/** Uploads the signed bundle to the Google Play internal testing track. */
export async function publishAndroid(
  artifact,
  buildNumber,
  credentials,
  request = fetch,
) {
  if (!/^[1-9][0-9]*$/.test(buildNumber ?? ""))
    throw new Error("Invalid build number");
  const base = `https://androidpublisher.googleapis.com/androidpublisher/v3/applications/${androidPackage}/edits`;
  const encode = (value) =>
    Buffer.from(JSON.stringify(value)).toString("base64url");
  const now = Math.floor(Date.now() / 1000);
  const unsigned = `${encode({ alg: "RS256", typ: "JWT" })}.${encode({
    iss: credentials.client_email,
    scope: "https://www.googleapis.com/auth/androidpublisher",
    aud: "https://oauth2.googleapis.com/token",
    iat: now,
    exp: now + 3600,
  })}`;
  const signature = createSign("RSA-SHA256")
    .update(unsigned)
    .sign(credentials.private_key, "base64url");
  const tokenResponse = await request("https://oauth2.googleapis.com/token", {
    method: "POST",
    body: new URLSearchParams({
      grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
      assertion: `${unsigned}.${signature}`,
    }),
  });
  if (!tokenResponse.ok)
    throw new Error(`Google authentication failed (${tokenResponse.status})`);
  const { access_token: token } = await tokenResponse.json();
  if (!token) throw new Error("Google authentication returned no access token");
  const call = async (url, method, body, type = "application/json") => {
    const response = await request(url, {
      method,
      headers: { Authorization: `Bearer ${token}`, "Content-Type": type },
      body,
    });
    // Never log provider response bodies: they may contain account information.
    if (!response.ok)
      throw new Error(`Google Play ${method} failed (${response.status})`);
    return response.json();
  };
  const edit = await call(base, "POST", "{}");
  if (!edit.id || !/^[a-zA-Z0-9_-]+$/.test(edit.id))
    throw new Error("Invalid Google Play edit ID");
  let committed = false;
  try {
    const uploaded = await call(
      `https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/${androidPackage}/edits/${edit.id}/bundles?uploadType=media`,
      "POST",
      artifact,
      "application/octet-stream",
    );
    if (String(uploaded.versionCode) !== buildNumber)
      throw new Error(
        "Uploaded Android versionCode does not match build number",
      );
    await call(
      `${base}/${edit.id}/tracks/internal`,
      "PUT",
      JSON.stringify({
        track: "internal",
        releases: [{ versionCodes: [buildNumber], status: "completed" }],
      }),
    );
    await call(`${base}/${edit.id}:commit`, "POST", "{}");
    committed = true;
    console.info(
      JSON.stringify({
        event: "anydash_android_published",
        track: "internal",
        buildNumber,
      }),
    );
  } finally {
    if (!committed)
      await request(`${base}/${edit.id}`, {
        method: "DELETE",
        headers: { Authorization: `Bearer ${token}` },
      }).catch(() => {});
  }
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const [command, artifact] = process.argv.slice(2);
  try {
    if (command === "verify-android") {
      const sha256 = verifyAndroidSigning(
        artifact,
        process.env.ANYDASH_ANDROID_CERTIFICATE_SHA256,
      );
      console.info(
        JSON.stringify({
          event: "anydash_android_certificate_verified",
          sha256,
        }),
      );
    } else if (command === "publish-android")
      await publishAndroid(
        readFileSync(artifact),
        process.env.ANYDASH_BUILD_NUMBER,
        JSON.parse(
          process.env.ANYDASH_GOOGLE_PLAY_SERVICE_ACCOUNT_JSON ?? "{}",
        ),
      );
    else throw new Error("Expected verify-android or publish-android");
  } catch (error) {
    console.error(
      JSON.stringify({ event: "anydash_store_failed", message: error.message }),
    );
    process.exitCode = 1;
  }
}
