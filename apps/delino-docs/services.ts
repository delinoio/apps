// SPDX-License-Identifier: Apache-2.0
import registry from "./i18n/locales.json";

export enum ServiceId {
  Delcore = "delcore",
  DevBase = "devbase",
  GitTracker = "git-tracker",
  DeliTools = "delitools",
  Perfine = "perfine",
}

// Pages after a service overview. `slug` is the route segment below
// `/services/<service-id>/` and `message` is the navigation text key in the
// language catalogs under `i18n/messages`.
export enum ServicePage {
  Setup = "setup",
  Cli = "cli",
  Adapters = "adapters",
  Policies = "policies",
  Hosting = "hosting",
  Publication = "publication",
  GithubActions = "github-actions",
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
  {
    id: ServiceId.Perfine,
    name: "Perfine",
    pages: [
      { slug: ServicePage.Setup, message: "perfineSetup" },
      { slug: ServicePage.Cli, message: "perfineCli" },
      { slug: ServicePage.Adapters, message: "perfineAdapters" },
      { slug: ServicePage.Policies, message: "perfinePolicies" },
      { slug: ServicePage.Hosting, message: "perfineHosting" },
      { slug: ServicePage.Publication, message: "perfinePublication" },
      { slug: ServicePage.GithubActions, message: "perfineGithubActions" },
      { slug: ServicePage.Troubleshooting, message: "troubleshooting" },
    ],
  },
  { id: ServiceId.Delcore, name: "Delcore", pages: [] },
  { id: ServiceId.DevBase, name: "DevBase", pages: [] },
  { id: ServiceId.GitTracker, name: "Git Tracker", pages: [] },
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

export function servicesForLanguage(language: string): readonly Service[] {
  return services.filter(
    (service) =>
      language === registry.sourceLang ||
      !registry.sourceOnly.pages.includes(`services/${service.id}/index.md`),
  );
}
