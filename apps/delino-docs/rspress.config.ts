import { defineConfig } from "@rspress/core";
import { readFileSync } from "node:fs";
import registry from "./i18n/locales.json";
import { services } from "./services";

const locales = registry.locales.map((locale) => {
  const messages: Record<string, string> = JSON.parse(
    readFileSync(
      new URL(`./i18n/messages/${locale.lang}.json`, import.meta.url),
      "utf8",
    ),
  );
  const prefix = locale.lang === registry.sourceLang ? "" : `/${locale.lang}`;
  return {
    ...locale,
    title: "Delino Docs",
    description: messages.description,
    nav: [
      { text: messages.guide, link: `${prefix}/guide/getting-started` },
      { text: messages.services, link: `${prefix}/services/` },
      { text: "Delino", link: `https://delino.io${prefix}/` },
    ],
    sidebar: {
      [`${prefix}/guide/`]: [
        {
          text: messages.guide,
          items: [
            {
              text: messages.gettingStarted,
              link: `${prefix}/guide/getting-started`,
            },
          ],
        },
      ],
      ...Object.fromEntries(
        services.map(({ id, pages }) => [
          `${prefix}/services/${id}/`,
          [
            { text: messages.overview, link: `${prefix}/services/${id}/` },
            ...pages.map(({ slug, message }) => ({
              text: messages[message],
              link: `${prefix}/services/${id}/${slug}`,
            })),
          ],
        ]),
      ),
      [`${prefix}/services/`]: [
        {
          text: messages.services,
          items: [
            { text: messages.overview, link: `${prefix}/services/` },
            ...services.map(({ id, name }) => ({
              text: name,
              link: `${prefix}/services/${id}/`,
            })),
          ],
        },
      ],
    },
  };
});

const themeText: Record<string, Record<string, string>> = {};
for (const locale of registry.locales) {
  const messages: Record<string, string> = JSON.parse(
    readFileSync(
      new URL(`./i18n/messages/${locale.lang}.json`, import.meta.url),
      "utf8",
    ),
  );
  for (const [key, value] of Object.entries(messages)) {
    if (!key.startsWith("theme.")) continue;
    (themeText[key.slice(6)] ??= {})[locale.lang] = value;
  }
}

export default defineConfig({
  root: "docs",
  outDir: "dist",
  title: "Delino Docs",
  description: "Documentation for Delino services.",
  siteOrigin: "https://docs.delino.io",
  base: "/",
  lang: registry.sourceLang,
  locales: locales.map(({ lang, label, title, description }) => ({
    lang,
    label,
    title,
    description,
  })),
  route: { localeRedirect: "never" },
  i18nSource: (defaults) => {
    for (const key of Object.keys(defaults)) {
      if (registry.locales.some(({ lang }) => !themeText[key]?.[lang])) {
        throw new Error(`Missing theme translation: theme.${key}`);
      }
    }
    for (const [key, translations] of Object.entries(themeText))
      defaults[key] = { ...defaults[key], ...translations };
    return defaults;
  },
  builderConfig: {
    server: { host: "127.0.0.1", port: 46315, strictPort: true },
  },
  themeConfig: { locales },
});
