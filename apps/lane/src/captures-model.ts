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
// RFC 3492 Punycode for one lowercase, NFC-normalized hostname label.
function punycode(label: string): string {
  const input = Array.from(label, (char) => char.codePointAt(0)!);
  const digit = (value: number) =>
    String.fromCharCode(value + (value < 26 ? 97 : 22));
  let output = input
    .filter((code) => code < 0x80)
    .map((code) => String.fromCharCode(code))
    .join("");
  const basic = output.length;
  let handled = basic;
  if (basic) output += "-";
  let n = 0x80;
  let delta = 0;
  let bias = 72;
  const adapt = (value: number, points: number, first: boolean) => {
    value = first ? Math.floor(value / 700) : value >> 1;
    value += Math.floor(value / points);
    let k = 0;
    for (; value > 455; k += 36) value = Math.floor(value / 35);
    return k + Math.floor((36 * value) / (value + 38));
  };
  while (handled < input.length) {
    const next = Math.min(...input.filter((code) => code >= n));
    delta += (next - n) * (handled + 1);
    n = next;
    for (const code of input) {
      if (code < n) delta++;
      if (code !== n) continue;
      let q = delta;
      for (let k = 36; ; k += 36) {
        const t = k <= bias ? 1 : k >= bias + 26 ? 26 : k - bias;
        if (q < t) break;
        output += digit(t + ((q - t) % (36 - t)));
        q = Math.floor((q - t) / (36 - t));
      }
      output += digit(q);
      bias = adapt(delta, handled + 1, handled === basic);
      delta = 0;
      handled++;
    }
    delta++;
    n++;
  }
  return output;
}
// Lynx isolates do not guarantee the WHATWG URL API. This covers the
// serialization steps the domain stores apply to page URLs: host case, IDN
// labels, default and empty ports, dot segments and path percent-encoding.
function normalizePageURL(url: string): string | undefined {
  const match = /^(https?):\/\/([^/?#]*)([^?#]*)/i.exec(url);
  if (!match || match[2].includes("@") || /[\\\s]/.test(url)) return;
  const scheme = match[1].toLowerCase();
  const authority = /^([^:[\]]+|\[[^\]]+\])(?::(\d*))?$/.exec(match[2]);
  if (!authority) return;
  let host = authority[1].toLowerCase();
  if (!host.startsWith("[")) {
    if (host.normalize) host = host.normalize("NFC");
    host = host
      .split(".")
      .map((label) =>
        /[^\x00-\x7f]/.test(label) ? `xn--${punycode(label)}` : label,
      )
      .join(".");
  }
  const portText = authority[2];
  if (portText) {
    const port = Number(portText);
    if (port > 65535) return;
    if (port !== (scheme === "https" ? 443 : 80)) host += `:${port}`;
  }
  const segments: string[] = [];
  const parts = match[3].split("/").slice(1);
  parts.forEach((segment, index) => {
    const last = index === parts.length - 1;
    if (segment === ".." || /^(\.|%2e){2}$/i.test(segment)) {
      segments.pop();
      if (last) segments.push("");
    } else if (segment === "." || /^%2e$/i.test(segment)) {
      if (last) segments.push("");
    } else segments.push(segment);
  });
  const path = Array.from(`/${segments.join("/")}`, (char) => {
    const code = char.codePointAt(0)!;
    return code < 0x21 || code > 0x7e || '"<>`{}'.includes(char)
      ? encodeURIComponent(char)
      : char;
  }).join("");
  return `${scheme}://${host}${path}`.replace(/\/+$/, "");
}
// Rules arrive normalized from the domain store. Preserve path case and segment boundaries.
// The caller excludes conflicted and deleted rules before matching.
export function matchingRepository(
  url: string | undefined,
  rules: readonly { prefix: string; repositoryId: string }[],
): string | undefined {
  if (!url) return;
  const normalized = normalizePageURL(url);
  if (normalized === undefined) return;
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
