// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { languageLink } from "../language-links.ts";
import registry from "../i18n/locales.json" with { type: "json" };

const root = fileURLToPath(new URL("../dist/", import.meta.url));
async function files(directory) {
  const result = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory())
      result.push(
        ...(await files(join(directory, entry.name))).map(
          (path) => `${entry.name}/${path}`,
        ),
      );
    else result.push(entry.name);
  }
  return result;
}
// Services whose whole section is English-only, from the registered directory
// sections of the service index.
const englishOnlyServices = registry.sourceOnly.sections["services/index.md"];
const route = (path) =>
  path
    .replace(/\.html$/u, "")
    .replace(/\/index$/u, "/")
    .replace(/\/$/u, "") || "/";

test("language menus fall back only for unavailable translations", () => {
  for (const { lang } of registry.locales.filter(
    ({ lang }) => lang !== registry.sourceLang,
  )) {
    for (const page of registry.sourceOnly.pages) {
      for (const suffix of ["", "?query=value#fragment"]) {
        const href = `/${lang}/${page.replace(/\.md$/u, ".html")}${suffix}`;
        assert.deepEqual(languageLink(href, lang, "alternate"), {
          href: `/${lang}/services/`,
          rel: undefined,
        });
        assert.deepEqual(languageLink(href, lang), { href, rel: undefined });
      }
    }
    const href = `/${lang}/services/delitools/api-keys.html`;
    assert.deepEqual(languageLink(href, lang, "alternate"), {
      href,
      rel: "alternate",
    });
  }
  assert.deepEqual(languageLink("/services/perfine/", "en", "alternate"), {
    href: "/services/perfine/",
    rel: "alternate",
  });
  assert.deepEqual(
    languageLink("https://example.com/ko/services/perfine/", "ko", "alternate"),
    { href: "https://example.com/ko/services/perfine/", rel: "alternate" },
  );
});

test("built English-only pages and locale navigation have existing destinations", async () => {
  const all = await files(root);
  const html = all.filter((path) => path.endsWith(".html"));
  const routes = new Set(html.map((path) => route(`/${path}`)));
  for (const page of registry.sourceOnly.pages) {
    assert.ok(routes.has(route(`/${page.replace(/\.md$/u, ".html")}`)), page);
    for (const { lang } of registry.locales.filter(
      ({ lang }) => lang !== registry.sourceLang,
    ))
      assert.ok(
        !routes.has(route(`/${lang}/${page.replace(/\.md$/u, ".html")}`)),
        `${lang}/${page}`,
      );
  }
  for (const path of html) {
    const text = await readFile(join(root, path), "utf8");
    for (const [, href] of text.matchAll(
      /href="(\/(?!\/)[^"?#]*)(?:[^"]*)"/gu,
    )) {
      if (/\.[a-z0-9]+$/iu.test(href) && !href.endsWith(".html")) continue;
      assert.ok(routes.has(route(href)), `${path} links to missing ${href}`);
    }
    for (const service of englishOnlyServices) {
      if (path.startsWith(`services/${service}/`)) {
        for (const { lang } of registry.locales.filter(
          ({ lang }) => lang !== registry.sourceLang,
        )) {
          assert.ok(
            text.includes(`href="/${lang}/services/`),
            `${path}: missing ${lang} fallback`,
          );
          assert.ok(
            !text.includes(`/${lang}/services/${service}`),
            `${path}: false translation link`,
          );
        }
      }
      if (
        registry.locales.some(
          ({ lang }) => lang !== "en" && path.startsWith(`${lang}/`),
        )
      )
        assert.ok(
          !text.includes(`/services/${service}/`),
          `${path}: English-only service ${service} leaked into locale navigation`,
        );
    }
  }
  const directory = await readFile(join(root, "services/index.html"), "utf8");
  assert.ok(
    directory.includes(
      "investigate performance locally with profiles, isolated candidates, and optional private hosting.",
    ),
  );
  assert.ok(
    directory.includes(
      "see AI coding subscription limits, local AI session usage from your devices, and deployment platform status and cost in one dashboard.",
    ),
  );
  const anydash = await readFile(
    join(root, "services/anydash/index.html"),
    "utf8",
  );
  for (const page of [
    "getting-started",
    "cli",
    "providers",
    "data",
    "troubleshooting",
  ])
    assert.ok(
      anydash.includes(`href="/services/anydash/${page}`),
      `AnyDash sidebar is missing ${page}`,
    );
});
