// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { parseMarkdown, safeImage } from "../src/markdown.ts";
test("embedded HTML is text, never an element", () => {
  const source = "<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>";
  const blocks = parseMarkdown(source);
  assert.equal(blocks[0].spans[0].text, "<script>alert(1)</script>");
  assert.equal(
    blocks.flatMap((b) => b.spans).some((s) => !!s.image),
    false,
  );
});
test("only HTTPS Markdown images are allowed to load", () => {
  for (const url of [
    "http://example.com/x",
    "//example.com/x",
    "data:image/svg+xml,x",
    "javascript:alert(1)",
    "file:///x",
  ])
    assert.equal(safeImage(url), false);
  assert.equal(safeImage("https://example.com/a.png"), true);
  const images = parseMarkdown(
    "![safe](https://example.com/x) ![blocked](http://example.com/x)",
  ).flatMap((b) => b.spans);
  assert.equal(images.filter((s) => s.image).length, 1);
  assert.equal(images.filter((s) => s.blocked).length, 1);
});
test("Markdown retains bold, emphasis, code, headings, and exact source content", () => {
  const blocks = parseMarkdown("# Heading\n\n**bold** *em* `code`");
  assert.equal(blocks[0].kind, "heading");
  assert.ok(blocks[1].spans.some((s) => s.bold && s.text === "bold"));
  assert.ok(blocks[1].spans.some((s) => s.italic && s.text === "em"));
  assert.ok(blocks[1].spans.some((s) => s.code && s.text === "code"));
});
