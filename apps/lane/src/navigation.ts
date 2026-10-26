// SPDX-License-Identifier: Apache-2.0
export enum LanePage {
  Tasks = "Tasks",
  Proposals = "Proposals",
  Repositories = "Repositories",
  Events = "Events",
  Settings = "Settings",
}

export enum DestinationKind {
  Page = "Pages",
  App = "Apps",
  Repository = "Repositories",
}

export interface Destination {
  id: string;
  label: string;
  kind: DestinationKind;
  target: string;
  context: string;
}

export function destinations(
  repositories: { id: string; name: string; url: string }[],
): Destination[] {
  return [
    ...Object.values(LanePage).map((page) => ({
      id: `page-${page}`,
      label: page === LanePage.Tasks ? "All tasks" : page,
      kind: DestinationKind.Page,
      target: page,
      context: "Lane",
    })),
    ...["Lane", "Pace", "RealQA"].map((app) => ({
      id: `app-${app}`,
      label: app,
      kind: DestinationKind.App,
      target: app,
      context: "App",
    })),
    ...repositories.map((repository) => ({
      id: `repo-${repository.id}`,
      label: repository.name,
      kind: DestinationKind.Repository,
      target: repository.id,
      context: repository.url,
    })),
  ];
}

export function filterDestinations(items: Destination[], query: string) {
  const words = query.trim().toLowerCase().split(/\s+/);
  return items.filter((item) => {
    const text = `${item.label} ${item.context}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
}

export function nextDestination(index: number, delta: number, count: number) {
  return count ? (index + delta + count) % count : 0;
}
