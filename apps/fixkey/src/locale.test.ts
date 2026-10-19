import { describe, expect, it } from "vitest";
import { messages, resolveLocale, isRtl } from "./locale";
import languages from "./languages.json";
import engineCatalog from "../../../crates/fixkey-core/assets/languages/catalog.json";

function shape(value: unknown): unknown {
  if (typeof value === "string") {
    expect(value.trim()).not.toBe("");
    return "string";
  }
  if (Array.isArray(value)) return value.map(shape);
  return Object.fromEntries(
    Object.entries(value as object).map(([key, child]) => [key, shape(child)]),
  );
}

describe("bundled language contracts", () => {
  it("keeps every translated message and all ten full help topics", () => {
    const expected = shape(messages.en);
    expect(Object.keys(messages)).toHaveLength(60);
    for (const translation of Object.values(messages)) {
      expect(shape(translation)).toEqual(expected);
      expect(translation.helpTopics).toHaveLength(10);
    }
  });

  it("resolves regional, script and POSIX locales before English fallback", () => {
    expect(resolveLocale("pt_PT.UTF-8")).toBe("pt-PT");
    expect(resolveLocale("sr-Latn-RS")).toBe("sr-Latn");
    expect(resolveLocale("de-AT")).toBe("de");
    expect(resolveLocale("ko_KR")).toBe("ko");
    expect(resolveLocale("unknown")).toBe("en");
    expect(resolveLocale("tlh-Latn")).toBe("en");
    expect(isRtl("he-IL")).toBe(true);
    expect(isRtl("fa_IR")).toBe(true);
    expect(isRtl("en")).toBe(false);
  });

  it("lists exactly the same offline dictionary and keyboard pairs as the engine", () => {
    expect(languages).toEqual(engineCatalog.languages);
    expect(new Set(languages.map((entry) => entry.id)).size).toBe(89);
    expect(
      languages.filter((entry) => entry.validation === "listed-words"),
    ).toHaveLength(9);
  });
});
