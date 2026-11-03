// SPDX-License-Identifier: Apache-2.0
export enum RouteKind {
  Home = "home",
  Dashboard = "dashboard",
  Connect = "connect",
  Settings = "settings",
  NotFound = "not-found",
}

export type Route =
  | { kind: RouteKind.Home }
  | {
      kind: RouteKind.Dashboard;
      spaceId: string;
      dashboardId: string;
      widgetId?: string;
    }
  | { kind: RouteKind.Connect; enrollmentId: string }
  | { kind: RouteKind.Settings }
  | { kind: RouteKind.NotFound };

export const DEEP_LINK_SCHEME = "anydash";
const id = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function splitQuery(path: string) {
  const hash = path.indexOf("#");
  const clean = hash >= 0 ? path.slice(0, hash) : path;
  const mark = clean.indexOf("?");
  const query: Record<string, string> = {};
  if (mark >= 0)
    for (const pair of clean.slice(mark + 1).split("&")) {
      if (!pair) continue;
      const [key, value = ""] = pair.split("=");
      try {
        query[decodeURIComponent(key)] = decodeURIComponent(value);
      } catch {
        /* Malformed escapes are ignored like unknown parameters. */
      }
    }
  return { path: mark >= 0 ? clean.slice(0, mark) : clean, query };
}

/** Parses a web path (`/spaces/...`) or a native deep link (`anydash://...`). */
export function parseRoute(input: string): Route {
  let value = input.trim();
  const scheme = `${DEEP_LINK_SCHEME}://`;
  if (value.toLowerCase().startsWith(scheme))
    value = `/${value.slice(scheme.length)}`;
  const { path, query } = splitQuery(value);
  const segments = path.split("/").filter(Boolean);
  if (segments.length === 0) return { kind: RouteKind.Home };
  if (segments.length === 1 && segments[0] === "settings")
    return { kind: RouteKind.Settings };
  if (
    segments.length === 2 &&
    segments[0] === "connect" &&
    id.test(segments[1])
  )
    return { kind: RouteKind.Connect, enrollmentId: segments[1] };
  if (
    segments.length === 4 &&
    segments[0] === "spaces" &&
    segments[2] === "dashboards" &&
    id.test(segments[1]) &&
    id.test(segments[3])
  ) {
    const widget = query.widget;
    return {
      kind: RouteKind.Dashboard,
      spaceId: segments[1],
      dashboardId: segments[3],
      ...(widget && id.test(widget) ? { widgetId: widget } : {}),
    };
  }
  return { kind: RouteKind.NotFound };
}

export function routePath(route: Route): string {
  switch (route.kind) {
    case RouteKind.Dashboard:
      return `/spaces/${route.spaceId}/dashboards/${route.dashboardId}${
        route.widgetId ? `?widget=${route.widgetId}` : ""
      }`;
    case RouteKind.Connect:
      return `/connect/${route.enrollmentId}`;
    case RouteKind.Settings:
      return "/settings";
    default:
      return "/";
  }
}

/** The deep link that native widgets open. */
export function widgetDeepLink(
  spaceId: string,
  dashboardId: string,
  widgetId: string,
) {
  return `${DEEP_LINK_SCHEME}://spaces/${spaceId}/dashboards/${dashboardId}?widget=${widgetId}`;
}
