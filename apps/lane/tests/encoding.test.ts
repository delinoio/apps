// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { runInNewContext } from "node:vm";
import { build } from "esbuild";
import { readFile } from "node:fs/promises";
import {
  TextEncoder as PolyEncoder,
  TextDecoder as PolyDecoder,
} from "text-encoding-utf-8";

const encoder = new PolyEncoder();
test("UTF-8 polyfill matches native bytes for multilingual, supplementary and lone-surrogate input", () => {
  for (const value of [
    "",
    "Lane\0task",
    "한국어 日本語 العربية",
    "😀𐀀𝄞",
    "e\u0301",
    "\ud800",
    "\udfff",
    "a\ud800b",
    "\ufeffTitle",
  ]) {
    const expected = new TextEncoder().encode(value);
    assert.deepEqual(encoder.encode(value), expected);
    assert.equal(
      new PolyDecoder().decode(expected),
      new TextDecoder().decode(expected),
    );
  }
});
test("UTF-8 decoder rejects malformed bytes in fatal mode and preserves standard replacement behavior", () => {
  for (const bytes of [
    [0xff],
    [0x80],
    [0xc0, 0xaf],
    [0xed, 0xa0, 0x80],
    [0xf4, 0x90, 0x80, 0x80],
    [0xe2, 0x82],
    [0xe2, 0x28, 0xa1],
  ]) {
    const input = Uint8Array.from(bytes);
    assert.throws(
      () => new PolyDecoder("utf-8", { fatal: true }).decode(input),
      TypeError,
    );
    assert.equal(
      new PolyDecoder().decode(input),
      new TextDecoder().decode(input),
    );
  }
  assert.throws(() => new PolyDecoder("utf-16"));
});
test("decoder handles split code points, BOM, and offset views", () => {
  const decoder = new PolyDecoder("utf-8", { fatal: true });
  assert.equal(decoder.decode(Uint8Array.of(0xf0, 0x9f), { stream: true }), "");
  assert.equal(decoder.decode(Uint8Array.of(0x98, 0x80)), "😀");
  assert.equal(new PolyDecoder().decode(encoder.encode("\ufeffLane")), "Lane");
  assert.equal(
    new PolyDecoder("utf-8", { ignoreBOM: true }).decode(
      encoder.encode("\ufeffLane"),
    ),
    "\ufeffLane",
  );
  const bytes = Uint8Array.of(0xff, 0x41, 0xff);
  assert.equal(
    new PolyDecoder("utf-8", { fatal: true }).decode(bytes.subarray(1, 2)),
    "A",
  );
});
test("bootstrap precedes eager generated descriptors in an isolate without web encoding APIs", async () => {
  const source = await readFile(
    new URL("../src/index.tsx", import.meta.url),
    "utf8",
  );
  assert.match(source, /^\/\/[^\n]*\nimport "\.\/encoding";/);
  const result = await build({
    stdin: {
      contents: `import './src/encoding.ts';
    import {create,toBinary,fromBinary} from '@bufbuild/protobuf';
    import {TaskSchema} from '@delino/protos/lane/v1/lane_pb';
    const task=create(TaskSchema,{title:'한국어 😀',description:'Unicode'});
    globalThis.result=fromBinary(TaskSchema,toBinary(TaskSchema,task)).title;`,
      resolveDir: new URL("..", import.meta.url).pathname,
    },
    bundle: true,
    write: false,
    format: "iife",
    platform: "neutral",
    mainFields: ["module", "main"],
  });
  // Separate fresh contexts model the two independently initialized Lynx isolates.
  for (const runtime of ["MTS", "BTS"]) {
    const context: Record<string, unknown> = {};
    runInNewContext(result.outputFiles[0].text, context);
    assert.equal(context.result, "한국어 😀", runtime);
    assert.equal(typeof context.TextEncoder, "function");
    assert.equal(typeof context.TextDecoder, "function");
  }
  const context = { TextEncoder, TextDecoder };
  runInNewContext(result.outputFiles[0].text, context);
  assert.equal(context.TextEncoder, TextEncoder);
  assert.equal(context.TextDecoder, TextDecoder);
});
