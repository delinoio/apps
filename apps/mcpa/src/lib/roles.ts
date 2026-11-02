// SPDX-License-Identifier: Apache-2.0
import { ResourceRole } from "@repo/protos/mcpa/v1/mcpa_pb";

export const canManage = (role: ResourceRole) => role === ResourceRole.MANAGE;
export const canInvoke = (role: ResourceRole) =>
  role === ResourceRole.MANAGE || role === ResourceRole.INVOKE;

/** Resource detail sections, in display order. */
export enum Section {
  Endpoints = "endpoints",
  Keys = "keys",
  Credentials = "credentials",
  Routing = "routing",
  Access = "access",
  Budget = "budget",
  Usage = "usage",
  Errors = "errors",
  Audit = "audit",
  Settings = "settings",
}

export const sectionLabels: Record<Section, string> = {
  [Section.Endpoints]: "Endpoints",
  [Section.Keys]: "Call keys",
  [Section.Credentials]: "Credentials",
  [Section.Routing]: "Routing",
  [Section.Access]: "Access",
  [Section.Budget]: "Budget",
  [Section.Usage]: "Usage",
  [Section.Errors]: "Errors",
  [Section.Audit]: "Audit log",
  [Section.Settings]: "Settings",
};

const invokeSections = [Section.Endpoints, Section.Keys, Section.Usage];

/** Shows only the sections that the caller's resource role permits. */
export function sectionsForRole(role: ResourceRole): Section[] {
  if (canManage(role)) return Object.values(Section);
  if (canInvoke(role)) return invokeSections;
  return [Section.Endpoints];
}
