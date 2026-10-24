// SPDX-License-Identifier: Apache-2.0
import { useLocation } from "@rspress/core/runtime";
import { forwardRef, type ComponentProps } from "react";
import { languageLink } from "../language-links";
import {
  Layout as BaseLayout,
  Link as BaseLink,
} from "@rspress/core/theme-original";
import { ServiceSwitcher } from "./service-switcher";

export * from "@rspress/core/theme-original";

export function Layout() {
  const { pathname } = useLocation();
  return <BaseLayout beforeSidebar={<ServiceSwitcher key={pathname} />} />;
}

export const Link = forwardRef<
  HTMLAnchorElement,
  ComponentProps<typeof BaseLink>
>((props, ref) => (
  <BaseLink
    {...props}
    {...languageLink(props.href, props.hrefLang, props.rel)}
    ref={ref}
  />
));
Link.displayName = "Link";
