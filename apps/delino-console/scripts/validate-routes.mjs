// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { readFile, access } from "node:fs/promises";

const app = new URL("../", import.meta.url);
const registry = JSON.parse(
  await readFile(new URL("i18n/locales.json", app), "utf8"),
);
const escapeHtml = (text) =>
  text.replace(
    /[&<>"']/g,
    (character) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      })[character],
  );

for (const { lang } of registry.locales) {
  const prefix = lang === registry.sourceLang ? "" : `${lang}/`;
  const html = await readFile(new URL(`dist/${prefix}index.html`, app), "utf8");
  const messages = JSON.parse(
    await readFile(new URL(`i18n/messages/${lang}.json`, app), "utf8"),
  );
  assert.ok(html.includes(`lang="${lang}"`), `${lang}: HTML language`);
  assert.ok(
    html.includes(`<title>${escapeHtml(messages["console.title"])}</title>`),
  );
  assert.ok(
    html.includes(
      `name="description" content="${escapeHtml(messages["console.description"])}"`,
    ),
  );
  assert.ok(
    html.includes(`rel="canonical" href="https://console.delino.io/${prefix}"`),
  );
  for (const { lang: alternate } of registry.locales) {
    const path = alternate === registry.sourceLang ? "" : `${alternate}/`;
    assert.ok(
      html.includes(
        `hreflang="${alternate}" href="https://console.delino.io/${path}"`,
      ),
    );
  }
  for (const key of [
    "console.loading",
    "console.loadFailed",
    "console.reload",
    "console.noScript",
  ]) {
    assert.ok(
      html.includes(escapeHtml(messages[key])),
      `${lang}: independent ${key} copy`,
    );
  }
  assert.match(html, /<main[^>]*id="locale-error"[^>]*hidden/);
  assert.match(html, /<noscript>/);
  const assets = [...html.matchAll(/(?:src|href)="(\/static\/[^"?#]+)"/g)];
  assert.ok(assets.length > 0, `${lang}: production assets`);
  for (const [, path] of assets) await access(new URL(`dist${path}`, app));
  assert.doesNotMatch(html, /api\.openai\.com|accessToken|refreshToken/);
  console.log(
    JSON.stringify({
      event: "delino_console_route",
      language: lang,
      result: "pass",
    }),
  );
}
for (const name of ["LICENSE", "NOTICE"]) {
  assert.equal(
    await readFile(new URL(`dist/${name}`, app), "utf8"),
    await readFile(new URL(`../../${name}`, app), "utf8"),
  );
}
