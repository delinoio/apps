// SPDX-License-Identifier: Apache-2.0
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type AnchorHTMLAttributes,
  type ReactNode,
} from "react";
import { Section } from "./roles";

export enum RouteName {
  Home = "home",
  Tenant = "tenant",
  Resource = "resource",
  NotFound = "not-found",
}

export enum TenantTab {
  Resources = "resources",
  Usage = "usage",
  Errors = "errors",
  Budget = "budget",
}

export type Route =
  | { name: RouteName.Home }
  | { name: RouteName.Tenant; tenantId: string; tab: TenantTab }
  | {
      name: RouteName.Resource;
      tenantId: string;
      resourceId: string;
      section: Section;
    }
  | { name: RouteName.NotFound };

const sections = new Set<string>(Object.values(Section));
const tenantTabs = new Set<string>(Object.values(TenantTab));

export function parseRoute(pathname: string): Route {
  let parts: string[];
  try {
    parts = pathname.split("/").filter(Boolean).map(decodeURIComponent);
  } catch {
    // Malformed percent-encoding (for example "%" or "%E0%A4%A") throws a
    // URIError; the URL is user-controlled, so show the not-found page.
    return { name: RouteName.NotFound };
  }
  if (parts.length === 0) return { name: RouteName.Home };
  if (parts[0] !== "tenants" || !parts[1]) return { name: RouteName.NotFound };
  const tenantId = parts[1];
  if (parts.length <= 3 && parts[2] !== "resources") {
    const tab = parts[2] ?? TenantTab.Resources;
    return tenantTabs.has(tab)
      ? { name: RouteName.Tenant, tenantId, tab: tab as TenantTab }
      : { name: RouteName.NotFound };
  }
  if (parts[2] === "resources" && parts[3] && parts.length <= 5) {
    const section = parts[4] ?? Section.Endpoints;
    return sections.has(section)
      ? {
          name: RouteName.Resource,
          tenantId,
          resourceId: parts[3],
          section: section as Section,
        }
      : { name: RouteName.NotFound };
  }
  return { name: RouteName.NotFound };
}

export const paths = {
  home: () => "/",
  tenant: (tenantId: string, tab = TenantTab.Resources) =>
    tab === TenantTab.Resources
      ? `/tenants/${encodeURIComponent(tenantId)}`
      : `/tenants/${encodeURIComponent(tenantId)}/${tab}`,
  resource: (
    tenantId: string,
    resourceId: string,
    section = Section.Endpoints,
  ) =>
    `/tenants/${encodeURIComponent(tenantId)}/resources/${encodeURIComponent(resourceId)}${
      section === Section.Endpoints ? "" : `/${section}`
    }`,
};

type Navigate = (path: string, options?: { replace?: boolean }) => void;
const RouterContext = createContext<{ path: string; navigate: Navigate }>({
  path: "/",
  navigate: () => {},
});

export function RouterProvider({ children }: { children: ReactNode }) {
  const [path, setPath] = useState(() => window.location.pathname);
  useEffect(() => {
    const onPop = () => setPath(window.location.pathname);
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);
  const navigate = useCallback<Navigate>((next, options) => {
    if (options?.replace) window.history.replaceState(null, "", next);
    else window.history.pushState(null, "", next);
    setPath(new URL(next, window.location.origin).pathname);
    window.scrollTo?.(0, 0);
  }, []);
  const value = useMemo(() => ({ path, navigate }), [path, navigate]);
  return (
    <RouterContext.Provider value={value}>{children}</RouterContext.Provider>
  );
}

export function useRouter() {
  const { path, navigate } = useContext(RouterContext);
  const route = useMemo(() => parseRoute(path), [path]);
  return { route, path, navigate };
}

/** An in-app link that keeps normal browser behavior for modified clicks. */
export function Link({
  to,
  children,
  className,
  ...rest
}: { to: string; children: ReactNode; className?: string } & Omit<
  AnchorHTMLAttributes<HTMLAnchorElement>,
  "href"
>) {
  const { navigate } = useRouter();
  return (
    <a
      {...rest}
      href={to}
      className={className}
      onClick={(event) => {
        rest.onClick?.(event);
        if (
          event.defaultPrevented ||
          event.button !== 0 ||
          event.metaKey ||
          event.ctrlKey ||
          event.shiftKey ||
          event.altKey
        )
          return;
        event.preventDefault();
        navigate(to);
      }}
    >
      {children}
    </a>
  );
}
