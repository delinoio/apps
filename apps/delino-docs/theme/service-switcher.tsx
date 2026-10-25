// SPDX-License-Identifier: Apache-2.0
import { useLang, useLocation } from "@rspress/core/runtime";
import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import registry from "../i18n/locales.json";
import { servicesForLanguage } from "../services";
import "./service-switcher.css";

export function ServiceSwitcher() {
  const lang = useLang();
  const services = servicesForLanguage(lang);
  const { pathname } = useLocation();
  const prefix = lang === registry.sourceLang ? "" : `/${lang}`;
  const current = services.find(({ id }) =>
    pathname.startsWith(`${prefix}/services/${id}/`),
  );
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLElement>(null);
  const menuId = useId();
  const triggerId = useId();

  useEffect(() => {
    if (!open) return;

    menu.current?.querySelector<HTMLAnchorElement>("[aria-current]")?.focus();

    function onPointerDown(event: PointerEvent) {
      if (
        event.target instanceof Node &&
        !root.current?.contains(event.target)
      ) {
        setOpen(false);
      }
    }

    function onKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        trigger.current?.focus();
      }
    }

    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  function moveFocus(event: KeyboardEvent<HTMLElement>) {
    const links = Array.from(menu.current?.querySelectorAll("a") ?? []);
    const index = links.indexOf(document.activeElement as HTMLAnchorElement);
    let next: number;
    switch (event.key) {
      case "ArrowDown":
        next = (index + 1) % links.length;
        break;
      case "ArrowUp":
        next = (index - 1 + links.length) % links.length;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = links.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    links[next]?.focus();
  }

  if (!current) return null;

  return (
    <div
      className="service-switcher"
      ref={root}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
    >
      <button
        id={triggerId}
        ref={trigger}
        type="button"
        className="service-switcher__trigger"
        aria-expanded={open}
        aria-controls={menuId}
        onClick={() => setOpen(!open)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            setOpen(true);
          }
        }}
      >
        {current.name}
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
          <path d="m4 6 4 4 4-4" />
        </svg>
      </button>
      {open && (
        <nav
          id={menuId}
          ref={menu}
          className="service-switcher__menu"
          aria-labelledby={triggerId}
          onKeyDown={moveFocus}
        >
          {services.map(({ id, name }) => (
            <a
              key={id}
              href={`${prefix}/services/${id}/`}
              aria-current={id === current.id ? "true" : undefined}
            >
              {name}
              {id === current.id && (
                <svg
                  width="16"
                  height="16"
                  viewBox="0 0 16 16"
                  aria-hidden="true"
                >
                  <path d="m3 8 3 3 7-7" />
                </svg>
              )}
            </a>
          ))}
        </nav>
      )}
    </div>
  );
}
