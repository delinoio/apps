// SPDX-License-Identifier: Apache-2.0
/**
 * Structured client logs. Callers pass only safe metadata: event names,
 * method names, error codes and correlation IDs. Never pass tokens, provider
 * data, usage values or user input.
 */
export type SafeFields = Record<string, string | number | boolean | undefined>;

export function logError(event: string, fields: SafeFields = {}) {
  console.error(JSON.stringify({ level: "error", event, ...fields }));
}
export function logInfo(event: string, fields: SafeFields = {}) {
  console.info(JSON.stringify({ level: "info", event, ...fields }));
}
