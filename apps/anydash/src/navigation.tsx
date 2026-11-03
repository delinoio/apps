// SPDX-License-Identifier: Apache-2.0
import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "@lynx-js/react";
import { hostCall, HostEvent, HostMethod, listenHost } from "./host";
import { parseRoute, routePath, type Route } from "./model/routes";

interface Navigation {
  route: Route;
  navigate(route: Route, options?: { replace?: boolean }): void;
}
const NavigationContext = createContext<Navigation | null>(null);
export function useNavigation() {
  const value = useContext(NavigationContext);
  if (!value) throw new Error("NavigationProvider is missing.");
  return value;
}

/**
 * The host owns platform history: browser history on the web, deep links on
 * native hosts. It emits HostEvent.Route with a path or `anydash://` link.
 */
export function NavigationProvider({
  initial,
  children,
}: {
  initial: string;
  children: ReactNode;
}) {
  const [route, setRoute] = useState<Route>(() => parseRoute(initial));
  useEffect(
    () =>
      listenHost(HostEvent.Route, (raw) => {
        const value = raw as { path?: unknown } | undefined;
        if (typeof value?.path === "string") setRoute(parseRoute(value.path));
      }),
    [],
  );
  const navigate: Navigation["navigate"] = (next, options = {}) => {
    setRoute(next);
    void hostCall(HostMethod.Route, {
      path: routePath(next),
      replace: options.replace === true,
    }).catch(() => undefined);
  };
  return (
    <NavigationContext.Provider value={{ route, navigate }}>
      {children}
    </NavigationContext.Provider>
  );
}
