// SPDX-License-Identifier: Apache-2.0
import { timestampDate, type Timestamp } from "@bufbuild/protobuf/wkt";
import {
  LocalTool,
  Provider,
  type Money,
} from "@repo/protos/anydash/v1/anydash_pb";

export const providerName = (provider: Provider) =>
  pick(
    {
      [Provider.CODEX]: "Codex",
      [Provider.CLAUDE_CODE]: "Claude Code",
      [Provider.GROK]: "Grok",
      [Provider.VERCEL]: "Vercel",
      [Provider.SUPABASE]: "Supabase",
      [Provider.CLOUDFLARE]: "Cloudflare",
    },
    provider,
    "This provider",
  );

/** CLI name shown in local-login authentication guidance. */
export const providerCli = (provider: Provider) =>
  ({
    [Provider.CODEX]: "codex",
    [Provider.CLAUDE_CODE]: "claude",
    [Provider.GROK]: "grok",
  })[provider as Provider.CODEX];

export const usesLocalLogin = (provider: Provider) =>
  provider === Provider.CODEX ||
  provider === Provider.CLAUDE_CODE ||
  provider === Provider.GROK;

export const toolName = (tool: LocalTool) =>
  tool === LocalTool.CODEX
    ? "Codex"
    : tool === LocalTool.CLAUDE_CODE
      ? "Claude Code"
      : "Local tool";

const dimensionLabels: Record<string, string> = {
  five_hour: "5-hour limit",
  weekly: "Weekly limit",
  seven_day: "Weekly limit",
  seven_day_opus: "Weekly limit (Opus)",
  seven_day_sonnet: "Weekly limit (Sonnet)",
  credits: "Credits",
};
export function dimensionLabel(dimension: string) {
  if (dimensionLabels[dimension]) return dimensionLabels[dimension];
  const words = dimension.replace(/[_-]+/g, " ").trim();
  return words ? words[0].toUpperCase() + words.slice(1) : "Total";
}

const hasIntl = () =>
  typeof Intl !== "undefined" && typeof Intl.DateTimeFormat === "function";

function parts(date: Date, timeZone: string) {
  if (hasIntl()) {
    try {
      const values: Record<string, string> = {};
      for (const part of new Intl.DateTimeFormat("en-US", {
        timeZone: timeZone || "UTC",
        year: "numeric",
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
        hourCycle: "h23",
      }).formatToParts(date))
        values[part.type] = part.value;
      return {
        month: values.month,
        day: values.day,
        hour: values.hour === "24" ? "00" : values.hour,
        minute: values.minute,
        dateKey: `${values.year}-${values.month}-${values.day}`,
      };
    } catch {
      /* An unknown zone falls back to UTC below. */
    }
  }
  // Hosts without Intl show UTC; the header still names the selected zone.
  const months = [
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
  ];
  const pad = (value: number) => String(value).padStart(2, "0");
  return {
    month: months[date.getUTCMonth()],
    day: String(date.getUTCDate()),
    hour: pad(date.getUTCHours()),
    minute: pad(date.getUTCMinutes()),
    dateKey: `${date.getUTCFullYear()}-${date.getUTCMonth()}-${date.getUTCDate()}`,
  };
}

export type TimeInput = Timestamp | Date | undefined;
const asDate = (value: TimeInput) =>
  value === undefined
    ? undefined
    : value instanceof Date
      ? value
      : timestampDate(value);

/** "12:41" on the same local day as now, otherwise "Oct 31, 18:05". */
export function formatTime(
  value: TimeInput,
  timeZone: string,
  now = new Date(),
) {
  const date = asDate(value);
  if (!date) return "";
  const local = parts(date, timeZone);
  const time = `${local.hour}:${local.minute}`;
  return local.dateKey === parts(now, timeZone).dateKey
    ? time
    : `${local.month} ${local.day}, ${time}`;
}

/** Always "Oct 31, 18:05", for lists such as the activity log. */
export function formatDateTime(value: TimeInput, timeZone: string) {
  const date = asDate(value);
  if (!date) return "";
  const local = parts(date, timeZone);
  return `${local.month} ${local.day}, ${local.hour}:${local.minute}`;
}

export function formatPercent(value: number) {
  if (!Number.isFinite(value)) return "0";
  const clamped = Math.max(0, Math.min(100, value));
  return Number.isInteger(clamped)
    ? String(clamped)
    : String(Math.round(clamped));
}

const groupThousands = (integer: string) =>
  integer.replace(/\B(?=(\d{3})+(?!\d))/g, ",");

/** Decimal strings stay exact; only display rounding is applied. */
export function formatDecimal(amount: string, fraction = 2) {
  const match = /^(-?)(\d+)(?:\.(\d+))?$/.exec(amount.trim());
  if (!match) return amount;
  const [, sign, whole, rest = ""] = match;
  let digits = (rest + "0".repeat(fraction)).slice(0, fraction);
  let integer = whole;
  // Round half up on the first dropped digit.
  if (Number(rest[fraction] ?? "0") >= 5) {
    const scaled = (BigInt(integer + digits) + 1n).toString();
    const padded = scaled.padStart(fraction + 1, "0");
    integer = padded.slice(0, padded.length - fraction) || "0";
    digits = padded.slice(padded.length - fraction);
  }
  return `${sign}${groupThousands(integer)}${fraction ? `.${digits}` : ""}`;
}

export function formatMoney(money: Money | undefined) {
  if (!money) return "";
  const amount = formatDecimal(money.amount);
  if (money.currencyCode === "USD")
    return amount.startsWith("-") ? `-$${amount.slice(1)}` : `$${amount}`;
  return money.currencyCode ? `${amount} ${money.currencyCode}` : amount;
}

export function formatCount(value: bigint | number) {
  return groupThousands(value.toString());
}

/** 48,200,000 → "48.2M". */
export function formatCompact(value: bigint | number) {
  const number = Number(value);
  const units: [number, string][] = [
    [1e12, "T"],
    [1e9, "B"],
    [1e6, "M"],
    [1e3, "K"],
  ];
  for (const [size, unit] of units) {
    if (Math.abs(number) >= size) {
      const scaled = number / size;
      const text = scaled >= 100 ? scaled.toFixed(0) : scaled.toFixed(1);
      return `${text.replace(/\.0$/, "")}${unit}`;
    }
  }
  return formatCount(value);
}

export function formatCountdown(milliseconds: number) {
  const total = Math.max(0, Math.ceil(milliseconds / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/** Typed enum-to-label lookup with a fallback for unknown or zero values. */
export function pick<K extends number, V>(
  table: Partial<Record<K, V>>,
  key: K,
  fallback: V,
): V {
  return table[key] ?? fallback;
}
