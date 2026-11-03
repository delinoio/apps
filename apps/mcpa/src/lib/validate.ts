// SPDX-License-Identifier: Apache-2.0

// Server limits from the Managed CPA API contract, measured in UTF-8 bytes.
export const NAME_MAX_BYTES = 100;
export const DESCRIPTION_MAX_BYTES = 1000;
export const ALLOWLIST_MAX_ENTRIES = 200;
export const MODEL_NAME_MAX_BYTES = 200;

export const byteLength = (value: string) =>
  new TextEncoder().encode(value).length;

export function nameError(value: string, what = "a name"): string | undefined {
  const trimmed = value.trim();
  if (!trimmed) return `Enter ${what}.`;
  if (byteLength(trimmed) > NAME_MAX_BYTES)
    return "Use a shorter name (up to 100 bytes).";
  return undefined;
}

export function descriptionError(value: string): string | undefined {
  return byteLength(value.trim()) > DESCRIPTION_MAX_BYTES
    ? "Use a shorter description (up to 1,000 bytes)."
    : undefined;
}

/** Validates one model name like the server: 1-200 bytes, no spaces or controls. */
export function modelNameError(value: string): string | undefined {
  const model = value.trim();
  if (!model) return "Enter a model name.";
  if (byteLength(model) > MODEL_NAME_MAX_BYTES)
    return "Use a shorter model name (up to 200 bytes).";
  if (/[\s\u0000-\u001f\u007f-\u009f]/u.test(model))
    return "A model name can't contain spaces or control characters.";
  return undefined;
}

/** Requires an https URL: the server rejects plain http upstream URLs. */
export function isHttpsUrl(value: string): boolean {
  try {
    return new URL(value.trim()).protocol === "https:";
  } catch {
    return false;
  }
}

/** Splits a comma- or line-separated list and removes duplicates. */
export function parseList(value: string): string[] {
  return [
    ...new Set(
      value
        .split(/[\n,]/)
        .map((item) => item.trim())
        .filter(Boolean),
    ),
  ];
}
