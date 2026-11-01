// SPDX-License-Identifier: Apache-2.0

// Server limits from the Managed CPA API contract, measured in UTF-8 bytes.
export const NAME_MAX_BYTES = 100;
export const DESCRIPTION_MAX_BYTES = 1000;
export const ALLOWLIST_MAX_ENTRIES = 200;

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

export function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value.trim());
    return url.protocol === "https:" || url.protocol === "http:";
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
