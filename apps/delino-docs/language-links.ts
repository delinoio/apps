// SPDX-License-Identifier: Apache-2.0
import registry from "./i18n/locales.json" with { type: "json" };

const sourceOnlyRoutes = new Set(
  registry.sourceOnly.pages.map((path) => normalizeRoute(`/${path}`)),
);

function normalizeRoute(path: string): string {
  return path
    .split(/[?#]/u)[0]
    .replace(/\.(?:md|html)$/u, "")
    .replace(/\/index$/u, "/")
    .replace(/\/$/u, "");
}

export function languageLink(
  href: string | undefined,
  language?: string,
  rel?: string,
) {
  if (
    !href ||
    rel !== "alternate" ||
    !language ||
    language === registry.sourceLang ||
    !registry.locales.some(({ lang }) => lang === language)
  )
    return { href, rel };
  const prefix = `/${language}`;
  if (
    href.startsWith(`${prefix}/`) &&
    sourceOnlyRoutes.has(normalizeRoute(href.slice(prefix.length)))
  ) {
    // Rspress assumes every page has a translation. Both its desktop and mobile
    // menus use the theme Link export. Keep their controls, but send readers to
    // the selected language's service directory when the page is English-only.
    // Remove this fallback when Rspress supports per-page language availability.
    return { href: `${prefix}/services/`, rel: undefined };
  }
  return { href, rel };
}
