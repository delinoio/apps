// SPDX-License-Identifier: Apache-2.0

const ACTIVE_TENANT_KEY = "mcpa.activeTenant";

/** The only console state kept in local storage: the active tenant ID. */
export function readActiveTenant(storage: Storage = localStorage): string {
  try {
    return storage.getItem(ACTIVE_TENANT_KEY) ?? "";
  } catch {
    return "";
  }
}

export function writeActiveTenant(
  tenantId: string,
  storage: Storage = localStorage,
) {
  try {
    if (tenantId) storage.setItem(ACTIVE_TENANT_KEY, tenantId);
    else storage.removeItem(ACTIVE_TENANT_KEY);
  } catch {
    // Storage can be unavailable in private modes; selection then lasts only
    // for the current page.
  }
}
