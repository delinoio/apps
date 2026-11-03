// SPDX-License-Identifier: Apache-2.0
import { timestampDate, type Timestamp } from "@bufbuild/protobuf/wkt";

/** Managed CPA traffic price: USD 0.07 per GiB (2^30 bytes). */
export const MICRO_USD_PER_GIB = 70_000n;
export const GIB = 1n << 30n;
export const PRICE_LABEL = "USD 0.07 per GiB";

function group(digits: string): string {
  return digits.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** Rounds a nonnegative decimal string half up to `places` digits, scaled. */
export function roundDecimal(
  value: string,
  places: number,
): bigint | undefined {
  const match = /^(\d+)(?:\.(\d+))?$/.exec(value.trim());
  if (!match) return undefined;
  const fraction = match[2] ?? "";
  const kept = fraction.slice(0, places).padEnd(places, "0");
  let scaled = BigInt(match[1] + kept);
  if ((fraction[places] ?? "0") >= "5") scaled += 1n;
  return scaled;
}

function formatScaled(
  scaled: bigint,
  places: number,
  minPlaces = places,
): string {
  const text = scaled.toString().padStart(places + 1, "0");
  const whole = text.slice(0, text.length - places);
  let fraction = text.slice(text.length - places);
  while (fraction.length > minPlaces && fraction.endsWith("0"))
    fraction = fraction.slice(0, -1);
  return fraction ? `${group(whole)}.${fraction}` : group(whole);
}

/**
 * Formats a USD amount with two to six fraction digits, rounded half up, for
 * example "$1,234.50" or "$0.000065".
 */
function formatMicroUsd(micro: bigint): string {
  return `$${formatScaled(micro, 6, 2)}`;
}

/** Formats a canonical USD decimal string from the API. */
export function formatUsd(value: string | undefined): string {
  if (!value) return formatMicroUsd(0n);
  const micro = roundDecimal(value, 6);
  return micro === undefined ? value : formatMicroUsd(micro);
}

/** Estimates USD for measured bytes at USD 0.07 per GiB. */
export function estimateUsdFromBytes(bytes: bigint): string {
  if (bytes <= 0n) return formatMicroUsd(0n);
  return formatMicroUsd((bytes * MICRO_USD_PER_GIB + GIB / 2n) / GIB);
}

/** Formats a canonical GiB decimal string with three decimals. */
export function formatGib(value: string | undefined): string {
  if (!value) return "0.000 GiB";
  const thousandths = roundDecimal(value, 3);
  if (thousandths === undefined) return `${value} GiB`;
  if (thousandths === 0n && /[1-9]/.test(value)) return "< 0.001 GiB";
  return `${formatScaled(thousandths, 3)} GiB`;
}

const byteUnits = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/** Formats a byte count with binary units, for example "1.5 MiB". */
export function formatBytes(bytes: bigint): string {
  if (bytes < 1024n) return `${bytes} B`;
  let unit = 0;
  let value = Number(bytes);
  while (value >= 1024 && unit < byteUnits.length - 1) {
    value /= 1024;
    unit += 1;
  }
  // Round to the display precision first: one decimal below 100, none from
  // 100. A value such as 1023.99 KiB rounds to 1024, which belongs to the
  // next unit, so promote it instead of printing "1024 KiB".
  const round = (amount: number) => {
    const tenths = Number(amount.toFixed(1));
    return tenths >= 100 ? Number(amount.toFixed(0)) : tenths;
  };
  while (round(value) >= 1024 && unit < byteUnits.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = round(value);
  return `${rounded >= 100 ? rounded.toFixed(0) : rounded.toFixed(1)} ${byteUnits[unit]}`;
}

export function formatCount(value: bigint): string {
  return group(value.toString());
}

/** Server limit pattern: up to 12 integer digits and 6 fraction digits. */
const USD_INPUT = /^\s*\$?\s*(\d{1,12})(?:\.(\d{0,6}))?\s*$/;

/**
 * Normalizes a USD amount typed by a person into the canonical decimal form
 * the API expects ("10.50" → "10.5"). Accepts up to 12 integer and 6 fraction
 * digits like the API. Returns undefined when it is invalid.
 */
export function normalizeUsdInput(input: string): string | undefined {
  const match = USD_INPUT.exec(input);
  if (!match) return undefined;
  const whole = match[1].replace(/^0+(?=\d)/, "");
  const fraction = (match[2] ?? "").replace(/0+$/, "");
  return fraction ? `${whole}.${fraction}` : whole;
}

const dateTime = new Intl.DateTimeFormat("en-US", {
  dateStyle: "medium",
  timeStyle: "short",
});
const day = new Intl.DateTimeFormat("en-US", {
  dateStyle: "medium",
  timeZone: "UTC",
});

export function toDate(value: Timestamp | undefined): Date | undefined {
  return value ? timestampDate(value) : undefined;
}

export function formatDateTime(
  value: Timestamp | undefined,
  empty = "—",
): string {
  const date = toDate(value);
  return date ? dateTime.format(date) : empty;
}

export function formatDay(value: Timestamp | undefined): string {
  const date = toDate(value);
  return date ? day.format(date) : "—";
}
