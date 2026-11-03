// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { generateKeyPairSync } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { publishAndroid, verifyAndroidSigning } from "./store.mjs";

// This public fixture certificate has no private key in the repository.
const pem = readFileSync(
  new URL("./fixtures/upload-certificate.pem", import.meta.url),
  "utf8",
);
const expected =
  "3D:29:0F:BE:59:8F:35:BB:38:6A:7B:7F:95:1C:5F:7A:D1:19:98:85:71:01:DC:E7:00:74:DD:AB:93:CB:C2:F7";

test("the Android bundle must carry the pinned upload certificate", () => {
  const run = (command, args) => {
    assert.equal(command, "keytool");
    assert.deepEqual(args, ["-printcert", "-rfc", "-jarfile", "anydash.aab"]);
    return { status: 0, stdout: `Signer #1:\n\n${pem}` };
  };
  assert.equal(
    verifyAndroidSigning("anydash.aab", expected, run),
    expected.replaceAll(":", "").toLowerCase(),
  );
  assert.throws(
    () => verifyAndroidSigning("anydash.aab", "0".repeat(64), run),
    /does not match/,
  );
  for (const value of [undefined, "", "a".repeat(63)])
    assert.throws(
      () =>
        verifyAndroidSigning("anydash.aab", value, () =>
          assert.fail("pin first"),
        ),
      /ANYDASH_ANDROID_CERTIFICATE_SHA256/,
    );
  assert.throws(
    () =>
      verifyAndroidSigning("anydash.aab", expected, () => ({
        status: 0,
        stdout: "unsigned",
      })),
    /no signing certificate/,
  );
  assert.throws(
    () =>
      verifyAndroidSigning("anydash.aab", expected, () => ({
        status: 1,
        stdout: pem,
      })),
    /Could not read/,
  );
});

const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const credentials = {
  client_email: "fixture@example.invalid",
  private_key: privateKey.export({ format: "pem", type: "pkcs8" }),
};
for (const mismatch of [false, true])
  test(`Google Play internal testing commits only the requested build (mismatch=${mismatch})`, async () => {
    const calls = [];
    const request = async (url, options) => {
      calls.push({ url, ...options });
      const body = url.includes("oauth2")
        ? { access_token: "fixture" }
        : url ===
            "https://androidpublisher.googleapis.com/androidpublisher/v3/applications/io.delino.anydash/edits"
          ? { id: "edit-1" }
          : url.includes("/bundles?")
            ? { versionCode: mismatch ? 8 : 7 }
            : {};
      return { ok: true, json: async () => body };
    };
    const promise = publishAndroid(
      Buffer.from("bundle"),
      "7",
      credentials,
      request,
    );
    if (mismatch) {
      await assert.rejects(promise, /does not match/);
      assert.equal(calls.at(-1).method, "DELETE");
    } else {
      await promise;
      assert.match(calls.at(-1).url, /:commit$/);
      assert.match(
        calls.find((call) => call.method === "PUT").body,
        /"track":"internal"/,
      );
    }
    assert.equal(
      calls.some((call) => call.url.includes(":commit")),
      !mismatch,
    );
  });
