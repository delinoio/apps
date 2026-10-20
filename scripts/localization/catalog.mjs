import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import yaml from "js-yaml";

export const fingerprint = (text) =>
  createHash("sha256")
    .update(text.replace(/\r\n/gu, "\n").trim())
    .digest("hex");
export const json = async (path) => JSON.parse(await readFile(path, "utf8"));
export async function optionalJson(path, fallback = {}) {
  try {
    return await json(path);
  } catch (error) {
    if (error.code === "ENOENT") return fallback;
    throw error;
  }
}
export async function writeJson(path, data) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, `${JSON.stringify(data, null, 2)}\n`);
}

export async function files(directory) {
  try {
    const entries = await readdir(directory, { withFileTypes: true });
    const nested = await Promise.all(
      entries.map(async (entry) => {
        if (entry.isSymbolicLink())
          throw new Error("Translation inputs must not be symbolic links.");
        return entry.isDirectory()
          ? (await files(join(directory, entry.name))).map(
              (path) => `${entry.name}/${path}`,
            )
          : [entry.name];
      }),
    );
    return nested.flat().sort();
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
}

function strings(value, path = "") {
  if (typeof value === "string") return [[path, value]];
  if (!value || typeof value !== "object") return [[path, value]];
  return Object.entries(value).flatMap(([key, item]) =>
    strings(item, `${path}/${key}`),
  );
}

function frontmatter(text) {
  const match = text.match(/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/u);
  return {
    data: match ? yaml.load(match[1]) : null,
    body: match ? text.slice(match[0].length) : text,
  };
}

// These are content fields in Rspress frontmatter. All other values are structural.
const TEXT_FIELD = /\/(?:title|description|name|text|tagline|footer)$/u;
export function validateTranslation(source, target, markdown = false) {
  if (typeof target !== "string" || !target.trim())
    throw new Error("Translation must be a nonempty string.");
  const captures = (text, expression) =>
    [...text.matchAll(expression)].map((match) => match[0]).sort();
  const same = (left, right, kind) => {
    if (JSON.stringify(left) !== JSON.stringify(right))
      throw new Error(`Translation changed protected ${kind}.`);
  };
  same(
    captures(source, /\{\{[^{}]+\}\}|\{[A-Za-z_][\w.]*\}/gu),
    captures(target, /\{\{[^{}]+\}\}|\{[A-Za-z_][\w.]*\}/gu),
    "placeholders",
  );
  if (!markdown) return;
  const original = frontmatter(source);
  const translated = frontmatter(target);
  const sourceFields = strings(original.data);
  const targetFields = strings(translated.data);
  same(
    sourceFields.map(([key]) => key),
    targetFields.map(([key]) => key),
    "frontmatter fields",
  );
  same(
    sourceFields.filter(([key]) => !TEXT_FIELD.test(key)),
    targetFields.filter(([key]) => !TEXT_FIELD.test(key)),
    "frontmatter values",
  );
  for (const [key, value] of sourceFields.filter(([key]) =>
    TEXT_FIELD.test(key),
  )) {
    const next = targetFields.find(([field]) => field === key)?.[1];
    if (
      typeof value !== typeof next ||
      (typeof next === "string" && !next.trim())
    )
      throw new Error("Invalid translated frontmatter text.");
  }
  same(
    captures(original.body, /^(`{3,}|~{3,})[^\n]*\n[\s\S]*?^\1\s*$/gmu),
    captures(translated.body, /^(`{3,}|~{3,})[^\n]*\n[\s\S]*?^\1\s*$/gmu),
    "code blocks",
  );
  same(
    captures(original.body, /`+[^`\n]+`+/gu),
    captures(translated.body, /`+[^`\n]+`+/gu),
    "code spans",
  );
  same(
    captures(
      original.body,
      /\]\([^\n)]*\)|^\[[^\]\n]+\]:\s*\S+|<https?:\/\/[^>]+>/gmu,
    ),
    captures(
      translated.body,
      /\]\([^\n)]*\)|^\[[^\]\n]+\]:\s*\S+|<https?:\/\/[^>]+>/gmu,
    ),
    "links",
  );
  same(
    [...original.body.matchAll(/^(#{1,6})\s/gmu)].map((match) => match[1]),
    [...translated.body.matchAll(/^(#{1,6})\s/gmu)].map((match) => match[1]),
    "heading structure",
  );
}

export function localizeLinks(text, lang, sourceLang) {
  if (lang === sourceLang) return text;
  const prefix = `/${lang}`;
  // Example URLs inside code are literal content, even when they look like Markdown links.
  const protectedCode = [];
  const masked = text.replace(
    /^(`{3,}|~{3,})[^\n]*\n[\s\S]*?^\1\s*$|`+[^`\n]+`+/gmu,
    (code) => {
      const index = protectedCode.push(code) - 1;
      return `\u0000CODE${index}\u0000`;
    },
  );
  return masked
    .replace(/(\]\()\/(?!\/)/gu, `$1${prefix}/`)
    .replace(/(^\s*(?:-\s*)?link:\s*["']?)\/(?!\/)/gmu, `$1${prefix}/`)
    .replace(/(^\[[^\]\n]+\]:\s*)\/(?!\/)/gmu, `$1${prefix}/`)
    .replace(
      /\u0000CODE(\d+)\u0000/gu,
      (_token, index) => protectedCode[Number(index)],
    );
}

export async function loadProject(root) {
  const registry = await json(join(root, "i18n/locales.json"));
  const { sourceLang, locales } = registry;
  if (
    !Array.isArray(locales) ||
    !locales.length ||
    !locales.some((locale) => locale.lang === sourceLang)
  )
    throw new Error("Invalid source language registry.");
  const seen = new Set();
  for (const locale of locales) {
    if (
      !/^[a-z]{2,3}(?:-[A-Za-z0-9]{2,8})*$/u.test(locale.lang) ||
      !locale.label?.trim() ||
      seen.has(locale.lang)
    )
      throw new Error("Invalid or duplicate language registry entry.");
    seen.add(locale.lang);
  }
  const messages = await json(join(root, `i18n/messages/${sourceLang}.json`));
  if (
    !messages ||
    Array.isArray(messages) ||
    typeof messages !== "object" ||
    !Object.keys(messages).length
  )
    throw new Error("Source catalog must be a nonempty object.");
  const units = Object.entries(messages).map(([key, text]) => {
    if (
      !/^[A-Za-z][\w.]*$/u.test(key) ||
      typeof text !== "string" ||
      !text.trim()
    )
      throw new Error(
        "Source catalog must contain stable keys and nonempty string values.",
      );
    return { id: `messages/${key}`, key, text, markdown: false };
  });
  for (const path of await files(join(root, "docs", sourceLang))) {
    if (path.endsWith(".mdx"))
      throw new Error(
        "Automated documentation translation accepts .md pages only; MDX needs an explicit translation contract.",
      );
    if (!path.endsWith(".md")) continue;
    units.push({
      id: `docs/${path}`,
      path,
      text: await readFile(join(root, "docs", sourceLang, path), "utf8"),
      markdown: true,
    });
  }
  const state = await optionalJson(join(root, "i18n/state.json"));
  return { root, sourceLang, locales, units, state };
}

async function output(project, locale) {
  const messages = await optionalJson(
    join(project.root, `i18n/messages/${locale.lang}.json`),
  );
  const values = {};
  for (const unit of project.units) {
    if (!unit.markdown) {
      values[unit.id] = messages[unit.key];
      continue;
    }
    try {
      values[unit.id] = await readFile(
        join(project.root, "docs", locale.lang, unit.path),
        "utf8",
      );
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  return { messages, values };
}

export async function inspect(project) {
  const issues = [];
  const pending = [];
  for (const locale of project.locales.filter(
    (locale) => locale.lang !== project.sourceLang,
  )) {
    const { messages, values } = await output(project, locale);
    const known = new Set(project.units.map((unit) => unit.id));
    const extra = [
      ...Object.keys(messages).map((key) => `messages/${key}`),
      ...(await files(join(project.root, "docs", locale.lang)))
        .filter((path) => path.endsWith(".md"))
        .map((path) => `docs/${path}`),
      ...Object.keys(project.state[locale.lang] ?? {}),
    ].filter((id) => !known.has(id));
    for (const id of new Set(extra))
      issues.push(
        `${locale.lang}: obsolete translation ${id}; remove it and its state entry explicitly.`,
      );
    for (const unit of project.units) {
      const value = values[unit.id];
      let invalid;
      try {
        validateTranslation(
          localizeLinks(unit.text, locale.lang, project.sourceLang),
          value,
          unit.markdown,
        );
      } catch (error) {
        invalid = error.message;
      }
      if (
        invalid ||
        project.state[locale.lang]?.[unit.id] !== fingerprint(unit.text)
      ) {
        pending.push({ locale, unit });
        issues.push(
          `${locale.lang}: ${unit.id}: ${invalid ?? "source changed or translation is untracked"}`,
        );
      }
    }
  }
  return { issues, pending };
}

export async function generate(project, translate, lang) {
  if (
    lang &&
    !project.locales.some(
      (locale) => locale.lang === lang && lang !== project.sourceLang,
    )
  )
    throw new Error("Unknown target language.");
  const { pending, issues } = await inspect(project);
  const obsolete = issues.filter((issue) =>
    issue.includes(": obsolete translation "),
  );
  if (obsolete.length) throw new Error(obsolete.join("\n"));
  let count = 0;
  for (const locale of project.locales.filter(
    (locale) =>
      locale.lang !== project.sourceLang && (!lang || locale.lang === lang),
  )) {
    const needed = pending
      .filter((item) => item.locale.lang === locale.lang)
      .map((item) => item.unit);
    if (!needed.length) continue;
    const { messages } = await output(project, locale);
    // Bound each request: one message batch or one Markdown page. Resume completed batches on rerun.
    const messageUnits = needed.filter((unit) => !unit.markdown);
    const batches = [];
    for (let index = 0; index < messageUnits.length; index += 40)
      batches.push(messageUnits.slice(index, index + 40));
    batches.push(
      ...needed.filter((unit) => unit.markdown).map((unit) => [unit]),
    );
    for (const batch of batches) {
      const input = Object.fromEntries(
        batch.map((unit) => [unit.id, unit.text]),
      );
      const result = await translate(locale, input);
      if (
        !result ||
        Array.isArray(result) ||
        typeof result !== "object" ||
        JSON.stringify(Object.keys(result).sort()) !==
          JSON.stringify(Object.keys(input).sort())
      )
        throw new Error(
          "Translation response keys did not match the requested units.",
        );
      for (const unit of batch)
        validateTranslation(unit.text, result[unit.id], unit.markdown);
      for (const unit of batch) {
        const text = localizeLinks(
          result[unit.id],
          locale.lang,
          project.sourceLang,
        );
        if (unit.markdown) {
          const path = join(project.root, "docs", locale.lang, unit.path);
          await mkdir(dirname(path), { recursive: true });
          await writeFile(path, text.endsWith("\n") ? text : `${text}\n`);
        } else {
          messages[unit.key] = text;
        }
        project.state[locale.lang] ??= {};
        project.state[locale.lang][unit.id] = fingerprint(unit.text);
      }
      await writeJson(
        join(project.root, `i18n/messages/${locale.lang}.json`),
        messages,
      );
      await writeJson(join(project.root, "i18n/state.json"), project.state);
      count += batch.length;
      console.log(
        JSON.stringify({
          event: "translations_updated",
          language: locale.lang,
          units: batch.length,
        }),
      );
    }
  }
  return count;
}
