// SPDX-License-Identifier: Apache-2.0
export enum ServiceId {
  Delcore = "delcore",
  DevBase = "devbase",
  GitTracker = "git-tracker",
  FixKey = "fixkey",
  DeliTools = "delitools",
}

// Pages after a service overview. `slug` is the route segment below
// `/services/<service-id>/` and `message` is the navigation text key in the
// language catalogs under `i18n/messages`.
export enum ServicePage {
  ApiReference = "api-reference",
  ApiKeys = "api-keys",
  Billing = "billing",
  Troubleshooting = "troubleshooting",
}

export interface ServicePageLink {
  slug: ServicePage;
  message: string;
}

export interface Service {
  id: ServiceId;
  name: string;
  pages: readonly ServicePageLink[];
}

export const services: readonly Service[] = [
  { id: ServiceId.Delcore, name: "Delcore", pages: [] },
  { id: ServiceId.DevBase, name: "DevBase", pages: [] },
  { id: ServiceId.GitTracker, name: "Git Tracker", pages: [] },
  { id: ServiceId.FixKey, name: "FixKey", pages: [] },
  {
    id: ServiceId.DeliTools,
    name: "DeliTools",
    pages: [
      { slug: ServicePage.ApiReference, message: "apiReference" },
      { slug: ServicePage.ApiKeys, message: "apiKeys" },
      { slug: ServicePage.Billing, message: "billing" },
      { slug: ServicePage.Troubleshooting, message: "troubleshooting" },
    ],
  },
];
