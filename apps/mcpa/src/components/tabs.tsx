// SPDX-License-Identifier: Apache-2.0
import { Link } from "../lib/router";

export function TabNav<T extends string>({
  label,
  tabs,
  current,
  href,
  names,
}: {
  label: string;
  tabs: T[];
  current: T;
  href: (tab: T) => string;
  names: Record<T, string>;
}) {
  return (
    <nav className="tabs" aria-label={label}>
      {tabs.map((tab) => (
        <Link
          key={tab}
          to={href(tab)}
          className="tab"
          aria-current={tab === current ? "page" : undefined}
        >
          {names[tab]}
        </Link>
      ))}
    </nav>
  );
}
