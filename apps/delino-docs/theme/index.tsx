// SPDX-License-Identifier: Apache-2.0
import { useLocation } from "@rspress/core/runtime";
import { Layout as BaseLayout } from "@rspress/core/theme-original";
import { ServiceSwitcher } from "./service-switcher";

export * from "@rspress/core/theme-original";

export function Layout() {
  const { pathname } = useLocation();
  return <BaseLayout beforeSidebar={<ServiceSwitcher key={pathname} />} />;
}
