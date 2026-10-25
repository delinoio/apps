// SPDX-License-Identifier: Apache-2.0
import MarkdownIt from "markdown-it";
const parser = new MarkdownIt({ html: false, linkify: false, breaks: true });
export interface MarkdownSpan {
  text: string;
  bold: boolean;
  italic: boolean;
  code: boolean;
  href?: string;
  image?: string;
  blocked?: boolean;
}
export interface MarkdownBlock {
  kind: "paragraph" | "heading" | "code";
  spans: MarkdownSpan[];
}
export function safeImage(url: string) {
  return /^https:\/\/[^\s/]+(?:\/[^\s]*)?$/i.test(url);
}
export function parseMarkdown(source: string): MarkdownBlock[] {
  const blocks: MarkdownBlock[] = [];
  let heading = false;
  let list = false;
  for (const token of parser.parse(source, {})) {
    if (token.type === "heading_open") heading = true;
    if (token.type === "heading_close") heading = false;
    if (token.type === "list_item_open") list = true;
    if (token.type === "fence" || token.type === "code_block")
      blocks.push({
        kind: "code",
        spans: [
          { text: token.content, bold: false, italic: false, code: true },
        ],
      });
    if (token.type !== "inline") continue;
    const spans: MarkdownSpan[] = [];
    let bold = false,
      italic = false;
    let href: string | undefined;
    if (list) {
      spans.push({ text: "• ", bold: false, italic: false, code: false });
      list = false;
    }
    for (const child of token.children || []) {
      if (child.type === "strong_open") bold = true;
      else if (child.type === "strong_close") bold = false;
      else if (child.type === "em_open") italic = true;
      else if (child.type === "em_close") italic = false;
      else if (child.type === "link_open")
        href = child.attrGet("href") || undefined;
      else if (child.type === "link_close") href = undefined;
      else if (child.type === "image") {
        const image = child.attrGet("src") || "";
        spans.push({
          text: child.content,
          image: safeImage(image) ? image : undefined,
          blocked: !safeImage(image),
          bold,
          italic,
          code: false,
        });
      } else if (
        child.type === "text" ||
        child.type === "code_inline" ||
        child.type === "softbreak" ||
        child.type === "hardbreak"
      )
        spans.push({
          text: child.type.endsWith("break") ? "\n" : child.content,
          bold,
          italic,
          href,
          code: child.type === "code_inline",
        });
    }
    blocks.push({ kind: heading ? "heading" : "paragraph", spans });
  }
  return blocks;
}
