// SPDX-License-Identifier: Apache-2.0
export enum ServiceId {
  Delcore = "delcore",
  DevBase = "devbase",
  GitTracker = "git-tracker",
  FixKey = "fixkey",
  DeliTools = "delitools",
}

export const services = [
  { id: ServiceId.Delcore, name: "Delcore" },
  { id: ServiceId.DevBase, name: "DevBase" },
  { id: ServiceId.GitTracker, name: "Git Tracker" },
  { id: ServiceId.FixKey, name: "FixKey" },
  { id: ServiceId.DeliTools, name: "DeliTools" },
] as const;
