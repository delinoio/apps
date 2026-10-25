// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { create } from "@bufbuild/protobuf";
import {
  CreateCaptureTaskRequestSchema,
  GetAttachmentResponseSchema,
  TaskSchema,
} from "@delino/protos/lane/v1/lane_pb";
import { fromUI, toUI } from "../src/serializable.ts";
import {
  attachmentSource,
  decodePNG,
  matchingRepository,
} from "../src/captures-model.ts";
import { createUITask, draftFor } from "../src/model.ts";
test("PNG bytes and uint64 metadata survive JSON transport and nested protobuf restoration", () => {
  const png = new Uint8Array([0, 1, 2, 127, 128, 254, 255]);
  const request = create(CreateCaptureTaskRequestSchema, {
    images: [{ png, capture: { byteSize: 18446744073709551615n } }],
  });
  const ui = JSON.parse(JSON.stringify(toUI(request)));
  assert.deepEqual(ui.images[0].png, [0, 1, 2, 127, 128, 254, 255]);
  assert.deepEqual(fromUI(CreateCaptureTaskRequestSchema, ui), request);
  const reply = create(GetAttachmentResponseSchema, { png });
  assert.deepEqual(
    fromUI(
      GetAttachmentResponseSchema,
      JSON.parse(JSON.stringify(toUI(reply))),
    ),
    reply,
  );
});
test("base64 round trips every byte and rejects executable or local attachment URLs", () => {
  for (const size of [1, 2, 3, 255, 256, 257]) {
    const bytes = Array.from({ length: size }, (_, i) => i % 256);
    const source = attachmentSource(bytes, "")!;
    assert.equal(
      source,
      `data:image/png;base64,${Buffer.from(bytes).toString("base64")}`,
    );
    assert.deepEqual(decodePNG(source.split(",")[1]), bytes);
  }
  for (const url of [
    "javascript:alert(1)",
    "file:///private/image.png",
    "http://insecure.test/image",
    "data:text/html,x",
  ])
    assert.equal(attachmentSource([], url), undefined);
  assert.equal(
    attachmentSource([], "https://private.test/signed"),
    "https://private.test/signed",
  );
  assert.throws(() => decodePNG("not a png!?"));
});
test("URL mapping uses longest boundary match, normalized host/port, and case-sensitive paths", () => {
  const rules = [
    { prefix: "https://example.test/app", repositoryId: "one" },
    { prefix: "https://example.test/app/deep", repositoryId: "two" },
  ];
  assert.equal(
    matchingRepository("https://EXAMPLE.test:443/app/deep/page?q=x#f", rules),
    "two",
  );
  assert.equal(
    matchingRepository("https://example.test/apple", rules),
    undefined,
  );
  assert.equal(
    matchingRepository("https://example.test/App", rules),
    undefined,
  );
  assert.equal(matchingRepository(undefined, rules), undefined);
  assert.equal(
    matchingRepository("https://user@example.test/app", rules),
    undefined,
  );
});
test("normal task edits retain capture references and exact byte sizes", () => {
  const task = toUI(
    create(TaskSchema, {
      title: "before",
      captures: [
        {
          attachmentId: "image",
          byteSize: 9007199254740993n,
          pageTitle: "literal <script>",
        },
      ],
    }),
  );
  const changed = createUITask({ ...task, title: "after" });
  assert.deepEqual(changed.captures, task.captures);
  assert.doesNotThrow(() => JSON.stringify(changed));
});

test("root URL rules match the origin and descendants without crossing host boundaries", () => {
  const rules = [
    { prefix: "https://example.com/", repositoryId: "root" },
    { prefix: "https://example.com/app", repositoryId: "app" },
  ];
  for (const url of [
    "https://example.com",
    "https://example.com/",
    "https://EXAMPLE.COM:443/other?query=1#part",
  ])
    assert.equal(matchingRepository(url, rules), "root");
  assert.equal(
    matchingRepository("https://example.com/app/page", rules),
    "app",
  );
  assert.equal(
    matchingRepository("https://example.com.evil/app", rules),
    undefined,
  );
});
