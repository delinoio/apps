// SPDX-License-Identifier: Apache-2.0
// Lynx isolates do not guarantee DOM, Buffer or btoa APIs.
export function attachmentSource(
  bytes: readonly number[],
  downloadUrl: string,
): string | undefined {
  if (bytes.length) {
    const alphabet =
      "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const chunks: string[] = [];
    for (let i = 0; i < bytes.length; i += 3) {
      const value =
        (bytes[i] << 16) | ((bytes[i + 1] || 0) << 8) | (bytes[i + 2] || 0);
      chunks.push(
        alphabet[(value >>> 18) & 63] +
          alphabet[(value >>> 12) & 63] +
          (i + 1 < bytes.length ? alphabet[(value >>> 6) & 63] : "=") +
          (i + 2 < bytes.length ? alphabet[value & 63] : "="),
      );
    }
    return `data:image/png;base64,${chunks.join("")}`;
  }
  return /^https:\/\//i.test(downloadUrl) ? downloadUrl : undefined;
}
export function decodePNG(base64: string): number[] {
  const alphabet =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  if (
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
      base64,
    )
  )
    throw new Error("Invalid capture bytes.");
  const bytes: number[] = [];
  for (let i = 0; i < base64.length; i += 4) {
    const value =
      (alphabet.indexOf(base64[i]) << 18) |
      (alphabet.indexOf(base64[i + 1]) << 12) |
      ((alphabet.indexOf(base64[i + 2]) & 63) << 6) |
      (alphabet.indexOf(base64[i + 3]) & 63);
    bytes.push((value >>> 16) & 255);
    if (base64[i + 2] !== "=") bytes.push((value >>> 8) & 255);
    if (base64[i + 3] !== "=") bytes.push(value & 255);
  }
  return bytes;
}
// Rules arrive normalized from the domain store. Preserve path case and segment boundaries.
export function matchingRepository(
  url: string | undefined,
  rules: readonly { prefix: string; repositoryId: string }[],
): string | undefined {
  if (!url) return;
  const match = /^(https?):\/\/([^/?#]+)([^?#]*)/i.exec(url);
  if (!match || match[2].includes("@") || /[\\\s]/.test(url)) return;
  const scheme = match[1].toLowerCase();
  const host = match[2]
    .toLowerCase()
    .replace(scheme === "https" ? /:443$/ : /:80$/, "");
  const normalized = `${scheme}://${host}${match[3]}`.replace(/\/+$/, "");
  return rules
    .filter((rule) => {
      // The domain store retains '/' at an origin root. Compare both sides
      // without terminal slashes so root rules also match descendant paths.
      const prefix = rule.prefix.replace(/\/+$/, "");
      return normalized === prefix || normalized.startsWith(`${prefix}/`);
    })
    .sort((a, b) => b.prefix.length - a.prefix.length)[0]?.repositoryId;
}

export function captureValidation(
  draft:
    | { repositoryId: string; title: string; images: readonly unknown[] }
    | null
    | undefined,
  repositoryIds: readonly string[],
) {
  return {
    repository:
      !draft || !repositoryIds.includes(draft.repositoryId)
        ? "Select a repository. A deleted or conflicted repository must be reselected."
        : "",
    title: !draft?.title.trim() ? "Enter a title." : "",
    images: !draft?.images.length ? "Add at least one capture." : "",
  };
}
export function savedCaptureRoute(saved: { taskId: string; spaceId?: string }) {
  if (!saved.taskId?.trim() || !saved.spaceId?.trim())
    throw new Error(
      "The saved task's data space is missing. Reopen Lane to locate the task; navigation was stopped.",
    );
  return { kind: "task" as const, id: saved.taskId, spaceId: saved.spaceId };
}
