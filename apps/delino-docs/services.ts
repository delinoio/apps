// SPDX-License-Identifier: Apache-2.0
export enum ServiceId {
  Delcore = "delcore",
  DevBase = "devbase",
  GitTracker = "git-tracker",
  FixKey = "fixkey",
}

export const services = [
  { id: ServiceId.Delcore, name: "Delcore" },
  { id: ServiceId.DevBase, name: "DevBase" },
  { id: ServiceId.GitTracker, name: "Git Tracker" },
  { id: ServiceId.FixKey, name: "FixKey" },
] as const;
